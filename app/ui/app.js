// CSE Trader UI. Talks to the Rust backend through one command API:
// Tauri's `call` command in the app, or POST /api/<cmd> in the dev server.
"use strict";

// ---------- backend ----------
async function call(cmd, args = {}) {
  if (window.__TAURI__) return window.__TAURI__.core.invoke("call", { cmd, args });
  const r = await fetch("/api/" + cmd, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(args),
  });
  const body = await r.json();
  if (!r.ok) throw new Error(body.error || r.statusText);
  return body;
}

// ---------- helpers ----------
const $ = (sel, root = document) => root.querySelector(sel);
const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) =>
  ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
const num = (v) => (v === null || v === undefined || v === "" ? NaN : Number(v));
function fmt(v, dp = 2) {
  const n = num(v);
  if (Number.isNaN(n)) return "–";
  return n.toLocaleString("en-US", { minimumFractionDigits: dp, maximumFractionDigits: dp });
}
const fmtInt = (v) => fmt(v, 0);
// Share counts: whole numbers, abbreviated from a million.
const fmtVol = (v) => (num(v) >= 1e6 ? fmtBig(v) : fmtInt(v));
function fmtBig(v) {
  const n = num(v);
  if (Number.isNaN(n)) return "–";
  if (Math.abs(n) >= 1e9) return fmt(n / 1e9) + " bn";
  if (Math.abs(n) >= 1e6) return fmt(n / 1e6) + " mn";
  return fmt(n);
}
const dir = (v) => (num(v) > 0 ? "up" : num(v) < 0 ? "down" : "flat");
const signed = (v, dp = 2) => (num(v) > 0 ? "+" : "") + fmt(v, dp);
const pct = (v) => signed(v) + "%";
function timeStr(iso) {
  if (!iso) return "–";
  return new Date(iso).toLocaleString("en-GB", {
    timeZone: "Asia/Colombo", day: "2-digit", month: "short",
    hour: "2-digit", minute: "2-digit",
  });
}
function statusName(s) { return typeof s === "string" ? s : Object.keys(s)[0]; }
function orderPrice(o) { return o.order_type === "Market" ? "Market" : fmt(o.order_type.Limit); }

function toast(msg, isErr = false) {
  const t = $("#toast");
  t.textContent = msg;
  t.className = "toast" + (isErr ? " err" : "");
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => t.classList.add("hidden"), 3200);
}
async function attempt(fn, okMsg) {
  try {
    const r = await fn();
    if (okMsg) toast(okMsg);
    return r;
  } catch (e) {
    toast(String(e.message || e), true);
    throw e;
  }
}
function prefGet(k, d) { try { return JSON.parse(localStorage.getItem(k)) ?? d; } catch { return d; } }
function prefSet(k, v) { try { localStorage.setItem(k, JSON.stringify(v)); } catch { /* ignore */ } }

// ---------- state ----------
const state = {
  tab: "market",
  market: null,
  quotes: [],
  topTab: prefGet("topTab", "gainers"),
  watch: { filter: "", sort: prefGet("watchSort", "look"), list: prefGet("watchList", "all"), mark: "all" },
  ordersTab: "open",
  portfolioTab: prefGet("portfolioTab", "paper"),
  ticket: { side: "Buy", symbol: "", type: "Limit", qty: "", price: "", tif: "Day", days: 2 },
};

// ---------- sheet (modal) ----------
function openSheet(html, onMount) {
  $("#sheet-body").innerHTML = html;
  $("#sheet").classList.remove("hidden");
  $$("[data-close]", $("#sheet")).forEach((b) => (b.onclick = closeSheet));
  if (onMount) onMount($("#sheet-body"));
}
function closeSheet() { $("#sheet").classList.add("hidden"); $("#sheet-body").innerHTML = ""; }
$("#sheet").addEventListener("click", (e) => { if (e.target.id === "sheet") closeSheet(); });

// ---------- header status ----------
function renderStatus() {
  const m = state.market;
  const el = $("#status");
  el.className = "status " + (m ? m.phase.toLowerCase() : "");
  $("#status-text").textContent = m ? (m.status || m.phase) : "Connecting…";
}

// ---------- Market ----------
function moverRows(list, kind) {
  if (!list || !list.length) return `<div class="empty">No data yet</div>`;
  if (kind === "active") {
    return list.map((r) => `
      <div class="row" data-symbol="${esc(r.symbol)}">
        <div><div class="sym">${esc(r.symbol)}</div><div class="sub">${fmtInt(r.trades)} trades</div></div>
        <div class="right num">${fmtBig(r.turnover)}</div>
        <div class="right num muted small">${fmtInt(r.share_volume)}</div>
      </div>`).join("");
  }
  return list.map((r) => `
    <div class="row" data-symbol="${esc(r.symbol)}">
      <div class="sym">${esc(r.symbol)}</div>
      <div class="right num">${fmt(r.price)}</div>
      <div class="right"><span class="pill ${dir(r.change)} num">${pct(r.change_pct)}</span></div>
    </div>`).join("");
}

function indexCard(name, idx) {
  if (!idx) return `<div class="card index-card"><div class="name">${name}</div><div class="value">–</div></div>`;
  return `<div class="card index-card">
    <div class="name">${name}</div>
    <div class="value num">${fmt(idx.value)}</div>
    <div class="num ${dir(idx.change)}">${signed(idx.change)} (${pct(idx.change_pct)})</div>
  </div>`;
}

function renderMarket() {
  const m = state.market;
  if (!m) return `<div class="empty">Loading market data…</div>`;
  const b = m.breadth;
  const total = b.positive + b.negative + b.neutral || 1;
  const tabs = { gainers: "Gainers", losers: "Losers", turnover: "Turnover", volume: "Volume", active: "Most active" };
  const lists = {
    gainers: m.gainers, losers: m.losers, turnover: m.top_turnover,
    volume: m.top_volume, active: m.most_active,
  };
  return `
    ${m.errors.length ? `<div class="banner">Some data could not be loaded: ${esc(m.errors.join("; "))}</div>` : ""}
    <div class="cards">${indexCard("ASPI", m.aspi)}${indexCard("S&amp;P SL20", m.snp_sl20)}</div>
    <div class="card" style="margin-top:10px">
      <div class="stats">
        <div class="stat"><div class="label">Turnover</div><div class="value num">${fmtBig(m.summary?.turnover)}</div></div>
        <div class="stat"><div class="label">Volume</div><div class="value num">${fmtBig(m.summary?.share_volume)}</div></div>
        <div class="stat"><div class="label">Trades</div><div class="value num">${fmtInt(m.summary?.trades)}</div></div>
      </div>
      <div class="breadth">
        <span style="width:${(b.positive / total) * 100}%;background:var(--up)"></span>
        <span style="width:${(b.neutral / total) * 100}%;background:var(--flat)"></span>
        <span style="width:${(b.negative / total) * 100}%;background:var(--down)"></span>
      </div>
      <div class="legend"><span class="up">▲ ${b.positive}</span><span class="flat">● ${b.neutral}</span><span class="down">▼ ${b.negative}</span></div>
    </div>
    <h2>ASPI chart</h2>
    <div class="card chart-box" id="aspi-chart"></div>
    <h2>Top stocks</h2>
    <div class="seg" id="top-tabs">${Object.entries(tabs).map(([k, v]) =>
      `<button data-k="${k}" class="${state.topTab === k ? "active" : ""}">${v}</button>`).join("")}</div>
    <div class="list" style="margin-top:10px">${moverRows(lists[state.topTab], state.topTab)}</div>
    <p class="muted small">Updated ${timeStr(m.updated_at)} · ${m.securities} securities</p>`;
}
function mountMarket(root) {
  const aspi = $("#aspi-chart", root);
  if (aspi) mountChart(aspi, null);
  $$("#top-tabs button", root).forEach((b) => (b.onclick = () => {
    state.topTab = b.dataset.k; prefSet("topTab", state.topTab); render();
  }));
}

