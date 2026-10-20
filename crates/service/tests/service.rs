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
                // Intraday chart requests (period=1) get their own fixture.
                let intraday = format!("{path}#1D");
                let key = if req.contains("period=1") && routes.contains_key(intraday.as_str()) {
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
            CseClient::new(base).with_retries(0),
            Arc::new(move || *c.lock().unwrap()),
        )
        .unwrap()
        .with_outbox(&outbox);
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

    // The outbox holds one line per alert for the Android service.
    let outbox = std::fs::read_to_string(&h.outbox).unwrap();
    assert_eq!(outbox.lines().count(), alerts.len());
    assert!(outbox.contains("JKH.N0000 · Breakout"));

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
