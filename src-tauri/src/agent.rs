//! The "brain": a local GGUF model served by llama.cpp's `llama-server`.
//! Everything stays on the user's machine. The model is ADVISORY only:
//! it can suggest what to delete/keep, but the safety layer has the last word.

use crate::scanner::Finding;
use crate::AppState;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, State};
use walkdir::WalkDir;

/// Random port + secret token per run, so no other program can use the engine.
static RT: Mutex<(u16, String)> = Mutex::new((0, String::new()));
static LAST_USED: AtomicU64 = AtomicU64::new(0);

pub fn endpoint() -> (u16, String) {
    match RT.lock() {
        Ok(g) => g.clone(),
        Err(_) => (0, String::new()),
    }
}

pub fn touch() {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    LAST_USED.store(t, Ordering::Relaxed);
}

pub fn url(path: &str) -> String {
    format!("http://127.0.0.1:{}{}", endpoint().0, path)
}

#[derive(Default)]
pub struct AgentProc {
    pub child: Option<Child>,
    pub job: Option<crate::perf::Job>,
}

pub fn es<E: ToString>(e: E) -> String {
    e.to_string()
}

pub fn base_dir() -> PathBuf {
    let b = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    b.join("GA-Agent")
}

/// Where the engine and downloaded models live. Defaults to the app folder,
/// but the user can point it to any drive (e.g. D:\\) so C: is not used.
pub fn data_dir() -> PathBuf {
    match load_config().data_dir {
        Some(d) if !d.trim().is_empty() => PathBuf::from(d),
        _ => base_dir(),
    }
}

fn models_dir() -> PathBuf {
    data_dir().join("models")
}

fn llama_dir() -> PathBuf {
    data_dir().join("llama")
}