// ---------- Watch ----------
let watchLists = [];
// Market-structure marker per favourite: [icon, text, class].
const MARK = {
  look_now: ["●", "Look now", "mk-look"],
  get_ready: ["◐", "Get ready", "mk-ready"],
  uptrend: ["↗", "Uptrend", "mk-up"],
  range: ["↔", "Range", "mk-range"],
  downtrend: ["↘", "Downtrend", "mk-down"],
  thin: ["·", "Thinly traded", "mk-thin"],
  pending: ["…", "Checking", "mk-thin"],
  no_data: ["–", "No history yet", "mk-thin"],
};
const MARK_ORDER = Object.keys(MARK);
let markers = { markers: {}, counts: {} };
const markOf = (sym) => markers.markers[sym];
function markBadge(m) {
  if (!m) return "";
  let [icon, text, cls] = MARK[m.status] || MARK.pending;
  const full = m.status === "look_now" || m.status === "get_ready";
  // Thin stocks get no alerts but still show their trend.
  if (m.status === "thin") icon = ({ Up: "↗", Down: "↘", Range: "↔" }[m.trend] || "") + " thin";
  return `<span class="mk ${cls}" title="${text}">${icon}${full ? " " + text : ""}${m.star ? " ⭐" : ""}</span>`;
}
function sortQuotes(list) { return sortQuotesBy(list, state.watch.sort); }
function sortQuotesBy(list, by) {
  const rank = (q) => { const m = markOf(q.symbol); return m ? MARK_ORDER.indexOf(m.status) : MARK_ORDER.length; };
  const key = {
    look: (q) => rank(q),
    symbol: (q) => q.symbol,
    change: (q) => -num(q.change_pct),
    turnover: (q) => -num(q.turnover),
    trades: (q) => -num(q.trades),
  }[by];
  return [...list].sort((a, b) => {
    const x = key(a), y = key(b);
    return (typeof x === "string" ? x.localeCompare(y) : x - y) || a.symbol.localeCompare(b.symbol);
  });
}
const MARK_FILTERS = {
  all: ["All", () => true],
  look_now: ["Look now", (m) => m && m.status === "look_now"],
  get_ready: ["Get ready", (m) => m && m.status === "get_ready"],
  uptrend: ["Uptrend", (m) => m && m.trend === "Up"],
};
const BG_TEXT = { off: "Background checks off", 60: "Checks every hour", 30: "Checks every 30 min", 15: "Checks every 15 min", live: "Live background checks" };
async function renderWatch() {
  try { markers = await call("favourite_markers"); } catch { /* keep the last markers */ }
  const f = state.watch.filter.trim().toUpperCase();
  let list = state.quotes;
  if (state.watch.list !== "all") {
    const wl = watchLists.find((w) => String(w.id) === String(state.watch.list));
    const set = new Set(wl ? wl.symbols : []);
    list = list.filter((q) => set.has(q.symbol));
  }
  const inList = state.watch.list !== "all";
  let summary = "";
  if (inList) {
    const ms = list.map((q) => markOf(q.symbol));
    const n = (k) => ms.filter(MARK_FILTERS[k][1]).length;
    summary = `
    <div class="chips mk-chips">${Object.entries(MARK_FILTERS).map(([k, [label]]) =>
      `<button class="chip ${state.watch.mark === k ? "on" : ""}" data-mark="${k}">${label}${k === "all" ? "" : ` ${n(k)}`}</button>`).join("")}</div>
    <div class="small muted mk-summary">${list.length} stocks · <span class="mk-look">${n("look_now")} look now</span> ·
      <span class="mk-ready">${n("get_ready")} get ready</span> · ${n("uptrend")} uptrend
      ${markers.updated_at ? ` · prices ${timeStr(markers.updated_at)}` : ""} · ${BG_TEXT[markers.background_mode] || ""}</div>`;
    list = list.filter((q) => MARK_FILTERS[state.watch.mark][1](markOf(q.symbol)));
  }
  if (f) list = list.filter((q) => q.symbol.includes(f) || q.name.toUpperCase().includes(f));
  list = sortQuotes(list);
  const opts = [`<option value="all">Full watch</option>`]
    .concat(watchLists.map((w) => `<option value="${w.id}" ${String(w.id) === String(state.watch.list) ? "selected" : ""}>★ ${esc(w.name)}</option>`))
    .join("");
  return `
    <div class="toolbar">
      <select id="wl-select">${opts}</select>
      <button class="btn ghost small" id="wl-new">+ List</button>
      <button class="btn ghost small" id="wl-import">Import</button>
    </div>
    <div class="toolbar">
      <input id="watch-search" placeholder="Search symbol or company" value="${esc(state.watch.filter)}" autocomplete="off">
      <select id="watch-sort" style="max-width:130px">
        ${["look", "symbol", "change", "turnover", "trades"].map((s) =>
          `<option value="${s}" ${state.watch.sort === s ? "selected" : ""}>${{ look: "Look first", symbol: "Symbol", change: "Change %", turnover: "Turnover", trades: "Trades" }[s]}</option>`).join("")}
      </select>
    </div>${summary}
    <div class="list">${list.length ? list.slice(0, 400).map((q) => {
      const m = markOf(q.symbol);
      const why = m && m.label ? `<div class="sub ${MARK[m.status][2]}">${esc(m.label)}${m.detail ? " · " + esc(m.detail) : ""}</div>` : "";
      return `
      <div class="row" data-symbol="${esc(q.symbol)}">
        <div><div class="sym">${esc(q.symbol)} ${markBadge(m)}</div><div class="sub">${esc(q.name)}</div>${why}</div>
        <div class="right"><div class="num">${fmt(q.last)}</div><div class="small muted num">${fmtInt(q.share_volume)}</div></div>
        <div class="right"><span class="pill ${dir(q.change)} num">${pct(q.change_pct)}</span></div>
      </div>`;
    }).join("") : `<div class="empty">${!inList ? "No securities match"
      : state.watch.mark !== "all" ? "No stocks with this marker right now."
      : "This list is empty. Open a stock and tap ☆ to add it."}</div>`}
    </div>`;
}
function mountWatch(root) {
  const search = $("#watch-search", root);
  search.oninput = () => {
    state.watch.filter = search.value;
    const pos = search.selectionStart;
    render();
    const s = $("#watch-search"); s.focus(); s.setSelectionRange(pos, pos);
  };
  $("#watch-sort", root).onchange = (e) => { state.watch.sort = e.target.value; prefSet("watchSort", state.watch.sort); render(); };
  $("#wl-select", root).onchange = (e) => { state.watch.list = e.target.value; prefSet("watchList", state.watch.list); render(); };
  $("#wl-import", root).onclick = () => openImport();
  $$("[data-mark]", root).forEach((b) => (b.onclick = () => { state.watch.mark = b.dataset.mark; render(); }));
  $("#wl-new", root).onclick = async () => {
    const name = prompt("Watch list name");
    if (!name) return;
    const r = await attempt(() => call("create_watch_list", { name }), "Watch list created");
    state.watch.list = String(r.id); prefSet("watchList", state.watch.list);
    await loadWatchLists(); render();
  };
}
function openImport() {
  const current = watchLists.find((w) => String(w.id) === String(state.watch.list));
  openSheet(`
    <div class="sheet-head"><h3>Import watch list</h3><button class="icon-btn" data-close>✕</button></div>
    <label>List name</label>
    <input id="im-name" value="${esc(current ? current.name : "My list")}" list="im-lists">
    <datalist id="im-lists">${watchLists.map((w) => `<option value="${esc(w.name)}">`).join("")}</datalist>
    <label>Company codes</label>
    <textarea id="im-text" placeholder="ABAN, AEL, ACL&#10;or ABAN.N, JKH.N0000 …"></textarea>
    <p class="muted small">Separate codes with commas, spaces or new lines. Codes without a class (ABAN) are taken as the ordinary share (ABAN.N0000).</p>
    <label class="switch-row"><span>Replace the list's stocks<br><span class="small muted">Off: add to the existing stocks</span></span>
      <input type="checkbox" id="im-replace" checked></label>
    <div class="btn-row"><button class="btn primary" id="im-go">Import</button></div>
    <div id="im-result"></div>`, (s) => {
    $("#im-go", s).onclick = async () => {
      const btn = $("#im-go", s);
      btn.disabled = true; btn.textContent = "Checking codes…";
      try {
        const r = await call("import_watch_list", {
          name: $("#im-name", s).value, text: $("#im-text", s).value, replace: $("#im-replace", s).checked,
        });
        const list = (label, xs, cls = "") => xs.length
          ? `<div class="small"><b class="${cls}">${label} (${xs.length}):</b> ${xs.map((x) => esc(x.replace(".N0000", ""))).join(", ")}</div>` : "";
        $("#im-result", s).innerHTML = `<div class="card" style="margin-top:12px">
          <div><b>${esc(r.name)}</b>: ${r.count} stocks</div>
          ${list("Newly in", r.added, "up")}${list("Newly out", r.removed, "down")}${list("Not found on the CSE", r.invalid, "down")}
          ${!r.added.length && !r.removed.length ? `<div class="small muted">No changes.</div>` : ""}</div>`;
        state.watch.list = String(r.id); prefSet("watchList", state.watch.list);
        await loadWatchLists();
        render();
      } catch (e) {
        toast(String(e.message || e), true);
      } finally {
        btn.disabled = false; btn.textContent = "Import";
      }
    };
  });
}

async function loadWatchLists() { watchLists = await call("watch_lists"); }

