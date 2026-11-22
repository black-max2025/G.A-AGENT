//! Profile Discovery Engine.
//! C:\Users and D:\Users are NOT assumed to be copies of each other, and USERPROFILE alone is not
//! enough. The real list of user profiles comes from Windows' ProfileList, and the real folder
//! locations (Documents, Downloads, Desktop...) from the user's Known Folders, which may have been
//! moved to another drive. This module only READS; nothing here changes a file.

use serde::Serialize;
use serde_json::Value;
use std::path::Path;
use std::sync::Mutex;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct KnownFolder {
    /// desktop | documents | pictures | videos | music | downloads | appdata
    pub kind: String,
    pub path: String,
    /// true when the folder lives outside the profile folder (e.g. moved to another drive)
    pub redirected: bool,
    pub exists: bool,
    /// true when Windows did not tell us (profile not loaded): the default location is assumed
    pub assumed: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct Profile {
    pub sid: String,
    pub name: String,
    pub path: String,
    pub drive: String,
    /// the user signed in right now (profile hive is loaded)
    pub active: bool,
    /// the user running this app
    pub current: bool,
    pub exists: bool,
    pub folders: Vec<KnownFolder>,
}

static CACHE: Mutex<Option<Vec<Profile>>> = Mutex::new(None);

pub fn drive_of(p: &str) -> String {
    let b: Vec<char> = p.chars().take(2).collect();
    if b.len() == 2 && b[1] == ':' {
        b[0].to_ascii_uppercase().to_string() + ":"
    } else {
        String::new()
    }
}

fn norm(p: &str) -> String {
    p.replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

pub fn is_inside(child: &str, parent: &str) -> bool {
    let c = norm(child);
    let p = norm(parent);
    c == p || c.starts_with(&format!("{}\\", p))
}

fn kind_of(key: &str) -> Option<&'static str> {
    Some(match key {
        "Desktop" => "desktop",
        "Personal" => "documents",
        "My Pictures" => "pictures",
        "My Video" => "videos",
        "My Music" => "music",
        "{374DE290-123F-4565-9164-39C4925E467B}" => "downloads",
        "AppData" => "appdata",
        _ => return None,
    })
}

const DEFAULTS: [(&str, &str); 7] = [
    ("desktop", "Desktop"),
    ("documents", "Documents"),
    ("pictures", "Pictures"),
    ("videos", "Videos"),
    ("music", "Music"),
    ("downloads", "Downloads"),
    ("appdata", "AppData\\Roaming"),
];

/// Expands %VAR% with this process' environment (other users' own variables are not needed:
/// the registry stores %USERPROFILE%, which is replaced by the profile path before this).
fn expand(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('%') {
            Some(j) => {
                let name = &after[..j];
                match std::env::var(name) {
                    Ok(v) if !name.is_empty() => out.push_str(&v),
                    _ => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[j + 1..];
            }
            None => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Turns the JSON produced by the PowerShell reader into profiles. Pure, so it is testable.
pub fn parse_profiles(json: &str) -> Vec<Profile> {
    let v: Value = match serde_json::from_str(json.trim().trim_start_matches('\u{feff}')) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let arr = match v {
        Value::Array(a) => a,
        Value::Null => Vec::new(),
        o => vec![o],
    };
    let mut out = Vec::new();
    for p in arr {
        let sid = p["sid"].as_str().unwrap_or("").to_string();
        // Only real people: local/domain accounts and Microsoft/Azure AD accounts.
        if !(sid.starts_with("S-1-5-21-") || sid.starts_with("S-1-12-1-")) {
            continue;
        }
        let path = expand(p["path"].as_str().unwrap_or(""));
        if path.is_empty() {
            continue;
        }
        let name = path
            .trim_end_matches('\\')
            .rsplit('\\')
            .next()
            .unwrap_or("")
            .to_string();
        let active = p["loaded"].as_bool().unwrap_or(false);
        let mut folders: Vec<KnownFolder> = Vec::new();
        if let Some(map) = p["folders"].as_object() {
            for (k, val) in map {
                let (Some(kind), Some(raw)) = (kind_of(k), val.as_str()) else {
                    continue;
                };
                let real = expand(&raw.replace("%USERPROFILE%", &path));
                folders.push(KnownFolder {
                    kind: kind.to_string(),
                    redirected: !is_inside(&real, &path),
                    exists: Path::new(&real).exists(),
                    path: real,
                    assumed: false,
                });
            }
        }
        for (kind, sub) in DEFAULTS {
            if !folders.iter().any(|f| f.kind == kind) {
                let real = format!("{}\\{}", path.trim_end_matches('\\'), sub);
                folders.push(KnownFolder {
                    kind: kind.to_string(),
                    exists: Path::new(&real).exists(),
                    path: real,
                    redirected: false,
                    assumed: true,
                });
            }
        }
        folders.sort_by(|a, b| a.kind.cmp(&b.kind));
        out.push(Profile {
            sid,
            name,
            drive: drive_of(&path),
            exists: Path::new(&path).exists(),
            current: p["current"].as_bool().unwrap_or(false),
            active,
            path,
            folders,
        });
    }
    out
}

const SCRIPT: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$cur = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$loaded = @(Get-ChildItem Registry::HKEY_USERS | ForEach-Object { $_.PSChildName })
$out = @()
Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\*' | ForEach-Object {
  $sid = $_.PSChildName
  $p = [Environment]::ExpandEnvironmentVariables($_.ProfileImagePath)
  $f = @{}
  if ($loaded -contains $sid) {
    $k = "Registry::HKEY_USERS\$sid\Software\Microsoft\Windows\CurrentVersion\Explorer\User Shell Folders"
    $v = Get-ItemProperty $k
    if ($v) {
      foreach ($n in @('Desktop','Personal','My Pictures','My Video','My Music','{374DE290-123F-4565-9164-39C4925E467B}','AppData')) {
        $x = $v.$n
        if ($x) { $f[$n] = $x }
      }
    }
  }
  $out += [pscustomobject]@{ sid = $sid; path = $p; current = ($sid -eq $cur); loaded = ($loaded -contains $sid); folders = $f }
}
ConvertTo-Json -InputObject @($out) -Depth 4 -Compress
"#;

#[cfg(windows)]
fn read_system() -> Vec<Profile> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    let file = crate::agent::base_dir().join("read_profiles.ps1");
    if std::fs::create_dir_all(crate::agent::base_dir()).is_err()
        || std::fs::write(&file, SCRIPT).is_err()
    {
        return Vec::new();
    }
    let out = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&file)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000)
        .output();
    match out {
        Ok(o) => parse_profiles(&String::from_utf8_lossy(&o.stdout)),
        Err(_) => Vec::new(),
    }
}

