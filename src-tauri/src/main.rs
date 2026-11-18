#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent;
mod assistant;
mod assistant_ext;
mod cleaner;
mod files;
mod overview;
mod rules;
mod safety;
mod scanner;
mod scheduler;
mod shrink;
mod store;
mod sysinfo;
mod sysops;

use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use tauri::{Emitter, Manager, State};

pub struct AppState {
    pub findings: Mutex<HashMap<String, scanner::Finding>>,
    pub cands: Mutex<HashMap<String, shrink::Candidate>>,
    pub agent: Mutex<agent::AgentProc>,
    pub session: Mutex<assistant::Session>,
}

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn register(state: &AppState, list: &mut [scanner::Finding]) -> Result<(), String> {
    let mut g = state.findings.lock().map_err(|e| e.to_string())?;
    for f in list.iter_mut() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        f.id = format!("f{}", n);
        g.insert(f.id.clone(), f.clone());
    }
    Ok(())
}

fn register_cands(state: &AppState, list: &mut [shrink::Candidate]) -> Result<(), String> {
    let mut g = state.cands.lock().map_err(|e| e.to_string())?;
    for c in list.iter_mut() {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        c.id = format!("c{}", n);
        g.insert(c.id.clone(), c.clone());
    }
    Ok(())
}

#[tauri::command]
fn home_dir() -> String {
    std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".to_string())
}

#[tauri::command]
fn disk_info() -> Vec<sysinfo::DriveInfo> {
    sysinfo::drives()
}

#[tauri::command]
async fn scan_system(state: State<'_, AppState>) -> Result<Vec<scanner::Finding>, String> {
    let mut list = tauri::async_runtime::spawn_blocking(scanner::scan_rules)
        .await
        .map_err(|e| e.to_string())?;
    register(&state, &mut list)?;
    Ok(list)
}

#[tauri::command]
async fn scan_folder(
    state: State<'_, AppState>,
    root: String,
) -> Result<Vec<scanner::Finding>, String> {
    let p = PathBuf::from(&root);
    if !p.is_dir() {
        return Err("المسار غير موجود أو ليس مجلداً".to_string());
    }
    let mut list = tauri::async_runtime::spawn_blocking(move || scanner::scan_folder(&p))
        .await
        .map_err(|e| e.to_string())?;
    register(&state, &mut list)?;
    Ok(list)
}

#[derive(Serialize)]
struct Plan {
    cleanup: Vec<scanner::Finding>,
    shrink: Vec<shrink::Candidate>,
    drives: Vec<sysinfo::DriveInfo>,
}

/// One-click scan for the simple mode: caches + shrinkable programs + cold folders.
#[tauri::command]
async fn plan(state: State<'_, AppState>) -> Result<Plan, String> {
    let h1 = tauri::async_runtime::spawn_blocking(scanner::scan_rules);
    let h2 = tauri::async_runtime::spawn_blocking(shrink::discover);
    let mut cleanup = h1.await.map_err(|e| e.to_string())?;
    let mut cands = h2.await.map_err(|e| e.to_string())?;
    register(&state, &mut cleanup)?;
    register_cands(&state, &mut cands)?;
    Ok(Plan {
        cleanup,
        shrink: cands,
        drives: sysinfo::drives(),
    })
}