// ---------- Quote sheet ----------
// ---------- charts ----------
const RANGES = ["1D", "1W", "1M", "3M", "1Y", "All"];
const chartCache = new Map(); // "key|range" -> { at, data }
// Swing points (HH/HL/LH/LL) drawn over the chart as positioned dots.
function chartSvg(points, swings = []) {
  const w = 600, h = 170, pad = 4;
  const ys = points.map((p) => num(p.price));
  const xs = points.map((p) => new Date(p.at).getTime());
  let minY = Math.min(...ys), maxY = Math.max(...ys);
  // Keep at least a 3% vertical span so one-tick moves don't fill the chart.
  const mid = (minY + maxY) / 2, minSpan = Math.abs(mid) * 0.03;
  if (maxY - minY < minSpan) { minY = mid - minSpan / 2; maxY = mid + minSpan / 2; }
  const minX = xs[0], maxX = xs[xs.length - 1];
  const sx = (x) => pad + ((x - minX) / (maxX - minX || 1)) * (w - 2 * pad);
  const sy = (y) => h - pad - ((y - minY) / (maxY - minY || 1)) * (h - 2 * pad);
  const line = points.map((p, i) => `${i ? "L" : "M"}${sx(xs[i]).toFixed(1)},${sy(ys[i]).toFixed(1)}`).join("");
  const area = `${line}L${sx(maxX).toFixed(1)},${h}L${sx(minX).toFixed(1)},${h}Z`;
  const up = ys[ys.length - 1] >= ys[0];
  const color = up ? "var(--up)" : "var(--down)";
  const gid = "g" + Math.random().toString(36).slice(2, 8);
  const dots = swings
    .filter((sw) => sw.time * 1000 >= minX && sw.time * 1000 <= maxX)
    .map((sw) => {
      const y = Math.min(Math.max(sy(sw.price), 0), h);
      const lbl = sw.label || (sw.kind === "High" ? "H" : "L");
      const cls = sw.label === "HH" || sw.label === "HL" ? "up" : sw.label ? "down" : "flat";
      return `<span class="swing ${cls} ${sw.kind === "High" ? "hi" : "lo"}" style="left:${(sx(sw.time * 1000) / w * 100).toFixed(2)}%;top:${(y / h * 100).toFixed(2)}%"><i></i>${lbl}</span>`;
    }).join("");
  return `<div class="chart-wrap"><svg class="chart" viewBox="0 0 ${w} ${h}" preserveAspectRatio="none" role="img" aria-label="Price chart">
    <defs><linearGradient id="${gid}" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="${up ? "#22c55e" : "#ef4444"}" stop-opacity=".28"/>
      <stop offset="1" stop-color="${up ? "#22c55e" : "#ef4444"}" stop-opacity="0"/></linearGradient></defs>
    <path d="${area}" fill="url(#${gid})" stroke="none"/>
    <path class="line" d="${line}" stroke="${color}" vector-effect="non-scaling-stroke"/></svg>${dots}</div>`;
}
function chartLabel(iso, range) {
  const d = new Date(iso);
  const opts = range === "1D"
    ? { hour: "2-digit", minute: "2-digit" }
    : range === "1W" || range === "1M" ? { day: "2-digit", month: "short" }
    : { month: "short", year: "numeric" };
  return d.toLocaleString("en-GB", { timeZone: "Asia/Colombo", ...opts });
}
function chartBody(data, swings) {
  const pts = data.points || [];
  const note = data.error ? `<div class="small muted">CSE history unavailable right now — showing saved data.</div>` : "";
  const saved = data.history_days
    ? `<div class="small muted">History saved on this phone: ${fmtInt(data.history_days)} days since ${esc(data.history_since)}</div>` : "";
  if (pts.length < 2) {
    return `<div class="empty small">${data.range === "1D" ? "No trades yet today." : "No price history yet."}</div>${note}${saved}`;
  }
  const ys = pts.map((p) => num(p.price));
  return `${chartSvg(pts, data.range === "1D" ? [] : swings)}
    <div class="legend small muted"><span>${chartLabel(pts[0].at, data.range)}</span>
      <span class="num ${dir(data.change)}">${signed(data.change)} (${pct(data.change_pct)})</span>
      <span>${chartLabel(pts[pts.length - 1].at, data.range)}</span></div>
    <div class="legend small muted"><span>Low ${fmt(Math.min(...ys))}</span><span>${fmtInt(pts.length)} points</span><span>High ${fmt(Math.max(...ys))}</span></div>
    ${note}${data.range === "All" ? saved : ""}`;
}
// Renders a chart with range buttons into `el`. symbol = null for ASPI.
async function mountChart(el, symbol, swings = []) {
  const key = symbol || "ASPI";
  let range = prefGet("chartRange", "1M");
  const draw = async () => {
    el.innerHTML = `<div class="seg chart-ranges">${RANGES.map((r) =>
      `<button data-r="${r}" class="${r === range ? "active" : ""}">${r}</button>`).join("")}</div>
      <div class="chart-area"></div>`;
    $$(".chart-ranges button", el).forEach((b) => (b.onclick = () => {
      range = b.dataset.r; prefSet("chartRange", range); draw();
    }));
    const area = $(".chart-area", el);
    const cacheKey = key + "|" + range;
    const ttl = range === "1D" ? 60e3 : 10 * 60e3;
    const hit = chartCache.get(cacheKey);
    if (hit && Date.now() - hit.at < ttl) { area.innerHTML = chartBody(hit.data, swings); return; }
    area.innerHTML = `<div class="empty small">Loading chart…</div>`;
    try {
      const data = await call("chart", symbol ? { symbol, range } : { range });
      chartCache.set(cacheKey, { at: Date.now(), data });
      if (el.isConnected) area.innerHTML = chartBody(data, swings);
    } catch (e) {
      area.innerHTML = `<div class="empty small">${esc(e.message || e)}</div>`;
    }
  };
  await draw();
}
async function openQuote(symbol) {
  const data = await attempt(() => call("quote", { symbol }));
  const q = data.quote;
  if (!q) { toast("No data for " + symbol, true); return; }
  await loadWatchLists();
  const inList = watchLists.some((w) => w.symbols.includes(symbol));
  let fav = null, alertsView = null;
  if (inList) {
    try { alertsView = await call("alerts"); fav = alertsView.favourites.find((f) => f.symbol === symbol); } catch { /* optional */ }
  }
  const field = (k, v) => `<div class="k">${k}</div><div class="v">${v}</div>`;
  openSheet(`
    <div class="sheet-head"><h3>${esc(q.symbol)}</h3>
      <div><button class="icon-btn" id="q-fav" title="Add to watch list">${inList ? "★" : "☆"}</button>
      <button class="icon-btn" data-close aria-label="Close">✕</button></div></div>
    <div class="muted small">${esc(q.name)}</div>
    <div class="quote-head" style="margin-top:8px">
      <div class="last num">${fmt(q.last)}</div>
      <div class="right"><span class="pill ${dir(q.change)} num">${signed(q.change)} (${pct(q.change_pct)})</span></div>
    </div>
    <div class="chart-box" id="q-chart"></div>
    ${fav ? structureCard(fav, alertsView) : ""}
    <div class="card" style="margin-top:12px"><div class="kv">
      ${field("Open", fmt(q.open))}${field("High", fmt(q.high))}${field("Low", fmt(q.low))}
      ${field("Prev. close", fmt(q.prev_close))}${field("Volume", fmtInt(q.share_volume))}
      ${field("Trades", fmtInt(q.trades))}${field("Turnover", fmt(q.turnover))}
      ${field("Last trade", timeStr(q.last_traded_at))}
    </div></div>
    <div class="card" style="margin-top:10px"><div class="kv">
      ${field("Paper holding", fmtInt(data.held))}${field("Available to sell", fmtInt(data.available_to_sell))}
      ${field("Pending buy / sell", fmtInt(data.pending_buy) + " / " + fmtInt(data.pending_sell))}
    </div></div>
    <div class="btn-row">
      <button class="btn buy" id="q-buy">Buy</button>
      <button class="btn sell" id="q-sell">Sell</button>
    </div>
    <div class="btn-row"><button class="btn ghost" id="q-tv">📈 TradingView chart &amp; tools</button></div>`, (root) => {
    mountChart($("#q-chart", root), symbol, (fav && fav.structure && fav.structure.swings) || []);
    $("#q-buy", root).onclick = () => { closeSheet(); goTicket(symbol, "Buy", q.last); };
    $("#q-sell", root).onclick = () => { closeSheet(); goTicket(symbol, "Sell", q.last); };
    $("#q-tv", root).onclick = () => { closeSheet(); openTradingView(symbol); };
    $("#q-fav", root).onclick = () => pickWatchList(symbol);
  });
}
async function pickWatchList(symbol) {
  if (!watchLists.length) {
    const name = prompt("Create a watch list", "Favourites");
    if (!name) return;
    await attempt(() => call("create_watch_list", { name }));
    await loadWatchLists();
  }
  const choices = watchLists.map((w, i) => `${i + 1}. ${w.name}${w.symbols.includes(symbol) ? " (remove)" : ""}`).join("\n");
  const pick = prompt(`Add ${symbol} to which list?\n${choices}`, "1");
  const wl = watchLists[Number(pick) - 1];
  if (!wl) return;
  const cmd = wl.symbols.includes(symbol) ? "remove_from_watch_list" : "add_to_watch_list";
  await attempt(() => call(cmd, { id: wl.id, symbol }), cmd === "add_to_watch_list" ? `Added to ${wl.name}` : `Removed from ${wl.name}`);
  await loadWatchLists();
  $("#q-fav").textContent = watchLists.some((w) => w.symbols.includes(symbol)) ? "★" : "☆";
}

