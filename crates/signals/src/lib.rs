//! Buy-side technical signals for CSE stocks, turned into price alert levels.
//!
//! Daily signals (from daily closes, highs, lows and volume — the CSE does
//! not publish daily opens):
//! - **Breakout**: price clears the prior 20-day high on strong volume.
//! - **Pullback**: in an uptrend, price dips to the 20-day average and holds.
//! - **Trend turn**: the 20-day average crosses above the 50-day average.
//! - **Oversold bounce**: RSI(14) recovers above 30.
//!
//! Intraday signals (from today's trades, 15-minute bars):
//! - **Opening range breakout**: price clears the 09:30–10:00 high.
//! - **Volume spike**: a 15-minute bar trades several times the usual volume
//!   while rising.
//!
//! Each daily signal is expressed as a price level so a live price can be
//! checked directly, and each has a one-year track record computed from the
//! same rules. Signals are prompts to look at a stock, not advice.

pub mod indicators;
pub mod structure;

use indicators::{prior_max, prior_mean, rsi, sma, RsiState};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SignalKind {
    HigherLowEntry,
    UptrendConfirmed,
    DowntrendBroken,
    Breakout,
    Pullback,
    TrendTurn,
    OversoldBounce,
    OpeningRange,
    VolumeSpike,
}

impl SignalKind {
    /// Market-structure setups (see [`structure`]).
    pub const STRUCTURE: [SignalKind; 3] = [
        SignalKind::HigherLowEntry,
        SignalKind::UptrendConfirmed,
        SignalKind::DowntrendBroken,
    ];
    pub const DAILY: [SignalKind; 4] = [
        SignalKind::Breakout,
        SignalKind::Pullback,
        SignalKind::TrendTurn,
        SignalKind::OversoldBounce,
    ];
    pub const INTRADAY: [SignalKind; 2] = [SignalKind::OpeningRange, SignalKind::VolumeSpike];

    pub fn label(self) -> &'static str {
        match self {
            SignalKind::HigherLowEntry => "Higher low entry",
            SignalKind::UptrendConfirmed => "Uptrend confirmed",
            SignalKind::DowntrendBroken => "Downtrend broken",
            SignalKind::Breakout => "Breakout",
            SignalKind::Pullback => "Pullback in uptrend",
            SignalKind::TrendTurn => "Trend turn (20/50 cross)",
            SignalKind::OversoldBounce => "Oversold bounce",
            SignalKind::OpeningRange => "Opening range breakout",
            SignalKind::VolumeSpike => "Volume spike",
        }
    }

    pub fn is_intraday(self) -> bool {
        SignalKind::INTRADAY.contains(&self)
    }
}

/// One period of price data. `time` is Unix seconds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bar {
    pub time: i64,
    pub close: f64,
    pub high: f64,
    pub low: f64,
    pub volume: f64,
}

