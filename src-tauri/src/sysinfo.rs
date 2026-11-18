use serde::Serialize;

#[derive(Serialize, Clone)]
pub struct DriveInfo {
    pub drive: String,
    pub free: u64,
    pub total: u64,
}

pub fn fmt_bytes(n: u64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0usize;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i > 1 {
        format!("{:.2} {}", v, units[i])
    } else {
        format!("{:.0} {}", v, units[i])
    }
}

#[cfg(windows)]
pub fn drives() -> Vec<DriveInfo> {
    use std::os::windows::ffi::OsStrExt;
    let mut out = Vec::new();
    for l in b'C'..=b'Z' {
        let root = format!("{}:\\", l as char);
        if !std::path::Path::new(&root).exists() {
            continue;
        }
        let wide: Vec<u16> = std::ffi::OsStr::new(&root)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut avail: u64 = 0;
        let mut total: u64 = 0;
        let mut free: u64 = 0;
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut avail,
                &mut total,
                &mut free,
            )
        };
        if ok != 0 {
            out.push(DriveInfo {
                drive: format!("{}:", l as char),
                free: avail,
                total,
            });
        }
    }
    out
}

#[cfg(not(windows))]
pub fn drives() -> Vec<DriveInfo> {
    Vec::new()
}

/// Bytes actually occupied on disk (reflects NTFS/WOF compression).
#[cfg(windows)]
pub fn physical_size(p: &std::path::Path, logical: u64) -> u64 {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = p
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut high: u32 = 0;
    let low = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetCompressedFileSizeW(wide.as_ptr(), &mut high)
    };
    if low == u32::MAX && high == 0 {
        return logical;
    }
    ((high as u64) << 32) | (low as u64)
}

#[cfg(not(windows))]
pub fn physical_size(_p: &std::path::Path, logical: u64) -> u64 {
    logical
}