// ---------- TradingView ----------
// CSE symbols on TradingView are prefixed with the exchange code CSELK.
const tvSymbol = (symbol) => "CSELK:" + symbol;
function openExternal(url) {
  if (window.__TAURI__) return window.__TAURI__.core.invoke("plugin:opener|open_url", { url });
  window.open(url, "_blank", "noopener");
}
function openTradingView(symbol) {
  const overlay = $("#tv");
  $("#tv-title").textContent = symbol;
  $("#tv-open").onclick = () =>
    attempt(() => openExternal("https://www.tradingview.com/chart/?symbol=" + encodeURIComponent(tvSymbol(symbol))));
  const host = $("#tv-host");
  host.innerHTML = `<div class="tradingview-widget-container" style="height:100%;width:100%">
    <div class="tradingview-widget-container__widget" style="height:100%;width:100%"></div></div>`;
  const script = document.createElement("script");
  script.src = "https://s3.tradingview.com/external-embedding/embed-widget-advanced-chart.js";
  script.async = true;
  script.textContent = JSON.stringify({
    autosize: true,
    symbol: tvSymbol(symbol),
    interval: "D",
    timezone: "Asia/Colombo",
    theme: "dark",
    style: "1",
    locale: "en",
    allow_symbol_change: true,
    hide_side_toolbar: false,
    withdateranges: true,
    save_image: false,
    calendar: false,
    support_host: "https://www.tradingview.com",
  });
  script.onerror = () => {
    host.innerHTML = `<div class="empty">Couldn't load TradingView. Check your internet connection, or use “Open in TradingView”.</div>`;
  };
  host.firstElementChild.appendChild(script);
  overlay.classList.remove("hidden");
}
$("#tv-close").onclick = () => { $("#tv").classList.add("hidden"); $("#tv-host").innerHTML = ""; };

// ---------- Buy / Sell ----------
function goTicket(symbol, side, price) {
  Object.assign(state.ticket, { symbol, side, price: price ? String(num(price)) : state.ticket.price });
  switchTab("trade");
}
function renderTrade() {
  const t = state.ticket;
  return `
    <div class="side-toggle">
      <button class="buy ${t.side === "Buy" ? "active" : ""}" data-side="Buy">BUY</button>
      <button class="sell ${t.side === "Sell" ? "active" : ""}" data-side="Sell">SELL</button>
    </div>
    <label for="t-symbol">Security</label>
    <input id="t-symbol" list="symbols" placeholder="e.g. JKH.N0000" value="${esc(t.symbol)}" autocomplete="off" autocapitalize="characters">
    <datalist id="symbols">${state.quotes.map((q) => `<option value="${esc(q.symbol)}">${esc(q.name)}</option>`).join("")}</datalist>
    <div class="card" id="t-quote" style="margin-top:10px"></div>
    <div class="grid2">
      <div><label>Order type</label>
        <select id="t-type"><option ${t.type === "Limit" ? "selected" : ""}>Limit</option><option ${t.type === "Market" ? "selected" : ""}>Market</option></select></div>
      <div><label>Time in force</label>
        <select id="t-tif"><option value="Day" ${t.tif === "Day" ? "selected" : ""}>Day</option><option value="Days" ${t.tif === "Days" ? "selected" : ""}>Days…</option></select></div>
    </div>
    <div id="t-days-wrap" class="${t.tif === "Days" ? "" : "hidden"}"><label for="t-days">Number of trading days</label>
      <input id="t-days" type="number" inputmode="numeric" min="1" max="30" value="${t.days}"></div>
    <label for="t-qty">Quantity</label>
    <input id="t-qty" type="number" inputmode="numeric" min="1" step="1" value="${esc(t.qty)}" placeholder="0">
    <div id="t-price-wrap" class="${t.type === "Market" ? "hidden" : ""}"><label for="t-price">Price</label>
      <div class="stepper"><button id="t-minus" aria-label="Lower price">−</button>
      <input id="t-price" type="number" inputmode="decimal" step="0.05" value="${esc(t.price)}" placeholder="0.00">
      <button id="t-plus" aria-label="Raise price">+</button></div></div>
    <div class="card" style="margin-top:14px"><div class="kv" id="t-est"></div></div>
    <div class="error" id="t-err"></div>
    <div class="btn-row">
      <button class="btn ghost" id="t-clear">Clear</button>
      <button class="btn ${t.side === "Buy" ? "buy" : "sell"}" id="t-submit">${t.side === "Buy" ? "Buy" : "Sell"}</button>
    </div>`;
}
let estimateSeq = 0;
async function updateEstimate() {
  const t = state.ticket;
  const quote = state.quotes.find((q) => q.symbol === t.symbol);
  $("#t-quote").innerHTML = quote
    ? `<div class="quote-head"><div><div class="sym">${esc(quote.symbol)}</div><div class="small muted">${esc(quote.name)}</div></div>
       <div class="right"><div class="num" style="font-size:20px;font-weight:700">${fmt(quote.last)}</div>
       <div class="num small ${dir(quote.change)}">${signed(quote.change)} (${pct(quote.change_pct)})</div></div></div>
       <div class="legend small muted" style="margin-top:6px"><span>H ${fmt(quote.high)}</span><span>L ${fmt(quote.low)}</span><span>Vol ${fmtInt(quote.share_volume)}</span></div>`
    : `<div class="muted small">${t.symbol ? "Unknown symbol" : "Choose a security"}</div>`;
  const qty = parseInt(t.qty, 10) || 0;
  const seq = ++estimateSeq;
  if (!t.symbol) { $("#t-est").innerHTML = ""; return; }
  try {
    const e = await call("estimate", {
      symbol: t.symbol, side: t.side, qty,
      price: t.type === "Market" || !t.price ? null : String(t.price),
    });
    if (seq !== estimateSeq) return;
    const row = (k, v, cls = "") => `<div class="k ${cls}">${k}</div><div class="v ${cls}">${v}</div>`;
    $("#t-est").innerHTML =
      (t.side === "Buy" ? row("Buying power", fmt(e.buying_power)) : row("Available shares", fmtInt(e.available_to_sell))) +
      row("Pending buy / sell qty", fmtInt(e.pending_buy) + " / " + fmtInt(e.pending_sell)) +
      row("Order value", fmt(e.order_value)) +
      row(`Commission (${fmt(e.fee_pct)}%)`, fmt(e.fees)) +
      row("Net value", fmt(e.net), "total");
    const err = [];
    if (t.type === "Limit" && t.price && !e.price_valid) err.push(`Price should be a multiple of ${fmt(e.tick)}`);
    if (qty > 0 && t.side === "Buy" && num(e.net) > num(e.buying_power)) err.push("Order value exceeds buying power");
    if (qty > 0 && t.side === "Sell" && qty > e.available_to_sell) err.push("Not enough shares available");
    $("#t-err").textContent = err.join(" · ");
    state.ticket.tick = e.tick;
  } catch (e) {
    $("#t-est").innerHTML = "";
    $("#t-err").textContent = e.message;
  }
}
function mountTrade(root) {
  const t = state.ticket;
  $$(".side-toggle button", root).forEach((b) => (b.onclick = () => { t.side = b.dataset.side; render(); }));
  const bind = (id, key, rerender = false) => {
    const el = $(id, root);
    el.oninput = el.onchange = () => {
      t[key] = key === "symbol" ? el.value.trim().toUpperCase() : el.value;
      if (rerender) render(); else updateEstimate();
    };
  };
  bind("#t-symbol", "symbol"); bind("#t-qty", "qty"); bind("#t-price", "price");
  bind("#t-days", "days"); bind("#t-type", "type", true); bind("#t-tif", "tif", true);
  const step = (sgn) => {
    const tick = num(t.tick) || 0.1;
    const p = Math.max(tick, (num(t.price) || 0) + sgn * tick);
    t.price = (Math.round(p / tick) * tick).toFixed(2);
    $("#t-price", root).value = t.price; updateEstimate();
  };
  $("#t-minus", root).onclick = () => step(-1);
  $("#t-plus", root).onclick = () => step(1);
  $("#t-clear", root).onclick = () => { Object.assign(t, { symbol: "", qty: "", price: "" }); render(); };
  $("#t-submit", root).onclick = submitOrder;
  updateEstimate();
}
async function submitOrder() {
  const t = state.ticket;
  const qty = parseInt(t.qty, 10);
  if (!t.symbol || !qty) { toast("Enter a security and quantity", true); return; }
  const priceTxt = t.type === "Market" ? "at market" : "@ " + fmt(t.price);
  if (!confirm(`${t.side} ${fmtInt(qty)} ${t.symbol} ${priceTxt}?\nThis is a paper trade.`)) return;
  const order = {
    symbol: t.symbol, side: t.side, qty,
    order_type: t.type === "Market" ? "Market" : { Limit: String(t.price) },
    tif: t.tif === "Day" ? "Day" : { Days: Math.max(1, parseInt(t.days, 10) || 1) },
  };
  await attempt(() => call("place_order", { order }), "Order placed");
  t.qty = "";
  state.ordersTab = "open";
  switchTab("orders");
}

