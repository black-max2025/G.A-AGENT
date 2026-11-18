const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const dialog = window.__TAURI__.dialog;

const $ = (id) => document.getElementById(id);
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
function setBusy(b) { document.querySelectorAll("main button").forEach((x) => (x.disabled = b)); }

// ---------------------------------------------------------------- tabs
document.querySelectorAll("#tabs button").forEach((b) =>
  b.addEventListener("click", () => {
    document.querySelectorAll("#tabs button").forEach((x) => x.classList.remove("active"));
    document.querySelectorAll(".tab").forEach((x) => x.classList.remove("active"));
    b.classList.add("active");
    $("tab-" + b.dataset.tab).classList.add("active");
    if (b.dataset.tab === "brain") refreshBrain();
    if (b.dataset.tab === "log") loadLog();
    if (b.dataset.tab === "start") loadDrives();
    if (b.dataset.tab === "folders" && !$("ovPath").value) initOverview();
  })
);

// ================================================================ START (simple mode)
let plan = null;
const psel = new Set();

async function loadDrives() {
  try {
    const d = await invoke("disk_info");
    const box = $("driveStrip");
    box.replaceChildren();
    d.forEach((x) => {
      const row = el("div", "drv");
      row.appendChild(el("span", "", `القرص ${x.drive} — المتاح ${fmt(x.free)} من ${fmt(x.total)}`));
      const bar = el("div", "dbar");
      const fill = el("div", "dfill");
      fill.style.width = Math.min(100, ((x.total - x.free) / x.total) * 100) + "%";
      if (x.free / x.total < 0.1) fill.classList.add("low");
      bar.appendChild(fill); row.appendChild(bar); box.appendChild(row);
    });
  } catch (e) { /* ignore */ }
}
async function homeFree() {
  try {
    const home = await invoke("home_dir");
    const letter = home.slice(0, 2).toUpperCase();
    const d = await invoke("disk_info");
    const m = d.find((x) => x.drive.toUpperCase() === letter);
    return m ? m.free : 0;
  } catch (e) { return 0; }
}

function planRow(id, title, desc, sizeText, tip) {
  const d = el("div", "item");
  const cb = document.createElement("input");
  cb.type = "checkbox"; cb.checked = psel.has(id);
  cb.addEventListener("change", () => { cb.checked ? psel.add(id) : psel.delete(id); updatePlanTotal(); });
  const mid = el("div");
  mid.appendChild(el("div", "nm", title));
  mid.appendChild(el("div", "rs", desc));
  if (tip) mid.appendChild(el("div", "pt", tip));
  d.append(cb, mid, el("div", "sz", sizeText));
  return d;
}
function section(title, hint) {
  const c = el("div", "card sect");
  c.appendChild(el("h3", "", title));
  c.appendChild(el("div", "muted", hint));
  return c;
}
function renderPlan() {
  const box = $("planSections");
  box.replaceChildren();
  const safe = plan.cleanup.filter((f) => f.risk === "safe");
  const review = plan.cleanup.filter((f) => f.risk !== "safe");
  const shrink = plan.shrink.filter((c) => c.kind === "shrink");
  const freeze = plan.shrink.filter((c) => c.kind === "freeze");

  if (safe.length) {
    const s = section("🧹 تنظيف آمن", "ملفات مؤقتة وذاكرة مخبأة تُعاد تلقائياً. لا تمس ملفاتك ولا حساباتك.");
    safe.forEach((f) => s.appendChild(planRow(f.id, f.name, f.reason, fmt(f.size), f.path)));
    box.appendChild(s);
  }
  if (shrink.length) {
    const s = section("📦 تصغير بدون حذف", "تُضغط ملفات البرنامج مكانها فتعمل كما هي، والتراجع عنها بضغطة من «السجل».");
    shrink.forEach((c) => s.appendChild(planRow(c.id, c.app, c.explain, "يوفّر ~" + fmt(c.est_saved), c.path)));
    box.appendChild(s);
  }
  if (freeze.length) {
    const s = section("❄ تجميد مجلدات قديمة", "مجلدات لم تُستخدم منذ شهور تُضغط في ملف واحد بعد التحقق منها، ويمكن استرجاعها كاملة. غير محدد افتراضياً.");
    freeze.forEach((c) => s.appendChild(planRow(c.id, c.app, c.explain, "يوفّر ~" + fmt(c.est_saved), c.path)));
    box.appendChild(s);
  }
  if (review.length) {
    const s = section("🟡 يحتاج قرارك", "ملفات كبيرة قابلة لإعادة التحميل. تذهب إلى سلة المحذوفات ويمكن استرجاعها. غير محدد افتراضياً.");
    review.forEach((f) => s.appendChild(planRow(f.id, f.name, f.reason, fmt(f.size), f.path)));
    box.appendChild(s);
  }
  if (!box.children.length) box.appendChild(el("div", "card center", "جهازك نظيف ولا أرى ما يستحق التوفير 🎉"));
  updatePlanTotal();
}
function updatePlanTotal() {
  let total = 0;
  plan.cleanup.forEach((f) => { if (psel.has(f.id)) total += f.size; });
  plan.shrink.forEach((c) => { if (psel.has(c.id)) total += c.est_saved; });
  $("planTotal").textContent = `المحدد: سيوفّر حوالي ${fmt(total)}`;
}

