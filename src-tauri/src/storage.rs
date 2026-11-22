//! Storage Intelligence Engine.
//! Rust measures, compares, estimates and executes; the language model only talks to the user.
//! It reuses the existing modules (scanner, shrink = NTFS compression + vault freeze, dedupe =
//! hard links, cleaner, safety) and adds: drive/profile overview, biggest files and folders,
//! cross-drive duplicate detection, a plan with expected savings and risks, and an apply step
//! that reports expected vs. ACTUAL savings.

use crate::{cleaner, dedupe, profiles, safety, scanner, shrink, sysinfo, AppState};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::State;
use walkdir::WalkDir;

const MB: u64 = 1024 * 1024;

pub fn kind_of_ext(ext: &str) -> &'static str {
    match ext {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "heic" | "bmp" | "tif" | "tiff" | "mp4"
        | "mkv" | "avi" | "mov" | "wmv" | "webm" | "mp3" | "wav" | "flac" | "m4a" | "ogg" => {
            "media"
        }
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "txt" | "odt" | "csv" => "docs",
        "zip" | "rar" | "7z" | "gz" | "xz" | "bz2" | "tar" | "iso" | "cab" => "archives",
        "exe" | "msi" | "dll" | "sys" => "programs",
        "tmp" | "log" | "dmp" | "etl" | "bak" => "temp",
        "vhd" | "vhdx" | "vmdk" | "vdi" | "img" => "disk_images",
        _ => "other",
    }
}

fn ext_of(p: &Path) -> String {
    p.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

// ------------------------------------------------------------------ walking

fn skip_dir(e: &walkdir::DirEntry) -> bool {
    if e.depth() == 0 {
        return false;
    }
    let name = e.file_name().to_string_lossy().to_lowercase();
    if name == "system volume information" || name == "$recycle.bin" {
        return true;
    }
    if e.file_type().is_dir() {
        if let Ok(m) = e.metadata() {
            // junctions / symlinks are never followed (no loops, no leaving the folder)
            return safety::is_reparse(&m);
        }
    }
    false
}

/// Size of a folder, bounded in time; cloud placeholders and links are not counted or read.
pub fn dir_size_limited(root: &Path, max_secs: u64) -> (u64, bool) {
    let deadline = Instant::now() + Duration::from_secs(max_secs);
    let mut total = 0u64;
    let mut n = 0u64;
    for e in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !skip_dir(e))
        .filter_map(|e| e.ok())
    {
        n += 1;
        if n % 512 == 0 && Instant::now() > deadline {
            return (total, true);
        }
        if !e.file_type().is_file() {
            continue;
        }
        if let Ok(m) = e.metadata() {
            if !safety::is_reparse(&m) && !safety::is_offline(&m) {
                total += m.len();
            }
        }
    }
    (total, false)
}

#[derive(Serialize, Clone)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub is_dir: bool,
    pub kind: String,
    pub age_days: Option<u64>,
}

#[derive(Serialize)]
pub struct TopReport {
    pub root: String,
    pub total: u64,
    pub files: Vec<Entry>,
    pub folders: Vec<Entry>,
    pub truncated: bool,
}

