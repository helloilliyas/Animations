//! Basic indicators over price series. All return one value per input
//! element, `None` until enough data exists.

/// Simple moving average.
pub fn sma(values: &[f64], period: usize) -> Vec<Option<f64>> {
    let mut out = vec![None; values.len()];
    if period == 0 {
        return out;
    }
    let mut sum = 0.0;
    for (i, v) in values.iter().enumerate() {
        sum += v;
        if i >= period {
            sum -= values[i - period];
        }
        if i + 1 >= period {
            out[i] = Some(sum / period as f64);
        }
    }
    out
}

/// Wilder's smoothed averages behind RSI, kept so a live price can be
/// applied without recomputing the whole series.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RsiState {
    pub avg_gain: f64,
    pub avg_loss: f64,
    pub last_close: f64,
    pub period: usize,
}

impl RsiState {
    pub fn value(&self) -> f64 {
        rsi_from(self.avg_gain, self.avg_loss)
    }

    /// RSI if the next close were `price`.
    pub fn with_price(&self, price: f64) -> f64 {
        let n = self.period as f64;
        let change = price - self.last_close;
        let gain = (self.avg_gain * (n - 1.0) + change.max(0.0)) / n;
        let loss = (self.avg_loss * (n - 1.0) + (-change).max(0.0)) / n;
        rsi_from(gain, loss)
    }
}

fn rsi_from(avg_gain: f64, avg_loss: f64) -> f64 {
    if avg_loss == 0.0 {
        if avg_gain == 0.0 {
            50.0
        } else {
            100.0
        }
    } else {
        100.0 - 100.0 / (1.0 + avg_gain / avg_loss)
    }
}

/// RSI series (Wilder) and the final smoothing state.
pub fn rsi(closes: &[f64], period: usize) -> (Vec<Option<f64>>, Option<RsiState>) {
    let mut out = vec![None; closes.len()];
    if period == 0 || closes.len() <= period {
        return (out, None);
    }
    let (mut gain, mut loss) = (0.0, 0.0);
    for i in 1..=period {
        let c = closes[i] - closes[i - 1];
        gain += c.max(0.0);
        loss += (-c).max(0.0);
    }
    let n = period as f64;
    let (mut avg_gain, mut avg_loss) = (gain / n, loss / n);
    out[period] = Some(rsi_from(avg_gain, avg_loss));
    for i in period + 1..closes.len() {
        let c = closes[i] - closes[i - 1];
        avg_gain = (avg_gain * (n - 1.0) + c.max(0.0)) / n;
        avg_loss = (avg_loss * (n - 1.0) + (-c).max(0.0)) / n;
        out[i] = Some(rsi_from(avg_gain, avg_loss));
    }
    let state = RsiState {
        avg_gain,
        avg_loss,
        last_close: *closes.last().unwrap(),
        period,
    };
    (out, Some(state))
}

/// Highest value of the `period` elements before each index (excluding it).
pub fn prior_max(values: &[f64], period: usize) -> Vec<Option<f64>> {
    (0..values.len())
        .map(|i| {
            (period > 0 && i >= period).then(|| {
                values[i - period..i]
                    .iter()
                    .cloned()
                    .fold(f64::MIN, f64::max)
            })
        })
        .collect()
}

/// Mean of the `period` elements before each index (excluding it).
pub fn prior_mean(values: &[f64], period: usize) -> Vec<Option<f64>> {
    (0..values.len())
        .map(|i| {
            (period > 0 && i >= period)
                .then(|| values[i - period..i].iter().sum::<f64>() / period as f64)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sma_values() {
        let s = sma(&[1.0, 2.0, 3.0, 4.0], 2);
        assert_eq!(s, vec![None, Some(1.5), Some(2.5), Some(3.5)]);
    }

    #[test]
    fn rsi_extremes_and_live_update() {
        let up: Vec<f64> = (0..20).map(|i| i as f64).collect();
        let (r, st) = rsi(&up, 14);
        assert_eq!(r[13], None);
        assert_eq!(r[19], Some(100.0));
        let st = st.unwrap();
        assert!(st.with_price(0.0) < 100.0);

        let flat = vec![5.0; 20];
        assert_eq!(rsi(&flat, 14).0[19], Some(50.0));
    }

    #[test]
    fn rsi_matches_known_value() {
        // Classic Wilder example closes; RSI(14) at index 14 is about 70.5.
        let c = [
            44.34, 44.09, 44.15, 43.61, 44.33, 44.83, 45.10, 45.42, 45.84, 46.08, 45.89, 46.03,
            45.61, 46.28, 46.28,
        ];
        let v = rsi(&c, 14).0[14].unwrap();
        assert!((v - 70.5).abs() < 0.6, "rsi {v}");
    }

    #[test]
    fn prior_windows_exclude_current() {
        let v = [1.0, 5.0, 2.0, 3.0];
        assert_eq!(prior_max(&v, 2), vec![None, None, Some(5.0), Some(5.0)]);
        assert_eq!(prior_mean(&v, 2), vec![None, None, Some(3.0), Some(3.5)]);
    }
}