fn config_path() -> PathBuf {
    base_dir().join("config.json")
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Config {
    pub model_path: Option<String>,
    #[serde(default)]
    pub data_dir: Option<String>,
    #[serde(default)]
    pub allow_commands: bool,
    #[serde(default)]
    pub disabled_domains: Vec<String>,
    #[serde(default)]
    pub cpu_cap: Option<u32>,
    #[serde(default)]
    pub threads: Option<u32>,
    #[serde(default)]
    pub ctx: Option<u32>,
    #[serde(default)]
    pub idle_secs: Option<u64>,
    #[serde(default)]
    pub lang: Option<String>,
    #[serde(default)]
    pub batch: Option<u32>,
    #[serde(default)]
    pub ubatch: Option<u32>,
    /// Strict Offline Mode (default ON): no network function may run.
    #[serde(default)]
    pub offline: Option<bool>,
    /// Windows notifications (default ON).
    #[serde(default)]
    pub notify: Option<bool>,
}

impl Config {
    pub fn cap(&self) -> u32 {
        self.cpu_cap.unwrap_or(60).clamp(10, 100)
    }
    pub fn threads(&self) -> u32 {
        self.threads.unwrap_or(2).clamp(1, 8)
    }
    pub fn ctx(&self) -> u32 {
        self.ctx.unwrap_or(4096).clamp(2048, 16384)
    }
    pub fn idle(&self) -> u64 {
        self.idle_secs.unwrap_or(120).clamp(60, 7200)
    }
    pub fn offline(&self) -> bool {
        self.offline.unwrap_or(true)
    }
    pub fn batch(&self) -> u32 {
        self.batch.unwrap_or(128).clamp(32, 1024)
    }
    pub fn ubatch(&self) -> u32 {
        self.ubatch.unwrap_or(64).clamp(16, 512)
    }
    pub fn lang(&self) -> String {
        self.lang.clone().unwrap_or_else(|| "ar".to_string())
    }
}

pub fn load_config() -> Config {
    std::fs::read_to_string(config_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_config(c: &Config) -> Result<(), String> {
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
    pub data_dir: String,
    pub allow_commands: bool,
    pub disabled_domains: Vec<String>,
    pub server_installed: bool,
    pub model_path: Option<String>,
    pub model_exists: bool,
    pub running: bool,
    pub downloaded_models: Vec<String>,
    pub cpu_cap: u32,
    pub threads: u32,
    pub ctx: u32,
    pub idle_secs: u64,
    pub lang: String,
    pub batch: u32,
    pub ubatch: u32,
    pub offline: bool,
    pub cpu_features: String,
    pub engine_verified: bool,
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
        data_dir: data_dir().to_string_lossy().to_string(),
        allow_commands: cfg.allow_commands,
        disabled_domains: cfg.disabled_domains.clone(),
        server_installed: find_server().is_some(),
        model_path: cfg.model_path.clone(),
        model_exists,
        running,
        downloaded_models: downloaded,
        cpu_cap: cfg.cap(),
        threads: cfg.threads(),
        ctx: cfg.ctx(),
        idle_secs: cfg.idle(),
        lang: cfg.lang(),
        batch: cfg.batch(),
        ubatch: cfg.ubatch(),
        offline: cfg.offline(),
        cpu_features: cpu_features(),
        engine_verified: engine_verified(),
    }
}

/// The ONLY internet access in the whole app: a one-time download of the engine or a model,
/// started by the user, from these hosts only. The agent itself has no network tools.
fn host_allowed(u: &reqwest::Url) -> bool {
    if u.scheme() != "https" {
        return false;
    }
    let h = u.host_str().unwrap_or("").to_lowercase();
    const OK: [&str; 7] = [
        "huggingface.co",
        "hf.co",
        "github.com",
        "api.github.com",
        "githubusercontent.com",
        "cdn-lfs.huggingface.co",
        "xethub.hf.co",
    ];
    OK.iter()
        .any(|d| h == *d || h.ends_with(&format!(".{}", d)))
}

fn download_client() -> Result<reqwest::Client, String> {
    crate::netguard::guard()?;
    reqwest::Client::builder()
        .user_agent("GA-Agent")
        .redirect(reqwest::redirect::Policy::custom(|a| {
            if a.previous().len() > 8 || !host_allowed(a.url()) {
                a.stop()
            } else {
                a.follow()
            }
        }))
        .build()
        .map_err(es)
}

async fn download_to(app: &AppHandle, url: &str, dest: &Path, label: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(es)?;
    if !host_allowed(&parsed) {
        return Err(
            "هذا العنوان غير مسموح. التحميل مسموح فقط من مصادر النماذج والمحرك الرسمية."
                .to_string(),
        );
    }
    let client = download_client()?;
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
    install_server(&app).await
}

/// Makes sure the engine exists; downloads it only when it is missing.
async fn ensure_server(app: &AppHandle) -> Result<PathBuf, String> {
    if let Some(p) = find_server() {
        return Ok(p);
    }
    install_server(app).await.map(PathBuf::from)
}

/// CPU instruction sets that matter for llama.cpp, read from the real processor.
pub fn cpu_features() -> String {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        let mut v = Vec::new();
        if std::is_x86_feature_detected!("sse4.2") {
            v.push("SSE4.2");
        }
        if std::is_x86_feature_detected!("avx") {
            v.push("AVX");
        }
        if std::is_x86_feature_detected!("avx2") {
            v.push("AVX2");
        }
        if std::is_x86_feature_detected!("avx512f") {
            v.push("AVX512");
        }
        v.join(" ")
    }
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    {
        String::new()
    }
}

/// Release-asset name fragments to try, best first. A package that needs AVX2 is NEVER
/// offered to a processor without AVX2 (an i5-2400 has AVX but not AVX2).
pub fn engine_candidates(features: &str) -> Vec<&'static str> {
    let avx2 = features.contains("AVX2");
    let avx = features.contains("AVX");
    if avx2 {
        vec!["bin-win-cpu-x64", "bin-win-avx2-x64", "bin-win-avx-x64"]
    } else if avx {
        vec!["bin-win-cpu-x64", "bin-win-avx-x64", "bin-win-noavx-x64"]
    } else {
        vec!["bin-win-noavx-x64", "bin-win-cpu-x64"]
    }
}

fn verified_marker() -> PathBuf {
    llama_dir().join(".verified")
}

pub fn engine_verified() -> bool {
    find_server().is_some() && verified_marker().exists()
}

/// Actually runs llama-server (`--version`) so a package is only called "installed" if it starts
/// on THIS processor. The name of a package alone proves nothing.
pub async fn verify_engine() -> Result<(), String> {
    let exe = find_server().ok_or_else(|| "ERR_NO_ENGINE".to_string())?;
    let ok = tauri::async_runtime::spawn_blocking(move || {
        let mut cmd = Command::new(&exe);
        cmd.arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(dir) = exe.parent() {
            cmd.current_dir(dir);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(_) => return false,
        };
        let t0 = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(st)) => return st.success(),
                Ok(None) => {
                    if t0.elapsed() > Duration::from_secs(25) {
                        let _ = child.kill();
                        return false;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(_) => return false,
            }
        }
    })
    .await
    .map_err(es)?;
    if ok {
        let _ = std::fs::write(verified_marker(), cpu_features());
        Ok(())
    } else {
        let _ = std::fs::remove_file(verified_marker());
        Err("ERR_ENGINE_INCOMPATIBLE".to_string())
    }
}