/// Biggest files and folders under `root` (folders are counted at depth 1 and 2).
pub fn scan_top(
    root: &Path,
    n_files: usize,
    n_dirs: usize,
    max_secs: u64,
) -> Result<TopReport, String> {
    if !root.is_dir() {
        return Err("ERR_BAD_FOLDER".to_string());
    }
    let deadline = Instant::now() + Duration::from_secs(max_secs);
    let mut total = 0u64;
    let mut truncated = false;
    let mut heap: BinaryHeap<Reverse<(u64, String)>> = BinaryHeap::new();
    let mut dirs: HashMap<String, u64> = HashMap::new();
    let mut count = 0u64;
    for e in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !skip_dir(e))
        .filter_map(|e| e.ok())
    {
        count += 1;
        if (count % 512 == 0 && Instant::now() > deadline) || count > 2_500_000 {
            truncated = true;
            break;
        }
        if !e.file_type().is_file() {
            continue;
        }
        let Ok(m) = e.metadata() else { continue };
        if safety::is_reparse(&m) || safety::is_offline(&m) {
            continue;
        }
        let size = m.len();
        total += size;
        if let Ok(rel) = e.path().strip_prefix(root) {
            let comps: Vec<_> = rel.components().collect();
            if comps.len() >= 2 {
                let d1 = root.join(comps[0].as_os_str());
                *dirs.entry(d1.to_string_lossy().to_string()).or_insert(0) += size;
                if comps.len() >= 3 {
                    let d2 = d1.join(comps[1].as_os_str());
                    *dirs.entry(d2.to_string_lossy().to_string()).or_insert(0) += size;
                }
            }
        }
        heap.push(Reverse((size, e.path().to_string_lossy().to_string())));
        if heap.len() > n_files {
            heap.pop();
        }
    }
    let mut files: Vec<Entry> = heap
        .into_iter()
        .map(|Reverse((size, p))| {
            let path = PathBuf::from(&p);
            Entry {
                name: path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
                kind: kind_of_ext(&ext_of(&path)).to_string(),
                age_days: scanner::age_days(&path),
                path: p,
                size,
                is_dir: false,
            }
        })
        .collect();
    files.sort_by(|a, b| b.size.cmp(&a.size));
    let mut dv: Vec<(String, u64)> = dirs.into_iter().collect();
    dv.sort_by(|a, b| b.1.cmp(&a.1));
    dv.truncate(n_dirs);
    let folders = dv
        .into_iter()
        .map(|(p, size)| {
            let path = PathBuf::from(&p);
            Entry {
                name: path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
                age_days: scanner::age_days(&path),
                path: p,
                size,
                is_dir: true,
                kind: "folder".to_string(),
            }
        })
        .collect();
    Ok(TopReport {
        root: root.to_string_lossy().to_string(),
        total,
        files,
        folders,
        truncated,
    })
}

// ----------------------------------------------------------------- overview

#[derive(Serialize)]
pub struct DriveRep {
    pub drive: String,
    pub total: u64,
    pub free: u64,
    pub used: u64,
    pub used_pct: u32,
}

#[derive(Serialize)]
pub struct FolderRep {
    pub kind: String,
    pub path: String,
    pub redirected: bool,
    pub assumed: bool,
    pub exists: bool,
    pub size: u64,
}

#[derive(Serialize)]
pub struct ProfRep {
    pub name: String,
    pub path: String,
    pub drive: String,
    pub active: bool,
    pub current: bool,
    pub exists: bool,
    pub folders: Vec<FolderRep>,
}

#[derive(Serialize)]
pub struct Overview {
    pub drives: Vec<DriveRep>,
    pub profiles: Vec<ProfRep>,
}

pub fn drive_reports() -> Vec<DriveRep> {
    sysinfo::drives()
        .into_iter()
        .map(|d| {
            let used = d.total.saturating_sub(d.free);
            DriveRep {
                used_pct: if d.total > 0 {
                    ((used as f64 / d.total as f64) * 100.0).round() as u32
                } else {
                    0
                },
                drive: d.drive,
                total: d.total,
                free: d.free,
                used,
            }
        })
        .collect()
}

/// Drives, user profiles and the real size of their Known Folders (time-boxed).
pub fn overview(with_sizes: bool) -> Overview {
    let started = Instant::now();
    let mut profs = Vec::new();
    for p in profiles::discover(true) {
        let mut folders = Vec::new();
        for f in &p.folders {
            let measure = with_sizes
                && f.exists
                && f.kind != "appdata"
                && (p.active || p.current)
                && started.elapsed() < Duration::from_secs(40);
            let size = if measure {
                dir_size_limited(Path::new(&f.path), 8).0
            } else {
                0
            };
            folders.push(FolderRep {
                kind: f.kind.clone(),
                path: f.path.clone(),
                redirected: f.redirected,
                assumed: f.assumed,
                exists: f.exists,
                size,
            });
        }
        profs.push(ProfRep {
            name: p.name,
            path: p.path,
            drive: p.drive,
            active: p.active,
            current: p.current,
            exists: p.exists,
            folders,
        });
    }
    Overview {
        drives: drive_reports(),
        profiles: profs,
    }
}

// -------------------------------------------------------- cross-drive copies

#[derive(Serialize, Clone)]
pub struct CrossDup {
    pub size: u64,
    pub paths: Vec<String>,
}