// ---------- Orders ----------
async function renderOrders() {
  const tabs = { open: "Open", all: "All orders", account: "Account" };
  const seg = `<div class="seg" id="order-tabs">${Object.entries(tabs).map(([k, v]) =>
    `<button data-k="${k}" class="${state.ordersTab === k ? "active" : ""}">${v}</button>`).join("")}</div>`;
  if (state.ordersTab === "account") {
    const [a, ledger] = await Promise.all([call("account"), call("ledger")]);
    const row = (k, v, cls = "") => `<div class="k ${cls}">${k}</div><div class="v ${cls}">${v}</div>`;
    return seg + `
      <div class="card" style="margin-top:10px"><div class="kv">
        ${row("Cash balance", fmt(a.cash))}${row("Buying power", fmt(a.buying_power))}
        ${row("Pending buy value", fmt(a.pending_buy_value))}
        ${row("Portfolio cost", fmt(a.portfolio.total_cost))}${row("Portfolio market value", fmt(a.portfolio.market_value))}
        ${row("Realised gain/loss", `<span class="${dir(a.portfolio.realized)}">${signed(a.portfolio.realized)}</span>`)}
        ${row("Total equity", fmt(a.equity), "total")}
        ${row("Return on " + fmt(a.starting_cash, 0), `<span class="${dir(a.total_return)}">${signed(a.total_return)} (${pct(a.total_return_pct)})</span>`)}
      </div></div>
      <h2>Cash movements</h2>
      <div class="list">${ledger.map((l) => `
        <div class="row" style="cursor:default"><div><div>${esc(l.note)}</div><div class="sub">${timeStr(l.at)}</div></div>
        <div class="right num ${dir(l.amount)}">${signed(l.amount)}</div><div class="right num muted small">${fmt(l.balance)}</div></div>`).join("")}</div>`;
  }
  let orders = await call("orders");
  if (state.ordersTab === "open") orders = orders.filter((o) => ["Open", "Queued"].includes(statusName(o.status)));
  return seg + `<div class="list" style="margin-top:10px">${orders.length ? orders.map((o) => {
    const st = statusName(o.status);
    const active = st === "Open" || st === "Queued";
    const fill = o.fill ? ` · filled @ ${fmt(o.fill.price)} · net ${fmt(o.fill.net)}` : "";
    return `<div class="order">
      <div class="head"><div><span class="side-${o.side.toLowerCase()}">${o.side.toUpperCase()}</span>
        <b>${esc(o.symbol)}</b></div><span class="badge ${st}">${st}</span></div>
      <div class="meta num">${fmtInt(o.qty)} @ ${orderPrice(o)} · ${o.tif === "Day" ? "Day" : o.tif.Days + " days"}${fill}</div>
      <div class="meta">Placed ${timeStr(o.placed_at)}${active ? " · expires " + timeStr(o.expires_at) : ""}${typeof o.status === "object" ? " · " + esc(o.status.Rejected) : ""}</div>
      <details><summary>History</summary>${o.history.map((h) =>
        `<div>${timeStr(h.at)} · ${esc(statusName(h.status))} · ${esc(h.note)}</div>`).join("")}</details>
      ${active ? `<div class="actions"><button class="btn small ghost" data-amend="${o.id}">Amend</button>
        <button class="btn small ghost" data-cancel="${o.id}">Cancel</button></div>` : ""}
    </div>`;
  }).join("") : `<div class="empty">No ${state.ordersTab === "open" ? "open " : ""}orders</div>`}</div>`;
}
function mountOrders(root) {
  $$("#order-tabs button", root).forEach((b) => (b.onclick = () => { state.ordersTab = b.dataset.k; render(); }));
  $$("[data-cancel]", root).forEach((b) => (b.onclick = async () => {
    if (!confirm("Cancel this order?")) return;
    await attempt(() => call("cancel_order", { id: Number(b.dataset.cancel) }), "Order cancelled");
    render();
  }));
  $$("[data-amend]", root).forEach((b) => (b.onclick = async () => {
    const orders = await call("orders");
    const o = orders.find((x) => x.id === Number(b.dataset.amend));
    const qty = prompt("New quantity", o.qty);
    if (qty === null) return;
    let price = null;
    if (o.order_type !== "Market") {
      price = prompt("New limit price", o.order_type.Limit);
      if (price === null) return;
    }
    await attempt(() => call("amend_order", { id: o.id, qty: parseInt(qty, 10), price: price && String(price) }), "Order amended");
    render();
  }));
}

// ---------- Portfolio ----------
function valuationHtml(v, title) {
  const row = (k, val) => `<div class="stat"><div class="label">${k}</div><div class="value num">${val}</div></div>`;
  return `
    <div class="card" style="margin-top:10px">
      <div class="muted small">${title}</div>
      <div class="value num" style="font-size:24px;font-weight:800;margin:4px 0">${fmt(v.market_value)}</div>
      <div class="num ${dir(v.unrealized)}">${signed(v.unrealized)} (${pct(v.unrealized_pct)}) unrealised</div>
      <div class="stats">
        ${row("Total cost", fmt(v.total_cost))}
        ${row("Today", `<span class="${dir(v.today)}">${signed(v.today)}</span>`)}
        ${row("Realised", `<span class="${dir(v.realized)}">${signed(v.realized)}</span>`)}
      </div>
    </div>
    <h2>Holdings</h2>
    <div class="list">${v.holdings.length ? v.holdings.map((h) => `
      <div class="holding" data-symbol="${esc(h.symbol)}">
        <div class="head"><div><b>${esc(h.symbol)}</b><span class="qty">${fmtInt(h.qty)} shares</span></div>
          <div class="pl num ${dir(h.unrealized)}">${signed(h.unrealized)}<div class="small">${pct(h.unrealized_pct)}</div></div></div>
        <div class="detail num">
          <div><span>Avg cost</span>${fmt(h.avg_cost)}</div>
          <div><span>B.E.S price</span>${fmt(h.break_even)}</div>
          <div><span>Last${h.priced ? "" : " (no quote)"}</span>${fmt(h.last)}</div>
          <div><span>Cost</span>${fmt(h.total_cost)}</div>
          <div><span>Market value</span>${fmt(h.market_value)}</div>
          <div><span>Today</span><b class="${dir(h.today)}">${signed(h.today)}</b></div>
        </div>
        <div class="weight" title="${fmt(h.weight_pct)}% of portfolio"><span style="width:${num(h.weight_pct)}%"></span></div>
      </div>`).join("") : `<div class="empty">No holdings yet</div>`}</div>`;
}
async function renderPortfolio() {
  const seg = `<div class="seg" id="pf-tabs">
    <button data-k="paper" class="${state.portfolioTab === "paper" ? "active" : ""}">Paper</button>
    <button data-k="real" class="${state.portfolioTab === "real" ? "active" : ""}">Real (manual)</button></div>`;
  if (state.portfolioTab === "paper") {
    const a = await call("account");
    return seg + valuationHtml(a.portfolio, "Paper portfolio value");
  }
  const r = await call("real_portfolio");
  return seg + valuationHtml(r.valuation, "Real portfolio value") + `
    <h2>Transactions</h2>
    <div class="btn-row wrap" style="margin-top:0">
      <button class="btn ghost small" id="rp-add">+ Add trade</button>
      <button class="btn ghost small" id="rp-import">Import CSV</button>
      <button class="btn ghost small" id="rp-export">Export CSV</button>
    </div>
    <div class="list" style="margin-top:10px">${r.transactions.length ? r.transactions.slice().reverse().map((t) => `
      <div class="row" style="cursor:default">
        <div><div><span class="side-${t.side.toLowerCase()}">${t.side.toUpperCase()}</span> <b>${esc(t.symbol)}</b></div>
          <div class="sub">${esc(t.date)}${t.fees !== null ? " · fees " + fmt(t.fees) : ""}</div></div>
        <div class="right num">${fmtInt(t.qty)} @ ${fmt(t.price)}</div>
        <button class="btn small ghost" data-del="${t.id}" aria-label="Delete">✕</button>
      </div>`).join("") : `<div class="empty">Add your ATrad holdings by hand or import a CSV<br><span class="small">date,symbol,side,qty,price,fees</span></div>`}</div>`;
}
function mountPortfolio(root) {
  $$("#pf-tabs button", root).forEach((b) => (b.onclick = () => {
    state.portfolioTab = b.dataset.k; prefSet("portfolioTab", state.portfolioTab); render();
  }));
  $$(".holding", root).forEach((h) => (h.onclick = () => openQuote(h.dataset.symbol)));
  if (state.portfolioTab !== "real") return;
  $$("[data-del]", root).forEach((b) => (b.onclick = async () => {
    if (!confirm("Delete this transaction?")) return;
    await attempt(() => call("remove_transaction", { id: Number(b.dataset.del) }), "Deleted");
    render();
  }));
  $("#rp-add", root).onclick = () => openSheet(`
    <div class="sheet-head"><h3>Add trade</h3><button class="icon-btn" data-close>✕</button></div>
    <div class="side-toggle"><button class="buy active" data-side="Buy">BUY</button><button class="sell" data-side="Sell">SELL</button></div>
    <label>Date</label><input id="tx-date" type="date" value="${new Date().toISOString().slice(0, 10)}">
    <label>Security</label><input id="tx-symbol" list="symbols2" placeholder="JKH.N0000" autocapitalize="characters">
    <datalist id="symbols2">${state.quotes.map((q) => `<option value="${esc(q.symbol)}">`).join("")}</datalist>
    <div class="grid2"><div><label>Quantity</label><input id="tx-qty" type="number" inputmode="numeric"></div>
    <div><label>Price</label><input id="tx-price" type="number" inputmode="decimal" step="0.01"></div></div>
    <label>Fees (optional, from contract note)</label><input id="tx-fees" type="number" inputmode="decimal" step="0.01" placeholder="Use fee schedule">
    <div class="btn-row"><button class="btn primary" id="tx-save">Save</button></div>`, (s) => {
    let side = "Buy";
    $$(".side-toggle button", s).forEach((b) => (b.onclick = () => {
      side = b.dataset.side;
      $$(".side-toggle button", s).forEach((x) => x.classList.toggle("active", x === b));
    }));
    $("#tx-save", s).onclick = async () => {
      const fees = $("#tx-fees", s).value;
      const tx = {
        date: $("#tx-date", s).value, symbol: $("#tx-symbol", s).value, side,
        qty: parseInt($("#tx-qty", s).value, 10) || 0, price: String($("#tx-price", s).value || "0"),
        fees: fees === "" ? null : String(fees),
      };
      await attempt(() => call("add_transaction", { tx }), "Trade added");
      closeSheet(); render();
    };
  });
  $("#rp-import", root).onclick = () => openSheet(`
    <div class="sheet-head"><h3>Import CSV</h3><button class="icon-btn" data-close>✕</button></div>
    <p class="muted small">One trade per line: <code>date,symbol,side,qty,price,fees</code> (fees optional). Dates as YYYY-MM-DD.</p>
    <textarea id="csv-text" placeholder="2026-01-05,JKH.N0000,BUY,1000,18.50,"></textarea>
    <div class="btn-row"><button class="btn primary" id="csv-go">Import</button></div>`, (s) => {
    $("#csv-go", s).onclick = async () => {
      const r = await attempt(() => call("import_csv", { csv: $("#csv-text", s).value }));
      toast(`Imported ${r.imported} trades`); closeSheet(); render();
    };
  });
  $("#rp-export", root).onclick = async () => {
    const csv = await call("export_csv");
    openSheet(`<div class="sheet-head"><h3>Export CSV</h3><button class="icon-btn" data-close>✕</button></div>
      <textarea readonly id="csv-out">${esc(csv)}</textarea>
      <div class="btn-row"><button class="btn primary" id="csv-copy">Copy</button></div>`, (s) => {
      $("#csv-copy", s).onclick = async () => {
        try { await navigator.clipboard.writeText(csv); toast("Copied"); } catch { $("#csv-out", s).select(); }
      };
    });
  };
}

