//! Client for the Colombo Stock Exchange's public market data.
//!
//! Data comes from the JSON feeds behind the public `cse.lk` website. They are
//! undocumented and may change without notice, so responses are parsed
//! leniently (see [`parse`]) and raw access is available for diagnostics via
//! [`CseClient::raw`].

pub mod parse;

use std::time::Duration;

use cse_core::{
    ActiveStock, IndexValue, MarketPhase, MarketSummary, Quote, TopMover, TopMoverKind,
};
use serde_json::Value;

pub const DEFAULT_BASE_URL: &str = "https://www.cse.lk/api/";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("CSE returned HTTP {0}")]
    Status(u16),
    #[error("invalid JSON from CSE: {0}")]
    Json(#[from] serde_json::Error),
    #[error("missing field (tried: {0})")]
    MissingField(String),
}

/// A CSE data feed. Each is an HTTP POST to `<base>/<path>` with form fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    MarketStatus,
    Aspi,
    SnpSl20,
    MarketSummary,
    /// Prices for every listed security.
    TradeSummary,
    TopGainers,
    TopLosers,
    MostActive,
    /// Detail for one security; needs a `symbol` form field.
    CompanyInfo,
}

impl Endpoint {
    pub const ALL: [Endpoint; 9] = [
        Endpoint::MarketStatus,
        Endpoint::Aspi,
        Endpoint::SnpSl20,
        Endpoint::MarketSummary,
        Endpoint::TradeSummary,
        Endpoint::TopGainers,
        Endpoint::TopLosers,
        Endpoint::MostActive,
        Endpoint::CompanyInfo,
    ];

    /// Path on the CSE site; the misspellings are the site's own.
    pub fn path(self) -> &'static str {
        match self {
            Endpoint::MarketStatus => "marketStatus",
            Endpoint::Aspi => "aspiData",
            Endpoint::SnpSl20 => "snpData",
            Endpoint::MarketSummary => "marketSummery",
            Endpoint::TradeSummary => "tradeSummary",
            Endpoint::TopGainers => "topGainers",
            Endpoint::TopLosers => "topLooses",
            Endpoint::MostActive => "mostActiveTrades",
            Endpoint::CompanyInfo => "companyInfoSummery",
        }
    }
}

/// How often to refresh for a given market phase.
pub fn poll_interval(phase: MarketPhase) -> Duration {
    match phase {
        MarketPhase::Open => Duration::from_secs(5),
        MarketPhase::PreOpen => Duration::from_secs(15),
        MarketPhase::Closed => Duration::from_secs(300),
    }
}

#[derive(Debug, Clone)]
pub struct CseClient {
    http: reqwest::Client,
    base_url: String,
    retries: u32,
}

impl Default for CseClient {
    fn default() -> Self {
        Self::new(DEFAULT_BASE_URL)
    }
}

impl CseClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        let mut base_url = base_url.into();
        if !base_url.ends_with('/') {
            base_url.push('/');
        }
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent(concat!("cse-trader/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("static reqwest configuration is valid");
        Self {
            http,
            base_url,
            retries: 2,
        }
    }

    /// Number of extra attempts after a network error or HTTP 5xx.
    pub fn with_retries(mut self, retries: u32) -> Self {
        self.retries = retries;
        self
    }

    /// Unparsed JSON from an endpoint.
    pub async fn raw(&self, endpoint: Endpoint, form: &[(&str, &str)]) -> Result<Value, Error> {
        let url = format!("{}{}", self.base_url, endpoint.path());
        let mut attempt = 0;
        loop {
            match self.post_once(&url, form).await {
                Err(e) if attempt < self.retries && is_transient(&e) => {
                    attempt += 1;
                    tokio::time::sleep(Duration::from_millis(500 * 2u64.pow(attempt))).await;
                }
                result => return result,
            }
        }
    }

    async fn post_once(&self, url: &str, form: &[(&str, &str)]) -> Result<Value, Error> {
        let resp = self.http.post(url).form(form).send().await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(Error::Status(status.as_u16()));
        }
        let body = resp.bytes().await?;
        Ok(serde_json::from_slice(&body)?)
    }

    /// The site's market status text, e.g. "Market Open".
    pub async fn market_status(&self) -> Result<String, Error> {
        map::market_status(&self.raw(Endpoint::MarketStatus, &[]).await?)
    }

    pub async fn aspi(&self) -> Result<IndexValue, Error> {
        map::index("ASPI", &self.raw(Endpoint::Aspi, &[]).await?)
    }

    pub async fn snp_sl20(&self) -> Result<IndexValue, Error> {
        map::index("S&P SL20", &self.raw(Endpoint::SnpSl20, &[]).await?)
    }

    pub async fn market_summary(&self) -> Result<MarketSummary, Error> {
        Ok(map::market_summary(
            &self.raw(Endpoint::MarketSummary, &[]).await?,
        ))
    }

    /// Latest quote for every listed security.
    pub async fn all_quotes(&self) -> Result<Vec<Quote>, Error> {
        map::quotes(&self.raw(Endpoint::TradeSummary, &[]).await?)
    }

    pub async fn quote(&self, symbol: &str) -> Result<Quote, Error> {
        let v = self
            .raw(Endpoint::CompanyInfo, &[("symbol", symbol)])
            .await?;
        map::company_quote(symbol, &v)
    }

    /// Gainers, losers or most active. Turnover and share-volume rankings are
    /// derived from [`CseClient::all_quotes`] instead (see [`top_by`]).
    /// Securities with the most trades today.
    pub async fn most_active(&self) -> Result<Vec<ActiveStock>, Error> {
        map::most_active(&self.raw(Endpoint::MostActive, &[]).await?)
    }

    pub async fn top(&self, kind: TopMoverKind) -> Result<Vec<TopMover>, Error> {
        let endpoint = match kind {
            TopMoverKind::Gainers => Endpoint::TopGainers,
            TopMoverKind::Losers => Endpoint::TopLosers,
            TopMoverKind::Turnover | TopMoverKind::ShareVolume => {
                return Ok(top_by(kind, &self.all_quotes().await?, 10));
            }
        };
        map::movers(&self.raw(endpoint, &[]).await?)
    }
}