fn sha256_file(p: &Path, deadline: Instant) -> Option<String> {
    let mut f = std::fs::File::open(p).ok()?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        if Instant::now() > deadline {
            return None;
        }
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Some(format!("{:x}", h.finalize()))
}

/// Identical files on DIFFERENT drives. Hard links cannot join two drives, so this is a report:
/// the user decides which copy to keep. SHA-256 confirms every match (size alone proves nothing).
pub fn cross_drive_duplicates(roots: &[String], max_secs: u64) -> Vec<CrossDup> {
    let deadline = Instant::now() + Duration::from_secs(max_secs);
    let mut by_size: HashMap<u64, Vec<String>> = HashMap::new();
    for r in roots {
        for e in WalkDir::new(r)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| !skip_dir(e))
            .filter_map(|e| e.ok())
        {
            if Instant::now() > deadline {
                break;
            }
            if !e.file_type().is_file() {
                continue;
            }
            let Ok(m) = e.metadata() else { continue };
            if m.len() < 16 * MB || safety::is_reparse(&m) || safety::is_offline(&m) {
                continue;
            }
            by_size
                .entry(m.len())
                .or_default()
                .push(e.path().to_string_lossy().to_string());
        }
    }
    let mut out = Vec::new();
    let mut hashed = 0u64;
    for (size, paths) in by_size {
        let drives: std::collections::HashSet<String> =
            paths.iter().map(|p| profiles::drive_of(p)).collect();
        if paths.len() < 2 || drives.len() < 2 {
            continue; // same-drive copies are handled by hard-link dedupe
        }
        if hashed > 6 * 1024 * MB || Instant::now() > deadline {
            break;
        }
        let mut groups: HashMap<String, Vec<String>> = HashMap::new();
        for p in paths {
            hashed += size;
            if let Some(h) = sha256_file(Path::new(&p), deadline) {
                groups.entry(h).or_default().push(p);
            }
        }
        for (_, g) in groups {
            let ds: std::collections::HashSet<String> =
                g.iter().map(|p| profiles::drive_of(p)).collect();
            if g.len() >= 2 && ds.len() >= 2 {
                out.push(CrossDup { size, paths: g });
            }
        }
    }
    out.sort_by(|a, b| {
        (b.size * (b.paths.len() as u64 - 1)).cmp(&(a.size * (a.paths.len() as u64 - 1)))
    });
    out.truncate(30);
    out
}

// --------------------------------------------------------------------- plan

#[derive(Serialize, Clone)]
pub struct PlanItem {
    pub id: String,
    /// cleanup | ntfs | vault | dedupe
    pub method: String,
    pub title: String,
    pub path: String,
    pub size: u64,
    /// EXPECTED saving (an estimate, never presented as a result)
    pub expected: u64,
    /// low | medium
    pub risk: String,
    pub reversible: bool,
    pub selected: bool,
    #[serde(skip)]
    pub reference: String,
}

#[derive(Serialize, Clone)]
pub struct StoragePlan {
    pub id: String,
    pub drives: Vec<String>,
    pub items: Vec<PlanItem>,
    pub expected_total: u64,
    pub cross: Vec<CrossDup>,
    pub cross_saving: u64,
}

static PLANS: Mutex<Vec<StoragePlan>> = Mutex::new(Vec::new());

fn stamp() -> String {
    format!(
        "p{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    )
}

/// Risk of a method on a path: user data (profiles) is never "low".
pub fn risk_of(method: &str, path: &str) -> &'static str {
    let in_user = profiles::discover(false)
        .iter()
        .any(|p| profiles::is_inside(path, &p.path));
    match method {
        "cleanup" | "ntfs" if !in_user => "low",
        _ => "medium",
    }
}

fn drive_wanted(drives: &[String], path: &str) -> bool {
    drives.is_empty()
        || drives
            .iter()
            .any(|d| d.eq_ignore_ascii_case(&profiles::drive_of(path)))
}

