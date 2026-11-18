//! The "brain": a local GGUF model served by llama.cpp's `llama-server`.
//! Everything stays on the user's machine. The model is ADVISORY only:
//! it can suggest what to delete/keep, but the safety layer has the last word.

use crate::scanner::Finding;
use crate::AppState;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};
use walkdir::WalkDir;

pub const PORT: u16 = 18765;

#[derive(Default)]
pub struct AgentProc {
    pub child: Option<Child>,
}

fn es<E: ToString>(e: E) -> String {
    e.to_string()
}

pub fn base_dir() -> PathBuf {
    let b = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    b.join("GA-Agent")
}

fn models_dir() -> PathBuf {
    base_dir().join("models")
}

fn llama_dir() -> PathBuf {
    base_dir().join("llama")
}

fn config_path() -> PathBuf {
    base_dir().join("config.json")
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct Config {
    model_path: Option<String>,
}

fn load_config() -> Config {
    std::fs::read_to_string(config_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_config(c: &Config) -> Result<(), String> {
    std::fs::create_dir_all(base_dir()).map_err(es)?;
    let s = serde_json::to_string_pretty(c).map_err(es)?;
    std::fs::write(config_path(), s).map_err(es)
}

fn find_server() -> Option<PathBuf> {
    let d = llama_dir();
    if !d.exists() {
        return None;
    }
    for e in WalkDir::new(d).into_iter().filter_map(|e| e.ok()) {
        if e.file_type().is_file()
            && e.file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case("llama-server.exe")
        {
            return Some(e.path().to_path_buf());
        }
    }
    None
}

#[derive(Serialize)]
pub struct AgentStatus {
    pub server_installed: bool,
    pub model_path: Option<String>,
    pub model_exists: bool,
    pub running: bool,
    pub downloaded_models: Vec<String>,
}

#[tauri::command]
pub fn agent_status(state: State<'_, AppState>) -> AgentStatus {
    let cfg = load_config();
    let model_exists = cfg
        .model_path
        .as_ref()
        .map(|p| Path::new(p).exists())
        .unwrap_or(false);
    let running = match state.agent.lock() {
        Ok(mut g) => match g.child.as_mut() {
            Some(c) => matches!(c.try_wait(), Ok(None)),
            None => false,
        },
        Err(_) => false,
    };
    let mut downloaded = Vec::new();
    if let Ok(rd) = std::fs::read_dir(models_dir()) {
        for e in rd.filter_map(|e| e.ok()) {
            let p = e.path();
            if p.extension().map(|x| x == "gguf").unwrap_or(false) {
                downloaded.push(p.to_string_lossy().to_string());
            }
        }
    }
    AgentStatus {
        server_installed: find_server().is_some(),
        model_path: cfg.model_path,
        model_exists,
        running,
        downloaded_models: downloaded,
    }
}

async fn download_to(app: &AppHandle, url: &str, dest: &Path, label: &str) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .user_agent("GA-Agent/0.1")
        .build()
        .map_err(es)?;
    let mut resp = client.get(url).send().await.map_err(es)?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let total = resp.content_length().unwrap_or(0);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(es)?;
    }
    let tmp = dest.with_extension("part");
    let mut file = std::fs::File::create(&tmp).map_err(es)?;
    let mut got: u64 = 0;
    let mut last: u64 = 0;
    while let Some(chunk) = resp.chunk().await.map_err(es)? {
        file.write_all(&chunk).map_err(es)?;
        got += chunk.len() as u64;
        if got - last > 2_000_000 {
            last = got;
            let _ = app.emit(
                "download-progress",
                serde_json::json!({ "label": label, "got": got, "total": total }),
            );
        }
    }
    drop(file);
    std::fs::rename(&tmp, dest).map_err(es)?;
    let _ = app.emit(
        "download-progress",
        serde_json::json!({ "label": label, "got": got, "total": got }),
    );
    Ok(())
}

fn extract_zip(zip_path: &Path, out: &Path) -> Result<(), String> {
    let f = std::fs::File::open(zip_path).map_err(es)?;
    let mut ar = zip::ZipArchive::new(f).map_err(es)?;
    std::fs::create_dir_all(out).map_err(es)?;
    for i in 0..ar.len() {
        let mut file = ar.by_index(i).map_err(es)?;
        let rel = match file.enclosed_name() {
            Some(r) => r,
            None => continue,
        };
        let target = out.join(rel);
        if file.is_dir() {
            std::fs::create_dir_all(&target).map_err(es)?;
            continue;
        }
        if let Some(p) = target.parent() {
            std::fs::create_dir_all(p).map_err(es)?;
        }
        let mut o = std::fs::File::create(&target).map_err(es)?;
        std::io::copy(&mut file, &mut o).map_err(es)?;
    }
    Ok(())
}

/// Downloads the official llama.cpp Windows (CPU) build, which contains llama-server.exe.
#[tauri::command]
pub async fn agent_install_server(app: AppHandle) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .user_agent("GA-Agent/0.1")
        .build()
        .map_err(es)?;
    let rel: serde_json::Value = client
        .get("https://api.github.com/repos/ggml-org/llama.cpp/releases/latest")
        .send()
        .await
        .map_err(es)?
        .json()
        .await
        .map_err(es)?;
    let assets = rel["assets"]
        .as_array()
        .ok_or_else(|| "تعذر قراءة إصدارات llama.cpp".to_string())?;
    let mut named: Vec<(String, String)> = Vec::new();
    for a in assets {
        if let (Some(n), Some(u)) = (a["name"].as_str(), a["browser_download_url"].as_str()) {
            named.push((n.to_string(), u.to_string()));
        }
    }
    let pick = |needle: &str| -> Option<String> {
        named
            .iter()
            .find(|(n, _)| n.contains(needle) && n.ends_with(".zip"))
            .map(|(_, u)| u.clone())
    };
    let url = pick("bin-win-cpu-x64")
        .or_else(|| pick("bin-win-avx2-x64"))
        .ok_or_else(|| "لم أجد نسخة Windows المناسبة من llama.cpp".to_string())?;
    let zip_path = base_dir().join("llama.zip");
    download_to(&app, &url, &zip_path, "llama-server").await?;
    extract_zip(&zip_path, &llama_dir())?;
    let _ = std::fs::remove_file(&zip_path);
    find_server()
        .map(|p| p.to_string_lossy().to_string())
        .ok_or_else(|| "لم يُعثر على llama-server.exe بعد الاستخراج".to_string())
}

