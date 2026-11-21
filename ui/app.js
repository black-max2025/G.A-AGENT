const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const dialog = window.__TAURI__.dialog;

const $ = (id) => document.getElementById(id);
const el = (tag, cls, text) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
};
function fmt(n) {
  if (!n) return "0 B";
  const u = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return n.toFixed(i > 1 ? 2 : 0) + " " + u[i];
}
function toast(text, ms = 4500) {
  const b = $("toast"); b.textContent = text; b.style.display = "block";
  clearTimeout(toast._t); toast._t = setTimeout(() => (b.style.display = "none"), ms);
}
// Backend errors are codes like "ERR_PATH_NOT_ALLOWED" (optionally with ": detail").
function errText(e) {
  const s = String(e && e.message ? e.message : e);
  const m = s.match(/ERR_[A-Z_]+/);
  if (!m) return s;
  const k = "err_" + m[0].slice(4).toLowerCase();
  return I18N.has(k) ? t(k) : t("err_generic", { c: m[0] });
}
const fail = (e, ms = 8000) => toast(t("error") + ": " + errText(e), ms);

// Own confirmation dialog (localized, unlike the browser's confirm()).
function ask(text) {
  return new Promise((res) => {
    $("dlgText").textContent = text; $("dlg").hidden = false;
    const done = (v) => { $("dlg").hidden = true; res(v); };
    $("dlgYes").onclick = () => done(true);
    $("dlgNo").onclick = () => done(false);
  });
}

// ---------------------------------------------------------------- button feedback
document.addEventListener("pointerdown", (ev) => {
  const b = ev.target.closest && ev.target.closest("button");
  if (!b || b.disabled) return;
  b.classList.add("pressed");
  setTimeout(() => b.classList.remove("pressed"), 180);
  const r = b.getBoundingClientRect(), d = Math.max(r.width, r.height);
  const s = el("span", "ripple");
  s.style.width = s.style.height = d + "px";
  s.style.left = ev.clientX - r.left - d / 2 + "px";
  s.style.top = ev.clientY - r.top - d / 2 + "px";
  b.appendChild(s);
  setTimeout(() => s.remove(), 600);
});
function setBusy(b) {
  document.querySelectorAll("main button").forEach((x) => (x.disabled = b));
  if (!b) document.querySelectorAll("button.busy").forEach((x) => x.classList.remove("busy"));
}
async function withBusy(btn, fn) {
  setBusy(true);
  if (btn) { btn.disabled = false; btn.classList.add("busy"); }
  try { return await fn(); } finally { if (btn) btn.classList.remove("busy"); setBusy(false); }
}

// ---------------------------------------------------------------- navigation + language
let page = "home";
const loaders = {
  home: () => loadDrives(),
  chat: () => refreshPlan(),
  space: () => { vaultInfo(); if (!$("ovPath").value) initOverview(); },
  tasks: () => { loadTasks(); loadMemory(); },
  accounts: () => { loadMail(); loadTg(); loadInbox(); },
  journal: () => loadLog(),
  settings: () => { refreshBrain(); loadDomains(); fillSafe(); },
};
function go(name) {
  page = name;
  document.querySelectorAll("#tabs button").forEach((x) => x.classList.toggle("active", x.dataset.tab === name));
  document.querySelectorAll(".tab").forEach((x) => x.classList.toggle("active", x.id === "tab-" + name));
  if (loaders[name]) loaders[name]();
}
document.querySelectorAll("#tabs button").forEach((b) => b.addEventListener("click", () => go(b.dataset.tab)));
$("brainPill").onclick = () => go("settings");

$("langSel").value = I18N.lang();
$("langSel").onchange = async (e) => {
  I18N.set(e.target.value);
  try { await invoke("agent_set_lang", { lang: e.target.value }); } catch (er) { /* ignore */ }
  buildStatic(); go(page);
};
const locale = () => ({ ar: "ar-EG", en: "en-GB", id: "id-ID", fr: "fr-FR", it: "it-IT" }[I18N.lang()]);
const when = (ts) => new Date(ts * 1000).toLocaleString(locale());

function fillSafe() {
  const ul = $("safeList"); ul.replaceChildren();
  for (let i = 1; i <= 8; i++) ul.appendChild(el("li", "", t("safe_" + i)));
}
function buildStatic() {
  $("tParamLbl").textContent = t("param_" + $("tKind").value);
  fillSuggest(); tickNow(); fillSafe(); refreshPill();
}

// ================================================================ HOME
let plan = null;
const psel = new Set();

