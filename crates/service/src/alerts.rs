//! Price alerts for favourite stocks: signal levels from the signals engine,
//! the user's own price levels, firing on live prices, history, and an
//! outbox file that the Android background service turns into
//! notifications.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;
use std::path::PathBuf;

use chrono::{DateTime, Duration, NaiveDate, Utc};
use cse_core::{ChartRange, MarketPhase};
use cse_signals::structure::{self, Break, Status, Structure};
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
/// Day each symbol last downloaded its one-year history (kept between runs
/// so scheduled checks do not re-download it).
pub(crate) const SYNCED_KEY: &str = "synced_days";
/// Alert settings format; v2 made market structure the default method.
const BOOK_VERSION: u32 = 2;
const HISTORY_LIMIT: usize = 200;
/// Daily histories fetched per refresh, to spread requests out.
const SYNCS_PER_REFRESH: usize = 10;
const UPTREND_BROKEN: &str = "Uptrend broken";
/// How long after the close the closing check of structure setups runs.
const CLOSING_CHECK_HOURS: i64 = 3;
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
    /// Entry, stop and target for market-structure entries; their results
    /// are tracked from later daily prices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<AlertPlan>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AlertPlan {
    pub entry: f64,
    pub stop: f64,
    pub target: f64,
}

/// How prices are checked when the app is closed (Android).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BackgroundMode {
    #[default]
    #[serde(rename = "off")]
    Off,
    #[serde(rename = "60")]
    Every60,
    #[serde(rename = "30")]
    Every30,
    #[serde(rename = "15")]
    Every15,
    #[serde(rename = "live")]
    Live,
}

/// Saved alert settings and history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AlertBook {
    pub enabled: bool,
    pub signals: BTreeMap<SignalKind, bool>,
    pub params: Params,
    /// Live background service running (the Android service follows it).
    pub background: bool,
    pub background_mode: BackgroundMode,
    /// Also notify when a stock comes within reach of a trigger.
    pub notify_get_ready: bool,
    /// Notify when a favourite's uptrend breaks (price below the last higher low).
    pub notify_broken: bool,
    /// Missing in settings saved before v2, so those load as 0 and migrate.
    #[serde(default)]
    pub version: u32,
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
            signals: default_signals(),
            params: Params::default(),
            background: false,
            background_mode: BackgroundMode::Every30,
            notify_get_ready: false,
            notify_broken: false,
            version: BOOK_VERSION,
            custom: Vec::new(),
            history: Vec::new(),
            last_fired: BTreeMap::new(),
            next_id: 1,
        }
    }
}

/// Structure entries on; the earlier indicator signals and "downtrend
/// broken" off.
fn default_signals() -> BTreeMap<SignalKind, bool> {
    SignalKind::STRUCTURE
        .iter()
        .map(|k| (*k, *k != SignalKind::DowntrendBroken))
        .chain(
            SignalKind::DAILY
                .iter()
                .chain(SignalKind::INTRADAY.iter())
                .map(|k| (*k, false)),
        )
        .collect()
}

impl AlertBook {
    /// Upgrades settings saved by older versions.
    pub(crate) fn migrate(&mut self) -> bool {
        if self.version >= BOOK_VERSION {
            return false;
        }
        self.signals = default_signals();
        self.background_mode = if self.background {
            BackgroundMode::Live
        } else {
            BackgroundMode::Every30
        };
        self.version = BOOK_VERSION;
        true
    }

