//! Market-structure history: the swing log, one structure row per trading
//! day, every alert ever fired, and how each alert turned out.

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, NaiveDate, Utc};
use cse_signals::structure::{self, Trend};
use cse_signals::{Bar, Params, SignalKind};
use cse_storage::{AlertRow, StructureDay, SwingRow};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::alerts::{status_name, to_f64, FiredAlert};
use crate::chart::lk_day;
use crate::{parse, AppService, Result};

/// Trading days after an alert at which its return is measured.
const HORIZONS: [usize; 3] = [5, 10, 20];

fn day_of(secs: i64) -> String {
    lk_day(DateTime::from_timestamp(secs, 0).unwrap_or_default()).to_string()
}

fn trend_name(t: Trend) -> &'static str {
    match t {
        Trend::Up => "Up",
        Trend::Down => "Down",
        Trend::Range => "Range",
    }
}

pub(crate) fn alert_row(a: &FiredAlert) -> AlertRow {
    AlertRow {
        id: a.id,
        at_ms: a.at.timestamp_millis(),
        symbol: a.symbol.clone(),
        kind: a.kind.clone(),
        price: a.price,
        level: a.level,
        message: a.message.clone(),
        entry: a.plan.map(|p| p.entry),
        stop: a.plan.map(|p| p.stop),
        target: a.plan.map(|p| p.target),
    }
}

/// How an alert turned out, from the daily bars after its day.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Outcome {
    /// `target` or `stop` (whichever came first) or `open`; none for alerts
    /// without a plan.
    pub status: Option<&'static str>,
    pub resolved_day: Option<String>,
    /// Return at the target or stop, after fees.
    pub result_pct: Option<f64>,
    /// Best and worst prices reached (vs entry) until resolved or now.
    pub best_pct: Option<f64>,
    pub worst_pct: Option<f64>,
    /// Close-to-entry returns after 5, 10 and 20 trading days, after fees.
    pub after: BTreeMap<usize, f64>,
    pub days: usize,
}

/// Bars strictly after the alert's day. A day that reaches both the stop
/// and the target counts as stopped (the order within a day is unknown).
pub fn outcome(a: &AlertRow, bars: &[Bar], fee_pct: f64) -> Outcome {
    let entry = a.entry.unwrap_or(a.price);
    let pct = |x: f64| (x / entry - 1.0) * 100.0;
    let mut out = Outcome {
        status: a.entry.map(|_| "open"),
        resolved_day: None,
        result_pct: None,
        best_pct: None,
        worst_pct: None,
        after: BTreeMap::new(),
        days: bars.len(),
    };
    if entry <= 0.0 {
        return out;
    }
    for h in HORIZONS {
        if let Some(b) = bars.get(h - 1) {
            out.after.insert(h, pct(b.close) - fee_pct);
        }
    }
    let (mut best, mut worst) = (f64::MIN, f64::MAX);
    for b in bars {
        best = best.max(b.high);
        worst = worst.min(b.low);
        let hit = match (a.stop, a.target) {
            (Some(stop), _) if b.low <= stop => Some(("stop", stop)),
            (_, Some(target)) if b.high >= target => Some(("target", target)),
            _ => None,
        };
        if let Some((status, level)) = hit {
            out.status = Some(status);
            out.resolved_day = Some(day_of(b.time));
            out.result_pct = Some(pct(level) - fee_pct);
            break;
        }
    }
    if !bars.is_empty() {
        out.best_pct = Some(pct(best));
        out.worst_pct = Some(pct(worst));
    }
    out
}

/// A run of days in the same state: a trend, or "broken".
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Period {
    pub state: String,
    pub from: String,
    pub to: String,
    pub days: usize,
    pub close_from: f64,
    pub close_to: f64,
    pub note: Option<String>,
}

