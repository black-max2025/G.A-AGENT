const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const dialog = window.__TAURI__.dialog;

const $ = (id) => document.getElementById(id);
let findings = [];
const checked = new Set();
const verdicts = {};

function fmt(n) {
  if (!n) return "0 B";
  const u = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return n.toFixed(i > 1 ? 2 : 0) + " " + u[i];
}
function toast(t, ms = 4000) {
  const el = $("toast"); el.textContent = t; el.style.display = "block";
  clearTimeout(toast._t); toast._t = setTimeout(() => (el.style.display = "none"), ms);
}
function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}
function setBusy(b, msg) { document.querySelectorAll("button").forEach((x) => (x.disabled = b)); if (msg) $("status").textContent = msg; }

// ---------- tabs
document.querySelectorAll("#tabs button").forEach((b) =>
  b.addEventListener("click", () => {
    document.querySelectorAll("#tabs button").forEach((x) => x.classList.remove("active"));
    document.querySelectorAll(".tab").forEach((x) => x.classList.remove("active"));
    b.classList.add("active");
    $("tab-" + b.dataset.tab).classList.add("active");
    if (b.dataset.tab === "brain") refreshBrain();
    if (b.dataset.tab === "folders" && !$("ovPath").value) initOverview();
  })
);

// ---------- cleaning
function render() {
  $("resultCard").hidden = findings.length === 0;
  const list = $("list");
  list.replaceChildren();
  const groups = { safe: [], review: [] };
  findings.forEach((f) => groups[f.risk === "safe" ? "safe" : "review"].push(f));
  const titles = { safe: "🟢 آمن: كاش وملفات مؤقتة تُعاد تلقائياً", review: "🟡 يحتاج مراجعتك: يُنقل إلى سلة المحذوفات" };
  for (const k of ["safe", "review"]) {
    if (!groups[k].length) continue;
    list.appendChild(el("div", "gh", titles[k]));
    groups[k].forEach((f) => list.appendChild(row(f)));
  }
  updateSummary();
}
function row(f) {
  const d = el("div", "item");
  const cb = document.createElement("input");
  cb.type = "checkbox"; cb.checked = checked.has(f.id);
  cb.addEventListener("change", () => { cb.checked ? checked.add(f.id) : checked.delete(f.id); updateSummary(); });
  const mid = el("div");
  const nm = el("div", "nm", f.name);
  nm.appendChild(el("span", "badge " + (f.risk === "safe" ? "b-safe" : "b-review"), f.category));
  const v = verdicts[f.id];
  if (v) nm.appendChild(el("span", "badge b-" + (v.verdict === "delete" ? "del" : v.verdict === "keep" ? "keep" : "ask"), "AI: " + { delete: "احذف", keep: "أبقِ", ask: "راجع" }[v.verdict]));
  if (f.admin) nm.appendChild(el("span", "badge b-ask", "يحتاج مدير"));
  mid.appendChild(nm);
  mid.appendChild(el("div", "rs", (v && v.note ? "🤖 " + v.note + " — " : "") + f.reason));
  mid.appendChild(el("div", "pt", f.path));
  const sz = el("div", "sz", fmt(f.size));
  d.append(cb, mid, sz);
  return d;
}
function updateSummary() {
  const total = findings.reduce((a, f) => a + f.size, 0);
  const sel = findings.filter((f) => checked.has(f.id)).reduce((a, f) => a + f.size, 0);
  $("summary").textContent = `${findings.length} عنصر — الإجمالي ${fmt(total)} — المحدد ${fmt(sel)}`;
}
function setFindings(list) {
  findings = list; checked.clear();
  for (const k in verdicts) delete verdicts[k];
  list.forEach((f) => { if (f.risk === "safe") checked.add(f.id); });
  render();
}
async function scan(kind) {
  try {
    setBusy(true, kind === "quick" ? "جارٍ فحص مواقع الكاش المعروفة..." : "جارٍ الفحص العميق، قد يستغرق دقائق...");
    const list = kind === "quick" ? await invoke("scan_system") : await invoke("scan_folder", { root: $("deepPath").value.trim() });
    setFindings(list);
    $("status").textContent = list.length ? "اكتمل الفحص. راجع القائمة ثم اضغط «نظّف المحدد»." : "لم يُعثر على شيء يستحق التنظيف 🎉";
  } catch (e) { $("status").textContent = "خطأ: " + e; }
  finally { setBusy(false); }
}
$("btnQuick").onclick = () => scan("quick");
$("btnDeep").onclick = () => { if (!$("deepPath").value.trim()) return toast("اكتب مساراً أو اضغط استعراض"); scan("deep"); };
$("btnBrowse").onclick = async () => { const p = await dialog.open({ directory: true }); if (p) $("deepPath").value = p; };
$("btnSelSafe").onclick = () => { checked.clear(); findings.filter((f) => f.risk === "safe").forEach((f) => checked.add(f.id)); render(); };
$("btnSelNone").onclick = () => { checked.clear(); render(); };
$("btnClean").onclick = async () => {
  const ids = [...checked];
  if (!ids.length) return toast("لم تحدد شيئاً");
  const sel = findings.filter((f) => checked.has(f.id));
  const rev = sel.filter((f) => f.risk !== "safe").length;
  const msg = `سيتم تنظيف ${ids.length} عنصر (${fmt(sel.reduce((a, f) => a + f.size, 0))}).\n` +
    (rev ? `منها ${rev} عنصر سيُنقل إلى سلة المحذوفات (قابل للاسترجاع).\n` : "") + "الكاش الآمن يُحذف نهائياً. هل تتابع؟";
  if (!confirm(msg)) return;
  try {
    setBusy(true, "جارٍ التنظيف...");
    const rep = await invoke("clean", { ids });
    const done = new Set(rep.results.filter((r) => r.ok).map((r) => r.id));
    findings = findings.filter((f) => !done.has(f.id));
    done.forEach((i) => checked.delete(i));
    render();
    const failed = rep.results.filter((r) => !r.ok).length;
    $("status").textContent = `تم تحرير ${fmt(rep.freed)}` + (failed ? ` — ${failed} عنصر لم يُحذف (قيد الاستخدام أو محمي)` : "");
    toast("تم تحرير " + fmt(rep.freed));
  } catch (e) { $("status").textContent = "خطأ: " + e; }
  finally { setBusy(false); }
};
$("btnAi").onclick = async () => {
  if (!findings.length) return;
  try {
    setBusy(true, "العقل يحلل العناصر...");
    const vs = await invoke("agent_analyze", { ids: findings.map((f) => f.id) });
    vs.forEach((v) => {
      verdicts[v.id] = v;
      if (v.verdict === "keep") checked.delete(v.id);
      if (v.verdict === "delete") checked.add(v.id);
    });
    render();
    $("status").textContent = `حلّل العقل ${vs.length} عنصراً. راجع الشارات ثم نفّذ.`;
  } catch (e) { $("status").textContent = "خطأ: " + e; }
  finally { setBusy(false); }
};

