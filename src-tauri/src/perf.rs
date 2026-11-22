//! Keeps the app light on an old PC (e.g. i5-2400 / 8 GB): the AI engine lives in a Windows
//! Job Object with a hard CPU cap, the whole app runs at below-normal priority, and the engine
//! is unloaded when idle. Nothing here can make the PC unusable.

use std::process::Child;

pub struct Job(isize);

unsafe impl Send for Job {}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, SetPriorityClass};

    const JOB_EXTENDED_LIMIT: i32 = 9;
    const JOB_CPU_RATE: i32 = 15;
    const KILL_ON_CLOSE: u32 = 0x2000;
    const BELOW_NORMAL: u32 = 0x4000;

    #[repr(C)]
    struct CpuRateInfo {
        flags: u32, // ENABLE (1) | HARD_CAP (4)
        rate: u32,  // percent * 100
    }

    pub fn lower_own_priority() {
        unsafe {
            SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL);
        }
    }

    pub fn cap_child(child: &Child, percent: u32) -> Result<Job, String> {
        unsafe {
            let h: HANDLE = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if h.is_null() {
                return Err("job".to_string());
            }
            let mut ext: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            ext.BasicLimitInformation.LimitFlags = KILL_ON_CLOSE;
            if SetInformationJobObject(
                h,
                JOB_EXTENDED_LIMIT,
                &ext as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
            {
                CloseHandle(h);
                return Err("job limit".to_string());
            }
            let info = CpuRateInfo {
                flags: 0x5,
                rate: percent.clamp(5, 100) * 100,
            };
            if SetInformationJobObject(
                h,
                JOB_CPU_RATE,
                &info as *const _ as *const _,
                std::mem::size_of::<CpuRateInfo>() as u32,
            ) == 0
            {
                CloseHandle(h);
                return Err("cpu cap".to_string());
            }
            if AssignProcessToJobObject(h, child.as_raw_handle() as HANDLE) == 0 {
                CloseHandle(h);
                return Err("assign".to_string());
            }
            Ok(Job(h as isize))
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0 as HANDLE);
            }
        }
    }
}

#[cfg(windows)]
pub use imp::{cap_child, lower_own_priority};

#[cfg(not(windows))]
pub fn lower_own_priority() {}

#[cfg(not(windows))]
pub fn cap_child(_c: &Child, _p: u32) -> Result<Job, String> {
    Ok(Job(0))
}

/// Number of worker threads for scans: never more than half of the cores.
pub fn scan_threads() -> usize {
    let n = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2);
    (n / 2).max(1)
}

/// Free physical memory in bytes (None when it cannot be read).
#[cfg(windows)]
pub fn available_ram() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    if unsafe { GlobalMemoryStatusEx(&mut m) } != 0 {
        Some(m.ullAvailPhys)
    } else {
        None
    }
}

#[cfg(not(windows))]
pub fn available_ram() -> Option<u64> {
    None
}

/// Lighter settings when memory is short, so the app never freezes the PC:
/// (context, threads). Pure function so it can be tested.
pub fn fit_to_memory(ctx: u32, threads: u32, model_bytes: u64, avail: Option<u64>) -> (u32, u32) {
    const GB: u64 = 1 << 30;
    match avail {
        Some(a) if a < model_bytes + GB => (ctx.min(2048), 1),
        Some(a) if a < model_bytes + 2 * GB => (ctx.min(3072), threads.min(2)),
        _ => (ctx, threads),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_memory_falls_back_to_light_settings() {
        const GB: u64 = 1 << 30;
        assert_eq!(fit_to_memory(4096, 2, 2 * GB, Some(2 * GB)), (2048, 1));
        assert_eq!(fit_to_memory(4096, 2, GB, Some(2 * GB + GB / 2)), (3072, 2));
        assert_eq!(fit_to_memory(4096, 2, GB, Some(6 * GB)), (4096, 2));
        assert_eq!(fit_to_memory(4096, 2, GB, None), (4096, 2));
    }
}
