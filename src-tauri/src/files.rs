//! File and document helpers: reversible folder organizing, document reading, content search,
//! and HTML-to-text conversion (offline only).

use crate::safety;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::SystemTime;
use walkdir::WalkDir;

fn es<E: ToString>(e: E) -> String {
    e.to_string()
}

#[derive(Clone)]
struct OrganizePlan {
    id: String,
    root: PathBuf,
    moves: Vec<(PathBuf, PathBuf)>,
}

static PLANS: Mutex<Vec<OrganizePlan>> = Mutex::new(Vec::new());
static PLAN_N: AtomicU64 = AtomicU64::new(1);

fn category(ext: &str) -> Option<&'static str> {
    match ext {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "heic" | "svg" | "tif" | "tiff" => {
            Some("صور")
        }
        "mp4" | "mkv" | "avi" | "mov" | "wmv" | "webm" | "flv" => Some("فيديو"),
        "mp3" | "wav" | "flac" | "aac" | "m4a" | "ogg" | "wma" => Some("صوت"),
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "txt" | "md" | "csv" | "odt"
        | "epub" | "rtf" => Some("مستندات"),
        "zip" | "rar" | "7z" | "tar" | "gz" | "iso" => Some("أرشيفات"),
        "exe" | "msi" | "msix" => Some("برامج"),
        _ => None,
    }
}

fn unique_dest(dir: &Path, name: &str, taken: &HashSet<PathBuf>) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() && !taken.contains(&first) {
        return first;
    }
    let p = Path::new(name);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = p
        .extension()
        .map(|s| format!(".{}", s.to_string_lossy()))
        .unwrap_or_default();
    for n in 1..1000 {
        let c = dir.join(format!("{} ({}){}", stem, n, ext));
        if !c.exists() && !taken.contains(&c) {
            return c;
        }
    }
    first
}

fn counts_text(moves: &[(PathBuf, PathBuf)], root: &Path) -> String {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (_, dst) in moves {
        let cat = dst
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        *counts.entry(cat).or_insert(0) += 1;
    }
    let mut cats: Vec<(String, usize)> = counts.into_iter().collect();
    cats.sort_by(|a, b| b.1.cmp(&a.1));
    let mut s = format!("المجلد: {}\nسيُنقل {} ملفاً:\n", root.display(), moves.len());
    for (c, n) in cats {
        s.push_str(&format!("- {}: {} ملف\n", c, n));
    }
    s
}

