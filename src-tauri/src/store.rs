//! Local memory (what the agent remembers about the user) and skills (recipes the agent can read).

use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

fn es<E: ToString>(e: E) -> String {
    e.to_string()
}

fn memory_path() -> PathBuf {
    crate::agent::base_dir().join("memory.json")
}

pub fn read_memory() -> BTreeMap<String, String> {
    fs::read_to_string(memory_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_memory(m: &BTreeMap<String, String>) -> Result<(), String> {
    fs::create_dir_all(crate::agent::base_dir()).map_err(es)?;
    let s = serde_json::to_string_pretty(m).map_err(es)?;
    fs::write(memory_path(), s).map_err(es)
}

pub fn remember(key: &str, value: &str) -> Result<String, String> {
    let k = key.trim();
    let v = value.trim();
    if k.is_empty() || v.is_empty() {
        return Err("المفتاح والقيمة مطلوبان".to_string());
    }
    let low = format!("{} {}", k, v).to_lowercase();
    for bad in [
        "password",
        "passwd",
        "token",
        "api key",
        "apikey",
        "secret",
        "cvv",
        "iban",
        "كلمة المرور",
        "كلمة السر",
        "رقم البطاقة",
    ] {
        if low.contains(bad) {
            return Err("لن أحفظ معلومات حساسة مثل كلمات المرور أو بيانات البطاقات".to_string());
        }
    }
    let k: String = k.chars().take(40).collect();
    let v: String = v.chars().take(300).collect();
    let mut m = read_memory();
    if m.len() >= 50 && !m.contains_key(&k) {
        return Err("الذاكرة ممتلئة. اطلب من المستخدم حذف بعض العناصر.".to_string());
    }
    m.insert(k.clone(), v);
    write_memory(&m)?;
    Ok(format!("تم حفظ: {}", k))
}

pub fn forget(key: &str) -> Result<(), String> {
    let mut m = read_memory();
    m.remove(key);
    write_memory(&m)
}

pub fn recall_text() -> String {
    let m = read_memory();
    if m.is_empty() {
        return "لا توجد معلومات محفوظة بعد.".to_string();
    }
    m.iter()
        .map(|(k, v)| format!("{}: {}", k, v))
        .collect::<Vec<String>>()
        .join("\n")
}

pub fn memory_prompt() -> String {
    let m = read_memory();
    if m.is_empty() {
        return String::new();
    }
    let lines = m
        .iter()
        .map(|(k, v)| format!("- {}: {}", k, v))
        .collect::<Vec<String>>()
        .join("\n");
    format!(
        "\nمعلومات محفوظة عن المستخدم (استفد منها عند الحاجة فقط):\n{}",
        lines
    )
}

// ------------------------------------------------------------------ skills

#[derive(Clone, Serialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
}

fn sk(name: &str, description: &str, body: &str) -> Skill {
    Skill {
        name: name.to_string(),
        description: description.to_string(),
        body: body.to_string(),
    }
}

fn builtin() -> Vec<Skill> {
    vec![
        sk(
            "free_space",
            "توفير مساحة القرص بأمان",
            "1) disk_free لمعرفة الوضع. 2) scan_cleanup ثم scan_shrink، وscan_dedupe على مجلدات الوسائط/التنزيلات. 3) لخّص للمستخدم بالأرقام: كم يوفّر التنظيف وكم يوفّر دمج المكرر وكم توفّر «الخزنة» (shrink_items للمجلدات الباردة). 4) اقترح clean_items للآمن أولاً ثم apply_dedupe ثم shrink_items، بمعرّفات حقيقية فقط. 5) بعد التنفيذ كرر disk_free وقارن. لا تحذف شيئاً بدون موافقة، وفضّل التصغير على الحذف.",
        ),
        sk(
            "organize_folder",
            "ترتيب مجلد (مثل التنزيلات) حسب نوع الملفات",
            "1) list_folder أو folder_sizes لفهم المحتوى. 2) plan_organize(path). 3) اشرح للمستخدم ما سيُنقل. 4) بعد موافقته apply_organize(plan_id). 5) أخبره أنه يستطيع التراجع من تبويب السجل.",
        ),
        sk(
            "find_document",
            "العثور على مستند أو معلومة داخل ملفاتك",
            "1) search_in_files بكلمة مميزة داخل المجلد المناسب. 2) افتح أفضل النتائج بـ read_document. 3) لخّص للمستخدم واذكر مسار الملف. كل العمل على جهازه فقط بلا إنترنت.",
        ),
        sk(
            "slow_pc",
            "تشخيص بطء الجهاز",
            "1) system_health. 2) list_processes لأكبر المستهلكين. 3) list_startup لبرامج بدء التشغيل. 4) اشرح بكلمات بسيطة السبب المحتمل، واقترح إغلاق برنامج (close_app) بعد موافقة المستخدم.",
        ),
        sk(
            "weekly_maintenance",
            "صيانة أسبوعية للجهاز",
            "1) system_health. 2) scan_cleanup ثم scan_shrink. 3) list_startup. 4) لخّص النتيجة. 5) اقترح جدولة تنظيف أسبوعي بـ schedule_task(kind=clean_safe, repeat_hours=168). استخدم current_time لحساب موعد البدء.",
        ),
    ]
}

pub fn skills() -> Vec<Skill> {
    let mut v = builtin();
    let dir = crate::agent::data_dir().join("skills");
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.filter_map(|e| e.ok()) {
            let p = e.path();
            if p.extension().map(|x| x == "md").unwrap_or(false) {
                let name = p
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                if let Ok(text) = fs::read_to_string(&p) {
                    let body: String = text.chars().take(4000).collect();
                    let desc = body
                        .lines()
                        .find(|l| !l.trim().is_empty())
                        .unwrap_or("")
                        .trim_start_matches('#')
                        .trim()
                        .to_string();
                    v.push(Skill {
                        name,
                        description: desc,
                        body,
                    });
                }
            }
        }
    }
    v
}

pub fn list_skills_text() -> String {
    skills()
        .iter()
        .map(|s| format!("{} — {}", s.name, s.description))
        .collect::<Vec<String>>()
        .join("\n")
}

pub fn read_skill(name: &str) -> Result<String, String> {
    skills()
        .into_iter()
        .find(|s| s.name == name)
        .map(|s| s.body)
        .ok_or_else(|| "مهارة غير موجودة. استخدم list_skills.".to_string())
}
