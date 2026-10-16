//! Disk benchmark engine.
//!
//! Every test opens the test file with `FILE_FLAG_NO_BUFFERING` so the OS cache
//! is bypassed, and keeps `queue` overlapped requests in flight per thread using
//! an I/O completion port — the same model CrystalDiskMark/DiskSpd use.

use std::alloc::{Layout, alloc, dealloc};
use std::fs::OpenOptions;
use std::io::Write;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_IO_PENDING, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_NO_BUFFERING, FILE_FLAG_OVERLAPPED, FILE_FLAG_WRITE_THROUGH, ReadFile, WriteFile,
};
use windows_sys::Win32::System::IO::{
    CreateIoCompletionPort, GetQueuedCompletionStatus, OVERLAPPED,
};

const ALIGN: usize = 4096;
pub const KIB: u32 = 1024;
pub const MIB: u64 = 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Access {
    Seq,
    Rnd,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TestSpec {
    pub access: Access,
    pub block: u32,
    pub queue: u32,
    pub threads: u32,
}

impl TestSpec {
    const fn new(access: Access, block: u32, queue: u32, threads: u32) -> Self {
        Self { access, block, queue, threads }
    }

    pub fn name(&self) -> String {
        let kind = match self.access {
            Access::Seq => "SEQ",
            Access::Rnd => "RND",
        };
        let size = if self.block >= 1024 * KIB {
            format!("{}M", self.block / (1024 * KIB))
        } else {
            format!("{}K", self.block / KIB)
        };
        format!("{kind}{size}")
    }

    pub fn qt(&self) -> String {
        format!("Q{}T{}", self.queue, self.threads)
    }

    /// Spaced-out labels for the UI: ("SEQ 1M", "Q8 · T1").
    pub fn display(&self) -> (String, String) {
        let name = self.name();
        (format!("{} {}", &name[..3], &name[3..]), format!("Q{} · T{}", self.queue, self.threads))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Profile {
    Default,
    NvmeSsd,
}

impl Profile {
    pub const ALL: [Profile; 2] = [Profile::Default, Profile::NvmeSsd];

    pub fn label(&self) -> &'static str {
        match self {
            Profile::Default => "Default",
            Profile::NvmeSsd => "NVMe SSD",
        }
    }

    pub fn tests(&self) -> [TestSpec; 4] {
        match self {
            Profile::Default => [
                TestSpec::new(Access::Seq, 1024 * KIB, 8, 1),
                TestSpec::new(Access::Seq, 1024 * KIB, 1, 1),
                TestSpec::new(Access::Rnd, 4 * KIB, 32, 1),
                TestSpec::new(Access::Rnd, 4 * KIB, 1, 1),
            ],
            Profile::NvmeSsd => [
                TestSpec::new(Access::Seq, 1024 * KIB, 8, 1),
                TestSpec::new(Access::Seq, 128 * KIB, 32, 1),
                TestSpec::new(Access::Rnd, 4 * KIB, 32, 16),
                TestSpec::new(Access::Rnd, 4 * KIB, 1, 1),
            ],
        }
    }
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Score {
    /// Decimal megabytes per second (1 MB = 1,000,000 bytes), like CrystalDiskMark.
    pub mbps: f64,
    pub iops: f64,
    /// Mean completion latency in microseconds.
    pub latency_us: f64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    ReadWrite,
    ReadOnly,
    WriteOnly,
}

impl Mode {
    pub fn reads(self) -> bool {
        self != Mode::WriteOnly
    }
    pub fn writes(self) -> bool {
        self != Mode::ReadOnly
    }
}

#[derive(Clone)]
pub struct BenchConfig {
    pub drive_root: PathBuf,
    pub file_size: u64,
    pub runs: u32,
    pub duration: Duration,
    pub interval: Duration,
    pub tests: [TestSpec; 4],
    /// Which of the four rows to run.
    pub rows: [bool; 4],
    pub mode: Mode,
}

/// State shared between the worker thread and the UI.
#[derive(Default)]
pub struct Shared {
    pub status: String,
    pub progress: f32,
    /// `[row][0 = read, 1 = write]`
    pub results: [[Option<Score>; 2]; 4],
    /// Cell currently being measured.
    pub active: Option<(usize, usize)>,
    pub running: bool,
    pub error: Option<String>,
}

pub struct BenchHandle {
    pub shared: Arc<Mutex<Shared>>,
    pub stop: Arc<AtomicBool>,
}

impl BenchHandle {
    pub fn cancel(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub fn start(cfg: BenchConfig, repaint: impl Fn() + Send + 'static) -> BenchHandle {
    let shared = Arc::new(Mutex::new(Shared { running: true, ..Default::default() }));
    let stop = Arc::new(AtomicBool::new(false));
    let handle = BenchHandle { shared: shared.clone(), stop: stop.clone() };

    std::thread::Builder::new()
        .name("bench".into())
        .spawn(move || {
            let result = run_all(&cfg, &shared, &stop, &repaint);
            let mut s = shared.lock().unwrap();
            s.running = false;
            s.active = None;
            match result {
                Ok(()) if stop.load(Ordering::Relaxed) => s.status = "Stopped".into(),
                Ok(()) => {
                    s.status = "Done".into();
                    s.progress = 1.0;
                }
                Err(e) => {
                    s.status = "Error".into();
                    s.error = Some(e);
                }
            }
            drop(s);
            repaint();
        })
        .expect("spawn bench thread");

    handle
}

fn run_all(
    cfg: &BenchConfig,
    shared: &Mutex<Shared>,
    stop: &AtomicBool,
    repaint: &dyn Fn(),
) -> Result<(), String> {
    let set_status = |status: String, progress: f32| {
        let mut s = shared.lock().unwrap();
        s.status = status;
        s.progress = progress;
        drop(s);
        repaint();
    };

    let dir = cfg.drive_root.join("DiskToolsBench.tmp");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
    let path = dir.join("testfile.bin");
    let _cleanup = Cleanup { dir: dir.clone(), file: path.clone() };

    // Build the list of measurements up-front so progress is meaningful.
    let mut jobs = Vec::new();
    for (row, spec) in cfg.tests.iter().enumerate() {
        if !cfg.rows[row] {
            continue;
        }
        if cfg.mode.reads() {
            jobs.push((row, 0usize, *spec));
        }
    }
    for (row, spec) in cfg.tests.iter().enumerate() {
        if !cfg.rows[row] {
            continue;
        }
        if cfg.mode.writes() {
            jobs.push((row, 1usize, *spec));
        }
    }
    if jobs.is_empty() {
        return Ok(());
    }

    {
        let mut s = shared.lock().unwrap();
        for &(row, col, _) in &jobs {
            s.results[row][col] = None;
        }
    }

    prepare_file(&path, cfg.file_size, stop, &|p| {
        set_status(format!("Preparing test file ({:.0}%)", p * 100.0), p * 0.1)
    })?;

    let total = (jobs.len() as u32 * cfg.runs) as f32;
    let mut done = 0u32;
    for (i, &(row, col, spec)) in jobs.iter().enumerate() {
        if i > 0 && !cfg.interval.is_zero() {
            set_status("Interval...".into(), 0.1 + 0.9 * done as f32 / total);
            sleep_cancellable(cfg.interval, stop);
        }
        let mut best: Option<Score> = None;
        for run in 0..cfg.runs {
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            {
                let mut s = shared.lock().unwrap();
                s.active = Some((row, col));
                s.status = format!(
                    "{} {} {} [{}/{}]",
                    spec.name(),
                    spec.qt(),
                    if col == 0 { "Read" } else { "Write" },
                    run + 1,
                    cfg.runs
                );
                s.progress = 0.1 + 0.9 * done as f32 / total;
            }
            repaint();

            let score = run_test(&path, cfg.file_size, spec, col == 1, cfg.duration, stop)?;
            if best.is_none_or(|b| score.mbps > b.mbps) {
                best = Some(score);
            }
            done += 1;
            {
                let mut s = shared.lock().unwrap();
                s.results[row][col] = best;
            }
            repaint();
        }
    }
    Ok(())
}

struct Cleanup {
    dir: PathBuf,
    file: PathBuf,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.file);
        let _ = std::fs::remove_dir(&self.dir);
    }
}

fn sleep_cancellable(d: Duration, stop: &AtomicBool) {
    let end = Instant::now() + d;
    while Instant::now() < end && !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Page-aligned heap buffer, required for unbuffered I/O.
pub(crate) struct AlignedBuf {
    ptr: *mut u8,
    layout: Layout,
}

unsafe impl Send for AlignedBuf {}

impl AlignedBuf {
    pub(crate) fn new(len: usize) -> Self {
        let layout = Layout::from_size_align(len.max(ALIGN), ALIGN).unwrap();
        let ptr = unsafe { alloc(layout) };
        assert!(!ptr.is_null(), "out of memory");
        Self { ptr, layout }
    }

    pub(crate) fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.layout.size()) }
    }

    fn fill_random(&mut self, rng: &mut Rng) {
        for chunk in self.as_mut_slice().as_chunks_mut::<8>().0 {
            *chunk = rng.next().to_le_bytes();
        }
    }
}

impl Drop for AlignedBuf {
    fn drop(&mut self) {
        unsafe { dealloc(self.ptr, self.layout) }
    }
}

/// xorshift64* — fast, good enough for offsets and incompressible data.
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn seeded(salt: u64) -> Self {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        Rng((t ^ salt.wrapping_mul(0x2545_F491_4F6C_DD1D)) | 1)
    }

    pub(crate) fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

fn prepare_file(
    path: &PathBuf,
    size: u64,
    stop: &AtomicBool,
    progress: &dyn Fn(f32),
) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .custom_flags(FILE_FLAG_NO_BUFFERING | FILE_FLAG_WRITE_THROUGH)
        .open(path)
        .map_err(|e| format!("Cannot create test file {}: {e}", path.display()))?;
    file.set_len(size).map_err(|e| format!("Cannot size test file: {e}"))?;

    let chunk = (8 * MIB).min(size) as usize;
    let mut buf = AlignedBuf::new(chunk);
    let mut rng = Rng::seeded(0);
    let mut written = 0u64;
    let mut last = Instant::now();
    while written < size {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        buf.fill_random(&mut rng);
        let n = chunk.min((size - written) as usize);
        file.write_all(&buf.as_mut_slice()[..n])
            .map_err(|e| format!("Writing test file failed: {e}"))?;
        written += n as u64;
        if last.elapsed() > Duration::from_millis(100) {
            progress(written as f32 / size as f32);
            last = Instant::now();
        }
    }
    file.sync_all().map_err(|e| format!("Flushing test file failed: {e}"))?;
    Ok(())
}

