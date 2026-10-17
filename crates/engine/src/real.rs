//! The user's real holdings, entered by hand or imported from CSV.
//!
//! Stored as a list of transactions; the [`Portfolio`] is rebuilt from them in
//! date order, so editing or deleting an old trade keeps average costs right.

use std::str::FromStr;

use chrono::NaiveDate;
use cse_core::FeeSchedule;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::order::Side;
use crate::portfolio::{Portfolio, PortfolioError};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transaction {
    pub id: u64,
    pub date: NaiveDate,
    pub symbol: String,
    pub side: Side,
    pub qty: u64,
    pub price: Decimal,
    /// Actual fees from the broker's contract note; `None` uses the schedule.
    pub fees: Option<Decimal>,
}

/// A transaction before it has an id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewTransaction {
    pub date: NaiveDate,
    pub symbol: String,
    pub side: Side,
    pub qty: u64,
    pub price: Decimal,
    pub fees: Option<Decimal>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RealError {
    #[error("price must be greater than zero")]
    InvalidPrice,
    #[error("{0}")]
    Portfolio(#[from] PortfolioError),
    #[error("transaction {0} not found")]
    NotFound(u64),
    #[error("line {line}: {message}")]
    Csv { line: usize, message: String },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RealPortfolio {
    transactions: Vec<Transaction>,
    next_id: u64,
}

pub const CSV_HEADER: &str = "date,symbol,side,qty,price,fees";

impl RealPortfolio {
    /// Transactions in date order.
    pub fn transactions(&self) -> Vec<&Transaction> {
        let mut t: Vec<_> = self.transactions.iter().collect();
        t.sort_by_key(|t| (t.date, t.id));
        t
    }

    /// Adds a transaction if the history stays valid (no selling shares that
    /// were not held at that date).
    pub fn add(&mut self, tx: NewTransaction, fees: &FeeSchedule) -> Result<u64, RealError> {
        let id = self.next_id.max(1);
        let mut candidate = self.clone();
        candidate.transactions.push(validate(id, tx)?);
        candidate.portfolio(fees)?;
        candidate.next_id = id + 1;
        *self = candidate;
        Ok(id)
    }

    pub fn remove(&mut self, id: u64, fees: &FeeSchedule) -> Result<(), RealError> {
        let mut candidate = self.clone();
        let before = candidate.transactions.len();
        candidate.transactions.retain(|t| t.id != id);
        if candidate.transactions.len() == before {
            return Err(RealError::NotFound(id));
        }
        candidate.portfolio(fees)?;
        *self = candidate;
        Ok(())
    }

    /// Holdings and realised P&L from all transactions.
    pub fn portfolio(&self, fees: &FeeSchedule) -> Result<Portfolio, RealError> {
        let mut p = Portfolio::default();
        for t in self.transactions() {
            let gross = Decimal::from(t.qty) * t.price;
            let fee = t.fees.unwrap_or_else(|| fees.fee_for(gross));
            match t.side {
                Side::Buy => p.record_buy(&t.symbol, t.qty, gross + fee)?,
                Side::Sell => {
                    p.record_sell(&t.symbol, t.qty, gross - fee)?;
                }
            }
        }
        Ok(p)
    }

    /// Imports `date,symbol,side,qty,price[,fees]` rows (header optional).
    /// All-or-nothing: any bad row rejects the whole file.
    pub fn import_csv(&mut self, csv: &str, fees: &FeeSchedule) -> Result<usize, RealError> {
        let mut candidate = self.clone();
        let mut count = 0;
        for (i, raw) in csv.lines().enumerate() {
            let line = i + 1;
            let row = raw.trim();
            if row.is_empty() || (line == 1 && row.to_ascii_lowercase().starts_with("date")) {
                continue;
            }
            let tx = parse_row(row).map_err(|message| RealError::Csv { line, message })?;
            candidate.add(tx, fees).map_err(|e| RealError::Csv {
                line,
                message: e.to_string(),
            })?;
            count += 1;
        }
        *self = candidate;
        Ok(count)
    }

    pub fn export_csv(&self) -> String {
        let mut out = String::from(CSV_HEADER);
        out.push('\n');
        for t in self.transactions() {
            let side = match t.side {
                Side::Buy => "BUY",
                Side::Sell => "SELL",
            };
            let fees = t.fees.map(|f| f.to_string()).unwrap_or_default();
            out.push_str(&format!(
                "{},{},{side},{},{},{fees}\n",
                t.date, t.symbol, t.qty, t.price
            ));
        }
        out
    }
}

fn validate(id: u64, tx: NewTransaction) -> Result<Transaction, RealError> {
    if tx.qty == 0 {
        return Err(PortfolioError::ZeroQuantity.into());
    }
    if tx.price <= Decimal::ZERO {
        return Err(RealError::InvalidPrice);
    }
    Ok(Transaction {
        id,
        date: tx.date,
        symbol: tx.symbol.trim().to_ascii_uppercase(),
        side: tx.side,
        qty: tx.qty,
        price: tx.price,
        fees: tx.fees,
    })
}

fn parse_row(row: &str) -> Result<NewTransaction, String> {
    let cols: Vec<&str> = row.split(',').map(str::trim).collect();
    if !(5..=6).contains(&cols.len()) {
        return Err(format!(
            "expected 5 or 6 columns ({CSV_HEADER}), found {}",
            cols.len()
        ));
    }
    let date = NaiveDate::parse_from_str(cols[0], "%Y-%m-%d")
        .map_err(|_| format!("date {:?} is not YYYY-MM-DD", cols[0]))?;
    let side = match cols[2].to_ascii_uppercase().as_str() {
        "BUY" | "B" => Side::Buy,
        "SELL" | "S" => Side::Sell,
        other => return Err(format!("side {other:?} must be BUY or SELL")),
    };
    let qty = cols[3]
        .replace(',', "")
        .parse::<u64>()
        .map_err(|_| format!("quantity {:?} is not a whole number", cols[3]))?;
    let price =
        Decimal::from_str(cols[4]).map_err(|_| format!("price {:?} is not a number", cols[4]))?;
    let fees = match cols.get(5) {
        Some(f) if !f.is_empty() => {
            Some(Decimal::from_str(f).map_err(|_| format!("fees {f:?} is not a number"))?)
        }
        _ => None,
    };
    Ok(NewTransaction {
        date,
        symbol: cols[1].to_owned(),
        side,
        qty,
        price,
        fees,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn tx(date: &str, side: Side, qty: u64, price: Decimal) -> NewTransaction {
        NewTransaction {
            date: d(date),
            symbol: "jkh.n0000".into(),
            side,
            qty,
            price,
            fees: None,
        }
    }

    #[test]
    fn builds_portfolio_in_date_order_regardless_of_entry_order() {
        let fees = FeeSchedule::default();
        let mut r = RealPortfolio::default();
        r.add(tx("2026-01-10", Side::Buy, 100, dec!(20)), &fees)
            .unwrap();
        // Entered later but dated earlier: a sell before the buy is invalid.
        assert!(matches!(
            r.add(tx("2026-01-05", Side::Sell, 50, dec!(21)), &fees),
            Err(RealError::Portfolio(PortfolioError::NotEnoughShares { .. }))
        ));
        r.add(tx("2026-01-05", Side::Buy, 100, dec!(18)), &fees)
            .unwrap();
        r.add(tx("2026-02-01", Side::Sell, 50, dec!(25)), &fees)
            .unwrap();

        let p = r.portfolio(&fees).unwrap();
        let h = p.holding("JKH.N0000").unwrap();
        assert_eq!(h.qty, 150);
        // Cost 1820.16 + 2022.40 = 3842.56 for 200; 50 sold at avg 19.2128.
        assert_eq!(h.total_cost, dec!(2881.92));
        assert!(p.realized("JKH.N0000") > dec!(0));
    }

    #[test]
    fn explicit_fees_override_the_schedule() {
        let fees = FeeSchedule::default();
        let mut r = RealPortfolio::default();
        let mut t = tx("2026-01-05", Side::Buy, 100, dec!(10));
        t.fees = Some(dec!(5));
        r.add(t, &fees).unwrap();
        let p = r.portfolio(&fees).unwrap();
        assert_eq!(p.holding("JKH.N0000").unwrap().total_cost, dec!(1005));
    }

    #[test]
    fn removing_a_buy_that_later_sells_depend_on_is_refused() {
        let fees = FeeSchedule::default();
        let mut r = RealPortfolio::default();
        let buy = r
            .add(tx("2026-01-05", Side::Buy, 100, dec!(10)), &fees)
            .unwrap();
        let sell = r
            .add(tx("2026-01-06", Side::Sell, 100, dec!(11)), &fees)
            .unwrap();
        assert!(r.remove(buy, &fees).is_err());
        r.remove(sell, &fees).unwrap();
        r.remove(buy, &fees).unwrap();
        assert!(r.transactions().is_empty());
        assert_eq!(r.remove(buy, &fees), Err(RealError::NotFound(buy)));
    }

    #[test]
    fn csv_import_and_export_round_trip() {
        let fees = FeeSchedule::default();
        let csv = "date,symbol,side,qty,price,fees\n\
                   2026-01-05,JKH.N0000,BUY,1000,18.50,\n\
                   2026-01-06,COMB.N0000,buy,\"500\",100,560\n";
        let mut r = RealPortfolio::default();
        // Quotes are not supported; the bad row is reported with its line.
        assert_eq!(
            r.import_csv(csv, &fees),
            Err(RealError::Csv {
                line: 3,
                message: "quantity \"\\\"500\\\"\" is not a whole number".into()
            })
        );
        assert!(r.transactions().is_empty(), "import is all-or-nothing");

        let csv = csv.replace("\"500\"", "500");
        assert_eq!(r.import_csv(&csv, &fees).unwrap(), 2);
        let out = r.export_csv();
        assert_eq!(
            out,
            "date,symbol,side,qty,price,fees\n\
             2026-01-05,JKH.N0000,BUY,1000,18.50,\n\
             2026-01-06,COMB.N0000,BUY,500,100,560\n"
        );
        let mut again = RealPortfolio::default();
        again.import_csv(&out, &fees).unwrap();
        assert_eq!(again.portfolio(&fees), r.portfolio(&fees));
    }

    #[test]
    fn csv_errors_are_specific() {
        let fees = FeeSchedule::default();
        let r = RealPortfolio::default();
        let err = |s: &str| r.clone().import_csv(s, &fees).unwrap_err().to_string();
        assert_eq!(
            err("05/01/2026,A,BUY,1,1"),
            "line 1: date \"05/01/2026\" is not YYYY-MM-DD"
        );
        assert_eq!(
            err("2026-01-05,A,HOLD,1,1"),
            "line 1: side \"HOLD\" must be BUY or SELL"
        );
        assert_eq!(
            err("2026-01-05,A,BUY,1"),
            "line 1: expected 5 or 6 columns (date,symbol,side,qty,price,fees), found 4"
        );
        assert_eq!(
            err("2026-01-05,A,SELL,1,1"),
            "line 1: only 0 shares of A held"
        );
        assert_eq!(
            err("2026-01-05,A,BUY,1,0"),
            "line 1: price must be greater than zero"
        );
    }
}
