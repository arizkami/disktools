//! Raw physical-disk tests in the style of HD Tune: a transfer-rate sweep across
//! the whole surface with access-time samples, burst rate, and an error scan.
//!
//! Every test only *reads* from `\\.\PhysicalDriveN` with `FILE_FLAG_NO_BUFFERING`.
//! Opening a physical drive needs administrator rights.

use std::fs::{File, OpenOptions};
use std::os::windows::fs::{FileExt, OpenOptionsExt};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_NO_BUFFERING;

use crate::bench::{AlignedBuf, MIB, Rng};

const ALIGN: u64 = 4096;

fn align_down(v: u64) -> u64 {
    v / ALIGN * ALIGN
}

pub fn disk_path(index: u32) -> String {
    format!("\\\\.\\PhysicalDrive{index}")
}

fn open_disk(path: &str) -> Result<File, String> {
    OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_NO_BUFFERING)
        .open(path)
        .map_err(|e| match e.raw_os_error() {
            Some(5) => "Access denied — raw disk tests need administrator rights".to_string(),
            _ => format!("Cannot open {path}: {e}"),
        })
}

/// Reads exactly `buf.len()` bytes at `offset`.
fn read_at(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
    let mut done = 0;
    while done < buf.len() {
        let n = file.seek_read(&mut buf[done..], offset + done as u64)?;
        if n == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "end of disk"));
        }
        done += n;
    }
    Ok(())
}

/// Shared progress + cancellation for a background test.
pub struct Task<T> {
    pub state: Arc<Mutex<T>>,
    stop: Arc<AtomicBool>,
}

impl<T> Task<T> {
    pub fn cancel(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn spawn<T: Send + 'static>(
    initial: T,
    repaint: impl Fn() + Send + 'static,
    work: impl FnOnce(&Mutex<T>, &AtomicBool, &dyn Fn()) + Send + 'static,
) -> Task<T> {
    let state = Arc::new(Mutex::new(initial));
    let stop = Arc::new(AtomicBool::new(false));
    let task = Task { state: state.clone(), stop: stop.clone() };
    std::thread::Builder::new()
        .name("surface".into())
        .spawn(move || work(&state, &stop, &repaint))
        .expect("spawn surface thread");
    task
}

// ---------------------------------------------------------------------------
// Transfer rate

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Accuracy {
    Fast,
    Balanced,
    Accurate,
}

impl Accuracy {
    pub const ALL: [Accuracy; 3] = [Accuracy::Fast, Accuracy::Balanced, Accuracy::Accurate];

    pub fn label(self) -> &'static str {
        match self {
            Accuracy::Fast => "Fast",
            Accuracy::Balanced => "Balanced",
            Accuracy::Accurate => "Accurate",
        }
    }

    /// (sample points across the disk, bytes read per point)
    fn plan(self) -> (usize, u64) {
        match self {
            Accuracy::Fast => (100, 8 * MIB),
            Accuracy::Balanced => (200, 16 * MIB),
            Accuracy::Accurate => (400, 32 * MIB),
        }
    }
}

#[derive(Clone)]
pub struct TransferConfig {
    /// Device path, normally from [`disk_path`].
    pub path: String,
    pub disk_size: u64,
    pub accuracy: Accuracy,
    /// Size of each read request.
    pub block: u64,
}

#[derive(Default, Clone)]
pub struct TransferState {
    /// (position in bytes, MB/s)
    pub rate: Vec<(u64, f64)>,
    /// (position in bytes, milliseconds)
    pub access: Vec<(u64, f64)>,
    pub burst_mbps: Option<f64>,
    pub disk_size: u64,
    pub progress: f32,
    pub status: String,
    pub running: bool,
    pub error: Option<String>,
}

impl TransferState {
    pub fn min_max_avg(&self) -> Option<(f64, f64, f64)> {
        if self.rate.is_empty() {
            return None;
        }
        let (mut min, mut max, mut sum) = (f64::MAX, 0.0f64, 0.0);
        for &(_, r) in &self.rate {
            min = min.min(r);
            max = max.max(r);
            sum += r;
        }
        Some((min, max, sum / self.rate.len() as f64))
    }

    pub fn access_avg_ms(&self) -> Option<f64> {
        (!self.access.is_empty()).then(|| self.access.iter().map(|a| a.1).sum::<f64>() / self.access.len() as f64)
    }
}

