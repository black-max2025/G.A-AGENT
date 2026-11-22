//! Telegram through the official Bot API, outbound HTTPS only (long polling). Off until the user
//! connects a bot token. The token lives in Windows Credential Manager.
//!  - the bot answers ONLY its owner, who proves ownership with a one-time code shown in this app
//!    (the code expires and wrong attempts lock pairing for a while);
//!  - the owner can chat with the agent from the phone, but remote chats are read-only: anything
//!    that changes something must be approved on the PC;
//!  - messages from anybody else go to the inbox, get no automatic answer, and are never given to
//!    the model unless the user asks for a draft.

use crate::agent::{base_dir, es};
use crate::{assistant, inbox, AppState};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

const SERVICE: &str = "GA-Agent";
static RUNNING: AtomicBool = AtomicBool::new(false);
static PAIR: Mutex<Option<(String, Instant)>> = Mutex::new(None);
static FAILS: Mutex<(u32, Option<Instant>)> = Mutex::new((0, None));

#[derive(Serialize, Deserialize, Default, Clone)]
struct Cfg {
    bot: Option<String>,
    owner: Option<i64>,
    offset: i64,
}

fn cfg_path() -> std::path::PathBuf {
    base_dir().join("telegram.json")
}

fn load() -> Cfg {
    fs::read_to_string(cfg_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save(c: &Cfg) {
    let _ = fs::create_dir_all(base_dir());
    if let Ok(s) = serde_json::to_string(c) {
        let _ = fs::write(cfg_path(), s);
    }
}

fn entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, "telegram-bot").map_err(es)
}

fn token() -> Option<String> {
    entry().ok()?.get_password().ok()
}

async fn api(tok: &str, method: &str, body: Value, secs: u64) -> Result<Value, String> {
    crate::netguard::guard()?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(secs))
        .build()
        .map_err(es)?;
    let r: Value = client
        .post(format!("https://api.telegram.org/bot{}/{}", tok, method))
        .json(&body)
        .send()
        .await
        .map_err(|_| "ERR_NET".to_string())?
        .json()
        .await
        .map_err(|_| "ERR_NET".to_string())?;
    if r["ok"].as_bool() == Some(true) {
        Ok(r["result"].clone())
    } else {
        Err("ERR_TELEGRAM".to_string())
    }
}

pub async fn send_message(chat: i64, text: &str) -> Result<(), String> {
    crate::netguard::guard()?;
    let tok = token().ok_or_else(|| "ERR_NO_BOT".to_string())?;
    api(
        &tok,
        "sendMessage",
        json!({ "chat_id": chat, "text": text }),
        20,
    )
    .await
    .map(|_| ())
}

fn msg(lang: &str, key: &str) -> &'static str {
    match (lang, key) {
        ("ar", "paired") => "تم الاقتران. أنت الآن مالك هذا الوكيل. أرسل سؤالك أو /help",
        ("id", "paired") => "Berhasil dipasangkan. Anda adalah pemilik agen ini. Kirim pertanyaan atau /help",
        ("fr", "paired") => "Appairage réussi. Vous êtes le propriétaire de cet agent. Envoyez votre question ou /help",
        ("it", "paired") => "Abbinamento riuscito. Sei il proprietario di questo agente. Invia una domanda o /help",
        (_, "paired") => "Paired. You are now the owner of this agent. Send a question or /help",
        ("ar", "help") => "اكتب سؤالك وسأجيب (قراءة فقط). أي تغيير على الجهاز يحتاج موافقتك على الكمبيوتر.",
        ("id", "help") => "Tulis pertanyaan Anda (hanya baca). Perubahan apa pun perlu persetujuan di komputer.",
        ("fr", "help") => "Posez votre question (lecture seule). Tout changement doit être approuvé sur l'ordinateur.",
        ("it", "help") => "Scrivi la tua domanda (sola lettura). Ogni modifica va approvata sul computer.",
        (_, "help") => "Ask your question (read-only). Any change must be approved on the computer.",
        _ => "",
    }
}

fn pair_ok(code: &str) -> bool {
    {
        let f = FAILS.lock().map(|g| g.clone()).unwrap_or((0, None));
        if let Some(t) = f.1 {
            if t.elapsed() < Duration::from_secs(900) {
                return false;
            }
        }
    }
    let ok = match PAIR.lock() {
        Ok(g) => match g.as_ref() {
            Some((c, t)) => c == code && t.elapsed() < Duration::from_secs(600),
            None => false,
        },
        Err(_) => false,
    };
    if let Ok(mut f) = FAILS.lock() {
        if ok {
            *f = (0, None);
        } else {
            f.0 += 1;
            if f.0 >= 5 {
                f.1 = Some(Instant::now());
                f.0 = 0;
            }
        }
    }
    if ok {
        if let Ok(mut g) = PAIR.lock() {
            *g = None;
        }
    }
    ok
}

