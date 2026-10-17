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
                let (status, body) = match routes.get(path) {
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

struct Harness {
    svc: AppService,
    clock: Arc<Mutex<DateTime<Utc>>>,
}

impl Harness {
    async fn new(status: &str, at: DateTime<Utc>) -> Self {
        let base = stub(routes(status)).await;
        let clock = Arc::new(Mutex::new(at));
        let c = clock.clone();
        let svc = AppService::with(
            Store::open_in_memory().unwrap(),
            CseClient::new(base).with_retries(0),
            Arc::new(move || *c.lock().unwrap()),
        )
        .unwrap();
        Self { svc, clock }
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
