//! Windows notifications (toast) without any extra library: a short PowerShell call.
//! Works also when the app is closed (scheduled tasks run without a window).
//! Local only - it never touches the network, so it is allowed in Strict Offline Mode.

use crate::agent::{load_config, save_config};
use std::process::{Command, Stdio};

fn text_for(kind: &str, lang: &str, detail: &str) -> (String, String) {
    let l = match lang {
        "en" | "id" | "fr" | "it" => lang,
        _ => "ar",
    };
    let title = match (kind, l) {
        ("approval", "ar") => "طلب موافقة ينتظرك",
        ("approval", "en") => "Waiting for your approval",
        ("approval", "id") => "Menunggu persetujuan Anda",
        ("approval", "fr") => "En attente de votre accord",
        ("approval", _) => "In attesa della tua approvazione",
        ("message", "ar") => "رسالة جديدة",
        ("message", "en") => "New message",
        ("message", "id") => "Pesan baru",
        ("message", "fr") => "Nouveau message",
        ("message", _) => "Nuovo messaggio",
        ("task", "ar") => "انتهت مهمة مجدولة",
        ("task", "en") => "Scheduled task finished",
        ("task", "id") => "Tugas terjadwal selesai",
        ("task", "fr") => "Tâche planifiée terminée",
        ("task", _) => "Attività pianificata completata",
        ("reminder", "ar") => "تذكير",
        ("reminder", "en") => "Reminder",
        ("reminder", "id") => "Pengingat",
        ("reminder", "fr") => "Rappel",
        ("reminder", _) => "Promemoria",
        (_, "ar") => "انتهى العمل",
        (_, "en") => "Done",
        (_, "id") => "Selesai",
        (_, "fr") => "Terminé",
        (_, _) => "Fatto",
    };
    let body: String = detail
        .chars()
        .filter(|c| !c.is_control())
        .take(200)
        .collect();
    (title.to_string(), body)
}

pub fn enabled() -> bool {
    load_config().notify.unwrap_or(true)
}

/// Shows one toast and waits for the helper to end (use in headless runs).
pub fn show(kind: &str, detail: &str) {
    if !enabled() {
        return;
    }
    let (title, body) = text_for(kind, &load_config().lang(), detail);
    send(&title, &body);
}

/// Same, but never blocks the caller.
pub fn show_bg(kind: &str, detail: &str) {
    let (k, d) = (kind.to_string(), detail.to_string());
    std::thread::spawn(move || show(&k, &d));
}

#[cfg(windows)]
fn send(title: &str, body: &str) {
    use std::os::windows::process::CommandExt;
    let script = "$ErrorActionPreference='Stop';\
        [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType=WindowsRuntime] | Out-Null;\
        $x=[Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent(1);\
        $t=$x.GetElementsByTagName('text');\
        [void]$t.Item(0).AppendChild($x.CreateTextNode($env:GA_NT));\
        [void]$t.Item(1).AppendChild($x.CreateTextNode($env:GA_NB));\
        $n=[Windows.UI.Notifications.ToastNotification]::new($x);\
        [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\WindowsPowerShell\\v1.0\\powershell.exe').Show($n)";
    let _ = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-Command",
            script,
        ])
        .env("GA_NT", title)
        .env("GA_NB", body)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000)
        .status();
}

#[cfg(not(windows))]
fn send(_title: &str, _body: &str) {
    let _ = Command::new("true").stdin(Stdio::null()).status();
}

#[tauri::command]
pub fn notify_get() -> bool {
    enabled()
}

#[tauri::command]
pub fn notify_set(on: bool) -> Result<(), String> {
    let mut c = load_config();
    c.notify = Some(on);
    save_config(&c)
}

/// The window asks for a toast only when it is hidden or not focused.
#[tauri::command]
pub fn notify_event(kind: String, detail: String) {
    show_bg(&kind, &detail);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_exist_in_all_languages() {
        for l in ["ar", "en", "id", "fr", "it"] {
            for k in ["approval", "message", "task", "reminder", "done"] {
                let (t, b) = text_for(k, l, "x\ny");
                assert!(!t.is_empty());
                assert_eq!(b, "xy");
            }
        }
    }

    #[test]
    fn body_is_cut() {
        let (_, b) = text_for("task", "en", &"a".repeat(500));
        assert_eq!(b.chars().count(), 200);
    }
}
