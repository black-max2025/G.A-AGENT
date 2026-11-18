//! Reversible space saving.
//!  - "shrink": transparent Windows compression (compact.exe, LZX). Files stay in place and open normally.
//!  - "freeze": cold folders are packed into one verified zip next to the folder; originals are removed
//!    only after the archive is verified. Both are journaled and can be undone.

use crate::cleaner::purge_dir;
use crate::safety;
use crate::scanner::dir_stats;
use crate::sysinfo::physical_size;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

const MB: u64 = 1024 * 1024;
static NONCE: AtomicU64 = AtomicU64::new(1);

fn es<E: ToString>(e: E) -> String {
    e.to_string()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Clone, Serialize)]
pub struct Candidate {
    pub id: String,
    pub app: String,
    pub explain: String,
    pub path: String,
    /// "shrink" or "freeze"
    pub kind: String,
    pub size: u64,
    pub est_saved: u64,
    #[serde(skip)]
    pub target: PathBuf,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    pub id: String,
    pub kind: String,
    pub app: String,
    pub path: String,
    pub archive: Option<String>,
    pub before: u64,
    pub after: u64,
    pub time: u64,
    pub undone: bool,
}

#[derive(Serialize)]
pub struct ShrinkResult {
    pub id: String,
    pub app: String,
    pub ok: bool,
    pub saved: u64,
    pub note: String,
}

// ------------------------------------------------------------------ journal

fn journal_path() -> PathBuf {
    crate::agent::base_dir().join("journal.json")
}

