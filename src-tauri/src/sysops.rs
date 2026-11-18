//! Programs and system operations. All scripts here are fixed (not model-written);
//! user-supplied values are validated against strict character sets and passed as
//! separate arguments, never through a shell string.

use crate::assistant::ps_raw;
use std::process::{Command, Stdio};

fn es<E: ToString>(e: E) -> String {
    e.to_string()
}

pub fn current_time() -> String {
    ps_raw("Get-Date -Format 'yyyy-MM-dd HH:mm dddd'")
        .trim()
        .to_string()
}

pub fn time_after(minutes: u64) -> String {
    ps_raw(&format!(
        "(Get-Date).AddMinutes({}).ToString('yyyy-MM-dd HH:mm')",
        minutes
    ))
    .trim()
    .to_string()
}

pub fn list_apps() -> String {
    ps_raw(
        r#"$p = Get-ItemProperty 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*','HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*','HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*' -ErrorAction SilentlyContinue | Where-Object { $_.DisplayName } | Sort-Object { [int64]$_.EstimatedSize } -Descending | Select-Object -First 40 DisplayName, DisplayVersion, @{n='MB';e={[int]($_.EstimatedSize/1024)}}; $p | ForEach-Object { "$($_.DisplayName) | $($_.DisplayVersion) | $($_.MB) MB" }"#,
    )
}

pub fn list_processes() -> String {
    ps_raw(
        r#"Get-Process | Sort-Object WS -Descending | Select-Object -First 15 | ForEach-Object { "$($_.ProcessName) | $([int]($_.WS/1MB)) MB | pid $($_.Id)" }"#,
    )
}

pub fn system_health() -> String {
    ps_raw(
        r#"$o = Get-CimInstance Win32_OperatingSystem; $c = (Get-CimInstance Win32_Processor | Measure-Object -Property LoadPercentage -Average).Average; "المعالج: $c% | الذاكرة المتاحة: $([int]($o.FreePhysicalMemory/1024)) MB من $([int]($o.TotalVisibleMemorySize/1024)) MB | آخر تشغيل: $($o.LastBootUpTime)""#,
    )
}

pub fn list_startup() -> String {
    ps_raw(
        r#"Get-CimInstance Win32_StartupCommand | ForEach-Object { "$($_.Name) | $($_.Command)" }"#,
    )
}

fn token_ok(s: &str, allow: &str, max: usize) -> bool {
    !s.is_empty()
        && s.chars().count() <= max
        && s.chars().all(|c| c.is_alphanumeric() || allow.contains(c))
}

fn hide(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

pub fn open_app(target: &str) -> Result<String, String> {
    let t = target.trim();
    if !token_ok(t, " _.:\\/()-", 200) || t.contains("..") {
        return Err("اسم أو مسار غير مقبول".to_string());
    }
    let low = t.to_lowercase();
    for bad in [
        ".bat", ".cmd", ".ps1", ".vbs", ".js", ".reg", ".scr", ".hta", ".msi",
    ] {
        if low.ends_with(bad) {
            return Err("لا أفتح هذا النوع من الملفات".to_string());
        }
    }
    let mut c = Command::new("cmd.exe");
    c.args(["/C", "start", "", t])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hide(&mut c);
    c.spawn().map_err(es)?;
    Ok(format!("تم طلب فتح: {}", t))
}

pub fn close_app(name: &str) -> Result<String, String> {
    let n = name.trim().trim_end_matches(".exe");
    if !token_ok(n, "_.-", 60) {
        return Err("اسم برنامج غير مقبول".to_string());
    }
    let low = n.to_lowercase();
    for p in [
        "explorer", "winlogon", "csrss", "system", "svchost", "lsass", "services", "wininit",
        "smss", "dwm", "ga-agent", "ga_agent", "taskmgr",
    ] {
        if low == p {
            return Err("هذه عملية نظام أو التطبيق نفسه، لن أغلقها".to_string());
        }
    }
    let mut c = Command::new("taskkill");
    c.args(["/IM", &format!("{}.exe", n)]);
    hide(&mut c);
    let out = c.output().map_err(es)?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(if text.trim().is_empty() {
        "تم".to_string()
    } else {
        text
    })
}
