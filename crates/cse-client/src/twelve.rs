//! US stock prices from Twelve Data (<https://twelvedata.com>), using the
//! user's own API key. Every request costs credits (one per symbol); the
//! caller keeps within the plan's per-minute and per-day limits.
//!
//! Replies use strings for numbers; errors come back as
//! `{"status": "error", "code": 429, "message": "..."}` in the body.

use std::time::Duration;

use chrono::{NaiveDate, NaiveDateTime, TimeZone, Utc};
use cse_core::market_hours::colombo;
use cse_core::us_market::{from_new_york, ticker, US_SUFFIX};
use cse_core::{ChartPoint, Quote};
use rust_decimal::Decimal;
use serde_json::Value;

use crate::parse::{count, count_opt, decimal, decimal_or_zero, list, string};
use crate::Error;

pub const DEFAULT_BASE_URL: &str = "https://api.twelvedata.com";

/// A US quote plus what the app needs beyond the common [`Quote`].
#[derive(Debug, Clone, PartialEq)]
pub struct UsQuote {
    pub quote: Quote,
    pub exchange: Option<String>,
    pub market_open: bool,
}

/// A symbol search result.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SearchHit {
    /// App symbol, e.g. `AAPL.US`.
    pub symbol: String,
    pub ticker: String,
    pub name: String,
    pub exchange: String,
    pub kind: String,
}

#[derive(Debug, Clone)]
pub struct TwelveData {
    http: reqwest::Client,
    base_url: String,
}

impl Default for TwelveData {
    fn default() -> Self {
        Self::new(DEFAULT_BASE_URL)
    }
}

