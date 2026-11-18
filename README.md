# G.A AGENT

منظّف ويندوز ذكي وآمن (Tauri + Rust) بعقل ذكاء اصطناعي محلي (GGUF عبر llama.cpp).

- **فحص سريع**: مواقع كاش معروفة (متصفحات، Windows، npm/pip، Discord، Telegram، VS Code ...).
- **فحص عميق**: node_modules/target/venv القديمة، مكررات (SHA-256)، ملفات كبيرة قديمة، مثبّتات قديمة.
- **تحليل الفولدرات**: حجم ونوع بيانات كل مجلد وتصنيفه (نظام/شخصي/يُعاد توليده).
- **العقل (AI)**: نموذج GGUF محلي يقترح DELETE/KEEP/ASK مع سبب؛ القرار النهائي لطبقة الأمان.

## الأمان
- الواجهة لا ترسل مسارات للحذف؛ فقط معرّفات نتائج فحص.
- قائمة محمية (Windows, Program Files, ProgramData, AppData, الجذر، المجلدات الشخصية).
- لا يتبع الروابط/Junctions، ويتخطى ملفات OneDrive غير المحمّلة.
- الكاش الآمن يُحذف نهائياً؛ غيره يذهب إلى سلة المحذوفات.

## البناء
GitHub Actions (`.github/workflows/build.yml`) يبني `ga-agent.exe` ومثبّت NSIS على `windows-latest`.
نزّل الملفات من تبويب **Actions → آخر تشغيل → Artifacts**، أو ادفع وسم `v0.1.0` لإنشاء Release.

محلياً: `npm install && npm run tauri -- dev` (يتطلب Rust وWebView2).
