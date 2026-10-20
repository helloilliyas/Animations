//! Price alerts for favourite stocks: signal levels from the signals engine,
//! the user's own price levels, firing on live prices, history, and an
//! outbox file that the Android background service turns into
//! notifications.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;
use std::path::PathBuf;

use chrono::{DateTime, Duration, NaiveDate, Utc};
use cse_core::{ChartRange, MarketPhase};
use cse_signals::{
    daily_levels, evaluate_daily, evaluate_intraday, track_record, Bar, DailyLevels,
    IntradayLevels, Live, Params, SignalKind, TrackRecord, Trigger,
};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::chart::{lk_day, lk_midnight};
use crate::{parse, AppService, Result};

pub(crate) const ALERTS_KEY: &str = "alerts";
const HISTORY_LIMIT: usize = 200;
/// Daily histories fetched per refresh, to spread requests out.
const SYNCS_PER_REFRESH: usize = 4;
/// Minutes between intraday trade fetches per stock.
const INTRADAY_EVERY_MIN: i64 = 5;

/// A price level the user set by hand. Fires once, then switches off.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomAlert {
    pub id: u64,
    pub symbol: String,
    /// True: alert when price rises to or above; false: falls to or below.
    pub above: bool,
    pub price: Decimal,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FiredAlert {
    pub id: u64,
    pub at: DateTime<Utc>,
    pub symbol: String,
    /// Signal name, or "Custom".
    pub kind: String,
    pub level: f64,
    pub price: f64,
    pub message: String,
}

/// Saved alert settings and history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AlertBook {
    pub enabled: bool,
    pub signals: BTreeMap<SignalKind, bool>,
    pub params: Params,
    /// The user's background-mode choice (the Android service follows it).
    pub background: bool,
    pub custom: Vec<CustomAlert>,
    pub history: Vec<FiredAlert>,
    /// "SYMBOL|Kind" -> last time it fired.
    pub last_fired: BTreeMap<String, DateTime<Utc>>,
    pub next_id: u64,
}

impl Default for AlertBook {
    fn default() -> Self {
        Self {
            enabled: true,
            signals: SignalKind::DAILY
                .iter()
                .chain(SignalKind::INTRADAY.iter())
                .map(|k| (*k, true))
                .collect(),
            params: Params::default(),
            background: false,
            custom: Vec::new(),
            history: Vec::new(),
            last_fired: BTreeMap::new(),
            next_id: 1,
        }
    }
}

impl AlertBook {
    fn signal_on(&self, kind: SignalKind) -> bool {
        self.signals.get(&kind).copied().unwrap_or(true)
    }

    fn take_id(&mut self) -> u64 {
        let id = self.next_id.max(1);
        self.next_id = id + 1;
        id
    }
}

/// Per-stock analysis, recomputed once a day (intraday part more often).
#[derive(Debug, Clone, Default)]
pub(crate) struct Analysis {
    day: Option<NaiveDate>,
    levels: Option<DailyLevels>,
    records: Vec<TrackRecord>,
    history_days: usize,
    intraday: Option<IntradayLevels>,
    intraday_at: Option<DateTime<Utc>>,
    intraday_triggers: Vec<Trigger>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct ConfigArgs {
    enabled: Option<bool>,
    background: Option<bool>,
    signals: Option<BTreeMap<SignalKind, bool>>,
    params: Option<Params>,
}

#[derive(Deserialize)]
struct CustomArgs {
    symbol: String,
    above: bool,
    price: Decimal,
}

#[derive(Deserialize)]
struct IdArg {
    id: u64,
}

#[derive(Deserialize)]
struct AfterArg {
    #[serde(default)]
    after_id: u64,
}

fn to_f64(d: Decimal) -> f64 {
    d.to_f64().unwrap_or(0.0)
}

impl AppService {
    fn favourites(&self) -> Result<BTreeSet<String>> {
        Ok(self
            .store
            .lock()
            .unwrap()
            .watch_lists()?
            .into_iter()
            .flat_map(|w| w.symbols)
            .collect())
    }

    fn save_book(&self, book: &AlertBook) -> Result<()> {
        self.store.lock().unwrap().save_json(ALERTS_KEY, book)?;
        Ok(())
    }