async fn install_server(app: &AppHandle) -> Result<String, String> {
    crate::netguard::guard()?;
    let client = download_client()?;
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
        .ok_or_else(|| "ERR_NET".to_string())?;
    let mut named: Vec<(String, String)> = Vec::new();
    for a in assets {
        if let (Some(n), Some(u)) = (a["name"].as_str(), a["browser_download_url"].as_str()) {
            named.push((n.to_string(), u.to_string()));
        }
    }
    let mut tried = false;
    for needle in engine_candidates(&cpu_features()) {
        let Some((_, url)) = named
            .iter()
            .find(|(n, _)| n.contains(needle) && n.ends_with(".zip") && !n.contains("arm64"))
        else {
            continue;
        };
        tried = true;
        let _ = std::fs::remove_dir_all(llama_dir());
        let zip_path = data_dir().join("llama.zip");
        download_to(app, url, &zip_path, "llama-server").await?;
        let ex = extract_zip(&zip_path, &llama_dir());
        let _ = std::fs::remove_file(&zip_path);
        ex?;
        if verify_engine().await.is_ok() {
            return find_server()
                .map(|p| p.to_string_lossy().to_string())
                .ok_or_else(|| "ERR_NO_ENGINE".to_string());
        }
        let _ = std::fs::remove_dir_all(llama_dir()); // does not run on this CPU: try the next
    }
    Err(if tried {
        "ERR_ENGINE_INCOMPATIBLE".to_string()
    } else {
        "ERR_NO_ENGINE".to_string()
    })
}

#[tauri::command]
pub async fn agent_download_model(
    app: AppHandle,
    url: String,
    file_name: String,
) -> Result<String, String> {
    crate::netguard::guard()?;
    let name = Path::new(&file_name)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .ok_or_else(|| "اسم ملف غير صالح".to_string())?;
    if !name.to_lowercase().ends_with(".gguf") {
        return Err("يجب أن يكون الملف بصيغة .gguf".to_string());
    }
    // The engine is required for the model to work, so it is fetched first if missing.
    ensure_server(&app).await?;
    let dest = models_dir().join(&name);
    download_to(&app, &url, &dest, &name).await?;
    let p = dest.to_string_lossy().to_string();
    let mut cfg = load_config();
    cfg.model_path = Some(p.clone());
    save_config(&cfg)?;
    Ok(p)
}

/// Offline import of the engine: a .zip file or a folder that contains llama-server.exe.
#[tauri::command]
pub async fn agent_import_server(path: String) -> Result<String, String> {
    let p = Path::new(&path);
    let out = llama_dir();
    std::fs::create_dir_all(&out).map_err(es)?;
    if p.is_file() && path.to_lowercase().ends_with(".zip") {
        extract_zip(p, &out)?;
    } else if p.is_dir() {
        for e in WalkDir::new(p).into_iter().filter_map(|e| e.ok()) {
            if !e.file_type().is_file() {
                continue;
            }
            let rel = e.path().strip_prefix(p).map_err(es)?;
            let dest = out.join(rel);
            if let Some(par) = dest.parent() {
                std::fs::create_dir_all(par).map_err(es)?;
            }
            std::fs::copy(e.path(), &dest).map_err(es)?;
        }
    } else {
        return Err("اختر ملف zip أو مجلداً يحتوي على llama-server.exe".to_string());
    }
    let exe = find_server().ok_or_else(|| "ERR_NO_ENGINE".to_string())?;
    verify_engine().await?; // an imported engine must start on this CPU too
    Ok(exe.to_string_lossy().to_string())
}

#[tauri::command]
pub fn agent_set_model(path: String) -> Result<(), String> {
    let p = Path::new(&path);
    if !p.is_file() || !path.to_lowercase().ends_with(".gguf") {
        return Err("اختر ملف نموذج بصيغة .gguf موجود فعلاً".to_string());
    }
    // The model stays exactly where the user keeps it; nothing is copied or moved.
    let mut cfg = load_config();
    cfg.model_path = Some(path);
    save_config(&cfg)
}

