//! Folder overview: sizes + what kind of data lives in each child folder.

use crate::safety;
use serde::Serialize;
use std::path::Path;
use walkdir::WalkDir;

#[derive(Serialize)]
pub struct FolderInfo {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub files: u64,
    /// media | docs | code | archives | programs | temp | other
    pub dominant: String,
    /// system | user | regenerable | normal
    pub class: String,
    pub age_days: Option<u64>,
}

const KINDS: [&str; 7] = ["media", "docs", "code", "archives", "programs", "temp", "other"];

fn kind_index(ext: &str) -> usize {
    match ext {
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "webp" | "heic" | "mp4" | "mkv" | "avi"
        | "mov" | "mp3" | "wav" | "flac" | "aac" | "m4a" | "webm" => 0,
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "txt" | "md" | "epub"
        | "odt" | "csv" => 1,
        "rs" | "js" | "ts" | "py" | "java" | "c" | "cpp" | "h" | "cs" | "go" | "json"
        | "html" | "css" | "toml" | "yml" | "yaml" | "map" => 2,
        "zip" | "rar" | "7z" | "tar" | "gz" | "iso" => 3,
        "exe" | "dll" | "msi" | "sys" | "node" | "lib" => 4,
        "tmp" | "cache" | "log" | "bak" | "etl" | "dmp" => 5,
        _ => 6,
    }
}

fn profile(path: &Path) -> (u64, u64, [u64; 7]) {
    let mut size = 0u64;
    let mut files = 0u64;
    let mut hist = [0u64; 7];
    for e in WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if e.file_type().is_file() {
            if let Ok(m) = e.metadata() {
                let ext = e
                    .path()
                    .extension()
                    .map(|x| x.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                hist[kind_index(&ext)] += m.len();
                size += m.len();
                files += 1;
            }
        }
    }
    (size, files, hist)
}

fn classify(path: &Path, name: &str) -> String {
    let n = name.to_lowercase();
    if matches!(
        n.as_str(),
        "node_modules" | "target" | "__pycache__" | ".cache" | ".gradle" | "venv" | ".venv" | "dist" | "build"
    ) {
        return "regenerable".to_string();
    }
    if safety::is_protected(path) {
        return "system".to_string();
    }
    if matches!(
        n.as_str(),
        "documents" | "desktop" | "pictures" | "videos" | "music" | "downloads"
    ) {
        return "user".to_string();
    }
    "normal".to_string()
}

pub fn overview(root: &Path) -> Vec<FolderInfo> {
    use rayon::prelude::*;
    let children: Vec<std::path::PathBuf> = match std::fs::read_dir(root) {
        Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| e.path()).collect(),
        Err(_) => return Vec::new(),
    };
    let mut out: Vec<FolderInfo> = children
        .par_iter()
        .filter_map(|p| {
            let meta = std::fs::symlink_metadata(p).ok()?;
            if safety::is_reparse(&meta) {
                return None;
            }
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if meta.is_dir() {
                let (size, files, hist) = profile(p);
                let mut best = 6usize;
                let mut best_v = 0u64;
                for (i, v) in hist.iter().enumerate() {
                    if *v > best_v {
                        best_v = *v;
                        best = i;
                    }
                }
                Some(FolderInfo {
                    class: classify(p, &name),
                    name,
                    path: p.to_string_lossy().to_string(),
                    is_dir: true,
                    size,
                    files,
                    dominant: KINDS[best].to_string(),
                    age_days: crate::scanner::age_days(p),
                })
            } else {
                let ext = p
                    .extension()
                    .map(|x| x.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                Some(FolderInfo {
                    class: "normal".to_string(),
                    name,
                    path: p.to_string_lossy().to_string(),
                    is_dir: false,
                    size: meta.len(),
                    files: 1,
                    dominant: KINDS[kind_index(&ext)].to_string(),
                    age_days: crate::scanner::age_days(p),
                })
            }
        })
        .collect();
    out.sort_by(|a, b| b.size.cmp(&a.size));
    out
}
