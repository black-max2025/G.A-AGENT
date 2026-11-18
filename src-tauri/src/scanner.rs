use crate::rules::{expand, Rule, RULES};
use crate::safety;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use walkdir::WalkDir;

const MB: u64 = 1024 * 1024;

#[derive(Clone, Serialize)]
pub struct Finding {
    pub id: String,
    pub name: String,
    pub category: String,
    /// "safe" (permanent delete) or "review" (Recycle Bin)
    pub risk: String,
    pub reason: String,
    pub path: String,
    pub size: u64,
    pub files: u64,
    pub age_days: Option<u64>,
    pub admin: bool,
    #[serde(skip)]
    pub targets: Vec<PathBuf>,
    /// true: the target itself is removed; false: only its contents
    #[serde(skip)]
    pub whole: bool,
    /// true: heuristic finding, extra protection checks apply
    #[serde(skip)]
    pub strict: bool,
}

pub fn dir_stats(path: &Path) -> (u64, u64) {
    let mut size = 0u64;
    let mut files = 0u64;
    for e in WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if e.file_type().is_file() {
            if let Ok(m) = e.metadata() {
                size += m.len();
                files += 1;
            }
        }
    }
    (size, files)
}

fn age_of(meta: &std::fs::Metadata) -> Option<u64> {
    let m = meta.modified().ok()?;
    let d = SystemTime::now().duration_since(m).ok()?;
    Some(d.as_secs() / 86400)
}

pub fn age_days(path: &Path) -> Option<u64> {
    age_of(&std::fs::metadata(path).ok()?)
}

// ---------------------------------------------------------------- rules scan

pub fn scan_rules() -> Vec<Finding> {
    use rayon::prelude::*;
    let mut out: Vec<Finding> = RULES.par_iter().filter_map(scan_rule).collect();
    out.sort_by(|a, b| b.size.cmp(&a.size));
    out
}

fn scan_rule(r: &Rule) -> Option<Finding> {
    let mut targets: Vec<PathBuf> = Vec::new();
    let opts = glob::MatchOptions {
        case_sensitive: false,
        require_literal_separator: false,
        require_literal_leading_dot: false,
    };
    for pat in r.patterns {
        let exp = match expand(pat) {
            Some(x) => x,
            None => continue,
        };
        if let Ok(paths) = glob::glob_with(&exp, opts) {
            for p in paths.flatten() {
                if let Ok(m) = std::fs::symlink_metadata(&p) {
                    if safety::is_reparse(&m) {
                        continue;
                    }
                    if !safety::is_sane_target(&p) {
                        continue;
                    }
                    if r.id == "user_temp" {
                        let last = p
                            .file_name()
                            .map(|n| n.to_string_lossy().to_lowercase())
                            .unwrap_or_default();
                        if !(last.contains("temp") || last.contains("tmp")) {
                            continue;
                        }
                    }
                    targets.push(p);
                }
            }
        }
    }
    if targets.is_empty() {
        return None;
    }
    let mut size = 0u64;
    let mut files = 0u64;
    for t in &targets {
        let (s, f) = dir_stats(t);
        size += s;
        files += f;
    }
    if size == 0 {
        return None;
    }
    let age = targets.iter().filter_map(|t| age_days(t)).min();
    Some(Finding {
        id: String::new(),
        name: r.name.to_string(),
        category: r.category.to_string(),
        risk: r.risk.to_string(),
        reason: r.reason.to_string(),
        path: targets[0].to_string_lossy().to_string(),
        size,
        files,
        age_days: age,
        admin: r.admin,
        targets,
        whole: false,
        strict: false,
    })
}

// ------------------------------------------------------------ heuristic scan

const SKIP_DIRS: &[&str] = &[
    "appdata",
    "windows",
    "program files",
    "program files (x86)",
    "programdata",
    "$recycle.bin",
    "system volume information",
    ".git",
];

fn hash_file(p: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = std::fs::File::open(p).ok()?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Some(format!("{:x}", h.finalize()))
}