pub fn build_plan(state: &AppState, drives: Vec<String>) -> Result<StoragePlan, String> {
    let drives: Vec<String> = drives
        .into_iter()
        .map(|d| {
            let d = d.trim().trim_end_matches('\\').to_uppercase();
            if d.len() == 1 {
                format!("{}:", d)
            } else {
                d
            }
        })
        .collect();
    let mut items: Vec<PlanItem> = Vec::new();

    // 1. Smart cleanup (rules)
    let mut cleanup = scanner::scan_rules();
    crate::register(state, &mut cleanup)?;
    for f in cleanup
        .iter()
        .filter(|f| f.size > 0 && drive_wanted(&drives, &f.path))
    {
        let safe = f.risk == "safe";
        items.push(PlanItem {
            id: format!("i{}", items.len() + 1),
            method: "cleanup".to_string(),
            title: f.name.clone(),
            path: f.path.clone(),
            size: f.size,
            expected: f.size,
            risk: if safe { "low" } else { "medium" }.to_string(),
            reversible: !safe, // unsafe-class items go to the Recycle Bin
            selected: safe,
            reference: f.id.clone(),
        });
    }

    // 2. NTFS compression and vault freeze
    let mut cands = shrink::discover();
    crate::register_cands(state, &mut cands)?;
    for c in cands.iter().filter(|c| {
        c.est_saved > 0 && drive_wanted(&drives, &c.path) && safety::check_target(&c.target).is_ok()
    }) {
        let vault = c.kind == "freeze";
        let method = if vault { "vault" } else { "ntfs" };
        items.push(PlanItem {
            id: format!("i{}", items.len() + 1),
            method: method.to_string(),
            title: c.app.clone(),
            path: c.path.clone(),
            size: c.size,
            expected: c.est_saved,
            risk: risk_of(method, &c.path).to_string(),
            reversible: true,
            selected: !vault && risk_of(method, &c.path) == "low",
            reference: c.id.clone(),
        });
    }

    // 3. Duplicate intelligence (same drive: hard links)
    let mut roots: Vec<String> = Vec::new();
    for p in profiles::discover(false) {
        if !(p.current || p.active) {
            continue; // other people's profiles are only reported, never modified
        }
        for f in &p.folders {
            if matches!(
                f.kind.as_str(),
                "documents" | "downloads" | "pictures" | "videos" | "desktop" | "music"
            ) && f.exists
                && drive_wanted(&drives, &f.path)
                && !roots.contains(&f.path)
            {
                roots.push(f.path.clone());
            }
        }
    }
    roots.truncate(8);
    for r in &roots {
        if let Ok(s) = dedupe::scan_folder(Path::new(r)) {
            if s.saved > 0 {
                items.push(PlanItem {
                    id: format!("i{}", items.len() + 1),
                    method: "dedupe".to_string(),
                    title: r.clone(),
                    path: r.clone(),
                    size: 0,
                    expected: s.saved,
                    risk: "medium".to_string(),
                    reversible: true,
                    selected: false,
                    reference: s.plan_id,
                });
            }
        }
    }

    // 4. Copies across drives (report only)
    let drives_in_roots: std::collections::HashSet<String> =
        roots.iter().map(|r| profiles::drive_of(r)).collect();
    let cross = if drives_in_roots.len() >= 2 {
        cross_drive_duplicates(&roots, 90)
    } else {
        Vec::new()
    };
    let cross_saving = cross
        .iter()
        .map(|c| c.size * (c.paths.len() as u64 - 1))
        .sum();

    items.sort_by(|a, b| b.expected.cmp(&a.expected));
    let expected_total = items
        .iter()
        .filter(|i| i.selected)
        .map(|i| i.expected)
        .sum();
    let plan = StoragePlan {
        id: stamp(),
        drives,
        items,
        expected_total,
        cross,
        cross_saving,
    };
    let mut g = PLANS.lock().map_err(|e| e.to_string())?;
    g.push(plan.clone());
    while g.len() > 3 {
        g.remove(0);
    }
    Ok(plan)
}

// -------------------------------------------------------------------- apply

#[derive(Serialize, Clone)]
pub struct ItemOutcome {
    pub id: String,
    pub title: String,
    pub method: String,
    pub risk: String,
    pub reversible: bool,
    pub before: u64,
    pub after: u64,
    pub expected: u64,
    pub actual: u64,
    pub ok: bool,
    pub note: String,
}

