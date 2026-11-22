//! Tool routing: the model only sees the tools that fit the user's request.
//! A small model gets confused (and runs out of context) when 40 tools are sent at once,
//! so the request is classified by Rust, not by the model.

use serde_json::Value;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Group {
    Storage,
    Files,
    Programs,
    Schedule,
    Web,
    Mail,
    Chat,
    Shell,
}

/// Always visible: planning, memory and the clock (cheap, needed everywhere).
const BASE: &[&str] = &[
    "current_time",
    "set_plan",
    "update_plan",
    "remember",
    "recall",
    "forget",
];

const GENERAL: &[&str] = &[
    "disk_free",
    "list_folder",
    "folder_sizes",
    "find_files",
    "read_text_file",
    "list_skills",
    "read_skill",
];

fn tools_of(g: Group) -> &'static [&'static str] {
    match g {
        Group::Storage => &[
            "disk_free",
            "list_profiles",
            "storage_overview",
            "storage_top",
            "folder_sizes",
            "scan_cleanup",
            "scan_shrink",
            "scan_dedupe",
            "storage_plan",
            "storage_apply",
            "clean_items",
            "shrink_items",
            "apply_dedupe",
            "undo_shrink",
        ],
        Group::Files => &[
            "list_folder",
            "folder_sizes",
            "find_files",
            "read_text_file",
            "read_document",
            "search_in_files",
            "plan_organize",
            "apply_organize",
            "move_path",
            "recycle_path",
            "write_text_file",
        ],
        Group::Programs => &[
            "list_apps",
            "list_processes",
            "system_health",
            "list_startup",
            "open_app",
            "close_app",
        ],
        Group::Schedule => &[
            "schedule_task",
            "list_tasks",
            "remove_task",
            "list_connectors",
            "run_connector",
        ],
        Group::Web => &["web_search", "fetch_page"],
        Group::Mail => &["email_search", "email_read", "email_send"],
        Group::Chat => &["inbox_list", "inbox_reply", "whatsapp_prepare"],
        Group::Shell => &["run_powershell"],
    }
}

/// Lower-case and fold Arabic letter variants so "اوفّر" and "أوفر" match the same keyword.
pub fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| !('\u{064B}'..='\u{0652}').contains(c) && *c != '\u{0640}')
        .map(|c| match c {
            'أ' | 'إ' | 'آ' => 'ا',
            'ى' => 'ي',
            'ة' => 'ه',
            c => c,
        })
        .collect::<String>()
        .to_lowercase()
}

fn has(text: &str, words: &[&str]) -> bool {
    words.iter().any(|w| text.contains(&normalize(w)))
}

pub fn groups(request: &str) -> Vec<Group> {
    let t = normalize(request);
    let mut out = Vec::new();
    if has(
        &t,
        &[
            "مساحه",
            "توفير",
            "وفر",
            "اوفر",
            "تنظيف",
            "نظف",
            "كاش",
            "ضغط",
            "اضغط",
            "تكرار",
            "مكرر",
            "متطابق",
            "قرص",
            "حجم",
            "الاكبر",
            "space",
            "storage",
            "disk",
            "free up",
            "clean",
            "cache",
            "compress",
            "duplicate",
            "drive",
            "espace",
            "disque",
            "nettoy",
            "doublon",
            "ruang",
            "penyimpanan",
            "bersihkan",
            "kompres",
            "duplikat",
            "spazio",
            "disco",
            "pulisci",
            "comprim",
            "duplicat",
            " c:",
            " d:",
            " c ",
            " d ",
            "في c",
            "في d",
        ],
    ) {
        out.push(Group::Storage);
    }
    if has(
        &t,
        &[
            "ملف",
            "مجلد",
            "رتب",
            "نظم",
            "ابحث عن",
            "اقرا",
            "مستند",
            "file",
            "folder",
            "organize",
            "sort",
            "document",
            "fichier",
            "dossier",
            "berkas",
            "cartella",
            "documento",
        ],
    ) {
        out.push(Group::Files);
    }
    if has(
        &t,
        &[
            "برنامج",
            "برامج",
            "تطبيق",
            "افتح",
            "اغلق",
            "عمليه",
            "بدء التشغيل",
            "app",
            "program",
            "process",
            "startup",
            "open ",
            "close ",
            "applic",
            "programme",
            "aplikasi",
            "programma",
            "avvio",
        ],
    ) {
        out.push(Group::Programs);
    }
    if has(
        &t,
        &[
            "جدول",
            "ذكرني",
            "تذكير",
            "موعد",
            "كل يوم",
            "غدا",
            "schedule",
            "remind",
            "every day",
            "tomorrow",
            "rappel",
            "planifi",
            "jadwal",
            "ingat",
            "promemoria",
            "pianifica",
            "رابط مع",
        ],
    ) {
        out.push(Group::Schedule);
    }
    if has(
        &t,
        &[
            "انترنت",
            "الانترنت",
            "ويب",
            "اخبار",
            "ابحث في",
            "search the web",
            "web",
            "internet",
            "news",
            "online",
            "recherche",
            "actualit",
            "cari",
            "berita",
            "cerca",
            "notizie",
        ],
    ) {
        out.push(Group::Web);
    }
    if has(
        &t,
        &[
            "ايميل",
            "بريد",
            "جيميل",
            "email",
            "e-mail",
            "mail",
            "gmail",
            "courriel",
            "surel",
            "posta",
        ],
    ) {
        out.push(Group::Mail);
    }
    if has(
        &t,
        &[
            "تيليجرام",
            "تلجرام",
            "واتساب",
            "رساله",
            "رسايل",
            "telegram",
            "whatsapp",
            "message",
            "messaggio",
            "pesan",
        ],
    ) {
        out.push(Group::Chat);
    }
    if has(
        &t,
        &["powershell", "بوروشيل", "سطر الاوامر", "command line", "cmd"],
    ) {
        out.push(Group::Shell);
    }
    out
}

