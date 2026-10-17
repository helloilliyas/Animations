//! The paper-trading account: validates orders, fills them against live
//! quotes, and keeps cash and holdings in step.
//!
//! The engine is deterministic: callers pass the current time and quotes, so
//! the app drives it from its polling loop and tests drive it directly.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use cse_core::market_hours::colombo;
use cse_core::{FeeSchedule, MarketCalendar, MarketPhase, Quote, TickTable};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::order::{Fill, Order, OrderId, OrderRequest, OrderStatus, OrderType, Side, Tif};
use crate::portfolio::{Portfolio, Valuation};

/// Rules the engine applies; all user-editable.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub fees: FeeSchedule,
    pub ticks: TickTable,
    pub calendar: MarketCalendar,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum OrderError {
    #[error("quantity must be greater than zero")]
    InvalidQuantity,
    #[error("price must be a positive multiple of {tick}")]
    InvalidPrice { tick: Decimal },
    #[error("time in force must be at least one day")]
    InvalidTif,
    #[error("a current price is needed for a market buy")]
    NoReferencePrice,
    #[error("order needs {needed} but buying power is {available}")]
    InsufficientBuyingPower { needed: Decimal, available: Decimal },
    #[error("only {available} shares of {symbol} available to sell")]
    InsufficientShares { symbol: String, available: u64 },
    #[error("order {0} not found")]
    NotFound(OrderId),
    #[error("order {0} can no longer be changed")]
    NotActive(OrderId),
    #[error("a market order's price cannot be changed")]
    PriceOnMarketOrder,
}

/// One movement of cash, for the running balance view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CashEntry {
    pub at: DateTime<Utc>,
    pub amount: Decimal,
    pub balance: Decimal,
    pub note: String,
}

/// ATrad's "Account Summary" for the paper account.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountSummary {
    pub starting_cash: Decimal,
    pub cash: Decimal,
    pub buying_power: Decimal,
    pub pending_buy_value: Decimal,
    /// Cash plus market value of holdings.
    pub equity: Decimal,
    /// Equity minus starting cash.
    pub total_return: Decimal,
    pub total_return_pct: Decimal,
    pub portfolio: Valuation,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub settings: Settings,
    starting_cash: Decimal,
    cash: Decimal,
    portfolio: Portfolio,
    orders: Vec<Order>,
    fills: Vec<Fill>,
    ledger: Vec<CashEntry>,
    next_id: OrderId,
}

impl Account {
    pub fn new(starting_cash: Decimal, settings: Settings, now: DateTime<Utc>) -> Self {
        let mut a = Self {
            settings,
            starting_cash,
            cash: Decimal::ZERO,
            portfolio: Portfolio::default(),
            orders: Vec::new(),
            fills: Vec::new(),
            ledger: Vec::new(),
            next_id: 1,
        };
        a.move_cash(now, starting_cash, "Starting cash");
        a
    }

    /// Clears everything and starts again with `starting_cash`.
    pub fn reset(&mut self, starting_cash: Decimal, now: DateTime<Utc>) {
        *self = Self::new(starting_cash, self.settings.clone(), now);
    }

    pub fn starting_cash(&self) -> Decimal {
        self.starting_cash
    }

    pub fn cash(&self) -> Decimal {
        self.cash
    }

    pub fn portfolio(&self) -> &Portfolio {
        &self.portfolio
    }

    /// All orders, oldest first.
    pub fn orders(&self) -> &[Order] {
        &self.orders
    }

    pub fn order(&self, id: OrderId) -> Option<&Order> {
        self.orders.iter().find(|o| o.id == id)
    }

    pub fn fills(&self) -> &[Fill] {
        &self.fills
    }

    pub fn ledger(&self) -> &[CashEntry] {
        &self.ledger
    }

    /// Cash set aside for active buy orders, fees included.
    pub fn pending_buy_value(&self) -> Decimal {
        self.active(Side::Buy)
            .map(|o| self.settings.fees.buy_net(o.qty, o.reserve_price))
            .sum()
    }

    pub fn buying_power(&self) -> Decimal {
        self.cash - self.pending_buy_value()
    }

    pub fn pending_qty(&self, symbol: &str, side: Side) -> u64 {
        self.active(side)
            .filter(|o| o.symbol == symbol)
            .map(|o| o.qty)
            .sum()
    }

    /// Shares held minus shares already in active sell orders.
    pub fn available_to_sell(&self, symbol: &str) -> u64 {
        self.portfolio
            .qty(symbol)
            .saturating_sub(self.pending_qty(symbol, Side::Sell))
    }

    /// Fees and cash effect the ticket shows before submitting.
    pub fn estimate(&self, side: Side, qty: u64, price: Decimal) -> (Decimal, Decimal) {
        let gross = Decimal::from(qty) * price;
        let fees = self.settings.fees.fee_for(gross);
        let net = match side {
            Side::Buy => gross + fees,
            Side::Sell => gross - fees,
        };
        (fees, net)
    }