pub fn local_client(timeout_secs: u64) -> Result<reqwest::Client, String> {
    let mut h = reqwest::header::HeaderMap::new();
    let tok = endpoint().1;
    if !tok.is_empty() {
        if let Ok(v) = reqwest::header::HeaderValue::from_str(&format!("Bearer {}", tok)) {
            h.insert(reqwest::header::AUTHORIZATION, v);
        }
    }
    reqwest::Client::builder()
        .no_proxy()
        .default_headers(h)
        .timeout(Duration::from_secs(timeout_secs))
        .build()
        .map_err(es)
}

#[tauri::command]
pub async fn agent_start(state: State<'_, AppState>) -> Result<(), String> {
    ensure_running(&state).await
}

/// Starts the engine if it is not running (also used automatically before every chat).
pub async fn ensure_running(state: &AppState) -> Result<(), String> {
    touch();
    if find_server().is_some() && !verified_marker().exists() {
        verify_engine().await?; // once: the engine must really start on this CPU
    }
    {
        let mut g = state.agent.lock().map_err(es)?;
        let mut already = false;
        if let Some(c) = g.child.as_mut() {
            if matches!(c.try_wait(), Ok(None)) {
                already = true;
            }
        }
        if !already {
            let cfg = load_config();
            let exe = find_server().ok_or_else(|| "ERR_NO_ENGINE".to_string())?;
            let model = cfg
                .model_path
                .clone()
                .ok_or_else(|| "ERR_NO_MODEL".to_string())?;
            if !Path::new(&model).exists() {
                return Err("ERR_MODEL_MISSING".to_string());
            }
            let port = std::net::TcpListener::bind("127.0.0.1:0")
                .map_err(es)?
                .local_addr()
                .map_err(es)?
                .port();
            let token: String = {
                use rand::{distributions::Alphanumeric, Rng};
                rand::thread_rng()
                    .sample_iter(&Alphanumeric)
                    .take(40)
                    .map(char::from)
                    .collect()
            };
            let model_bytes = std::fs::metadata(&model).map(|m| m.len()).unwrap_or(0);
            let (ctx, thr) = crate::perf::fit_to_memory(
                cfg.ctx(),
                cfg.threads(),
                model_bytes,
                crate::perf::available_ram(),
            );
            let threads = thr.to_string();
            let mut cmd = Command::new(&exe);
            cmd.arg("-m")
                .arg(&model)
                .arg("--host")
                .arg("127.0.0.1")
                .arg("--port")
                .arg(port.to_string())
                .arg("--api-key")
                .arg(&token)
                .arg("-c")
                .arg(ctx.to_string())
                .arg("-t")
                .arg(&threads)
                .arg("-tb")
                .arg(&threads)
                .arg("-np")
                .arg("1")
                .arg("-b")
                .arg(cfg.batch().to_string())
                .arg("-ub")
                .arg(cfg.ubatch().to_string())
                .arg("--jinja");
            if let Some(dir) = exe.parent() {
                cmd.current_dir(dir);
            }
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0800_0000 | 0x0000_4000);
            }
            let child = cmd.spawn().map_err(es)?;
            g.job = crate::perf::cap_child(&child, cfg.cap()).ok();
            g.child = Some(child);
            if let Ok(mut r) = RT.lock() {
                *r = (port, token);
            }
        }
    }
    let client = local_client(5)?;
    let url = url("/health");
    for _ in 0..480 {
        let died: Option<String> = match state.agent.lock() {
            Ok(mut g) => match g.child.as_mut().map(|c| c.try_wait()) {
                Some(Ok(Some(st))) => {
                    g.child = None;
                    if st.code().map(|c| c as u32) == Some(0xC000_001D) {
                        let _ = std::fs::remove_file(verified_marker());
                        Some("ERR_ENGINE_INCOMPATIBLE".to_string())
                    } else {
                        Some("ERR_ENGINE_CRASHED".to_string())
                    }
                }
                _ => None,
            },
            Err(_) => None,
        };
        if let Some(e) = died {
            return Err(e);
        }
        if let Ok(r) = client.get(&url).send().await {
            if r.status().is_success() {
                touch();
                return Ok(());
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err("ERR_LOAD_TIMEOUT".to_string())
}

/// Unloads the engine after a period without use, freeing RAM and CPU.
pub fn spawn_idle_watcher(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(30));
        let idle = load_config().idle();
        let last = LAST_USED.load(Ordering::Relaxed);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if last > 0 && now.saturating_sub(last) > idle {
            use tauri::Manager;
            if let Some(st) = app.try_state::<AppState>() {
                if let Ok(mut g) = st.agent.lock() {
                    if let Some(mut c) = g.child.take() {
                        let _ = c.kill();
                        let _ = c.wait();
                    }
                    g.job = None;
                }
            }
            LAST_USED.store(0, Ordering::Relaxed);
        }
    });
}

