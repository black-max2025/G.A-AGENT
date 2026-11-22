//! Real scheduling through the Windows Task Scheduler. Tasks keep working when this app is closed,
//! and a task whose time passed while the PC was off runs as soon as the PC is back on
//! (StartWhenAvailable). The scheduled program is this same exe started with `--task <id>`.
//! Only a fixed set of actions exists: clean_safe, organize, dedupe, reminder.

use crate::{cleaner, dedupe, files, scanner, sysinfo, timeutil};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static NONCE: AtomicU64 = AtomicU64::new(1);
const FOLDER: &str = "GA-Agent";

fn es<E: ToString>(e: E) -> String {
    e.to_string()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Task {
    pub id: String,
    pub title: String,
    /// clean_safe | organize | dedupe | reminder
    pub kind: String,
    pub param: String,
    /// First run, local time "YYYY-MM-DDTHH:MM"
    pub at: String,
    /// 0 = once, 24 = daily, 168 = weekly, or any number of hours
    pub repeat_hours: u64,
    pub last_result: Option<String>,
    #[serde(default)]
    pub last_run: Option<String>,
    #[serde(default)]
    pub unseen: bool,
    pub enabled: bool,
}

#[derive(Serialize)]
pub struct TaskDto {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub param: String,
    pub at: String,
    pub repeat_hours: u64,
    pub last_result: Option<String>,
    pub last_run: Option<String>,
    pub enabled: bool,
    /// minutes until the next run (None when finished)
    pub next_in_min: Option<i64>,
    pub next_at: Option<String>,
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

/// Minutes until the next run, on the local clock.
fn next_in(t: &Task, now: i64) -> Option<i64> {
    let (y, mo, d, h, mi) = timeutil::parse_local(&t.at)?;
    let start = timeutil::local_minutes(y, mo, d, h, mi);
    if now <= start {
        return Some(start - now);
    }
    if t.repeat_hours == 0 || !t.enabled {
        return None;
    }
    let step = t.repeat_hours as i64 * 60;
    let k = (now - start + step - 1) / step;
    Some(start + k * step - now)
}

pub fn to_dto(t: &Task) -> TaskDto {
    let now = timeutil::now_minutes();
    let n = if t.enabled { next_in(t, now) } else { None };
    TaskDto {
        id: t.id.clone(),
        title: t.title.clone(),
        kind: t.kind.clone(),
        param: t.param.clone(),
        at: t.at.clone(),
        repeat_hours: t.repeat_hours,
        last_result: t.last_result.clone(),
        last_run: t.last_run.clone(),
        enabled: t.enabled,
        next_in_min: n,
        next_at: n.map(|m| {
            let (y, mo, d, h, mi) = timeutil::from_minutes(now + m);
            timeutil::iso(y, mo, d, h, mi)
        }),
    }
}

// ------------------------------------------------------------ Task Scheduler

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn task_name(id: &str) -> String {
    format!("{}\\{}", FOLDER, id)
}

fn build_xml(t: &Task, exe: &str) -> String {
    let (y, mo, d, h, mi) = timeutil::parse_local(&t.at).unwrap_or((2030, 1, 1, 0, 0));
    let rep = if t.repeat_hours > 0 {
        format!(
            "<Repetition><Interval>PT{}H</Interval></Repetition>",
            t.repeat_hours.clamp(1, 24 * 60)
        )
    } else {
        String::new()
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>G.A AGENT: {desc}</Description></RegistrationInfo>
  <Triggers>
    <TimeTrigger>
      <StartBoundary>{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:00</StartBoundary>
      <Enabled>true</Enabled>
      {rep}
    </TimeTrigger>
  </Triggers>
  <Principals><Principal id="Author"><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <StartWhenAvailable>true</StartWhenAvailable>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <ExecutionTimeLimit>PT2H</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author"><Exec><Command>{exe}</Command><Arguments>--task {id}</Arguments></Exec></Actions>
</Task>"#,
        desc = xml_escape(&t.title),
        exe = xml_escape(exe),
        id = xml_escape(&t.id),
    )
}

fn schtasks(args: &[&str]) -> Result<(), String> {
    let mut c = Command::new("schtasks.exe");
    c.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    let st = c.status().map_err(es)?;
    if st.success() {
        Ok(())
    } else {
        Err("ERR_SCHEDULER".to_string())
    }
}

fn register(t: &Task) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(es)?;
    let xml = build_xml(t, &exe.to_string_lossy());
    let path = std::env::temp_dir().join(format!("ga-task-{}.xml", t.id));
    let mut bytes: Vec<u8> = vec![0xFF, 0xFE]; // UTF-16 LE BOM, which schtasks expects
    for u in xml.encode_utf16() {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    fs::write(&path, bytes).map_err(es)?;
    let r = schtasks(&[
        "/Create",
        "/TN",
        &task_name(&t.id),
        "/XML",
        &path.to_string_lossy(),
        "/F",
    ]);
    let _ = fs::remove_file(&path);
    r
}

fn unregister(id: &str) {
    let _ = schtasks(&["/Delete", "/TN", &task_name(id), "/F"]);
}

// --------------------------------------------------------------------- API

pub fn add_task(
    kind: &str,
    title: &str,
    param: &str,
    at: &str,
    first_in_minutes: u64,
    repeat_hours: u64,
) -> Result<Task, String> {
    match kind {
        "clean_safe" => {}
        "organize" | "dedupe" => {
            let p = Path::new(param);
            if !p.is_absolute() || !p.is_dir() || crate::safety::is_protected(p) {
                return Err("ERR_BAD_FOLDER".to_string());
            }
        }
        "reminder" => {
            if param.trim().is_empty() || param.chars().count() > 200 {
                return Err("ERR_BAD_REMINDER".to_string());
            }
        }
        _ => return Err("ERR_BAD_KIND".to_string()),
    }
    if title.trim().is_empty() {
        return Err("ERR_NO_TITLE".to_string());
    }
    if repeat_hours > 24 * 60 {
        return Err("ERR_BAD_REPEAT".to_string());
    }
    let now = timeutil::now_minutes();
    let start = if at.trim().is_empty() {
        now + first_in_minutes as i64
    } else {
        let (y, mo, d, h, mi) =
            timeutil::parse_local(at).ok_or_else(|| "ERR_BAD_TIME".to_string())?;
        timeutil::local_minutes(y, mo, d, h, mi)
    };
    if start < now - 1 && repeat_hours == 0 {
        return Err("ERR_TIME_PAST".to_string());
    }
    if start > now + 366 * 1440 {
        return Err("ERR_TIME_FAR".to_string());
    }
    let (y, mo, d, h, mi) = timeutil::from_minutes(start.max(now + 1));
    let mut all = read_tasks();
    if all.len() >= 30 {
        return Err("ERR_TOO_MANY".to_string());
    }
    let t = Task {
        id: format!("t{}-{}", now_secs(), NONCE.fetch_add(1, Ordering::SeqCst)),
        title: title.trim().chars().take(80).collect(),
        kind: kind.to_string(),
        param: param.to_string(),
        at: timeutil::iso(y, mo, d, h, mi),
        repeat_hours,
        last_result: None,
        last_run: None,
        unseen: false,
        enabled: true,
    };
    register(&t)?;
    all.push(t.clone());
    write_tasks(&all);
    Ok(t)
}

pub fn remove_task(id: &str) -> Result<(), String> {
    let mut all = read_tasks();
    let before = all.len();
    all.retain(|t| t.id != id);
    if all.len() == before {
        return Err("ERR_NO_TASK".to_string());
    }
    unregister(id);
    write_tasks(&all);
    Ok(())
}

fn popup(text: &str) {
    let clean: String = text
        .chars()
        .filter(|c| !c.is_control())
        .take(300)
        .collect::<String>()
        .replace('\'', "’");
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; [void][System.Windows.Forms.MessageBox]::Show('{}','G.A AGENT',0,64,0,[System.Windows.Forms.MessageBoxOptions]::DefaultDesktopOnly -bor 0)",
        clean
    );
    let mut c = Command::new("powershell.exe");
    c.args([
        "-NoProfile",
        "-NonInteractive",
        "-WindowStyle",
        "Hidden",
        "-Command",
        &script,
    ])
    .stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    let _ = c.status();
}

/// Result is "code|detail"; the UI turns the code into the user's language.
pub fn run_task(t: &Task) -> String {
    match t.kind.as_str() {
        "clean_safe" => {
            let list = scanner::scan_rules();
            let mut freed = 0u64;
            for f in list.iter().filter(|f| f.risk == "safe" && !f.admin) {
                freed += cleaner::clean_finding(f).freed;
            }
            format!("cleaned|{}", sysinfo::fmt_bytes(freed))
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
                    match files::apply_organize(&id) {
                        Ok(m) => format!("organized|{}", m.lines().next().unwrap_or("")),
                        Err(e) => format!("error|{}", e),
                    }
                }
                Err(e) => format!("error|{}", e),
            }
        }
        "dedupe" => match dedupe::scan_folder(Path::new(&t.param)) {
            Ok(r) if r.files > 0 => match dedupe::apply(&r.plan_id) {
                Ok(m) => format!("deduped|{}", m),
                Err(e) => format!("error|{}", e),
            },
            Ok(_) => "deduped|0".to_string(),
            Err(e) => format!("error|{}", e),
        },
        "reminder" => {
            popup(&t.param);
            format!("reminded|{}", t.param)
        }
        _ => "error|ERR_BAD_KIND".to_string(),
    }
}