#[tauri::command]
pub async fn agent_download_model(
    app: AppHandle,
    url: String,
    file_name: String,
) -> Result<String, String> {
    let name = Path::new(&file_name)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .ok_or_else(|| "اسم ملف غير صالح".to_string())?;
    if !name.to_lowercase().ends_with(".gguf") {
        return Err("يجب أن يكون الملف بصيغة .gguf".to_string());
    }
    let dest = models_dir().join(&name);
    download_to(&app, &url, &dest, &name).await?;
    let p = dest.to_string_lossy().to_string();
    save_config(&Config {
        model_path: Some(p.clone()),
    })?;
    Ok(p)
}

#[tauri::command]
pub fn agent_set_model(path: String) -> Result<(), String> {
    let p = Path::new(&path);
    if !p.is_file() || !path.to_lowercase().ends_with(".gguf") {
        return Err("اختر ملف نموذج بصيغة .gguf موجود فعلاً".to_string());
    }
    save_config(&Config {
        model_path: Some(path),
    })
}

fn local_client(timeout_secs: u64) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(timeout_secs))
        .build()
        .map_err(es)
}

#[tauri::command]
pub async fn agent_start(state: State<'_, AppState>) -> Result<(), String> {
    {
        let mut g = state.agent.lock().map_err(es)?;
        let mut already = false;
        if let Some(c) = g.child.as_mut() {
            if matches!(c.try_wait(), Ok(None)) {
                already = true;
            }
        }
        if !already {
            let exe = find_server().ok_or_else(|| "خادم الذكاء الاصطناعي غير مثبّت بعد".to_string())?;
            let model = load_config()
                .model_path
                .ok_or_else(|| "لم يتم اختيار نموذج".to_string())?;
            if !Path::new(&model).exists() {
                return Err("ملف النموذج غير موجود".to_string());
            }
            let mut cmd = Command::new(&exe);
            cmd.arg("-m")
                .arg(&model)
                .arg("--host")
                .arg("127.0.0.1")
                .arg("--port")
                .arg(PORT.to_string())
                .arg("-c")
                .arg("4096");
            if let Some(dir) = exe.parent() {
                cmd.current_dir(dir);
            }
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0800_0000);
            }
            let child = cmd.spawn().map_err(es)?;
            g.child = Some(child);
        }
    }
    let client = local_client(5)?;
    let url = format!("http://127.0.0.1:{}/health", PORT);
    for _ in 0..360 {
        if let Ok(r) = client.get(&url).send().await {
            if r.status().is_success() {
                return Ok(());
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err("انتهت مهلة تحميل النموذج. جرّب نموذجاً أصغر.".to_string())
}

#[tauri::command]
pub fn agent_stop(state: State<'_, AppState>) -> Result<(), String> {
    let mut g = state.agent.lock().map_err(es)?;
    if let Some(mut c) = g.child.take() {
        let _ = c.kill();
        let _ = c.wait();
    }
    Ok(())
}

async fn complete(system: &str, user: &str, max_tokens: u32) -> Result<String, String> {
    let client = local_client(900)?;
    let body = serde_json::json!({
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user }
        ],
        "temperature": 0.2,
        "max_tokens": max_tokens
    });
    let v: serde_json::Value = client
        .post(format!("http://127.0.0.1:{}/v1/chat/completions", PORT))
        .json(&body)
        .send()
        .await
        .map_err(|_| "العقل غير مشغّل. شغّله من تبويب العقل أولاً.".to_string())?
        .json()
        .await
        .map_err(es)?;
    v["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "رد غير متوقع من النموذج".to_string())
}

fn describe(f: &Finding) -> String {
    format!(
        "{} | الاسم: {} | النوع: {} | الخطورة: {} | الحجم: {} MB | العمر: {} يوم | المسار: {}",
        f.id,
        f.name,
        f.category,
        if f.risk == "safe" { "كاش آمن" } else { "يحتاج مراجعة" },
        f.size / (1024 * 1024),
        f.age_days.map(|d| d.to_string()).unwrap_or_else(|| "?".to_string()),
        f.path
    )
}

#[derive(Serialize)]
pub struct Verdict {
    pub id: String,
    /// delete | keep | ask
    pub verdict: String,
    pub note: String,
}

const ANALYZE_SYSTEM: &str = "أنت G.A AGENT، خبير في نظام Windows وتنظيف التخزين بأمان. \
ستُعطى قائمة عناصر مرشحة للحذف. لكل عنصر قرّر: \
DELETE إن كان كاش أو ملفاً مؤقتاً يمكن إعادة توليده بلا خسارة، \
KEEP إن كان قد يحتوي بيانات شخصية أو مهمة أو غير قابلة للتعويض، \
ASK إن لم تكن متأكداً. كن متحفظاً: عند الشك اختر ASK. \
أجب بسطر واحد لكل عنصر وبهذا الشكل فقط: المعرّف|القرار|سبب قصير بالعربية (أقل من 12 كلمة). \
لا تكتب أي شيء آخر.";

#[tauri::command]
pub async fn agent_analyze(
    state: State<'_, AppState>,
    ids: Vec<String>,
) -> Result<Vec<Verdict>, String> {
    let items: Vec<(String, String)> = {
        let g = state.findings.lock().map_err(es)?;
        ids.iter()
            .filter_map(|id| g.get(id).map(|f| (id.clone(), describe(f))))
            .collect()
    };
    let mut verdicts: Vec<Verdict> = Vec::new();
    for batch in items.chunks(8) {
        let user = batch
            .iter()
            .map(|(_, d)| d.clone())
            .collect::<Vec<_>>()
            .join("\n");
        let reply = complete(ANALYZE_SYSTEM, &user, 700).await?;
        for line in reply.lines() {
            let parts: Vec<&str> = line.split('|').collect();
            if parts.len() < 2 {
                continue;
            }
            let id = parts[0]
                .trim()
                .trim_start_matches(|c: char| !c.is_alphanumeric())
                .to_string();
            if !batch.iter().any(|(bid, _)| *bid == id) {
                continue;
            }
            let v = parts[1].trim().to_uppercase();
            let verdict = if v.contains("DELETE") {
                "delete"
            } else if v.contains("KEEP") {
                "keep"
            } else {
                "ask"
            };
            let note = if parts.len() > 2 {
                parts[2..].join("|").trim().to_string()
            } else {
                String::new()
            };
            verdicts.push(Verdict {
                id,
                verdict: verdict.to_string(),
                note,
            });
        }
    }
    Ok(verdicts)
}

#[tauri::command]
pub async fn agent_chat(message: String, context: String) -> Result<String, String> {
    let system = format!(
        "أنت G.A AGENT، مساعد ذكي لتنظيف Windows بأمان. أجب بالعربية باختصار ووضوح. \
لا تقترح أبداً حذف ملفات النظام أو مستندات المستخدم الشخصية. \
هذه نتائج آخر فحص:\n{}",
        context
    );
    complete(&system, &message, 600).await
}
