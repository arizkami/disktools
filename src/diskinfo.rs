//! Physical drive enumeration and S.M.A.R.T. / NVMe health reading.
//!
//! Identity data (model, serial, bus) is readable by any user. Health data
//! needs an elevated process because the drive must be opened read/write.

use std::collections::BTreeMap;

use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, GetLogicalDrives, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::{
    IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, IOCTL_STORAGE_GET_DEVICE_NUMBER,
    IOCTL_STORAGE_QUERY_PROPERTY, SMART_RCV_DRIVE_DATA,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HealthStatus {
    Good,
    Caution,
    Bad,
    Unknown,
}

impl HealthStatus {
    pub fn label(self) -> &'static str {
        match self {
            HealthStatus::Good => "Good",
            HealthStatus::Caution => "Caution",
            HealthStatus::Bad => "Bad",
            HealthStatus::Unknown => "Unknown",
        }
    }
}

#[derive(Clone, Debug)]
pub struct AtaAttribute {
    pub id: u8,
    pub current: u8,
    pub worst: u8,
    pub threshold: u8,
    pub raw: u64,
}

impl AtaAttribute {
    pub fn name(&self) -> &'static str {
        ata_attribute_name(self.id)
    }

    pub fn failing(&self) -> bool {
        self.threshold > 0 && self.current <= self.threshold
    }
}

#[derive(Clone, Debug, Default)]
pub struct NvmeHealth {
    pub critical_warning: u8,
    pub temperature_k: u16,
    pub available_spare: u8,
    pub spare_threshold: u8,
    pub percentage_used: u8,
    pub data_units_read: u128,
    pub data_units_written: u128,
    pub host_read_commands: u128,
    pub host_write_commands: u128,
    pub controller_busy_minutes: u128,
    pub power_cycles: u128,
    pub power_on_hours: u128,
    pub unsafe_shutdowns: u128,
    pub media_errors: u128,
    pub error_log_entries: u128,
    pub warning_temp_minutes: u32,
    pub critical_temp_minutes: u32,
}

impl NvmeHealth {
    /// Rows shown in the attribute table: (id, name, raw value as text).
    pub fn rows(&self) -> Vec<(u8, &'static str, String)> {
        let t = |k: u16| if k > 0 { format!("{} °C", k as i32 - 273) } else { "-".into() };
        vec![
            (0x01, "Critical Warning", format!("0x{:02X}", self.critical_warning)),
            (0x02, "Composite Temperature", t(self.temperature_k)),
            (0x03, "Available Spare", format!("{}%", self.available_spare)),
            (0x04, "Available Spare Threshold", format!("{}%", self.spare_threshold)),
            (0x05, "Percentage Used", format!("{}%", self.percentage_used)),
            (0x06, "Data Units Read", format!("{} ({})", self.data_units_read, fmt_bytes_u128(self.data_units_read * 512_000))),
            (0x07, "Data Units Written", format!("{} ({})", self.data_units_written, fmt_bytes_u128(self.data_units_written * 512_000))),
            (0x08, "Host Read Commands", self.host_read_commands.to_string()),
            (0x09, "Host Write Commands", self.host_write_commands.to_string()),
            (0x0A, "Controller Busy Time (min)", self.controller_busy_minutes.to_string()),
            (0x0B, "Power Cycles", self.power_cycles.to_string()),
            (0x0C, "Power On Hours", self.power_on_hours.to_string()),
            (0x0D, "Unsafe Shutdowns", self.unsafe_shutdowns.to_string()),
            (0x0E, "Media and Data Integrity Errors", self.media_errors.to_string()),
            (0x0F, "Number of Error Information Log Entries", self.error_log_entries.to_string()),
            (0x10, "Warning Composite Temperature Time (min)", self.warning_temp_minutes.to_string()),
            (0x11, "Critical Composite Temperature Time (min)", self.critical_temp_minutes.to_string()),
        ]
    }
}

#[derive(Clone, Debug)]
pub enum Smart {
    None,
    Ata(Vec<AtaAttribute>),
    Nvme(NvmeHealth),
}

