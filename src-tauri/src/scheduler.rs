//! Simple in-app scheduler. Tasks live in tasks.json and run while the app is open.
//! Overdue tasks run once when the app starts. Only a fixed set of safe actions exists.

use crate::{cleaner, files, scanner, sysinfo};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::Emitter;

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

#[derive(Serialize, Deserialize, Clone)]
pub struct Task {
    pub id: String,
    pub title: String,
    /// clean_safe | organize | reminder
    pub kind: String,
    pub param: String,
    pub next_run: u64,
    pub repeat_hours: u64,
    pub last_result: Option<String>,
    pub enabled: bool,
}

fn tasks_path() -> PathBuf {
    crate::agent::base_dir().join("tasks.json")
}

pub fn read_tasks() -> Vec<Task> {
    fs::read_to_string(tasks_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_tasks(v: &[Task]) {
    let _ = fs::create_dir_all(crate::agent::base_dir());
    if let Ok(s) = serde_json::to_string_pretty(v) {
        let _ = fs::write(tasks_path(), s);
    }
}

pub fn add_task(
    kind: &str,
    title: &str,
    param: &str,
    first_in_minutes: u64,
    repeat_hours: u64,
) -> Result<Task, String> {
    match kind {
        "clean_safe" => {}
        "organize" => {
            let p = Path::new(param);
            if !p.is_absolute() || !p.is_dir() || crate::safety::is_system_location(p) {
                return Err("مجلد الترتيب غير صالح".to_string());
            }
        }
        "reminder" => {
            if param.trim().is_empty() || param.chars().count() > 200 {
                return Err("نص التذكير فارغ أو طويل".to_string());
            }
        }
        _ => return Err("نوع المهمة غير معروف (clean_safe أو organize أو reminder)".to_string()),
    }
    if first_in_minutes > 60 * 24 * 60 || repeat_hours > 24 * 60 {
        return Err("مدة غير منطقية".to_string());
    }
    if title.trim().is_empty() {
        return Err("عنوان المهمة مطلوب".to_string());
    }
    let mut all = read_tasks();
    if all.len() >= 30 {
        return Err("عدد المهام كبير جداً".to_string());
    }
    let t = Task {
        id: format!("t{}-{}", now(), NONCE.fetch_add(1, Ordering::SeqCst)),
        title: title.trim().chars().take(80).collect(),
        kind: kind.to_string(),
        param: param.to_string(),
        next_run: now() + first_in_minutes * 60,
        repeat_hours,
        last_result: None,
        enabled: true,
    };
    all.push(t.clone());
    write_tasks(&all);
    Ok(t)
}

pub fn remove_task(id: &str) -> Result<(), String> {
    let mut all = read_tasks();
    let before = all.len();
    all.retain(|t| t.id != id);
    if all.len() == before {
        return Err("المهمة غير موجودة".to_string());
    }
    write_tasks(&all);
    Ok(())
}

fn run_task(t: &Task) -> String {
    match t.kind.as_str() {
        "clean_safe" => {
            let list = scanner::scan_rules();
            let mut freed = 0u64;
            for f in list.iter().filter(|f| f.risk == "safe" && !f.admin) {
                freed += cleaner::clean_finding(f).freed;
            }
            format!("تم تنظيف الكاش الآمن وتحرير {}", sysinfo::fmt_bytes(freed))
        }
        "organize" => {
            let p = PathBuf::from(&t.param);
            match files::plan_organize(&p) {
                Ok(text) => {
                    let id = text
                        .lines()
                        .next()
                        .and_then(|l| l.split(':').nth(1))
                        .map(|s| s.trim().to_string())
                        .unwrap_or_default();
                    files::apply_organize(&id).unwrap_or_else(|e| e)
                }
                Err(e) => e,
            }
        }
        "reminder" => format!("تذكير: {}", t.param),
        _ => "نوع مهمة غير معروف".to_string(),
    }
}

pub async fn run_loop(app: tauri::AppHandle) {
    loop {
        tokio::time::sleep(Duration::from_secs(30)).await;
        let n = now();
        let due: Vec<Task> = read_tasks()
            .into_iter()
            .filter(|t| t.enabled && t.next_run <= n)
            .collect();
        for t in due {
            let t2 = t.clone();
            let res = match tauri::async_runtime::spawn_blocking(move || run_task(&t2)).await {
                Ok(r) => r,
                Err(e) => format!("خطأ: {}", es(e)),
            };
            let mut all = read_tasks();
            if let Some(x) = all.iter_mut().find(|x| x.id == t.id) {
                x.last_result = Some(res.clone());
                if x.repeat_hours > 0 {
                    x.next_run = now() + x.repeat_hours * 3600;
                } else {
                    x.enabled = false;
                }
            }
            write_tasks(&all);
            let _ = app.emit(
                "task-ran",
                serde_json::json!({ "title": t.title, "result": res }),
            );
        }
    }
}