    /// Validates and places an order. `reference_price` is the current price,
    /// required for market buys to reserve buying power.
    pub fn place(
        &mut self,
        req: OrderRequest,
        reference_price: Option<Decimal>,
        now: DateTime<Utc>,
    ) -> Result<OrderId, OrderError> {
        if req.qty == 0 {
            return Err(OrderError::InvalidQuantity);
        }
        if req.tif == Tif::Days(0) {
            return Err(OrderError::InvalidTif);
        }
        let reserve_price = match req.order_type {
            OrderType::Limit(p) => {
                self.check_price(p)?;
                p
            }
            OrderType::Market => match (req.side, reference_price) {
                (_, Some(p)) if p > Decimal::ZERO => p,
                (Side::Sell, _) => Decimal::ZERO,
                (Side::Buy, _) => return Err(OrderError::NoReferencePrice),
            },
        };
        match req.side {
            Side::Buy => self.check_buying_power(req.qty, reserve_price, None)?,
            Side::Sell => self.check_shares(&req.symbol, req.qty, None)?,
        }

        let (status, note) = match self.settings.calendar.phase_at(now) {
            MarketPhase::Open => (OrderStatus::Open, "Order placed"),
            _ => (OrderStatus::Queued, "Market closed: order queued"),
        };
        let id = self.next_id;
        self.next_id += 1;
        let mut order = Order {
            id,
            symbol: req.symbol,
            side: req.side,
            order_type: req.order_type,
            qty: req.qty,
            tif: req.tif,
            status: OrderStatus::Queued,
            placed_at: now,
            expires_at: self.expiry(req.tif, now),
            reserve_price,
            fill: None,
            history: Vec::new(),
        };
        order.set_status(now, status, note);
        self.orders.push(order);
        Ok(id)
    }

    /// Changes quantity and/or limit price of an active order.
    pub fn amend(
        &mut self,
        id: OrderId,
        qty: u64,
        limit_price: Option<Decimal>,
        now: DateTime<Utc>,
    ) -> Result<(), OrderError> {
        let order = self.active_order(id)?;
        if qty == 0 {
            return Err(OrderError::InvalidQuantity);
        }
        let new_type = match (order.order_type, limit_price) {
            (OrderType::Market, Some(_)) => return Err(OrderError::PriceOnMarketOrder),
            (OrderType::Limit(_), Some(p)) => {
                self.check_price(p)?;
                OrderType::Limit(p)
            }
            (t, None) => t,
        };
        let reserve_price = match new_type {
            OrderType::Limit(p) => p,
            OrderType::Market => order.reserve_price,
        };
        let (side, symbol) = (order.side, order.symbol.clone());
        match side {
            Side::Buy => self.check_buying_power(qty, reserve_price, Some(id))?,
            Side::Sell => self.check_shares(&symbol, qty, Some(id))?,
        }

        let order = self.order_mut(id)?;
        order.qty = qty;
        order.order_type = new_type;
        order.reserve_price = reserve_price;
        let status = order.status.clone();
        let note = match new_type {
            OrderType::Limit(p) => format!("Amended to {qty} @ {p}"),
            OrderType::Market => format!("Amended to {qty} @ market"),
        };
        order.set_status(now, status, &note);
        Ok(())
    }

    pub fn cancel(&mut self, id: OrderId, now: DateTime<Utc>) -> Result<(), OrderError> {
        self.active_order(id)?;
        self.order_mut(id)?
            .set_status(now, OrderStatus::Cancelled, "Cancelled by user");
        Ok(())
    }

    /// Advances the engine: expires old orders, opens queued ones when the
    /// market opens, and fills open orders whose price condition is met by
    /// the latest quotes. Returns the new fills.
    pub fn on_market(&mut self, now: DateTime<Utc>, quotes: &HashMap<String, Quote>) -> Vec<Fill> {
        for o in self.orders.iter_mut().filter(|o| o.status.is_active()) {
            if now >= o.expires_at {
                o.set_status(now, OrderStatus::Expired, "Time in force ended");
            }
        }
        if self.settings.calendar.phase_at(now) != MarketPhase::Open {
            return Vec::new();
        }
        for o in self
            .orders
            .iter_mut()
            .filter(|o| o.status == OrderStatus::Queued)
        {
            o.set_status(now, OrderStatus::Open, "Market opened");
        }

        let mut new_fills = Vec::new();
        for i in 0..self.orders.len() {
            let o = &self.orders[i];
            if o.status != OrderStatus::Open {
                continue;
            }
            let Some(q) = quotes.get(&o.symbol) else {
                continue;
            };
            let last = q.last;
            let triggered = last > Decimal::ZERO
                && match (o.side, o.order_type) {
                    (_, OrderType::Market) => true,
                    (Side::Buy, OrderType::Limit(p)) => last <= p,
                    (Side::Sell, OrderType::Limit(p)) => last >= p,
                };
            if triggered {
                if let Some(fill) = self.execute(i, last, now) {
                    new_fills.push(fill);
                }
            }
        }
        new_fills
    }