#[tauri::command]
async fn folder_overview(path: String) -> Result<Vec<overview::FolderInfo>, String> {
    let p = PathBuf::from(&path);
    if !p.is_dir() {
        return Err("المسار غير موجود أو ليس مجلداً".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || overview::overview(&p))
        .await
        .map_err(|e| e.to_string())
}

#[derive(Serialize)]
struct CleanReport {
    freed: u64,
    results: Vec<cleaner::CleanResult>,
}

#[tauri::command]
async fn clean(state: State<'_, AppState>, ids: Vec<String>) -> Result<CleanReport, String> {
    let items: Vec<scanner::Finding> = {
        let g = state.findings.lock().map_err(|e| e.to_string())?;
        let v: Vec<scanner::Finding> = ids.iter().filter_map(|i| g.get(i).cloned()).collect();
        v
    };
    let results = tauri::async_runtime::spawn_blocking(move || {
        items
            .iter()
            .map(cleaner::clean_finding)
            .collect::<Vec<cleaner::CleanResult>>()
    })
    .await
    .map_err(|e| e.to_string())?;
    let freed: u64 = results.iter().map(|r| r.freed).sum();
    {
        let mut g = state.findings.lock().map_err(|e| e.to_string())?;
        for r in &results {
            if r.ok {
                g.remove(&r.id);
            }
        }
    }
    Ok(CleanReport { freed, results })
}

#[derive(Serialize)]
struct ShrinkReport {
    saved: u64,
    results: Vec<shrink::ShrinkResult>,
}

#[tauri::command]
async fn shrink_apply(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    ids: Vec<String>,
) -> Result<ShrinkReport, String> {
    let items: Vec<shrink::Candidate> = {
        let g = state.cands.lock().map_err(|e| e.to_string())?;
        let v: Vec<shrink::Candidate> = ids.iter().filter_map(|i| g.get(i).cloned()).collect();
        v
    };
    let results = tauri::async_runtime::spawn_blocking(move || {
        shrink::apply_many(&items, |i, total, name| {
            let _ = app.emit(
                "shrink-progress",
                serde_json::json!({ "i": i, "total": total, "app": name }),
            );
        })
    })
    .await
    .map_err(|e| e.to_string())?;
    let saved: u64 = results.iter().map(|r| r.saved).sum();
    {
        let mut g = state.cands.lock().map_err(|e| e.to_string())?;
        for r in &results {
            if r.ok {
                g.remove(&r.id);
            }
        }
    }
    Ok(ShrinkReport { saved, results })
}

#[tauri::command]
fn journal_list() -> Vec<shrink::JournalEntry> {
    let mut v = shrink::read_journal();
    v.reverse();
    v
}

#[tauri::command]
async fn journal_undo(id: String) -> Result<shrink::JournalEntry, String> {
    tauri::async_runtime::spawn_blocking(move || shrink::undo(&id))
        .await
        .map_err(|e| e.to_string())?
}

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            findings: Mutex::new(HashMap::new()),
            cands: Mutex::new(HashMap::new()),
            agent: Mutex::new(agent::AgentProc::default()),
            session: Mutex::new(assistant::Session::default()),
        })
        .setup(|app| {
            let h = app.handle().clone();
            tauri::async_runtime::spawn(scheduler::run_loop(h));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            home_dir,
            disk_info,
            scan_system,
            scan_folder,
            plan,
            folder_overview,
            clean,
            shrink_apply,
            journal_list,
            journal_undo,
            agent::agent_status,
            agent::agent_install_server,
            agent::agent_import_server,
            agent::agent_download_model,
            agent::agent_set_model,
            agent::agent_set_data_dir,
            agent::agent_set_allow_commands,
            agent::agent_start,
            agent::agent_stop,
            agent::agent_analyze,
            assistant::agent_send,
            assistant::agent_confirm,
            assistant::agent_reset,
            assistant_ext::agent_plan,
            assistant_ext::memory_list,
            assistant_ext::memory_forget,
            assistant_ext::agent_set_domain,
            assistant_ext::tasks_list,
            assistant_ext::task_remove
        ])
        .build(tauri::generate_context!())
        .expect("error while building G.A AGENT");

    app.run(|app_handle, event| {
        if let tauri::RunEvent::Exit = event {
            if let Some(st) = app_handle.try_state::<AppState>() {
                if let Ok(mut g) = st.agent.lock() {
                    if let Some(c) = g.child.as_mut() {
                        let _ = c.kill();
                    }
                }
            }
        }
    });
}