$("btnPlan").onclick = async () => {
  try {
    setBusy(true);
    $("runResult").hidden = true;
    $("planStatus").textContent = "جارٍ فحص جهازك... قد يستغرق دقيقة أو أكثر.";
    plan = await invoke("plan");
    psel.clear();
    plan.cleanup.forEach((f) => { if (f.risk === "safe") psel.add(f.id); });
    plan.shrink.forEach((c) => { if (c.kind === "shrink") psel.add(c.id); });
    $("planCard").hidden = false;
    $("planStatus").textContent = "هذه خطتي. عدّل التحديد كما تشاء ثم اضغط «نفّذ».";
    renderPlan();
    loadDrives();
  } catch (e) { $("planStatus").textContent = "حدث خطأ: " + e; }
  finally { setBusy(false); }
};

listen("shrink-progress", (ev) => {
  const p = ev.payload;
  $("planStatus").textContent = `جارٍ التصغير (${p.i + 1} من ${p.total}): ${p.app} — قد يستغرق بعض الوقت، لا تُغلق التطبيق.`;
});

$("btnRun").onclick = async () => {
  const cleanIds = plan.cleanup.filter((f) => psel.has(f.id)).map((f) => f.id);
  const shrinkIds = plan.shrink.filter((c) => psel.has(c.id)).map((c) => c.id);
  if (!cleanIds.length && !shrinkIds.length) return toast("لم تحدد شيئاً");
  const reviewN = plan.cleanup.filter((f) => psel.has(f.id) && f.risk !== "safe").length;
  const freezeN = plan.shrink.filter((c) => psel.has(c.id) && c.kind === "freeze").length;
  let msg = "سأنفّذ الخطة المحددة.\n";
  msg += "• الكاش الآمن يُحذف نهائياً (يُعاد تلقائياً).\n";
  if (shrinkIds.length) msg += "• التصغير بدون حذف ويمكن التراجع عنه.\n";
  if (freezeN) msg += `• ${freezeN} مجلد سيُجمَّد: يُضغط ويُتحقق منه ثم يُحذف الأصل، ويمكن استرجاعه.\n`;
  if (reviewN) msg += `• ${reviewN} عنصر سينتقل إلى سلة المحذوفات.\n`;
  msg += "\nهل تتابع؟";
  if (!confirm(msg)) return;
  try {
    setBusy(true);
    const before = await homeFree();
    let cleaned = 0, saved = 0;
    const notes = [];
    if (cleanIds.length) {
      $("planStatus").textContent = "جارٍ التنظيف...";
      const rep = await invoke("clean", { ids: cleanIds });
      cleaned = rep.freed;
      rep.results.filter((r) => !r.ok).forEach((r) => notes.push(`${r.name}: ${r.note}`));
    }
    if (shrinkIds.length) {
      const rep = await invoke("shrink_apply", { ids: shrinkIds });
      saved = rep.saved;
      rep.results.forEach((r) => notes.push(`${r.app}: ${r.note}`));
    }
    const after = await homeFree();
    const box = $("runResult");
    box.hidden = false; box.replaceChildren();
    box.appendChild(el("h3", "", "اكتملت الخطة ✅"));
    box.appendChild(el("div", "", `تنظيف: ${fmt(cleaned)} — تصغير/تجميد: ${fmt(saved)}`));
    if (before && after) box.appendChild(el("div", "", `المساحة الفارغة الآن: ${fmt(after)} (كانت ${fmt(before)})`));
    if (reviewN) box.appendChild(el("div", "muted", "العناصر المنقولة إلى سلة المحذوفات تظهر مساحتها بعد إفراغ السلة."));
    notes.forEach((n) => box.appendChild(el("div", "rs", "• " + n)));
    $("planStatus").textContent = "";
    $("planCard").hidden = true;
    plan = null;
    loadDrives();
  } catch (e) { $("planStatus").textContent = "حدث خطأ: " + e; }
  finally { setBusy(false); }
};

