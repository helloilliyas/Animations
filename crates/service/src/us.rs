//! US stocks from Twelve Data, for watching and alerts (no paper trading).
//!
//! The user's free key allows 8 requests a minute and 800 a day, so every
//! request goes through a small budget. Each US favourite gets one daily
//! history download per New York trading day and a quote every few minutes
//! during the session (plus one after the close); quotes are fetched in
//! batches of up to 8, so 20 stocks take three minutes to refresh.

use std::collections::HashMap;

use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use cse_client::twelve::UsQuote;
use cse_core::market_hours::colombo;
use cse_core::us_market::{is_us, new_york_date, us_close_on, us_phase};
use cse_core::{ChartPoint, ChartRange, MarketPhase, Quote};
use cse_signals::Live;
use cse_storage::Store;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::alerts::{fire_structure, to_f64, FiredAlert, HISTORY_LIMIT, SYNCED_KEY};
use crate::chart::lk_day;
use crate::{parse, AppService, Result, ServiceError};

const SETTINGS_KEY: &str = "us_settings";
const QUOTES_KEY: &str = "us_quotes";
const USAGE_KEY: &str = "us_usage";
/// Free-plan limits (credits; one per symbol per request).
pub(crate) const PER_MINUTE: u32 = 8;
pub(crate) const PER_DAY: u32 = 800;
/// Left unused each day so searches still work.
const DAY_RESERVE: u32 = 20;
/// Quotes are never refreshed more often than this.
const QUOTE_EVERY_MIN: i64 = 5;
const CLOSING_CHECK_HOURS: i64 = 3;
/// Daily bars downloaded the first time (about a year and a half), then
/// just the latest days.
const FIRST_DAYS: u32 = 400;
const UPDATE_DAYS: u32 = 10;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct UsSettings {
    pub api_key: Option<String>,
    /// Background checks during US hours, in minutes (0 = off). 15 and up
    /// use Android's job scheduler; 10 and 5 keep a "US watch" service
    /// running during the session.
    pub check_minutes: u32,
}

impl Default for UsSettings {
    fn default() -> Self {
        Self {
            api_key: None,
            check_minutes: 15,
        }
    }
}

/// US background check choices, in minutes (0 = off).
pub(crate) const CHECK_CHOICES: [u32; 6] = [0, 60, 30, 15, 10, 5];
/// Regular session length.
const SESSION_MINUTES: i64 = 390;

/// What a US check frequency costs for `stocks` stocks: requests per day
/// (quotes each round, one round after the close, daily history), the
/// interval the budget actually allows, and whether it is worth offering.
pub(crate) fn check_cost(minutes: u32, stocks: usize) -> Value {
    let n = stocks.max(1) as i64;
    let budget = (PER_DAY - DAY_RESERVE) as i64 - 2 * n;
    let rounds = if minutes == 0 {
        0
    } else {
        SESSION_MINUTES / minutes as i64
    };
    let requests = (rounds + 2) * n;
    let allowed = quote_interval(SESSION_MINUTES, stocks.max(1), budget.max(0) as u32)
        .num_minutes()
        .max(minutes as i64);
    json!({
        "minutes": minutes,
        "requests": requests.min(budget + 2 * n),
        "every": allowed,
        // Not offered when the budget would stretch it by over half.
        "possible": minutes == 0 || allowed * 2 <= minutes as i64 * 3,
        "service": minutes > 0 && minutes < 15,
    })
}

/// Credits used today (UTC day, as the provider counts) and this minute.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct UsUsage {
    pub day: Option<NaiveDate>,
    pub used: u32,
    #[serde(skip)]
    pub minute_start: Option<DateTime<Utc>>,
    #[serde(skip)]
    pub minute_used: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct UsEntry {
    pub quote: Quote,
    pub exchange: Option<String>,
    pub market_open: bool,
    pub fetched_at: DateTime<Utc>,
}

#[derive(Debug, Default)]
pub(crate) struct UsState {
    pub settings: UsSettings,
    pub usage: UsUsage,
    pub quotes: HashMap<String, UsEntry>,
    pub error: Option<String>,
    /// Work left over because the minute's credits ran out.
    pub deferred: bool,
    pub intraday: HashMap<String, (DateTime<Utc>, Vec<ChartPoint>)>,
}