// ---------- Alerts ----------
// Android-only bridge to the background service (see MainActivity.kt).
const bgBridge = (() => {
  let api;
  return () => {
    if (api !== undefined) return api;
    const port = window.CseBackgroundPort;
    if (!port) return (api = null);
    let seq = 0;
    const waiting = new Map();
    port.onmessage = (e) => {
      const m = JSON.parse(e.data);
      const done = waiting.get(m.id);
      if (done) { waiting.delete(m.id); done(m.result); }
    };
    const send = (cmd, extra = {}) => new Promise((resolve) => {
      const id = ++seq;
      waiting.set(id, resolve);
      port.postMessage(JSON.stringify({ id, cmd, ...extra }));
    });
    return (api = {
      start: (path) => send("start", { path }),
      stop: () => send("stop"),
      isRunning: () => send("isRunning"),
      wanted: () => send("wanted"),
      notify: (id, title, body) => send("notify", { nid: Number(id), title, body }),
      schedule: (minutes) => send("schedule", { minutes }),
    });
  };
})();
let lastAlertId = prefGet("lastAlertId", null);
let unseenAlerts = prefGet("unseenAlerts", 0);
function setBellDot() { $("#open-alerts").classList.toggle("has-new", unseenAlerts > 0); }

async function checkAlerts() {
  const r = await call("alerts_pending", { after_id: lastAlertId ?? 0 });
  if (lastAlertId === null) { lastAlertId = r.latest_id; prefSet("lastAlertId", lastAlertId); return; }
  const bridge = bgBridge();
  for (const a of r.alerts) {
    const title = `${a.symbol.replace(/\.N0000$/, "")} · ${a.kind}`;
    toast(`🔔 ${title}: ${a.message}`);
    if (bridge) {
      // The background service posts its own notifications while running.
      if (!(await bridge.isRunning())) await bridge.notify(a.id, title, a.message);
    } else if ("Notification" in window && Notification.permission === "granted") {
      try { new Notification(title, { body: a.message }); } catch { /* ignore */ }
    }
  }
  if (r.alerts.length) {
    lastAlertId = r.latest_id; prefSet("lastAlertId", lastAlertId);
    if (state.tab !== "alerts") { unseenAlerts += r.alerts.length; prefSet("unseenAlerts", unseenAlerts); }
    setBellDot();
    if (state.tab === "alerts") render();
  }
}

function recordLine(r) {
  if (!r || !r.fired) return `<span class="muted">No signals in the past year</span>`;
  return `<span class="${r.win_rate_pct >= 50 ? "up" : "down"}">${r.wins}/${r.fired} beat fees (${fmt(r.win_rate_pct, 0)}%)</span>
    · avg ${signed(r.avg_return_pct)}% after ${r.horizon_days} days`;
}
function distance(price, level) {
  if (!price || !level) return "";
  const d = (num(level) / num(price) - 1) * 100;
  return `<span class="muted small">(${d >= 0 ? "+" : ""}${fmt(d, 1)}% away)</span>`;
}
const TREND = { Up: ["↗ Uptrend", "up"], Down: ["↘ Downtrend", "down"], Range: ["↔ Range", "flat"] };
function swingChain(st) {
  return st.swings.slice(-6).map((w) => {
    const cls = w.label === "HH" || w.label === "HL" ? "up" : w.label ? "down" : "muted";
    return `<span class="${cls}">${w.label || (w.kind === "High" ? "H" : "L")}&nbsp;${fmt(w.price)}</span>`;
  }).join(" → ");
}
function setupBlock(x, labels) {
  const P = x.plan;
  const when = x.triggered ? `<span class="mk-look">Triggered</span>` : `<span class="${x.distance_pct <= 2 ? "mk-ready" : "muted"}">${fmt(x.distance_pct, 1)}% to ${fmt(x.trigger)}</span>`;
  return `<div class="sig ${P.passes ? "" : "off"}">
    <div class="sig-head"><b>${esc(labels[x.kind] || x.kind)}</b><span class="small">${when}</span></div>
    <div class="small muted">${esc(x.note)}</div>
    <div class="plan num small"><span>Entry <b>${fmt(P.entry)}</b></span><span>Stop <b>${fmt(P.stop)}</b> <span class="down">−${fmt(P.risk_pct, 1)}%</span></span>
      <span>Target <b>${fmt(P.target)}</b> <span class="up">+${fmt(P.reward_pct, 1)}%</span></span><span><b>${fmt(P.reward_risk, 1)}×</b></span></div>
    ${P.passes ? "" : `<div class="small muted">Skipped: reward/risk under the ${fmt(labels.min_rr, 1)}× minimum or too small to cover fees.</div>`}
  </div>`;
}
function structureCard(f, v) {
  const labels = Object.fromEntries(v.structure_labels);
  labels.min_rr = v.params.min_reward_risk;
  const st = f.structure, sv = f.structure_view;
  if (!st) {
    return `<div class="card struct"><b>Market structure</b><div class="small muted">${f.pending ? "Checking… (needs the next data refresh)" : esc(f.error || "Not enough history yet (needs 30 trading days)")}</div></div>`;
  }
  const [tl, tc] = TREND[st.trend];
  const m = markOf(f.symbol);
  const recs = (f.structure_records || []).filter((r) => r.fired);
  return `<div class="card struct">
    <div class="sig-head"><b>Market structure</b>${markBadge(m || (sv && { status: statusKey(sv.status), star: sv.weekly_star }))}</div>
    <div class="small"><span class="${tc}">${tl}</span> · weekly ${st.weekly_trend ? `<span class="${TREND[st.weekly_trend][1]}">${st.weekly_trend}</span>` : "—"}
      · swings of ${fmt(st.threshold_pct, 1)}%+${st.liquid ? "" : ` · <span class="muted">thinly traded (Rs. ${fmtVol(st.avg_turnover)}/day) — no alerts</span>`}</div>
    <div class="small swings">${swingChain(st)}</div>
    ${sv && sv.setups.length ? sv.setups.map((x) => setupBlock(x, labels)).join("")
      : `<div class="small muted" style="margin-top:6px">${st.trend === "Up"
        ? "Uptrend, no entry yet: waiting for a pullback that holds above the last low, or a break of the last high."
        : st.trend === "Down" ? "Downtrend: lower highs and lower lows. Nothing to do until it breaks the last high."
        : "No clear trend: highs and lows are mixed."}</div>`}
    <div class="small" style="margin-top:6px">${recs.length
      ? recs.map((r) => `${esc(labels[r.kind] || r.kind)}: ${recordLine(r)}`).join("<br>")
      : `<span class="muted">No structure signals on this stock in the past year.</span>`}</div>
  </div>`;
}
const statusKey = (s) => ({ LookNow: "look_now", GetReady: "get_ready", Uptrend: "uptrend", Range: "range", Downtrend: "downtrend", Thin: "thin" })[s] || "pending";
function favouriteCard(f, v) {
  const L = f.levels, recs = {};
  (f.records || []).forEach((r) => (recs[r.kind] = r));
  const on = (k) => v.signals[k] === true;
  const row = (kind, title, valueHtml, detail) => `
    <div class="sig ${on(kind) ? "" : "off"}">
      <div class="sig-head"><b>${title}</b><span class="num">${valueHtml}</span></div>
      ${detail ? `<div class="small muted">${detail}</div>` : ""}
      ${recs[kind] ? `<div class="small">${recordLine(recs[kind])}</div>` : ""}
    </div>`;
  let body;
  if (f.pending) body = `<div class="small muted">Preparing levels… (needs the market data refresh)</div>`;
  else if (!L) body = `<div class="small muted">${esc(f.error || "Not enough history yet")}</div>`;
  else {
    const I = f.intraday;
    body = `<details class="indicators"><summary class="small muted">Other indicators${Object.keys(v.labels).some(on) ? "" : " (off)"}</summary>` + (L.liquid ? "" : `<div class="banner">Thinly traded (avg turnover Rs. ${fmtBig(L.avg_turnover)}/day) — signals are off for this stock.</div>`) +
      row("Breakout", "Breakout above", `${fmt(L.breakout_level)} ${distance(f.price, L.breakout_level)}`,
        `20-day high + buffer, with volume above ${fmtVol(L.breakout_volume)} shares/day`) +
      row("Pullback", "Pullback to", L.pullback_level ? `${fmt(L.pullback_level)} ${distance(f.price, L.pullback_level)}` : "—",
        L.uptrend ? "20-day average, while in an uptrend" : "Not in an uptrend (price or 20-day average below 50-day)") +
      row("TrendTurn", "Trend turn at", L.trend_turn_level ? `${fmt(L.trend_turn_level)} ${distance(f.price, L.trend_turn_level)}` : "—",
        L.sma_fast > L.sma_slow ? "20-day average already above the 50-day"
          : `20-day avg ${fmt(L.sma_fast)} below 50-day ${fmt(L.sma_slow)}${L.trend_turn_level ? "" : " — crossing is far off"}`) +
      row("OversoldBounce", "Oversold bounce at", L.oversold_level ? `${fmt(L.oversold_level)} ${distance(f.price, L.oversold_level)}` : "—",
        `RSI(14) ${fmt(L.rsi, 0)}${L.oversold_level ? " — below 30" : " — not oversold"}`) +
      row("OpeningRange", "Opening range breakout",
        I && I.breakout_level ? fmt(I.breakout_level) : "—",
        I ? (I.moves_cover_fees ? `09:30–10:00 high ${fmt(I.opening_high)}` : "Typical daily move too small to cover fees — off")
          : "Checked during market hours") +
      row("VolumeSpike", "Volume spike", I ? `> ${fmtVol(I.avg_bar_volume * v.params.spike_mult)} shares` : "—",
        `${fmt(v.params.spike_mult, 0)}× usual volume in a rising 15-minute bar`) + `</details>`;
  }
  return `<div class="card fav-card">
    <div class="fav-head" data-symbol="${esc(f.symbol)}">
      <div><b>${esc(f.symbol)}</b><div class="small muted">${esc(f.name || "")}</div></div>
      <div class="right"><div class="num">${fmt(f.price)}</div><div class="small num ${dir(f.change_pct)}">${f.change_pct != null ? pct(f.change_pct) : ""}</div></div>
    </div>
    ${structureCard(f, v)}
    ${body}
    <div class="btn-row"><button class="btn small ghost" data-custom="${esc(f.symbol)}">+ My own price alert</button></div>
  </div>`;
}