    /// Daily bars for a symbol, excluding `today` (still forming).
    fn daily_bars(&self, symbol: &str, today: NaiveDate) -> Result<Vec<Bar>> {
        let points = self.store.lock().unwrap().daily_history(symbol, None)?;
        Ok(points
            .into_iter()
            .filter(|p| lk_day(p.at) < today)
            .map(|p| {
                let close = to_f64(p.price);
                Bar {
                    time: p.at.timestamp(),
                    close,
                    high: p.high.map_or(close, to_f64),
                    low: p.low.map_or(close, to_f64),
                    volume: p.volume.unwrap_or(0) as f64,
                }
            })
            .collect())
    }

    /// Called after every market refresh.
    pub(crate) async fn update_alerts(&self) -> Result<()> {
        let book = self.state.read().unwrap().alerts.clone();
        if !book.enabled {
            return Ok(());
        }
        let favourites = self.favourites()?;
        let now = self.now();
        let today = lk_day(now);
        let params = book.params.clone();

        // 1. Daily levels and track records, once a day per stock.
        let stale: Vec<String> = {
            let s = self.state.read().unwrap();
            favourites
                .iter()
                .filter(|sym| s.analysis.get(*sym).and_then(|a| a.day) != Some(today))
                .take(SYNCS_PER_REFRESH)
                .cloned()
                .collect()
        };
        for sym in stale {
            let mut a = Analysis {
                day: Some(today),
                ..Analysis::default()
            };
            let synced = self.state.read().unwrap().synced.get(&sym) == Some(&today);
            if !synced {
                match self.fetch_year(Some(&sym)).await {
                    Ok(points) => {
                        self.store.lock().unwrap().save_daily(&sym, &points)?;
                        self.state
                            .write()
                            .unwrap()
                            .synced
                            .insert(sym.clone(), today);
                    }
                    Err(e) => a.error = Some(e.to_string()),
                }
            }
            let bars = self.daily_bars(&sym, today)?;
            a.history_days = bars.len();
            a.levels = daily_levels(&bars, &params);
            a.records = track_record(&bars, &params);
            if a.levels.is_none() && a.error.is_none() {
                a.error = Some(format!("Not enough history yet ({} days)", bars.len()));
            }
            self.state.write().unwrap().analysis.insert(sym, a);
        }

        // 2. Live checks only while the exchange is trading.
        let (phase, exchange_open, session) = {
            let s = self.state.read().unwrap();
            let cal = &s.account.settings.calendar;
            let open = lk_midnight(today) + (cal.open - chrono::NaiveTime::MIN);
            let close = lk_midnight(today) + (cal.close - chrono::NaiveTime::MIN);
            let ex_open = !s
                .market
                .status
                .as_deref()
                .is_some_and(|t| t.to_ascii_lowercase().contains("close"));
            (cal.phase_at(now), ex_open, (open, close))
        };
        if phase != MarketPhase::Open || !exchange_open {
            return Ok(());
        }
        let (open, close) = session;
        let fraction = ((now - open).num_seconds() as f64 / (close - open).num_seconds() as f64)
            .clamp(0.0, 1.0);

        // Intraday trades, refreshed every few minutes per stock.
        let intraday_on = SignalKind::INTRADAY.iter().any(|k| book.signal_on(*k));
        if intraday_on {
            for sym in &favourites {
                let due = {
                    let s = self.state.read().unwrap();
                    s.analysis.get(sym).is_some_and(|a| {
                        a.levels.is_some()
                            && a.intraday_at
                                .is_none_or(|t| now - t >= Duration::minutes(INTRADAY_EVERY_MIN))
                    })
                };
                if !due {
                    continue;
                }
                let trades = match self.security_id(sym).await {
                    Ok(id) => self.client.stock_chart(id, ChartRange::Day).await,
                    Err(e) => Err(e),
                };
                let mut s = self.state.write().unwrap();
                let Some(a) = s.analysis.get_mut(sym) else {
                    continue;
                };
                a.intraday_at = Some(now);
                if let (Ok(points), Some(levels)) = (trades, a.levels.as_ref()) {
                    let t: Vec<(i64, f64, f64)> = points
                        .iter()
                        .filter(|p| lk_day(p.at) == today)
                        .map(|p| {
                            (
                                p.at.timestamp(),
                                to_f64(p.price),
                                p.volume.unwrap_or(0) as f64,
                            )
                        })
                        .collect();
                    let (lv, trig) =
                        evaluate_intraday(&t, open.timestamp(), now.timestamp(), levels, &params);
                    a.intraday = Some(lv);
                    a.intraday_triggers = trig;
                }
            }
        }

        // 3. Evaluate and fire.
        let mut fired: Vec<FiredAlert> = Vec::new();
        let mut guard = self.state.write().unwrap();
        let s = &mut *guard;
        let mut book = s.alerts.clone();
        for sym in &favourites {
            let Some(q) = s.market.quotes.get(sym) else {
                continue;
            };
            let price = to_f64(q.last);
            let traded_today = q.last_traded_at.is_some_and(|t| lk_day(t) == today);
            if !traded_today {
                continue;
            }
            let mut triggers: Vec<Trigger> = Vec::new();
            if let Some(a) = s.analysis.get(sym) {
                if let Some(levels) = &a.levels {
                    let live = Live {
                        price,
                        day_low: q.low.map_or(price, to_f64),
                        volume: q.share_volume as f64,
                        session_fraction: fraction,
                    };
                    triggers.extend(evaluate_daily(levels, &live, &params));
                }
                triggers.extend(a.intraday_triggers.iter().cloned());
            }
            triggers.retain(|t| book.signal_on(t.kind));
            for t in triggers {
                let key = format!("{sym}|{:?}", t.kind);
                // Intraday signals once per day; daily ones respect the cooldown.
                let wait = if t.kind.is_intraday() {
                    Duration::hours(12)
                } else {
                    Duration::days(params.cooldown_days as i64)
                };
                if book
                    .last_fired
                    .get(&key)
                    .is_some_and(|last| now - *last < wait)
                {
                    continue;
                }
                book.last_fired.insert(key, now);
                let id = book.take_id();
                fired.push(FiredAlert {
                    id,
                    at: now,
                    symbol: sym.clone(),
                    kind: t.kind.label().to_owned(),
                    level: t.level,
                    price,
                    message: t.message,
                });
            }
        }
        // Custom levels: any stock with a quote, favourite or not.
        for c in book.custom.iter_mut().filter(|c| c.active) {
            let Some(q) = s.market.quotes.get(&c.symbol) else {
                continue;
            };
            let hit = if c.above {
                q.last >= c.price
            } else {
                q.last <= c.price
            };
            if hit {
                c.active = false;
                fired.push(FiredAlert {
                    id: 0,
                    at: now,
                    symbol: c.symbol.clone(),
                    kind: "Custom".into(),
                    level: to_f64(c.price),
                    price: to_f64(q.last),
                    message: format!(
                        "Price {} {} your level {}",
                        q.last,
                        if c.above {
                            "rose to or above"
                        } else {
                            "fell to or below"
                        },
                        c.price
                    ),
                });
            }
        }
        for f in fired.iter_mut().filter(|f| f.id == 0) {
            f.id = book.take_id();
        }
        if fired.is_empty() {
            return Ok(());
        }
        book.history.extend(fired.iter().cloned());
        let excess = book.history.len().saturating_sub(HISTORY_LIMIT);
        book.history.drain(..excess);
        s.alerts = book.clone();
        drop(guard);
        self.save_book(&book)?;
        self.write_outbox(&fired);
        Ok(())
    }