    pub fn summary(&self, quotes: &HashMap<String, Quote>) -> AccountSummary {
        let portfolio = self
            .portfolio
            .valuation(quotes, &self.settings.fees, &self.settings.ticks);
        let equity = self.cash + portfolio.market_value;
        let total_return = equity - self.starting_cash;
        AccountSummary {
            starting_cash: self.starting_cash,
            cash: self.cash,
            buying_power: self.buying_power(),
            pending_buy_value: self.pending_buy_value(),
            equity,
            total_return,
            total_return_pct: if self.starting_cash.is_zero() {
                Decimal::ZERO
            } else {
                (total_return * Decimal::ONE_HUNDRED / self.starting_cash).round_dp(2)
            },
            portfolio,
        }
    }

    fn execute(&mut self, index: usize, price: Decimal, now: DateTime<Utc>) -> Option<Fill> {
        let (id, symbol, side, qty) = {
            let o = &self.orders[index];
            (o.id, o.symbol.clone(), o.side, o.qty)
        };
        let (fees, net) = self.estimate(side, qty, price);
        let realized = match side {
            Side::Buy => {
                if net > self.cash {
                    self.orders[index].set_status(
                        now,
                        OrderStatus::Rejected("insufficient cash at fill".into()),
                        &format!("Needed {net}, cash {}", self.cash),
                    );
                    return None;
                }
                self.portfolio
                    .record_buy(&symbol, qty, net)
                    .expect("quantity validated at placement");
                self.move_cash(now, -net, &format!("Bought {qty} {symbol} @ {price}"));
                None
            }
            Side::Sell => match self.portfolio.record_sell(&symbol, qty, net) {
                Ok(gain) => {
                    self.move_cash(now, net, &format!("Sold {qty} {symbol} @ {price}"));
                    Some(gain)
                }
                Err(e) => {
                    self.orders[index].set_status(
                        now,
                        OrderStatus::Rejected(e.to_string()),
                        "Sell could not be filled",
                    );
                    return None;
                }
            },
        };
        let fill = Fill {
            order_id: id,
            symbol,
            side,
            qty,
            price,
            gross: Decimal::from(qty) * price,
            fees,
            net,
            at: now,
            realized,
        };
        let o = &mut self.orders[index];
        o.fill = Some(fill.clone());
        o.set_status(now, OrderStatus::Filled, &format!("Filled {qty} @ {price}"));
        self.fills.push(fill.clone());
        Some(fill)
    }

    fn move_cash(&mut self, at: DateTime<Utc>, amount: Decimal, note: &str) {
        self.cash += amount;
        self.ledger.push(CashEntry {
            at,
            amount,
            balance: self.cash,
            note: note.to_owned(),
        });
    }

    fn active(&self, side: Side) -> impl Iterator<Item = &Order> {
        self.orders
            .iter()
            .filter(move |o| o.side == side && o.status.is_active())
    }

    fn active_order(&self, id: OrderId) -> Result<&Order, OrderError> {
        let o = self.order(id).ok_or(OrderError::NotFound(id))?;
        if o.status.is_active() {
            Ok(o)
        } else {
            Err(OrderError::NotActive(id))
        }
    }

    fn order_mut(&mut self, id: OrderId) -> Result<&mut Order, OrderError> {
        self.orders
            .iter_mut()
            .find(|o| o.id == id)
            .ok_or(OrderError::NotFound(id))
    }

    fn check_price(&self, price: Decimal) -> Result<(), OrderError> {
        if self.settings.ticks.is_valid(price) {
            Ok(())
        } else {
            Err(OrderError::InvalidPrice {
                tick: self.settings.ticks.tick_for(price.max(Decimal::ZERO)),
            })
        }
    }

    /// `replacing` excludes an order being amended from the reservation.
    fn check_buying_power(
        &self,
        qty: u64,
        price: Decimal,
        replacing: Option<OrderId>,
    ) -> Result<(), OrderError> {
        let released = replacing
            .and_then(|id| self.order(id))
            .map_or(Decimal::ZERO, |o| {
                self.settings.fees.buy_net(o.qty, o.reserve_price)
            });
        let available = self.buying_power() + released;
        let needed = self.settings.fees.buy_net(qty, price);
        if needed > available {
            Err(OrderError::InsufficientBuyingPower { needed, available })
        } else {
            Ok(())
        }
    }

    fn check_shares(
        &self,
        symbol: &str,
        qty: u64,
        replacing: Option<OrderId>,
    ) -> Result<(), OrderError> {
        let released = replacing.and_then(|id| self.order(id)).map_or(0, |o| o.qty);
        let available = self.available_to_sell(symbol) + released;
        if qty > available {
            Err(OrderError::InsufficientShares {
                symbol: symbol.to_owned(),
                available,
            })
        } else {
            Ok(())
        }
    }

    /// Close of the order's last trading session.
    fn expiry(&self, tif: Tif, now: DateTime<Utc>) -> DateTime<Utc> {
        let cal = &self.settings.calendar;
        let mut date = cal.next_open(now).with_timezone(&colombo()).date_naive();
        let sessions = match tif {
            Tif::Day => 1,
            Tif::Days(n) => n,
        };
        for _ in 1..sessions {
            date += Duration::days(1);
            while !cal.is_trading_day(date) {
                date += Duration::days(1);
            }
        }
        cal.close_on(date).expect("date is a trading day")
    }
}
