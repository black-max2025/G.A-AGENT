//! Sense of time: the agent always knows the local date, time and weekday of this PC.

#[derive(Clone, Copy, Debug)]
pub struct Local {
    pub y: i32,
    pub mo: u32,
    pub d: u32,
    pub h: u32,
    pub mi: u32,
    pub wd: u32,       // 0 = Sunday
    pub bias_min: i32, // minutes WEST of UTC, as Windows reports it
}

#[cfg(windows)]
pub fn now_local() -> Local {
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    use windows_sys::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    unsafe {
        let mut t: windows_sys::Win32::Foundation::SYSTEMTIME = std::mem::zeroed();
        GetLocalTime(&mut t);
        let mut tz: TIME_ZONE_INFORMATION = std::mem::zeroed();
        let id = GetTimeZoneInformation(&mut tz);
        let extra = if id == 2 { tz.DaylightBias } else { 0 };
        Local {
            y: t.wYear as i32,
            mo: t.wMonth as u32,
            d: t.wDay as u32,
            h: t.wHour as u32,
            mi: t.wMinute as u32,
            wd: t.wDayOfWeek as u32,
            bias_min: tz.Bias + extra,
        }
    }
}

#[cfg(not(windows))]
pub fn now_local() -> Local {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (y, mo, d) = civil_from_days(days);
    Local {
        y,
        mo,
        d,
        h: (rem / 3600) as u32,
        mi: ((rem % 3600) / 60) as u32,
        wd: ((days + 4).rem_euclid(7)) as u32,
        bias_min: 0,
    }
}

fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((if m <= 2 { y + 1 } else { y }) as i32, m, d)
}

fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y as i64 - 1 } else { y as i64 };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Minutes since 1970 on the local clock (no timezone conversion).
pub fn local_minutes(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
    days_from_civil(y, mo, d) * 1440 + h as i64 * 60 + mi as i64
}

pub fn now_minutes() -> i64 {
    let n = now_local();
    local_minutes(n.y, n.mo, n.d, n.h, n.mi)
}

/// Parses "YYYY-MM-DDTHH:MM" (local time).
pub fn parse_local(s: &str) -> Option<(i32, u32, u32, u32, u32)> {
    let s = s.trim();
    let (date, time) = s.split_once('T').or_else(|| s.split_once(' '))?;
    let mut dp = date.split('-');
    let y: i32 = dp.next()?.parse().ok()?;
    let mo: u32 = dp.next()?.parse().ok()?;
    let d: u32 = dp.next()?.parse().ok()?;
    let mut tp = time.split(':');
    let h: u32 = tp.next()?.parse().ok()?;
    let mi: u32 = tp.next()?.parse().ok()?;
    if !(1..=12).contains(&mo)
        || !(1..=31).contains(&d)
        || h > 23
        || mi > 59
        || !(2000..=2100).contains(&y)
    {
        return None;
    }
    Some((y, mo, d, h, mi))
}

pub fn iso(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> String {
    format!("{:04}-{:02}-{:02}T{:02}:{:02}", y, mo, d, h, mi)
}

pub fn from_minutes(m: i64) -> (i32, u32, u32, u32, u32) {
    let days = m.div_euclid(1440);
    let rem = m.rem_euclid(1440);
    let (y, mo, d) = civil_from_days(days);
    (y, mo, d, (rem / 60) as u32, (rem % 60) as u32)
}

pub fn weekday_name(wd: u32, lang: &str) -> &'static str {
    const AR: [&str; 7] = [
        "الأحد",
        "الاثنين",
        "الثلاثاء",
        "الأربعاء",
        "الخميس",
        "الجمعة",
        "السبت",
    ];
    const EN: [&str; 7] = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    const ID: [&str; 7] = [
        "Minggu", "Senin", "Selasa", "Rabu", "Kamis", "Jumat", "Sabtu",
    ];
    const FR: [&str; 7] = [
        "dimanche", "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi",
    ];
    const IT: [&str; 7] = [
        "domenica",
        "lunedì",
        "martedì",
        "mercoledì",
        "giovedì",
        "venerdì",
        "sabato",
    ];
    let i = (wd % 7) as usize;
    match lang {
        "ar" => AR[i],
        "id" => ID[i],
        "fr" => FR[i],
        "it" => IT[i],
        _ => EN[i],
    }
}

pub fn offset_label(bias_min: i32) -> String {
    let east = -bias_min;
    format!(
        "UTC{}{:02}:{:02}",
        if east >= 0 { "+" } else { "-" },
        east.abs() / 60,
        east.abs() % 60
    )
}

/// e.g. "Saturday 2026-11-21 08:58 (UTC+02:00)"
pub fn describe_now(lang: &str) -> String {
    let n = now_local();
    format!(
        "{} {} ({})",
        weekday_name(n.wd, lang),
        iso(n.y, n.mo, n.d, n.h, n.mi).replace('T', " "),
        offset_label(n.bias_min)
    )
}

/// Human countdown such as "in 2h 15m" (language-neutral compact form).
pub fn countdown(minutes: i64) -> String {
    if minutes <= 0 {
        return "0m".to_string();
    }
    let (d, h, m) = (minutes / 1440, (minutes % 1440) / 60, minutes % 60);
    if d > 0 {
        format!("{}d {}h", d, h)
    } else if h > 0 {
        format!("{}h {}m", h, m)
    } else {
        format!("{}m", m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        let m = local_minutes(2026, 11, 21, 8, 58);
        assert_eq!(from_minutes(m), (2026, 11, 21, 8, 58));
        assert_eq!(parse_local("2026-11-21T08:58"), Some((2026, 11, 21, 8, 58)));
        assert!(parse_local("2026-13-01T00:00").is_none());
    }
    #[test]
    fn weekday_known() {
        // 1970-01-01 was a Thursday
        assert_eq!(((days_from_civil(1970, 1, 1) + 4).rem_euclid(7)) as u32, 4);
    }
}
