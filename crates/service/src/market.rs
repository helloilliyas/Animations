//! Cached market data and the Market screen view.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use cse_client::top_by;
use cse_core::{
    ActiveStock, IndexValue, MarketPhase, MarketSummary, Quote, TopMover, TopMoverKind,
};
use rust_decimal::Decimal;
use serde::Serialize;

/// Latest data from the CSE feeds.
#[derive(Debug, Clone, Default)]
pub struct MarketData {
    pub status: Option<String>,
    pub aspi: Option<IndexValue>,
    pub snp_sl20: Option<IndexValue>,
    pub summary: Option<MarketSummary>,
    pub quotes: HashMap<String, Quote>,
    pub gainers: Vec<TopMover>,
    pub losers: Vec<TopMover>,
    pub most_active: Vec<ActiveStock>,
    pub updated_at: Option<DateTime<Utc>>,
    /// Feeds that failed on the last refresh.
    pub errors: Vec<String>,
}

/// Counts of securities up, down and unchanged ("buy sentiment").
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Breadth {
    pub positive: usize,
    pub negative: usize,
    pub neutral: usize,
}

/// Everything the Market and Top Stocks screens show.
#[derive(Debug, Clone, Serialize)]
pub struct MarketView<'a> {
    pub status: Option<&'a str>,
    pub phase: MarketPhase,
    pub aspi: Option<&'a IndexValue>,
    pub snp_sl20: Option<&'a IndexValue>,
    pub summary: Option<&'a MarketSummary>,
    pub breadth: Breadth,
    pub gainers: &'a [TopMover],
    pub losers: &'a [TopMover],
    pub top_turnover: Vec<TopMover>,
    pub top_volume: Vec<TopMover>,
    pub most_active: &'a [ActiveStock],
    pub securities: usize,
    pub updated_at: Option<DateTime<Utc>>,
    pub errors: &'a [String],
}

impl<'a> MarketView<'a> {
    pub fn new(m: &'a MarketData, phase: MarketPhase) -> Self {
        let quotes: Vec<Quote> = m.quotes.values().cloned().collect();
        let mut breadth = Breadth::default();
        for q in m.quotes.values().filter(|q| q.trades > 0) {
            match q.change.cmp(&Decimal::ZERO) {
                std::cmp::Ordering::Greater => breadth.positive += 1,
                std::cmp::Ordering::Less => breadth.negative += 1,
                std::cmp::Ordering::Equal => breadth.neutral += 1,
            }
        }
        Self {
            status: m.status.as_deref(),
            phase,
            aspi: m.aspi.as_ref(),
            snp_sl20: m.snp_sl20.as_ref(),
            summary: m.summary.as_ref(),
            breadth,
            gainers: &m.gainers,
            losers: &m.losers,
            top_turnover: top_by(TopMoverKind::Turnover, &quotes, 10),
            top_volume: top_by(TopMoverKind::ShareVolume, &quotes, 10),
            most_active: &m.most_active,
            securities: m.quotes.len(),
            updated_at: m.updated_at,
            errors: &m.errors,
        }
    }
}