fn finish(id: &str, res: String) {
    let n = timeutil::now_local();
    let mut all = read_tasks();
    let mut once_done = false;
    if let Some(x) = all.iter_mut().find(|x| x.id == id) {
        x.last_result = Some(res);
        x.last_run = Some(timeutil::iso(n.y, n.mo, n.d, n.h, n.mi));
        x.unseen = true;
        if x.repeat_hours == 0 {
            x.enabled = false;
            once_done = true;
        }
    }
    write_tasks(&all);
    if once_done {
        unregister(id);
    }
}

/// `ga-agent.exe --task <id>`: runs one task without opening any window.
pub fn run_headless(id: &str) {
    crate::perf::lower_own_priority();
    let _ = rayon::ThreadPoolBuilder::new()
        .num_threads(crate::perf::scan_threads())
        .build_global();
    if let Some(t) = read_tasks().into_iter().find(|t| t.id == id && t.enabled) {
        let res = run_task(&t);
        let (code, detail) = res.split_once('|').unwrap_or((res.as_str(), ""));
        let kind = if code == "reminded" {
            "reminder"
        } else {
            "task"
        };
        let detail = detail.to_string();
        finish(id, res.clone());
        crate::notify::show(kind, &detail);
    }
}

#[tauri::command(async)]
pub fn task_run_now(id: String) -> Result<String, String> {
    let t = read_tasks()
        .into_iter()
        .find(|t| t.id == id)
        .ok_or_else(|| "ERR_NO_TASK".to_string())?;
    let res = run_task(&t);
    let n = timeutil::now_local();
    let mut all = read_tasks();
    if let Some(x) = all.iter_mut().find(|x| x.id == id) {
        x.last_result = Some(res.clone());
        x.last_run = Some(timeutil::iso(n.y, n.mo, n.d, n.h, n.mi));
    }
    write_tasks(&all);
    Ok(res)
}

/// Results of tasks that ran while the app was closed; each is returned once.
#[tauri::command]
pub fn tasks_unseen() -> Vec<TaskDto> {
    let mut all = read_tasks();
    let out: Vec<TaskDto> = all.iter().filter(|t| t.unseen).map(to_dto).collect();
    for t in all.iter_mut() {
        t.unseen = false;
    }
    if !out.is_empty() {
        write_tasks(&all);
    }
    out
}

/// Created by the user himself from the Tasks page (his own click is the approval).
#[tauri::command(async)]
pub fn task_add(
    kind: String,
    title: String,
    param: String,
    at: String,
    repeat_hours: u64,
) -> Result<TaskDto, String> {
    let t = add_task(&kind, &title, &param, &at, 0, repeat_hours)?;
    Ok(to_dto(&t))
}