// ================================================================ ASSISTANT
function addMsg(cls, text) { const m = el("div", "msg " + cls, text); $("chat").appendChild(m); $("chat").scrollTop = 1e9; return m; }
let pendingOpen = false;
function showTurn(turn, holder) {
  const parts = [];
  if (turn.log && turn.log.length) {
    const row = el("div");
    turn.log.forEach((l) => row.appendChild(el("span", "chip", "🔧 " + l)));
    holder.parentNode.insertBefore(row, holder);
  }
  holder.textContent = turn.text || (turn.pending ? "أحتاج موافقتك على الإجراء التالي." : "...");
  if (turn.pending) openModal(turn.pending);
}
function openModal(p) {
  pendingOpen = true;
  $("mTool").textContent = p.tool;
  $("mText").textContent = p.summary;
  $("modal").hidden = false;
}
async function answer(approve) {
  $("modal").hidden = true; pendingOpen = false;
  const w = addMsg("ai", approve ? "جارٍ التنفيذ..." : "تم الإلغاء...");
  try { showTurn(await invoke("agent_confirm", { approve }), w); } catch (e) { w.textContent = "خطأ: " + e; }
}
$("mYes").onclick = () => answer(true);
$("mNo").onclick = () => answer(false);
async function send() {
  const t = $("chatIn").value.trim(); if (!t || pendingOpen) return;
  $("chatIn").value = ""; addMsg("me", t);
  const w = addMsg("ai", "أفكّر...");
  try { showTurn(await invoke("agent_send", { message: t }), w); } catch (e) { w.textContent = "خطأ: " + e; }
}
$("btnSend").onclick = send;
$("chatIn").addEventListener("keydown", (e) => { if (e.key === "Enter") send(); });
$("btnResetChat").onclick = async () => { await invoke("agent_reset"); $("chat").replaceChildren(); };