#[derive(Serialize)]
pub struct ApplyReport {
    pub items: Vec<ItemOutcome>,
    pub expected_total: u64,
    /// sum of what each operation reported
    pub reported_actual: u64,
    /// change of free space measured on the drives before/after (may include other activity)
    pub measured_gain: i64,
    pub free_before: Vec<(String, u64)>,
    pub free_after: Vec<(String, u64)>,
    pub errors: Vec<String>,
}

fn free_map() -> Vec<(String, u64)> {
    sysinfo::drives()
        .into_iter()
        .map(|d| (d.drive, d.free))
        .collect()
}

pub fn apply_plan(
    state: &AppState,
    plan_id: &str,
    ids: Option<Vec<String>>,
) -> Result<ApplyReport, String> {
    let plan = {
        let g = PLANS.lock().map_err(|e| e.to_string())?;
        g.iter()
            .find(|p| p.id == plan_id)
            .cloned()
            .ok_or_else(|| "ERR_NO_PLAN".to_string())?
    };
    let chosen: Vec<PlanItem> = plan
        .items
        .iter()
        .filter(|i| match &ids {
            Some(v) => v.contains(&i.id),
            None => i.selected,
        })
        .cloned()
        .collect();
    if chosen.is_empty() {
        return Err("ERR_NOTHING_SELECTED".to_string());
    }
    let free_before = free_map();
    let mut out: Vec<ItemOutcome> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for it in &chosen {
        let (ok, actual, note) = run_item(state, it);
        if !ok {
            errors.push(format!("{}: {}", it.title, note));
        }
        out.push(ItemOutcome {
            id: it.id.clone(),
            title: it.title.clone(),
            method: it.method.clone(),
            risk: it.risk.clone(),
            reversible: it.reversible,
            before: it.size,
            after: if it.size > 0 {
                it.size.saturating_sub(actual)
            } else {
                0
            },
            expected: it.expected,
            actual,
            ok,
            note,
        });
    }
    let free_after = free_map();
    let gain: i64 = free_after
        .iter()
        .map(|(d, a)| {
            let b = free_before
                .iter()
                .find(|(x, _)| x == d)
                .map(|(_, v)| *v)
                .unwrap_or(*a);
            *a as i64 - b as i64
        })
        .sum();
    Ok(ApplyReport {
        expected_total: chosen.iter().map(|i| i.expected).sum(),
        reported_actual: out.iter().map(|o| o.actual).sum(),
        measured_gain: gain,
        free_before,
        free_after,
        errors,
        items: out,
    })
}

/// Executes ONE plan item. Every path is validated again right before the change.
fn run_item(state: &AppState, it: &PlanItem) -> (bool, u64, String) {
    match it.method.as_str() {
        "cleanup" => {
            let f = match state.findings.lock() {
                Ok(g) => g.get(&it.reference).cloned(),
                Err(_) => None,
            };
            let Some(f) = f else {
                return (false, 0, "ERR_NO_RECORD".to_string());
            };
            let r = cleaner::clean_finding(&f);
            (r.ok, r.freed, r.note)
        }
        "ntfs" | "vault" => {
            let c = match state.cands.lock() {
                Ok(g) => g.get(&it.reference).cloned(),
                Err(_) => None,
            };
            let Some(c) = c else {
                return (false, 0, "ERR_NO_RECORD".to_string());
            };
            if let Err(e) = safety::check_target(&c.target) {
                return (false, 0, e);
            }
            let r = shrink::apply_many(&[c], |_, _, _| {});
            match r.into_iter().next() {
                Some(x) => (x.ok, x.saved, x.note),
                None => (false, 0, "ERR_NO_RECORD".to_string()),
            }
        }
        "dedupe" => match dedupe::apply_counts(&it.reference) {
            Ok((_, saved)) => (true, saved, String::new()),
            Err(e) => (false, 0, e),
        },
        _ => (false, 0, "ERR_BAD_KIND".to_string()),
    }
}

// ------------------------------------------------------------- text for tools

fn b(n: u64) -> String {
    sysinfo::fmt_bytes(n)
}

