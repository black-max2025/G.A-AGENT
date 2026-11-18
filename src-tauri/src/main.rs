#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent;
mod cleaner;
mod overview;
mod rules;
mod safety;
mod scanner;

use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use tauri::{Manager, State};

pub struct AppState {
    pub findings: Mutex<HashMap<String, scanner::Finding>>,
    pub agent: Mutex<agent::AgentProc>,
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

#[tauri::command]
fn home_dir() -> String {
    std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".to_string())
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
        ids.iter().filter_map(|i| g.get(i).cloned()).collect()
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

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            findings: Mutex::new(HashMap::new()),
            agent: Mutex::new(agent::AgentProc::default()),
        })
        .invoke_handler(tauri::generate_handler![
            home_dir,
            scan_system,
            scan_folder,
            folder_overview,
            clean,
            agent::agent_status,
            agent::agent_install_server,
            agent::agent_download_model,
            agent::agent_set_model,
            agent::agent_start,
            agent::agent_stop,
            agent::agent_analyze,
            agent::agent_chat
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
