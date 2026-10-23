//! End-to-end tests of the command API, with a local stub serving the real
//! CSE responses captured in `crates/cse-client/tests/fixtures`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, TimeZone, Utc};
use cse_client::CseClient;
use cse_core::market_hours::colombo;
use cse_service::AppService;
use cse_storage::Store;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/../cse-client/tests/fixtures/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(path).unwrap()
}

/// Serves `routes[path]` for `POST /api/<path>`, forever.
async fn stub(routes: HashMap<&'static str, String>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let routes = Arc::new(routes);
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = listener.accept().await.unwrap();
            let routes = routes.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let path = req
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("")
                    .trim_start_matches("/api/");
                // Twelve Data requests are GETs with a query string.
                let path = path.split('?').next().unwrap_or("");
                // Intraday chart requests (CSE period=1, Twelve Data 5min)
                // get their own fixture.
                let intraday = format!("{path}#1D");
                let is_intraday = req.contains("period=1") || req.contains("interval=5min");
                let key = if is_intraday && routes.contains_key(intraday.as_str()) {
                    intraday.as_str()
                } else {
                    path
                };
                let (status, body) = match routes.get(key) {
                    Some(b) => (200, b.clone()),
                    None => (404, "{}".to_owned()),
                };
                let reply = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(reply.as_bytes()).await;
            });
        }
    });
    format!("http://{addr}/api")
}

fn chart_routes(r: &mut HashMap<&'static str, String>) {
    r.insert("companyChartDataByStock", fixture("stockChart-1Y"));
    r.insert("companyChartDataByStock#1D", fixture("stockChart-1D"));
    r.insert("chartData", fixture("indexChart-1Y"));
    r.insert("chartData#1D", fixture("indexChart-1D"));
}