#[derive(Clone, Debug)]
pub struct DiskInfo {
    pub index: u32,
    pub model: String,
    pub vendor: String,
    pub firmware: String,
    pub serial: String,
    pub bus: &'static str,
    pub size_bytes: u64,
    pub removable: bool,
    pub ssd: Option<bool>,
    pub trim: Option<bool>,
    pub letters: Vec<char>,
    pub health: HealthStatus,
    /// Remaining life estimate in percent, when the drive reports one.
    pub life_percent: Option<u8>,
    pub temperature_c: Option<i32>,
    pub power_on_hours: Option<u64>,
    pub power_cycles: Option<u64>,
    pub host_reads: Option<u128>,
    pub host_writes: Option<u128>,
    pub smart: Smart,
    pub smart_note: Option<String>,
}

impl DiskInfo {
    pub fn display_name(&self) -> String {
        let model = if self.model.is_empty() { "Unknown drive" } else { &self.model };
        if self.vendor.is_empty() || model.starts_with(&self.vendor) {
            model.to_string()
        } else {
            format!("{} {}", self.vendor, model)
        }
    }
}

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn open_device(path: &str, access: u32) -> Option<Handle> {
    let w = wide(path);
    let h = unsafe {
        CreateFileW(
            w.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE || h.is_null() { None } else { Some(Handle(h)) }
}

/// `DeviceIoControl` that returns the bytes written into `out`.
fn ioctl(h: &Handle, code: u32, input: &[u8], out: &mut [u8]) -> Option<usize> {
    let mut returned = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            h.0,
            code,
            input.as_ptr().cast(),
            input.len() as u32,
            out.as_mut_ptr().cast(),
            out.len() as u32,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(returned as usize)
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(b[o..o + 2].try_into().unwrap())
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
fn u128_at(b: &[u8], o: usize) -> u128 {
    u128::from_le_bytes(b[o..o + 16].try_into().unwrap())
}

fn c_string_at(b: &[u8], offset: u32) -> String {
    let o = offset as usize;
    if o == 0 || o >= b.len() {
        return String::new();
    }
    let end = b[o..].iter().position(|&c| c == 0).map_or(b.len(), |p| o + p);
    String::from_utf8_lossy(&b[o..end]).split_whitespace().collect::<Vec<_>>().join(" ")
}

// STORAGE_PROPERTY_ID values
const STORAGE_DEVICE_PROPERTY: u32 = 0;
const STORAGE_DEVICE_SEEK_PENALTY_PROPERTY: u32 = 7;
const STORAGE_DEVICE_TRIM_PROPERTY: u32 = 8;
const STORAGE_ADAPTER_PROTOCOL_SPECIFIC_PROPERTY: u32 = 49;
const STORAGE_DEVICE_PROTOCOL_SPECIFIC_PROPERTY: u32 = 50;

fn property_query(id: u32) -> [u8; 12] {
    // STORAGE_PROPERTY_QUERY { PropertyId, QueryType = PropertyStandardQuery, AdditionalParameters }
    let mut q = [0u8; 12];
    q[..4].copy_from_slice(&id.to_le_bytes());
    q
}

fn bus_name(bus: u32) -> &'static str {
    match bus {
        0x01 => "SCSI",
        0x02 => "ATAPI",
        0x03 => "ATA",
        0x04 => "IEEE 1394",
        0x05 => "SSA",
        0x06 => "Fibre Channel",
        0x07 => "USB",
        0x08 => "RAID",
        0x09 => "iSCSI",
        0x0A => "SAS",
        0x0B => "SATA",
        0x0C => "SD",
        0x0D => "MMC",
        0x0E => "Virtual",
        0x0F => "File Backed Virtual",
        0x10 => "Storage Spaces",
        0x11 => "NVMe",
        0x12 => "SCM",
        0x13 => "UFS",
        _ => "Unknown",
    }
}

/// Maps physical disk number -> drive letters on it.
fn drive_letters() -> BTreeMap<u32, Vec<char>> {
    let mut map: BTreeMap<u32, Vec<char>> = BTreeMap::new();
    let mask = unsafe { GetLogicalDrives() };
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let Some(h) = open_device(&format!("\\\\.\\{letter}:"), 0) else { continue };
        let mut out = [0u8; 12]; // STORAGE_DEVICE_NUMBER
        if ioctl(&h, IOCTL_STORAGE_GET_DEVICE_NUMBER, &[], &mut out).is_some() {
            map.entry(u32_at(&out, 4)).or_default().push(letter);
        }
    }
    map
}

pub fn enumerate() -> Vec<DiskInfo> {
    let letters = drive_letters();
    let mut disks = Vec::new();
    for index in 0..32u32 {
        let path = format!("\\\\.\\PhysicalDrive{index}");
        let Some(query) = open_device(&path, 0) else { continue };
        // Read/write access is needed for SMART; only succeeds when elevated.
        let rw = open_device(&path, GENERIC_READ | GENERIC_WRITE);

        let mut disk = DiskInfo {
            index,
            model: String::new(),
            vendor: String::new(),
            firmware: String::new(),
            serial: String::new(),
            bus: "Unknown",
            size_bytes: 0,
            removable: false,
            ssd: None,
            trim: None,
            letters: letters.get(&index).cloned().unwrap_or_default(),
            health: HealthStatus::Unknown,
            life_percent: None,
            temperature_c: None,
            power_on_hours: None,
            power_cycles: None,
            host_reads: None,
            host_writes: None,
            smart: Smart::None,
            smart_note: None,
        };
        let mut bus_code = 0;

        let mut buf = vec![0u8; 4096];
        if ioctl(&query, IOCTL_STORAGE_QUERY_PROPERTY, &property_query(STORAGE_DEVICE_PROPERTY), &mut buf)
            .is_some_and(|n| n >= 36)
        {
            // STORAGE_DEVICE_DESCRIPTOR
            disk.removable = buf[10] != 0;
            disk.vendor = c_string_at(&buf, u32_at(&buf, 12));
            disk.model = c_string_at(&buf, u32_at(&buf, 16));
            disk.firmware = c_string_at(&buf, u32_at(&buf, 20));
            disk.serial = c_string_at(&buf, u32_at(&buf, 24));
            bus_code = u32_at(&buf, 28);
            disk.bus = bus_name(bus_code);
        }

        let mut out = [0u8; 12];
        if ioctl(&query, IOCTL_STORAGE_QUERY_PROPERTY, &property_query(STORAGE_DEVICE_SEEK_PENALTY_PROPERTY), &mut out)
            .is_some()
        {
            disk.ssd = Some(out[8] == 0);
        }
        let mut out = [0u8; 12];
        if ioctl(&query, IOCTL_STORAGE_QUERY_PROPERTY, &property_query(STORAGE_DEVICE_TRIM_PROPERTY), &mut out)
            .is_some()
        {
            disk.trim = Some(out[8] != 0);
        }

        let mut geo = [0u8; 256];
        if ioctl(&query, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, &[], &mut geo).is_some_and(|n| n >= 32) {
            disk.size_bytes = u64_at(&geo, 24);
        }

        match &rw {
            None => disk.smart_note = Some("Run as administrator to read health data".into()),
            Some(rw) => {
                if bus_code == 0x11 {
                    match read_nvme_health(rw) {
                        Some(h) => apply_nvme(&mut disk, h),
                        None => disk.smart_note = Some("NVMe health log not available".into()),
                    }
                } else {
                    match read_ata_smart(rw) {
                        Some(attrs) => apply_ata(&mut disk, attrs),
                        None => {
                            // Some NVMe drives behind RAID/USB bridges report another bus.
                            match read_nvme_health(rw) {
                                Some(h) => apply_nvme(&mut disk, h),
                                None => disk.smart_note = Some("S.M.A.R.T. not supported on this interface".into()),
                            }
                        }
                    }
                }
            }
        }

        disks.push(disk);
    }
    disks
}

fn read_nvme_health(h: &Handle) -> Option<NvmeHealth> {
    const HEADER: usize = 8; // STORAGE_PROPERTY_QUERY up to AdditionalParameters
    const PROTO: usize = 40; // STORAGE_PROTOCOL_SPECIFIC_DATA
    const LOG_LEN: usize = 512;

    for property in [STORAGE_DEVICE_PROTOCOL_SPECIFIC_PROPERTY, STORAGE_ADAPTER_PROTOCOL_SPECIFIC_PROPERTY] {
        let mut buf = vec![0u8; HEADER + PROTO + LOG_LEN];
        buf[0..4].copy_from_slice(&property.to_le_bytes());
        // QueryType = PropertyStandardQuery (0)
        let p = HEADER;
        buf[p..p + 4].copy_from_slice(&3u32.to_le_bytes()); // ProtocolTypeNvme
        buf[p + 4..p + 8].copy_from_slice(&2u32.to_le_bytes()); // NVMeDataTypeLogPage
        buf[p + 8..p + 12].copy_from_slice(&2u32.to_le_bytes()); // NVME_LOG_PAGE_HEALTH_INFO
        buf[p + 16..p + 20].copy_from_slice(&(PROTO as u32).to_le_bytes()); // ProtocolDataOffset
        buf[p + 20..p + 24].copy_from_slice(&(LOG_LEN as u32).to_le_bytes()); // ProtocolDataLength

        let input = buf.clone();
        if ioctl(h, IOCTL_STORAGE_QUERY_PROPERTY, &input, &mut buf).is_none() {
            continue;
        }
        // STORAGE_PROTOCOL_DATA_DESCRIPTOR { Version, Size, ProtocolSpecificData }
        let offset = u32_at(&buf, 8 + 16) as usize;
        let len = u32_at(&buf, 8 + 20) as usize;
        let start = 8 + offset;
        if len < LOG_LEN || start + LOG_LEN > buf.len() {
            continue;
        }
        let l = &buf[start..start + LOG_LEN];
        return Some(NvmeHealth {
            critical_warning: l[0],
            temperature_k: u16_at(l, 1),
            available_spare: l[3],
            spare_threshold: l[4],
            percentage_used: l[5],
            data_units_read: u128_at(l, 32),
            data_units_written: u128_at(l, 48),
            host_read_commands: u128_at(l, 64),
            host_write_commands: u128_at(l, 80),
            controller_busy_minutes: u128_at(l, 96),
            power_cycles: u128_at(l, 112),
            power_on_hours: u128_at(l, 128),
            unsafe_shutdowns: u128_at(l, 144),
            media_errors: u128_at(l, 160),
            error_log_entries: u128_at(l, 176),
            warning_temp_minutes: u32_at(l, 192),
            critical_temp_minutes: u32_at(l, 196),
        });
    }
    None
}

fn apply_nvme(disk: &mut DiskInfo, h: NvmeHealth) {
    let mut status = HealthStatus::Good;
    if h.critical_warning != 0 || (h.spare_threshold > 0 && h.available_spare < h.spare_threshold) {
        status = HealthStatus::Bad;
    } else if h.media_errors > 0 || h.percentage_used >= 90 {
        status = HealthStatus::Caution;
    }
    disk.health = status;
    disk.life_percent = Some(100u8.saturating_sub(h.percentage_used));
    if h.temperature_k > 0 {
        disk.temperature_c = Some(h.temperature_k as i32 - 273);
    }
    disk.power_on_hours = Some(h.power_on_hours as u64);
    disk.power_cycles = Some(h.power_cycles as u64);
    disk.host_reads = Some(h.data_units_read * 512_000);
    disk.host_writes = Some(h.data_units_written * 512_000);
    disk.smart = Smart::Nvme(h);
}

fn smart_command(h: &Handle, feature: u8) -> Option<[u8; 512]> {
    // SENDCMDINPARAMS (packed): cBufferSize, IDEREGS[8], bDriveNumber, reserved...
    let mut input = [0u8; 32];
    input[0..4].copy_from_slice(&512u32.to_le_bytes());
    input[4] = feature; // bFeaturesReg
    input[5] = 1; // bSectorCountReg
    input[6] = 1; // bSectorNumberReg
    input[7] = 0x4F; // bCylLowReg  (SMART_CYL_LOW)
    input[8] = 0xC2; // bCylHighReg (SMART_CYL_HI)
    input[9] = 0xA0; // bDriveHeadReg
    input[10] = 0xB0; // bCommandReg (SMART_CMD)
    // SENDCMDOUTPARAMS: cBufferSize(4) + DRIVERSTATUS(12) + data
    let mut out = [0u8; 16 + 512];
    ioctl(h, SMART_RCV_DRIVE_DATA, &input, &mut out)?;
    if out[4] != 0 {
        return None; // bDriverError
    }
    let mut data = [0u8; 512];
    data.copy_from_slice(&out[16..]);
    Some(data)
}

fn read_ata_smart(h: &Handle) -> Option<Vec<AtaAttribute>> {
    let values = smart_command(h, 0xD0)?; // READ ATTRIBUTE VALUES
    let thresholds = smart_command(h, 0xD1).unwrap_or([0u8; 512]); // READ ATTRIBUTE THRESHOLDS

    let mut thresh = [0u8; 256];
    for i in 0..30 {
        let o = 2 + i * 12;
        let id = thresholds[o];
        if id != 0 {
            thresh[id as usize] = thresholds[o + 1];
        }
    }

    let mut attrs = Vec::new();
    for i in 0..30 {
        let o = 2 + i * 12;
        let id = values[o];
        if id == 0 {
            continue;
        }
        let mut raw = [0u8; 8];
        raw[..6].copy_from_slice(&values[o + 5..o + 11]);
        attrs.push(AtaAttribute {
            id,
            current: values[o + 3],
            worst: values[o + 4],
            threshold: thresh[id as usize],
            raw: u64::from_le_bytes(raw),
        });
    }
    (!attrs.is_empty()).then_some(attrs)
}

fn apply_ata(disk: &mut DiskInfo, attrs: Vec<AtaAttribute>) {
    let get = |id: u8| attrs.iter().find(|a| a.id == id);

    let mut status = HealthStatus::Good;
    // Reallocated, pending and uncorrectable sectors are early warning signs.
    for id in [0x05, 0xC5, 0xC6] {
        if get(id).is_some_and(|a| a.raw & 0xFFFF_FFFF > 0) {
            status = HealthStatus::Caution;
        }
    }
    if attrs.iter().any(|a| a.failing()) {
        status = HealthStatus::Bad;
    }
    disk.health = status;

    // Vendor "life remaining" attributes report percent in the normalized value.
    disk.life_percent = [0xE7, 0xCA, 0xA9]
        .iter()
        .find_map(|&id| get(id))
        .filter(|_| disk.ssd == Some(true))
        .map(|a| a.current.min(100));

    disk.temperature_c = get(0xC2)
        .or_else(|| get(0xBE))
        .map(|a| (a.raw & 0xFF) as i32)
        .filter(|&t| t > 0 && t < 120);
    disk.power_on_hours = get(0x09).map(|a| a.raw & 0xFFFF_FFFF);
    disk.power_cycles = get(0x0C).map(|a| a.raw & 0xFFFF_FFFF);
    disk.smart = Smart::Ata(attrs);
}

pub fn ata_attribute_name(id: u8) -> &'static str {
    match id {
        0x01 => "Read Error Rate",
        0x02 => "Throughput Performance",
        0x03 => "Spin-Up Time",
        0x04 => "Start/Stop Count",
        0x05 => "Reallocated Sectors Count",
        0x06 => "Read Channel Margin",
        0x07 => "Seek Error Rate",
        0x08 => "Seek Time Performance",
        0x09 => "Power-On Hours",
        0x0A => "Spin Retry Count",
        0x0B => "Recalibration Retries",
        0x0C => "Power Cycle Count",
        0x0D => "Soft Read Error Rate",
        0x16 => "Current Helium Level",
        0xAA => "Available Reserved Space",
        0xAB => "Program Fail Count",
        0xAC => "Erase Fail Count",
        0xAD => "Wear Leveling Count",
        0xAE => "Unexpected Power Loss Count",
        0xA9 => "Remaining Life",
        0xB1 => "Wear Range Delta",
        0xB3 => "Used Reserved Block Count Total",
        0xB4 => "Unused Reserved Block Count Total",
        0xB5 => "Program Fail Count Total",
        0xB6 => "Erase Fail Count",
        0xB7 => "SATA Downshift Error Count",
        0xB8 => "End-to-End Error",
        0xBB => "Reported Uncorrectable Errors",
        0xBC => "Command Timeout",
        0xBD => "High Fly Writes",
        0xBE => "Airflow Temperature",
        0xBF => "G-Sense Error Rate",
        0xC0 => "Power-off Retract Count",
        0xC1 => "Load/Unload Cycle Count",
        0xC2 => "Temperature",
        0xC3 => "Hardware ECC Recovered",
        0xC4 => "Reallocation Event Count",
        0xC5 => "Current Pending Sector Count",
        0xC6 => "Uncorrectable Sector Count",
        0xC7 => "UltraDMA CRC Error Count",
        0xC8 => "Multi-Zone Error Rate",
        0xC9 => "Soft Read Error Rate",
        0xCA => "Data Address Mark Errors / Life Remaining",
        0xCB => "Run Out Cancel",
        0xCC => "Soft ECC Correction",
        0xCD => "Thermal Asperity Rate",
        0xCE => "Flying Height",
        0xCF => "Spin High Current",
        0xD0 => "Spin Buzz",
        0xD1 => "Offline Seek Performance",
        0xDC => "Disk Shift",
        0xDD => "G-Sense Error Rate",
        0xDE => "Loaded Hours",
        0xDF => "Load/Unload Retry Count",
        0xE0 => "Load Friction",
        0xE1 => "Load/Unload Cycle Count",
        0xE2 => "Load 'In'-time",
        0xE3 => "Torque Amplification Count",
        0xE4 => "Power-Off Retract Cycle",
        0xE6 => "GMR Head Amplitude / Drive Life Protection",
        0xE7 => "SSD Life Left",
        0xE8 => "Endurance Remaining",
        0xE9 => "Media Wearout Indicator",
        0xEA => "Average Erase Count",
        0xEB => "Good Block Count",
        0xF0 => "Head Flying Hours",
        0xF1 => "Total Host Writes",
        0xF2 => "Total Host Reads",
        0xF3 => "Total LBAs Written Expanded",
        0xF4 => "Total LBAs Read Expanded",
        0xF9 => "NAND Writes (1GiB)",
        0xFA => "Read Error Retry Rate",
        0xFE => "Free Fall Protection",
        _ => "Vendor Specific",
    }
}

pub fn fmt_bytes(b: u64) -> String {
    fmt_bytes_u128(b as u128)
}

/// Decimal units, the way drive capacities are marketed.
pub fn fmt_bytes_u128(b: u128) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 999.95 && i < UNITS.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 { format!("{b} B") } else { format!("{v:.1} {}", UNITS[i]) }
}

#[cfg(test)]
mod tests {
    #[test]
    fn enumerates_physical_drives() {
        let disks = super::enumerate();
        assert!(!disks.is_empty(), "no physical drives found");
        for d in &disks {
            println!(
                "#{} {} | {} | {} | ssd={:?} trim={:?} letters={:?} health={:?} temp={:?} note={:?}",
                d.index,
                d.display_name(),
                d.bus,
                super::fmt_bytes(d.size_bytes),
                d.ssd,
                d.trim,
                d.letters,
                d.health,
                d.temperature_c,
                d.smart_note
            );
        }
    }
}