#[cfg(not(windows))]
fn read_system() -> Vec<Profile> {
    let _ = SCRIPT;
    Vec::new()
}

/// All user profiles of this PC (cached; pass `refresh` to read Windows again).
pub fn discover(refresh: bool) -> Vec<Profile> {
    if !refresh {
        if let Ok(g) = CACHE.lock() {
            if let Some(v) = g.as_ref() {
                return v.clone();
            }
        }
    }
    let mut v = read_system();
    if v.is_empty() {
        // Fallback when Windows could not be read: at least protect the current user.
        if let Ok(up) = std::env::var("USERPROFILE") {
            let json = format!(
                r#"[{{"sid":"S-1-5-21-0-0-0-0","path":{},"current":true,"loaded":true,"folders":{{}}}}]"#,
                serde_json::to_string(&up).unwrap_or_default()
            );
            v = parse_profiles(&json);
        }
    }
    if let Ok(mut g) = CACHE.lock() {
        *g = Some(v.clone());
    }
    v
}

/// Normalised paths that must never be deleted or moved as a whole: every profile folder and
/// every known folder of every profile (including ones moved to another drive).
pub fn protected_roots() -> Vec<String> {
    let mut v = Vec::new();
    for p in discover(false) {
        v.push(norm(&p.path));
        for f in &p.folders {
            v.push(norm(&f.path));
        }
        v.push(norm(&format!("{}\\AppData", p.path.trim_end_matches('\\'))));
    }
    v.sort();
    v.dedup();
    v
}

/// True when `path` is inside a profile that is NOT the current user's (never touched).
pub fn in_other_profile(path: &str) -> bool {
    discover(false)
        .iter()
        .any(|p| !p.current && is_inside(path, &p.path))
}

#[tauri::command(async)]
pub fn profiles_list(refresh: bool) -> Vec<Profile> {
    discover(refresh)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"[
      {"sid":"S-1-5-18","path":"C:\\Windows\\system32\\config\\systemprofile","current":false,"loaded":true,"folders":{}},
      {"sid":"S-1-5-21-1-2-3-1001","path":"C:\\Users\\Ahmed","current":true,"loaded":true,
       "folders":{"Personal":"D:\\Docs","{374DE290-123F-4565-9164-39C4925E467B}":"%USERPROFILE%\\Downloads","My Video":"D:\\Videos"}},
      {"sid":"S-1-5-21-1-2-3-1002","path":"D:\\Users\\Old","current":false,"loaded":false,"folders":{}}
    ]"#;

    #[test]
    fn system_accounts_are_ignored_and_two_users_dirs_are_kept_separate() {
        let v = parse_profiles(SAMPLE);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].drive, "C:");
        assert_eq!(v[1].drive, "D:");
        assert_ne!(v[0].path, v[1].path);
    }

    #[test]
    fn active_and_old_profiles_are_told_apart() {
        let v = parse_profiles(SAMPLE);
        assert!(v[0].active && v[0].current);
        assert!(!v[1].active && !v[1].current);
    }

    #[test]
    fn redirected_known_folders_are_detected() {
        let v = parse_profiles(SAMPLE);
        let f = |k: &str| v[0].folders.iter().find(|f| f.kind == k).unwrap().clone();
        assert_eq!(f("documents").path, "D:\\Docs");
        assert!(f("documents").redirected && !f("documents").assumed);
        assert!(f("videos").redirected);
        assert!(!f("downloads").redirected);
        assert_eq!(f("downloads").path, "C:\\Users\\Ahmed\\Downloads");
    }

    #[test]
    fn unloaded_profile_uses_assumed_default_folders() {
        let v = parse_profiles(SAMPLE);
        let d = v[1].folders.iter().find(|f| f.kind == "downloads").unwrap();
        assert!(d.assumed);
        assert_eq!(d.path, "D:\\Users\\Old\\Downloads");
    }

    #[test]
    fn single_object_json_is_accepted() {
        let one = r#"{"sid":"S-1-5-21-9-9-9-1001","path":"C:\\Users\\X","current":true,"loaded":true,"folders":{}}"#;
        assert_eq!(parse_profiles(one).len(), 1);
        assert!(parse_profiles("garbage").is_empty());
    }

    #[test]
    fn inside_check_is_case_and_slash_insensitive() {
        assert!(is_inside("c:/users/ahmed/Docs/a.txt", "C:\\Users\\Ahmed"));
        assert!(!is_inside("C:\\Users\\Ahmed2", "C:\\Users\\Ahmed"));
    }

    #[test]
    fn percent_expansion_keeps_unknown_variables() {
        assert_eq!(expand("%NO_SUCH_VAR_X%\\a"), "%NO_SUCH_VAR_X%\\a");
        assert_eq!(expand("plain"), "plain");
    }
}
