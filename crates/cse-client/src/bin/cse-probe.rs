//! Fetches every CSE feed once, saves the raw JSON, and reports whether our
//! parsers understand it. Use it to capture real fixtures and to diagnose
//! changes on the CSE side.
//!
//! Usage: cse-probe [OUT_DIR] [SYMBOL]
//! Defaults: crates/cse-client/tests/fixtures, JKH.N0000

use std::path::PathBuf;

use cse_client::{map, CseClient, Endpoint};

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let out = PathBuf::from(
        args.next()
            .unwrap_or_else(|| "crates/cse-client/tests/fixtures".into()),
    );
    let symbol = args.next().unwrap_or_else(|| "JKH.N0000".into());
    std::fs::create_dir_all(&out).expect("create output directory");

    let client = CseClient::default();
    let mut failures = 0;
    for endpoint in Endpoint::ALL {
        let form: Vec<(&str, &str)> = match endpoint {
            Endpoint::CompanyInfo => vec![("symbol", symbol.as_str())],
            // JKH.N0000, one year of daily prices.
            Endpoint::StockChart => vec![("stockId", "297"), ("period", "5")],
            Endpoint::IndexChart => vec![("chartId", "1"), ("period", "5")],
            _ => vec![],
        };
        let raw = match client.raw(endpoint, &form).await {
            Ok(v) => v,
            Err(e) => {
                failures += 1;
                println!("{:<20} FETCH FAILED: {e}", endpoint.path());
                continue;
            }
        };
        let path = out.join(format!("{}.json", endpoint.path()));
        std::fs::write(&path, serde_json::to_string_pretty(&raw).unwrap()).expect("write fixture");

        let parsed =
            match endpoint {
                Endpoint::MarketStatus => map::market_status(&raw).map(|s| s.to_string()),
                Endpoint::Aspi | Endpoint::SnpSl20 => map::index(endpoint.path(), &raw)
                    .map(|i| format!("{} ({:+})", i.value, i.change)),
                Endpoint::MarketSummary => Ok(format!("{:?}", map::market_summary(&raw))),
                Endpoint::TradeSummary => map::quotes(&raw).map(|q| format!("{} quotes", q.len())),
                Endpoint::TopGainers | Endpoint::TopLosers => {
                    map::movers(&raw).map(|m| format!("{} rows", m.len()))
                }
                Endpoint::MostActive => map::most_active(&raw).map(|m| format!("{} rows", m.len())),
                Endpoint::CompanyInfo => map::company_quote(&symbol, &raw)
                    .map(|q| format!("{} last {}", q.symbol, q.last)),
                Endpoint::StockChart => Ok(format!("{} points", map::stock_chart(&raw).len())),
                Endpoint::IndexChart => Ok(format!("{} points", map::index_chart(&raw).len())),
            };
        match parsed {
            Ok(summary) => println!("{:<20} ok    {summary}", endpoint.path()),
            Err(e) => {
                failures += 1;
                println!("{:<20} PARSE FAILED: {e}", endpoint.path());
            }
        }
    }
    println!("\nRaw responses saved in {}", out.display());
    if failures > 0 {
        std::process::exit(1);
    }
}
