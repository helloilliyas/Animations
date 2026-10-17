//! Holdings, average cost and profit/loss.
//!
//! Used by both the paper account (filled by the engine) and the real
//! portfolio the user enters by hand. Costs include buy-side fees, so the
//! average cost is what one share really cost, as in ATrad's "Avg. Cost".

use std::collections::{BTreeMap, HashMap};

use cse_core::{FeeSchedule, Quote, TickTable};
use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Holding {
    pub symbol: String,
    pub qty: u64,
    /// Total cost of the shares held, buy fees included.
    pub total_cost: Decimal,
}

impl Holding {
    /// Weighted average cost per share, fees included.
    pub fn avg_cost(&self) -> Decimal {
        if self.qty == 0 {
            Decimal::ZERO
        } else {
            self.total_cost / Decimal::from(self.qty)
        }
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PortfolioError {
    #[error("quantity must be greater than zero")]
    ZeroQuantity,
    #[error("only {held} shares of {symbol} held")]
    NotEnoughShares { symbol: String, held: u64 },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Portfolio {
    holdings: BTreeMap<String, Holding>,
    /// Realised gain/loss per symbol, after all fees.
    realized: BTreeMap<String, Decimal>,
}

impl Portfolio {
    pub fn holdings(&self) -> impl Iterator<Item = &Holding> {
        self.holdings.values()
    }

    pub fn holding(&self, symbol: &str) -> Option<&Holding> {
        self.holdings.get(symbol)
    }

    pub fn qty(&self, symbol: &str) -> u64 {
        self.holdings.get(symbol).map_or(0, |h| h.qty)
    }

    pub fn realized(&self, symbol: &str) -> Decimal {
        self.realized.get(symbol).copied().unwrap_or_default()
    }

    pub fn total_realized(&self) -> Decimal {
        self.realized.values().copied().sum()
    }

    /// Adds shares bought for `net_cost` (gross plus fees).
    pub fn record_buy(
        &mut self,
        symbol: &str,
        qty: u64,
        net_cost: Decimal,
    ) -> Result<(), PortfolioError> {
        if qty == 0 {
            return Err(PortfolioError::ZeroQuantity);
        }
        let h = self
            .holdings
            .entry(symbol.to_owned())
            .or_insert_with(|| Holding {
                symbol: symbol.to_owned(),
                qty: 0,
                total_cost: Decimal::ZERO,
            });
        h.qty += qty;
        h.total_cost += net_cost;
        Ok(())
    }

    /// Removes shares sold for `net_proceeds` (gross minus fees) and returns
    /// the realised gain/loss on them.
    pub fn record_sell(
        &mut self,
        symbol: &str,
        qty: u64,
        net_proceeds: Decimal,
    ) -> Result<Decimal, PortfolioError> {
        if qty == 0 {
            return Err(PortfolioError::ZeroQuantity);
        }
        let held = self.qty(symbol);
        if qty > held {
            return Err(PortfolioError::NotEnoughShares {
                symbol: symbol.to_owned(),
                held,
            });
        }
        let h = self.holdings.get_mut(symbol).expect("checked above");
        let cost_of_sold = if qty == h.qty {
            h.total_cost
        } else {
            round_cents(h.total_cost * Decimal::from(qty) / Decimal::from(h.qty))
        };
        h.qty -= qty;
        h.total_cost -= cost_of_sold;
        if h.qty == 0 {
            self.holdings.remove(symbol);
        }
        let gain = net_proceeds - cost_of_sold;
        *self.realized.entry(symbol.to_owned()).or_default() += gain;
        Ok(gain)
    }

    /// Values every holding at the latest prices. Holdings without a quote are
    /// valued at cost and flagged with `priced = false`.
    pub fn valuation(
        &self,
        quotes: &HashMap<String, Quote>,
        fees: &FeeSchedule,
        ticks: &TickTable,
    ) -> Valuation {
        let rate = fees.total_rate();
        let mut rows: Vec<HoldingValue> = self
            .holdings
            .values()
            .map(|h| {
                let quote = quotes.get(&h.symbol);
                let last = quote.map_or(h.avg_cost(), |q| q.last);
                let qty = Decimal::from(h.qty);
                let market_value = round_cents(qty * last);
                let unrealized = round_cents(qty * last * (Decimal::ONE - rate)) - h.total_cost;
                HoldingValue {
                    symbol: h.symbol.clone(),
                    qty: h.qty,
                    avg_cost: round_cents(h.avg_cost()),
                    break_even: fees.break_even_price(h.avg_cost(), ticks),
                    last,
                    total_cost: h.total_cost,
                    market_value,
                    unrealized,
                    unrealized_pct: pct(unrealized, h.total_cost),
                    today: quote.map_or(Decimal::ZERO, |q| round_cents(qty * q.change)),
                    weight_pct: Decimal::ZERO,
                    priced: quote.is_some(),
                }
            })
            .collect();

        let market_value: Decimal = rows.iter().map(|r| r.market_value).sum();
        for r in &mut rows {
            r.weight_pct = pct(r.market_value, market_value);
        }
        let total_cost = rows.iter().map(|r| r.total_cost).sum();
        let unrealized = rows.iter().map(|r| r.unrealized).sum();
        Valuation {
            total_cost,
            market_value,
            unrealized,
            unrealized_pct: pct(unrealized, total_cost),
            today: rows.iter().map(|r| r.today).sum(),
            realized: self.total_realized(),
            holdings: rows,
        }
    }
}

/// One row of the Portfolio "Detailed" view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HoldingValue {
    pub symbol: String,
    pub qty: u64,
    pub avg_cost: Decimal,
    /// Break-even selling price (B.E.S).
    pub break_even: Decimal,
    pub last: Decimal,
    pub total_cost: Decimal,
    pub market_value: Decimal,
    /// Gain/loss if sold now at `last`, after sell fees.
    pub unrealized: Decimal,
    pub unrealized_pct: Decimal,
    /// Change in value since the previous close.
    pub today: Decimal,
    /// Share of portfolio market value.
    pub weight_pct: Decimal,
    /// False when no quote was available and `last` is the average cost.
    pub priced: bool,
}

/// The Portfolio "Summary" view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Valuation {
    pub total_cost: Decimal,
    pub market_value: Decimal,
    pub unrealized: Decimal,
    pub unrealized_pct: Decimal,
    pub today: Decimal,
    pub realized: Decimal,
    pub holdings: Vec<HoldingValue>,
}

pub(crate) fn round_cents(v: Decimal) -> Decimal {
    v.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero)
}