pub fn periods(days: &[StructureDay]) -> Vec<Period> {
    let state = |d: &StructureDay| match d.status.as_str() {
        "broken" => "Broken".to_string(),
        "breaking_down" => "BreakingDown".to_string(),
        _ => d.trend.clone(),
    };
    let mut out: Vec<Period> = Vec::new();
    for d in days {
        let s = state(d);
        match out.last_mut() {
            Some(p) if p.state == s => {
                p.to = d.day.clone();
                p.days += 1;
                p.close_to = d.close;
            }
            _ => out.push(Period {
                state: s,
                from: d.day.clone(),
                to: d.day.clone(),
                days: 1,
                close_from: d.close,
                close_to: d.close,
                note: d.note.clone(),
            }),
        }
    }
    out
}

#[derive(Deserialize)]
struct SymbolArgs {
    symbol: String,
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

fn opt(v: Option<f64>) -> String {
    v.map(|x| format!("{x:.2}")).unwrap_or_default()
}

impl AppService {
    /// Brings the swing log and the daily structure rows up to date from
    /// completed daily bars. The first run reconstructs the past year
    /// (marked as backfilled); later runs only add new days and swings.
    pub(crate) fn record_history(
        &self,
        symbol: &str,
        bars: &[Bar],
        p: &Params,
        enabled: &[SignalKind],
    ) -> Result<()> {
        let Some(st) = structure::analyze(bars, p) else {
            return Ok(());
        };
        let (last_swing, last_day) = {
            let store = self.store.lock().unwrap();
            (
                store.last_swing_day(symbol)?,
                store.last_structure_day(symbol)?,
            )
        };

        // Swings: only ones newer than the last logged, so the log never
        // rewrites what was recorded earlier.
        let z = structure::zigzag(bars, st.threshold_pct);
        let swings: Vec<SwingRow> = z
            .swings
            .iter()
            .enumerate()
            .filter_map(|(i, w)| {
                let day = day_of(w.time);
                if last_swing.as_ref().is_some_and(|l| day <= *l) {
                    return None;
                }
                Some(SwingRow {
                    symbol: symbol.to_owned(),
                    day,
                    kind: format!("{:?}", w.kind),
                    label: w.label.map(|l| format!("{l:?}")),
                    price: w.price,
                    trend: trend_name(structure::trend_of(&z.swings[..=i])).to_owned(),
                    backfilled: last_swing.is_none(),
                })
            })
            .collect();

        // Days: each completed day from the last saved one (redone, in case
        // it was saved before the close) onwards.
        let mut days = Vec::new();
        for i in 29..bars.len() {
            let day = day_of(bars[i].time);
            if last_day.as_ref().is_some_and(|l| day < *l) {
                continue;
            }
            let Some(s) = structure::analyze(&bars[..=i], p) else {
                continue;
            };
            let v = structure::evaluate(&s, None, p, enabled);
            days.push(StructureDay {
                symbol: symbol.to_owned(),
                day,
                trend: trend_name(s.trend).to_owned(),
                weekly: s.weekly_trend.map(|t| trend_name(t).to_owned()),
                status: status_name(v.status).to_owned(),
                close: bars[i].close,
                note: v.break_note,
                backfilled: last_day.is_none(),
            });
        }
        let mut store = self.store.lock().unwrap();
        store.log_swings(&swings)?;
        store.save_structure_days(&days)?;
        Ok(())
    }

    pub(crate) fn log_alerts(&self, fired: &[FiredAlert]) -> Result<()> {
        let rows: Vec<AlertRow> = fired.iter().map(alert_row).collect();
        self.store.lock().unwrap().log_alerts(&rows)?;
        Ok(())
    }

    fn with_outcome(&self, a: &AlertRow, fee_pct: f64) -> Result<Value> {
        let at = DateTime::<Utc>::from_timestamp_millis(a.at_ms).unwrap_or_default();
        let next: NaiveDate = lk_day(at) + Duration::days(1);
        let bars: Vec<Bar> = self
            .store
            .lock()
            .unwrap()
            .daily_history(&a.symbol, Some(next))?
            .into_iter()
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
            .collect();
        let mut v = serde_json::to_value(a).unwrap_or_default();
        v["at"] = json!(at);
        v["outcome"] = json!(outcome(a, &bars, fee_pct));
        Ok(v)
    }

