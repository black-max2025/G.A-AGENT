//! Safety layer: decides which paths may NEVER be touched.
//! The frontend can never send raw paths for deletion; only ids of findings
//! produced by a scan are accepted, and every target is re-checked here.

use std::path::{Path, PathBuf};

/// Lower-case, backslash-normalised path string without trailing slash.
pub fn norm(p: &Path) -> String {
    p.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

#[cfg(windows)]
pub fn is_reparse(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
pub fn is_reparse(meta: &std::fs::Metadata) -> bool {
    meta.file_type().is_symlink()
}

/// Cloud placeholder / offline files (OneDrive etc.): never read or hash them.
#[cfg(windows)]
pub fn is_offline(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    // OFFLINE | RECALL_ON_OPEN | RECALL_ON_DATA_ACCESS
    meta.file_attributes() & (0x1000 | 0x40000 | 0x400000) != 0
}

#[cfg(not(windows))]
pub fn is_offline(_meta: &std::fs::Metadata) -> bool {
    false
}

fn user_profile() -> Option<PathBuf> {
    std::env::var("USERPROFILE").ok().map(PathBuf::from)
}

/// Roots that hold the user's own work: the folders themselves are never deleted.
fn user_folder_roots() -> Vec<String> {
    let mut v = Vec::new();
    if let Some(up) = user_profile() {
        v.push(norm(&up));
        for d in [
            "Documents",
            "Desktop",
            "Pictures",
            "Videos",
            "Music",
            "Downloads",
            "OneDrive",
            "AppData",
        ] {
            v.push(norm(&up.join(d)));
        }
    }
    // Every registered profile (C:\Users and D:\Users...) and every Known Folder, wherever
    // Windows has moved it.
    v.extend(crate::profiles::protected_roots());
    v
}

fn system_roots() -> Vec<String> {
    let mut v = Vec::new();
    for var in [
        "WINDIR",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramW6432",
        "ProgramData",
    ] {
        if let Ok(x) = std::env::var(var) {
            v.push(norm(Path::new(&x)));
        }
    }
    if let Ok(sd) = std::env::var("SystemDrive") {
        let base = PathBuf::from(format!("{}\\", sd));
        for d in [
            "$Recycle.Bin",
            "System Volume Information",
            "Recovery",
            "Boot",
        ] {
            v.push(norm(&base.join(d)));
        }
    }
    if let Ok(la) = std::env::var("LOCALAPPDATA") {
        // The app's own data (models, llama server) must never be cleaned.
        v.push(norm(&Path::new(&la).join("GA-Agent")));
    }
    v
}

/// Minimal sanity check applied to EVERY target (rules and heuristics).
pub fn is_sane_target(path: &Path) -> bool {
    let p = norm(path);
    if p.len() <= 3 {
        return false; // drive root
    }
    if user_folder_roots().iter().any(|r| *r == p) {
        return false;
    }
    if p.contains("\\system32") || p.contains("\\winsxs") || p.contains("\\.git\\") {
        return false;
    }
    if p.ends_with("\\.git") {
        return false;
    }
    if is_app_owned(path) {
        return false;
    }
    if crate::profiles::in_other_profile(&p) {
        return false; // other users' files are never touched
    }
    path.components().count() >= 3
}

/// Full validation of a path BEFORE any change (compress, link, move, delete).
/// Returns an ERR_ code the UI can translate. Pure checks first, then the file system.
pub fn check_target(path: &Path) -> Result<(), String> {
    let s = path.to_string_lossy();
    if !path.is_absolute() || s.contains("..") || s.contains('\0') {
        return Err("ERR_PATH_NOT_ALLOWED".to_string());
    }
    if !is_sane_target(path) || is_protected(path) {
        return Err("ERR_PATH_NOT_ALLOWED".to_string());
    }
    let n = norm(path);
    if n.contains("\\winsxs") || n.contains("\\program files") || n.contains("\\windows\\") {
        return Err("ERR_PATH_NOT_ALLOWED".to_string());
    }
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if is_reparse(&meta) {
            return Err("ERR_SPECIAL_LINKS".to_string());
        }
        if is_offline(&meta) {
            return Err("ERR_FILE_OFFLINE".to_string());
        }
    }
    Ok(())
}

/// Strict check for heuristic (user-folder) findings: system locations are off limits.
pub fn is_protected(path: &Path) -> bool {
    if !is_sane_target(path) {
        return true;
    }
    let p = norm(path);
    for root in system_roots() {
        if p == root || p.starts_with(&format!("{}\\", root)) {
            return true;
        }
    }
    false
}

/// Paths owned by the app itself (settings, journal, engine, and the user's chosen
/// model file / data folder). They are never cleaned, compressed, or reported.
pub fn owned_paths() -> Vec<String> {
    let mut v = vec![norm(&crate::agent::base_dir())];
    let c = crate::agent::load_config();
    if let Some(m) = c.model_path {
        v.push(norm(Path::new(&m)));
    }
    if let Some(d) = c.data_dir {
        if !d.trim().is_empty() {
            v.push(norm(Path::new(&d)));
        }
    }
    v
}

pub fn is_owned_by(owned: &[String], path: &Path) -> bool {
    let p = norm(path);
    owned
        .iter()
        .any(|o| p == *o || p.starts_with(&format!("{}\\", o)))
}

pub fn is_app_owned(path: &Path) -> bool {
    is_owned_by(&owned_paths(), path)
}

/// True for places a file-organizing or download tool must never touch.
pub fn is_system_location(path: &Path) -> bool {
    let p = norm(path);
    if p.len() <= 3 {
        return true;
    }
    if is_app_owned(path) {
        return true;
    }
    if let Some(up) = user_profile() {
        let u = norm(&up);
        if p == u || p == format!("{}\\appdata", u) || p.starts_with(&format!("{}\\appdata\\", u)) {
            return true;
        }
    }
    for r in system_roots() {
        if p == r || p.starts_with(&format!("{}\\", r)) {
            return true;
        }
    }
    p.contains("\\system32") || p.contains("\\winsxs")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_and_traversal_paths_are_refused() {
        assert!(check_target(Path::new("relative\\dir")).is_err());
        assert!(check_target(Path::new("C:\\Users\\a\\..\\..\\Windows")).is_err());
    }

    #[test]
    fn drive_roots_and_system_folders_are_refused() {
        assert!(!is_sane_target(Path::new("C:\\")));
        assert!(!is_sane_target(Path::new("C:\\Windows\\System32\\drivers")));
        assert!(!is_sane_target(Path::new("C:\\Windows\\WinSxS\\x")));
    }

    #[test]
    fn norm_is_case_and_slash_insensitive() {
        assert_eq!(norm(Path::new("C:/Users/Ahmed/")), "c:\\users\\ahmed");
    }
}
