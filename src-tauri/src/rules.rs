//! Knowledge base of well-known, regenerable locations.
//! risk = "safe"   : pure cache/temp data, rebuilt automatically -> deleted permanently
//! risk = "review" : regenerable but costly (re-download) -> moved to the Recycle Bin

pub struct Rule {
    pub id: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    pub risk: &'static str,
    pub reason: &'static str,
    pub patterns: &'static [&'static str],
    pub admin: bool,
}

macro_rules! rule {
    ($id:expr, $name:expr, $cat:expr, $risk:expr, $admin:expr, $reason:expr, [$($p:expr),+ $(,)?]) => {
        Rule {
            id: $id,
            name: $name,
            category: $cat,
            risk: $risk,
            reason: $reason,
            patterns: &[$($p),+],
            admin: $admin,
        }
    };
}

pub static RULES: &[Rule] = &[
    rule!("user_temp", "ملفات مؤقتة للمستخدم", "نظام Windows", "safe", false,
        "ملفات مؤقتة تنشئها البرامج أثناء العمل ويمكن حذفها بأمان.",
        ["%TEMP%"]),
    rule!("win_temp", "ملفات Windows المؤقتة", "نظام Windows", "safe", true,
        "مجلد Temp الخاص بالنظام، يحتاج صلاحية مدير.",
        ["%WINDIR%\\Temp"]),
    rule!("inet_cache", "كاش الإنترنت القديم", "نظام Windows", "safe", false,
        "ملفات إنترنت مؤقتة تُعاد تلقائياً.",
        ["%LOCALAPPDATA%\\Microsoft\\Windows\\INetCache"]),
    rule!("thumbcache", "كاش الصور المصغرة", "نظام Windows", "safe", false,
        "تُعيد Windows بناءه تلقائياً عند فتح المجلدات.",
        ["%LOCALAPPDATA%\\Microsoft\\Windows\\Explorer\\thumbcache_*.db"]),
    rule!("crash_dumps", "ملفات انهيار البرامج", "نظام Windows", "safe", false,
        "تقارير أعطال قديمة لا تفيد غير المطورين.",
        ["%LOCALAPPDATA%\\CrashDumps"]),
    rule!("wer", "تقارير الأخطاء", "نظام Windows", "safe", false,
        "تقارير Windows Error Reporting المخزنة.",
        ["%LOCALAPPDATA%\\Microsoft\\Windows\\WER",
         "%PROGRAMDATA%\\Microsoft\\Windows\\WER\\ReportQueue",
         "%PROGRAMDATA%\\Microsoft\\Windows\\WER\\ReportArchive"]),
    rule!("win_update", "تنزيلات Windows Update المخزنة", "نظام Windows", "safe", true,
        "ملفات تحديثات تم تثبيتها وتبقى مخزنة، يحتاج صلاحية مدير.",
        ["%WINDIR%\\SoftwareDistribution\\Download"]),
    rule!("delivery_opt", "كاش Delivery Optimization", "نظام Windows", "safe", true,
        "كاش مشاركة التحديثات، يحتاج صلاحية مدير.",
        ["%WINDIR%\\ServiceProfiles\\NetworkService\\AppData\\Local\\Microsoft\\Windows\\DeliveryOptimization\\Cache"]),
    rule!("chrome_cache", "كاش Google Chrome", "المتصفحات", "safe", false,
        "كاش الصفحات فقط، لا يمس كلمات المرور ولا الكوكيز ولا السجل.",
        ["%LOCALAPPDATA%\\Google\\Chrome\\User Data\\*\\Cache",
         "%LOCALAPPDATA%\\Google\\Chrome\\User Data\\*\\Code Cache",
         "%LOCALAPPDATA%\\Google\\Chrome\\User Data\\*\\GPUCache",
         "%LOCALAPPDATA%\\Google\\Chrome\\User Data\\*\\Service Worker\\CacheStorage"]),
    rule!("edge_cache", "كاش Microsoft Edge", "المتصفحات", "safe", false,
        "كاش الصفحات فقط، لا يمس كلمات المرور ولا الكوكيز ولا السجل.",
        ["%LOCALAPPDATA%\\Microsoft\\Edge\\User Data\\*\\Cache",
         "%LOCALAPPDATA%\\Microsoft\\Edge\\User Data\\*\\Code Cache",
         "%LOCALAPPDATA%\\Microsoft\\Edge\\User Data\\*\\GPUCache",
         "%LOCALAPPDATA%\\Microsoft\\Edge\\User Data\\*\\Service Worker\\CacheStorage"]),
    rule!("brave_cache", "كاش Brave", "المتصفحات", "safe", false,
        "كاش الصفحات فقط.",
        ["%LOCALAPPDATA%\\BraveSoftware\\Brave-Browser\\User Data\\*\\Cache",
         "%LOCALAPPDATA%\\BraveSoftware\\Brave-Browser\\User Data\\*\\Code Cache",
         "%LOCALAPPDATA%\\BraveSoftware\\Brave-Browser\\User Data\\*\\GPUCache"]),
    rule!("firefox_cache", "كاش Firefox", "المتصفحات", "safe", false,
        "كاش الصفحات فقط.",
        ["%LOCALAPPDATA%\\Mozilla\\Firefox\\Profiles\\*\\cache2"]),
    rule!("shader_cache", "كاش الشيدر والرسوميات", "الرسوميات", "safe", false,
        "تعيد اللعبة أو التعريف بناءه تلقائياً (قد يبطؤ أول تشغيل قليلاً).",
        ["%LOCALAPPDATA%\\D3DSCache",
         "%LOCALAPPDATA%\\NVIDIA\\DXCache",
         "%LOCALAPPDATA%\\NVIDIA\\GLCache",
         "%LOCALAPPDATA%\\AMD\\DxCache"]),
    rule!("npm_cache", "كاش npm", "أدوات المطورين", "safe", false,
        "حزم محمّلة سابقاً تُنزَّل عند الحاجة.",
        ["%LOCALAPPDATA%\\npm-cache", "%APPDATA%\\npm-cache"]),
    rule!("pip_cache", "كاش pip", "أدوات المطورين", "safe", false,
        "حزم بايثون المخزنة.",
        ["%LOCALAPPDATA%\\pip\\Cache"]),
    rule!("yarn_cache", "كاش Yarn", "أدوات المطورين", "safe", false,
        "حزم مخزنة تُنزَّل عند الحاجة.",
        ["%LOCALAPPDATA%\\Yarn\\Cache"]),
    rule!("electron_cache", "كاش Electron", "أدوات المطورين", "safe", false,
        "ملفات بناء Electron المحمّلة، تُنزَّل عند الحاجة.",
        ["%LOCALAPPDATA%\\electron\\Cache", "%LOCALAPPDATA%\\electron-builder\\Cache"]),
    rule!("vscode_cache", "كاش VS Code", "تطبيقات", "safe", false,
        "كاش المحرر فقط، لا يمس الإعدادات ولا الإضافات.",
        ["%APPDATA%\\Code\\Cache",
         "%APPDATA%\\Code\\CachedData",
         "%APPDATA%\\Code\\Code Cache",
         "%APPDATA%\\Code\\GPUCache",
         "%APPDATA%\\Code\\CachedExtensionVSIXs"]),
    rule!("discord_cache", "كاش Discord", "تطبيقات", "safe", false,
        "كاش الوسائط والواجهة فقط.",
        ["%APPDATA%\\discord\\Cache",
         "%APPDATA%\\discord\\Code Cache",
         "%APPDATA%\\discord\\GPUCache"]),
    rule!("telegram_cache", "كاش Telegram Desktop", "تطبيقات", "safe", false,
        "وسائط مخزنة مؤقتاً تُنزَّل من السيرفر عند الحاجة، لا يحذف المحادثات.",
        ["%APPDATA%\\Telegram Desktop\\tdata\\user_data\\cache",
         "%APPDATA%\\Telegram Desktop\\tdata\\user_data\\media_cache"]),
    rule!("spotify_cache", "كاش Spotify", "تطبيقات", "review", false,
        "قد يتضمن أغاني محمّلة للاستماع بدون إنترنت.",
        ["%LOCALAPPDATA%\\Spotify\\Data"]),
    rule!("cargo_cache", "كاش Cargo (Rust)", "أدوات المطورين", "review", false,
        "حزم Rust المحمّلة، إعادة تنزيلها تأخذ وقتاً.",
        ["%USERPROFILE%\\.cargo\\registry\\cache"]),
    rule!("gradle_cache", "كاش Gradle", "أدوات المطورين", "review", false,
        "مكتبات مخزنة، إعادة تنزيلها تأخذ وقتاً.",
        ["%USERPROFILE%\\.gradle\\caches"]),
    rule!("nuget_cache", "حزم NuGet", "أدوات المطورين", "review", false,
        "حزم .NET المحمّلة، تُنزَّل عند البناء التالي.",
        ["%USERPROFILE%\\.nuget\\packages"]),
    rule!("ai_models_cache", "نماذج ذكاء اصطناعي محمّلة (HuggingFace/Torch)", "أدوات المطورين", "review", false,
        "نماذج كبيرة محمّلة تلقائياً؛ احذفها فقط إن لم تعد تستخدمها.",
        ["%USERPROFILE%\\.cache\\huggingface", "%USERPROFILE%\\.cache\\torch"]),
];

/// Expand %VAR% placeholders. Returns None if any variable is missing.
pub fn expand(p: &str) -> Option<String> {
    let mut out = String::new();
    let mut rest = p;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let j = after.find('%')?;
        let name = &after[..j];
        let val = std::env::var(name).ok()?;
        out.push_str(&val);
        rest = &after[j + 1..];
    }
    out.push_str(rest);
    Some(out)
}