    pub(crate) fn history_command(&self, cmd: &str, args: Value) -> Option<Result<Value>> {
        let r = match cmd {
            "structure_history" => {
                parse::<SymbolArgs>(args).and_then(|a| self.structure_history(&a.symbol))
            }
            "alert_results" => self.alert_results(),
            "export_history" => self.export_history(),
            _ => return None,
        };
        Some(r)
    }

    fn fee_pct(&self) -> f64 {
        self.state.read().unwrap().alerts.params.round_trip_fee_pct
    }

    fn structure_history(&self, symbol: &str) -> Result<Value> {
        let (mut swings, days, alerts) = {
            let store = self.store.lock().unwrap();
            (
                store.swings(Some(symbol))?,
                store.structure_days(Some(symbol))?,
                store.alert_log(Some(symbol))?,
            )
        };
        swings.reverse();
        let mut periods = periods(&days);
        periods.reverse();
        let fee = self.fee_pct();
        let alerts = alerts
            .iter()
            .map(|a| self.with_outcome(a, fee))
            .collect::<Result<Vec<_>>>()?;
        Ok(json!({
            "symbol": symbol,
            "swings": swings,
            "periods": periods,
            "days": days.len(),
            "since": days.first().map(|d| d.day.clone()),
            "alerts": alerts,
        }))
    }

    fn alert_results(&self) -> Result<Value> {
        let log = self.store.lock().unwrap().alert_log(None)?;
        let fee = self.fee_pct();
        let rows = log
            .iter()
            .map(|a| self.with_outcome(a, fee))
            .collect::<Result<Vec<_>>>()?;
        // Per signal: how the alerts with a plan turned out.
        let mut summary: BTreeMap<String, Value> = BTreeMap::new();
        for r in &rows {
            let Some(status) = r["outcome"]["status"].as_str() else {
                continue;
            };
            let kind = r["kind"]
                .as_str()
                .unwrap_or("")
                .trim_end_matches(" ⭐")
                .to_owned();
            let e = summary.entry(kind).or_insert_with(
                || json!({"alerts": 0, "target": 0, "stop": 0, "open": 0, "r10_sum": 0.0, "r10_n": 0}),
            );
            e["alerts"] = json!(e["alerts"].as_u64().unwrap_or(0) + 1);
            e[status] = json!(e[status].as_u64().unwrap_or(0) + 1);
            if let Some(x) = r["outcome"]["after"]["10"].as_f64() {
                e["r10_sum"] = json!(e["r10_sum"].as_f64().unwrap_or(0.0) + x);
                e["r10_n"] = json!(e["r10_n"].as_u64().unwrap_or(0) + 1);
            }
        }
        for e in summary.values_mut() {
            let n = e["r10_n"].as_u64().unwrap_or(0);
            e["avg_10d_pct"] = if n > 0 {
                json!(e["r10_sum"].as_f64().unwrap_or(0.0) / n as f64)
            } else {
                Value::Null
            };
        }
        Ok(json!({ "alerts": rows, "summary": summary, "fee_pct": fee }))
    }