pub fn start_transfer(cfg: TransferConfig, repaint: impl Fn() + Send + 'static) -> Task<TransferState> {
    let initial = TransferState {
        disk_size: cfg.disk_size,
        running: true,
        status: "Opening drive".into(),
        ..Default::default()
    };
    spawn(initial, repaint, move |state, stop, repaint| {
        let result = run_transfer(&cfg, state, stop, repaint);
        let mut s = state.lock().unwrap();
        s.running = false;
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
}

fn run_transfer(
    cfg: &TransferConfig,
    state: &Mutex<TransferState>,
    stop: &AtomicBool,
    repaint: &dyn Fn(),
) -> Result<(), String> {
    let file = open_disk(&cfg.path)?;
    let (points, per_point) = cfg.accuracy.plan();
    let block = cfg.block.min(per_point).max(ALIGN);
    let per_point = per_point / block * block;
    let size = align_down(cfg.disk_size);
    if size < per_point + ALIGN {
        return Err("Drive is too small for this test".into());
    }

    let mut buf = AlignedBuf::new(block as usize);
    let mut probe = AlignedBuf::new(ALIGN as usize);
    let mut rng = Rng::seeded(points as u64);

    // Burst rate: repeatedly read the same 512 KiB, which the drive serves from its cache.
    {
        state.lock().unwrap().status = "Burst rate".into();
        repaint();
        let burst = 512 * 1024u64;
        let mut bbuf = AlignedBuf::new(burst as usize);
        let read = |b: &mut AlignedBuf| read_at(&file, b.as_mut_slice(), 0).map_err(|e| format!("Read failed: {e}"));
        read(&mut bbuf)?; // warm the cache
        let rounds = 32;
        let t = Instant::now();
        for _ in 0..rounds {
            read(&mut bbuf)?;
        }
        let mbps = (burst * rounds) as f64 / t.elapsed().as_secs_f64().max(1e-9) / 1e6;
        state.lock().unwrap().burst_mbps = Some(mbps);
    }

    let span = size - per_point;
    let mut last_paint = Instant::now();
    for i in 0..points {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        let pos = align_down(span * i as u64 / (points as u64 - 1));

        let t = Instant::now();
        let mut off = pos;
        while off < pos + per_point {
            read_at(&file, buf.as_mut_slice(), off)
                .map_err(|e| format!("Read failed at {:.2} GB: {e}", off as f64 / 1e9))?;
            off += block;
        }
        let mbps = per_point as f64 / t.elapsed().as_secs_f64().max(1e-9) / 1e6;

        // One random 4 KiB read per point gives the access-time scatter.
        let target = align_down(rng.next() % (size - ALIGN));
        let t = Instant::now();
        let access_ok = read_at(&file, probe.as_mut_slice(), target).is_ok();
        let ms = t.elapsed().as_secs_f64() * 1000.0;

        let mut s = state.lock().unwrap();
        s.rate.push((pos, mbps));
        if access_ok {
            s.access.push((target, ms));
        }
        s.progress = (i + 1) as f32 / points as f32;
        s.status = format!("Reading {:.1} / {:.1} GB", pos as f64 / 1e9, size as f64 / 1e9);
        drop(s);
        if last_paint.elapsed() > Duration::from_millis(30) {
            repaint();
            last_paint = Instant::now();
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Error scan

pub const SCAN_CELLS: usize = 1000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cell {
    Pending,
    Ok,
    Bad,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScanMode {
    /// Reads the first 8 MiB of every block.
    Quick,
    /// Reads every byte of the drive.
    Full,
}

#[derive(Clone)]
pub struct ScanConfig {
    pub path: String,
    pub disk_size: u64,
    pub mode: ScanMode,
}

#[derive(Clone)]
pub struct ScanState {
    pub cells: Vec<Cell>,
    pub current: Option<usize>,
    pub cell_bytes: u64,
    pub bytes_read: u64,
    pub bytes_total: u64,
    pub bad: usize,
    pub elapsed: Duration,
    pub progress: f32,
    pub status: String,
    pub running: bool,
    pub error: Option<String>,
}

impl ScanState {
    pub fn empty() -> Self {
        Self {
            cells: vec![Cell::Pending; SCAN_CELLS],
            current: None,
            cell_bytes: 0,
            bytes_read: 0,
            bytes_total: 0,
            bad: 0,
            elapsed: Duration::ZERO,
            progress: 0.0,
            status: "Ready".into(),
            running: false,
            error: None,
        }
    }

    pub fn speed_mbps(&self) -> f64 {
        self.bytes_read as f64 / self.elapsed.as_secs_f64().max(1e-9) / 1e6
    }
}

pub fn start_scan(cfg: ScanConfig, repaint: impl Fn() + Send + 'static) -> Task<ScanState> {
    let initial = ScanState { running: true, status: "Opening drive".into(), ..ScanState::empty() };
    spawn(initial, repaint, move |state, stop, repaint| {
        let result = run_scan(&cfg, state, stop, repaint);
        let mut s = state.lock().unwrap();
        s.running = false;
        s.current = None;
        match result {
            Ok(()) if stop.load(Ordering::Relaxed) => s.status = "Stopped".into(),
            Ok(()) => {
                s.status = if s.bad == 0 { "Done — no damaged blocks".into() } else { format!("Done — {} damaged blocks", s.bad) };
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
}

fn run_scan(cfg: &ScanConfig, state: &Mutex<ScanState>, stop: &AtomicBool, repaint: &dyn Fn()) -> Result<(), String> {
    let file = open_disk(&cfg.path)?;
    let size = align_down(cfg.disk_size);
    let cell_bytes = align_down(size / SCAN_CELLS as u64);
    if cell_bytes == 0 {
        return Err("Drive is too small to scan".into());
    }
    let chunk = 4 * MIB;
    let per_cell = match cfg.mode {
        ScanMode::Quick => (8 * MIB).min(cell_bytes),
        ScanMode::Full => cell_bytes,
    };
    let total = match cfg.mode {
        ScanMode::Quick => per_cell * SCAN_CELLS as u64,
        // The last block also covers the remainder.
        ScanMode::Full => size,
    };
    {
        let mut s = state.lock().unwrap();
        s.cell_bytes = cell_bytes;
        s.bytes_total = total;
        s.status = "Scanning".into();
    }

    let mut buf = AlignedBuf::new(chunk as usize);
    let start = Instant::now();
    let mut last_paint = Instant::now();
    let mut read_total = 0u64;
    for cell in 0..SCAN_CELLS {
        let begin = cell as u64 * cell_bytes;
        let end = match (cfg.mode, cell == SCAN_CELLS - 1) {
            (ScanMode::Full, true) => size,
            _ => begin + per_cell,
        };
        state.lock().unwrap().current = Some(cell);

        let mut ok = true;
        let mut off = begin;
        while off < end {
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            let len = chunk.min(end - off) as usize;
            if read_at(&file, &mut buf.as_mut_slice()[..len], off).is_err() {
                ok = false;
            }
            off += len as u64;
            read_total += len as u64;

            if last_paint.elapsed() > Duration::from_millis(50) {
                let mut s = state.lock().unwrap();
                s.bytes_read = read_total;
                s.elapsed = start.elapsed();
                s.progress = read_total as f32 / total as f32;
                drop(s);
                repaint();
                last_paint = Instant::now();
            }
        }

        let mut s = state.lock().unwrap();
        s.cells[cell] = if ok { Cell::Ok } else { Cell::Bad };
        if !ok {
            s.bad += 1;
        }
        s.bytes_read = read_total;
        s.elapsed = start.elapsed();
        s.progress = read_total as f32 / total as f32;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_file(size: u64) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("disktools-surface-{}-{size}.bin", std::process::id()));
        let mut f = File::create(&path).unwrap();
        let chunk = vec![0xA5u8; MIB as usize];
        for _ in 0..size / MIB {
            f.write_all(&chunk).unwrap();
        }
        path
    }

    fn wait<T>(task: &Task<T>, running: impl Fn(&T) -> bool) {
        while running(&task.state.lock().unwrap()) {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn transfer_sweep_fills_graph() {
        let path = temp_file(64 * MIB);
        let cfg = TransferConfig {
            path: path.to_string_lossy().into(),
            disk_size: 64 * MIB,
            accuracy: Accuracy::Fast,
            block: MIB,
        };
        let task = start_transfer(cfg, || {});
        wait(&task, |s| s.running);
        let s = task.state.lock().unwrap().clone();
        let _ = std::fs::remove_file(path);
        assert!(s.error.is_none(), "{:?}", s.error);
        assert_eq!(s.rate.len(), 100);
        assert_eq!(s.access.len(), 100);
        assert!(s.burst_mbps.unwrap() > 0.0);
        let (min, max, avg) = s.min_max_avg().unwrap();
        assert!(min > 0.0 && max >= avg && avg >= min);
        assert!(s.rate.windows(2).all(|w| w[0].0 < w[1].0), "positions increase");
        assert!(s.rate.last().unwrap().0 + 8 * MIB <= 64 * MIB, "last sample stays inside the disk");
    }

    #[test]
    fn full_scan_marks_every_block() {
        let size = 32 * MIB;
        let path = temp_file(size);
        let cfg = ScanConfig { path: path.to_string_lossy().into(), disk_size: size, mode: ScanMode::Full };
        let task = start_scan(cfg, || {});
        wait(&task, |s| s.running);
        let s = task.state.lock().unwrap().clone();
        let _ = std::fs::remove_file(path);
        assert!(s.error.is_none(), "{:?}", s.error);
        assert!(s.cells.iter().all(|c| *c == Cell::Ok));
        assert_eq!(s.bad, 0);
        assert_eq!(s.bytes_read, size);
    }
}