    /// Appends fired alerts to the outbox file read by the Android service.
    fn write_outbox(&self, fired: &[FiredAlert]) {
        let Some(path) = &self.outbox else { return };
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            for a in fired {
                let line = json!({
                    "id": a.id,
                    "title": format!("{} · {}", a.symbol, a.kind),
                    "body": a.message,
                    "symbol": a.symbol,
                });
                let _ = writeln!(f, "{line}");
            }
        }
    }

    /// Alert commands; `None` if `cmd` is not an alerts command.
    pub(crate) fn alerts_command(&self, cmd: &str, args: Value) -> Option<Result<Value>> {
        let r = match cmd {
            "alerts" => self.alerts_view(),
            "update_alert_config" => (|| {
                let a: ConfigArgs = parse(args)?;
                let book = {
                    let mut s = self.state.write().unwrap();
                    let b = &mut s.alerts;
                    if let Some(v) = a.enabled {
                        b.enabled = v;
                    }
                    if let Some(v) = a.background {
                        b.background = v;
                    }
                    if let Some(v) = a.signals {
                        b.signals.extend(v);
                    }
                    if let Some(v) = a.params {
                        b.params = v;
                        // New thresholds: recompute levels.
                        s.analysis.clear();
                    }
                    s.alerts.clone()
                };
                self.save_book(&book)?;
                Ok(Value::Null)
            })(),
            "add_custom_alert" => (|| {
                let a: CustomArgs = parse(args)?;
                let book = {
                    let mut s = self.state.write().unwrap();
                    let id = s.alerts.take_id();
                    s.alerts.custom.push(CustomAlert {
                        id,
                        symbol: a.symbol.trim().to_ascii_uppercase(),
                        above: a.above,
                        price: a.price,
                        active: true,
                    });
                    s.alerts.clone()
                };
                self.save_book(&book)?;
                Ok(Value::Null)
            })(),
            "remove_custom_alert" => (|| {
                let a: IdArg = parse(args)?;
                let book = {
                    let mut s = self.state.write().unwrap();
                    s.alerts.custom.retain(|c| c.id != a.id);
                    s.alerts.clone()
                };
                self.save_book(&book)?;
                Ok(Value::Null)
            })(),
            "clear_alert_history" => (|| {
                let book = {
                    let mut s = self.state.write().unwrap();
                    s.alerts.history.clear();
                    s.alerts.clone()
                };
                self.save_book(&book)?;
                Ok(Value::Null)
            })(),
            "alerts_pending" => (|| {
                let a: AfterArg = parse(args)?;
                let s = self.state.read().unwrap();
                let latest = s.alerts.history.last().map_or(0, |f| f.id);
                let new: Vec<&FiredAlert> = s
                    .alerts
                    .history
                    .iter()
                    .filter(|f| f.id > a.after_id)
                    .collect();
                Ok(json!({ "latest_id": latest, "alerts": new }))
            })(),
            "outbox_path" => Ok(json!(self.outbox.as_ref().map(|p| p.display().to_string()))),
            _ => return None,
        };
        Some(r)
    }