#[tauri::command]
pub fn agent_set_perf(
    cpu_cap: u32,
    threads: u32,
    ctx: u32,
    idle_secs: u64,
    batch: u32,
    ubatch: u32,
) -> Result<(), String> {
    let mut c = load_config();
    c.cpu_cap = Some(cpu_cap.clamp(10, 100));
    c.threads = Some(threads.clamp(1, 8));
    c.ctx = Some(ctx.clamp(2048, 16384));
    c.batch = Some(batch.clamp(32, 1024));
    c.ubatch = Some(ubatch.clamp(16, 512));
    c.idle_secs = Some(idle_secs.clamp(60, 7200));
    save_config(&c)
}

#[tauri::command]
pub fn agent_set_lang(lang: String) -> Result<(), String> {
    if !["ar", "en", "id", "fr", "it"].contains(&lang.as_str()) {
        return Err("lang".to_string());
    }
    let mut c = load_config();
    c.lang = Some(lang);
    save_config(&c)
}

#[tauri::command]
pub fn agent_stop(state: State<'_, AppState>) -> Result<(), String> {
    let mut g = state.agent.lock().map_err(es)?;
    if let Some(mut c) = g.child.take() {
        let _ = c.kill();
        let _ = c.wait();
    }
    g.job = None;
    Ok(())
}

pub async fn complete(system: &str, user: &str, max_tokens: u32) -> Result<String, String> {
    touch();
    let client = local_client(900)?;
    let body = serde_json::json!({
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user }
        ],
        "temperature": 0.2,
        "max_tokens": max_tokens,
        "chat_template_kwargs": { "enable_thinking": false }
    });
    let v: serde_json::Value = client
        .post(url("/v1/chat/completions"))
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
        if f.risk == "safe" {
            "كاش آمن"
        } else {
            "يحتاج مراجعة"
        },
        f.size / (1024 * 1024),
        f.age_days
            .map(|d| d.to_string())
            .unwrap_or_else(|| "?".to_string()),
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
        let v: Vec<(String, String)> = ids
            .iter()
            .filter_map(|id| g.get(id).map(|f| (id.clone(), describe(f))))
            .collect();
        v
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

#[tauri::command]
pub fn agent_set_data_dir(path: String) -> Result<(), String> {
    let p = Path::new(&path);
    if !p.is_absolute() {
        return Err("اكتب مساراً كاملاً مثل D:\\GA-Models".to_string());
    }
    std::fs::create_dir_all(p).map_err(es)?;
    let mut cfg = load_config();
    cfg.data_dir = Some(path);
    save_config(&cfg)
}

#[tauri::command]
pub fn agent_set_allow_commands(allow: bool) -> Result<(), String> {
    let mut cfg = load_config();
    cfg.allow_commands = allow;
    save_config(&cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i5_2400_never_gets_an_avx2_package() {
        // Sandy Bridge: AVX yes, AVX2 no.
        let c = engine_candidates("SSE4.2 AVX");
        assert!(!c.iter().any(|n| n.contains("avx2")));
        assert!(!c.iter().any(|n| n.contains("avx512")));
        assert_eq!(c[0], "bin-win-cpu-x64");
        assert!(c.contains(&"bin-win-avx-x64"));
    }

    #[test]
    fn avx2_cpu_may_use_avx2_package() {
        assert!(engine_candidates("SSE4.2 AVX AVX2").contains(&"bin-win-avx2-x64"));
    }

    #[test]
    fn cpu_without_avx_gets_noavx_first() {
        assert_eq!(engine_candidates("SSE4.2")[0], "bin-win-noavx-x64");
    }

    #[test]
    fn performance_defaults_match_the_target_machine() {
        let c = Config::default();
        assert_eq!(c.threads(), 2);
        assert_eq!(c.cap(), 60);
        assert_eq!(c.ctx(), 4096);
        assert_eq!(c.batch(), 128);
        assert_eq!(c.ubatch(), 64);
        assert_eq!(c.idle(), 120);
        assert!(c.offline());
    }
}