/// Tunable strategy settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Params {
    pub breakout_lookback: usize,
    pub volume_mult: f64,
    pub sma_fast: usize,
    pub sma_slow: usize,
    pub rsi_period: usize,
    pub rsi_oversold: f64,
    /// Percent above a level a price must reach to count as a cross.
    pub buffer_pct: f64,
    /// Minimum average daily turnover (Rs.) for any signal.
    pub min_avg_turnover: f64,
    /// Trading days before the same signal may fire again.
    pub cooldown_days: u32,
    /// Trading days after a signal used to judge its outcome.
    pub horizon_days: usize,
    /// Buy plus sell fees, percent.
    pub round_trip_fee_pct: f64,
    /// Skip intraday signals for stocks whose typical daily range is
    /// smaller than the round-trip fees.
    pub fee_aware: bool,
    pub opening_range_minutes: i64,
    pub spike_mult: f64,
    /// Trend-turn levels further than this (percent above the last close)
    /// are unreachable in a session and are not offered.
    pub max_level_distance_pct: f64,
    /// Smallest reversal (percent) that counts as a swing.
    pub min_swing_pct: f64,
    /// Swing threshold as a multiple of the typical daily range, if larger.
    pub swing_range_mult: f64,
    /// Smallest weekly swing, percent.
    pub weekly_min_swing_pct: f64,
    /// Minimum potential gain / risk for structure setups.
    pub min_reward_risk: f64,
    /// "Get ready" when price is within this percent below a trigger.
    pub near_pct: f64,
    /// Stop sits this percent below the swing low.
    pub stop_buffer_pct: f64,
    /// Bounce off a pullback low, as a share of the swing threshold, that
    /// confirms a higher low.
    pub bounce_frac: f64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            breakout_lookback: 20,
            volume_mult: 1.5,
            sma_fast: 20,
            sma_slow: 50,
            rsi_period: 14,
            rsi_oversold: 30.0,
            buffer_pct: 1.0,
            min_avg_turnover: 1_000_000.0,
            cooldown_days: 5,
            horizon_days: 10,
            round_trip_fee_pct: 2.24,
            fee_aware: true,
            opening_range_minutes: 30,
            spike_mult: 3.0,
            max_level_distance_pct: 10.0,
            min_swing_pct: 5.0,
            swing_range_mult: 2.0,
            weekly_min_swing_pct: 10.0,
            min_reward_risk: 2.0,
            near_pct: 2.0,
            stop_buffer_pct: 1.0,
            bounce_frac: 0.5,
        }
    }
}

/// Alert levels for the next session, computed from completed daily bars.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DailyLevels {
    /// Time of the last completed bar.
    pub as_of: i64,
    pub last_close: f64,
    pub avg_volume: f64,
    pub avg_turnover: f64,
    /// Average (high - low) / close over the lookback, percent.
    pub avg_range_pct: f64,
    /// Enough turnover for signals to be meaningful.
    pub liquid: bool,
    /// Price that confirms a breakout (prior high plus buffer).
    pub breakout_level: f64,
    /// Full-day volume a breakout should reach.
    pub breakout_volume: f64,
    pub sma_fast: f64,
    pub sma_slow: f64,
    pub uptrend: bool,
    /// The 20-day average while in an uptrend.
    pub pullback_level: Option<f64>,
    /// Price at which the fast average would cross above the slow one today.
    pub trend_turn_level: Option<f64>,
    pub rsi: f64,
    /// Price at which RSI would recover above the oversold line today.
    pub oversold_level: Option<f64>,
    #[serde(skip)]
    rsi_state: Option<RsiState>,
}

/// Today's live figures for one stock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Live {
    pub price: f64,
    pub day_low: f64,
    /// Shares traded so far today.
    pub volume: f64,
    /// Share of the trading session elapsed, 0 to 1.
    pub session_fraction: f64,
}

/// A signal that has fired.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Trigger {
    pub kind: SignalKind,
    pub level: f64,
    pub message: String,
}

fn closes(bars: &[Bar]) -> Vec<f64> {
    bars.iter().map(|b| b.close).collect()
}

