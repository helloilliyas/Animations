//! Market data models, independent of any data source's wire format.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Latest trading figures for one security, as shown in a watch list or quote.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Quote {
    /// CSE symbol, e.g. `JKH.N0000`.
    pub symbol: String,
    pub name: String,
    /// Last traded price.
    pub last: Decimal,
    pub change: Decimal,
    pub change_pct: Decimal,
    pub prev_close: Decimal,
    pub open: Option<Decimal>,
    pub high: Option<Decimal>,
    pub low: Option<Decimal>,
    /// Number of shares traded today.
    pub share_volume: u64,
    /// Number of trades today.
    pub trades: u64,
    pub turnover: Decimal,
    pub last_traded_at: Option<DateTime<Utc>>,
    /// CSE's numeric security id, needed to request price history.
    #[serde(default)]
    pub security_id: Option<u64>,
}

/// A market index such as ASPI or S&P SL20.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexValue {
    pub name: String,
    pub value: Decimal,
    pub change: Decimal,
    pub change_pct: Decimal,
    pub high: Option<Decimal>,
    pub low: Option<Decimal>,
    pub as_of: Option<DateTime<Utc>>,
}

/// Market-wide totals for the day.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MarketSummary {
    pub turnover: Decimal,
    pub share_volume: u64,
    pub trades: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TopMoverKind {
    Gainers,
    Losers,
    Turnover,
    ShareVolume,
}

/// One row of a Top Stocks list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TopMover {
    pub symbol: String,
    pub price: Decimal,
    pub change: Decimal,
    pub change_pct: Decimal,
}

/// A point on a price chart: one trade (intraday) or one day's close.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChartPoint {
    pub at: DateTime<Utc>,
    /// Trade price, closing price or index value.
    pub price: Decimal,
    pub high: Option<Decimal>,
    pub low: Option<Decimal>,
    /// Shares traded (per trade intraday, per day for daily points).
    pub volume: Option<u64>,
}

/// Chart time ranges the CSE serves (and "All" from local history).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChartRange {
    #[serde(rename = "1D")]
    Day,
    #[serde(rename = "1W")]
    Week,
    #[serde(rename = "1M")]
    Month,
    #[serde(rename = "3M")]
    Quarter,
    #[serde(rename = "1Y")]
    Year,
    #[serde(rename = "All")]
    All,
}

impl ChartRange {
    /// Calendar days covered by a daily range; `None` for intraday or all.
    pub fn days(self) -> Option<i64> {
        match self {
            ChartRange::Day | ChartRange::All => None,
            ChartRange::Week => Some(7),
            ChartRange::Month => Some(31),
            ChartRange::Quarter => Some(92),
            ChartRange::Year => Some(366),
        }
    }
}

/// One row of the most-active list (ranked by number of trades).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActiveStock {
    pub symbol: String,
    pub share_volume: u64,
    pub trades: u64,
    pub turnover: Decimal,
}

/// A CSE company announcement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Announcement {
    pub id: String,
    pub symbol: Option<String>,
    pub company: Option<String>,
    pub title: String,
    pub category: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
    pub url: Option<String>,
}