pub fn read_journal() -> Vec<JournalEntry> {
    fs::read_to_string(journal_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_journal(v: &[JournalEntry]) {
    let _ = fs::create_dir_all(crate::agent::base_dir());
    if let Ok(s) = serde_json::to_string_pretty(v) {
        let _ = fs::write(journal_path(), s);
    }
}

fn add_journal(e: JournalEntry) {
    let mut v = read_journal();
    v.push(e);
    write_journal(&v);
}

fn new_id() -> String {
    format!("j{}-{}", now(), NONCE.fetch_add(1, Ordering::SeqCst))
}

// -------------------------------------------------------------- app knowledge

fn app_info(name: &str) -> (String, String) {
    let n = name.to_lowercase();
    let table: &[(&str, &str, &str)] = &[
        ("discord", "ديسكورد", "برنامج محادثة. تصغير ملفاته لا يغيّر طريقة عمله."),
        ("vs code", "محرر الأكواد VS Code", "محرر برمجة. يعمل كالمعتاد بعد التصغير."),
        ("spotify", "سبوتيفاي", "مشغّل موسيقى. ملفات البرنامج فقط تُصغَّر."),
        ("slack", "سلاك", "برنامج عمل جماعي. يعمل كالمعتاد."),
        ("teams", "مايكروسوفت تيمز", "برنامج اجتماعات. يعمل كالمعتاد."),
        ("zoom", "زوم", "برنامج اجتماعات. يعمل كالمعتاد."),
        ("postman", "بوستمان", "أداة مطورين. تعمل كالمعتاد."),
        ("steam", "ستيم", "منصة ألعاب. ملفات الألعاب قد لا تُضغط كثيراً."),
        ("epic", "إيبك جيمز", "منصة ألعاب."),
        ("adobe", "أدوبي", "برامج تصميم. ملفاتها تُضغط جيداً عادةً."),
        ("obs", "OBS للتسجيل", "برنامج تسجيل. يعمل كالمعتاد."),
        ("python", "بايثون", "لغة برمجة. ملفاتها تُضغط جيداً وتعمل كالمعتاد."),
        ("rustup", "أدوات لغة Rust", "أدوات مطورين تُضغط جيداً."),
        ("cargo", "حزم لغة Rust", "مكتبات مطورين تُضغط جيداً."),
        ("gradle", "أدوات Gradle", "أدوات بناء تُضغط جيداً."),
        ("nuget", "حزم NuGet", "مكتبات مطورين تُضغط جيداً."),
        (".android", "أدوات أندرويد", "أدوات مطورين وأجهزة افتراضية."),
        ("docker", "دوكر", "بيئات تطوير."),
        ("unity", "يونيتي", "محرك ألعاب. ملفاته تُضغط جيداً."),
        ("blender", "بلندر", "برنامج ثلاثي الأبعاد."),
        ("node", "Node.js", "أدوات مطورين تُضغط جيداً."),
    ];
    for (key, ar, ex) in table {
        if n.contains(key) {
            return (ar.to_string(), ex.to_string());
        }
    }
    (
        format!("برنامج: {}", name),
        "ملفات برنامج. التصغير آمن ويمكن التراجع عنه بضغطة.".to_string(),
    )
}

// ---------------------------------------------------------------- estimation

fn incompressible(ext: &str) -> bool {
    matches!(
        ext,
        "zip" | "rar" | "7z" | "gz" | "xz" | "bz2" | "jpg" | "jpeg" | "png" | "webp" | "gif"
            | "mp4" | "mkv" | "avi" | "mov" | "webm" | "mp3" | "aac" | "flac" | "m4a" | "ogg"
            | "pak" | "vpk" | "bik" | "iso" | "cab" | "msi"
    )
}

fn ratio_of(path: &Path) -> Option<f64> {
    use flate2::{write::DeflateEncoder, Compression};
    let mut f = fs::File::open(path).ok()?;
    let mut buf = vec![0u8; 128 * 1024];
    let n = f.read(&mut buf).ok()?;
    if n == 0 {
        return None;
    }
    let mut enc = DeflateEncoder::new(Vec::new(), Compression::fast());
    enc.write_all(&buf[..n]).ok()?;
    let out = enc.finish().ok()?;
    Some((out.len() as f64 / n as f64).min(1.0))
}

/// Estimate saved bytes by deflating a sample of files.
fn estimate_saved(dir: &Path, total: u64) -> u64 {
    let mut w_size = 0f64;
    let mut w_comp = 0f64;
    let mut n = 0u32;
    for e in WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !e.file_type().is_file() {
            continue;
        }
        let meta = match e.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.len() < 4096 || safety::is_offline(&meta) {
            continue;
        }
        let ext = e
            .path()
            .extension()
            .map(|x| x.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let ratio = if incompressible(&ext) {
            1.0
        } else {
            match ratio_of(e.path()) {
                Some(r) => r,
                None => continue,
            }
        };
        let size = meta.len() as f64;
        w_size += size;
        w_comp += size * ratio;
        n += 1;
        if n >= 300 {
            break;
        }
    }
    if w_size <= 0.0 {
        return 0;
    }
    let ratio = w_comp / w_size;
    ((total as f64) * (1.0 - ratio) * 0.9).max(0.0) as u64
}

pub fn physical_dir(dir: &Path) -> u64 {
    let mut t = 0u64;
    for e in WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if e.file_type().is_file() {
            let logical = e.metadata().map(|m| m.len()).unwrap_or(0);
            t += physical_size(e.path(), logical);
        }
    }
    t
}

fn contains_reparse(dir: &Path) -> bool {
    for e in WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if let Ok(m) = e.metadata() {
            if safety::is_reparse(&m) || safety::is_offline(&m) {
                return true;
            }
        }
    }
    false
}

fn newest_age_days(dir: &Path) -> Option<u64> {
    let mut newest: Option<SystemTime> = None;
    for e in WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if let Ok(m) = e.metadata() {
            if let Ok(t) = m.modified() {
                if newest.map(|n| t > n).unwrap_or(true) {
                    newest = Some(t);
                }
            }
        }
    }
    let d = SystemTime::now().duration_since(newest?).ok()?;
    Some(d.as_secs() / 86400)
}

// ----------------------------------------------------------------- discovery

fn children_of(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.filter_map(|e| e.ok()) {
            let p = e.path();
            if p.is_dir() {
                out.push(p);
            }
        }
    }
}