/// Levels from completed daily bars (oldest first). Needs at least
/// `sma_slow` bars.
pub fn daily_levels(bars: &[Bar], p: &Params) -> Option<DailyLevels> {
    let n = bars.len();
    if n < p.sma_slow.max(p.breakout_lookback).max(p.rsi_period + 1) {
        return None;
    }
    let c = closes(bars);
    let last = &bars[n - 1];
    let look = &bars[n - p.breakout_lookback..];
    let prior_high = look.iter().map(|b| b.high).fold(f64::MIN, f64::max);
    let avg_volume = look.iter().map(|b| b.volume).sum::<f64>() / look.len() as f64;
    let avg_turnover = look.iter().map(|b| b.volume * b.close).sum::<f64>() / look.len() as f64;
    let avg_range_pct = look
        .iter()
        .map(|b| {
            if b.close > 0.0 {
                (b.high - b.low) / b.close * 100.0
            } else {
                0.0
            }
        })
        .sum::<f64>()
        / look.len() as f64;

    let fast = c[n - p.sma_fast..].iter().sum::<f64>() / p.sma_fast as f64;
    let slow = c[n - p.sma_slow..].iter().sum::<f64>() / p.sma_slow as f64;
    let uptrend = last.close > slow && fast > slow;
    let buf = 1.0 + p.buffer_pct / 100.0;

    // Solve (S_f + x) / f > (S_s + x) / s for x, where S_* are the sums of
    // the latest f-1 and s-1 closes (today's price x completes each window).
    let trend_turn_level = (fast < slow)
        .then(|| {
            let (f, s) = (p.sma_fast as f64, p.sma_slow as f64);
            let sf: f64 = c[n - (p.sma_fast - 1)..].iter().sum();
            let ss: f64 = c[n - (p.sma_slow - 1)..].iter().sum();
            (ss / s - sf / f) / (1.0 / f - 1.0 / s)
        })
        .filter(|x| *x > 0.0)
        .map(|x| x * buf)
        .filter(|x| *x <= last.close * (1.0 + p.max_level_distance_pct / 100.0));

    let (_, rsi_state) = rsi(&c, p.rsi_period);
    let rsi_now = rsi_state.map_or(50.0, |s| s.value());
    let oversold_level = rsi_state
        .filter(|s| s.value() < p.rsi_oversold)
        .and_then(|s| price_for_rsi(&s, p.rsi_oversold));

    Some(DailyLevels {
        as_of: last.time,
        last_close: last.close,
        avg_volume,
        avg_turnover,
        avg_range_pct,
        liquid: avg_turnover >= p.min_avg_turnover,
        breakout_level: prior_high * buf,
        breakout_volume: avg_volume * p.volume_mult,
        sma_fast: fast,
        sma_slow: slow,
        uptrend,
        pullback_level: uptrend.then_some(fast),
        trend_turn_level,
        rsi: rsi_now,
        oversold_level,
        rsi_state,
    })
}

