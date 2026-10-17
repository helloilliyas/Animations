//! Order types and lifecycle, mirroring ATrad's order ticket and blotter.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

pub type OrderId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderType {
    Limit(Decimal),
    Market,
}

/// Time in force.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tif {
    /// Expires at the close of its first trading session.
    Day,
    /// Expires at the close of its Nth trading session (N >= 1).
    Days(u32),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderStatus {
    /// Placed while the market was closed; becomes Open at the next open.
    Queued,
    Open,
    Filled,
    Cancelled,
    Expired,
    /// Could not be filled, e.g. cash ran short when a market order filled.
    Rejected(String),
}

impl OrderStatus {
    /// Still working: can be filled, amended or cancelled.
    pub fn is_active(&self) -> bool {
        matches!(self, OrderStatus::Queued | OrderStatus::Open)
    }
}

/// What the user enters on the order ticket.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrderRequest {
    pub symbol: String,
    pub side: Side,
    pub order_type: OrderType,
    pub qty: u64,
    pub tif: Tif,
}

/// One line of an order's history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrderEvent {
    pub at: DateTime<Utc>,
    pub status: OrderStatus,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Order {
    pub id: OrderId,
    pub symbol: String,
    pub side: Side,
    pub order_type: OrderType,
    pub qty: u64,
    pub tif: Tif,
    pub status: OrderStatus,
    pub placed_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    /// Price used to reserve buying power for a buy (limit price, or the
    /// reference price for market orders).
    pub reserve_price: Decimal,
    pub fill: Option<Fill>,
    pub history: Vec<OrderEvent>,
}

impl Order {
    pub fn limit_price(&self) -> Option<Decimal> {
        match self.order_type {
            OrderType::Limit(p) => Some(p),
            OrderType::Market => None,
        }
    }

    pub(crate) fn set_status(&mut self, at: DateTime<Utc>, status: OrderStatus, note: &str) {
        self.status = status.clone();
        self.history.push(OrderEvent {
            at,
            status,
            note: note.to_owned(),
        });
    }
}

/// An execution. Paper orders fill in full at one price.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fill {
    pub order_id: OrderId,
    pub symbol: String,
    pub side: Side,
    pub qty: u64,
    pub price: Decimal,
    pub gross: Decimal,
    pub fees: Decimal,
    /// Cash paid (buy) or received (sell).
    pub net: Decimal,
    pub at: DateTime<Utc>,
    /// Realised gain/loss for sells.
    pub realized: Option<Decimal>,
}
