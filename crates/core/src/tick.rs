//! Minimum price steps (tick sizes) for order prices.
//!
//! ATrad rejects prices that are not a multiple of 0.10 or 0.25 depending on
//! the price band. The band boundary is configurable because the exchange can
//! change it.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Prices at or above `from` must be multiples of `tick`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TickBand {
    pub from: Decimal,
    pub tick: Decimal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TickTable {
    /// Sorted by `from`, first band starting at zero.
    bands: Vec<TickBand>,
}

impl Default for TickTable {
    /// 0.10 below Rs. 100 and 0.25 from Rs. 100 (to confirm against current
    /// CSE trading rules).
    fn default() -> Self {
        Self::new(vec![
            TickBand {
                from: Decimal::ZERO,
                tick: Decimal::new(10, 2),
            },
            TickBand {
                from: Decimal::ONE_HUNDRED,
                tick: Decimal::new(25, 2),
            },
        ])
    }
}

impl TickTable {
    /// Builds a table, sorting bands and forcing the first to start at zero.
    pub fn new(mut bands: Vec<TickBand>) -> Self {
        assert!(!bands.is_empty(), "tick table needs at least one band");
        assert!(
            bands.iter().all(|b| b.tick > Decimal::ZERO),
            "tick sizes must be positive"
        );
        bands.sort_by(|a, b| a.from.cmp(&b.from));
        bands[0].from = Decimal::ZERO;
        Self { bands }
    }

    pub fn bands(&self) -> &[TickBand] {
        &self.bands
    }

    pub fn tick_for(&self, price: Decimal) -> Decimal {
        self.bands
            .iter()
            .rev()
            .find(|b| price >= b.from)
            .unwrap_or(&self.bands[0])
            .tick
    }

    pub fn is_valid(&self, price: Decimal) -> bool {
        price > Decimal::ZERO && (price % self.tick_for(price)).is_zero()
    }

    /// Largest valid price not above `price`.
    pub fn round_down(&self, price: Decimal) -> Decimal {
        let tick = self.tick_for(price);
        (price / tick).floor() * tick
    }

    /// Smallest valid price not below `price`. Crossing into a higher band
    /// re-rounds with that band's tick.
    pub fn round_up(&self, price: Decimal) -> Decimal {
        let tick = self.tick_for(price);
        let up = (price / tick).ceil() * tick;
        if self.tick_for(up) == tick {
            up
        } else {
            let next = self.tick_for(up);
            (up / next).ceil() * next
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn tick_depends_on_band() {
        let t = TickTable::default();
        assert_eq!(t.tick_for(dec!(99.90)), dec!(0.10));
        assert_eq!(t.tick_for(dec!(100)), dec!(0.25));
    }

    #[test]
    fn validates_multiples() {
        let t = TickTable::default();
        assert!(t.is_valid(dec!(25.30)));
        assert!(!t.is_valid(dec!(25.35)));
        assert!(t.is_valid(dec!(150.75)));
        assert!(!t.is_valid(dec!(150.10)));
        assert!(!t.is_valid(dec!(0)));
    }

    #[test]
    fn rounding() {
        let t = TickTable::default();
        assert_eq!(t.round_down(dec!(25.37)), dec!(25.30));
        assert_eq!(t.round_up(dec!(25.31)), dec!(25.40));
        assert_eq!(t.round_up(dec!(150.01)), dec!(150.25));
        assert_eq!(t.round_up(dec!(25.30)), dec!(25.30));
        // 99.95 rounds to 100.00, which is also valid in the 0.25 band.
        assert_eq!(t.round_up(dec!(99.95)), dec!(100.00));
    }

    #[test]
    fn new_sorts_and_anchors_bands() {
        let t = TickTable::new(vec![
            TickBand {
                from: dec!(50),
                tick: dec!(0.5),
            },
            TickBand {
                from: dec!(1),
                tick: dec!(0.01),
            },
        ]);
        assert_eq!(t.bands()[0].from, dec!(0));
        assert_eq!(t.tick_for(dec!(0.5)), dec!(0.01));
        assert_eq!(t.tick_for(dec!(60)), dec!(0.5));
    }
}