/// Lowest price at which RSI would reach `target` on the next bar.
fn price_for_rsi(s: &RsiState, target: f64) -> Option<f64> {
    let (mut lo, mut hi) = (s.last_close, s.last_close * 3.0);
    if s.with_price(hi) < target {
        return None;
    }
    for _ in 0..60 {
        let mid = (lo + hi) / 2.0;
        if s.with_price(mid) >= target {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    Some(hi)
}

/// Daily signals that the live price confirms. Disabled for illiquid stocks.
pub fn evaluate_daily(l: &DailyLevels, live: &Live, p: &Params) -> Vec<Trigger> {
    let mut out = Vec::new();
    if !l.liquid || live.price <= 0.0 {
        return out;
    }
    let buf = 1.0 + p.buffer_pct / 100.0;
    // Volume is judged against the share of the day that has passed.
    let needed_volume = l.breakout_volume * live.session_fraction.clamp(0.25, 1.0);
    if live.price >= l.breakout_level && live.volume >= needed_volume {
        out.push(Trigger {
            kind: SignalKind::Breakout,
            level: l.breakout_level,
            message: format!(
                "Broke above {:.2} (20-day high) on {:.1}x usual volume",
                l.breakout_level,
                if l.avg_volume > 0.0 {
                    live.volume / (l.avg_volume * live.session_fraction.clamp(0.25, 1.0))
                } else {
                    0.0
                }
            ),
        });
    }
    if let Some(level) = l.pullback_level {
        if live.day_low <= level * buf && live.price > level {
            out.push(Trigger {
                kind: SignalKind::Pullback,
                level,
                message: format!("Dipped to the 20-day average {level:.2} in an uptrend and held"),
            });
        }
    }
    if let Some(level) = l.trend_turn_level {
        if live.price >= level {
            out.push(Trigger {
                kind: SignalKind::TrendTurn,
                level,
                message: format!("20-day average crossing above 50-day at {level:.2}"),
            });
        }
    }
    if let Some(level) = l.oversold_level {
        if live.price >= level {
            let now = l.rsi_state.map_or(0.0, |s| s.with_price(live.price));
            out.push(Trigger {
                kind: SignalKind::OversoldBounce,
                level,
                message: format!(
                    "RSI recovering above {:.0} (now {now:.0}) from oversold",
                    p.rsi_oversold
                ),
            });
        }
    }
    out
}

/// How one signal performed historically.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TrackRecord {
    pub kind: SignalKind,
    pub fired: usize,
    /// Signals whose return after `horizon_days` beat the round-trip fees.
    pub wins: usize,
    pub win_rate_pct: f64,
    pub avg_return_pct: f64,
    pub last_fired: Option<i64>,
    pub horizon_days: usize,
}

/// Replays the daily rules over history (close-based, same filters and
/// cooldown) and measures each signal's return `horizon_days` later.
pub fn track_record(bars: &[Bar], p: &Params) -> Vec<TrackRecord> {
    let n = bars.len();
    let c = closes(bars);
    let highs: Vec<f64> = bars.iter().map(|b| b.high).collect();
    let vols: Vec<f64> = bars.iter().map(|b| b.volume).collect();
    let turnover: Vec<f64> = bars.iter().map(|b| b.volume * b.close).collect();
    let fast = sma(&c, p.sma_fast);
    let slow = sma(&c, p.sma_slow);
    let (rsi_v, _) = rsi(&c, p.rsi_period);
    let pmax = prior_max(&highs, p.breakout_lookback);
    let pvol = prior_mean(&vols, p.breakout_lookback);
    let pturn = prior_mean(&turnover, p.breakout_lookback);
    let buf = 1.0 + p.buffer_pct / 100.0;

    SignalKind::DAILY
        .iter()
        .map(|&kind| {
            let mut returns = Vec::new();
            let mut last_fired: Option<usize> = None;
            for i in 1..n {
                let liquid = pturn[i].is_some_and(|t| t >= p.min_avg_turnover);
                let fires = liquid
                    && match kind {
                        SignalKind::Breakout => matches!((pmax[i], pvol[i]), (Some(h), Some(v))
                            if c[i] >= h * buf && vols[i] >= v * p.volume_mult),
                        SignalKind::Pullback => {
                            matches!((fast[i - 1], slow[i - 1]), (Some(f), Some(s))
                            if c[i - 1] > s && f > s && bars[i].low <= f * buf && c[i] > f)
                        }
                        SignalKind::TrendTurn => matches!(
                            (fast[i - 1], slow[i - 1], fast[i], slow[i]),
                            (Some(f0), Some(s0), Some(f1), Some(s1)) if f0 <= s0 && f1 > s1),
                        SignalKind::OversoldBounce => {
                            matches!((rsi_v[i - 1], rsi_v[i]), (Some(a), Some(b))
                            if a < p.rsi_oversold && b >= p.rsi_oversold)
                        }
                        _ => false,
                    };
                let cooled = last_fired.is_none_or(|j| i - j > p.cooldown_days as usize);
                if fires && cooled {
                    last_fired = Some(i);
                    if i + p.horizon_days < n && c[i] > 0.0 {
                        returns.push((c[i + p.horizon_days] / c[i] - 1.0) * 100.0);
                    }
                }
            }
            let wins = returns
                .iter()
                .filter(|r| **r > p.round_trip_fee_pct)
                .count();
            let fired = returns.len();
            TrackRecord {
                kind,
                fired,
                wins,
                win_rate_pct: if fired > 0 {
                    wins as f64 * 100.0 / fired as f64
                } else {
                    0.0
                },
                avg_return_pct: if fired > 0 {
                    returns.iter().sum::<f64>() / fired as f64
                } else {
                    0.0
                },
                last_fired: last_fired.map(|i| bars[i].time),
                horizon_days: p.horizon_days,
            }
        })
        .collect()
}

/// Intraday state shown on the Alerts screen.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IntradayLevels {
    pub opening_high: Option<f64>,
    pub opening_complete: bool,
    pub breakout_level: Option<f64>,
    pub avg_bar_volume: f64,
    /// False when fee-aware filtering disables intraday signals.
    pub moves_cover_fees: bool,
}