fn is_transient(e: &Error) -> bool {
    match e {
        Error::Http(e) => e.is_timeout() || e.is_connect() || e.is_request(),
        Error::Status(code) => *code >= 500 || *code == 429,
        _ => false,
    }
}

/// Ranks quotes locally, as ATrad's Top Stocks tabs do.
pub fn top_by(kind: TopMoverKind, quotes: &[Quote], n: usize) -> Vec<TopMover> {
    let mut sorted: Vec<&Quote> = quotes.iter().collect();
    match kind {
        TopMoverKind::Gainers => sorted.sort_by(|a, b| b.change_pct.cmp(&a.change_pct)),
        TopMoverKind::Losers => sorted.sort_by(|a, b| a.change_pct.cmp(&b.change_pct)),
        TopMoverKind::Turnover => sorted.sort_by(|a, b| b.turnover.cmp(&a.turnover)),
        TopMoverKind::ShareVolume => sorted.sort_by(|a, b| b.share_volume.cmp(&a.share_volume)),
    }
    sorted
        .into_iter()
        .take(n)
        .map(|q| TopMover {
            symbol: q.symbol.clone(),
            price: q.last,
            change: q.change,
            change_pct: q.change_pct,
        })
        .collect()
}

/// Conversion from CSE JSON to core models.
pub mod map {
    use super::*;
    use crate::parse::*;

    const SYMBOL: &[&str] = &["symbol", "securityCode", "sym"];
    const NAME: &[&str] = &["name", "companyName", "securityName"];
    const PRICE: &[&str] = &["price", "lastTradedPrice", "lastPrice", "closingPrice"];
    const CHANGE: &[&str] = &["change", "priceChange"];
    const CHANGE_PCT: &[&str] = &[
        "percentageChange",
        "changePercentage",
        "changePercent",
        "percentage",
    ];

    pub fn market_status(v: &Value) -> Result<String, Error> {
        req_string(v, &["status", "marketStatus"])
    }

    pub fn index(name: &str, v: &Value) -> Result<IndexValue, Error> {
        Ok(IndexValue {
            name: name.to_owned(),
            value: req_decimal(v, &["value", "indexValue", "price"])?,
            change: decimal_or_zero(v, CHANGE),
            change_pct: decimal_or_zero(v, CHANGE_PCT),
            high: decimal(v, &["highValue", "high"]),
            low: decimal(v, &["lowValue", "low"]),
            as_of: timestamp(v, &["timestamp", "lastUpdated", "time"]),
        })
    }

    pub fn market_summary(v: &Value) -> MarketSummary {
        let v = object(v, &["reqMarketSummery", "marketSummary"]);
        MarketSummary {
            // In this feed `tradeVolume` is the rupee turnover, not a count.
            turnover: decimal_or_zero(v, &["turnover", "tradeValue", "tradeVolume"]),
            share_volume: count(v, &["shareVolume", "sharevolume", "volume"]),
            trades: count(v, &["trades", "noOfTrades"]),
        }
    }