async function loadDrives() {
  try {
    const d = await invoke("disk_info");
    const box = $("driveStrip"); box.replaceChildren();
    d.forEach((x) => {
      const row = el("div", "drv");
      row.appendChild(el("span", "", t("drive_line", { d: x.drive, free: fmt(x.free), total: fmt(x.total) })));
      const bar = el("div", "dbar"), fill = el("div", "dfill");
      fill.style.width = Math.min(100, ((x.total - x.free) / x.total) * 100) + "%";
      if (x.free / x.total < 0.1) fill.classList.add("low");
      bar.appendChild(fill); row.appendChild(bar); box.appendChild(row);
    });
  } catch (e) { /* ignore */ }
}
async function homeFree() {
  try {
    const letter = (await invoke("home_dir")).slice(0, 2).toUpperCase();
    const m = (await invoke("disk_info")).find((x) => x.drive.toUpperCase() === letter);
    return m ? m.free : 0;
  } catch (e) { return 0; }
}
function planRow(id, title, desc, size, tip) {
  const d = el("div", "item");
  const cb = document.createElement("input");
  cb.type = "checkbox"; cb.checked = psel.has(id);
  cb.onchange = () => { cb.checked ? psel.add(id) : psel.delete(id); updatePlanTotal(); };
  const mid = el("div");
  mid.append(el("div", "nm", title), el("div", "rs", desc));
  if (tip) mid.appendChild(el("div", "pt", tip));
  d.append(cb, mid, el("div", "sz", size));
  return d;
}
function section(title, hint) {
  const c = el("div", "card sect");
  c.append(el("h3", "", title), el("div", "muted", hint));
  return c;
}
function renderPlan() {
  const box = $("planSections"); box.replaceChildren();
  const safe = plan.cleanup.filter((f) => f.risk === "safe");
  const review = plan.cleanup.filter((f) => f.risk !== "safe");
  const shrink = plan.shrink.filter((c) => c.kind === "shrink");
  const freeze = plan.shrink.filter((c) => c.kind === "freeze");
  const add = (list, tk, rowf) => {
    if (!list.length) return;
    const s = section(t(tk), t(tk + "_h"));
    list.forEach((x) => s.appendChild(rowf(x)));
    box.appendChild(s);
  };
  add(safe, "sec_safe", (f) => planRow(f.id, f.name, f.reason, fmt(f.size), f.path));
  add(shrink, "sec_shrink", (c) => planRow(c.id, c.app, c.explain, "~" + fmt(c.est_saved), c.path));
  add(freeze, "sec_freeze", (c) => planRow(c.id, c.app, c.explain, "~" + fmt(c.est_saved), c.path));
  add(review, "sec_review", (f) => planRow(f.id, f.name, f.reason, fmt(f.size), f.path));
  if (!box.children.length) box.appendChild(el("div", "card center", t("nothing_to_do")));
  updatePlanTotal();
}
function updatePlanTotal() {
  let total = 0;
  plan.cleanup.forEach((f) => { if (psel.has(f.id)) total += f.size; });
  plan.shrink.forEach((c) => { if (psel.has(c.id)) total += c.est_saved; });
  $("planTotal").textContent = t("plan_total", { n: fmt(total) });
}
$("btnPlan").onclick = async () => {
  try {
    setBusy(true); $("runResult").hidden = true;
    $("planStatus").textContent = t("scanning");
    plan = await invoke("plan");
    psel.clear();
    plan.cleanup.forEach((f) => { if (f.risk === "safe") psel.add(f.id); });
    plan.shrink.forEach((c) => { if (c.kind === "shrink") psel.add(c.id); });
    $("planCard").hidden = false;
    $("planStatus").textContent = t("plan_ready");
    renderPlan(); loadDrives();
  } catch (e) { $("planStatus").textContent = t("error") + ": " + errText(e); }
  finally { setBusy(false); }
};
listen("shrink-progress", (ev) => {
  const p = ev.payload;
  $("planStatus").textContent = t("shrinking", { i: p.i + 1, n: p.total, app: p.app });
});
$("btnRun").onclick = async () => {
  const cleanIds = plan.cleanup.filter((f) => psel.has(f.id)).map((f) => f.id);
  const shrinkIds = plan.shrink.filter((c) => psel.has(c.id)).map((c) => c.id);
  if (!cleanIds.length && !shrinkIds.length) return toast(t("nothing_selected"));
  const reviewN = plan.cleanup.filter((f) => psel.has(f.id) && f.risk !== "safe").length;
  const freezeN = plan.shrink.filter((c) => psel.has(c.id) && c.kind === "freeze").length;
  let msg = t("cf_head") + "\n• " + t("cf_safe");
  if (shrinkIds.length) msg += "\n• " + t("cf_shrink");
  if (freezeN) msg += "\n• " + t("cf_freeze", { n: freezeN });
  if (reviewN) msg += "\n• " + t("cf_review", { n: reviewN });
  if (!(await ask(msg + "\n\n" + t("cf_go")))) return;
  try {
    setBusy(true);
    const before = await homeFree();
    let cleaned = 0, saved = 0;
    const notes = [];
    if (cleanIds.length) {
      $("planStatus").textContent = t("cleaning");
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
    const box = $("runResult"); box.hidden = false; box.replaceChildren();
    box.appendChild(el("h3", "", t("done")));
    box.appendChild(el("div", "", t("done_line", { c: fmt(cleaned), s: fmt(saved) })));
    if (before && after) box.appendChild(el("div", "", t("free_now", { a: fmt(after), b: fmt(before) })));
    if (reviewN) box.appendChild(el("div", "muted", t("bin_note")));
    notes.forEach((n) => box.appendChild(el("div", "rs", "• " + n)));
    $("planStatus").textContent = ""; $("planCard").hidden = true; plan = null;
    loadDrives();
  } catch (e) { $("planStatus").textContent = t("error") + ": " + errText(e); }
  finally { setBusy(false); }
};

// ================================================================ CHAT
function addMsg(cls, text) { const m = el("div", "msg " + cls, text); $("chat").appendChild(m); $("chat").scrollTop = 1e9; return m; }
let pendingOpen = false;
function fillSuggest() {
  const box = $("suggest"); box.replaceChildren();
  ["s1", "s2", "s3", "s4", "s5"].forEach((k) => {
    const b = el("button", "chip", t(k));
    b.onclick = () => { $("chatIn").value = t(k); $("chatIn").focus(); };
    box.appendChild(b);
  });
}
function describe(p) {
  const a = p.args || {};
  switch (p.name) {
    case "email_send": return t("act_email", { to: a.to, subj: a.subject }) + "\n\n" + (a.body || "");
    case "inbox_reply": return t("act_reply") + "\n\n" + (a.text || "");
    case "whatsapp_prepare": return t("act_wa", { p: a.phone }) + "\n\n" + (a.text || "");
    case "schedule_task": return t("act_sched", { title: a.title, kind: t("k_" + a.kind), at: a.at || "" });
    case "run_connector": return t("act_conn", { n: a.name }) + (a.input ? "\n" + a.input : "");
    case "move_path": return t("act_move", { a: a.src, b: a.dst });
    case "recycle_path": return t("act_bin", { a: a.path });
    default: return p.summary;
  }
}
function openModal(p) {
  pendingOpen = true;
  const k = "tool_" + p.name;
  $("mTitle").textContent = I18N.has(k) ? t(k) : p.tool;
  $("mText").textContent = describe(p);
  $("modal").hidden = false;
}
function showTurn(turn, holder) {
  if (turn.log && turn.log.length) {
    const row = el("div");
    turn.log.forEach((l) => row.appendChild(el("span", "chip", "🔧 " + l)));
    holder.parentNode.insertBefore(row, holder);
  }
  refreshPlan();
  holder.textContent = turn.text || (turn.pending ? t("needs_ok") : "…");
  if (turn.pending) openModal(turn.pending);
}
async function answer(approve) {
  $("modal").hidden = true; pendingOpen = false;
  const w = addMsg("ai", approve ? t("working") : t("cancelled"));
  try { showTurn(await invoke("agent_confirm", { approve }), w); } catch (e) { w.textContent = t("error") + ": " + errText(e); }
}
$("mYes").onclick = () => answer(true);
$("mNo").onclick = () => answer(false);
async function send() {
  const text = $("chatIn").value.trim();
  if (!text || pendingOpen) return;
  $("chatIn").value = ""; addMsg("me", text);
  const w = addMsg("ai", t("thinking"));
  try { showTurn(await invoke("agent_send", { message: text }), w); } catch (e) { w.textContent = t("error") + ": " + errText(e); }
}
$("btnSend").onclick = send;
$("chatIn").addEventListener("keydown", (e) => { if (e.key === "Enter") send(); });
$("btnResetChat").onclick = async () => { await invoke("agent_reset"); $("chat").replaceChildren(); refreshPlan(); };
async function refreshPlan() {
  try {
    const p = await invoke("agent_plan");
    const box = $("planBox"); box.replaceChildren();
    if (!p.length) { box.hidden = true; return; }
    box.hidden = false;
    box.appendChild(el("b", "", t("agent_plan")));
    p.forEach((s, i) => box.appendChild(el("div", s.done ? "muted" : "", (s.done ? "✅ " : "⬜ ") + (i + 1) + ". " + s.text)));
  } catch (e) { /* ignore */ }
}

// ================================================================ SPACE
let ddPlan = null;
$("btnDdBrowse").onclick = async () => { const p = await dialog.open({ directory: true }); if (p) $("ddPath").value = p; };
$("btnDdScan").onclick = (ev) => withBusy(ev.currentTarget, async () => {
  const path = $("ddPath").value.trim();
  if (!path) return toast(t("path_needed"));
  $("ddResult").textContent = t("scanning"); $("btnDdApply").hidden = true;
  try {
    const r = await invoke("dedupe_scan", { path });
    ddPlan = r.plan_id;
    if (!r.files) { $("ddResult").textContent = t("dd_none"); return; }
    $("ddResult").textContent = t("dd_found", { g: r.groups, f: r.files, s: fmt(r.saved) }) + (r.examples.length ? "\n" + r.examples.join("\n") : "");
    $("ddResult").style.whiteSpace = "pre-wrap";
    $("btnDdApply").hidden = false;
  } catch (e) { $("ddResult").textContent = t("error") + ": " + errText(e); }
});
$("btnDdApply").onclick = (ev) => withBusy(ev.currentTarget, async () => {
  if (!ddPlan || !(await ask(t("dd_confirm")))) return;
  try {
    const m = await invoke("dedupe_apply", { planId: ddPlan });
    toast(t("dd_done") + " " + (m || "")); ddPlan = null; $("btnDdApply").hidden = true; $("ddResult").textContent = "";
    vaultInfo();
  } catch (e) { fail(e); }
});
async function vaultInfo() {
  try { $("vaultInfo").textContent = t("vault_size", { n: fmt(await invoke("vault_size")) }); } catch (e) { /* ignore */ }
}

// advanced clean
let findings = [];
const checked = new Set();
const verdicts = {};
function render() {
  $("resultCard").hidden = findings.length === 0;
  const list = $("list"); list.replaceChildren();
  const groups = { safe: [], review: [] };
  findings.forEach((f) => groups[f.risk === "safe" ? "safe" : "review"].push(f));
  for (const k of ["safe", "review"]) {
    if (!groups[k].length) continue;
    list.appendChild(el("div", "gh", t("grp_" + k)));
    groups[k].forEach((f) => list.appendChild(row(f)));
  }
  updateSummary();
}
function row(f) {
  const d = el("div", "item");
  const cb = document.createElement("input");
  cb.type = "checkbox"; cb.checked = checked.has(f.id);
  cb.onchange = () => { cb.checked ? checked.add(f.id) : checked.delete(f.id); updateSummary(); };
  const mid = el("div"), nm = el("div", "nm", f.name);
  nm.appendChild(el("span", "badge " + (f.risk === "safe" ? "b-safe" : "b-review"), f.category));
  const v = verdicts[f.id];
  if (v) nm.appendChild(el("span", "badge b-" + (v.verdict === "delete" ? "del" : v.verdict === "keep" ? "keep" : "ask"), "AI: " + t("v_" + v.verdict)));
  if (f.admin) nm.appendChild(el("span", "badge b-ask", t("needs_admin")));
  mid.append(nm, el("div", "rs", (v && v.note ? "🤖 " + v.note + " — " : "") + f.reason), el("div", "pt", f.path));
  d.append(cb, mid, el("div", "sz", fmt(f.size)));
  return d;
}
function updateSummary() {
  const total = findings.reduce((a, f) => a + f.size, 0);
  const sel = findings.filter((f) => checked.has(f.id)).reduce((a, f) => a + f.size, 0);
  $("summary").textContent = t("sum_line", { n: findings.length, a: fmt(total), b: fmt(sel) });
}
function setFindings(list) {
  findings = list; checked.clear();
  for (const k in verdicts) delete verdicts[k];
  list.forEach((f) => { if (f.risk === "safe") checked.add(f.id); });
  render();
}
async function scan(kind) {
  try {
    setBusy(true); $("status").textContent = t(kind === "quick" ? "scan_quick" : "scan_deep");
    const list = kind === "quick" ? await invoke("scan_system") : await invoke("scan_folder", { root: $("deepPath").value.trim() });
    setFindings(list);
    $("status").textContent = list.length ? t("scan_done") : t("scan_empty");
  } catch (e) { $("status").textContent = t("error") + ": " + errText(e); }
  finally { setBusy(false); }
}
$("btnQuick").onclick = () => scan("quick");
$("btnDeep").onclick = () => { if (!$("deepPath").value.trim()) return toast(t("path_needed")); scan("deep"); };
$("btnBrowse").onclick = async () => { const p = await dialog.open({ directory: true }); if (p) $("deepPath").value = p; };
$("btnSelSafe").onclick = () => { checked.clear(); findings.filter((f) => f.risk === "safe").forEach((f) => checked.add(f.id)); render(); };
$("btnSelNone").onclick = () => { checked.clear(); render(); };
$("btnClean").onclick = async () => {
  const ids = [...checked];
  if (!ids.length) return toast(t("nothing_selected"));
  const sel = findings.filter((f) => checked.has(f.id));
  const rev = sel.filter((f) => f.risk !== "safe").length;
  const msg = t("cl_head", { n: ids.length, s: fmt(sel.reduce((a, f) => a + f.size, 0)) }) + (rev ? "\n" + t("cf_review", { n: rev }) : "") + "\n" + t("cf_safe") + "\n" + t("cf_go");
  if (!(await ask(msg))) return;
  try {
    setBusy(true); $("status").textContent = t("cleaning");
    const rep = await invoke("clean", { ids });
    const done = new Set(rep.results.filter((r) => r.ok).map((r) => r.id));
    findings = findings.filter((f) => !done.has(f.id));
    done.forEach((i) => checked.delete(i));
    render();
    const failed = rep.results.filter((r) => !r.ok).length;
    $("status").textContent = t("freed", { n: fmt(rep.freed) }) + (failed ? " — " + t("failed_n", { n: failed }) : "");
    toast(t("freed", { n: fmt(rep.freed) }));
  } catch (e) { $("status").textContent = t("error") + ": " + errText(e); }
  finally { setBusy(false); }
};
$("btnAi").onclick = async () => {
  if (!findings.length) return;
  try {
    setBusy(true); $("status").textContent = t("ai_working");
    const vs = await invoke("agent_analyze", { ids: findings.map((f) => f.id) });
    vs.forEach((v) => {
      verdicts[v.id] = v;
      if (v.verdict === "keep") checked.delete(v.id);
      if (v.verdict === "delete") checked.add(v.id);
    });
    render(); $("status").textContent = t("ai_done", { n: vs.length });
  } catch (e) { $("status").textContent = t("error") + ": " + errText(e); }
  finally { setBusy(false); }
};

// folder overview
async function initOverview() { try { $("ovPath").value = await invoke("home_dir"); loadOverview(); } catch (e) { /* ignore */ } }
async function loadOverview() {
  const box = $("ovList"); box.textContent = t("analyzing");
  try {
    const items = await invoke("folder_overview", { path: $("ovPath").value.trim() });
    box.replaceChildren();
    const max = Math.max(1, ...items.map((i) => i.size));
    items.forEach((i) => {
      const r = el("div", "ov"), left = el("div");
      const nm = el("div", "nm", (i.is_dir ? "📁 " : "📄 ") + i.name);
      const cls = { system: "b-sys", user: "b-usr", regenerable: "b-reg", normal: "" }[i.class] || "";
      nm.appendChild(el("span", "badge " + cls, t("cls_" + (i.class || "normal"))));
      const bar = el("div", "bar"); bar.style.width = Math.max(2, (i.size / max) * 100) + "%";
      left.append(nm, bar);
      r.append(left, el("div", "muted", I18N.has("kd_" + i.dominant) ? t("kd_" + i.dominant) : ""), el("div", "muted", i.is_dir ? t("n_files", { n: i.files }) : ""), el("div", "sz", fmt(i.size)));
      if (i.is_dir) r.ondblclick = () => { $("ovPath").value = i.path; loadOverview(); };
      box.appendChild(r);
    });
    if (!items.length) box.textContent = t("empty_folder");
  } catch (e) { box.textContent = t("error") + ": " + errText(e); }
}
$("btnOv").onclick = loadOverview;
$("btnUp").onclick = () => {
  const p = $("ovPath").value.replace(/[\\/]+$/, "");
  const i = Math.max(p.lastIndexOf("\\"), p.lastIndexOf("/"));
  if (i > 2) { $("ovPath").value = p.slice(0, i); loadOverview(); }
};

// ================================================================ TASKS (real scheduling + sense of time)
function dur(min) {
  if (min == null) return "";
  if (min <= 0) return t("now_due");
  const d = Math.floor(min / 1440), h = Math.floor((min % 1440) / 60), m = min % 60;
  const parts = [];
  if (d) parts.push(d + t("u_d"));
  if (h) parts.push(h + t("u_h"));
  if (m && !d) parts.push(m + t("u_m"));
  return parts.join(" ");
}
function tickNow() {
  const n = new Date(), L = locale();
  const box = $("nowBox"); if (!box) return;
  box.replaceChildren(
    document.createTextNode(n.toLocaleTimeString(L, { hour: "2-digit", minute: "2-digit", second: "2-digit" })),
    el("small", "", n.toLocaleDateString(L, { weekday: "long", year: "numeric", month: "long", day: "numeric" }) + " — " + Intl.DateTimeFormat().resolvedOptions().timeZone)
  );
}
setInterval(tickNow, 1000);
function resultText(r) {
  if (!r) return "";
  const i = r.indexOf("|"), code = i < 0 ? r : r.slice(0, i), det = i < 0 ? "" : r.slice(i + 1);
  const k = "res_" + code;
  if (code === "error") return t("res_error", { d: det.startsWith("ERR_") ? errText(det) : det });
  return I18N.has(k) ? t(k, { d: det }) : r;
}
async function loadTasks() {
  const box = $("taskList"); box.replaceChildren();
  try {
    const list = await invoke("tasks_list");
    if (!list.length) { box.appendChild(el("div", "card muted", t("no_tasks"))); return; }
    list.forEach((x) => {
      const c = el("div", "card"), info = el("div");
      info.appendChild(el("div", "nm", x.title + (x.enabled ? "" : " (" + t("finished") + ")")));
      const rep = x.repeat_hours ? t("every_h", { h: x.repeat_hours }) : t("once");
      info.appendChild(el("div", "rs", t("k_" + x.kind) + " • " + rep + (x.param ? " • " + x.param : "")));
      if (x.enabled && x.next_in_min != null) {
        const cd = el("div", "rs"); cd.append(t("next_in") + " ", el("span", "cd", dur(x.next_in_min)), document.createTextNode(" — " + (x.next_at || "")));
        info.appendChild(cd);
      }
      if (x.last_result) info.appendChild(el("div", "rs", t("last_result") + ": " + resultText(x.last_result) + (x.last_run ? " (" + x.last_run + ")" : "")));
      const row = el("div", "row");
      const run = el("button", "", t("run_now"));
      run.onclick = () => withBusy(run, async () => { try { toast(resultText(await invoke("task_run_now", { id: x.id })), 7000); } catch (e) { fail(e); } loadTasks(); });
      const del = el("button", "danger", t("delete"));
      del.onclick = async () => { try { await invoke("task_remove", { id: x.id }); } catch (e) { fail(e); } loadTasks(); };
      row.append(run, del);
      c.append(info, row); box.appendChild(c);
    });
  } catch (e) { box.textContent = t("error") + ": " + errText(e); }
}
$("tKind").onchange = () => { $("tParamLbl").textContent = t("param_" + $("tKind").value); $("tParamWrap").style.display = $("tKind").value === "clean_safe" ? "none" : ""; };
$("btnTaskAdd").onclick = (ev) => withBusy(ev.currentTarget, async () => {
  try {
    await invoke("task_add", { kind: $("tKind").value, title: $("tTitle").value.trim(), param: $("tParam").value.trim(), at: $("tAt").value, repeatHours: Number($("tRepeat").value) });
    $("tTitle").value = ""; $("tParam").value = ""; toast(t("task_added"));
  } catch (e) { fail(e); }
  loadTasks();
});
async function loadMemory() {
  const box = $("memList"); box.replaceChildren();
  try {
    const list = await invoke("memory_list");
    if (!list.length) { box.appendChild(el("div", "muted", t("no_memory"))); return; }
    list.forEach(([k, v]) => {
      const r = el("div", "lg"); r.appendChild(el("div", "", k + ": " + v)); r.appendChild(el("span"));
      const b = el("button", "", t("forget"));
      b.onclick = async () => { try { await invoke("memory_forget", { key: k }); } catch (e) { fail(e); } loadMemory(); };
      r.appendChild(b); box.appendChild(r);
    });
  } catch (e) { box.textContent = t("error") + ": " + errText(e); }
}
$("btnMemRefresh").onclick = loadMemory;
async function checkUnseen() {
  try {
    const list = await invoke("tasks_unseen");
    list.forEach((x) => toast("⏰ " + x.title + ": " + resultText(x.last_result), 9000));
    const b = $("taskBadge");
    if (list.length) { b.hidden = false; b.textContent = list.length; } else if (page === "tasks") b.hidden = true;
    if (list.length && page === "tasks") loadTasks();
  } catch (e) { /* ignore */ }
}
setInterval(checkUnseen, 60000);

// ================================================================ ACCOUNTS
async function loadMail() {
  try { const a = await invoke("email_status"); $("mailStatus").textContent = a ? "✅ " + a : t("not_linked"); }
  catch (e) { $("mailStatus").textContent = errText(e); }
}
$("btnMailConnect").onclick = (ev) => withBusy(ev.currentTarget, async () => {
  try { await invoke("email_connect", { address: $("mailAddr").value.trim(), password: $("mailPw").value }); $("mailPw").value = ""; toast(t("gm_ok")); }
  catch (e) { fail(e); }
  loadMail();
});
$("btnMailOff").onclick = async () => { try { await invoke("email_disconnect"); $("mailPw").value = ""; toast(t("disconnected")); } catch (e) { fail(e); } loadMail(); };

async function loadTg() {
  try {
    const s = await invoke("tg_status");
    $("tgStatus").textContent = !s.connected ? t("not_linked") : "✅ @" + (s.bot || "bot") + " — " + (s.paired ? t("tg_paired") : t("tg_unpaired")) + (s.running ? " — " + t("tg_running") : "");
  } catch (e) { $("tgStatus").textContent = errText(e); }
}
$("btnTgConnect").onclick = (ev) => withBusy(ev.currentTarget, async () => {
  try { await invoke("tg_connect", { botToken: $("tgToken").value.trim() }); $("tgToken").value = ""; toast(t("tg_ok")); } catch (e) { fail(e); }
  loadTg();
});
$("btnTgPair").onclick = async () => {
  try {
    const code = await invoke("tg_pair_code");
    const b = $("tgPair"); b.hidden = false; b.textContent = "/pair " + code;
    toast(t("tg_pair_info"), 9000);
  } catch (e) { fail(e); }
};
$("btnTgOff").onclick = async () => { try { await invoke("tg_disconnect"); $("tgPair").hidden = true; toast(t("disconnected")); } catch (e) { fail(e); } loadTg(); };
listen("telegram-paired", () => { $("tgPair").hidden = true; toast(t("tg_paired")); loadTg(); });
listen("inbox-new", () => { const b = $("inboxBadge"); b.hidden = false; b.textContent = "!"; if (page === "accounts") loadInbox(); });

$("btnWaAdd").onclick = async () => {
  try { await invoke("inbox_add_manual", { phone: $("waPhone").value.trim(), from: $("waFrom").value.trim() || $("waPhone").value.trim(), text: $("waText").value }); $("waText").value = ""; loadInbox(); toast(t("wa_added")); }
  catch (e) { fail(e); }
};
$("btnWaOpen").onclick = async () => {
  const text = $("waText").value, phone = $("waPhone").value.trim();
  if (!(await ask(t("wa_confirm", { p: phone }) + "\n\n" + text))) return;
  try { await invoke("whatsapp_compose", { phone, text }); toast(t("wa_opened")); } catch (e) { fail(e); }
};
async function loadInbox() {
  const box = $("inboxList"); box.replaceChildren();
  $("inboxBadge").hidden = true;
  try {
    const list = await invoke("inbox_list");
    const live = list.filter((m) => m.status !== "dismissed");
    if (!live.length) { box.appendChild(el("div", "muted", t("inbox_empty"))); return; }
    live.forEach((m) => {
      const c = el("div", "msgbox");
      const h = el("div", "nm", m.from + " ");
      h.appendChild(el("span", "badge", m.channel));
      h.appendChild(el("span", "badge " + (m.status === "replied" ? "b-safe" : "b-review"), t("st_" + m.status)));
      c.append(h, el("div", "rs", m.time), el("div", "txt", m.text));
      const ta = document.createElement("textarea"); ta.rows = 3; ta.value = m.draft || ""; ta.placeholder = t("reply_ph");
      const row = el("div", "row");
      const dr = el("button", "", t("draft"));
      dr.onclick = () => withBusy(dr, async () => { try { ta.value = await invoke("inbox_draft", { id: m.id }); } catch (e) { fail(e); } });
      const sd = el("button", "primary", t("send_reply"));
      sd.onclick = async () => {
        if (!ta.value.trim()) return;
        if (!(await ask(t("reply_confirm", { to: m.from }) + "\n\n" + ta.value))) return;
        try { toast(await invoke("inbox_send", { id: m.id, text: ta.value }) || t("sent")); } catch (e) { fail(e); }
        loadInbox();
      };
      const ds = el("button", "", t("dismiss"));
      ds.onclick = async () => { try { await invoke("inbox_dismiss", { id: m.id }); } catch (e) { fail(e); } loadInbox(); };
      row.append(dr, sd, ds); c.append(ta, row); box.appendChild(c);
    });
  } catch (e) { box.textContent = t("error") + ": " + errText(e); }
}
$("btnInboxRefresh").onclick = loadInbox;
setInterval(() => { if (page === "accounts") loadTg(); }, 4000);

// ================================================================ JOURNAL
async function loadLog() {
  const box = $("logList"); box.replaceChildren();
  try {
    const list = await invoke("journal_list");
    if (!list.length) { box.appendChild(el("div", "card muted", t("log_empty"))); return; }
    list.forEach((e) => {
      const r = el("div", "card lg" + (e.undone ? " undone" : "")), left = el("div");
      left.append(
        el("div", "nm", t(I18N.has("jk_" + e.kind) ? "jk_" + e.kind : "jk_other") + ": " + e.app),
        el("div", "rs", `${fmt(e.before)} → ${fmt(e.after)} • ${t("saved")} ${fmt(Math.max(0, e.before - e.after))} • ${when(e.time)}`),
        el("div", "pt", e.path));
      const btn = el("button", "", e.undone ? t("undone") : t("undo"));
      btn.disabled = e.undone;
      btn.onclick = async () => {
        if (!(await ask(t("undo_confirm") + (e.kind === "freeze" || e.kind === "vault" ? "\n" + t("undo_space") : "")))) return;
        try { setBusy(true); await invoke("journal_undo", { id: e.id }); toast(t("undo_ok")); } catch (er) { fail(er); }
        finally { setBusy(false); loadLog(); }
      };
      r.append(left, el("span"), btn); box.appendChild(r);
    });
  } catch (e) { box.textContent = t("error") + ": " + errText(e); }
}
$("btnLogRefresh").onclick = loadLog;

// ================================================================ SETTINGS (brain, performance, data, permissions)
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
  let last = "ERR_NET";
  for (const u of m.urls) {
    try { return await invoke("agent_download_model", { url: u, fileName: m.name }); } catch (e) { last = e; }
  }
  throw last;
}
let lastStatus = null;
function refreshPill() {
  const s = lastStatus, p = $("brainPill");
  p.classList.toggle("on", !!(s && s.running));
  $("brainPillText").textContent = !s ? "…" : s.running ? t("pill_on") : s.server_installed && s.model_exists ? t("pill_idle") : t("pill_setup");
}
async function refreshBrain() {
  try {
    const s = await invoke("agent_status");
    lastStatus = s; refreshPill();
    $("dataDir").value = s.data_dir;
    $("allowCmd").checked = s.allow_commands;
    $("pCap").value = s.cpu_cap; $("pThr").value = Math.min(4, s.threads); $("pCtx").value = String(s.ctx);
    const opt = [...$("pIdle").options].reduce((b, o) => (Math.abs(o.value - s.idle_secs) < Math.abs(b.value - s.idle_secs) ? o : b));
    $("pIdle").value = opt.value; updPerfLabels();
    $("langSel").value = I18N.lang();
    $("brainStatus").textContent =
      `${t("engine")}: ${s.server_installed ? "✅" : "❌"}  |  ${t("model")}: ${s.model_path ? (s.model_exists ? "✅ " + s.model_path : "⚠ " + s.model_path) : "❌"}  |  ${s.running ? "🟢 " + t("pill_on") : "⚪ " + t("pill_idle")}`;
    return s;
  } catch (e) { $("brainStatus").textContent = errText(e); }
}
function updPerfLabels() { $("capV").textContent = $("pCap").value + "%"; $("thrV").textContent = $("pThr").value; }
$("pCap").oninput = updPerfLabels; $("pThr").oninput = updPerfLabels;
async function savePerf() {
  try {
    await invoke("agent_set_perf", { cpuCap: Number($("pCap").value), threads: Number($("pThr").value), ctx: Number($("pCtx").value), idleSecs: Number($("pIdle").value) });
    toast(t("perf_saved"));
  } catch (e) { fail(e); }
}
$("btnPerfSave").onclick = savePerf;
$("btnPerfLow").onclick = () => { $("pCap").value = 50; $("pThr").value = 2; $("pCtx").value = "8192"; $("pIdle").value = "120"; updPerfLabels(); savePerf(); };
listen("download-progress", (ev) => {
  const p = ev.payload;
  $("dlProgress").textContent = t("downloading", { l: p.label, a: fmt(p.got), b: p.total ? fmt(p.total) : "?", pct: p.total ? Math.round((p.got / p.total) * 100) + "%" : "" });
});
$("btnPickDir").onclick = async () => { const p = await dialog.open({ directory: true }); if (p) $("dataDir").value = p; };
$("btnSaveDir").onclick = async () => {
  try { await invoke("agent_set_data_dir", { path: $("dataDir").value.trim() }); toast(t("data_saved")); } catch (e) { fail(e); }
  refreshBrain();
};
async function prepareBrain(wantModel) {
  let s = await refreshBrain();
  if (!s.server_installed && !wantModel) { $("dlProgress").textContent = t("installing_engine"); await invoke("agent_install_server"); s = await refreshBrain(); }
  if (wantModel) { await downloadModel(wantModel); s = await refreshBrain(); }
  else if (!s.model_path || !s.model_exists) { await downloadModel(MODELS.q35); s = await refreshBrain(); }
  if (!s.server_installed) throw "ERR_NO_ENGINE";
  $("dlProgress").textContent = t("loading_model");
  await invoke("agent_start");
  $("dlProgress").textContent = "";
  toast(t("brain_ready"));
}
const guard = (fn) => (ev) => withBusy(ev.currentTarget, async () => {
  try { await fn(ev); } catch (e) { fail(e, 9000); $("dlProgress").textContent = ""; }
  refreshBrain();
});
$("btnAuto").onclick = guard(() => prepareBrain(null));
document.querySelectorAll("[data-dl]").forEach((b) => (b.onclick = guard(() => prepareBrain(MODELS[b.dataset.dl]))));
$("btnPickModel").onclick = guard(async () => {
  const p = await dialog.open({ filters: [{ name: "GGUF", extensions: ["gguf"] }] });
  if (!p) return;
  await invoke("agent_set_model", { path: p });
  toast(t("model_kept"));
  const s = await refreshBrain();
  if (s && s.server_installed) await prepareBrain(null);
});
async function importEngine(directory) {
  const p = await dialog.open(directory ? { directory: true } : { filters: [{ name: "ZIP", extensions: ["zip"] }] });
  if (!p) return;
  await invoke("agent_import_server", { path: p });
  toast(t("engine_imported"));
  const s = await refreshBrain();
  if (s && s.model_exists) await prepareBrain(null);
}
$("btnImportZip").onclick = guard(() => importEngine(false));
$("btnImportDir").onclick = guard(() => importEngine(true));
$("btnStop").onclick = async () => { await invoke("agent_stop"); refreshBrain(); };
$("allowCmd").onchange = async (e) => {
  const on = e.target.checked;
  if (on && !(await ask(t("cmd_confirm")))) { e.target.checked = false; return; }
  try { await invoke("agent_set_allow_commands", { allow: on }); } catch (er) { fail(er); }
};
async function loadDomains() {
  try {
    const s = await invoke("agent_status");
    document.querySelectorAll("[data-domain]").forEach((c) => (c.checked = !s.disabled_domains.includes(c.dataset.domain)));
  } catch (e) { /* ignore */ }
}
document.querySelectorAll("[data-domain]").forEach((c) => (c.onchange = async () => {
  try { await invoke("agent_set_domain", { domain: c.dataset.domain, enabled: c.checked }); } catch (e) { fail(e); }
}));

// ================================================================ start
(async function init() {
  I18N.apply();
  buildStatic();
  try { await invoke("agent_set_lang", { lang: I18N.lang() }); } catch (e) { /* ignore */ }
  $("tKind").onchange();
  go("home");
  refreshBrain();
  checkUnseen();
})();