// ---------- folders
const KIND = { media: "وسائط", docs: "مستندات", code: "برمجة", archives: "أرشيفات", programs: "برامج", temp: "مؤقت/سجلات", other: "متنوع" };
const CLS = { system: ["نظام", "b-sys"], user: ["شخصي", "b-usr"], regenerable: ["يُعاد توليده", "b-reg"], normal: ["عادي", "b-nor"] };
async function initOverview() {
  $("ovPath").value = await invoke("home_dir");
  loadOverview();
}
async function loadOverview() {
  const path = $("ovPath").value.trim();
  const box = $("ovList");
  box.textContent = "جارٍ التحليل...";
  try {
    const items = await invoke("folder_overview", { path });
    box.replaceChildren();
    const max = Math.max(1, ...items.map((i) => i.size));
    items.forEach((i) => {
      const r = el("div", "ov");
      const left = el("div");
      const nm = el("div", "nm", (i.is_dir ? "📁 " : "📄 ") + i.name);
      const c = CLS[i.class] || CLS.normal;
      nm.appendChild(el("span", "badge " + c[1], c[0]));
      left.appendChild(nm);
      const bar = el("div", "bar"); bar.style.width = Math.max(2, (i.size / max) * 100) + "%";
      left.appendChild(bar);
      r.append(left, el("div", "muted", KIND[i.dominant] || ""), el("div", "muted", i.is_dir ? i.files + " ملف" : ""), el("div", "sz", fmt(i.size)));
      if (i.is_dir) r.ondblclick = () => { $("ovPath").value = i.path; loadOverview(); };
      box.appendChild(r);
    });
    if (!items.length) box.textContent = "المجلد فارغ أو لا يمكن قراءته.";
  } catch (e) { box.textContent = "خطأ: " + e; }
}
$("btnOv").onclick = loadOverview;
$("btnUp").onclick = () => {
  const p = $("ovPath").value.replace(/[\\/]+$/, "");
  const i = Math.max(p.lastIndexOf("\\"), p.lastIndexOf("/"));
  if (i > 2) { $("ovPath").value = p.slice(0, i); loadOverview(); }
};