    pub fn quote(v: &Value) -> Result<Quote, Error> {
        let last = req_decimal(v, PRICE)?;
        let prev_close = decimal(v, &["previousClose", "prevClose", "previousClosePrice"]);
        let change = decimal(v, CHANGE)
            .or_else(|| prev_close.map(|p| last - p))
            .unwrap_or_default();
        Ok(Quote {
            symbol: req_string(v, SYMBOL)?,
            name: string(v, NAME).unwrap_or_default(),
            last,
            change,
            change_pct: decimal_or_zero(v, CHANGE_PCT),
            prev_close: prev_close.unwrap_or(last - change),
            open: decimal(v, &["open", "openPrice"]),
            high: decimal(v, &["high", "highPrice", "hiTrade"]),
            low: decimal(v, &["low", "lowPrice", "lowTrade"]),
            share_volume: count(v, &["sharevolume", "shareVolume", "tdyShareVolume"]),
            trades: count(v, &["tradevolume", "tradeVolume", "tdyTradeVolume"]),
            turnover: decimal_or_zero(v, &["turnover", "tdyTurnover"]),
            last_traded_at: timestamp(v, &["lastTradedTime", "lastTradedDate", "time"]),
        })
    }

    /// Rows that fail to parse are skipped rather than failing the whole list.
    pub fn quotes(v: &Value) -> Result<Vec<Quote>, Error> {
        let rows = list(v, &["reqTradeSummery", "tradeSummary", "data"]);
        let quotes: Vec<Quote> = rows.iter().filter_map(|r| quote(r).ok()).collect();
        if quotes.is_empty() && !rows.is_empty() {
            // Every row failed: surface the reason from the first one.
            quote(&rows[0])?;
        }
        Ok(quotes)
    }

    pub fn company_quote(symbol: &str, v: &Value) -> Result<Quote, Error> {
        let info = object(v, &["reqSymbolInfo", "symbolInfo", "data"]);
        let mut q = quote(info).or_else(|_| {
            // Some responses omit the symbol inside the info block.
            let mut with_symbol = info.clone();
            if let Value::Object(m) = &mut with_symbol {
                m.insert("symbol".into(), Value::String(symbol.to_owned()));
            }
            quote(&with_symbol)
        })?;
        if q.symbol.is_empty() {
            q.symbol = symbol.to_owned();
        }
        Ok(q)
    }

    /// Most active securities (by trades). This feed carries no prices.
    pub fn most_active(v: &Value) -> Result<Vec<ActiveStock>, Error> {
        list(v, &["data"])
            .iter()
            .map(|r| {
                Ok(ActiveStock {
                    symbol: req_string(r, SYMBOL)?,
                    share_volume: count(r, &["shareVolume", "sharevolume"]),
                    trades: count(r, &["tradeVolume", "tradevolume", "trades"]),
                    turnover: decimal_or_zero(r, &["turnover"]),
                })
            })
            .collect()
    }

    pub fn movers(v: &Value) -> Result<Vec<TopMover>, Error> {
        list(v, &["data"])
            .iter()
            .map(|r| {
                Ok(TopMover {
                    symbol: req_string(r, SYMBOL)?,
                    price: req_decimal(r, PRICE)?,
                    change: decimal_or_zero(r, CHANGE),
                    change_pct: decimal_or_zero(r, CHANGE_PCT),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn q(
        symbol: &str,
        pct: rust_decimal::Decimal,
        turnover: rust_decimal::Decimal,
        vol: u64,
    ) -> Quote {
        Quote {
            symbol: symbol.into(),
            name: String::new(),
            last: dec!(10),
            change: dec!(0),
            change_pct: pct,
            prev_close: dec!(10),
            open: None,
            high: None,
            low: None,
            share_volume: vol,
            trades: 0,
            turnover,
            last_traded_at: None,
        }
    }

    #[test]
    fn top_by_ranks_locally() {
        let quotes = vec![
            q("A", dec!(5), dec!(100), 1),
            q("B", dec!(-3), dec!(900), 50),
            q("C", dec!(1), dec!(500), 20),
        ];
        let sym = |k| -> Vec<String> {
            top_by(k, &quotes, 2)
                .into_iter()
                .map(|m| m.symbol)
                .collect()
        };
        assert_eq!(sym(TopMoverKind::Gainers), ["A", "C"]);
        assert_eq!(sym(TopMoverKind::Losers), ["B", "C"]);
        assert_eq!(sym(TopMoverKind::Turnover), ["B", "C"]);
        assert_eq!(sym(TopMoverKind::ShareVolume), ["B", "C"]);
    }

    #[test]
    fn poll_faster_when_open() {
        assert!(poll_interval(MarketPhase::Open) < poll_interval(MarketPhase::PreOpen));
        assert!(poll_interval(MarketPhase::PreOpen) < poll_interval(MarketPhase::Closed));
    }

    #[test]
    fn base_url_gets_trailing_slash() {
        assert_eq!(CseClient::new("http://x/api").base_url, "http://x/api/");
    }
}
