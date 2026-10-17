//! Parsing against real CSE responses captured with `cse-probe`
//! (market closed, captured 2026-10-17). Re-capture with:
//! `cargo run -p cse-client --bin cse-probe -- crates/cse-client/tests/fixtures JKH.N0000`

use cse_client::map;
use rust_decimal_macros::dec;
use serde_json::Value;

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn market_status() {
    assert_eq!(
        map::market_status(&fixture("marketStatus")).unwrap(),
        "Market Closed"
    );
}

#[test]
fn indices() {
    let aspi = map::index("ASPI", &fixture("aspiData")).unwrap();
    assert_eq!(aspi.value, dec!(20185.05));
    assert_eq!(aspi.change, dec!(213.9));
    assert_eq!(aspi.high, Some(dec!(20219.67)));
    assert_eq!(aspi.low, Some(dec!(19971.03)));
    assert!(aspi.as_of.is_some());

    let snp = map::index("S&P SL20", &fixture("snpData")).unwrap();
    assert_eq!(snp.value, dec!(5738.83));
    assert_eq!(snp.change_pct, dec!(0.7));
}

#[test]
fn market_summary_reads_turnover_and_trade_count() {
    let s = map::market_summary(&fixture("marketSummery"));
    assert_eq!(s.turnover, dec!(1143231039.7));
    assert_eq!(s.share_volume, 68_439_738);
    assert_eq!(s.trades, 15_619);
}

#[test]
fn trade_summary_parses_every_security() {
    let quotes = map::quotes(&fixture("tradeSummary")).unwrap();
    assert_eq!(quotes.len(), 279);

    let aban = quotes.iter().find(|q| q.symbol == "ABAN.N0000").unwrap();
    assert_eq!(aban.name, "ABANS ELECTRICALS PLC");
    assert_eq!(aban.last, dec!(1025.0));
    assert_eq!(aban.change, dec!(2.0));
    assert_eq!(aban.prev_close, dec!(1023.0));
    assert_eq!(aban.open, Some(dec!(1034.0)));
    assert_eq!(aban.high, Some(dec!(1034.0)));
    assert_eq!(aban.low, Some(dec!(1025.0)));
    assert_eq!(aban.share_volume, 569);
    assert_eq!(aban.trades, 20);
    assert_eq!(aban.turnover, dec!(583532.75));
    assert!(aban.last_traded_at.is_some());
}

#[test]
fn company_info_quote_uses_todays_figures() {
    let jkh = map::company_quote("JKH.N0000", &fixture("companyInfoSummery")).unwrap();
    assert_eq!(jkh.symbol, "JKH.N0000");
    assert_eq!(jkh.name, "JOHN KEELLS HOLDINGS PLC");
    assert_eq!(jkh.last, dec!(18.0));
    assert_eq!(jkh.prev_close, dec!(17.9));
    assert_eq!(jkh.high, Some(dec!(18.1)));
    assert_eq!(jkh.low, Some(dec!(17.9)));
    assert_eq!(jkh.share_volume, 1_186_254);
    assert_eq!(jkh.trades, 349);
    assert_eq!(jkh.turnover, dec!(21328106.0));
}

#[test]
fn top_gainers_and_losers() {
    let gainers = map::movers(&fixture("topGainers")).unwrap();
    assert_eq!(gainers.len(), 10);
    assert_eq!(gainers[0].symbol, "ASPH.N0000");
    assert_eq!(gainers[0].price, dec!(0.6));
    assert_eq!(gainers[0].change_pct, dec!(20.0));

    let losers = map::movers(&fixture("topLooses")).unwrap();
    assert_eq!(losers.len(), 10);
    assert!(losers.iter().all(|m| m.change <= dec!(0)));
}

#[test]
fn most_active() {
    let active = map::most_active(&fixture("mostActiveTrades")).unwrap();
    assert!(!active.is_empty());
    assert_eq!(active[0].symbol, "YORK.N0000");
    assert_eq!(active[0].share_volume, 3_130_940);
    assert_eq!(active[0].trades, 644);
    assert_eq!(active[0].turnover, dec!(22269067.2));
}

#[test]
fn quotes_carry_security_ids() {
    let quotes = map::quotes(&fixture("tradeSummary")).unwrap();
    let jkh = quotes.iter().find(|q| q.symbol == "JKH.N0000").unwrap();
    assert_eq!(jkh.security_id, Some(297));
    let info = map::company_quote("JKH.N0000", &fixture("companyInfoSummery")).unwrap();
    assert_eq!(info.security_id, Some(297));
}

#[test]
fn stock_chart_daily_and_intraday() {
    let year = map::stock_chart(&fixture("stockChart-1Y"));
    assert_eq!(year.len(), 241);
    assert!(year.windows(2).all(|w| w[0].at <= w[1].at));
    let last = year.last().unwrap();
    // Friday 16 Oct 2026 (00:00 Colombo = 15 Oct 18:30 UTC).
    assert_eq!(last.at.to_rfc3339(), "2026-10-15T18:30:00+00:00");
    assert_eq!(last.price, dec!(18.0));
    assert_eq!(last.high, Some(dec!(18.1)));
    assert_eq!(last.low, Some(dec!(17.9)));
    assert_eq!(last.volume, Some(1_186_254));

    let day = map::stock_chart(&fixture("stockChart-1D"));
    assert_eq!(day.len(), 349); // one point per trade, matching 349 trades
                                // Last trade was 18.10; the official close (18.00) is computed separately.
    assert_eq!(day.last().unwrap().price, dec!(18.1));
}

#[test]
fn aspi_chart() {
    let year = map::index_chart(&fixture("indexChart-1Y"));
    assert_eq!(year.len(), 240);
    assert_eq!(year.last().unwrap().price, dec!(20185.05));
    let day = map::index_chart(&fixture("indexChart-1D"));
    assert_eq!(day.len(), 300);
}
