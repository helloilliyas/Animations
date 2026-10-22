//! Market structure: swing highs and lows, higher-high / higher-low trends,
//! and buy-side setups with a trade plan.
//!
//! Swings come from a percentage zigzag: a new swing is confirmed only once
//! price reverses by the threshold, which is the larger of
//! `min_swing_pct` and `swing_range_mult` x the stock's typical daily range.
//! That ignores the tick-size bouncing common on the CSE.
//!
//! Setups (all buy-side):
//! - **Higher low entry**: in an uptrend, a pullback that stays above the
//!   previous swing low bounces by `bounce_frac` of the threshold.
//! - **Uptrend confirmed**: after a higher low, price clears the last high.
//! - **Downtrend broken**: in a downtrend, price clears the last lower high.

use serde::Serialize;

use crate::{Bar, Live, Params, SignalKind, TrackRecord};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SwingKind {
    High,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SwingLabel {
    HH,
    LH,
    HL,
    LL,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Swing {
    pub index: usize,
    pub time: i64,
    pub price: f64,
    pub kind: SwingKind,
    /// Compared with the previous swing of the same kind.
    pub label: Option<SwingLabel>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Trend {
    Up,
    Down,
    Range,
}

/// Confirmed swings plus the leg in progress.
#[derive(Debug, Clone, PartialEq)]
pub struct Zigzag {
    pub swings: Vec<Swing>,
    /// Direction of the unconfirmed leg since the last swing.
    pub leg_up: Option<bool>,
    /// Highest high (up leg) or lowest low (down leg) of that leg.
    pub extreme: Option<f64>,
}

fn push(swings: &mut Vec<Swing>, bars: &[Bar], index: usize, price: f64, kind: SwingKind) {
    let prev = swings
        .iter()
        .rev()
        .find(|s| s.kind == kind)
        .map(|s| s.price);
    let label = prev.map(|p| match (kind, price > p) {
        (SwingKind::High, true) => SwingLabel::HH,
        (SwingKind::High, false) => SwingLabel::LH,
        (SwingKind::Low, true) => SwingLabel::HL,
        (SwingKind::Low, false) => SwingLabel::LL,
    });
    swings.push(Swing {
        index,
        time: bars[index].time,
        price,
        kind,
        label,
    });
}

fn min_low(bars: &[Bar], from: usize, to: usize) -> (usize, f64) {
    (from..=to).fold((from, f64::MAX), |acc, i| {
        if bars[i].low < acc.1 {
            (i, bars[i].low)
        } else {
            acc
        }
    })
}

fn max_high(bars: &[Bar], from: usize, to: usize) -> (usize, f64) {
    (from..=to).fold((from, f64::MIN), |acc, i| {
        if bars[i].high > acc.1 {
            (i, bars[i].high)
        } else {
            acc
        }
    })
}

/// Percentage zigzag over highs and lows.
pub fn zigzag(bars: &[Bar], threshold_pct: f64) -> Zigzag {
    let mut swings = Vec::new();
    if bars.is_empty() {
        return Zigzag {
            swings,
            leg_up: None,
            extreme: None,
        };
    }
    let thr = threshold_pct / 100.0;
    let mut up: Option<bool> = None;
    let (mut hi_i, mut hi) = (0, bars[0].high);
    let (mut lo_i, mut lo) = (0, bars[0].low);
    for i in 1..bars.len() {
        let b = bars[i];
        match up {
            None => {
                if b.high > hi {
                    (hi_i, hi) = (i, b.high);
                }
                if b.low < lo {
                    (lo_i, lo) = (i, b.low);
                }
                if hi >= lo * (1.0 + thr) {
                    if lo_i < hi_i {
                        push(&mut swings, bars, lo_i, lo, SwingKind::Low);
                        up = Some(true);
                    } else {
                        push(&mut swings, bars, hi_i, hi, SwingKind::High);
                        (lo_i, lo) = min_low(bars, hi_i, i);
                        up = Some(false);
                    }
                }
            }
            Some(true) => {
                if b.high > hi {
                    (hi_i, hi) = (i, b.high);
                }
                if b.low <= hi * (1.0 - thr) {
                    push(&mut swings, bars, hi_i, hi, SwingKind::High);
                    (lo_i, lo) = min_low(bars, hi_i + 1, i);
                    up = Some(false);
                }
            }
            Some(false) => {
                if b.low < lo {
                    (lo_i, lo) = (i, b.low);
                }
                if b.high >= lo * (1.0 + thr) {
                    push(&mut swings, bars, lo_i, lo, SwingKind::Low);
                    (hi_i, hi) = max_high(bars, lo_i + 1, i);
                    up = Some(true);
                }
            }
        }
    }
    let extreme = up.map(|u| if u { hi } else { lo });
    Zigzag {
        swings,
        leg_up: up,
        extreme,
    }
}

fn last_two(swings: &[Swing], kind: SwingKind) -> (Option<f64>, Option<f64>) {
    let mut it = swings
        .iter()
        .rev()
        .filter(|s| s.kind == kind)
        .map(|s| s.price);
    let newer = it.next();
    (it.next(), newer)
}

/// Trend from the last two swing highs and lows.
pub fn trend_of(swings: &[Swing]) -> Trend {
    match (
        last_two(swings, SwingKind::High),
        last_two(swings, SwingKind::Low),
    ) {
        ((Some(h1), Some(h2)), (Some(l1), Some(l2))) if h2 > h1 && l2 > l1 => Trend::Up,
        ((Some(h1), Some(h2)), (Some(l1), Some(l2))) if h2 < h1 && l2 < l1 => Trend::Down,
        _ => Trend::Range,
    }
}

fn avg_range_pct(bars: &[Bar]) -> f64 {
    let look = &bars[bars.len().saturating_sub(20)..];
    if look.is_empty() {
        return 0.0;
    }
    look.iter()
        .map(|b| {
            if b.close > 0.0 {
                (b.high - b.low) / b.close * 100.0
            } else {
                0.0
            }
        })
        .sum::<f64>()
        / look.len() as f64
}

/// Daily bars grouped into Monday-to-Sunday weeks.
pub fn weekly_bars(bars: &[Bar]) -> Vec<Bar> {
    let mut out: Vec<(i64, Bar)> = Vec::new();
    for b in bars {
        // Unix day 0 was a Thursday; +3 makes weeks start on Monday.
        let week = (b.time.div_euclid(86_400) + 3).div_euclid(7);
        match out.last_mut() {
            Some((w, wb)) if *w == week => {
                wb.high = wb.high.max(b.high);
                wb.low = wb.low.min(b.low);
                wb.close = b.close;
                wb.volume += b.volume;
                wb.time = b.time;
            }
            _ => out.push((week, *b)),
        }
    }
    out.into_iter().map(|(_, b)| b).collect()
}

/// Structure of a stock from completed daily bars (oldest first).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Structure {
    pub as_of: i64,
    pub last_close: f64,
    pub threshold_pct: f64,
    pub trend: Trend,
    pub weekly_trend: Option<Trend>,
    /// The most recent swings, oldest first.
    pub swings: Vec<Swing>,
    pub leg_up: Option<bool>,
    pub leg_extreme: Option<f64>,
    pub last_high: Option<f64>,
    pub prev_high: Option<f64>,
    pub last_low: Option<f64>,
    pub prev_low: Option<f64>,
    /// Identifies the latest swing, so a setup fires once per swing.
    pub swing_key: i64,
    pub avg_turnover: f64,
    pub liquid: bool,
}

/// Needs at least 30 daily bars.
pub fn analyze(bars: &[Bar], p: &Params) -> Option<Structure> {
    if bars.len() < 30 {
        return None;
    }
    let threshold_pct = p
        .min_swing_pct
        .max(p.swing_range_mult * avg_range_pct(bars));
    let z = zigzag(bars, threshold_pct);
    let weeks = weekly_bars(bars);
    let weekly_threshold = p
        .weekly_min_swing_pct
        .max(p.swing_range_mult * avg_range_pct(&weeks));
    let wz = zigzag(&weeks, weekly_threshold);
    let enough = |s: &[Swing]| {
        s.iter().filter(|x| x.kind == SwingKind::High).count() >= 2
            && s.iter().filter(|x| x.kind == SwingKind::Low).count() >= 2
    };
    let (prev_high, last_high) = last_two(&z.swings, SwingKind::High);
    let (prev_low, last_low) = last_two(&z.swings, SwingKind::Low);
    let look = &bars[bars.len().saturating_sub(20)..];
    let avg_turnover = look.iter().map(|b| b.close * b.volume).sum::<f64>() / look.len() as f64;
    let last = bars.last().unwrap();
    Some(Structure {
        as_of: last.time,
        last_close: last.close,
        threshold_pct,
        trend: trend_of(&z.swings),
        weekly_trend: enough(&wz.swings).then(|| trend_of(&wz.swings)),
        swing_key: z.swings.last().map_or(0, |s| s.time),
        swings: z.swings[z.swings.len().saturating_sub(8)..].to_vec(),
        leg_up: z.leg_up,
        leg_extreme: z.extreme,
        last_high,
        prev_high,
        last_low,
        prev_low,
        avg_turnover,
        liquid: avg_turnover >= p.min_avg_turnover,
    })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Plan {
    pub entry: f64,
    pub stop: f64,
    pub target: f64,
    pub risk_pct: f64,
    pub reward_pct: f64,
    pub reward_risk: f64,
    /// Reward/risk and fees filters both pass.
    pub passes: bool,
}

fn plan(entry: f64, stop: f64, target: f64, p: &Params) -> Plan {
    let risk = entry - stop;
    let reward = target - entry;
    let reward_risk = if risk > 0.0 { reward / risk } else { 0.0 };
    let reward_pct = reward / entry * 100.0;
    Plan {
        entry,
        stop,
        target,
        risk_pct: risk / entry * 100.0,
        reward_pct,
        reward_risk,
        passes: risk > 0.0 && reward_risk >= p.min_reward_risk && reward_pct > p.round_trip_fee_pct,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Setup {
    pub kind: SignalKind,
    pub trigger: f64,
    pub plan: Plan,
    pub triggered: bool,
    /// Percent the price must rise to reach the trigger (0 once reached).
    pub distance_pct: f64,
    pub note: String,
}

/// What to do with a stock right now, most urgent first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Status {
    LookNow,
    GetReady,
    Uptrend,
    Range,
    Downtrend,
    Thin,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct View {
    pub status: Status,
    pub setups: Vec<Setup>,
    /// The weekly trend is up too.
    pub weekly_star: bool,
}

/// Setups for today using the live price (or the last close).
pub fn evaluate(s: &Structure, live: Option<&Live>, p: &Params, enabled: &[SignalKind]) -> View {
    let price = live.map_or(s.last_close, |l| l.price);
    let day_low = live.map_or(s.last_close, |l| l.day_low.min(l.price));
    let thr = s.threshold_pct / 100.0;
    let buf = 1.0 + p.buffer_pct / 100.0;
    let stop_of = |low: f64| low * (1.0 - p.stop_buffer_pct / 100.0);
    let mut setups = Vec::new();
    let mut add = |kind: SignalKind, trigger: f64, plan: Plan, note: String| {
        if enabled.contains(&kind) {
            setups.push(Setup {
                kind,
                trigger,
                triggered: price >= trigger,
                distance_pct: ((trigger / price - 1.0) * 100.0).max(0.0),
                plan,
                note,
            });
        }
    };

    let last_is_hl = s
        .swings
        .last()
        .is_some_and(|w| w.kind == SwingKind::Low && w.label == Some(SwingLabel::HL));
    if let (Some(h2), Some(l2)) = (s.last_high, s.last_low) {
        // Pullback after a higher high, still above the previous low.
        if s.trend == Trend::Up && s.leg_up == Some(false) {
            let low = s.leg_extreme.unwrap_or(s.last_close).min(day_low);
            let depth = (h2 - low) / h2;
            if low > l2 && depth >= p.bounce_frac * thr {
                let trigger = low * (1.0 + p.bounce_frac * thr);
                let pl = plan(trigger, stop_of(low), h2, p);
                add(
                    SignalKind::HigherLowEntry,
                    trigger,
                    pl,
                    format!("Pulled back to {low:.2}, above the last low {l2:.2}; bounce to {trigger:.2} confirms a higher low"),
                );
            }
        }
        // After a higher low, the next leg up clearing the last high.
        if last_is_hl && s.leg_up == Some(true) {
            let trigger = h2 * buf;
            // Breakout plan: a fall back below the old high by half a swing
            // means the breakout failed; target the size of the prior advance.
            let stop = stop_of((h2 * (1.0 - 0.5 * thr)).max(l2));
            let base = s.prev_low.map_or(l2, |l1| l1.min(l2));
            let pl = plan(trigger, stop, trigger + (h2 - base), p);
            add(
                SignalKind::UptrendConfirmed,
                trigger,
                pl,
                format!("Higher low at {l2:.2}; clearing {h2:.2} makes a higher high"),
            );
        }
        // Downtrend: clearing the last lower high.
        if s.trend == Trend::Down && s.leg_up == Some(true) {
            let trigger = h2 * buf;
            let target = s
                .prev_high
                .filter(|h| *h > trigger)
                .unwrap_or(trigger + (h2 - l2));
            let stop = stop_of((h2 * (1.0 - 0.5 * thr)).max(l2));
            let pl = plan(trigger, stop, target, p);
            add(
                SignalKind::DowntrendBroken,
                trigger,
                pl,
                format!("Lower highs and lows; clearing {h2:.2} breaks the downtrend"),
            );
        }
    }

    let status = if !s.liquid {
        Status::Thin
    } else if setups.iter().any(|x| x.triggered && x.plan.passes) {
        Status::LookNow
    } else if setups
        .iter()
        .any(|x| !x.triggered && x.plan.passes && x.distance_pct <= p.near_pct)
    {
        Status::GetReady
    } else {
        match s.trend {
            Trend::Up => Status::Uptrend,
            Trend::Range => Status::Range,
            Trend::Down => Status::Downtrend,
        }
    };
    View {
        status,
        setups,
        weekly_star: s.weekly_trend == Some(Trend::Up),
    }
}

/// Replays each day of history: analyse the bars before it, evaluate with
/// that day's close and low, and measure the return `horizon_days` later.
/// A setup counts once per swing.
pub fn track_record(bars: &[Bar], p: &Params, kinds: &[SignalKind]) -> Vec<TrackRecord> {
    let n = bars.len();
    let mut returns: Vec<Vec<f64>> = vec![Vec::new(); kinds.len()];
    let mut last: Vec<Option<i64>> = vec![None; kinds.len()];
    let mut seen: Vec<(SignalKind, i64)> = Vec::new();
    for i in 40..n {
        let Some(s) = analyze(&bars[..i], p) else {
            continue;
        };
        if !s.liquid {
            continue;
        }
        let b = bars[i];
        let live = Live {
            price: b.close,
            day_low: b.low,
            volume: b.volume,
            session_fraction: 1.0,
        };
        for setup in evaluate(&s, Some(&live), p, kinds).setups {
            if !(setup.triggered && setup.plan.passes) || seen.contains(&(setup.kind, s.swing_key))
            {
                continue;
            }
            seen.push((setup.kind, s.swing_key));
            let k = kinds.iter().position(|x| *x == setup.kind).unwrap();
            last[k] = Some(b.time);
            if i + p.horizon_days < n && b.close > 0.0 {
                returns[k].push((bars[i + p.horizon_days].close / b.close - 1.0) * 100.0);
            }
        }
    }
    kinds
        .iter()
        .enumerate()
        .map(|(k, &kind)| {
            let r = &returns[k];
            let wins = r.iter().filter(|x| **x > p.round_trip_fee_pct).count();
            TrackRecord {
                kind,
                fired: r.len(),
                wins,
                win_rate_pct: if r.is_empty() {
                    0.0
                } else {
                    wins as f64 * 100.0 / r.len() as f64
                },
                avg_return_pct: if r.is_empty() {
                    0.0
                } else {
                    r.iter().sum::<f64>() / r.len() as f64
                },
                last_fired: last[k],
                horizon_days: p.horizon_days,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400;

    /// Bars following a path of closes with a 1% high/low band.
    fn path(closes: &[f64]) -> Vec<Bar> {
        closes
            .iter()
            .enumerate()
            .map(|(i, &c)| Bar {
                time: i as i64 * DAY,
                close: c,
                high: c * 1.005,
                low: c * 0.995,
                volume: 1e6,
            })
            .collect()
    }

    /// Linear moves between turning points, `steps` days per leg.
    fn legs(points: &[f64], steps: usize) -> Vec<f64> {
        let mut out = vec![points[0]];
        for w in points.windows(2) {
            for s in 1..=steps {
                out.push(w[0] + (w[1] - w[0]) * s as f64 / steps as f64);
            }
        }
        out
    }

    fn params() -> Params {
        Params {
            min_avg_turnover: 0.0,
            ..Params::default()
        }
    }

    #[test]
    fn zigzag_finds_swings_and_labels() {
        // 100 -> 120 -> 108 -> 130 -> 115 -> 140
        let bars = path(&legs(&[100.0, 120.0, 108.0, 130.0, 115.0, 140.0], 5));
        let z = zigzag(&bars, 5.0);
        let kinds: Vec<_> = z.swings.iter().map(|s| (s.kind, s.label)).collect();
        assert_eq!(
            kinds,
            vec![
                (SwingKind::Low, None),
                (SwingKind::High, None),
                (SwingKind::Low, Some(SwingLabel::HL)),
                (SwingKind::High, Some(SwingLabel::HH)),
                (SwingKind::Low, Some(SwingLabel::HL)),
            ]
        );
        assert_eq!(z.leg_up, Some(true));
        assert_eq!(trend_of(&z.swings), Trend::Up);
    }

    #[test]
    fn small_moves_are_ignored() {
        // 3% wiggles never make a 5% swing.
        let bars = path(&legs(&[100.0, 103.0, 100.0, 103.0, 100.0], 4));
        assert!(zigzag(&bars, 5.0).swings.is_empty());
    }

    #[test]
    fn downtrend_detected() {
        let bars = path(&legs(&[140.0, 120.0, 132.0, 110.0, 121.0, 100.0], 5));
        assert_eq!(trend_of(&zigzag(&bars, 5.0).swings), Trend::Down);
    }

    #[test]
    fn higher_low_entry_with_plan() {
        // Uptrend, then a pullback from 140 toward 126 (above the 115 low).
        let mut c = legs(&[100.0, 120.0, 108.0, 130.0, 115.0, 140.0], 5);
        c.extend(legs(&[140.0, 126.0], 7).into_iter().skip(1));
        let bars = path(&c);
        let p = params();
        let s = analyze(&bars, &p).unwrap();
        assert_eq!(s.trend, Trend::Up);
        assert_eq!(s.leg_up, Some(false));
        let low = s.leg_extreme.unwrap();
        // Before the bounce: a Get ready / waiting setup.
        let v = evaluate(&s, None, &p, &SignalKind::STRUCTURE);
        let hl = v
            .setups
            .iter()
            .find(|x| x.kind == SignalKind::HigherLowEntry)
            .unwrap();
        assert!(!hl.triggered);
        assert!((hl.trigger - low * (1.0 + 0.5 * s.threshold_pct / 100.0)).abs() < 1e-9);
        assert!((hl.plan.target - s.last_high.unwrap()).abs() < 1e-9);
        assert!(hl.plan.stop < low);
        // A live bounce through the trigger: Look now.
        let live = Live {
            price: hl.trigger + 0.1,
            day_low: low,
            volume: 1e6,
            session_fraction: 0.5,
        };
        let v = evaluate(&s, Some(&live), &p, &SignalKind::STRUCTURE);
        assert!(v
            .setups
            .iter()
            .any(|x| x.kind == SignalKind::HigherLowEntry && x.triggered));
        assert_eq!(v.status, Status::LookNow);
    }

    #[test]
    fn uptrend_confirmed_after_higher_low() {
        // Range then higher low: 100 -> 120 -> 105 -> 118 -> 109 (HL) -> rising.
        let mut c = legs(&[100.0, 120.0, 105.0, 118.0, 109.0, 117.0], 6);
        c.extend([117.5, 118.0]);
        let bars = path(&c);
        let p = params();
        let s = analyze(&bars, &p).unwrap();
        assert_eq!(s.leg_up, Some(true));
        let v = evaluate(&s, None, &p, &SignalKind::STRUCTURE);
        let up = v
            .setups
            .iter()
            .find(|x| x.kind == SignalKind::UptrendConfirmed)
            .unwrap();
        let h2 = s.last_high.unwrap();
        assert!((up.trigger - h2 * 1.01).abs() < 1e-9);
        let live = Live {
            price: up.trigger,
            day_low: 117.0,
            volume: 1e6,
            session_fraction: 1.0,
        };
        assert!(evaluate(&s, Some(&live), &p, &SignalKind::STRUCTURE)
            .setups
            .iter()
            .any(|x| x.kind == SignalKind::UptrendConfirmed && x.triggered));
    }

    #[test]
    fn downtrend_broken_and_disabled_kinds() {
        let mut c = legs(&[140.0, 120.0, 132.0, 110.0, 121.0, 100.0, 106.0], 5);
        c.push(106.5);
        let bars = path(&c);
        let p = params();
        let s = analyze(&bars, &p).unwrap();
        assert_eq!(s.trend, Trend::Down);
        let v = evaluate(&s, None, &p, &SignalKind::STRUCTURE);
        assert!(v
            .setups
            .iter()
            .any(|x| x.kind == SignalKind::DowntrendBroken));
        let only = [SignalKind::HigherLowEntry, SignalKind::UptrendConfirmed];
        let v = evaluate(&s, None, &p, &only);
        assert!(v
            .setups
            .iter()
            .all(|x| x.kind != SignalKind::DowntrendBroken));
        assert_eq!(v.status, Status::Downtrend);
    }

    #[test]
    fn get_ready_when_near_and_reward_risk_filter() {
        let mut c = legs(&[100.0, 120.0, 105.0, 118.0, 109.0, 117.0], 6);
        c.extend([117.5, 118.0]);
        let bars = path(&c);
        let p = params();
        let s = analyze(&bars, &p).unwrap();
        let trigger = s.last_high.unwrap() * 1.01;
        let near = Live {
            price: trigger * 0.99,
            day_low: 117.0,
            volume: 1e6,
            session_fraction: 1.0,
        };
        assert_eq!(
            evaluate(&s, Some(&near), &p, &SignalKind::STRUCTURE).status,
            Status::GetReady
        );
        // Demanding a 10x reward/risk rejects the setup.
        let strict = Params {
            min_reward_risk: 10.0,
            ..params()
        };
        let v = evaluate(&s, Some(&near), &strict, &SignalKind::STRUCTURE);
        assert!(v.setups.iter().all(|x| !x.plan.passes));
        assert_ne!(v.status, Status::GetReady);
    }

    #[test]
    fn thin_stocks_are_marked() {
        let bars: Vec<Bar> = path(&legs(&[100.0, 120.0, 108.0, 130.0], 10))
            .into_iter()
            .map(|b| Bar { volume: 1.0, ..b })
            .collect();
        let s = analyze(&bars, &Params::default()).unwrap();
        assert_eq!(
            evaluate(&s, None, &Params::default(), &SignalKind::STRUCTURE).status,
            Status::Thin
        );
    }

    #[test]
    fn weekly_bars_group_by_week() {
        // 14 consecutive days starting on a Monday (day 4 since epoch).
        let bars: Vec<Bar> = (4..18)
            .map(|d| Bar {
                time: d * DAY,
                close: d as f64,
                high: d as f64 + 1.0,
                low: d as f64 - 1.0,
                volume: 1.0,
            })
            .collect();
        let w = weekly_bars(&bars);
        assert_eq!(w.len(), 2);
        assert_eq!(
            (w[0].low, w[0].high, w[0].close, w[0].volume),
            (3.0, 11.0, 10.0, 7.0)
        );
    }

    #[test]
    fn track_record_counts_once_per_swing() {
        let mut c = legs(
            &[
                100.0, 120.0, 108.0, 130.0, 115.0, 140.0, 125.0, 150.0, 135.0, 165.0,
            ],
            8,
        );
        c.extend(std::iter::repeat_n(165.0, 15));
        let bars = path(&c);
        let r = track_record(&bars, &params(), &SignalKind::STRUCTURE);
        let hl = r
            .iter()
            .find(|x| x.kind == SignalKind::HigherLowEntry)
            .unwrap();
        assert!(hl.fired >= 1, "{r:?}");
        assert!(hl.wins >= 1);
        assert!(hl.fired <= 4);
    }
}