impl UsState {
    pub fn load(store: &Store) -> Self {
        Self {
            settings: store
                .load_json(SETTINGS_KEY)
                .ok()
                .flatten()
                .unwrap_or_default(),
            usage: store
                .load_json(USAGE_KEY)
                .ok()
                .flatten()
                .unwrap_or_default(),
            quotes: store
                .load_json(QUOTES_KEY)
                .ok()
                .flatten()
                .unwrap_or_default(),
            ..Self::default()
        }
    }
}

#[derive(Deserialize)]
struct KeyArgs {
    api_key: String,
}

#[derive(Deserialize)]
struct ChecksArgs {
    minutes: u32,
}

#[derive(Deserialize)]
struct SearchArgs {
    text: String,
}

/// How often each stock's quote can be refreshed so that `credits` last for
/// the `minutes_left` of the session: never more often than every 5
/// minutes; hourly once the budget is nearly gone.
pub(crate) fn quote_interval(minutes_left: i64, stocks: usize, credits: u32) -> Duration {
    let rounds = credits as i64 / stocks.max(1) as i64;
    if rounds == 0 {
        return Duration::hours(1);
    }
    // Rounded up, so the rounds never outrun the credits.
    Duration::minutes(((minutes_left + rounds - 1) / rounds).max(QUOTE_EVERY_MIN))
}

/// A daily point dated at 00:00 Sri Lanka time of its New York trading date.
fn day_point(q: &Quote, ny_day: NaiveDate) -> ChartPoint {
    ChartPoint {
        at: colombo()
            .from_local_datetime(&ny_day.and_hms_opt(0, 0, 0).unwrap())
            .unwrap()
            .with_timezone(&Utc),
        price: q.last,
        high: q.high,
        low: q.low,
        volume: Some(q.share_volume),
    }
}

impl AppService {
    /// Takes `n` credits if the minute's and the day's budget allow.
    fn take_credits(&self, n: u32) -> bool {
        let now = self.now();
        let usage = {
            let mut s = self.state.write().unwrap();
            let u = &mut s.us.usage;
            if u.day != Some(now.date_naive()) {
                u.day = Some(now.date_naive());
                u.used = 0;
            }
            if u.minute_start
                .is_none_or(|t| now - t >= Duration::seconds(61))
            {
                u.minute_start = Some(now);
                u.minute_used = 0;
            }
            if u.minute_used + n > PER_MINUTE || u.used + n > PER_DAY - DAY_RESERVE {
                return false;
            }
            u.minute_used += n;
            u.used += n;
            u.clone()
        };
        let _ = self.store.lock().unwrap().save_json(USAGE_KEY, &usage);
        true
    }

    fn credits_left_this_minute(&self) -> u32 {
        let now = self.now();
        let s = self.state.read().unwrap();
        let u = &s.us.usage;
        if u.minute_start
            .is_none_or(|t| now - t >= Duration::seconds(61))
        {
            PER_MINUTE
        } else {
            PER_MINUTE.saturating_sub(u.minute_used)
        }
    }

    /// Credits still usable today, keeping one round for the check after
    /// the close.
    fn credits_left_today(&self) -> u32 {
        let today = self.now().date_naive();
        let s = self.state.read().unwrap();
        let used = if s.us.usage.day == Some(today) {
            s.us.usage.used
        } else {
            0
        };
        let after_close = s.us.quotes.len() as u32;
        (PER_DAY - DAY_RESERVE).saturating_sub(used + after_close)
    }

    fn us_key(&self) -> Option<String> {
        self.state.read().unwrap().us.settings.api_key.clone()
    }

    fn note_us_error(&self, e: &cse_client::Error) {
        self.state.write().unwrap().us.error = Some(e.to_string());
    }