// ---------- brain
const MODELS = {
  "1.5": ["https://huggingface.co/Qwen/Qwen2.5-1.5B-Instruct-GGUF/resolve/main/qwen2.5-1.5b-instruct-q4_k_m.gguf", "qwen2.5-1.5b-instruct-q4_k_m.gguf"],
  "3": ["https://huggingface.co/Qwen/Qwen2.5-3B-Instruct-GGUF/resolve/main/qwen2.5-3b-instruct-q4_k_m.gguf", "qwen2.5-3b-instruct-q4_k_m.gguf"],
};
async function refreshBrain() {
  try {
    const s = await invoke("agent_status");
    $("brainStatus").textContent =
      `المحرك: ${s.server_installed ? "✅ مثبّت" : "❌ غير مثبّت"}  |  النموذج: ${s.model_path ? (s.model_exists ? "✅ " + s.model_path : "⚠ مفقود: " + s.model_path) : "❌ لم يُختر"}  |  الحالة: ${s.running ? "🟢 يعمل" : "⚪ متوقف"}`;
  } catch (e) { $("brainStatus").textContent = "خطأ: " + e; }
}
listen("download-progress", (ev) => {
  const p = ev.payload;
  const pct = p.total ? Math.round((p.got / p.total) * 100) + "%" : fmt(p.got);
  $("dlProgress").textContent = `تنزيل ${p.label}: ${fmt(p.got)} / ${p.total ? fmt(p.total) : "?"} (${pct})`;
});
$("btnInstallServer").onclick = async () => {
  try { $("dlProgress").textContent = "جارٍ التنزيل..."; await invoke("agent_install_server"); toast("تم تثبيت المحرك"); }
  catch (e) { toast("خطأ: " + e, 8000); }
  refreshBrain();
};
document.querySelectorAll("[data-dl]").forEach((b) => (b.onclick = async () => {
  const [url, name] = MODELS[b.dataset.dl];
  try { await invoke("agent_download_model", { url, fileName: name }); toast("تم تنزيل النموذج"); }
  catch (e) { toast("خطأ: " + e, 8000); }
  refreshBrain();
}));
$("btnPickModel").onclick = async () => {
  const p = await dialog.open({ filters: [{ name: "GGUF", extensions: ["gguf"] }] });
  if (!p) return;
  try { await invoke("agent_set_model", { path: p }); toast("تم اختيار النموذج"); } catch (e) { toast("خطأ: " + e, 8000); }
  refreshBrain();
};
$("btnStart").onclick = async () => {
  try { $("dlProgress").textContent = "جارٍ تحميل النموذج في الذاكرة..."; await invoke("agent_start"); $("dlProgress").textContent = ""; toast("العقل يعمل الآن"); }
  catch (e) { toast("خطأ: " + e, 8000); }
  refreshBrain();
};
$("btnStop").onclick = async () => { await invoke("agent_stop"); refreshBrain(); };

function addMsg(cls, text) { const m = el("div", "msg " + cls, text); $("chat").appendChild(m); $("chat").scrollTop = 1e9; return m; }
async function send() {
  const t = $("chatIn").value.trim(); if (!t) return;
  $("chatIn").value = ""; addMsg("me", t);
  const ctx = findings.slice(0, 20).map((f) => `- ${f.name} (${f.risk === "safe" ? "آمن" : "مراجعة"}, ${fmt(f.size)}): ${f.reason}`).join("\n") || "لا توجد نتائج فحص بعد.";
  const w = addMsg("ai", "...");
  try { w.textContent = await invoke("agent_chat", { message: t, context: ctx }); } catch (e) { w.textContent = "خطأ: " + e; }
}
$("btnSend").onclick = send;
$("chatIn").addEventListener("keydown", (e) => { if (e.key === "Enter") send(); });