impl TwelveData {
    pub fn new(base_url: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .user_agent(concat!("cse-trader/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("static reqwest configuration is valid");
        Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_owned(),
        }
    }

    async fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<Value, Error> {
        let url = format!("{}/{path}", self.base_url);
        let resp = self.http.get(&url).query(query).send().await?;
        let status = resp.status();
        let body = resp.bytes().await?;
        let v: Value = serde_json::from_slice(&body).map_err(|e| {
            if status.is_success() {
                Error::Json(e)
            } else {
                Error::Status(status.as_u16())
            }
        })?;
        if let Some(err) = provider_error(&v) {
            return Err(err);
        }
        if !status.is_success() {
            return Err(Error::Status(status.as_u16()));
        }
        Ok(v)
    }

    /// Latest quotes for up to a handful of app symbols (`AAPL.US`); costs
    /// one credit per symbol. Symbols the provider rejects are left out.
    pub async fn quotes(&self, symbols: &[&str], key: &str) -> Result<Vec<UsQuote>, Error> {
        if symbols.is_empty() {
            return Ok(Vec::new());
        }
        let tickers: Vec<&str> = symbols.iter().map(|s| ticker(s)).collect();
        let v = self
            .get("quote", &[("symbol", &tickers.join(",")), ("apikey", key)])
            .await?;
        Ok(map_quotes(&v, &tickers))
    }

    /// Daily bars, oldest first, each dated at 00:00 Sri Lanka time of its
    /// New York trading date (the app's daily-history convention).
    pub async fn daily(
        &self,
        symbol: &str,
        days: u32,
        key: &str,
    ) -> Result<Vec<ChartPoint>, Error> {
        let n = days.to_string();
        let v = self
            .get(
                "time_series",
                &[
                    ("symbol", ticker(symbol)),
                    ("interval", "1day"),
                    ("outputsize", &n),
                    ("apikey", key),
                ],
            )
            .await?;
        Ok(map_daily(&v))
    }

    /// Today's 5-minute bars, oldest first, at their real times.
    pub async fn intraday(&self, symbol: &str, key: &str) -> Result<Vec<ChartPoint>, Error> {
        let v = self
            .get(
                "time_series",
                &[
                    ("symbol", ticker(symbol)),
                    ("interval", "5min"),
                    ("outputsize", "80"),
                    ("apikey", key),
                ],
            )
            .await?;
        Ok(map_intraday(&v))
    }

    /// US stocks and ETFs matching `text`.
    pub async fn search(&self, text: &str, key: &str) -> Result<Vec<SearchHit>, Error> {
        let v = self
            .get(
                "symbol_search",
                &[("symbol", text), ("outputsize", "120"), ("apikey", key)],
            )
            .await?;
        Ok(map_search(&v, text))
    }
}

fn provider_error(v: &Value) -> Option<Error> {
    (v.get("status").and_then(Value::as_str) == Some("error")).then(|| Error::Provider {
        code: count(v, &["code"]) as u16,
        message: string(v, &["message"]).unwrap_or_else(|| "request failed".into()),
    })
}

fn map_quote(v: &Value, fallback_ticker: &str) -> Option<UsQuote> {
    if provider_error(v).is_some() {
        return None;
    }
    let t = string(v, &["symbol"]).unwrap_or_else(|| fallback_ticker.to_owned());
    let last = decimal(v, &["close", "price"])?;
    let volume = count(v, &["volume"]);
    Some(UsQuote {
        quote: Quote {
            symbol: format!("{t}{US_SUFFIX}"),
            name: string(v, &["name"]).unwrap_or_else(|| t.clone()),
            last,
            change: decimal_or_zero(v, &["change"]),
            change_pct: decimal_or_zero(v, &["percent_change"]),
            prev_close: decimal_or_zero(v, &["previous_close"]),
            open: decimal(v, &["open"]),
            high: decimal(v, &["high"]),
            low: decimal(v, &["low"]),
            share_volume: volume,
            trades: 0,
            turnover: last * Decimal::from(volume),
            last_traded_at: count_opt(v, &["last_quote_at", "timestamp"])
                .and_then(|s| Utc.timestamp_opt(s as i64, 0).single()),
            security_id: None,
        },
        exchange: string(v, &["exchange"]),
        market_open: v
            .get("is_market_open")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// One symbol: the quote object itself. Several: an object keyed by ticker.
pub fn map_quotes(v: &Value, tickers: &[&str]) -> Vec<UsQuote> {
    if tickers.len() == 1 && v.get("symbol").is_some() {
        return map_quote(v, tickers[0]).into_iter().collect();
    }
    tickers
        .iter()
        .filter_map(|t| v.get(*t).and_then(|q| map_quote(q, t)))
        .collect()
}

fn bar(r: &Value, at: chrono::DateTime<Utc>) -> Option<ChartPoint> {
    Some(ChartPoint {
        at,
        price: decimal(r, &["close"])?,
        high: decimal(r, &["high"]),
        low: decimal(r, &["low"]),
        volume: count_opt(r, &["volume"]),
    })
}

pub fn map_daily(v: &Value) -> Vec<ChartPoint> {
    let mut out: Vec<ChartPoint> = list(v, &["values"])
        .iter()
        .filter_map(|r| {
            let d = NaiveDate::parse_from_str(&string(r, &["datetime"])?[..10], "%Y-%m-%d").ok()?;
            let at = colombo()
                .from_local_datetime(&d.and_hms_opt(0, 0, 0)?)
                .single()?
                .with_timezone(&Utc);
            bar(r, at)
        })
        .collect();
    out.sort_by_key(|p| p.at);
    out
}

pub fn map_intraday(v: &Value) -> Vec<ChartPoint> {
    let mut out: Vec<ChartPoint> = list(v, &["values"])
        .iter()
        .filter_map(|r| {
            let dt = NaiveDateTime::parse_from_str(&string(r, &["datetime"])?, "%Y-%m-%d %H:%M:%S")
                .ok()?;
            bar(r, from_new_york(dt))
        })
        .collect();
    out.sort_by_key(|p| p.at);
    // Only the latest session.
    if let Some(last) = out.last().map(|p| cse_core::us_market::new_york_date(p.at)) {
        out.retain(|p| cse_core::us_market::new_york_date(p.at) == last);
    }
    out
}

/// Kinds of security worth watching (not structured notes or warrants).
const SEARCH_KINDS: [&str; 4] = ["Common Stock", "ETF", "Depositary Receipt", "REIT"];

/// US stocks, ETFs, ADRs and REITs, one per ticker (the main exchange
/// rather than IEX), exact ticker matches first, then stocks before funds;
/// at most 20.
pub fn map_search(v: &Value, text: &str) -> Vec<SearchHit> {
    let mut hits: Vec<SearchHit> = Vec::new();
    for r in list(v, &["data"]) {
        let country = string(r, &["country"]).unwrap_or_default();
        let kind = string(r, &["instrument_type", "type"]).unwrap_or_default();
        if !(country.is_empty() || country == "United States")
            || !SEARCH_KINDS.contains(&kind.as_str())
        {
            continue;
        }
        let Some(t) = string(r, &["symbol"]) else {
            continue;
        };
        let hit = SearchHit {
            symbol: format!("{t}{US_SUFFIX}"),
            name: string(r, &["instrument_name", "name"]).unwrap_or_else(|| t.clone()),
            exchange: string(r, &["exchange"]).unwrap_or_default(),
            kind,
            ticker: t,
        };
        match hits.iter_mut().find(|h| h.ticker == hit.ticker) {
            Some(h) if h.exchange == "IEX" => *h = hit,
            Some(_) => {}
            None => hits.push(hit),
        }
    }
    let q = text.trim().to_ascii_uppercase();
    hits.sort_by_key(|h| (h.ticker != q, h.kind != "Common Stock"));
    hits.truncate(20);
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;
    use serde_json::json;

    fn aapl() -> Value {
        json!({"symbol": "AAPL", "name": "Apple Inc.", "exchange": "NASDAQ", "currency": "USD",
               "datetime": "2026-10-22", "timestamp": 1792699200, "last_quote_at": 1792699199,
               "open": "250.10", "high": "253.00", "low": "249.50", "close": "252.40",
               "volume": "41234567", "previous_close": "250.00", "change": "2.40",
               "percent_change": "0.96", "is_market_open": true})
    }

    #[test]
    fn single_and_batch_quotes() {
        let one = map_quotes(&aapl(), &["AAPL"]);
        assert_eq!(one.len(), 1);
        let q = &one[0];
        assert_eq!(q.quote.symbol, "AAPL.US");
        assert_eq!(q.quote.last, dec!(252.40));
        assert_eq!(q.quote.change_pct, dec!(0.96));
        assert_eq!(q.quote.prev_close, dec!(250.00));
        assert_eq!(q.quote.share_volume, 41_234_567);
        assert!(q.market_open);
        assert_eq!(q.exchange.as_deref(), Some("NASDAQ"));
        assert_eq!(q.quote.last_traded_at.unwrap().timestamp(), 1792699199);

        let batch = json!({"AAPL": aapl(),
            "XXXX": {"code": 400, "message": "symbol not found", "status": "error"}});
        let qs = map_quotes(&batch, &["AAPL", "XXXX"]);
        assert_eq!(qs.len(), 1);
    }

    #[test]
    fn errors_in_the_body() {
        let v = json!({"code": 429, "message": "You have run out of API credits for the current minute.", "status": "error"});
        match provider_error(&v) {
            Some(Error::Provider { code, message }) => {
                assert_eq!(code, 429);
                assert!(message.contains("credits"));
            }
            other => panic!("{other:?}"),
        }
        assert!(provider_error(&aapl()).is_none());
    }

    #[test]
    fn daily_bars_oldest_first_on_sri_lanka_midnight() {
        let v = json!({"meta": {"symbol": "AAPL", "interval": "1day",
                       "exchange_timezone": "America/New_York"},
            "values": [
                {"datetime": "2026-10-22", "open": "250", "high": "253", "low": "249.5", "close": "252.4", "volume": "41234567"},
                {"datetime": "2026-10-21", "open": "248", "high": "251", "low": "247", "close": "250", "volume": "39000000"}
            ], "status": "ok"});
        let bars = map_daily(&v);
        assert_eq!(bars.len(), 2);
        assert_eq!(bars[0].price, dec!(250));
        assert_eq!(bars[1].high, Some(dec!(253)));
        // 2026-10-22 00:00 +05:30
        assert_eq!(bars[1].at.to_rfc3339(), "2026-10-21T18:30:00+00:00");
    }

    #[test]
    fn intraday_in_new_york_time_latest_session_only() {
        let v = json!({"values": [
            {"datetime": "2026-10-22 15:55:00", "open": "252", "high": "252.6", "low": "251.9", "close": "252.4", "volume": "900000"},
            {"datetime": "2026-10-22 09:30:00", "open": "250", "high": "250.5", "low": "249.8", "close": "250.2", "volume": "1500000"},
            {"datetime": "2026-10-21 15:55:00", "open": "249", "high": "250", "low": "248.8", "close": "250", "volume": "800000"}
        ], "status": "ok"});
        let bars = map_intraday(&v);
        assert_eq!(bars.len(), 2);
        // 09:30 New York (UTC-4) = 13:30 UTC.
        assert_eq!(bars[0].at.to_rfc3339(), "2026-10-22T13:30:00+00:00");
    }

    #[test]
    fn search_keeps_us_listings() {
        let v = json!({"data": [
            {"symbol": "AAPL", "instrument_name": "Apple Inc", "exchange": "NASDAQ", "instrument_type": "Common Stock", "country": "United States", "currency": "USD"},
            {"symbol": "AAPL", "instrument_name": "Apple Inc", "exchange": "BMV", "instrument_type": "Common Stock", "country": "Mexico", "currency": "MXN"}
        ], "status": "ok"});
        let hits = map_search(&v, "aapl");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].symbol, "AAPL.US");
        assert_eq!(hits[0].exchange, "NASDAQ");
        // Company names: one listing per ticker, main exchange, stocks first.
        let v = json!({"data": [
            {"symbol": "MSFY", "instrument_name": "Kurv Microsoft ETF", "exchange": "IEX", "instrument_type": "ETF", "country": "United States"},
            {"symbol": "MSFT", "instrument_name": "Microsoft Corporation", "exchange": "IEX", "instrument_type": "Common Stock", "country": "United States"},
            {"symbol": "MSFT", "instrument_name": "Microsoft Corporation", "exchange": "NASDAQ", "instrument_type": "Common Stock", "country": "United States"},
            {"symbol": "4MSFT", "instrument_name": "MICROSOFT", "exchange": "MTA", "instrument_type": "Common Stock", "country": "Italy"},
            {"symbol": "BDTMQXX", "instrument_name": "BMO AutoCallable Barrier Note", "exchange": "NASDAQ", "instrument_type": "Structured Product", "country": "United States"}
        ], "status": "ok"});
        let hits = map_search(&v, "Microsoft");
        assert_eq!(hits.len(), 2);
        assert_eq!(
            (hits[0].ticker.as_str(), hits[0].exchange.as_str()),
            ("MSFT", "NASDAQ")
        );
        assert_eq!(hits[1].ticker, "MSFY");
    }
}