    /// History, analysis, quotes and alerts for US favourites; called after
    /// every refresh. Does nothing without a key or US favourites.
    pub(crate) async fn update_us(&self) -> Result<()> {
        let Some(key) = self.us_key() else {
            return Ok(());
        };
        let favs: Vec<String> = self
            .favourites()?
            .into_iter()
            .filter(|s| is_us(s))
            .collect();
        if favs.is_empty() {
            return Ok(());
        }
        let now = self.now();
        let ny_today = new_york_date(now);
        let book = self.state.read().unwrap().alerts.clone();
        let params = book.params.clone();
        let mut deferred = false;

        // 1. Daily history and analysis, once per New York trading day.
        for sym in &favs {
            let (synced, analysed) = {
                let s = self.state.read().unwrap();
                (
                    s.synced.get(sym) == Some(&ny_today),
                    s.analysis.get(sym).and_then(|a| a.day) == Some(ny_today),
                )
            };
            if analysed {
                continue;
            }
            let mut error = None;
            if !synced {
                if !self.take_credits(1) {
                    deferred = true;
                    break;
                }
                let have = self.store.lock().unwrap().daily_history(sym, None)?.len();
                let days = if have > 250 { UPDATE_DAYS } else { FIRST_DAYS };
                match self.twelve.daily(sym, days, &key).await {
                    Ok(points) => {
                        self.store.lock().unwrap().save_daily(sym, &points)?;
                        let synced = {
                            let mut s = self.state.write().unwrap();
                            s.synced.insert(sym.to_string(), ny_today);
                            s.synced.clone()
                        };
                        self.store.lock().unwrap().save_json(SYNCED_KEY, &synced)?;
                    }
                    Err(e) => {
                        self.note_us_error(&e);
                        error = Some(e.to_string());
                    }
                }
            }
            let a = self.build_analysis(sym, ny_today, &params, &book, error)?;
            self.state
                .write()
                .unwrap()
                .analysis
                .insert(sym.to_string(), a);
        }

        // 2. Quotes: every few minutes in the session, once after the close,
        //    and once for a stock that has none yet.
        let phase = us_phase(now);
        let close = us_close_on(ny_today);
        let closing = phase == MarketPhase::Closed
            && close.is_some_and(|c| now >= c && now < c + Duration::hours(CLOSING_CHECK_HOURS));
        // Spread the day's remaining credits over the rest of the session.
        let every = match close {
            Some(c) if phase == MarketPhase::Open => quote_interval(
                (c - now).num_minutes(),
                favs.len(),
                self.credits_left_today(),
            ),
            _ => Duration::minutes(QUOTE_EVERY_MIN),
        };
        let stale: Vec<String> = {
            let s = self.state.read().unwrap();
            favs.iter()
                .filter(|sym| match s.us.quotes.get(*sym) {
                    None => true,
                    Some(e) if phase == MarketPhase::Open => now - e.fetched_at >= every,
                    Some(e) => closing && close.is_some_and(|c| e.fetched_at < c),
                })
                .cloned()
                .collect()
        };
        if stale.is_empty() {
            self.state.write().unwrap().us.deferred = deferred;
            return Ok(());
        }
        let n = (self.credits_left_this_minute() as usize).min(stale.len());
        if n < stale.len() {
            deferred = true;
        }
        self.state.write().unwrap().us.deferred = deferred;
        if n == 0 || !self.take_credits(n as u32) {
            return Ok(());
        }
        let batch: Vec<&str> = stale[..n].iter().map(String::as_str).collect();
        let got: Vec<UsQuote> = match self.twelve.quotes(&batch, &key).await {
            Ok(q) => q,
            Err(e) => {
                self.note_us_error(&e);
                return Ok(());
            }
        };

        // 3. Store the quotes and today's bar; check structure alerts.
        let mut fired: Vec<FiredAlert> = Vec::new();
        let mut guard = self.state.write().unwrap();
        let s = &mut *guard;
        s.us.error = None;
        let mut book = s.alerts.clone();
        let mut today_bars = Vec::new();
        for u in got {
            let sym = u.quote.symbol.clone();
            let traded_day = u.quote.last_traded_at.map(new_york_date);
            if let Some(d) = traded_day {
                today_bars.push((sym.clone(), day_point(&u.quote, d)));
            }
            let live_now = (phase == MarketPhase::Open && u.market_open) || closing;
            if live_now && book.enabled && traded_day == Some(ny_today) {
                if let Some(st) = s.analysis.get(&sym).and_then(|a| a.structure.as_ref()) {
                    let price = to_f64(u.quote.last);
                    let live = Live {
                        price,
                        day_low: u.quote.low.map_or(price, to_f64),
                        volume: u.quote.share_volume as f64,
                        session_fraction: 1.0,
                    };
                    fire_structure(&mut book, &sym, st, &live, &params, now, &mut fired);
                }
            }
            s.us.quotes.insert(
                sym,
                UsEntry {
                    quote: u.quote,
                    exchange: u.exchange,
                    market_open: u.market_open,
                    fetched_at: now,
                },
            );
        }
        let quotes = s.us.quotes.clone();
        if !fired.is_empty() {
            book.history.extend(fired.iter().cloned());
            let excess = book.history.len().saturating_sub(HISTORY_LIMIT);
            book.history.drain(..excess);
            s.alerts = book.clone();
        }
        drop(guard);
        {
            let mut store = self.store.lock().unwrap();
            store.save_json(QUOTES_KEY, &quotes)?;
            for (sym, p) in &today_bars {
                store.save_daily(sym, std::slice::from_ref(p))?;
            }
        }
        if !fired.is_empty() {
            self.publish_alerts(&book, &fired)?;
        }
        Ok(())
    }

