//! Signals on JKH.N0000's real one-year daily history (captured from the CSE).

use cse_signals::{daily_levels, evaluate_daily, track_record, Bar, Live, Params, SignalKind};
use serde_json::Value;

fn jkh_bars() -> Vec<Bar> {
    let path = format!(
        "{}/../cse-client/tests/fixtures/stockChart-1Y.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut bars: Vec<Bar> = v["chartData"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| {
            let close = r["p"].as_f64()?;
            Some(Bar {
                time: r["t"].as_i64()? / 1000,
                close,
                high: r["h"].as_f64().unwrap_or(close),
                low: r["l"].as_f64().unwrap_or(close),
                volume: r["q"].as_f64().unwrap_or(0.0),
            })
        })
        .collect();
    bars.sort_by_key(|b| b.time);
    bars
}

#[test]
fn levels_from_real_history_are_sane() {
    let bars = jkh_bars();
    assert_eq!(bars.len(), 241);
    let p = Params::default();
    let l = daily_levels(&bars, &p).unwrap();
    assert_eq!(l.last_close, 18.0);
    assert!(
        l.liquid,
        "JKH trades tens of millions a day: {}",
        l.avg_turnover
    );
    // Breakout level sits above the recent closes and below the year high.
    assert!(
        l.breakout_level > 18.0 && l.breakout_level < 23.5,
        "{}",
        l.breakout_level
    );
    assert!(l.rsi > 0.0 && l.rsi < 100.0);
    // JKH has been falling, so it is not in an uptrend.
    assert!(!l.uptrend);
    assert!(l.pullback_level.is_none());

    // Friday's actual figures should not trigger a breakout.
    let live = Live {
        price: 18.0,
        day_low: 17.9,
        volume: 1_186_254.0,
        session_fraction: 1.0,
    };
    assert!(!evaluate_daily(&l, &live, &p)
        .iter()
        .any(|t| t.kind == SignalKind::Breakout));
}

#[test]
fn track_record_runs_on_real_history() {
    let bars = jkh_bars();
    let r = track_record(&bars, &Params::default());
    assert_eq!(r.len(), 4);
    for t in &r {
        assert!(t.wins <= t.fired);
        assert!(t.fired < 60, "{:?} fired implausibly often", t.kind);
        assert!((0.0..=100.0).contains(&t.win_rate_pct));
        println!(
            "{:?}: fired {}, wins {} ({:.0}%), avg {:+.2}%",
            t.kind, t.fired, t.wins, t.win_rate_pct, t.avg_return_pct
        );
    }
}

#[test]
fn structure_on_real_history() {
    use cse_signals::structure::{analyze, evaluate, track_record, Trend};
    let bars = jkh_bars();
    let p = Params::default();
    let s = analyze(&bars, &p).unwrap();
    println!(
        "JKH threshold {:.1}% trend {:?} weekly {:?} swings {:?}",
        s.threshold_pct,
        s.trend,
        s.weekly_trend,
        s.swings
            .iter()
            .map(|w| (w.price, w.label))
            .collect::<Vec<_>>()
    );
    // JKH fell from ~23 to 18 over the year: not an uptrend.
    assert_ne!(s.trend, Trend::Up);
    assert!(s.threshold_pct >= 5.0);
    let v = evaluate(&s, None, &p, &SignalKind::STRUCTURE);
    println!("status {:?} setups {:?}", v.status, v.setups);
    for r in track_record(&bars, &p, &SignalKind::STRUCTURE) {
        assert!(r.wins <= r.fired && r.fired < 40);
        println!(
            "{:?}: fired {} wins {} avg {:+.2}%",
            r.kind, r.fired, r.wins, r.avg_return_pct
        );
    }
}