fn make_shrink(d: &Path) -> Option<Candidate> {
    if safety::is_protected(d) {
        return None;
    }
    let meta = fs::symlink_metadata(d).ok()?;
    if safety::is_reparse(&meta) || !meta.is_dir() {
        return None;
    }
    let (size, _) = dir_stats(d);
    if size < 300 * MB {
        return None;
    }
    let physical = physical_dir(d);
    if (physical as f64) < (size as f64) * 0.85 {
        return None; // already compressed
    }
    let saved = estimate_saved(d, size);
    if saved < 80 * MB {
        return None;
    }
    let name = d
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let (app, explain) = app_info(&name);
    Some(Candidate {
        id: String::new(),
        app,
        explain,
        path: d.to_string_lossy().to_string(),
        kind: "shrink".to_string(),
        size,
        est_saved: saved,
        target: d.to_path_buf(),
    })
}

fn make_freeze(d: &Path) -> Option<Candidate> {
    if safety::is_protected(d) {
        return None;
    }
    let meta = fs::symlink_metadata(d).ok()?;
    if safety::is_reparse(&meta) || !meta.is_dir() {
        return None;
    }
    let (size, _) = dir_stats(d);
    if size < 200 * MB {
        return None;
    }
    let age = newest_age_days(d)?;
    if age < 180 || contains_reparse(d) {
        return None;
    }
    let saved = estimate_saved(d, size);
    if saved < 50 * MB {
        return None;
    }
    let name = d
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    Some(Candidate {
        id: String::new(),
        app: format!("مجلد قديم: {}", name),
        explain: format!(
            "لم يُستخدم منذ {} يوماً. يُضغط في ملف واحد ويمكن استرجاعه كاملاً بضغطة (تتغير تواريخ الملفات).",
            age
        ),
        path: d.to_string_lossy().to_string(),
        kind: "freeze".to_string(),
        size,
        est_saved: saved,
        target: d.to_path_buf(),
    })
}

pub fn discover() -> Vec<Candidate> {
    use rayon::prelude::*;
    let mut shrink_dirs: Vec<PathBuf> = Vec::new();
    let mut cold_dirs: Vec<PathBuf> = Vec::new();

    if let Ok(la) = std::env::var("LOCALAPPDATA") {
        let la = PathBuf::from(la);
        children_of(&la.join("Programs"), &mut shrink_dirs);
        let skip = [
            "microsoft",
            "packages",
            "google",
            "mozilla",
            "bravesoftware",
            "temp",
            "ga-agent",
            "programs",
            "d3dscache",
            "nvidia",
            "amd",
            "comms",
            "connecteddevices",
            "publishers",
            "virtualstore",
            "history",
        ];
        let mut kids = Vec::new();
        children_of(&la, &mut kids);
        for k in kids {
            let name = k
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if !skip.contains(&name.as_str()) {
                shrink_dirs.push(k);
            }
        }
    }
    if let Ok(up) = std::env::var("USERPROFILE") {
        let up = PathBuf::from(up);
        for d in [".cargo", ".rustup", ".gradle", ".nuget", ".m2", ".android"] {
            let p = up.join(d);
            if p.is_dir() {
                shrink_dirs.push(p);
            }
        }
        for sub in ["Documents", "Desktop", "Downloads"] {
            children_of(&up.join(sub), &mut cold_dirs);
        }
    }

    let mut out: Vec<Candidate> = shrink_dirs
        .par_iter()
        .filter_map(|d| make_shrink(d))
        .collect();
    out.sort_by(|a, b| b.est_saved.cmp(&a.est_saved));
    out.truncate(15);

    let mut cold: Vec<Candidate> = cold_dirs
        .par_iter()
        .filter_map(|d| make_freeze(d))
        .collect();
    cold.sort_by(|a, b| b.est_saved.cmp(&a.est_saved));
    cold.truncate(10);
    out.extend(cold);
    out
}

// -------------------------------------------------------------------- shrink