    /// Swings, daily structure and alert results as one CSV text.
    fn export_history(&self) -> Result<Value> {
        let (swings, days) = {
            let store = self.store.lock().unwrap();
            (store.swings(None)?, store.structure_days(None)?)
        };
        let results = self.alert_results()?;
        let mut out =
            String::from("# Swings\nsymbol,day,kind,label,price,trend_after,backfilled\n");
        for w in &swings {
            out += &format!(
                "{},{},{},{},{:.2},{},{}\n",
                w.symbol,
                w.day,
                w.kind,
                w.label.as_deref().unwrap_or(""),
                w.price,
                w.trend,
                w.backfilled
            );
        }
        out += "\n# Daily structure\nsymbol,day,trend,weekly,status,close,note,backfilled\n";
        for d in &days {
            out += &format!(
                "{},{},{},{},{},{:.2},{},{}\n",
                d.symbol,
                d.day,
                d.trend,
                d.weekly.as_deref().unwrap_or(""),
                d.status,
                d.close,
                csv_field(d.note.as_deref().unwrap_or("")),
                d.backfilled
            );
        }
        out += "\n# Alerts\nid,time,symbol,kind,price,entry,stop,target,result,resolved_day,result_pct,best_pct,worst_pct,after_5d_pct,after_10d_pct,after_20d_pct\n";
        for r in results["alerts"].as_array().into_iter().flatten() {
            let o = &r["outcome"];
            let f = |v: &Value| opt(v.as_f64());
            out += &format!(
                "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
                r["id"],
                r["at"].as_str().unwrap_or(""),
                r["symbol"].as_str().unwrap_or(""),
                csv_field(r["kind"].as_str().unwrap_or("")),
                f(&r["price"]),
                f(&r["entry"]),
                f(&r["stop"]),
                f(&r["target"]),
                o["status"].as_str().unwrap_or(""),
                o["resolved_day"].as_str().unwrap_or(""),
                f(&o["result_pct"]),
                f(&o["best_pct"]),
                f(&o["worst_pct"]),
                f(&o["after"]["5"]),
                f(&o["after"]["10"]),
                f(&o["after"]["20"]),
            );
        }
        Ok(json!({ "csv": out, "swings": swings.len(), "days": days.len() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(day: i64, close: f64, high: f64, low: f64) -> Bar {
        Bar {
            time: day * 86_400,
            close,
            high,
            low,
            volume: 1e6,
        }
    }

    fn alert(entry: Option<f64>) -> AlertRow {
        AlertRow {
            id: 1,
            at_ms: 0,
            symbol: "X".into(),
            kind: "Higher low entry".into(),
            price: 100.0,
            level: 100.0,
            message: String::new(),
            entry,
            stop: entry.map(|_| 95.0),
            target: entry.map(|_| 110.0),
        }
    }

    #[test]
    fn outcome_target_stop_and_open() {
        let up = [bar(1, 103.0, 104.0, 99.0), bar(2, 109.0, 111.0, 104.0)];
        let o = outcome(&alert(Some(100.0)), &up, 2.0);
        assert_eq!(o.status, Some("target"));
        assert!((o.result_pct.unwrap() - 8.0).abs() < 1e-9);
        assert!((o.best_pct.unwrap() - 11.0).abs() < 1e-9);

        let down = [bar(1, 97.0, 101.0, 96.0), bar(2, 94.0, 98.0, 93.0)];
        let o = outcome(&alert(Some(100.0)), &down, 2.0);
        assert_eq!(o.status, Some("stop"));
        assert!((o.result_pct.unwrap() + 7.0).abs() < 1e-9);

        // Both levels on one day: counted as stopped.
        let both = [bar(1, 100.0, 112.0, 94.0)];
        assert_eq!(
            outcome(&alert(Some(100.0)), &both, 2.0).status,
            Some("stop")
        );

        let flat: Vec<Bar> = (1..=12).map(|d| bar(d, 101.0, 102.0, 99.0)).collect();
        let o = outcome(&alert(Some(100.0)), &flat, 2.0);
        assert_eq!(o.status, Some("open"));
        assert!((o.after[&5] + 1.0).abs() < 1e-9);
        assert!((o.after[&10] + 1.0).abs() < 1e-9);
        assert!(!o.after.contains_key(&20));

        // No plan: returns only.
        let o = outcome(&alert(None), &flat, 2.0);
        assert_eq!(o.status, None);
        assert_eq!(o.after.len(), 2);
    }

    #[test]
    fn periods_group_runs_of_the_same_state() {
        let d = |day: &str, trend: &str, status: &str| StructureDay {
            symbol: "X".into(),
            day: day.into(),
            trend: trend.into(),
            weekly: None,
            status: status.into(),
            close: 1.0,
            note: None,
            backfilled: false,
        };
        let p = periods(&[
            d("2026-10-19", "Range", "range"),
            d("2026-10-20", "Up", "uptrend"),
            d("2026-10-21", "Up", "look_now"),
            d("2026-10-22", "Up", "broken"),
        ]);
        let states: Vec<(&str, usize)> = p.iter().map(|x| (x.state.as_str(), x.days)).collect();
        assert_eq!(states, vec![("Range", 1), ("Up", 2), ("Broken", 1)]);
        assert_eq!(p[1].from, "2026-10-20");
        assert_eq!(p[1].to, "2026-10-21");
    }
}