/// Builds (does not apply) a plan that groups loose files of a folder by type.
pub fn plan_organize(root: &Path) -> Result<String, String> {
    if !root.is_dir() {
        return Err("المجلد غير موجود".to_string());
    }
    if safety::is_system_location(root) {
        return Err("هذا مكان نظام ولن أرتّبه".to_string());
    }
    let owned = safety::owned_paths();
    let mut moves: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut taken: HashSet<PathBuf> = HashSet::new();
    for e in fs::read_dir(root).map_err(es)?.filter_map(|e| e.ok()) {
        if moves.len() >= 300 {
            break;
        }
        let p = e.path();
        let m = match e.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !m.is_file() || safety::is_reparse(&m) || safety::is_offline(&m) {
            continue;
        }
        if safety::is_owned_by(&owned, &p) {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        let lower = name.to_lowercase();
        if lower.starts_with('.')
            || lower.ends_with(".part")
            || lower.ends_with(".crdownload")
            || lower.ends_with(".tmp")
            || lower.ends_with(".ini")
            || lower.ends_with(".lnk")
        {
            continue;
        }
        if let Ok(t) = m.modified() {
            if let Ok(d) = SystemTime::now().duration_since(t) {
                if d.as_secs() < 3600 {
                    continue;
                }
            }
        }
        let ext = p
            .extension()
            .map(|x| x.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let cat = match category(&ext) {
            Some(c) => c,
            None => continue,
        };
        let dest = unique_dest(&root.join(cat), &name, &taken);
        taken.insert(dest.clone());
        moves.push((p, dest));
    }
    if moves.is_empty() {
        return Err("لا يوجد ما يستحق الترتيب في هذا المجلد".to_string());
    }
    let id = format!("o{}", PLAN_N.fetch_add(1, Ordering::SeqCst));
    let mut text = format!("معرّف الخطة: {}\n{}", id, counts_text(&moves, root));
    text.push_str("أمثلة:\n");
    for (src, dst) in moves.iter().take(5) {
        let a = src
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let b = dst
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        text.push_str(&format!("  {} ← {}\n", a, b));
    }
    let mut g = PLANS.lock().map_err(es)?;
    g.push(OrganizePlan {
        id,
        root: root.to_path_buf(),
        moves,
    });
    while g.len() > 5 {
        g.remove(0);
    }
    Ok(text)
}

pub fn plan_summary(id: &str) -> Option<String> {
    let g = PLANS.lock().ok()?;
    let p = g.iter().find(|p| p.id == id)?;
    Some(counts_text(&p.moves, &p.root))
}

pub fn apply_organize(id: &str) -> Result<String, String> {
    let plan = {
        let g = PLANS.lock().map_err(es)?;
        let found: Option<OrganizePlan> = g.iter().find(|p| p.id == id).cloned();
        found
    }
    .ok_or_else(|| "الخطة غير موجودة. أنشئها أولاً بـ plan_organize.".to_string())?;
    let mut done: Vec<(String, String)> = Vec::new();
    let mut failed = 0usize;
    for (src, dst) in &plan.moves {
        if !src.exists() || dst.exists() {
            failed += 1;
            continue;
        }
        if let Some(par) = dst.parent() {
            if fs::create_dir_all(par).is_err() {
                failed += 1;
                continue;
            }
        }
        if fs::rename(src, dst).is_ok() {
            done.push((
                src.to_string_lossy().to_string(),
                dst.to_string_lossy().to_string(),
            ));
        } else {
            failed += 1;
        }
    }
    let n = done.len();
    if n > 0 {
        crate::shrink::journal_move(
            &format!("ترتيب مجلد: {}", plan.root.display()),
            &plan.root.to_string_lossy(),
            done,
        );
    }
    if let Ok(mut g) = PLANS.lock() {
        g.retain(|p| p.id != id);
    }
    Ok(format!(
        "تم نقل {} ملف{} — يمكن التراجع من تبويب السجل",
        n,
        if failed > 0 {
            format!(" (تعذّر {})", failed)
        } else {
            String::new()
        }
    ))
}

// ---------------------------------------------------------- reading / search

fn is_text_ext(ext: &str) -> bool {
    matches!(
        ext,
        "txt"
            | "md"
            | "csv"
            | "json"
            | "log"
            | "xml"
            | "html"
            | "htm"
            | "py"
            | "js"
            | "ts"
            | "rs"
            | "java"
            | "c"
            | "cpp"
            | "h"
            | "cs"
            | "go"
            | "toml"
            | "yml"
            | "yaml"
            | "ini"
            | "cfg"
            | "css"
            | "sql"
            | "bat"
            | "ps1"
            | "sh"
    )
}

pub fn read_document(p: &Path, max_chars: usize) -> Result<String, String> {
    let low = p.to_string_lossy().to_lowercase();
    for bad in [
        "login data",
        "cookies",
        "id_rsa",
        "\\.ssh\\",
        "wallet.dat",
        "passwords",
        "\\credentials",
    ] {
        if low.contains(bad) {
            return Err("ملف حساس، لن أقرأه".to_string());
        }
    }
    let meta = fs::metadata(p).map_err(es)?;
    if !meta.is_file() {
        return Err("ليس ملفاً".to_string());
    }
    let ext = p
        .extension()
        .map(|x| x.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let text = if ext == "docx" {
        read_docx(p)?
    } else if ext == "pdf" {
        return Err("ملفات PDF غير مدعومة بعد. أستطيع قراءة txt وmd وdocx وcsv وjson.".to_string());
    } else if is_text_ext(&ext) || ext.is_empty() {
        if meta.len() > 5 * 1024 * 1024 {
            return Err("الملف كبير جداً".to_string());
        }
        let mut f = fs::File::open(p).map_err(es)?;
        let mut buf = vec![0u8; 300_000];
        let n = f.read(&mut buf).map_err(es)?;
        buf.truncate(n);
        if buf.iter().take(1024).any(|b| *b == 0) {
            return Err("ملف ثنائي وليس نصاً".to_string());
        }
        String::from_utf8_lossy(&buf).to_string()
    } else {
        return Err("نوع الملف غير مدعوم".to_string());
    };
    Ok(text.chars().take(max_chars).collect())
}

fn read_docx(p: &Path) -> Result<String, String> {
    let f = fs::File::open(p).map_err(es)?;
    let mut ar = zip::ZipArchive::new(f).map_err(es)?;
    let mut file = ar.by_name("word/document.xml").map_err(es)?;
    let mut xml = String::new();
    file.read_to_string(&mut xml).map_err(es)?;
    let xml = xml
        .replace("</w:p>", "\n")
        .replace("<w:tab/>", " ")
        .replace("<w:br/>", "\n");
    Ok(strip_tags(&xml, true))
}

pub fn search_in_files(root: &Path, needle: &str, limit: usize) -> Result<String, String> {
    if needle.trim().is_empty() {
        return Err("النص المطلوب البحث عنه فارغ".to_string());
    }
    let n = needle.to_lowercase();
    let mut out: Vec<String> = Vec::new();
    let mut visited = 0usize;
    for e in WalkDir::new(root)
        .max_depth(6)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !e.file_type().is_file() {
            continue;
        }
        let ext = e
            .path()
            .extension()
            .map(|x| x.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if !is_text_ext(&ext) {
            continue;
        }
        let meta = match e.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.len() > 1_000_000 || safety::is_offline(&meta) {
            continue;
        }
        visited += 1;
        if visited > 5000 {
            break;
        }
        let mut f = match fs::File::open(e.path()) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let mut buf = Vec::new();
        if f.read_to_end(&mut buf).is_err() {
            continue;
        }
        let text = String::from_utf8_lossy(&buf);
        for (i, line) in text.lines().enumerate() {
            if line.to_lowercase().contains(&n) {
                let snippet: String = line.trim().chars().take(120).collect();
                out.push(format!("{}:{}: {}", e.path().display(), i + 1, snippet));
                break;
            }
        }
        if out.len() >= limit {
            break;
        }
    }
    if out.is_empty() {
        return Ok("لا نتائج".to_string());
    }
    Ok(out.join("\n"))
}

// ------------------------------------------------------------- html -> text

fn remove_block(s: &str, tag: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let open = format!("<{}", tag);
    let close = format!("</{}>", tag);
    let mut out = String::new();
    let mut pos = 0usize;
    while let Some(i) = lower[pos..].find(&open) {
        let start = pos + i;
        out.push_str(&s[pos..start]);
        match lower[start..].find(&close) {
            Some(j) => pos = start + j + close.len(),
            None => {
                pos = s.len();
                break;
            }
        }
    }
    out.push_str(&s[pos..]);
    out
}

pub fn strip_tags(s: &str, keep_newlines: bool) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => {
                in_tag = true;
                out.push(' ');
            }
            '>' => in_tag = false,
            _ => {
                if !in_tag {
                    out.push(ch);
                }
            }
        }
    }
    let out = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'");
    if keep_newlines {
        out.lines()
            .map(|l| l.split_whitespace().collect::<Vec<&str>>().join(" "))
            .filter(|l| !l.is_empty())
            .collect::<Vec<String>>()
            .join("\n")
    } else {
        out.split_whitespace().collect::<Vec<&str>>().join(" ")
    }
}

pub fn html_to_text(html: &str) -> String {
    let mut s = html.to_string();
    for tag in ["script", "style", "noscript", "svg"] {
        s = remove_block(&s, tag);
    }
    strip_tags(&s, false)
}
