//! De-duplication with NTFS hard links: identical files keep their names and places, but share one
//! copy of the data, so the space is really freed. Only "static" file types (media, archives,
//! installers, PDFs) are linked, because editing one hard-linked file would change all of them.
//! Every link can be undone from the journal (the file becomes a separate copy again).

use crate::{safety, shrink};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;
use walkdir::WalkDir;

const MIN_SIZE: u64 = 1024 * 1024;
const STATIC_EXT: [&str; 34] = [
    "jpg", "jpeg", "png", "gif", "webp", "heic", "bmp", "tif", "tiff", "mp4", "mkv", "avi", "mov",
    "wmv", "webm", "mp3", "wav", "flac", "m4a", "ogg", "zip", "rar", "7z", "iso", "exe", "msi",
    "pdf", "cab", "gz", "tar", "bin", "img", "dmg", "apk",
];

fn es<E: ToString>(e: E) -> String {
    e.to_string()
}

struct Group {
    size: u64,
    keeper: String,
    others: Vec<String>,
}

struct Plan {
    id: String,
    root: String,
    groups: Vec<Group>,
    saved: u64,
    stamp: HashMap<String, (u64, u64)>, // path -> (size, mtime)
}

static PLANS: Mutex<Vec<Plan>> = Mutex::new(Vec::new());

#[derive(Serialize)]
pub struct DedupeScan {
    pub plan_id: String,
    pub groups: usize,
    pub files: usize,
    pub saved: u64,
    pub examples: Vec<String>,
}

#[cfg(windows)]
fn file_id(p: &Path) -> Option<(u32, u64)> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };
    let f = fs::File::open(p).ok()?;
    unsafe {
        let mut i: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
        if GetFileInformationByHandle(f.as_raw_handle() as HANDLE, &mut i) == 0 {
            return None;
        }
        Some((
            i.dwVolumeSerialNumber,
            ((i.nFileIndexHigh as u64) << 32) | i.nFileIndexLow as u64,
        ))
    }
}

#[cfg(not(windows))]
fn file_id(_p: &Path) -> Option<(u32, u64)> {
    None
}

fn hash_file(p: &Path, limit: Option<usize>) -> Option<String> {
    let mut f = fs::File::open(p).ok()?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut left = limit.unwrap_or(usize::MAX);
    while left > 0 {
        let want = buf.len().min(left);
        let n = f.read(&mut buf[..want]).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        left -= n;
    }
    Some(h.finalize().iter().map(|b| format!("{:02x}", b)).collect())
}

fn stamp_of(p: &Path) -> Option<(u64, u64)> {
    let m = fs::metadata(p).ok()?;
    let t = m
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some((m.len(), t))
}