// ================================================================ LOG
async function loadLog() {
  const box = $("logList"); box.replaceChildren();
  try {
    const list = await invoke("journal_list");
    if (!list.length) { box.appendChild(el("div", "muted", "لا توجد عمليات بعد.")); return; }
    list.forEach((e) => {
      const r = el("div", "lg" + (e.undone ? " undone" : ""));
      const left = el("div");
      left.appendChild(el("div", "nm", (e.kind === "freeze" ? "❄ تجميد: " : "📦 تصغير: ") + e.app));
      left.appendChild(el("div", "rs", `${fmt(e.before)} ← ${fmt(e.after)} — وفّر ${fmt(Math.max(0, e.before - e.after))} — ${new Date(e.time * 1000).toLocaleString("ar-EG")}`));
      left.appendChild(el("div", "pt", e.path));
      const btn = el("button", "", e.undone ? "تم التراجع" : "تراجع");
      btn.disabled = e.undone;
      btn.onclick = async () => {
        if (!confirm("هل تريد التراجع عن هذه العملية؟ " + (e.kind === "freeze" ? "سيُفك الأرشيف ويعود المجلد كما كان وقد يحتاج مساحة مؤقتة." : ""))) return;
        try { setBusy(true); await invoke("journal_undo", { id: e.id }); toast("تم التراجع"); } catch (er) { toast("خطأ: " + er, 8000); }
        finally { setBusy(false); loadLog(); }
      };
      r.append(left, el("div"), btn);
      box.appendChild(r);
    });
  } catch (e) { box.textContent = "خطأ: " + e; }
}
$("btnLogRefresh").onclick = loadLog;

