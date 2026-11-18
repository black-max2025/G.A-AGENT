use crate::safety;
use crate::scanner::{dir_stats, Finding};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Serialize)]
pub struct CleanResult {
    pub id: String,
    pub name: String,
    pub ok: bool,
    pub freed: u64,
    pub note: String,
}

fn remove_file_force(p: &Path) {
    if fs::remove_file(p).is_ok() {
        return;
    }
    if let Ok(m) = fs::metadata(p) {
        let mut perm = m.permissions();
        perm.set_readonly(false);
        let _ = fs::set_permissions(p, perm);
    }
    let _ = fs::remove_file(p);
}

/// Delete everything inside `dir`, continuing past locked / denied entries.
/// Links are never followed (only the link itself is removed).
fn purge_contents(dir: &Path) {
    for e in WalkDir::new(dir)
        .follow_links(false)
        .contents_first(true)
        .min_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let p = e.path();
        let ft = e.file_type();
        if ft.is_dir() {
            let _ = fs::remove_dir(p);
        } else {
            remove_file_force(p);
            if ft.is_symlink() {
                let _ = fs::remove_dir(p);
            }
        }
    }
}

fn recycle(t: &Path, whole: bool) {
    if whole {
        let _ = trash::delete(t);
    } else {
        let kids: Vec<PathBuf> = fs::read_dir(t)
            .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect())
            .unwrap_or_default();
        if !kids.is_empty() {
            let _ = trash::delete_all(kids);
        }
    }
}

pub fn clean_finding(f: &Finding) -> CleanResult {
    let mut skipped_unsafe = false;
    for t in &f.targets {
        if !safety::is_sane_target(t) || (f.strict && safety::is_protected(t)) {
            skipped_unsafe = true;
            continue;
        }
        // Never act through a link/junction.
        if let Ok(m) = fs::symlink_metadata(t) {
            if safety::is_reparse(&m) {
                skipped_unsafe = true;
                continue;
            }
        } else {
            continue; // already gone
        }
        let is_file = t.is_file();
        if f.risk == "safe" {
            if is_file {
                remove_file_force(t);
            } else {
                purge_contents(t);
                if f.whole {
                    let _ = fs::remove_dir(t);
                }
            }
        } else {
            recycle(t, f.whole || is_file);
        }
    }
    let after: u64 = f
        .targets
        .iter()
        .map(|t| if t.exists() { dir_stats(t).0 } else { 0 })
        .sum();
    let freed = f.size.saturating_sub(after);
    let note = if skipped_unsafe {
        "تم تخطي عناصر محمية".to_string()
    } else if after == 0 {
        if f.risk == "safe" {
            "تم الحذف".to_string()
        } else {
            "نُقل إلى سلة المحذوفات (يمكن استرجاعه)".to_string()
        }
    } else {
        "بقي جزء قيد الاستخدام أو يحتاج صلاحية مدير".to_string()
    };
    CleanResult {
        id: f.id.clone(),
        name: f.name.clone(),
        ok: freed > 0 || f.size == 0,
        freed,
        note,
    }
}