fn run_compact(dir: &Path, undo: bool) -> Result<(), String> {
    let sets: Vec<Vec<&str>> = if undo {
        vec![
            vec!["/U", "/S", "/I", "/Q", "/EXE"],
            vec!["/U", "/S", "/I", "/Q"],
        ]
    } else {
        vec![vec!["/C", "/S", "/I", "/Q", "/EXE:LZX"]]
    };
    for args in sets {
        let mut cmd = Command::new("compact.exe");
        cmd.args(&args)
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        cmd.status().map_err(es)?;
    }
    Ok(())
}

fn shrink_one(c: &Candidate) -> Result<JournalEntry, String> {
    let dir = &c.target;
    if !dir.is_dir() || safety::is_protected(dir) {
        return Err("مسار غير مسموح".to_string());
    }
    let before = physical_dir(dir);
    run_compact(dir, false)?;
    let after = physical_dir(dir);
    let entry = JournalEntry {
        id: new_id(),
        kind: "shrink".to_string(),
        app: c.app.clone(),
        path: c.path.clone(),
        archive: None,
        before,
        after,
        time: now(),
        undone: false,
    };
    if after < before {
        add_journal(entry.clone());
    }
    Ok(entry)
}

// -------------------------------------------------------------------- freeze

fn build_zip(dir: &Path, archive: &Path) -> Result<(u64, u64), String> {
    let f = fs::File::create(archive).map_err(es)?;
    let mut zw = zip::ZipWriter::new(f);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .large_file(true);
    let mut files = 0u64;
    let mut bytes = 0u64;
    for e in WalkDir::new(dir)
        .follow_links(false)
        .min_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let rel = match e.path().strip_prefix(dir) {
            Ok(r) => r.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };
        if e.file_type().is_dir() {
            zw.add_directory(format!("{}/", rel), opts.clone())
                .map_err(es)?;
        } else if e.file_type().is_file() {
            zw.start_file(rel, opts.clone()).map_err(es)?;
            let mut src = fs::File::open(e.path()).map_err(es)?;
            let n = std::io::copy(&mut src, &mut zw).map_err(es)?;
            files += 1;
            bytes += n;
        }
    }
    zw.finish().map_err(es)?;
    Ok((files, bytes))
}

fn verify_zip(archive: &Path, expected_files: u64, expected_bytes: u64) -> Result<(), String> {
    let f = fs::File::open(archive).map_err(es)?;
    let mut ar = zip::ZipArchive::new(f).map_err(es)?;
    let mut files = 0u64;
    let mut bytes = 0u64;
    for i in 0..ar.len() {
        let mut file = ar.by_index(i).map_err(es)?;
        if file.is_dir() {
            continue;
        }
        let n = std::io::copy(&mut file, &mut std::io::sink()).map_err(es)?;
        files += 1;
        bytes += n;
    }
    if files != expected_files || bytes != expected_bytes {
        return Err("فشل التحقق من الأرشيف، لم يُحذف شيء".to_string());
    }
    Ok(())
}

fn freeze_one(c: &Candidate) -> Result<JournalEntry, String> {
    let dir = &c.target;
    if !dir.is_dir() || safety::is_protected(dir) {
        return Err("مسار غير مسموح".to_string());
    }
    let parent = dir
        .parent()
        .ok_or_else(|| "لا يوجد مجلد أب".to_string())?;
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let archive = parent.join(format!("{}.ga-frozen.zip", name));
    if archive.exists() {
        return Err("يوجد أرشيف بنفس الاسم مسبقاً".to_string());
    }
    if contains_reparse(dir) {
        return Err("المجلد يحتوي روابط خاصة أو ملفات سحابية؛ لن أجمّده".to_string());
    }
    let before = dir_stats(dir).0;
    let (files, bytes) = match build_zip(dir, &archive) {
        Ok(x) => x,
        Err(e) => {
            let _ = fs::remove_file(&archive);
            return Err(e);
        }
    };
    if let Err(e) = verify_zip(&archive, files, bytes) {
        let _ = fs::remove_file(&archive);
        return Err(e);
    }
    purge_dir(dir);
    let leftover = if dir.exists() { dir_stats(dir).0 } else { 0 };
    let arch_size = fs::metadata(&archive).map(|m| m.len()).unwrap_or(0);
    let entry = JournalEntry {
        id: new_id(),
        kind: "freeze".to_string(),
        app: c.app.clone(),
        path: c.path.clone(),
        archive: Some(archive.to_string_lossy().to_string()),
        before,
        after: arch_size + leftover,
        time: now(),
        undone: false,
    };
    add_journal(entry.clone());
    Ok(entry)
}