fn file_name_of(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn file_finding(
    path: PathBuf,
    size: u64,
    age: u64,
    name: String,
    category: &str,
    reason: String,
) -> Finding {
    Finding {
        id: String::new(),
        name,
        category: category.to_string(),
        risk: "review".to_string(),
        reason,
        path: path.to_string_lossy().to_string(),
        size,
        files: 1,
        age_days: Some(age),
        admin: false,
        targets: vec![path],
        whole: true,
        strict: true,
    }
}

pub fn scan_folder(root: &Path) -> Vec<Finding> {
    let mut out: Vec<Finding> = Vec::new();
    let mut size_groups: HashMap<u64, Vec<PathBuf>> = HashMap::new();
    let mut big_old: Vec<(PathBuf, u64, u64)> = Vec::new();
    let mut installers: Vec<(PathBuf, u64, u64)> = Vec::new();
    let mut pyc_targets: Vec<PathBuf> = Vec::new();
    let mut pyc_size = 0u64;
    let mut pyc_files = 0u64;

    let mut it = WalkDir::new(root).follow_links(false).into_iter();
    loop {
        let entry = match it.next() {
            None => break,
            Some(Err(_)) => continue,
            Some(Ok(e)) => e,
        };
        let path = entry.path().to_path_buf();
        let ft = entry.file_type();

        if ft.is_dir() {
            if entry.depth() == 0 {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if SKIP_DIRS.contains(&name.as_str()) || name.starts_with("onedrive") {
                it.skip_current_dir();
                continue;
            }
            if safety::is_protected(&path) {
                it.skip_current_dir();
                continue;
            }
            let parent = path.parent().unwrap_or(&path).to_path_buf();

            if name == "__pycache__" {
                let (s, f) = dir_stats(&path);
                pyc_size += s;
                pyc_files += f;
                pyc_targets.push(path);
                it.skip_current_dir();
                continue;
            }

            let hit: Option<(&str, &str)> = match name.as_str() {
                "node_modules" if parent.join("package.json").exists() => Some((
                    "node_modules",
                    "حزم مشروع Node.js، تُعاد بأمر npm install. الأفضل لمشاريع لم تعد تعمل عليها.",
                )),
                "target" if parent.join("Cargo.toml").exists() => Some((
                    "مخرجات بناء Rust",
                    "ملفات بناء مشروع Rust، تُعاد بأمر cargo build.",
                )),
                ".venv" | "venv" if path.join("pyvenv.cfg").exists() => Some((
                    "بيئة بايثون افتراضية",
                    "بيئة افتراضية يمكن إعادة إنشائها من requirements.",
                )),
                ".gradle" if parent.join("build.gradle").exists() || parent.join("build.gradle.kts").exists() => Some((
                    "كاش Gradle للمشروع",
                    "كاش بناء المشروع، يُعاد تلقائياً.",
                )),
                _ => None,
            };
            if let Some((label, reason)) = hit {
                let (s, f) = dir_stats(&path);
                if s >= 20 * MB {
                    let age = age_days(&path).unwrap_or(0);
                    out.push(Finding {
                        id: String::new(),
                        name: format!("{} ({})", label, file_name_of(&parent)),
                        category: "مشاريع".to_string(),
                        risk: "review".to_string(),
                        reason: format!("{} آخر تعديل منذ {} يوماً.", reason, age),
                        path: path.to_string_lossy().to_string(),
                        size: s,
                        files: f,
                        age_days: Some(age),
                        admin: false,
                        targets: vec![path],
                        whole: true,
                        strict: true,
                    });
                }
                it.skip_current_dir();
            }
        } else if ft.is_file() {
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if safety::is_offline(&meta) {
                continue;
            }
            let size = meta.len();
            let age = age_of(&meta).unwrap_or(0);
            let ext = path
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if size >= 500 * MB && age >= 180 {
                big_old.push((path.clone(), size, age));
            }
            let in_downloads = path.components().any(|c| {
                c.as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case("downloads")
            });
            if in_downloads
                && matches!(ext.as_str(), "exe" | "msi" | "iso" | "msix")
                && age >= 60
                && size >= 5 * MB
            {
                installers.push((path.clone(), size, age));
            }
            if size >= MB && size <= 2048 * MB {
                size_groups.entry(size).or_default().push(path);
            }
        }
    }

    if pyc_size > 0 {
        out.push(Finding {
            id: String::new(),
            name: "كاش بايثون (__pycache__)".to_string(),
            category: "مشاريع".to_string(),
            risk: "safe".to_string(),
            reason: "ملفات bytecode تُعاد تلقائياً عند تشغيل البرامج.".to_string(),
            path: root.to_string_lossy().to_string(),
            size: pyc_size,
            files: pyc_files,
            age_days: None,
            admin: false,
            targets: pyc_targets,
            whole: true,
            strict: true,
        });
    }

    big_old.sort_by(|a, b| b.1.cmp(&a.1));
    big_old.truncate(25);
    for (p, size, age) in big_old {
        let name = file_name_of(&p);
        out.push(file_finding(
            p,
            size,
            age,
            format!("ملف كبير قديم: {}", name),
            "ملفات كبيرة",
            format!("حجمه كبير ولم يُعدَّل منذ {} يوماً. قرّر أنت إن كنت تحتاجه.", age),
        ));
    }

    installers.sort_by(|a, b| b.1.cmp(&a.1));
    installers.truncate(30);
    for (p, size, age) in installers {
        let name = file_name_of(&p);
        out.push(file_finding(
            p,
            size,
            age,
            format!("مثبّت قديم: {}", name),
            "مثبّتات",
            format!("ملف تثبيت في التنزيلات منذ {} يوماً، غالباً تم استخدامه.", age),
        ));
    }

    // Duplicates: same size, then same SHA-256.
    let mut groups: Vec<(u64, Vec<PathBuf>)> = size_groups
        .into_iter()
        .filter(|(_, v)| v.len() >= 2)
        .collect();
    groups.sort_by(|a, b| b.0.cmp(&a.0));
    groups.truncate(300);
    let mut dups: Vec<Finding> = Vec::new();
    for (size, paths) in groups {
        let mut by_hash: HashMap<String, Vec<PathBuf>> = HashMap::new();
        for p in paths {
            if let Some(h) = hash_file(&p) {
                by_hash.entry(h).or_default().push(p);
            }
        }
        for (_, mut group) in by_hash {
            if group.len() < 2 {
                continue;
            }
            group.sort_by_key(|p| {
                std::fs::metadata(p)
                    .and_then(|m| m.modified())
                    .unwrap_or(SystemTime::now())
            });
            let keep = group.remove(0);
            let extra = group.len() as u64;
            let name = file_name_of(&keep);
            dups.push(Finding {
                id: String::new(),
                name: format!("نسخ مكررة: {}", name),
                category: "مكررات".to_string(),
                risk: "review".to_string(),
                reason: format!(
                    "{} نسخة إضافية مطابقة تماماً. سيُبقى الأقدم: {}",
                    extra,
                    keep.to_string_lossy()
                ),
                path: group[0].to_string_lossy().to_string(),
                size: size * extra,
                files: extra,
                age_days: None,
                admin: false,
                targets: group,
                whole: true,
                strict: true,
            });
        }
    }
    dups.sort_by(|a, b| b.size.cmp(&a.size));
    dups.truncate(40);
    out.extend(dups);

    out.sort_by(|a, b| b.size.cmp(&a.size));
    out
}