    fn enabled_structure(&self) -> Vec<SignalKind> {
        SignalKind::STRUCTURE
            .iter()
            .copied()
            .filter(|k| self.signal_on(*k))
            .collect()
    }

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
    structure: Option<Structure>,
    structure_records: Vec<TrackRecord>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct ConfigArgs {
    enabled: Option<bool>,
    background: Option<bool>,
    background_mode: Option<BackgroundMode>,
    notify_get_ready: Option<bool>,
    notify_broken: Option<bool>,
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

pub(crate) fn to_f64(d: Decimal) -> f64 {
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
    pub(crate) fn daily_bars(&self, symbol: &str, today: NaiveDate) -> Result<Vec<Bar>> {
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
        let favourites = self.favourites()?;
        let now = self.now();
        let today = lk_day(now);
        self.look_up_missing(&favourites, today).await;
        let book = self.state.read().unwrap().alerts.clone();
        if !book.enabled {
            return Ok(());
        }
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
                        let synced = {
                            let mut s = self.state.write().unwrap();
                            s.synced.insert(sym.clone(), today);
                            s.synced.clone()
                        };
                        self.store.lock().unwrap().save_json(SYNCED_KEY, &synced)?;
                    }
                    Err(e) => a.error = Some(e.to_string()),
                }
            }
            let bars = self.daily_bars(&sym, today)?;
            a.history_days = bars.len();
            a.levels = daily_levels(&bars, &params);
            a.records = track_record(&bars, &params);
            a.structure = structure::analyze(&bars, &params);
            a.structure_records = structure::track_record(&bars, &params, &SignalKind::STRUCTURE);
            // History problems must not stop alerts.
            let _ = self.record_history(&sym, &bars, &params, &book.enabled_structure());
            if a.levels.is_none() && a.error.is_none() {
                a.error = Some(format!("Not enough history yet ({} days)", bars.len()));
            }
            self.state.write().unwrap().analysis.insert(sym, a);
        }

        // 2. Live checks while the exchange is trading, plus a closing check
        //    of the structure setups on the day's last prices.
        let (phase, exchange_open, session, trading_day) = {
            let s = self.state.read().unwrap();
            let cal = &s.account.settings.calendar;
            let open = lk_midnight(today) + (cal.open - chrono::NaiveTime::MIN);
            let close = lk_midnight(today) + (cal.close - chrono::NaiveTime::MIN);
            let ex_open = !s
                .market
                .status
                .as_deref()
                .is_some_and(|t| t.to_ascii_lowercase().contains("close"));
            (
                cal.phase_at(now),
                ex_open,
                (open, close),
                cal.is_trading_day(today),
            )
        };
        let (open, close) = session;
        let closing = phase == MarketPhase::Closed
            && trading_day
            && now >= close
            && now < close + Duration::hours(CLOSING_CHECK_HOURS);
        let trading = phase == MarketPhase::Open && exchange_open;
        if !trading && !closing {
            return Ok(());
        }
        let fraction = ((now - open).num_seconds() as f64 / (close - open).num_seconds() as f64)
            .clamp(0.0, 1.0);

        // Intraday trades, refreshed every few minutes per stock.
        let intraday_on = SignalKind::INTRADAY.iter().any(|k| book.signal_on(*k));
        if intraday_on && !closing {
            for sym in &favourites {
                let due = {
                    let s = self.state.read().unwrap();
                    // Only liquid stocks: illiquid ones never signal anyway.
                    s.analysis.get(sym).is_some_and(|a| {
                        a.levels.as_ref().is_some_and(|l| l.liquid)
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
            let live = Live {
                price,
                day_low: q.low.map_or(price, to_f64),
                volume: q.share_volume as f64,
                session_fraction: fraction,
            };
            if let Some(a) = s.analysis.get(sym) {
                if !closing {
                    if let Some(levels) = &a.levels {
                        triggers.extend(evaluate_daily(levels, &live, &params));
                    }
                    triggers.extend(a.intraday_triggers.iter().cloned());
                }
                // Market structure: each setup fires once per swing.
                if let Some(st) = &a.structure {
                    let view =
                        structure::evaluate(st, Some(&live), &params, &book.enabled_structure());
                    let star = if view.weekly_star { " ⭐" } else { "" };
                    for x in view.setups.iter().filter(|x| x.plan.passes) {
                        let ready = !x.triggered && x.distance_pct <= params.near_pct;
                        if !(x.triggered || (ready && book.notify_get_ready)) {
                            continue;
                        }
                        let key = format!(
                            "{sym}|{:?}|{}|{}",
                            x.kind,
                            if x.triggered { "go" } else { "ready" },
                            st.swing_key
                        );
                        if book.last_fired.contains_key(&key) {
                            continue;
                        }
                        book.last_fired.insert(key, now);
                        let id = book.take_id();
                        let (kind, message) = if x.triggered {
                            (
                                format!("{}{star}", x.kind.label()),
                                format!(
                                    "{} · entry {:.2}, stop {:.2}, target {:.2} ({:.1}x)",
                                    x.note,
                                    x.plan.entry,
                                    x.plan.stop,
                                    x.plan.target,
                                    x.plan.reward_risk
                                ),
                            )
                        } else {
                            (
                                format!("Get ready: {}{star}", x.kind.label()),
                                format!(
                                    "{:.1}% below the {} trigger {:.2}",
                                    x.distance_pct,
                                    x.kind.label(),
                                    x.trigger
                                ),
                            )
                        };
                        fired.push(FiredAlert {
                            id,
                            at: now,
                            symbol: sym.clone(),
                            kind,
                            level: x.trigger,
                            price,
                            message,
                            plan: x.triggered.then_some(AlertPlan {
                                entry: x.plan.entry,
                                stop: x.plan.stop,
                                target: x.plan.target,
                            }),
                        });
                    }
                    // A favourite losing its uptrend, once per swing (optional).
                    if book.notify_broken && view.breaking == Some(Break::UptrendBroken) {
                        let key = format!("{sym}|Broken|{}", st.swing_key);
                        if let std::collections::btree_map::Entry::Vacant(e) =
                            book.last_fired.entry(key)
                        {
                            e.insert(now);
                            let id = book.take_id();
                            fired.push(FiredAlert {
                                id,
                                at: now,
                                symbol: sym.clone(),
                                kind: UPTREND_BROKEN.into(),
                                level: st.last_low.unwrap_or(price),
                                price,
                                message: view.break_note.clone().unwrap_or_default(),
                                plan: None,
                            });
                        }
                    }
                }
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
                    plan: None,
                });
            }
        }
        // Custom levels: any stock with a quote, favourite or not.
        for c in book.custom.iter_mut().filter(|c| c.active && !closing) {
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
                    plan: None,
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
        self.log_alerts(&fired)?;
        self.write_outbox(&fired);
        Ok(())
    }

    /// Fetches quotes for favourites that are not in today's price list
    /// (stocks that have not traded), a few per refresh, once a day each.
    async fn look_up_missing(&self, favourites: &BTreeSet<String>, today: NaiveDate) {
        const PER_REFRESH: usize = 10;
        let missing: Vec<String> = {
            let s = self.state.read().unwrap();
            favourites
                .iter()
                .filter(|f| {
                    !s.market.quotes.contains_key(*f) && s.looked_up.get(*f) != Some(&today)
                })
                .take(PER_REFRESH)
                .cloned()
                .collect()
        };
        for sym in missing {
            let q = self.client.quote(&sym).await;
            let mut s = self.state.write().unwrap();
            s.looked_up.insert(sym.clone(), today);
            if let Ok(q) = q {
                s.market.quotes.entry(sym).or_insert(q);
            }
        }
    }

    /// Appends fired alerts to the outbox file read by the Android service.
    fn write_outbox(&self, fired: &[FiredAlert]) {
        let Some(path) = &self.outbox else { return };
        // One notification per check: a single alert in full, several summarised.
        let line = match fired {
            [] => return,
            [a] => json!({
                "id": a.id,
                "title": format!("{} · {}", short(&a.symbol), a.kind),
                "body": a.message,
            }),
            many => json!({
                "id": many.iter().map(|a| a.id).max(),
                "title": format!("🔔 {} stocks to look at", many.len()),
                "body": many
                    .iter()
                    .map(|a| format!("{} {}", short(&a.symbol), a.kind))
                    .collect::<Vec<_>>()
                    .join(" · "),
            }),
        };
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "{line}");
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
                    if let Some(v) = a.background_mode {
                        b.background_mode = v;
                    }
                    if let Some(v) = a.notify_get_ready {
                        b.notify_get_ready = v;
                    }
                    if let Some(v) = a.notify_broken {
                        b.notify_broken = v;
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
            "favourite_markers" => self.favourite_markers(),
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
                    "structure": a.and_then(|a| a.structure.as_ref()),
                    "structure_records": a.map(|a| &a.structure_records),
                    "structure_view": a.and_then(|a| a.structure.as_ref()).map(|st| {
                        let live = q.and_then(live_of);
                        structure::evaluate(st, live.as_ref(), &b.params, &b.enabled_structure())
                    }),
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
        let structure_labels: Vec<(String, &str)> = SignalKind::STRUCTURE
            .iter()
            .map(|k| (format!("{k:?}"), k.label()))
            .collect();
        Ok(json!({
            "enabled": b.enabled,
            "background": b.background,
            "background_mode": b.background_mode,
            "notify_get_ready": b.notify_get_ready,
            "notify_broken": b.notify_broken,
            "signals": b.signals,
            "labels": labels,
            "structure_labels": structure_labels,
            "params": b.params,
            "favourites": rows,
            "custom": b.custom,
            "history": history,
        }))
    }
}

/// `ABAN.N0000` -> `ABAN` for notifications.
fn short(symbol: &str) -> &str {
    symbol.strip_suffix(".N0000").unwrap_or(symbol)
}

/// Today's figures from a quote, for evaluating levels outside the refresh.
fn live_of(q: &cse_core::Quote) -> Option<Live> {
    let price = to_f64(q.last);
    (price > 0.0).then(|| Live {
        price,
        day_low: q.low.map_or(price, to_f64),
        volume: q.share_volume as f64,
        session_fraction: 1.0,
    })
}

pub(crate) fn status_name(s: Status) -> &'static str {
    match s {
        Status::LookNow => "look_now",
        Status::GetReady => "get_ready",
        Status::Uptrend => "uptrend",
        Status::Broken => "broken",
        Status::Range => "range",
        Status::Downtrend => "downtrend",
        Status::Thin => "thin",
    }
}

