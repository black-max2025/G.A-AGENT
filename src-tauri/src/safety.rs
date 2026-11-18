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
        for d in ["$Recycle.Bin", "System Volume Information", "Recovery", "Boot"] {
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
    if let Ok(la) = std::env::var("LOCALAPPDATA") {
        let own = norm(&Path::new(&la).join("GA-Agent"));
        if p == own || p.starts_with(&format!("{}\\", own)) {
            return false;
        }
    }
    path.components().count() >= 3
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