async function renderAlerts() {
  let v = await call("alerts");
  const br = bgBridge();
  if (br) {
    const running = await br.isRunning();
    if (running !== v.background) {
      // Stopped from the notification: fall back to timed checks.
      const mode = running ? "live" : v.background_mode === "live" ? "30" : v.background_mode;
      await call("update_alert_config", { background: running, background_mode: mode });
      if (!running && v.background_mode === "live") await br.schedule(30);
      v = { ...v, background: running, background_mode: mode };
    }
  }

  unseenAlerts = 0; prefSet("unseenAlerts", 0); setBellDot();
  markers = await call("favourite_markers").catch(() => markers);
  const bridge = bgBridge();
  const chip = (k, label) => `<button class="chip ${v.signals[k] === true ? "on" : ""}" data-sig="${k}">${esc(label)}</button>`;
  const structChips = v.structure_labels.map(([k, label]) => chip(k, label)).join("");
  const chips = Object.keys(v.labels).map((k) => chip(k, v.labels[k])).join("");
  const P = v.params;
  const numIn = (id, val, step, label, help) => `<div><label for="${id}">${label}</label>
    <input id="${id}" type="number" inputmode="decimal" step="${step}" value="${esc(val)}"><div class="small muted">${help}</div></div>`;
  const modes = [["off", "Off"], ["60", "Every hour"], ["30", "Every 30 min"], ["15", "Every 15 min"], ["live", "Live"]];
  return `
    <div class="card">
      <label class="switch-row"><span><b>Alerts</b><br><span class="small muted">Signals for stocks in your watch lists</span></span>
        <input type="checkbox" id="al-enabled" ${v.enabled ? "checked" : ""}></label>
      <div class="switch-row"><span><b>Background checks</b><br><span class="small muted">
        ${bridge ? "When the app is closed, during trading hours 9:30–2:30 plus one check after the close. Live checks every few seconds and shows a permanent notification."
                 : "Available in the Android app."}</span></span></div>
      <div class="seg bg-modes">${modes.map(([k, label]) =>
        `<button data-bg="${k}" class="${v.background_mode === k ? "active" : ""}" ${bridge ? "" : "disabled"}>${label}</button>`).join("")}</div>
      <label class="switch-row"><span><b>Get ready notifications</b><br><span class="small muted">Also notify when a stock is within ${fmt(P.near_pct, 1)}% of a trigger (always marked in the Watch list)</span></span>
        <input type="checkbox" id="al-ready" ${v.notify_get_ready ? "checked" : ""}></label>
    </div>
    <h2>Market structure</h2>
    <div class="chips">${structChips}</div>
    <details class="card struct-settings"><summary><b>Structure settings</b> <span class="small muted">swings ${fmt(P.min_swing_pct, 0)}%+ · reward/risk ${fmt(P.min_reward_risk, 1)}×</span></summary>
      <div class="grid2">
        ${numIn("p-swing", P.min_swing_pct, "0.5", "Smallest swing (%)", "Moves smaller than this are noise")}
        ${numIn("p-mult", P.swing_range_mult, "0.5", "× daily range", "Swing must also be this many typical days")}
        ${numIn("p-rr", P.min_reward_risk, "0.5", "Min reward/risk", "Skip setups paying less than this")}
        ${numIn("p-near", P.near_pct, "0.5", "Get ready within (%)", "Distance to trigger for “Get ready”")}
        ${numIn("p-turn", P.min_avg_turnover, "100000", "Min daily turnover (Rs.)", "Below this a stock is “thin”: no alerts")}
      </div>
      <div class="btn-row"><button class="btn small primary" id="p-save">Save</button></div>
    </details>
    <h2>Other indicators</h2>
    <div class="chips">${chips}</div>
    <h2>Favourites</h2>
    ${v.favourites.length ? sortQuotesBy(v.favourites, "look").map((f) => favouriteCard(f, v)).join("")
      : `<div class="card empty">No favourites yet. Open a stock and tap ☆ to add it to a watch list.</div>`}
    <h2>My price alerts</h2>
    <div class="list">${v.custom.length ? v.custom.map((c) => `
      <div class="row" style="cursor:default"><div><b>${esc(c.symbol)}</b>
        <div class="sub">${c.above ? "Rises to or above" : "Falls to or below"} ${fmt(c.price)}</div></div>
        <div class="right small ${c.active ? "up" : "muted"}">${c.active ? "Active" : "Triggered"}</div>
        <button class="btn small ghost" data-delcustom="${c.id}" aria-label="Delete">✕</button></div>`).join("")
      : `<div class="empty">None. Use “+ My own price alert” on a favourite.</div>`}</div>
    <h2>Alert history</h2>
    <div class="list">${v.history.length ? v.history.map((a) => `
      <div class="row" data-symbol="${esc(a.symbol)}"><div><b>${esc(a.symbol)}</b> <span class="small">${esc(a.kind)}</span>
        <div class="sub">${esc(a.message)}</div></div>
        <div class="right num small">${fmt(a.price)}</div><div class="right small muted">${timeStr(a.at)}</div></div>`).join("")
      : `<div class="empty">No alerts yet. Signals are checked during market hours.</div>`}</div>
    ${v.history.length ? `<div class="btn-row"><button class="btn small ghost" id="al-clear">Clear history</button></div>` : ""}
    <p class="muted small">Signals are prompts to look at a stock, not buy recommendations. Higher low entry and Uptrend confirmed fire once per swing; “Downtrend broken” is riskier and off by default. Track records show how each signal did on that stock over the past year, after ~${fmt(v.params.round_trip_fee_pct)}% buy+sell fees.</p>`;
}
function mountAlerts(root) {
  $("#al-enabled", root).onchange = (e) => attempt(() => call("update_alert_config", { enabled: e.target.checked }));
  $$("[data-bg]", root).forEach((b) => (b.onclick = async () => {
    const mode = b.dataset.bg;
    try {
      await applyBackground(mode);
      await call("update_alert_config", { background_mode: mode, background: mode === "live" });
      toast(BG_TEXT[mode]);
    } catch (err) { toast(String(err.message || err), true); }
    render();
  }));
  $("#al-ready", root).onchange = (e) => attempt(() => call("update_alert_config", { notify_get_ready: e.target.checked }));
  $("#p-save", root).onclick = async () => {
    const v = await call("alerts");
    const val = (id) => num($(id, root).value);
    const params = { ...v.params, min_swing_pct: val("#p-swing"), swing_range_mult: val("#p-mult"), min_reward_risk: val("#p-rr"), near_pct: val("#p-near"), min_avg_turnover: val("#p-turn") };
    if (Object.values(params).some((x) => typeof x === "number" && !Number.isFinite(x))) { toast("Enter numbers only", true); return; }
    await attempt(() => call("update_alert_config", { params }), "Saved — markers update on the next check");
    render();
  };
  $$("[data-sig]", root).forEach((b) => (b.onclick = async () => {
    const on = !b.classList.contains("on");
    await attempt(() => call("update_alert_config", { signals: { [b.dataset.sig]: on } }));
    render();
  }));
  $$("[data-custom]", root).forEach((b) => (b.onclick = () => openCustomAlert(b.dataset.custom)));
  $$("[data-delcustom]", root).forEach((b) => (b.onclick = async () => {
    await attempt(() => call("remove_custom_alert", { id: Number(b.dataset.delcustom) }), "Alert removed");
    render();
  }));
  $$(".fav-head", root).forEach((h) => (h.onclick = () => openQuote(h.dataset.symbol)));
  const clear = $("#al-clear", root);
  if (clear) clear.onclick = async () => { await attempt(() => call("clear_alert_history")); render(); };
}
function openCustomAlert(symbol) {
  const q = state.quotes.find((x) => x.symbol === symbol);
  openSheet(`
    <div class="sheet-head"><h3>Price alert · ${esc(symbol)}</h3><button class="icon-btn" data-close>✕</button></div>
    <p class="muted small">Current price ${fmt(q && q.last)}. Fires once, during market hours.</p>
    <div class="side-toggle"><button class="buy active" data-dir="up">Rises to</button><button class="sell" data-dir="down">Falls to</button></div>
    <label>Price</label><input id="ca-price" type="number" inputmode="decimal" step="0.05" value="${q ? esc(num(q.last)) : ""}">
    <div class="btn-row"><button class="btn primary" id="ca-save">Create alert</button></div>`, (s) => {
    let above = true;
    $$(".side-toggle button", s).forEach((b) => (b.onclick = () => {
      above = b.dataset.dir === "up";
      $$(".side-toggle button", s).forEach((x) => x.classList.toggle("active", x === b));
    }));
    $("#ca-save", s).onclick = async () => {
      const price = String($("#ca-price", s).value || "");
      if (!price) return;
      await attempt(() => call("add_custom_alert", { symbol, above, price }), "Alert created");
      closeSheet(); render();
    };
  });
}
$("#open-alerts").onclick = () => switchTab("alerts");