pub fn overview_text() -> String {
    let o = overview(true);
    let mut s = String::from("DRIVES\n");
    for d in &o.drives {
        s += &format!(
            "{} total {} used {} ({}%) free {}\n",
            d.drive,
            b(d.total),
            b(d.used),
            d.used_pct,
            b(d.free)
        );
    }
    s += "USER PROFILES (each profile is separate; none is assumed to be a copy of another)\n";
    for p in &o.profiles {
        s += &format!(
            "{} at {} [{}{}]\n",
            p.name,
            p.path,
            if p.active { "active" } else { "not signed in" },
            if p.current { ", current user" } else { "" }
        );
        for f in p.folders.iter().filter(|f| f.kind != "appdata") {
            s += &format!(
                "  {}: {}{}{} {}\n",
                f.kind,
                f.path,
                if f.redirected {
                    " (moved outside the profile)"
                } else {
                    ""
                },
                if f.assumed { " (assumed location)" } else { "" },
                if f.size > 0 { b(f.size) } else { String::new() }
            );
        }
    }
    s
}

pub fn top_text(path: &str) -> Result<String, String> {
    let r = scan_top(Path::new(path), 15, 12, 60)?;
    let mut s = format!(
        "{}: {} scanned{}\nBIGGEST FOLDERS\n",
        r.root,
        b(r.total),
        if r.truncated {
            " (partial: time limit)"
        } else {
            ""
        }
    );
    for e in &r.folders {
        s += &format!("{} {}\n", b(e.size), e.path);
    }
    s += "BIGGEST FILES\n";
    for e in &r.files {
        s += &format!(
            "{} {} [{}{}]\n",
            b(e.size),
            e.path,
            e.kind,
            e.age_days
                .map(|d| format!(", {} days old", d))
                .unwrap_or_default()
        );
    }
    Ok(s)
}

pub fn plan_text(p: &StoragePlan) -> String {
    let mut s = format!(
        "plan_id={} (expected savings are ESTIMATES until applied)\n",
        p.id
    );
    for i in &p.items {
        s += &format!(
            "{} [{}] {} expected {} risk {}{}{}\n",
            i.id,
            i.method,
            i.title,
            b(i.expected),
            i.risk,
            if i.reversible { ", reversible" } else { "" },
            if i.selected { ", recommended" } else { "" }
        );
    }
    s += &format!(
        "expected total of recommended items: {}\n",
        b(p.expected_total)
    );
    if !p.cross.is_empty() {
        s += &format!(
            "identical copies on different drives: {} groups, about {} if one copy of each is removed by the user (hard links cannot join two drives)\n",
            p.cross.len(),
            b(p.cross_saving)
        );
    }
    s
}

pub fn report_text(r: &ApplyReport) -> String {
    let mut s = String::new();
    for i in &r.items {
        s += &format!(
            "{} [{}] before {} after {} expected {} actual {} {} {}\n",
            i.title,
            i.method,
            b(i.before),
            b(i.after),
            b(i.expected),
            b(i.actual),
            if i.ok { "OK" } else { "FAILED" },
            i.note
        );
    }
    s += &format!(
        "expected total {} | reported actual {} | measured free-space change {}\n",
        b(r.expected_total),
        b(r.reported_actual),
        if r.measured_gain >= 0 {
            b(r.measured_gain as u64)
        } else {
            format!("-{}", b((-r.measured_gain) as u64))
        }
    );
    if !r.errors.is_empty() {
        s += &format!("errors: {}\n", r.errors.join("; "));
    }
    s += "Everything marked reversible can be undone from the undo history.";
    s
}

// ----------------------------------------------------------------- commands

#[tauri::command(async)]
pub fn storage_overview(with_sizes: bool) -> Overview {
    overview(with_sizes)
}

#[tauri::command(async)]
pub fn storage_top(path: String) -> Result<TopReport, String> {
    scan_top(Path::new(&path), 20, 15, 60)
}

#[tauri::command(async)]
pub fn storage_plan(
    state: State<'_, AppState>,
    drives: Vec<String>,
) -> Result<StoragePlan, String> {
    build_plan(&state, drives)
}

#[tauri::command(async)]
pub fn storage_apply(
    state: State<'_, AppState>,
    plan_id: String,
    ids: Vec<String>,
) -> Result<ApplyReport, String> {
    apply_plan(&state, &plan_id, Some(ids))
}