/// Names of the tools to expose for this request.
pub fn names_for(request: &str, offline: bool) -> Vec<&'static str> {
    let mut names: Vec<&'static str> = BASE.to_vec();
    let g = groups(request);
    let mut add = |list: &'static [&'static str]| {
        for n in list {
            if !names.contains(n) {
                names.push(n);
            }
        }
    };
    let mut any = false;
    for x in g {
        if offline && matches!(x, Group::Web | Group::Mail | Group::Chat) {
            continue;
        }
        any = true;
        add(tools_of(x));
    }
    if !any {
        add(GENERAL);
    }
    names
}

/// Keep only the routed tools from the complete tool list.
pub fn filter(all: Vec<Value>, request: &str, offline: bool) -> Vec<Value> {
    let names = names_for(request, offline);
    all.into_iter()
        .filter(|t| {
            t["function"]["name"]
                .as_str()
                .map(|n| names.contains(&n))
                .unwrap_or(false)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_request_gets_only_storage_tools() {
        let n = names_for("عايز أوفر مساحة في C وD", false);
        assert!(n.contains(&"scan_cleanup"));
        assert!(n.contains(&"scan_dedupe"));
        assert!(n.contains(&"storage_plan"));
        for bad in [
            "email_send",
            "email_search",
            "inbox_reply",
            "whatsapp_prepare",
            "open_app",
            "close_app",
            "web_search",
            "fetch_page",
            "run_powershell",
        ] {
            assert!(!n.contains(&bad), "{} must not be visible", bad);
        }
    }

    #[test]
    fn offline_hides_every_network_tool_even_when_asked() {
        let n = names_for("search the web and send an email on telegram", true);
        for bad in [
            "web_search",
            "fetch_page",
            "email_send",
            "email_read",
            "inbox_reply",
            "whatsapp_prepare",
        ] {
            assert!(!n.contains(&bad), "{} leaked in offline mode", bad);
        }
    }

    #[test]
    fn online_routes_web_only_for_web_requests() {
        assert!(names_for("search the web for news", false).contains(&"web_search"));
        assert!(!names_for("how much free space", false).contains(&"web_search"));
    }

    #[test]
    fn unknown_request_gets_small_general_set() {
        let n = names_for("مرحبا", false);
        assert!(n.contains(&"disk_free"));
        assert!(!n.contains(&"email_send"));
        assert!(n.len() <= BASE.len() + GENERAL.len());
    }

    #[test]
    fn arabic_variants_match() {
        assert!(groups("أريد توفير مساحه").contains(&Group::Storage));
        assert!(groups("اوفّر مساحة").contains(&Group::Storage));
    }
}