fn restore(e: &JournalEntry) -> Result<(), String> {
    let archive = PathBuf::from(
        e.archive
            .as_ref()
            .ok_or_else(|| "لا يوجد أرشيف".to_string())?,
    );
    let dir = PathBuf::from(&e.path);
    if safety::is_protected(&dir) {
        return Err("مسار غير مسموح".to_string());
    }
    let f = fs::File::open(&archive).map_err(es)?;
    let mut ar = zip::ZipArchive::new(f).map_err(es)?;
    fs::create_dir_all(&dir).map_err(es)?;
    let mut expected = 0u64;
    let mut done = 0u64;
    for i in 0..ar.len() {
        let mut file = ar.by_index(i).map_err(es)?;
        let rel = match file.enclosed_name() {
            Some(r) => r,
            None => continue,
        };
        let target = dir.join(rel);
        if file.is_dir() {
            fs::create_dir_all(&target).map_err(es)?;
            continue;
        }
        expected += 1;
        if let Some(p) = target.parent() {
            fs::create_dir_all(p).map_err(es)?;
        }
        let mut o = fs::File::create(&target).map_err(es)?;
        std::io::copy(&mut file, &mut o).map_err(es)?;
        done += 1;
    }
    drop(ar);
    if done != expected {
        return Err("لم يكتمل الاسترجاع، الأرشيف باقٍ".to_string());
    }
    fs::remove_file(&archive).map_err(es)?;
    Ok(())
}

// ----------------------------------------------------------------- public API

pub fn apply_many<F: Fn(usize, usize, &str)>(items: &[Candidate], progress: F) -> Vec<ShrinkResult> {
    let total = items.len();
    let mut out = Vec::new();
    for (i, c) in items.iter().enumerate() {
        progress(i, total, &c.app);
        let r = if c.kind == "freeze" {
            freeze_one(c)
        } else {
            shrink_one(c)
        };
        match r {
            Ok(e) => {
                let saved = e.before.saturating_sub(e.after);
                out.push(ShrinkResult {
                    id: c.id.clone(),
                    app: c.app.clone(),
                    ok: saved > 0,
                    saved,
                    note: if saved > 0 {
                        "تم — يمكن التراجع من السجل".to_string()
                    } else {
                        "لم يوفّر مساحة (الملفات مضغوطة أصلاً أو قيد الاستخدام)".to_string()
                    },
                });
            }
            Err(er) => out.push(ShrinkResult {
                id: c.id.clone(),
                app: c.app.clone(),
                ok: false,
                saved: 0,
                note: er,
            }),
        }
    }
    out
}

pub fn undo(id: &str) -> Result<JournalEntry, String> {
    let mut all = read_journal();
    let idx = all
        .iter()
        .position(|e| e.id == id && !e.undone)
        .ok_or_else(|| "العملية غير موجودة أو تم التراجع عنها".to_string())?;
    let entry = all[idx].clone();
    if entry.kind == "freeze" {
        restore(&entry)?;
    } else {
        let dir = PathBuf::from(&entry.path);
        if !dir.is_dir() || safety::is_protected(&dir) {
            return Err("المجلد غير موجود أو غير مسموح".to_string());
        }
        run_compact(&dir, true)?;
    }
    all[idx].undone = true;
    write_journal(&all);
    Ok(all[idx].clone())
}