    /// Whether the last US update left work for the next minute.
    pub(crate) fn us_deferred(&self) -> bool {
        self.state.read().unwrap().us.deferred
    }

    /// Faster refreshes while US favourites are trading.
    pub(crate) fn us_active(&self) -> bool {
        let s = self.state.read().unwrap();
        s.us.settings.api_key.is_some()
            && !s.us.quotes.is_empty()
            && us_phase(self.now()) == MarketPhase::Open
    }

    pub(crate) fn us_command(&self, cmd: &str, args: Value) -> Option<Result<Value>> {
        let r = match cmd {
            "us_settings" => Ok(self.us_settings()),
            "set_us_checks" => parse::<ChecksArgs>(args).and_then(|a| {
                if !CHECK_CHOICES.contains(&a.minutes) {
                    return Err(ServiceError::BadArgs(format!(
                        "US checks can be every {CHECK_CHOICES:?} minutes"
                    )));
                }
                let settings = UsSettings {
                    check_minutes: a.minutes,
                    ..self.state.read().unwrap().us.settings.clone()
                };
                self.store
                    .lock()
                    .unwrap()
                    .save_json(SETTINGS_KEY, &settings)?;
                self.state.write().unwrap().us.settings = settings;
                Ok(self.us_settings())
            }),
            "set_us_key" => parse::<KeyArgs>(args).and_then(|a| {
                let key = a.api_key.trim().to_owned();
                let settings = UsSettings {
                    api_key: (!key.is_empty()).then_some(key),
                    ..self.state.read().unwrap().us.settings.clone()
                };
                self.store
                    .lock()
                    .unwrap()
                    .save_json(SETTINGS_KEY, &settings)?;
                let mut s = self.state.write().unwrap();
                s.us.settings = settings;
                s.us.error = None;
                drop(s);
                Ok(self.us_settings())
            }),
            _ => return None,
        };
        Some(r)
    }

    fn us_settings(&self) -> Value {
        let stocks = self
            .favourites()
            .map(|f| f.iter().filter(|s| is_us(s)).count())
            .unwrap_or(0);
        let s = self.state.read().unwrap();
        let today = self.now().date_naive();
        let used = if s.us.usage.day == Some(today) {
            s.us.usage.used
        } else {
            0
        };
        let key = s.us.settings.api_key.as_deref();
        json!({
            "has_key": key.is_some(),
            "key_hint": key.map(|k| format!("…{}", &k[k.len().saturating_sub(4)..])),
            "used_today": used,
            "day_limit": PER_DAY,
            "per_minute": PER_MINUTE,
            "error": s.us.error,
            "stocks": stocks,
            "market_open": us_phase(self.now()) == MarketPhase::Open,
            "check_minutes": s.us.settings.check_minutes,
            "options": CHECK_CHOICES.iter().map(|m| check_cost(*m, stocks)).collect::<Vec<_>>(),
        })
    }

