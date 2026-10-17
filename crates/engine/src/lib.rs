//! Paper-trading engine for CSE stocks.
//!
//! - [`Account`]: cash, orders, fills and the paper portfolio.
//! - [`Portfolio`]: holdings with average cost and P&L, also used for the
//!   user's real (manually entered) holdings.
//!
//! Paper orders fill in full at the last traded price once their condition is
//! met: market orders immediately, limit buys when last <= limit, limit sells
//! when last >= limit. Orders placed while the market is closed are queued and
//! open at the next session.

pub mod account;
pub mod order;
pub mod portfolio;
pub mod real;

pub use account::{Account, AccountSummary, CashEntry, OrderError, Settings};
pub use order::{
    Fill, Order, OrderEvent, OrderId, OrderRequest, OrderStatus, OrderType, Side, Tif,
};
pub use portfolio::{Holding, HoldingValue, Portfolio, PortfolioError, Valuation};
pub use real::{NewTransaction, RealError, RealPortfolio, Transaction};