#[derive(Default)]
struct ThreadStats {
    bytes: u64,
    ios: u64,
    latency_sum: Duration,
    elapsed: Duration,
}

/// One in-flight request. `ov` must stay the first field so the `OVERLAPPED*`
/// returned by the completion port can be cast back to the slot.
#[repr(C)]
struct Slot {
    ov: OVERLAPPED,
    buf: AlignedBuf,
    issued: Instant,
}

fn run_test(
    path: &PathBuf,
    file_size: u64,
    spec: TestSpec,
    write: bool,
    duration: Duration,
    stop: &AtomicBool,
) -> Result<Score, String> {
    let threads = spec.threads.max(1) as usize;
    let barrier = Arc::new(Barrier::new(threads));

    let results: Vec<Result<ThreadStats, String>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|t| {
                let barrier = barrier.clone();
                scope.spawn(move || {
                    io_worker(path, file_size, spec, write, duration, stop, t, threads, &barrier)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    let mut total = ThreadStats::default();
    for r in results {
        let s = r?;
        total.bytes += s.bytes;
        total.ios += s.ios;
        total.latency_sum += s.latency_sum;
        total.elapsed = total.elapsed.max(s.elapsed);
    }
    let secs = total.elapsed.as_secs_f64().max(1e-9);
    Ok(Score {
        mbps: total.bytes as f64 / secs / 1e6,
        iops: total.ios as f64 / secs,
        latency_us: if total.ios > 0 {
            total.latency_sum.as_secs_f64() * 1e6 / total.ios as f64
        } else {
            0.0
        },
    })
}

#[allow(clippy::too_many_arguments)]
fn io_worker(
    path: &PathBuf,
    file_size: u64,
    spec: TestSpec,
    write: bool,
    duration: Duration,
    stop: &AtomicBool,
    thread_idx: usize,
    threads: usize,
    barrier: &Barrier,
) -> Result<ThreadStats, String> {
    let block = spec.block as u64;
    let blocks = (file_size / block).max(1);

    let mut flags = FILE_FLAG_NO_BUFFERING | FILE_FLAG_OVERLAPPED;
    if write {
        flags |= FILE_FLAG_WRITE_THROUGH;
    }
    let opened = OpenOptions::new().read(true).write(true).custom_flags(flags).open(path);
    let file = match opened {
        Ok(f) => f,
        Err(e) => {
            barrier.wait();
            return Err(format!("Cannot open test file: {e}"));
        }
    };
    let handle = file.as_raw_handle() as HANDLE;

    let port = unsafe { CreateIoCompletionPort(handle, std::ptr::null_mut(), 0, 1) };
    if port.is_null() || port == INVALID_HANDLE_VALUE {
        barrier.wait();
        return Err("CreateIoCompletionPort failed".into());
    }
    let _port_guard = HandleGuard(port);

    let mut rng = Rng::seeded(thread_idx as u64 + 1);
    // Sequential threads each start in their own region of the file.
    let mut seq_next = (blocks / threads as u64) * thread_idx as u64;
    let mut next_offset = |rng: &mut Rng| -> u64 {
        match spec.access {
            Access::Seq => {
                let off = seq_next * block;
                seq_next = (seq_next + 1) % blocks;
                off
            }
            Access::Rnd => (rng.next() % blocks) * block,
        }
    };

    let mut slots: Vec<Box<Slot>> = (0..spec.queue.max(1))
        .map(|_| {
            let mut buf = AlignedBuf::new(spec.block as usize);
            buf.fill_random(&mut rng);
            Box::new(Slot { ov: unsafe { std::mem::zeroed() }, buf, issued: Instant::now() })
        })
        .collect();

    let issue = |slot: &mut Slot, offset: u64| -> Result<(), String> {
        slot.ov = unsafe { std::mem::zeroed() };
        slot.ov.Anonymous.Anonymous.Offset = offset as u32;
        slot.ov.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;
        slot.issued = Instant::now();
        let ok = unsafe {
            if write {
                WriteFile(handle, slot.buf.ptr, spec.block, std::ptr::null_mut(), &mut slot.ov)
            } else {
                ReadFile(handle, slot.buf.ptr, spec.block, std::ptr::null_mut(), &mut slot.ov)
            }
        };
        // A synchronous success still posts a completion packet, so both paths are "in flight".
        if ok == 0 {
            let err = unsafe { GetLastError() };
            if err != ERROR_IO_PENDING {
                return Err(format!("I/O submit failed (Win32 error {err})"));
            }
        }
        Ok(())
    };

    barrier.wait();

    let mut stats = ThreadStats::default();
    let mut outstanding = 0usize;
    let start = Instant::now();
    let deadline = start + duration;
    let mut failure = None;

    for slot in slots.iter_mut() {
        let off = next_offset(&mut rng);
        match issue(slot, off) {
            Ok(()) => outstanding += 1,
            Err(e) => {
                failure = Some(e);
                break;
            }
        }
    }

    while failure.is_none() && outstanding > 0 {
        let mut bytes = 0u32;
        let mut key = 0usize;
        let mut ov: *mut OVERLAPPED = std::ptr::null_mut();
        let ok = unsafe { GetQueuedCompletionStatus(port, &mut bytes, &mut key, &mut ov, 1000) };
        if ov.is_null() {
            // Timeout without a completion; keep waiting unless asked to stop.
            if stop.load(Ordering::Relaxed) {
                break;
            }
            continue;
        }
        outstanding -= 1;
        let slot = unsafe { &mut *(ov as *mut Slot) };
        if ok == 0 {
            let err = unsafe { GetLastError() };
            failure = Some(format!("I/O failed (Win32 error {err})"));
            break;
        }
        let now = Instant::now();
        stats.bytes += bytes as u64;
        stats.ios += 1;
        stats.latency_sum += now - slot.issued;

        if now < deadline && !stop.load(Ordering::Relaxed) {
            let off = next_offset(&mut rng);
            match issue(slot, off) {
                Ok(()) => outstanding += 1,
                Err(e) => failure = Some(e),
            }
        }
    }
    stats.elapsed = start.elapsed();

    // Drain anything still in flight before buffers are freed.
    while outstanding > 0 {
        let mut bytes = 0u32;
        let mut key = 0usize;
        let mut ov: *mut OVERLAPPED = std::ptr::null_mut();
        unsafe { GetQueuedCompletionStatus(port, &mut bytes, &mut key, &mut ov, 30_000) };
        if ov.is_null() {
            // The device stopped responding; leak the buffers rather than free memory
            // the kernel may still write into.
            std::mem::forget(slots);
            return Err("Timed out waiting for outstanding I/O".into());
        }
        outstanding -= 1;
    }

    drop(slots);
    drop(file);
    match failure {
        Some(e) => Err(e),
        None => Ok(stats),
    }
}

struct HandleGuard(HANDLE);

impl Drop for HandleGuard {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_benchmark_produces_scores() {
        let root = std::env::temp_dir().join(format!("disktools-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let cfg = BenchConfig {
            drive_root: root.clone(),
            file_size: 16 * MIB,
            runs: 1,
            duration: Duration::from_millis(300),
            interval: Duration::ZERO,
            tests: Profile::NvmeSsd.tests(),
            rows: [true; 4],
            mode: Mode::ReadWrite,
        };
        let handle = start(cfg, || {});
        while handle.shared.lock().unwrap().running {
            std::thread::sleep(Duration::from_millis(50));
        }
        let s = handle.shared.lock().unwrap();
        assert!(s.error.is_none(), "{:?}", s.error);
        for row in s.results {
            for cell in row {
                let score = cell.expect("missing score");
                assert!(score.mbps > 0.0 && score.iops > 0.0 && score.latency_us > 0.0, "{score:?}");
            }
        }
        drop(s);
        assert!(!root.join("DiskToolsBench.tmp").exists(), "test file not cleaned up");
        let _ = std::fs::remove_dir_all(root);
    }
}