// On launch, restore background mode if it was on and Android stopped it
// (e.g. after a restart), but not if the user tapped Stop.
// Live runs the foreground service; the timed modes schedule periodic
// checks (Android WorkManager) that run the Rust check without the UI.
async function applyBackground(mode) {
  const br = bgBridge();
  if (!br) throw new Error("Background checks need the Android app");
  if (mode === "live") {
    await br.schedule(0);
    await br.start((await call("outbox_path")) || "");
  } else {
    await br.stop();
    await br.schedule(mode === "off" ? 0 : Number(mode));
  }
}
async function restoreBackground() {
  const br = bgBridge();
  if (!br) return;
  const cfg = await call("alerts");
  if (cfg.background_mode !== "live") { await br.schedule(cfg.background_mode === "off" ? 0 : Number(cfg.background_mode)); return; }
  if (await br.isRunning()) return;
  if (await br.wanted()) await br.start((await call("outbox_path")) || "");
  else await call("update_alert_config", { background: false, background_mode: "30" }).then(() => br.schedule(30));
}

// ---------- Settings ----------
async function openSettings() {
  const [settings, account] = await Promise.all([call("settings"), call("account")]);
  const fees = settings.fees.components;
  const bands = settings.ticks.bands;
  openSheet(`
    <div class="sheet-head"><h3>Settings</h3><button class="icon-btn" data-close>✕</button></div>
    <h2>Fees per side (%)</h2>
    <div id="fee-rows">${fees.map((f, i) => `<div class="grid2" style="margin-bottom:6px">
      <input data-fee-name="${i}" value="${esc(f.name)}"><input data-fee-rate="${i}" type="number" step="0.001" value="${esc(f.rate_pct)}"></div>`).join("")}</div>
    <p class="muted small">Total: <b id="fee-total">${fmt(fees.reduce((s, f) => s + num(f.rate_pct), 0), 3)}%</b>. Check these against your broker's contract note.</p>
    <h2>Tick sizes</h2>
    ${bands.map((b, i) => `<div class="grid2" style="margin-bottom:6px">
      <div><label>From price</label><input data-band-from="${i}" type="number" step="0.01" value="${esc(b.from)}"></div>
      <div><label>Tick</label><input data-band-tick="${i}" type="number" step="0.01" value="${esc(b.tick)}"></div></div>`).join("")}
    <h2>Market holidays</h2>
    <textarea id="holidays" placeholder="YYYY-MM-DD, one per line">${esc(settings.calendar.holidays.join("\n"))}</textarea>
    <div class="btn-row"><button class="btn primary" id="set-save">Save settings</button></div>
    <h2>Paper account</h2>
    <label>Starting cash (Rs.)</label><input id="start-cash" type="number" value="${esc(num(account.starting_cash))}">
    <div class="btn-row"><button class="btn sell" id="reset">Reset paper account</button></div>
    <p class="muted small">Market data: Colombo Stock Exchange public website. Paper trading only — no real orders are ever sent.</p>`, (s) => {
    $$("[data-fee-rate]", s).forEach((el) => (el.oninput = () => {
      const total = $$("[data-fee-rate]", s).reduce((t, x) => t + (num(x.value) || 0), 0);
      $("#fee-total", s).textContent = fmt(total, 3) + "%";
    }));
    $("#set-save", s).onclick = async () => {
      settings.fees.components = fees.map((f, i) => ({
        name: $(`[data-fee-name="${i}"]`, s).value,
        rate_pct: String($(`[data-fee-rate="${i}"]`, s).value || "0"),
      }));
      settings.ticks.bands = bands.map((b, i) => ({
        from: String($(`[data-band-from="${i}"]`, s).value || "0"),
        tick: String($(`[data-band-tick="${i}"]`, s).value || "0.1"),
      }));
      settings.calendar.holidays = $("#holidays", s).value.split(/[\s,]+/).filter(Boolean);
      await attempt(() => call("update_settings", { settings }), "Settings saved");
      closeSheet(); render();
    };
    $("#reset", s).onclick = async () => {
      const cash = String($("#start-cash", s).value || "0");
      if (!confirm(`Reset the paper account to Rs. ${fmt(cash, 0)}? All paper orders and holdings will be deleted.`)) return;
      await attempt(() => call("reset_account", { starting_cash: cash }), "Paper account reset");
      closeSheet(); render();
    };
  });
}
$("#open-settings").onclick = () => attempt(openSettings);

// ---------- routing & refresh ----------
const views = {
  market: [renderMarket, mountMarket],
  watch: [renderWatch, mountWatch],
  trade: [renderTrade, mountTrade],
  orders: [renderOrders, mountOrders],
  portfolio: [renderPortfolio, mountPortfolio],
  alerts: [renderAlerts, mountAlerts],
};
let renderSeq = 0;
async function render() {
  const seq = ++renderSeq;
  const [view, mount] = views[state.tab];
  let html;
  try { html = await view(); } catch (e) { html = `<div class="empty">${esc(e.message || e)}</div>`; }
  if (seq !== renderSeq) return;
  const root = $("#view");
  root.innerHTML = html;
  $$("[data-symbol]", root).forEach((r) => {
    if (!r.classList.contains("holding")) r.onclick = () => openQuote(r.dataset.symbol);
  });
  mount(root);
}
function switchTab(tab) {
  state.tab = tab;
  $$(".tabbar button").forEach((b) => b.classList.toggle("active", b.dataset.tab === tab));
  window.scrollTo(0, 0);
  render();
}
$$(".tabbar button").forEach((b) => (b.onclick = () => switchTab(b.dataset.tab)));

async function poll() {
  try {
    const [market, quotes] = await Promise.all([call("market"), call("quotes")]);
    state.market = market;
    state.quotes = quotes;
    renderStatus();
    // Live-update screens that show prices, but never wipe what the user is typing.
    const typing = document.activeElement && ["INPUT", "TEXTAREA", "SELECT"].includes(document.activeElement.tagName);
    const sheetOpen = !$("#sheet").classList.contains("hidden");
    if (!typing && !sheetOpen && ["market", "watch", "portfolio"].includes(state.tab)) render();
    if (state.tab === "trade") updateEstimate();
    checkAlerts().catch(() => {});
  } catch (e) {
    $("#status-text").textContent = "Offline";
  }
}

(async function init() {
  try { await call("refresh"); } catch { /* poll will retry */ }
  await loadWatchLists().catch(() => {});
  restoreBackground().catch(() => {});
  await poll();
  render();
  setInterval(poll, 5000);
})();