fn routes(status: &str) -> HashMap<&'static str, String> {
    let mut r: HashMap<&'static str, String> = [
        "aspiData",
        "snpData",
        "marketSummery",
        "tradeSummary",
        "topGainers",
        "topLooses",
        "mostActiveTrades",
        "companyInfoSummery",
    ]
    .into_iter()
    .map(|p| (p, fixture(p)))
    .collect();
    r.insert("marketStatus", format!(r#"{{"status":"{status}"}}"#));
    r
}

/// Thursday 2026-10-15 in Sri Lanka time.
fn lk(h: u32, m: u32) -> DateTime<Utc> {
    colombo()
        .with_ymd_and_hms(2026, 10, 15, h, m, 0)
        .unwrap()
        .with_timezone(&Utc)
}

/// Sri Lanka local time on any October 2026 day.
fn lk_on(d: u32, h: u32, m: u32) -> DateTime<Utc> {
    colombo()
        .with_ymd_and_hms(2026, 10, d, h, m, 0)
        .unwrap()
        .with_timezone(&Utc)
}

struct Harness {
    svc: AppService,
    clock: Arc<Mutex<DateTime<Utc>>>,
    outbox: std::path::PathBuf,
}

impl Harness {
    async fn new(status: &str, at: DateTime<Utc>) -> Self {
        let mut r = routes(status);
        chart_routes(&mut r);
        Self::with_routes(r, at).await
    }

    async fn with_routes(routes: HashMap<&'static str, String>, at: DateTime<Utc>) -> Self {
        let base = stub(routes).await;
        let clock = Arc::new(Mutex::new(at));
        let c = clock.clone();
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let outbox = std::env::temp_dir().join(format!(
            "cse-outbox-{}-{}.jsonl",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let _ = std::fs::remove_file(&outbox);
        let svc = AppService::with(
            Store::open_in_memory().unwrap(),
            CseClient::new(base.clone()).with_retries(0),
            Arc::new(move || *c.lock().unwrap()),
        )
        .unwrap()
        .with_outbox(&outbox)
        .with_twelve(cse_client::twelve::TwelveData::new(base));
        Self { svc, clock, outbox }
    }

    async fn call(&self, cmd: &str, args: Value) -> Value {
        self.svc
            .dispatch(cmd, args)
            .await
            .unwrap_or_else(|e| panic!("{cmd} failed: {e}"))
    }

    fn set_time(&self, at: DateTime<Utc>) {
        *self.clock.lock().unwrap() = at;
    }
}

#[tokio::test]
async fn refresh_builds_market_view_from_real_data() {
    let h = Harness::new("Market Open", lk(10, 0)).await;
    let m = h.call("refresh", Value::Null).await;
    assert_eq!(m["status"], "Market Open");
    assert_eq!(m["phase"], "Open");
    assert_eq!(m["aspi"]["value"], "20185.05");
    assert_eq!(m["snp_sl20"]["value"], "5738.83");
    assert_eq!(m["summary"]["trades"], 15619);
    assert_eq!(m["securities"], 279);
    assert_eq!(m["gainers"].as_array().unwrap().len(), 10);
    assert_eq!(m["top_turnover"].as_array().unwrap().len(), 10);
    assert_eq!(m["most_active"][0]["symbol"], "YORK.N0000");
    let b = &m["breadth"];
    assert!(b["positive"].as_u64().unwrap() > 0);
    assert!(m["errors"].as_array().unwrap().is_empty());

    let quotes = h.call("quotes", Value::Null).await;
    assert_eq!(quotes.as_array().unwrap().len(), 279);
}

#[tokio::test]
async fn market_order_fills_immediately_and_shows_in_portfolio() {
    let h = Harness::new("Market Open", lk(10, 0)).await;
    h.call("refresh", Value::Null).await;

    let est = h
        .call(
            "estimate",
            json!({"symbol": "JKH.N0000", "side": "Buy", "qty": 1000, "price": null}),
        )
        .await;
    assert_eq!(est["price"], "18.0");
    assert_eq!(est["net"], "18201.60");
    assert_eq!(est["price_valid"], true);

    let placed = h
        .call(
            "place_order",
            json!({"order": {"symbol": "JKH.N0000", "side": "Buy",
                             "order_type": "Market", "qty": 1000, "tif": "Day"}}),
        )
        .await;
    let id = placed["id"].as_u64().unwrap();

    let orders = h.call("orders", Value::Null).await;
    assert_eq!(orders[0]["id"], id);
    assert_eq!(orders[0]["status"], "Filled");

    let acc = h.call("account", Value::Null).await;
    assert_eq!(acc["cash"], "981798.40");
    let holding = &acc["portfolio"]["holdings"][0];
    assert_eq!(holding["symbol"], "JKH.N0000");
    assert_eq!(holding["qty"], 1000);
    assert_eq!(holding["break_even"], "18.50"); // 18.2016 / 0.9888 = 18.408 -> next tick

    let q = h.call("quote", json!({"symbol": "JKH.N0000"})).await;
    assert_eq!(q["held"], 1000);
    assert_eq!(q["available_to_sell"], 1000);
}

#[tokio::test]
async fn limit_order_queues_when_closed_and_errors_are_reported() {
    let h = Harness::new("Market Closed", lk(16, 0)).await;
    h.call("refresh", Value::Null).await;
    let placed = h
        .call(
            "place_order",
            json!({"order": {"symbol": "JKH.N0000", "side": "Buy",
                             "order_type": {"Limit": "18.0"}, "qty": 100, "tif": "Day"}}),
        )
        .await;
    let orders = h.call("orders", Value::Null).await;
    assert_eq!(orders[0]["status"], "Queued");

    // Invalid tick size is a readable error, not a panic.
    let err = h
        .svc
        .dispatch(
            "place_order",
            json!({"order": {"symbol": "JKH.N0000", "side": "Buy",
                             "order_type": {"Limit": "18.05"}, "qty": 100, "tif": "Day"}}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "price must be a positive multiple of 0.10");

    h.call("cancel_order", json!({"id": placed["id"]})).await;
    let orders = h.call("orders", Value::Null).await;
    assert_eq!(orders[0]["status"], "Cancelled");
}

#[tokio::test]
async fn exchange_closed_status_prevents_fills_on_a_calendar_day() {
    // Calendar says open (Thursday 10:00) but CSE reports closed.
    let h = Harness::new("Market Closed", lk(10, 0)).await;
    h.call("refresh", Value::Null).await;
    h.call(
        "place_order",
        json!({"order": {"symbol": "JKH.N0000", "side": "Buy",
                         "order_type": {"Limit": "20.0"}, "qty": 10, "tif": "Day"}}),
    )
    .await;
    h.set_time(lk(10, 5));
    h.call("refresh", Value::Null).await;
    let orders = h.call("orders", Value::Null).await;
    assert_eq!(orders[0]["status"], "Open");
}

#[tokio::test]
async fn real_portfolio_watch_lists_and_settings() {
    let h = Harness::new("Market Open", lk(10, 0)).await;
    h.call("refresh", Value::Null).await;

    let r = h
        .call(
            "import_csv",
            json!({"csv": "date,symbol,side,qty,price,fees\n2026-01-05,JKH.N0000,BUY,1000,20.00,\n"}),
        )
        .await;
    assert_eq!(r["imported"], 1);
    let real = h.call("real_portfolio", Value::Null).await;
    let row = &real["valuation"]["holdings"][0];
    assert_eq!(row["last"], "18.0");
    assert_eq!(row["priced"], true);
    assert_eq!(real["transactions"].as_array().unwrap().len(), 1);
    let csv = h.call("export_csv", Value::Null).await;
    assert!(csv.as_str().unwrap().contains("JKH.N0000,BUY,1000,20.00"));

    let wl = h.call("create_watch_list", json!({"name": "Banks"})).await;
    h.call(
        "add_to_watch_list",
        json!({"id": wl["id"], "symbol": "COMB.N0000"}),
    )
    .await;
    let lists = h.call("watch_lists", Value::Null).await;
    assert_eq!(lists[0]["symbols"][0], "COMB.N0000");

    let mut settings = h.call("settings", Value::Null).await;
    settings["fees"]["components"] = json!([{"name": "Flat", "rate_pct": "1.0"}]);
    h.call("update_settings", json!({ "settings": settings }))
        .await;
    let est = h
        .call(
            "estimate",
            json!({"symbol": "JKH.N0000", "side": "Buy", "qty": 100, "price": "18.0"}),
        )
        .await;
    assert_eq!(est["fees"], "18.00");
}

#[tokio::test]
async fn bad_input_is_rejected_cleanly() {
    let h = Harness::new("Market Open", lk(10, 0)).await;
    let e = h.svc.dispatch("nope", Value::Null).await.unwrap_err();
    assert_eq!(e.to_string(), "unknown command \"nope\"");
    let e = h
        .svc
        .dispatch("cancel_order", json!({"wrong": 1}))
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("bad arguments"));
}

#[tokio::test]
async fn charts_from_cse_history_and_intraday() {
    // Saturday 17 Oct 2026, after Friday's session.
    let h = Harness::new("Market Closed", lk_on(17, 10, 0)).await;
    h.call("refresh", Value::Null).await;

    let year = h
        .call("chart", json!({"symbol": "JKH.N0000", "range": "1Y"}))
        .await;
    assert_eq!(year["source"], "history");
    assert_eq!(year["error"], Value::Null);
    assert_eq!(year["points"].as_array().unwrap().len(), 241);
    assert_eq!(year["history_days"], 241);
    assert_eq!(year["points"][240]["price"], "18.0");

    let month = h
        .call("chart", json!({"symbol": "JKH.N0000", "range": "1M"}))
        .await;
    let n = month["points"].as_array().unwrap().len();
    assert!((18..=24).contains(&n), "1M had {n} points");

    let day = h
        .call("chart", json!({"symbol": "JKH.N0000", "range": "1D"}))
        .await;
    assert_eq!(day["source"], "cse");
    assert_eq!(day["points"].as_array().unwrap().len(), 349);

    let aspi = h.call("chart", json!({"range": "1Y"})).await;
    assert_eq!(aspi["key"], "ASPI");
    assert_eq!(aspi["points"].as_array().unwrap().len(), 240);
    assert_eq!(aspi["points"][239]["price"], "20185.05");
    let aspi_day = h.call("chart", json!({"range": "1D"})).await;
    assert_eq!(aspi_day["points"].as_array().unwrap().len(), 300);
}

#[tokio::test]
async fn session_closes_are_stored_and_survive_cse_outages() {
    // No chart routes: the CSE history feed is "down".
    // Friday 16 Oct 15:00, after the close; fixture trades are from that day.
    let h = Harness::with_routes(routes("Market Closed"), lk_on(16, 15, 0)).await;
    h.call("refresh", Value::Null).await;

    let all = h
        .call("chart", json!({"symbol": "ABAN.N0000", "range": "All"}))
        .await;
    assert!(all["error"].as_str().unwrap().contains("404"));
    let points = all["points"].as_array().unwrap();
    assert_eq!(points.len(), 1, "today's close was saved locally");
    assert_eq!(points[0]["price"], "1025.0");
    assert_eq!(all["history_since"], "2026-10-16");

    let aspi = h.call("chart", json!({"range": "All"})).await;
    assert_eq!(aspi["points"][0]["price"], "20185.05");

    // Intraday falls back to the app's own snapshots (none here) with an error.
    let day = h
        .call("chart", json!({"symbol": "ABAN.N0000", "range": "1D"}))
        .await;
    assert_eq!(day["source"], "local");
    assert!(day["error"].is_string());
}

/// Real price list with JKH changed to a breakout: price 25.00 on 50M shares.
fn breakout_trade_summary() -> String {
    let mut v: Value = serde_json::from_str(&fixture("tradeSummary")).unwrap();
    for q in v["reqTradeSummery"].as_array_mut().unwrap() {
        if q["symbol"] == "JKH.N0000" {
            q["price"] = json!(25.0);
            q["high"] = json!(25.0);
            q["low"] = json!(24.0);
            q["sharevolume"] = json!(50_000_000);
        }
    }
    v.to_string()
}

#[tokio::test]
async fn alerts_fire_for_favourites_and_custom_levels() {
    let mut r = routes("Market Open");
    chart_routes(&mut r);
    r.insert("tradeSummary", breakout_trade_summary());
    // Friday 16 Oct 11:00, a trading day matching the fixture's trades.
    let h = Harness::with_routes(r, lk_on(16, 11, 0)).await;

    let wl = h
        .call("create_watch_list", json!({"name": "Favourites"}))
        .await;
    h.call(
        "add_to_watch_list",
        json!({"id": wl["id"], "symbol": "JKH.N0000"}),
    )
    .await;
    // The indicator signals are off by default since market structure.
    h.call(
        "update_alert_config",
        json!({"signals": {"Breakout": true, "OpeningRange": true}}),
    )
    .await;
    h.call(
        "add_custom_alert",
        json!({"symbol": "COMB.N0000", "above": true, "price": "150"}),
    )
    .await;

    h.call("refresh", Value::Null).await;
    let pending = h.call("alerts_pending", json!({"after_id": 0})).await;
    let alerts = pending["alerts"].as_array().unwrap();
    let kinds: Vec<(&str, &str)> = alerts
        .iter()
        .map(|a| (a["symbol"].as_str().unwrap(), a["kind"].as_str().unwrap()))
        .collect();
    assert!(kinds.contains(&("JKH.N0000", "Breakout")), "{kinds:?}");
    assert!(kinds.contains(&("COMB.N0000", "Custom")), "{kinds:?}");
    let latest = pending["latest_id"].as_u64().unwrap();

    // One combined notification per check for the Android service.
    let outbox = std::fs::read_to_string(&h.outbox).unwrap();
    assert_eq!(outbox.lines().count(), 1);
    let line: Value = serde_json::from_str(outbox.trim()).unwrap();
    assert_eq!(
        line["title"],
        format!("🔔 {} stocks to look at", alerts.len())
    );
    assert!(line["body"].as_str().unwrap().contains("JKH Breakout"));

    // A minute later nothing repeats: cooldown, and custom alerts are one-shot.
    h.set_time(lk_on(16, 11, 1));
    h.call("refresh", Value::Null).await;
    let again = h
        .call("alerts_pending", json!({ "after_id": latest }))
        .await;
    assert!(again["alerts"].as_array().unwrap().is_empty());

    let view = h.call("alerts", Value::Null).await;
    let fav = &view["favourites"][0];
    assert_eq!(fav["symbol"], "JKH.N0000");
    assert!(fav["levels"]["breakout_level"].as_f64().unwrap() > 18.0);
    assert_eq!(fav["records"].as_array().unwrap().len(), 4);
    assert_eq!(fav["history_days"], 240); // today's bar excluded
    assert!(fav["intraday"]["opening_high"].is_number());
    assert_eq!(view["custom"][0]["active"], false);
    assert_eq!(view["history"].as_array().unwrap().len(), alerts.len());
}

#[tokio::test]
async fn alert_settings_switch_signals_off() {
    let mut r = routes("Market Open");
    chart_routes(&mut r);
    r.insert("tradeSummary", breakout_trade_summary());
    let h = Harness::with_routes(r, lk_on(16, 11, 0)).await;
    let wl = h.call("create_watch_list", json!({"name": "Fav"})).await;
    h.call(
        "add_to_watch_list",
        json!({"id": wl["id"], "symbol": "JKH.N0000"}),
    )
    .await;
    h.call(
        "update_alert_config",
        json!({"signals": {"Breakout": false}, "background": true}),
    )
    .await;
    h.call("refresh", Value::Null).await;
    let pending = h.call("alerts_pending", json!({})).await;
    assert!(!pending["alerts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["kind"] == "Breakout"));
    let view = h.call("alerts", Value::Null).await;
    assert_eq!(view["background"], true);
    assert_eq!(view["signals"]["Breakout"], false);
}

#[tokio::test]
async fn no_live_alerts_when_market_closed() {
    let mut r = routes("Market Closed");
    chart_routes(&mut r);
    r.insert("tradeSummary", breakout_trade_summary());
    let h = Harness::with_routes(r, lk_on(17, 11, 0)).await; // Saturday
    let wl = h.call("create_watch_list", json!({"name": "Fav"})).await;
    h.call(
        "add_to_watch_list",
        json!({"id": wl["id"], "symbol": "JKH.N0000"}),
    )
    .await;
    h.call("refresh", Value::Null).await;
    let pending = h.call("alerts_pending", json!({})).await;
    assert!(pending["alerts"].as_array().unwrap().is_empty());
    // Levels are still prepared for the next session.
    let view = h.call("alerts", Value::Null).await;
    assert!(view["favourites"][0]["levels"]["breakout_level"].is_number());
}

#[tokio::test]
async fn empty_pre_open_price_list_keeps_last_prices() {
    // Yesterday's prices, as the app would have cached them.
    let full = Harness::new("Market Closed", lk_on(16, 15, 0)).await;
    full.call("refresh", Value::Null).await;
    let cached: Vec<cse_core::Quote> =
        serde_json::from_value(full.call("quotes", Value::Null).await).unwrap();
    assert_eq!(cached.len(), 279);

    // Next morning the CSE serves an empty list before the open.
    let mut r = routes("Pre Trading");
    r.insert("tradeSummary", r#"{"reqTradeSummery":[]}"#.to_owned());
    let h = Harness::with_routes(r, lk_on(19, 9, 10)).await;
    h.svc.set_market(cse_service::MarketData {
        quotes: cached.into_iter().map(|q| (q.symbol.clone(), q)).collect(),
        ..Default::default()
    });
    h.call("refresh", Value::Null).await;
    assert_eq!(
        h.call("quotes", Value::Null)
            .await
            .as_array()
            .unwrap()
            .len(),
        279
    );
}

#[tokio::test]
async fn import_watch_list_from_codes() {
    let mut r = routes("Market Closed");
    r.remove("companyInfoSummery"); // lookups for untraded codes fail here
    let h = Harness::with_routes(r, lk_on(17, 10, 0)).await;
    h.call("refresh", Value::Null).await;

    let res = h
        .call(
            "import_watch_list",
            json!({"name": "Shariah", "text": "ABAN.N, jkh\nCOMB.N0000; NOPE.N"}),
        )
        .await;
    assert_eq!(res["count"], 3);
    assert_eq!(
        res["added"],
        json!(["ABAN.N0000", "JKH.N0000", "COMB.N0000"])
    );
    assert_eq!(res["invalid"], json!(["NOPE.N0000"]));

    // Re-import replaces: JKH and COMB out, HNB in.
    let res = h
        .call(
            "import_watch_list",
            json!({"name": "Shariah", "text": "ABAN HNB"}),
        )
        .await;
    assert_eq!(res["added"], json!(["HNB.N0000"]));
    assert_eq!(res["removed"], json!(["COMB.N0000", "JKH.N0000"]));
    let lists = h.call("watch_lists", Value::Null).await;
    assert_eq!(lists[0]["symbols"], json!(["ABAN.N0000", "HNB.N0000"]));

    // Nothing valid: a clear error, list untouched.
    let err = h
        .svc
        .dispatch(
            "import_watch_list",
            json!({"name": "Shariah", "text": "XXQ.N"}),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("none of the codes"));
}

/// A clean uptrend for JKH: higher highs (110, 120, 135), higher lows
/// (90, 105), now pulled back to 122 and bouncing.
fn uptrend_chart() -> String {
    let legs: [f64; 7] = [100.0, 110.0, 90.0, 120.0, 105.0, 135.0, 122.0];
    let mut closes = vec![legs[0]];
    for w in legs.windows(2) {
        let steps = ((w[1] / w[0]).ln().abs() / 0.01).ceil() as usize;
        for i in 1..=steps {
            closes.push(w[0] * (w[1] / w[0]).powf(i as f64 / steps as f64));
        }
    }
    // The last bar is Thursday 15 Oct, the day before the test's session.
    let end = lk_on(15, 0, 0).timestamp_millis();
    let n = closes.len() as i64;
    let bars: Vec<Value> = closes
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let c = (c * 100.0_f64).round() / 100.0;
            json!({"h": c * 1.004, "l": c * 0.996, "p": c, "q": 1_000_000,
                   "t": end - (n - 1 - i as i64) * 86_400_000, "id": 297})
        })
        .collect();
    json!({ "chartData": bars, "id": 297 }).to_string()
}

fn price_jkh(price: f64, low: f64) -> String {
    let mut v: Value = serde_json::from_str(&fixture("tradeSummary")).unwrap();
    for q in v["reqTradeSummery"].as_array_mut().unwrap() {
        if q["symbol"] == "JKH.N0000" {
            q["price"] = json!(price);
            q["high"] = json!(price);
            q["low"] = json!(low);
        }
    }
    v.to_string()
}

#[tokio::test]
async fn higher_low_entry_fires_once_and_marks_the_favourite() {
    let mut r = routes("Market Open");
    chart_routes(&mut r);
    r.insert("companyChartDataByStock", uptrend_chart());
    r.insert("tradeSummary", price_jkh(126.0, 124.0));
    let h = Harness::with_routes(r, lk_on(16, 11, 0)).await;
    let wl = h
        .call("create_watch_list", json!({"name": "Favourites"}))
        .await;
    for sym in ["JKH.N0000"] {
        h.call("add_to_watch_list", json!({"id": wl["id"], "symbol": sym}))
            .await;
    }

    // Before any check the markers are pending.
    let m = h.call("favourite_markers", Value::Null).await;
    assert_eq!(m["markers"]["JKH.N0000"]["status"], "pending");

    let fired = h.svc.run_check().await.unwrap();
    let pending = h.call("alerts_pending", json!({"after_id": 0})).await;
    let alerts = pending["alerts"].as_array().unwrap();
    assert_eq!(fired, alerts.len());
    let entry = alerts
        .iter()
        .find(|a| a["symbol"] == "JKH.N0000")
        .unwrap_or_else(|| panic!("{alerts:?}"));
    assert!(entry["kind"]
        .as_str()
        .unwrap()
        .starts_with("Higher low entry"));
    let msg = entry["message"].as_str().unwrap();
    assert!(
        msg.contains("entry 124.55") && msg.contains("target 135.54"),
        "{msg}"
    );

    let outbox = std::fs::read_to_string(&h.outbox).unwrap();
    let line: Value = serde_json::from_str(outbox.trim()).unwrap();
    assert!(
        line["title"]
            .as_str()
            .unwrap()
            .starts_with("JKH · Higher low entry"),
        "{line} {alerts:?}"
    );

    let m = h.call("favourite_markers", Value::Null).await;
    let jkh = &m["markers"]["JKH.N0000"];
    assert_eq!(jkh["status"], "look_now", "{m}");
    assert_eq!(jkh["trend"], "Up");
    assert!(jkh["label"]
        .as_str()
        .unwrap()
        .starts_with("Higher low entry"));
    assert_eq!(m["counts"]["look_now"], 1);
    assert_eq!(m["background_mode"], "30");

    // The stock screen gets the structure and the plan.
    let view = h.call("alerts", Value::Null).await;
    let fav = view["favourites"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["symbol"] == "JKH.N0000")
        .unwrap();
    assert_eq!(fav["structure"]["trend"], "Up");
    assert_eq!(fav["structure_view"]["setups"][0]["triggered"], true);

    // Later checks on the same swing stay quiet.
    h.set_time(lk_on(16, 11, 30));
    assert_eq!(h.svc.run_check().await.unwrap(), 0);
    h.set_time(lk_on(16, 14, 0));
    assert_eq!(h.svc.run_check().await.unwrap(), 0);
}

#[tokio::test]
async fn get_ready_is_a_marker_unless_asked_to_notify() {
    let mut r = routes("Market Open");
    chart_routes(&mut r);
    r.insert("companyChartDataByStock", uptrend_chart());
    // 123.5 is within 2% of the 124.55 trigger.
    r.insert("tradeSummary", price_jkh(123.5, 122.5));
    let h = Harness::with_routes(r, lk_on(16, 11, 0)).await;
    let wl = h
        .call("create_watch_list", json!({"name": "Favourites"}))
        .await;
    h.call(
        "add_to_watch_list",
        json!({"id": wl["id"], "symbol": "JKH.N0000"}),
    )
    .await;
    assert_eq!(h.svc.run_check().await.unwrap(), 0);
    let m = h.call("favourite_markers", Value::Null).await;
    assert_eq!(m["markers"]["JKH.N0000"]["status"], "get_ready", "{m}");

    h.call(
        "update_alert_config",
        json!({"notify_get_ready": true, "background_mode": "15"}),
    )
    .await;
    h.set_time(lk_on(16, 11, 15));
    assert_eq!(h.svc.run_check().await.unwrap(), 1);
    let view = h.call("alerts", Value::Null).await;
    assert_eq!(view["background_mode"], "15");
    assert!(view["history"][0]["kind"]
        .as_str()
        .unwrap()
        .starts_with("Get ready"));
}

#[tokio::test]
async fn old_alert_settings_migrate_to_structure() {
    let base = stub(routes("Market Closed")).await;
    let mut store = Store::open_in_memory().unwrap();
    store
        .save_json(
            "alerts",
            &json!({"enabled": true, "signals": {"Breakout": true, "VolumeSpike": true},
                    "background": true}),
        )
        .unwrap();
    let svc = AppService::with(
        store,
        CseClient::new(base).with_retries(0),
        Arc::new(|| lk_on(16, 15, 0)),
    )
    .unwrap();
    let view = svc.dispatch("alerts", Value::Null).await.unwrap();
    assert_eq!(view["signals"]["Breakout"], false);
    assert_eq!(view["signals"]["VolumeSpike"], false);
    assert_eq!(view["signals"]["HigherLowEntry"], true);
    assert_eq!(view["signals"]["UptrendConfirmed"], true);
    assert_eq!(view["signals"]["DowntrendBroken"], false);
    assert_eq!(view["background_mode"], "live");
}

#[tokio::test]
async fn closing_check_fires_structure_setups_after_the_close() {
    let mut r = routes("Market Closed");
    chart_routes(&mut r);
    r.insert("companyChartDataByStock", uptrend_chart());
    r.insert("tradeSummary", price_jkh(126.0, 124.0));
    // Friday 16 Oct, 14:45: after the close on a trading day.
    let h = Harness::with_routes(r, lk_on(16, 14, 45)).await;
    let wl = h
        .call("create_watch_list", json!({"name": "Favourites"}))
        .await;
    h.call(
        "add_to_watch_list",
        json!({"id": wl["id"], "symbol": "JKH.N0000"}),
    )
    .await;
    assert_eq!(h.svc.run_check().await.unwrap(), 1);
    // Saturday: nothing.
    h.set_time(lk_on(17, 10, 0));
    assert_eq!(h.svc.run_check().await.unwrap(), 0);
}

#[tokio::test]
async fn saved_prices_bridge_the_gap_before_the_open() {
    let db = std::env::temp_dir().join(format!("cse-copy-{}.sqlite", std::process::id()));
    let _ = std::fs::remove_file(&db);
    let open = |routes: HashMap<&'static str, String>, at: DateTime<Utc>| {
        let db = db.clone();
        async move {
            let base = stub(routes).await;
            AppService::with(
                Store::open(&db).unwrap(),
                CseClient::new(base).with_retries(0),
                Arc::new(move || at),
            )
            .unwrap()
        }
    };

    // Friday after the close: the day's prices are copied.
    let svc = open(routes("Market Closed"), lk_on(16, 15, 0)).await;
    svc.dispatch("refresh", Value::Null).await.unwrap();
    let before = svc.dispatch("market", Value::Null).await.unwrap();
    assert!(before["saved_copy"].is_null());
    drop(svc);

    // Monday 09:10, a fresh start: the CSE lists are still empty.
    let mut empty = routes("Pre Trading");
    empty.insert("tradeSummary", r#"{"reqTradeSummery":[]}"#.into());
    empty.insert("topGainers", "[]".into());
    empty.insert("topLooses", "[]".into());
    empty.insert("mostActiveTrades", "[]".into());
    let svc = open(empty, lk_on(19, 9, 10)).await;
    svc.dispatch("refresh", Value::Null).await.unwrap();
    let m = svc.dispatch("market", Value::Null).await.unwrap();
    assert!(m["saved_copy"].is_string(), "{m}");
    assert_eq!(m["securities"], before["securities"]);
    assert_eq!(m["gainers"], before["gainers"]);
    assert_eq!(m["aspi"]["value"], before["aspi"]["value"]);
    let quotes = svc.dispatch("quotes", Value::Null).await.unwrap();
    assert!(quotes.as_array().unwrap().len() > 200);
    drop(svc);

    // 09:31: live prices replace the copy.
    let svc = open(routes("Market Open"), lk_on(19, 9, 31)).await;
    svc.dispatch("refresh", Value::Null).await.unwrap();
    let m = svc.dispatch("market", Value::Null).await.unwrap();
    assert!(m["saved_copy"].is_null());
    let _ = std::fs::remove_file(&db);
}

#[tokio::test]
async fn structure_history_alert_results_and_export() {
    let mut r = routes("Market Open");
    chart_routes(&mut r);
    r.insert("companyChartDataByStock", uptrend_chart());
    r.insert("tradeSummary", price_jkh(126.0, 124.0));
    let h = Harness::with_routes(r, lk_on(16, 11, 0)).await;
    let wl = h
        .call("create_watch_list", json!({"name": "Favourites"}))
        .await;
    h.call(
        "add_to_watch_list",
        json!({"id": wl["id"], "symbol": "JKH.N0000"}),
    )
    .await;
    assert_eq!(h.svc.run_check().await.unwrap(), 1);

    let hist = h
        .call("structure_history", json!({"symbol": "JKH.N0000"}))
        .await;
    let swings = hist["swings"].as_array().unwrap();
    assert!(swings.len() >= 5, "{hist}");
    // Newest first, reconstructed from the downloaded year.
    assert_eq!(swings[0]["label"], "HH");
    assert_eq!(swings[0]["trend"], "Up");
    assert!(swings.iter().all(|w| w["backfilled"] == true));
    assert_eq!(hist["periods"][0]["state"], "Up");
    assert!(hist["days"].as_u64().unwrap() > 50);
    let alert = &hist["alerts"][0];
    assert!(alert["kind"]
        .as_str()
        .unwrap()
        .starts_with("Higher low entry"));
    assert_eq!(alert["outcome"]["status"], "open");
    assert!(alert["entry"].as_f64().unwrap() > 124.0);

    let results = h.call("alert_results", Value::Null).await;
    let s = &results["summary"]["Higher low entry"];
    assert_eq!(s["alerts"], 1, "{results}");
    assert_eq!(s["open"], 1);

    let csv = h.call("export_history", Value::Null).await;
    let text = csv["csv"].as_str().unwrap();
    assert!(text.starts_with("# Swings\n"));
    assert!(text.contains("\n# Daily structure\n") && text.contains("\n# Alerts\n"));
    assert!(text.contains("JKH.N0000,2026-"));

    // The next day's check adds no duplicate swings.
    h.set_time(lk_on(19, 11, 0));
    h.svc.run_check().await.unwrap();
    let again = h
        .call("structure_history", json!({"symbol": "JKH.N0000"}))
        .await;
    assert_eq!(again["swings"].as_array().unwrap().len(), swings.len());
}

#[tokio::test]
async fn uptrend_broken_marker_and_optional_notification() {
    let mut r = routes("Market Open");
    chart_routes(&mut r);
    r.insert("companyChartDataByStock", uptrend_chart());
    // The last higher low is about 104.6; today's low goes under it.
    r.insert("tradeSummary", price_jkh(103.0, 102.0));
    let h = Harness::with_routes(r, lk_on(16, 11, 0)).await;
    let wl = h
        .call("create_watch_list", json!({"name": "Favourites"}))
        .await;
    h.call(
        "add_to_watch_list",
        json!({"id": wl["id"], "symbol": "JKH.N0000"}),
    )
    .await;
    // Off by default: a marker, no notification.
    assert_eq!(h.svc.run_check().await.unwrap(), 0);
    let m = h.call("favourite_markers", Value::Null).await;
    let jkh = &m["markers"]["JKH.N0000"];
    assert_eq!(jkh["status"], "broken", "{m}");
    assert_eq!(jkh["label"], "Uptrend broken");

    h.call("update_alert_config", json!({"notify_broken": true}))
        .await;
    h.set_time(lk_on(16, 11, 30));
    assert_eq!(h.svc.run_check().await.unwrap(), 1);
    let view = h.call("alerts", Value::Null).await;
    assert_eq!(view["history"][0]["kind"], "Uptrend broken");
    assert!(view["history"][0]["message"]
        .as_str()
        .unwrap()
        .contains("below the last higher low"));
    // Once per swing.
    h.set_time(lk_on(16, 12, 0));
    assert_eq!(h.svc.run_check().await.unwrap(), 0);
}

/// Twelve Data daily bars for the uptrend used in the CSE tests, ending
/// Thursday 22 Oct (New York date).
fn us_daily_uptrend() -> String {
    let legs: [f64; 7] = [100.0, 110.0, 90.0, 120.0, 105.0, 135.0, 122.0];
    let mut closes = vec![legs[0]];
    for w in legs.windows(2) {
        let steps = ((w[1] / w[0]).ln().abs() / 0.01).ceil() as usize;
        for i in 1..=steps {
            closes.push(w[0] * (w[1] / w[0]).powf(i as f64 / steps as f64));
        }
    }
    let end = chrono::NaiveDate::from_ymd_opt(2026, 10, 22).unwrap();
    let n = closes.len();
    // Newest first, as Twelve Data sends them.
    let values: Vec<Value> = closes
        .iter()
        .enumerate()
        .rev()
        .map(|(i, c)| {
            let c = (c * 100.0_f64).round() / 100.0;
            let day = end - chrono::Duration::days((n - 1 - i) as i64);
            json!({"datetime": day.to_string(), "open": c, "high": c * 1.004,
                   "low": c * 0.996, "close": c, "volume": "30000000"})
        })
        .collect();
    json!({"meta": {"symbol": "AAPL", "interval": "1day"}, "values": values, "status": "ok"})
        .to_string()
}

fn us_quote(t: &str, price: f64, low: f64, at: DateTime<Utc>, open: bool) -> Value {
    json!({"symbol": t, "name": format!("{t} Inc."), "exchange": "NASDAQ", "currency": "USD",
           "timestamp": at.timestamp(), "last_quote_at": at.timestamp(),
           "open": price, "high": price, "low": low, "close": price, "volume": "1000000",
           "previous_close": price - 1.0, "change": "1.0", "percent_change": "0.8",
           "is_market_open": open})
}

#[tokio::test]
async fn us_favourites_get_prices_markers_and_alerts() {
    // Friday 23 Oct 2026 14:00 UTC = 10:00 New York (open), 19:30 Sri Lanka.
    let at = Utc.with_ymd_and_hms(2026, 10, 23, 14, 0, 0).unwrap();
    let mut r = routes("Market Closed");
    chart_routes(&mut r);
    r.insert("time_series", us_daily_uptrend());
    r.insert(
        "quote",
        json!({"AAPL": us_quote("AAPL", 126.0, 124.0, at, true),
               "MSFT": us_quote("MSFT", 100.0, 99.0, at, true)})
        .to_string(),
    );
    r.insert("symbol_search", fixture("twelve-search"));
    let h = Harness::with_routes(r, at).await;

    // Nothing happens without a key.
    let wl = h.call("create_watch_list", json!({"name": "US"})).await;
    for sym in ["AAPL.US", "MSFT.US"] {
        h.call("add_to_watch_list", json!({"id": wl["id"], "symbol": sym}))
            .await;
    }
    h.call("refresh", Value::Null).await;
    assert_eq!(h.call("us_settings", Value::Null).await["used_today"], 0);

    let st = h
        .call("set_us_key", json!({"api_key": " test-key-1234 "}))
        .await;
    assert_eq!(st["has_key"], true);
    assert_eq!(st["key_hint"], "…1234");

    let fired = h.svc.run_check().await.unwrap();
    let quotes = h.call("quotes", Value::Null).await;
    let aapl = quotes
        .as_array()
        .unwrap()
        .iter()
        .find(|q| q["symbol"] == "AAPL.US")
        .expect("US quote in the list");
    assert_eq!(aapl["last"], "126.0");
    // Two history downloads and two quotes.
    let st = h.call("us_settings", Value::Null).await;
    assert_eq!(st["used_today"], 4, "{st}");
    assert!(st["error"].is_null(), "{st}");

    // Same structure engine as the CSE: AAPL bounced off a higher low.
    // Both share the fixture history; MSFT at 100 is below its last higher
    // low, so only AAPL fires.
    assert_eq!(fired, 1);
    let m = h.call("favourite_markers", Value::Null).await;
    assert_eq!(m["markers"]["AAPL.US"]["status"], "look_now", "{m}");
    assert_eq!(m["markers"]["MSFT.US"]["status"], "broken", "{m}");
    let alerts = h.call("alerts_pending", json!({"after_id": 0})).await;
    assert!(alerts["alerts"][0]["kind"]
        .as_str()
        .unwrap()
        .starts_with("Higher low entry"));

    // A few minutes later the quotes are still fresh: no credits used.
    h.set_time(at + chrono::Duration::minutes(2));
    h.call("refresh", Value::Null).await;
    assert_eq!(h.call("us_settings", Value::Null).await["used_today"], 4);

    // Charts come from the stored history.
    let chart = h
        .call("chart", json!({"symbol": "AAPL.US", "range": "1Y"}))
        .await;
    assert!(chart["points"].as_array().unwrap().len() > 100);
    assert_eq!(chart["source"], "twelvedata");

    // Watching only: no paper orders.
    let err = h
        .svc
        .dispatch(
            "place_order",
            json!({"order": {"symbol": "AAPL.US", "side": "Buy", "order_type": "Market", "qty": 1, "tif": "Day"}}),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("watching"), "{err}");

    let hits = h.call("us_search", json!({"text": "AAPL"})).await;
    assert_eq!(hits[0]["symbol"], "AAPL.US");
}

#[tokio::test]
async fn us_requests_stay_within_eight_a_minute() {
    let at = Utc.with_ymd_and_hms(2026, 10, 23, 14, 0, 0).unwrap();
    let mut r = routes("Market Closed");
    chart_routes(&mut r);
    r.insert("time_series", us_daily_uptrend());
    let tickers = ["A", "B", "C", "D", "E", "F"];
    let batch: serde_json::Map<String, Value> = tickers
        .iter()
        .map(|t| (t.to_string(), us_quote(t, 50.0, 49.0, at, true)))
        .collect();
    r.insert("quote", Value::Object(batch).to_string());
    let h = Harness::with_routes(r, at).await;
    h.call("set_us_key", json!({"api_key": "k"})).await;
    let wl = h.call("create_watch_list", json!({"name": "US"})).await;
    for t in tickers {
        h.call(
            "add_to_watch_list",
            json!({"id": wl["id"], "symbol": format!("{t}.US")}),
        )
        .await;
    }
    // Six history downloads, then only two of six quotes fit this minute.
    h.call("refresh", Value::Null).await;
    let st = h.call("us_settings", Value::Null).await;
    assert_eq!(st["used_today"], 8, "{st}");
    // The next minute finishes the quotes.
    h.set_time(at + chrono::Duration::seconds(65));
    h.call("refresh", Value::Null).await;
    assert_eq!(h.call("us_settings", Value::Null).await["used_today"], 12);
    let quotes = h.call("quotes", Value::Null).await;
    let us = quotes
        .as_array()
        .unwrap()
        .iter()
        .filter(|q| q["symbol"].as_str().unwrap().ends_with(".US"))
        .count();
    assert_eq!(us, 6);
}

#[tokio::test]
async fn us_check_frequency_setting_shows_costs() {
    let h = Harness::new("Market Closed", lk_on(23, 20, 0)).await;
    let st = h.call("us_settings", Value::Null).await;
    assert_eq!(st["check_minutes"], 15);
    let wl = h.call("create_watch_list", json!({"name": "US"})).await;
    for i in 0..20 {
        h.call(
            "add_to_watch_list",
            json!({"id": wl["id"], "symbol": format!("T{i}.US")}),
        )
        .await;
    }
    let st = h.call("set_us_checks", json!({"minutes": 10})).await;
    assert_eq!(st["check_minutes"], 10);
    assert_eq!(st["stocks"], 20);
    let opt = |m: u64| {
        st["options"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["minutes"] == m)
            .unwrap()
            .clone()
    };
    assert_eq!(opt(30)["requests"], 300);
    assert_eq!(opt(10)["service"], true);
    assert_eq!(opt(5)["possible"], false);
    assert!(h
        .svc
        .dispatch("set_us_checks", json!({"minutes": 7}))
        .await
        .is_err());
}