    fn alerts_view(&self) -> Result<Value> {
        let favourites = self.favourites()?;
        let s = self.state.read().unwrap();
        let b = &s.alerts;
        let rows: Vec<Value> = favourites
            .iter()
            .map(|sym| {
                let q = s.market.quotes.get(sym);
                let a = s.analysis.get(sym);
                json!({
                    "symbol": sym,
                    "name": q.map(|q| q.name.clone()),
                    "price": q.map(|q| q.last),
                    "change_pct": q.map(|q| q.change_pct),
                    "levels": a.and_then(|a| a.levels.as_ref()),
                    "records": a.map(|a| &a.records),
                    "history_days": a.map(|a| a.history_days),
                    "intraday": a.and_then(|a| a.intraday.as_ref()),
                    "pending": a.is_none(),
                    "error": a.and_then(|a| a.error.as_ref()),
                })
            })
            .collect();
        let mut history: Vec<&FiredAlert> = b.history.iter().rev().take(50).collect();
        history.sort_by(|x, y| y.at.cmp(&x.at));
        let labels: HashMap<String, &str> = SignalKind::DAILY
            .iter()
            .chain(SignalKind::INTRADAY.iter())
            .map(|k| (format!("{k:?}"), k.label()))
            .collect();
        Ok(json!({
            "enabled": b.enabled,
            "background": b.background,
            "signals": b.signals,
            "labels": labels,
            "params": b.params,
            "favourites": rows,
            "custom": b.custom,
            "history": history,
        }))
    }
}

/// Default location of the outbox next to the database.
pub(crate) fn outbox_beside(db_path: &std::path::Path) -> PathBuf {
    db_path.parent().map_or_else(
        || PathBuf::from("alerts-outbox.jsonl"),
        |d| d.join("alerts-outbox.jsonl"),
    )
}