#[tauri::command(async)]
pub fn storage_cross_dups(plan_id: String) -> Vec<CrossDup> {
    PLANS
        .lock()
        .ok()
        .and_then(|g| g.iter().find(|p| p.id == plan_id).map(|p| p.cross.clone()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("ga_storage_test_{}_{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn already_compressed_types_are_not_compressed() {
        for e in ["zip", "jpg", "mp4", "7z", "mkv", "mp3"] {
            assert!(shrink::incompressible(e), "{} must be skipped", e);
        }
        assert!(!shrink::incompressible("txt"));
    }

    #[test]
    fn top_report_orders_files_by_size_and_counts_folders() {
        let d = tmp("top");
        fs::create_dir_all(d.join("a")).unwrap();
        fs::create_dir_all(d.join("b")).unwrap();
        fs::write(d.join("a").join("big.bin"), vec![1u8; 5000]).unwrap();
        fs::write(d.join("a").join("mid.bin"), vec![1u8; 3000]).unwrap();
        fs::write(d.join("b").join("small.txt"), vec![1u8; 10]).unwrap();
        let r = scan_top(&d, 2, 5, 10).unwrap();
        assert_eq!(r.total, 8010);
        assert_eq!(r.files.len(), 2);
        assert_eq!(r.files[0].size, 5000);
        assert_eq!(r.folders[0].size, 8000);
        assert!(!r.truncated);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn missing_folder_is_an_error_not_a_panic() {
        assert!(scan_top(Path::new("/no/such/folder/ga"), 3, 3, 1).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_loops_are_not_followed() {
        let d = tmp("loop");
        fs::create_dir_all(d.join("x")).unwrap();
        fs::write(d.join("x").join("f.bin"), vec![1u8; 100]).unwrap();
        std::os::unix::fs::symlink(&d, d.join("x").join("back")).unwrap();
        let r = scan_top(&d, 5, 5, 10).unwrap();
        assert_eq!(r.total, 100);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn identical_files_on_one_drive_are_not_reported_as_cross_drive() {
        let d = tmp("cross");
        fs::write(d.join("a.bin"), vec![7u8; 17 * 1024 * 1024]).unwrap();
        fs::write(d.join("b.bin"), vec![7u8; 17 * 1024 * 1024]).unwrap();
        let r = cross_drive_duplicates(&[d.to_string_lossy().to_string()], 20);
        assert!(r.is_empty());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn sha256_distinguishes_same_size_files() {
        let d = tmp("sha");
        fs::write(d.join("a"), vec![1u8; 4096]).unwrap();
        fs::write(d.join("b"), vec![2u8; 4096]).unwrap();
        let dl = Instant::now() + Duration::from_secs(5);
        assert_ne!(sha256_file(&d.join("a"), dl), sha256_file(&d.join("b"), dl));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn drive_filter_matches_letters_case_insensitively() {
        assert!(drive_wanted(&["C:".to_string()], "c:\\Users\\x"));
        assert!(!drive_wanted(&["D:".to_string()], "C:\\Users\\x"));
        assert!(drive_wanted(&[], "C:\\anything"));
    }

    #[test]
    fn reports_never_present_expected_as_actual() {
        let r = ApplyReport {
            items: vec![],
            expected_total: 100,
            reported_actual: 40,
            measured_gain: 30,
            free_before: vec![],
            free_after: vec![],
            errors: vec!["x: failed".to_string()],
        };
        let t = report_text(&r);
        assert!(t.contains("expected total") && t.contains("reported actual"));
        assert!(t.contains("errors"));
    }
}

/// Human description of what `storage_apply` is about to do (shown in the approval box).
pub fn describe_apply(plan_id: &str, ids: Option<Vec<String>>) -> String {
    let g = match PLANS.lock() {
        Ok(g) => g,
        Err(_) => return String::new(),
    };
    let Some(p) = g.iter().find(|p| p.id == plan_id) else {
        return "ERR_NO_PLAN".to_string();
    };
    let mut s = String::new();
    let mut total = 0u64;
    for i in p.items.iter().filter(|i| match &ids {
        Some(v) => v.contains(&i.id),
        None => i.selected,
    }) {
        total += i.expected;
        s += &format!(
            "• [{}] {} — expected {} — risk {}{}\n",
            i.method,
            i.title,
            b(i.expected),
            i.risk,
            if i.reversible { " — reversible" } else { "" }
        );
    }
    s += &format!("Expected total (estimate): {}", b(total));
    s
}
