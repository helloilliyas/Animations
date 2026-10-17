//! Transaction costs charged on every CSE buy and sell.
//!
//! ATrad gets commission from the broker's server; we compute it locally from
//! an editable schedule so the user can match their own broker's rates.

use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize};

use crate::tick::TickTable;

/// One line of the fee schedule, as a percentage of order value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeeComponent {
    pub name: String,
    /// Percentage of order value, e.g. `0.64` for 0.64%.
    pub rate_pct: Decimal,
}

/// Fees applied per side (buy and sell are charged the same rates).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeeSchedule {
    pub components: Vec<FeeComponent>,
}

impl Default for FeeSchedule {
    /// Commonly published CSE schedule for orders up to Rs. 100 million
    /// (1.12% per side). Rates change; users should confirm with their broker.
    fn default() -> Self {
        let c = |name: &str, rate_pct: Decimal| FeeComponent {
            name: name.to_owned(),
            rate_pct,
        };
        Self {
            components: vec![
                c("Brokerage", Decimal::new(64, 2)),
                c("CSE fees", Decimal::new(84, 3)),
                c("CDS fees", Decimal::new(24, 3)),
                c("SEC cess", Decimal::new(72, 3)),
                c("Share Transaction Levy", Decimal::new(30, 2)),
            ],
        }
    }
}

impl FeeSchedule {
    /// Total fee as a percentage, e.g. `1.12`.
    pub fn total_pct(&self) -> Decimal {
        self.components.iter().map(|c| c.rate_pct).sum()
    }

    /// Total fee as a fraction, e.g. `0.0112`.
    pub fn total_rate(&self) -> Decimal {
        self.total_pct() / Decimal::ONE_HUNDRED
    }

    /// Fees on an order of `gross` value, rounded to cents.
    pub fn fee_for(&self, gross: Decimal) -> Decimal {
        round_cents(gross * self.total_rate())
    }

    /// Cash paid for a buy: gross value plus fees.
    pub fn buy_net(&self, qty: u64, price: Decimal) -> Decimal {
        let gross = Decimal::from(qty) * price;
        gross + self.fee_for(gross)
    }

    /// Cash received for a sell: gross value minus fees.
    pub fn sell_net(&self, qty: u64, price: Decimal) -> Decimal {
        let gross = Decimal::from(qty) * price;
        gross - self.fee_for(gross)
    }

    /// Break-even selling (B.E.S) price: the lowest valid price at which
    /// selling recovers `avg_cost` per share, where `avg_cost` already
    /// includes the buy-side fees.
    pub fn break_even_price(&self, avg_cost: Decimal, ticks: &TickTable) -> Decimal {
        let raw = avg_cost / (Decimal::ONE - self.total_rate());
        ticks.round_up(raw)
    }
}

pub(crate) fn round_cents(value: Decimal) -> Decimal {
    value.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn default_schedule_totals_1_12_pct() {
        let fees = FeeSchedule::default();
        assert_eq!(fees.total_pct(), dec!(1.12));
        assert_eq!(fees.total_rate(), dec!(0.0112));
    }

    #[test]
    fn buy_and_sell_net_include_fees() {
        let fees = FeeSchedule::default();
        // 100 x 150.00 = 15,000.00; fee = 168.00
        assert_eq!(fees.buy_net(100, dec!(150)), dec!(15168.00));
        assert_eq!(fees.sell_net(100, dec!(150)), dec!(14832.00));
    }

    #[test]
    fn fee_rounds_to_cents() {
        let fees = FeeSchedule::default();
        // 33 x 10.10 = 333.30; 1.12% = 3.73296 -> 3.73
        assert_eq!(fees.fee_for(dec!(333.30)), dec!(3.73));
    }

    #[test]
    fn break_even_covers_sell_fees_and_rounds_up_to_tick() {
        let fees = FeeSchedule::default();
        let ticks = TickTable::default();
        // 151.68 / 0.9888 = 153.398... -> next 0.25 tick is 153.50
        let bes = fees.break_even_price(dec!(151.68), &ticks);
        assert_eq!(bes, dec!(153.50));
        // Selling at B.E.S never loses money.
        assert!(fees.sell_net(1, bes) >= dec!(151.68));
    }

    #[test]
    fn custom_schedule() {
        let fees = FeeSchedule {
            components: vec![FeeComponent {
                name: "Flat".into(),
                rate_pct: dec!(1),
            }],
        };
        assert_eq!(fees.buy_net(10, dec!(100)), dec!(1010));
    }
}