async fn handle(app: &AppHandle, tok: &str, cfg: &mut Cfg, u: &Value) {
    let m = &u["message"];
    let (Some(chat), Some(text)) = (m["chat"]["id"].as_i64(), m["text"].as_str()) else {
        return;
    };
    if m["chat"]["type"].as_str() != Some("private") {
        return;
    }
    let text = text.trim();
    let lang = crate::agent::load_config().lang();
    if Some(chat) == cfg.owner {
        if text == "/help" || text == "/start" {
            let _ = api(
                tok,
                "sendMessage",
                json!({ "chat_id": chat, "text": msg(&lang, "help") }),
                20,
            )
            .await;
            return;
        }
        if let Some(st) = app.try_state::<AppState>() {
            let _ = crate::agent::ensure_running(&st).await;
            let ans = assistant::remote_answer(&st, text).await;
            let ans: String = ans.chars().take(3800).collect();
            let _ = api(
                tok,
                "sendMessage",
                json!({ "chat_id": chat, "text": ans }),
                20,
            )
            .await;
        }
        return;
    }
    if let Some(code) = text.strip_prefix("/pair ") {
        if pair_ok(code.trim()) {
            cfg.owner = Some(chat);
            save(cfg);
            let _ = api(
                tok,
                "sendMessage",
                json!({ "chat_id": chat, "text": msg(&lang, "paired") }),
                20,
            )
            .await;
            let _ = app.emit("telegram-paired", json!({}));
        }
        return; // a wrong code gets no reply
    }
    let from = {
        let f = &m["from"];
        let name = f["first_name"].as_str().unwrap_or("");
        match f["username"].as_str() {
            Some(un) => format!("{} (@{})", name, un),
            None => name.to_string(),
        }
    };
    inbox::add("telegram", &chat.to_string(), &from, text);
    let _ = app.emit("inbox-new", json!({}));
}

pub fn start(app: AppHandle) {
    if crate::netguard::strict() {
        return;
    }
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let mut backoff = 2u64;
        loop {
            if !RUNNING.load(Ordering::SeqCst) {
                break;
            }
            let Some(tok) = token() else {
                RUNNING.store(false, Ordering::SeqCst);
                break;
            };
            let mut cfg = load();
            match api(
                &tok,
                "getUpdates",
                json!({ "timeout": 25, "offset": cfg.offset, "allowed_updates": ["message"] }),
                40,
            )
            .await
            {
                Ok(v) => {
                    backoff = 2;
                    for u in v.as_array().cloned().unwrap_or_default() {
                        cfg.offset = u["update_id"].as_i64().unwrap_or(cfg.offset) + 1;
                        handle(&app, &tok, &mut cfg, &u).await;
                        save(&cfg);
                    }
                }
                Err(_) => {
                    tokio::time::sleep(Duration::from_secs(backoff)).await;
                    backoff = (backoff * 2).min(120);
                }
            }
        }
    });
}

// ---------------------------------------------------------------- commands

#[derive(Serialize)]
pub struct TgStatus {
    pub connected: bool,
    pub bot: Option<String>,
    pub paired: bool,
    pub running: bool,
}

#[tauri::command]
pub async fn tg_connect(app: AppHandle, bot_token: String) -> Result<String, String> {
    crate::netguard::guard()?;
    let t = bot_token.trim().to_string();
    if t.len() < 30 || t.len() > 80 || !t.contains(':') || t.chars().any(|c| c.is_whitespace()) {
        return Err("ERR_BAD_TOKEN".to_string());
    }
    let me = api(&t, "getMe", json!({}), 20).await?;
    let name = me["username"].as_str().unwrap_or("bot").to_string();
    entry()?.set_password(&t).map_err(es)?;
    // drop any old updates so history is not replayed
    let _ = api(
        &t,
        "deleteWebhook",
        json!({ "drop_pending_updates": true }),
        20,
    )
    .await;
    let mut c = load();
    c.bot = Some(name.clone());
    c.owner = None;
    c.offset = 0;
    save(&c);
    start(app);
    Ok(name)
}

#[tauri::command]
pub fn tg_disconnect() -> Result<(), String> {
    RUNNING.store(false, Ordering::SeqCst);
    let _ = entry().and_then(|e| e.delete_credential().map_err(es));
    let _ = fs::remove_file(cfg_path());
    Ok(())
}

/// One-time code, valid 10 minutes. The owner sends "/pair CODE" to the bot from his phone.
#[tauri::command]
pub fn tg_pair_code() -> String {
    use rand::{distributions::Alphanumeric, Rng};
    let code: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(8)
        .map(|b| (b as char).to_ascii_uppercase())
        .collect();
    if let Ok(mut g) = PAIR.lock() {
        *g = Some((code.clone(), Instant::now()));
    }
    code
}

#[tauri::command]
pub fn tg_unpair() {
    let mut c = load();
    c.owner = None;
    save(&c);
}

#[tauri::command]
pub fn tg_status() -> TgStatus {
    let c = load();
    TgStatus {
        connected: token().is_some(),
        bot: c.bot,
        paired: c.owner.is_some(),
        running: RUNNING.load(Ordering::SeqCst),
    }
}

pub fn stop_listener() {
    RUNNING.store(false, Ordering::SeqCst);
}

pub fn autostart(app: AppHandle) {
    if token().is_some() {
        start(app);
    }
}