/// Buckets trades `(unix secs, price, qty)` into bars of `minutes` from
/// `session_open`.
pub fn bars_from_trades(trades: &[(i64, f64, f64)], session_open: i64, minutes: i64) -> Vec<Bar> {
    let width = minutes * 60;
    let mut bars: Vec<Bar> = Vec::new();
    for &(t, price, qty) in trades.iter().filter(|(t, ..)| *t >= session_open) {
        let start = session_open + (t - session_open) / width * width;
        match bars.last_mut() {
            Some(b) if b.time == start => {
                b.close = price;
                b.high = b.high.max(price);
                b.low = b.low.min(price);
                b.volume += qty;
            }
            _ => bars.push(Bar {
                time: start,
                close: price,
                high: price,
                low: price,
                volume: qty,
            }),
        }
    }
    bars
}

/// Intraday signals from today's trades (oldest first). `daily` supplies
/// the usual volume and liquidity; a 5-hour session has 20 15-minute bars.
pub fn evaluate_intraday(
    trades: &[(i64, f64, f64)],
    session_open: i64,
    now: i64,
    daily: &DailyLevels,
    p: &Params,
) -> (IntradayLevels, Vec<Trigger>) {
    const BAR_MINUTES: i64 = 15;
    const BARS_PER_SESSION: f64 = 20.0;
    let or_end = session_open + p.opening_range_minutes * 60;
    let opening_high = trades
        .iter()
        .filter(|(t, ..)| *t >= session_open && *t < or_end)
        .map(|(_, price, _)| *price)
        .reduce(f64::max);
    let buf = 1.0 + p.buffer_pct / 100.0;
    let avg_bar_volume = daily.avg_volume / BARS_PER_SESSION;
    let moves_cover_fees = !p.fee_aware || daily.avg_range_pct >= p.round_trip_fee_pct;
    let levels = IntradayLevels {
        opening_high,
        opening_complete: now >= or_end,
        breakout_level: opening_high.map(|h| h * buf),
        avg_bar_volume,
        moves_cover_fees,
    };
    let mut out = Vec::new();
    if !daily.liquid || !moves_cover_fees {
        return (levels, out);
    }
    let last_price = trades.last().map(|t| t.1);
    if let (Some(level), Some(price), true) =
        (levels.breakout_level, last_price, levels.opening_complete)
    {
        let after: f64 = trades
            .iter()
            .filter(|(t, ..)| *t >= or_end)
            .map(|t| t.2)
            .sum();
        let elapsed_bars = ((now - or_end) as f64 / (BAR_MINUTES * 60) as f64).max(1.0);
        if price >= level && after >= avg_bar_volume * elapsed_bars * p.volume_mult.min(1.5) {
            out.push(Trigger {
                kind: SignalKind::OpeningRange,
                level,
                message: format!("Broke above the opening-range high {level:.2} with volume"),
            });
        }
    }
    let bars = bars_from_trades(trades, session_open, BAR_MINUTES);
    // The latest completed bar, compared with the bar before it.
    let completed: Vec<&Bar> = bars
        .iter()
        .filter(|b| b.time + BAR_MINUTES * 60 <= now)
        .collect();
    if let Some(&b) = completed.last() {
        let prev_close = completed.len().checked_sub(2).map(|i| completed[i].close);
        let rising = prev_close.map_or(b.close >= b.low && b.close > b.low, |pc| b.close > pc);
        if avg_bar_volume > 0.0 && b.volume >= avg_bar_volume * p.spike_mult && rising {
            out.push(Trigger {
                kind: SignalKind::VolumeSpike,
                level: b.close,
                message: format!(
                    "{:.1}x usual 15-minute volume while rising to {:.2}",
                    b.volume / avg_bar_volume,
                    b.close
                ),
            });
        }
    }
    (levels, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400;

    fn bar(i: usize, close: f64, volume: f64) -> Bar {
        Bar {
            time: i as i64 * DAY,
            close,
            high: close * 1.01,
            low: close * 0.99,
            volume,
        }
    }

    fn params() -> Params {
        Params {
            min_avg_turnover: 0.0,
            ..Params::default()
        }
    }

    fn live(price: f64, day_low: f64, volume: f64) -> Live {
        Live {
            price,
            day_low,
            volume,
            session_fraction: 1.0,
        }
    }

    #[test]
    fn needs_enough_history() {
        let bars: Vec<Bar> = (0..30).map(|i| bar(i, 10.0, 1000.0)).collect();
        assert!(daily_levels(&bars, &params()).is_none());
    }

    #[test]
    fn breakout_needs_price_and_volume() {
        let bars: Vec<Bar> = (0..60).map(|i| bar(i, 10.0, 1000.0)).collect();
        let l = daily_levels(&bars, &params()).unwrap();
        // Prior high 10.10 plus 1% buffer.
        assert!((l.breakout_level - 10.201).abs() < 1e-9);
        assert_eq!(l.breakout_volume, 1500.0);
        let p = params();
        assert!(evaluate_daily(&l, &live(10.30, 10.0, 1000.0), &p).is_empty());
        let t = evaluate_daily(&l, &live(10.30, 10.0, 2000.0), &p);
        assert_eq!(t[0].kind, SignalKind::Breakout);
        // Halfway through the day, half the volume is enough.
        let half = Live {
            session_fraction: 0.5,
            ..live(10.30, 10.0, 800.0)
        };
        assert_eq!(evaluate_daily(&l, &half, &p)[0].kind, SignalKind::Breakout);
    }

    #[test]
    fn pullback_in_uptrend() {
        // Steady rise: uptrend with the 20-day average below price.
        let bars: Vec<Bar> = (0..60)
            .map(|i| bar(i, 10.0 + i as f64 * 0.1, 1000.0))
            .collect();
        let l = daily_levels(&bars, &params()).unwrap();
        assert!(l.uptrend);
        let level = l.pullback_level.unwrap();
        let p = params();
        let t = evaluate_daily(&l, &live(level + 0.2, level - 0.05, 500.0), &p);
        assert!(t.iter().any(|t| t.kind == SignalKind::Pullback));
        // Did not dip near the average: no pullback.
        let t = evaluate_daily(&l, &live(level + 1.0, level + 0.8, 500.0), &p);
        assert!(!t.iter().any(|t| t.kind == SignalKind::Pullback));
    }

    #[test]
    fn trend_turn_level_is_the_crossing_price() {
        // Long decline then flat: fast average below slow.
        let mut bars: Vec<Bar> = (0..40)
            .map(|i| bar(i, 20.0 - i as f64 * 0.2, 1000.0))
            .collect();
        bars.extend((40..60).map(|i| bar(i, 12.0, 1000.0)));
        let p = Params {
            buffer_pct: 0.0,
            max_level_distance_pct: 1000.0,
            ..params()
        };
        let l = daily_levels(&bars, &p).unwrap();
        let x = l.trend_turn_level.unwrap();
        // With the default 10% cap this far-off level is not offered.
        assert!(x > 12.0 * 1.1);
        assert!(daily_levels(&bars, &params())
            .unwrap()
            .trend_turn_level
            .is_none());
        // Appending x makes the averages equal; a little above crosses.
        let mut c: Vec<f64> = bars.iter().map(|b| b.close).collect();
        c.push(x + 0.01);
        let f = c[c.len() - 20..].iter().sum::<f64>() / 20.0;
        let s = c[c.len() - 50..].iter().sum::<f64>() / 50.0;
        assert!(f > s);
        assert_eq!(
            evaluate_daily(&l, &live(x + 0.01, 11.0, 1000.0), &p)[0].kind,
            SignalKind::TrendTurn
        );
    }

    #[test]
    fn oversold_level_brings_rsi_back_to_30() {
        let bars: Vec<Bar> = (0..60)
            .map(|i| bar(i, 30.0 - i as f64 * 0.3, 1000.0))
            .collect();
        let p = params();
        let l = daily_levels(&bars, &p).unwrap();
        assert!(l.rsi < 30.0);
        let x = l.oversold_level.unwrap();
        assert!(x > l.last_close);
        assert!((l.rsi_state.unwrap().with_price(x) - 30.0).abs() < 0.01);
        let t = evaluate_daily(&l, &live(x + 0.01, l.last_close, 1000.0), &p);
        assert!(t.iter().any(|t| t.kind == SignalKind::OversoldBounce));
    }

    #[test]
    fn illiquid_stocks_never_signal() {
        let bars: Vec<Bar> = (0..60).map(|i| bar(i, 10.0, 10.0)).collect();
        let l = daily_levels(&bars, &Params::default()).unwrap();
        assert!(!l.liquid);
        assert!(evaluate_daily(&l, &live(50.0, 1.0, 1e9), &Params::default()).is_empty());
    }

    #[test]
    fn track_record_counts_breakouts_with_cooldown() {
        // Flat, then a jump on big volume at day 60, then flat higher.
        let mut bars: Vec<Bar> = (0..60).map(|i| bar(i, 10.0, 1000.0)).collect();
        bars.extend((60..90).map(|i| {
            bar(
                i,
                if i == 60 { 11.0 } else { 11.5 },
                if i == 60 { 5000.0 } else { 1000.0 },
            )
        }));
        let r = track_record(&bars, &params());
        let b = r.iter().find(|r| r.kind == SignalKind::Breakout).unwrap();
        assert_eq!(b.fired, 1);
        assert_eq!(b.wins, 1); // 11.0 -> 11.5 is +4.5%, above 2.24% fees
        assert!((b.avg_return_pct - 4.545).abs() < 0.01);
        assert_eq!(b.last_fired, Some(60 * DAY));
    }

    #[test]
    fn bars_from_trades_buckets_by_15_minutes() {
        let open = 1000;
        let trades = [
            (1000, 10.0, 5.0),
            (1500, 10.2, 5.0),
            (1900, 9.9, 1.0),
            (2000, 10.1, 2.0),
        ];
        let b = bars_from_trades(&trades, open, 15);
        assert_eq!(b.len(), 2);
        assert_eq!((b[0].close, b[0].high, b[0].volume), (10.2, 10.2, 10.0));
        assert_eq!(
            (b[1].time, b[1].low, b[1].close, b[1].volume),
            (1900, 9.9, 10.1, 3.0)
        );
    }

    #[test]
    fn intraday_opening_range_and_volume_spike() {
        // Wide-ranging liquid stock so the fee filter allows intraday alerts.
        let bars: Vec<Bar> = (0..60)
            .map(|i| Bar {
                time: i * DAY,
                close: 10.0,
                high: 10.3,
                low: 9.8,
                volume: 20_000.0,
            })
            .collect();
        let p = params();
        let d = daily_levels(&bars, &p).unwrap();
        assert!(d.avg_range_pct > p.round_trip_fee_pct);
        let open = 0;
        let mut trades = vec![(60, 10.0, 100.0), (900, 10.2, 100.0)]; // opening range high 10.20
        trades.push((1800 + 60, 10.4, 1000.0)); // after 10:00, above 10.302
        trades.push((2700 + 60, 10.5, 4000.0)); // spike in the 10:15 bar
        let (lv, t) = evaluate_intraday(&trades, open, 3600 + 60, &d, &p);
        assert_eq!(lv.opening_high, Some(10.2));
        let kinds: Vec<_> = t.iter().map(|t| t.kind).collect();
        assert!(kinds.contains(&SignalKind::OpeningRange), "{kinds:?}");
        assert!(kinds.contains(&SignalKind::VolumeSpike), "{kinds:?}");
    }

    #[test]
    fn fee_aware_filter_blocks_narrow_stocks() {
        let bars: Vec<Bar> = (0..60).map(|i| bar(i, 10.0, 20_000.0)).collect(); // 2% range
        let p = params();
        let d = daily_levels(&bars, &p).unwrap();
        let trades = vec![(60, 10.0, 100.0), (1900, 11.0, 100_000.0)];
        let (lv, t) = evaluate_intraday(&trades, 0, 3600, &d, &p);
        assert!(!lv.moves_cover_fees);
        assert!(t.is_empty());
        let off = Params {
            fee_aware: false,
            ..p
        };
        assert!(!evaluate_intraday(&trades, 0, 3600, &d, &off).1.is_empty());
    }
}
