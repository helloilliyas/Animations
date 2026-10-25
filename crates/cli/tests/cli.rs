//! The CLI against an in-memory database (no network needed).

use std::sync::Arc;

use clap::Parser;
use cse_cli::{run, symbol, Cli, COMMANDS};
use cse_client::CseClient;
use cse_service::AppService;
use cse_storage::Store;
use serde_json::{json, Value};

fn service() -> AppService {
    // Nothing listens on port 9: network commands fail fast.
    AppService::with(
        Store::open_in_memory().unwrap(),
        CseClient::new("http://127.0.0.1:9/api").with_retries(0),
        Arc::new(chrono::Utc::now),
    )
    .unwrap()
}

async fn cse(svc: &AppService, args: &[&str]) -> Result<Value, String> {
    let cli = Cli::try_parse_from(std::iter::once("cse").chain(args.iter().copied()))
        .map_err(|e| e.to_string())?;
    run(svc, &cli).await
}

#[test]
fn symbols_get_the_normal_share_class() {
    assert_eq!(symbol("jkh"), "JKH.N0000");
    assert_eq!(symbol("ACL.X0000"), "ACL.X0000");
    assert_eq!(symbol("aapl.us"), "AAPL.US");
    assert_eq!(symbol("aspi"), "ASPI");
}

#[tokio::test]
async fn watch_lists_by_name() {
    let svc = service();
    cse(&svc, &["list-create", "Shariah"]).await.unwrap();
    let list = cse(&svc, &["list-add", "shariah", "wata", "DIPD", "ACL.X0000"])
        .await
        .unwrap();
    assert_eq!(
        list["symbols"],
        json!(["WATA.N0000", "DIPD.N0000", "ACL.X0000"])
    );
    let list = cse(&svc, &["list-remove", "Shariah", "dipd"])
        .await
        .unwrap();
    assert_eq!(list["symbols"], json!(["WATA.N0000", "ACL.X0000"]));
    assert!(cse(&svc, &["list-add", "nope", "JKH"])
        .await
        .unwrap_err()
        .contains("no watch list"));
}

#[tokio::test]
async fn real_trades_round_trip() {
    let svc = service();
    cse(
        &svc,
        &[
            "real-add",
            "wata",
            "buy",
            "1000",
            "44.10",
            "--date",
            "2026-10-20",
        ],
    )
    .await
    .unwrap();
    cse(
        &svc,
        &[
            "real-add",
            "wata",
            "sell",
            "400",
            "46.50",
            "--date",
            "2026-10-22",
            "--fees",
            "200",
        ],
    )
    .await
    .unwrap();
    let csv = cse(&svc, &["real-export"]).await.unwrap();
    assert!(csv
        .as_str()
        .unwrap()
        .contains("2026-10-22,WATA.N0000,SELL,400,46.50,200"));
    // Selling more than held is refused, as in the app.
    let err = cse(
        &svc,
        &[
            "real-add",
            "wata",
            "sell",
            "5000",
            "46",
            "--date",
            "2026-10-23",
        ],
    )
    .await
    .unwrap_err();
    assert!(!err.is_empty());
}

#[tokio::test]
async fn every_listed_command_exists() {
    let svc = service();
    for cmd in COMMANDS {
        if let Err(e) = cse(&svc, &["call", cmd]).await {
            assert!(!e.contains("unknown command"), "{cmd}: {e}");
        }
    }
    let err = cse(&svc, &["call", "nope"]).await.unwrap_err();
    assert!(err.contains("unknown command"));
    let err = cse(&svc, &["call", "quote", "{not json"])
        .await
        .unwrap_err();
    assert!(err.contains("bad JSON"));
}

#[test]
fn floats_are_tidied() {
    let v = cse_cli::tidy(json!({"entry": 48.480000000000004, "qty": 100, "s": "1.5"}));
    assert_eq!(v, json!({"entry": 48.48, "qty": 100, "s": "1.5"}));
}