impl AppService {
    /// Marker per favourite for the Watch list: what to look at.
    fn favourite_markers(&self) -> Result<Value> {
        let favourites = self.favourites()?;
        let today = lk_day(self.now());
        let s = self.state.read().unwrap();
        let b = &s.alerts;
        let enabled = b.enabled_structure();
        let structure_labels: Vec<&str> = SignalKind::STRUCTURE.iter().map(|k| k.label()).collect();
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        let mut markers = serde_json::Map::new();
        for sym in &favourites {
            let a = s.analysis.get(sym);
            let q = s.market.quotes.get(sym);
            let marker = match a.and_then(|a| a.structure.as_ref()) {
                None => json!({ "status": if a.is_none() { "pending" } else { "no_data" } }),
                Some(st) => {
                    let live = q.and_then(live_of);
                    let view = structure::evaluate(st, live.as_ref(), &b.params, &enabled);
                    // A setup that fired today keeps the stock in "look now".
                    let fired_today = b.history.iter().rev().find(|f| {
                        f.symbol == *sym
                            && lk_day(f.at) == today
                            && structure_labels.iter().any(|l| f.kind.starts_with(l))
                    });
                    let status = if fired_today.is_some() && st.liquid {
                        Status::LookNow
                    } else {
                        view.status
                    };
                    let best = view
                        .setups
                        .iter()
                        .filter(|x| x.plan.passes)
                        .min_by(|x, y| x.distance_pct.total_cmp(&y.distance_pct));
                    let (label, detail) = match (status, best, fired_today) {
                        (Status::LookNow, _, Some(f)) => (f.kind.clone(), f.message.clone()),
                        (Status::LookNow | Status::GetReady, Some(x), _) => (
                            x.kind.label().to_string(),
                            if x.triggered {
                                format!(
                                    "entry {:.2} · stop {:.2} · {:.1}x",
                                    x.plan.entry, x.plan.stop, x.plan.reward_risk
                                )
                            } else {
                                format!(
                                    "{:.1}% to {:.2} · {:.1}x",
                                    x.distance_pct, x.trigger, x.plan.reward_risk
                                )
                            },
                        ),
                        _ => match view.breaking {
                            Some(Break::UptrendBroken) => (
                                UPTREND_BROKEN.to_string(),
                                format!(
                                    "below the last higher low {:.2}",
                                    st.last_low.unwrap_or(0.0)
                                ),
                            ),
                            Some(Break::DowntrendBreaking) => (
                                "Downtrend breaking".to_string(),
                                format!(
                                    "above the last lower high {:.2}",
                                    st.last_high.unwrap_or(0.0)
                                ),
                            ),
                            None => (String::new(), String::new()),
                        },
                    };
                    json!({
                        "status": status_name(status),
                        "trend": st.trend,
                        "star": view.weekly_star,
                        "label": label,
                        "detail": detail,
                    })
                }
            };
            let st = marker["status"].as_str().unwrap_or("pending").to_string();
            *counts.entry(st).or_default() += 1;
            markers.insert(sym.clone(), marker);
        }
        Ok(json!({
            "markers": markers,
            "counts": counts,
            "updated_at": s.market.updated_at,
            "background_mode": b.background_mode,
        }))
    }

    /// One complete check for scheduled background runs: refresh prices,
    /// update every favourite and fire alerts. Returns the number of new
    /// alerts.
    pub async fn run_check(&self) -> Result<usize> {
        let before = self.state.read().unwrap().alerts.next_id;
        self.refresh().await?;
        let after = self.state.read().unwrap().alerts.next_id;
        Ok((after - before) as usize)
    }
}

/// Default location of the outbox next to the database.
pub(crate) fn outbox_beside(db_path: &std::path::Path) -> PathBuf {
    db_path.parent().map_or_else(
        || PathBuf::from("alerts-outbox.jsonl"),
        |d| d.join("alerts-outbox.jsonl"),
    )
}