// ================================================================ ADVANCED CLEAN
let findings = [];
const checked = new Set();
const verdicts = {};
function render() {
  $("resultCard").hidden = findings.length === 0;
  const list = $("list"); list.replaceChildren();
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
  d.append(cb, mid, el("div", "sz", fmt(f.size)));
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
    setBusy(true);
    $("status").textContent = kind === "quick" ? "جارٍ فحص مواقع الكاش المعروفة..." : "جارٍ الفحص العميق، قد يستغرق دقائق...";
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
    setBusy(true); $("status").textContent = "جارٍ التنظيف...";
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
    setBusy(true); $("status").textContent = "العقل يحلل العناصر...";
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

// ================================================================ FOLDERS
const KIND = { media: "وسائط", docs: "مستندات", code: "برمجة", archives: "أرشيفات", programs: "برامج", temp: "مؤقت/سجلات", other: "متنوع" };
const CLS = { system: ["نظام", "b-sys"], user: ["شخصي", "b-usr"], regenerable: ["يُعاد توليده", "b-reg"], normal: ["عادي", "b-nor"] };
async function initOverview() { $("ovPath").value = await invoke("home_dir"); loadOverview(); }
async function loadOverview() {
  const path = $("ovPath").value.trim();
  const box = $("ovList"); box.textContent = "جارٍ التحليل...";
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

// ================================================================ BRAIN
const HF = "https://huggingface.co/";
const MODELS = {
  q35: { name: "Qwen3.5-2B-Q4_K_M.gguf", urls: [
    HF + "unsloth/Qwen3.5-2B-GGUF/resolve/main/Qwen3.5-2B-Q4_K_M.gguf",
    HF + "volantis-labs/qwen3.5-2b/resolve/main/Qwen3.5-2B-Q4_K_M.gguf",
    HF + "Repasar-Labs/qwen3.5-2b-gguf/resolve/main/Qwen3.5-2B-Q4_K_M.gguf"] },
  "1.5": { name: "qwen2.5-1.5b-instruct-q4_k_m.gguf", urls: [HF + "Qwen/Qwen2.5-1.5B-Instruct-GGUF/resolve/main/qwen2.5-1.5b-instruct-q4_k_m.gguf"] },
  "3": { name: "qwen2.5-3b-instruct-q4_k_m.gguf", urls: [HF + "Qwen/Qwen2.5-3B-Instruct-GGUF/resolve/main/qwen2.5-3b-instruct-q4_k_m.gguf"] },
};
async function downloadModel(m) {
  let last = "تعذر التنزيل";
  for (const u of m.urls) {
    try { return await invoke("agent_download_model", { url: u, fileName: m.name }); } catch (e) { last = e; }
  }
  throw last;
}
let lastStatus = null;
async function refreshBrain() {
  try {
    const s = await invoke("agent_status");
    lastStatus = s;
    $("dataDir").value = s.data_dir;
    $("allowCmd").checked = s.allow_commands;
    $("brainStatus").textContent =
      `المحرك: ${s.server_installed ? "✅ مثبّت" : "❌ غير مثبّت"}  |  النموذج: ${s.model_path ? (s.model_exists ? "✅ " + s.model_path : "⚠ مفقود: " + s.model_path) : "❌ لم يُختر"}  |  الحالة: ${s.running ? "🟢 يعمل" : "⚪ متوقف"}  |  مكان الملفات: ${s.data_dir}`;
    return s;
  } catch (e) { $("brainStatus").textContent = "خطأ: " + e; }
}
listen("download-progress", (ev) => {
  const p = ev.payload;
  const pct = p.total ? Math.round((p.got / p.total) * 100) + "%" : fmt(p.got);
  $("dlProgress").textContent = `تنزيل ${p.label}: ${fmt(p.got)} / ${p.total ? fmt(p.total) : "?"} (${pct})`;
});
$("btnPickDir").onclick = async () => { const p = await dialog.open({ directory: true }); if (p) $("dataDir").value = p; };
$("btnSaveDir").onclick = async () => {
  try { await invoke("agent_set_data_dir", { path: $("dataDir").value.trim() }); toast("تم حفظ المكان. التنزيلات الجديدة ستذهب إليه."); }
  catch (e) { toast("خطأ: " + e, 8000); }
  refreshBrain();
};
$("btnAuto").onclick = async () => {
  try {
    setBusy(true);
    let s = await refreshBrain();
    if (!s.server_installed) { $("dlProgress").textContent = "تثبيت المحرك..."; await invoke("agent_install_server"); s = await refreshBrain(); }
    if (!s.model_path || !s.model_exists) { await downloadModel(MODELS.q35); s = await refreshBrain(); }
    $("dlProgress").textContent = "جارٍ تحميل النموذج في الذاكرة...";
    await invoke("agent_start");
    $("dlProgress").textContent = "";
    toast("العقل جاهز ويعمل 🎉");
  } catch (e) { toast("خطأ: " + e, 9000); $("dlProgress").textContent = ""; }
  finally { setBusy(false); refreshBrain(); }
};
document.querySelectorAll("[data-dl]").forEach((b) => (b.onclick = async () => {
  try { setBusy(true); await downloadModel(MODELS[b.dataset.dl]); toast("تم تنزيل النموذج"); }
  catch (e) { toast("خطأ: " + e, 8000); }
  finally { setBusy(false); refreshBrain(); }
}));
$("btnPickModel").onclick = async () => {
  const p = await dialog.open({ filters: [{ name: "GGUF", extensions: ["gguf"] }] });
  if (!p) return;
  try { await invoke("agent_set_model", { path: p }); toast("تم اختيار النموذج وسيبقى في مكانه"); } catch (e) { toast("خطأ: " + e, 8000); }
  refreshBrain();
};
$("btnStart").onclick = async () => {
  try { setBusy(true); $("dlProgress").textContent = "جارٍ تحميل النموذج في الذاكرة..."; await invoke("agent_start"); $("dlProgress").textContent = ""; toast("العقل يعمل الآن"); }
  catch (e) { toast("خطأ: " + e, 8000); }
  finally { setBusy(false); refreshBrain(); }
};
$("btnStop").onclick = async () => { await invoke("agent_stop"); refreshBrain(); };
$("allowCmd").onchange = async (e) => {
  const on = e.target.checked;
  if (on && !confirm("تفعيل الأوامر يسمح للوكيل باقتراح أوامر على جهازك. كل أمر سيطلب موافقتك. هل تريد التفعيل؟")) { e.target.checked = false; return; }
  try { await invoke("agent_set_allow_commands", { allow: on }); } catch (er) { toast("خطأ: " + er); }
};

loadDrives();