    /// The `us_search` command: US stocks and ETFs matching some text.
    pub(crate) async fn us_search(&self, args: Value) -> Result<Value> {
        let a: SearchArgs = parse(args)?;
        let key = self
            .us_key()
            .ok_or_else(|| ServiceError::BadArgs("Add your Twelve Data API key first".into()))?;
        if !self.take_credits(1) {
            return Err(ServiceError::BadArgs(
                "Twelve Data's limit for this minute is used up; try again shortly".into(),
            ));
        }
        let hits = self.twelve.search(a.text.trim(), &key).await?;
        Ok(json!(hits))
    }

    /// Charts for US stocks: daily ranges from stored history, 1D from
    /// 5-minute bars (fetched at most every 5 minutes).
    pub(crate) async fn us_chart(&self, sym: &str, range: ChartRange) -> Result<Value> {
        let now = self.now();
        let mut error = None;
        let points = if range == ChartRange::Day {
            let cached = self
                .state
                .read()
                .unwrap()
                .us
                .intraday
                .get(sym)
                .filter(|(at, _)| now - *at < Duration::minutes(5))
                .map(|(_, p)| p.clone());
            match (cached, self.us_key()) {
                (Some(p), _) => p,
                (None, Some(key)) if self.take_credits(1) => {
                    match self.twelve.intraday(sym, &key).await {
                        Ok(p) => {
                            self.state
                                .write()
                                .unwrap()
                                .us
                                .intraday
                                .insert(sym.to_owned(), (now, p.clone()));
                            p
                        }
                        Err(e) => {
                            error = Some(e.to_string());
                            Vec::new()
                        }
                    }
                }
                (None, None) => {
                    error = Some("Add your Twelve Data API key in Settings".into());
                    Vec::new()
                }
                _ => {
                    error = Some("Twelve Data's limit for this minute is used up".into());
                    Vec::new()
                }
            }
        } else {
            let today = lk_day(now);
            let since = range.days().map(|d| today - Duration::days(d));
            self.store.lock().unwrap().daily_history(sym, since)?
        };
        let stored = self.store.lock().unwrap().daily_history(sym, None)?;
        let (change, change_pct) = match (points.first(), points.last()) {
            (Some(f), Some(l)) if !f.price.is_zero() => {
                let c = l.price - f.price;
                (c, (c * Decimal::ONE_HUNDRED / f.price).round_dp(2))
            }
            _ => (Decimal::ZERO, Decimal::ZERO),
        };
        Ok(json!({
            "key": sym,
            "range": range,
            "source": "twelvedata",
            "error": error,
            "points": points,
            "change": change,
            "change_pct": change_pct,
            "history_days": stored.len(),
            "history_since": stored.first().map(|p| lk_day(p.at)),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_spread_over_the_session() {
        // Full session, 20 stocks, a fresh day: about every 11 minutes.
        assert_eq!(quote_interval(390, 20, 760), Duration::minutes(11));
        // Few stocks: the 5-minute floor.
        assert_eq!(quote_interval(390, 5, 760), Duration::minutes(5));
        // Late in the session with credits left: faster again.
        assert_eq!(quote_interval(60, 20, 400), Duration::minutes(5));
        // Budget nearly spent: slow down, never stop the session check.
        assert_eq!(quote_interval(200, 20, 40), Duration::minutes(100));
        assert_eq!(quote_interval(200, 20, 10), Duration::hours(1));
        // What the settings screen offers for 20 stocks.
        let c = |m| check_cost(m, 20);
        assert_eq!(c(15)["requests"], (26 + 2) * 20);
        assert_eq!(c(15)["possible"], true);
        assert_eq!(c(15)["service"], false);
        assert_eq!(c(10)["possible"], true);
        assert_eq!(c(10)["every"], 11);
        assert_eq!(c(10)["service"], true);
        assert_eq!(c(5)["possible"], false);
        // Few stocks: every 5 minutes fits.
        assert_eq!(check_cost(5, 8)["possible"], true);
        assert_eq!(check_cost(5, 8)["every"], 5);
        // Whole-session cost stays within the plan.
        assert!(390 / 11 * 20 <= (PER_DAY - DAY_RESERVE - 20) as i64);
    }
}