pub fn scan_folder(root: &Path) -> Result<DedupeScan, String> {
    if !root.is_dir() || safety::is_protected(root) {
        return Err("ERR_PATH_NOT_ALLOWED".to_string());
    }
    let owned = safety::owned_paths();
    let mut by_size: HashMap<u64, Vec<PathBuf>> = HashMap::new();
    for e in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let p = e.path();
        let ext = p
            .extension()
            .map(|x| x.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if !STATIC_EXT.contains(&ext.as_str()) {
            continue;
        }
        let Ok(meta) = e.metadata() else { continue };
        if meta.len() < MIN_SIZE
            || safety::is_reparse(&meta)
            || safety::is_offline(&meta)
            || safety::is_system_location(p)
            || safety::is_owned_by(&owned, p)
        {
            continue;
        }
        by_size.entry(meta.len()).or_default().push(p.to_path_buf());
    }
    let mut groups = Vec::new();
    let mut stamp = HashMap::new();
    let mut saved = 0u64;
    for (size, paths) in by_size.into_iter().filter(|(_, v)| v.len() > 1) {
        // cheap pre-filter: first 256 KiB, then the whole file
        let mut by_head: HashMap<String, Vec<PathBuf>> = HashMap::new();
        for p in paths {
            if let Some(h) = hash_file(&p, Some(256 * 1024)) {
                by_head.entry(h).or_default().push(p);
            }
        }
        for (_, hp) in by_head.into_iter().filter(|(_, v)| v.len() > 1) {
            let mut by_full: HashMap<String, Vec<PathBuf>> = HashMap::new();
            for p in hp {
                if let Some(h) = hash_file(&p, None) {
                    by_full.entry(h).or_default().push(p);
                }
            }
            for (_, same) in by_full.into_iter().filter(|(_, v)| v.len() > 1) {
                // hard links only work inside one volume
                let mut by_vol: HashMap<u32, Vec<(PathBuf, u64)>> = HashMap::new();
                for p in same {
                    if let Some((vol, idx)) = file_id(&p) {
                        by_vol.entry(vol).or_default().push((p, idx));
                    }
                }
                for (_, mut v) in by_vol {
                    v.sort_by_key(|(p, _)| p.to_string_lossy().len());
                    let Some((keeper, kid)) = v.first().cloned() else {
                        continue;
                    };
                    let mut others = Vec::new();
                    let mut ids = std::collections::HashSet::new();
                    for (p, id) in v.iter().skip(1) {
                        if *id != kid {
                            others.push(p.to_string_lossy().to_string());
                            ids.insert(*id);
                        }
                    }
                    if others.is_empty() {
                        continue;
                    }
                    saved += size * ids.len() as u64;
                    if let Some(s) = stamp_of(&keeper) {
                        stamp.insert(keeper.to_string_lossy().to_string(), s);
                    }
                    for o in &others {
                        if let Some(s) = stamp_of(Path::new(o)) {
                            stamp.insert(o.clone(), s);
                        }
                    }
                    groups.push(Group {
                        size,
                        keeper: keeper.to_string_lossy().to_string(),
                        others,
                    });
                }
            }
        }
    }
    groups.sort_by(|a, b| (b.size * b.others.len() as u64).cmp(&(a.size * a.others.len() as u64)));
    let files: usize = groups.iter().map(|g| g.others.len()).sum();
    let examples: Vec<String> = groups
        .iter()
        .take(5)
        .map(|g| format!("{} (+{})", g.keeper, g.others.len()))
        .collect();
    let id = format!("d{}", shrink::now());
    let out = DedupeScan {
        plan_id: id.clone(),
        groups: groups.len(),
        files,
        saved,
        examples,
    };
    let mut g = PLANS.lock().map_err(es)?;
    g.retain(|p| p.id != id);
    g.push(Plan {
        id,
        root: root.to_string_lossy().to_string(),
        groups,
        saved,
        stamp,
    });
    while g.len() > 3 {
        g.remove(0);
    }
    Ok(out)
}

pub fn summary(plan_id: &str) -> Option<String> {
    let g = PLANS.lock().ok()?;
    let p = g.iter().find(|p| p.id == plan_id)?;
    Some(format!(
        "{} | {} groups | {} files | {} MB",
        p.root,
        p.groups.len(),
        p.groups.iter().map(|x| x.others.len()).sum::<usize>(),
        p.saved >> 20
    ))
}

pub fn apply(plan_id: &str) -> Result<String, String> {
    let (root, groups, stamp) = {
        let mut g = PLANS.lock().map_err(es)?;
        let i = g
            .iter()
            .position(|p| p.id == plan_id)
            .ok_or_else(|| "ERR_NO_PLAN".to_string())?;
        let p = g.remove(i);
        (p.root, p.groups, p.stamp)
    };
    let mut links: Vec<(String, String)> = Vec::new();
    let mut saved = 0u64;
    let mut seen_ids = std::collections::HashSet::new();
    for gr in &groups {
        let k = Path::new(&gr.keeper);
        if stamp_of(k) != stamp.get(&gr.keeper).copied() {
            continue; // keeper changed since the scan
        }
        for o in &gr.others {
            let op = Path::new(o);
            if stamp_of(op) != stamp.get(o).copied() {
                continue;
            }
            let tmp = PathBuf::from(format!("{}.ga-link", o));
            let _ = fs::remove_file(&tmp);
            if fs::hard_link(k, &tmp).is_err() {
                continue;
            }
            let same = fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0) == gr.size;
            if same && fs::rename(&tmp, op).is_ok() {
                links.push((gr.keeper.clone(), o.clone()));
                if seen_ids.insert(o.clone()) {
                    saved += gr.size;
                }
            } else {
                let _ = fs::remove_file(&tmp);
            }
        }
    }
    if links.is_empty() {
        return Err("ERR_DEDUPE_NOTHING".to_string());
    }
    let n = links.len();
    shrink::journal_dedupe(&root, links, saved);
    Ok(format!("{} files linked, {} MB freed", n, saved >> 20))
}

// ---------------------------------------------------------------- commands

#[tauri::command(async)]
pub fn dedupe_scan(path: String) -> Result<DedupeScan, String> {
    scan_folder(Path::new(&path))
}

#[tauri::command(async)]
pub fn dedupe_apply(plan_id: String) -> Result<String, String> {
    apply(&plan_id)
}
