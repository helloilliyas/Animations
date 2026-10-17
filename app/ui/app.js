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
  watch: { filter: "", sort: prefGet("watchSort", "symbol"), list: prefGet("watchList", "all") },
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
    <h2>Top stocks</h2>
    <div class="seg" id="top-tabs">${Object.entries(tabs).map(([k, v]) =>
      `<button data-k="${k}" class="${state.topTab === k ? "active" : ""}">${v}</button>`).join("")}</div>
    <div class="list" style="margin-top:10px">${moverRows(lists[state.topTab], state.topTab)}</div>
    <p class="muted small">Updated ${timeStr(m.updated_at)} · ${m.securities} securities</p>`;
}
function mountMarket(root) {
  $$("#top-tabs button", root).forEach((b) => (b.onclick = () => {
    state.topTab = b.dataset.k; prefSet("topTab", state.topTab); render();
  }));
}

// ---------- Watch ----------
let watchLists = [];
function sortQuotes(list) {
  const by = state.watch.sort;
  const key = {
    symbol: (q) => q.symbol,
    change: (q) => -num(q.change_pct),
    turnover: (q) => -num(q.turnover),
    trades: (q) => -num(q.trades),
  }[by];
  return [...list].sort((a, b) => {
    const x = key(a), y = key(b);
    return typeof x === "string" ? x.localeCompare(y) : x - y;
  });
}
function renderWatch() {
  const f = state.watch.filter.trim().toUpperCase();
  let list = state.quotes;
  if (state.watch.list !== "all") {
    const wl = watchLists.find((w) => String(w.id) === String(state.watch.list));
    const set = new Set(wl ? wl.symbols : []);
    list = list.filter((q) => set.has(q.symbol));
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
    </div>
    <div class="toolbar">
      <input id="watch-search" placeholder="Search symbol or company" value="${esc(state.watch.filter)}" autocomplete="off">
      <select id="watch-sort" style="max-width:130px">
        ${["symbol", "change", "turnover", "trades"].map((s) =>
          `<option value="${s}" ${state.watch.sort === s ? "selected" : ""}>${{ symbol: "Symbol", change: "Change %", turnover: "Turnover", trades: "Trades" }[s]}</option>`).join("")}
      </select>
    </div>
    <div class="list">${list.length ? list.slice(0, 400).map((q) => `
      <div class="row" data-symbol="${esc(q.symbol)}">
        <div><div class="sym">${esc(q.symbol)}</div><div class="sub">${esc(q.name)}</div></div>
        <div class="right"><div class="num">${fmt(q.last)}</div><div class="small muted num">${fmtInt(q.share_volume)}</div></div>
        <div class="right"><span class="pill ${dir(q.change)} num">${pct(q.change_pct)}</span></div>
      </div>`).join("") : `<div class="empty">${state.watch.list === "all" ? "No securities match" : "This list is empty. Open a stock and tap ☆ to add it."}</div>`}
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
  $("#wl-new", root).onclick = async () => {
    const name = prompt("Watch list name");
    if (!name) return;
    const r = await attempt(() => call("create_watch_list", { name }), "Watch list created");
    state.watch.list = String(r.id); prefSet("watchList", state.watch.list);
    await loadWatchLists(); render();
  };
}
async function loadWatchLists() { watchLists = await call("watch_lists"); }

// ---------- Quote sheet ----------
function chartSvg(points) {
  if (!points || points.length < 2) {
    return `<div class="empty small">The chart fills in as the app records prices during market hours.</div>`;
  }
  const w = 600, h = 160, pad = 6;
  const ys = points.map((p) => num(p.price));
  const xs = points.map((p) => new Date(p.at).getTime());
  const minY = Math.min(...ys), maxY = Math.max(...ys);
  const minX = xs[0], maxX = xs[xs.length - 1];
  const sx = (x) => pad + ((x - minX) / (maxX - minX || 1)) * (w - 2 * pad);
  const sy = (y) => h - pad - ((y - minY) / (maxY - minY || 1)) * (h - 2 * pad);
  const d = points.map((p, i) => `${i ? "L" : "M"}${sx(xs[i]).toFixed(1)},${sy(ys[i]).toFixed(1)}`).join("");
  const color = ys[ys.length - 1] >= ys[0] ? "var(--up)" : "var(--down)";
  return `<svg class="chart" viewBox="0 0 ${w} ${h}" preserveAspectRatio="none" role="img" aria-label="Price chart">
    <path class="line" d="${d}" stroke="${color}"/></svg>
    <div class="legend small muted"><span>${fmt(minY)} – ${fmt(maxY)}</span><span>${points.length} points</span></div>`;
}
async function openQuote(symbol) {
  const data = await attempt(() => call("quote", { symbol }));
  const q = data.quote;
  if (!q) { toast("No data for " + symbol, true); return; }
  await loadWatchLists();
  const inList = watchLists.some((w) => w.symbols.includes(symbol));
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
    ${chartSvg(data.history)}
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
    </div>`, (root) => {
    $("#q-buy", root).onclick = () => { closeSheet(); goTicket(symbol, "Buy", q.last); };
    $("#q-sell", root).onclick = () => { closeSheet(); goTicket(symbol, "Sell", q.last); };
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
  } catch (e) {
    $("#status-text").textContent = "Offline";
  }
}

(async function init() {
  try { await call("refresh"); } catch { /* poll will retry */ }
  await loadWatchLists().catch(() => {});
  await poll();
  render();
  setInterval(poll, 5000);
})();
