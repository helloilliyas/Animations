//! Price charts: intraday and up to one year from the CSE, plus a local daily
//! history that keeps every day ever seen, so "All" grows beyond a year.

use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use cse_core::market_hours::colombo;
use cse_core::{ChartPoint, ChartRange, MarketPhase, Quote};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{parse, AppService, Result, State};

/// Key under which ASPI history is stored.
pub const ASPI_KEY: &str = "ASPI";

#[derive(Deserialize)]
pub(crate) struct ChartArgs {
    /// A security symbol; omit for the ASPI index.
    symbol: Option<String>,
    range: ChartRange,
}

pub(crate) fn lk_day(at: DateTime<Utc>) -> NaiveDate {
    at.with_timezone(&colombo()).date_naive()
}

fn lk_midnight(day: NaiveDate) -> DateTime<Utc> {
    colombo()
        .from_local_datetime(&day.and_hms_opt(0, 0, 0).unwrap())
        .unwrap()
        .with_timezone(&Utc)
}

fn quote_point(q: &Quote, day: NaiveDate) -> ChartPoint {
    ChartPoint {
        at: lk_midnight(day),
        price: q.last,
        high: q.high,
        low: q.low,
        volume: Some(q.share_volume),
    }
}

impl AppService {
    /// After a session closes, stores that day's close for every security
    /// that traded, and ASPI, once per day. This is what lets history keep
    /// growing for every stock without fetching each one.
    pub(crate) fn save_session_closes(
        &self,
        s: &mut State,
        store: &mut cse_storage::Store,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let today = lk_day(now);
        let cal = &s.account.settings.calendar;
        let after_close = cal.close_on(today).is_some_and(|close| now >= close);
        if s.last_daily_save == Some(today)
            || !after_close
            || cal.phase_at(now) != MarketPhase::Closed
        {
            return Ok(());
        }
        let traded: Vec<&Quote> = s
            .market
            .quotes
            .values()
            .filter(|q| q.last_traded_at.is_some_and(|t| lk_day(t) == today))
            .collect();
        if traded.is_empty() {
            return Ok(());
        }
        for q in traded {
            store.save_daily(&q.symbol, &[quote_point(q, today)])?;
        }
        if let Some(aspi) = s
            .market
            .aspi
            .as_ref()
            .filter(|i| i.as_of.is_some_and(|t| lk_day(t) == today))
        {
            store.save_daily(
                ASPI_KEY,
                &[ChartPoint {
                    at: lk_midnight(today),
                    price: aspi.value,
                    high: aspi.high,
                    low: aspi.low,
                    volume: None,
                }],
            )?;
        }
        s.last_daily_save = Some(today);
        Ok(())
    }

    /// The `chart` command.
    pub(crate) async fn chart(&self, args: Value) -> Result<Value> {
        let a: ChartArgs = parse(args)?;
        let key = a.symbol.clone().unwrap_or_else(|| ASPI_KEY.to_owned());
        let now = self.now();
        let today = lk_day(now);
        let mut error: Option<String> = None;

        let (points, source) = if a.range == ChartRange::Day {
            let fetched = match &a.symbol {
                None => self.client.aspi_chart(ChartRange::Day).await,
                Some(sym) => match self.security_id(sym).await {
                    Ok(id) => self.client.stock_chart(id, ChartRange::Day).await,
                    Err(e) => Err(e),
                },
            };
            match fetched {
                Ok(p) if !p.is_empty() => (p, "cse"),
                other => {
                    if let Err(e) = other {
                        error = Some(e.to_string());
                    }
                    (self.local_intraday(&key, a.symbol.is_none(), now)?, "local")
                }
            }
        } else {
            let synced = self.state.read().unwrap().synced.get(&key) == Some(&today);
            if !synced {
                match self.fetch_year(a.symbol.as_deref()).await {
                    Ok(p) => {
                        self.store.lock().unwrap().save_daily(&key, &p)?;
                        self.state
                            .write()
                            .unwrap()
                            .synced
                            .insert(key.clone(), today);
                    }
                    Err(e) => error = Some(e.to_string()),
                }
            }
            let since = a.range.days().map(|d| today - Duration::days(d));
            let mut points = self.store.lock().unwrap().daily_history(&key, since)?;
            self.append_live_point(&mut points, a.symbol.as_deref(), today);
            (points, "history")
        };

        let stored = self.store.lock().unwrap().daily_history(&key, None)?;
        let (change, change_pct) = match (points.first(), points.last()) {
            (Some(f), Some(l)) if !f.price.is_zero() => {
                let c = l.price - f.price;
                (c, (c * Decimal::ONE_HUNDRED / f.price).round_dp(2))
            }
            _ => (Decimal::ZERO, Decimal::ZERO),
        };
        Ok(json!({
            "key": key,
            "range": a.range,
            "source": source,
            "error": error,
            "points": points,
            "change": change,
            "change_pct": change_pct,
            "history_days": stored.len(),
            "history_since": stored.first().map(|p| lk_day(p.at)),
        }))
    }

    async fn fetch_year(&self, symbol: Option<&str>) -> Result<Vec<ChartPoint>> {
        Ok(match symbol {
            None => self.client.aspi_chart(ChartRange::Year).await?,
            Some(sym) => {
                let id = self.security_id(sym).await?;
                self.client.stock_chart(id, ChartRange::Year).await?
            }
        })
    }

    async fn security_id(&self, symbol: &str) -> std::result::Result<u64, cse_client::Error> {
        let cached = self
            .state
            .read()
            .unwrap()
            .market
            .quotes
            .get(symbol)
            .and_then(|q| q.security_id);
        if let Some(id) = cached {
            return Ok(id);
        }
        let q = self.client.quote(symbol).await?;
        q.security_id
            .ok_or_else(|| cse_client::Error::MissingField("security id".into()))
    }

    /// Today's prices the app recorded itself, when the CSE feed is down.
    fn local_intraday(
        &self,
        key: &str,
        is_index: bool,
        now: DateTime<Utc>,
    ) -> Result<Vec<ChartPoint>> {
        let start = lk_midnight(lk_day(now));
        let store = self.store.lock().unwrap();
        Ok(if is_index {
            store
                .index_history(key, start, now + Duration::seconds(1))?
                .into_iter()
                .map(|(at, price)| ChartPoint {
                    at,
                    price,
                    high: None,
                    low: None,
                    volume: None,
                })
                .collect()
        } else {
            store
                .price_history(key, start, now + Duration::seconds(1))?
                .into_iter()
                .map(|p| ChartPoint {
                    at: p.at,
                    price: p.price,
                    high: None,
                    low: None,
                    volume: Some(p.share_volume),
                })
                .collect()
        })
    }

    /// Adds today's live price when daily history does not include today yet.
    fn append_live_point(
        &self,
        points: &mut Vec<ChartPoint>,
        symbol: Option<&str>,
        today: NaiveDate,
    ) {
        if points.last().is_some_and(|p| lk_day(p.at) >= today) {
            return;
        }
        let s = self.state.read().unwrap();
        let live = match symbol {
            Some(sym) => s
                .market
                .quotes
                .get(sym)
                .filter(|q| q.last_traded_at.is_some_and(|t| lk_day(t) == today))
                .map(|q| quote_point(q, today)),
            None => s
                .market
                .aspi
                .as_ref()
                .filter(|i| i.as_of.is_some_and(|t| lk_day(t) == today))
                .map(|i| ChartPoint {
                    at: lk_midnight(today),
                    price: i.value,
                    high: i.high,
                    low: i.low,
                    volume: None,
                }),
        };
        points.extend(live);
    }
}