fn pct(part: Decimal, whole: Decimal) -> Decimal {
    if whole.is_zero() {
        Decimal::ZERO
    } else {
        (part * Decimal::ONE_HUNDRED / whole).round_dp(2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn quote(symbol: &str, last: Decimal, change: Decimal) -> (String, Quote) {
        (
            symbol.to_owned(),
            Quote {
                symbol: symbol.into(),
                name: String::new(),
                last,
                change,
                change_pct: Decimal::ZERO,
                prev_close: last - change,
                open: None,
                high: None,
                low: None,
                share_volume: 0,
                trades: 0,
                turnover: Decimal::ZERO,
                last_traded_at: None,
                security_id: None,
            },
        )
    }

    #[test]
    fn average_cost_is_weighted() {
        let mut p = Portfolio::default();
        p.record_buy("JKH", 100, dec!(2022.40)).unwrap(); // 100 @ 20 + fees
        p.record_buy("JKH", 100, dec!(2528.00)).unwrap(); // 100 @ 25 + fees
        let h = p.holding("JKH").unwrap();
        assert_eq!(h.qty, 200);
        assert_eq!(h.avg_cost(), dec!(22.752));
    }

    #[test]
    fn selling_realises_gain_against_average_cost() {
        let mut p = Portfolio::default();
        p.record_buy("JKH", 200, dec!(4000)).unwrap(); // avg 20
        let gain = p.record_sell("JKH", 50, dec!(1200)).unwrap();
        assert_eq!(gain, dec!(200)); // 1200 - 50 * 20
        let h = p.holding("JKH").unwrap();
        assert_eq!(h.qty, 150);
        assert_eq!(h.avg_cost(), dec!(20)); // unchanged by a sale
        assert_eq!(p.realized("JKH"), dec!(200));
    }

    #[test]
    fn selling_everything_removes_holding_and_uses_exact_cost() {
        let mut p = Portfolio::default();
        p.record_buy("A", 3, dec!(10)).unwrap(); // avg 3.333...
        p.record_sell("A", 1, dec!(5)).unwrap();
        p.record_sell("A", 2, dec!(5)).unwrap();
        assert!(p.holding("A").is_none());
        // Total realised equals total proceeds minus total cost, no drift.
        assert_eq!(p.total_realized(), dec!(0));
    }

    #[test]
    fn cannot_oversell_or_trade_zero() {
        let mut p = Portfolio::default();
        p.record_buy("A", 10, dec!(100)).unwrap();
        assert_eq!(
            p.record_sell("A", 11, dec!(1)),
            Err(PortfolioError::NotEnoughShares {
                symbol: "A".into(),
                held: 10
            })
        );
        assert_eq!(
            p.record_buy("A", 0, dec!(1)),
            Err(PortfolioError::ZeroQuantity)
        );
    }

    #[test]
    fn valuation_matches_atrad_detailed_view() {
        let fees = FeeSchedule::default();
        let ticks = TickTable::default();
        let mut p = Portfolio::default();
        // 100 @ 150.00 -> cost 15,168.00, avg 151.68
        p.record_buy("ABC", 100, fees.buy_net(100, dec!(150)))
            .unwrap();
        p.record_buy("XYZ", 1000, fees.buy_net(1000, dec!(10)))
            .unwrap();
        let quotes: HashMap<_, _> = [
            quote("ABC", dec!(160), dec!(2.5)),
            quote("XYZ", dec!(9.5), dec!(-0.1)),
        ]
        .into();

        let v = p.valuation(&quotes, &fees, &ticks);
        let abc = &v.holdings[0];
        assert_eq!(abc.symbol, "ABC");
        assert_eq!(abc.avg_cost, dec!(151.68));
        assert_eq!(abc.break_even, dec!(153.50));
        assert_eq!(abc.market_value, dec!(16000.00));
        // 16,000 * 0.9888 = 15,820.80 - 15,168.00
        assert_eq!(abc.unrealized, dec!(652.80));
        assert_eq!(abc.unrealized_pct, dec!(4.30));
        assert_eq!(abc.today, dec!(250.00));

        let xyz = &v.holdings[1];
        // 9,500 * 0.9888 = 9,393.60 - 10,112.00
        assert_eq!(xyz.unrealized, dec!(-718.40));
        assert_eq!(xyz.today, dec!(-100.00));

        assert_eq!(v.market_value, dec!(25500.00));
        assert_eq!(v.total_cost, dec!(25280.00));
        assert_eq!(v.unrealized, dec!(-65.60));
        assert_eq!(v.today, dec!(150.00));
        assert_eq!(abc.weight_pct + xyz.weight_pct, dec!(100.00));
    }

    #[test]
    fn unpriced_holdings_are_valued_at_cost() {
        let mut p = Portfolio::default();
        p.record_buy("A", 10, dec!(100)).unwrap();
        let v = p.valuation(
            &HashMap::new(),
            &FeeSchedule::default(),
            &TickTable::default(),
        );
        assert!(!v.holdings[0].priced);
        assert_eq!(v.holdings[0].market_value, dec!(100));
    }
}
