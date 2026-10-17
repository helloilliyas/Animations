//! Domain models and CSE trading rules shared by every crate in the app.
//!
//! Nothing in here does I/O: the CSE client fills these models, the storage
//! crate persists them, and the paper-trading engine reasons about them.

pub mod fees;
pub mod market_hours;
pub mod models;
pub mod tick;

pub use fees::{FeeComponent, FeeSchedule};
pub use market_hours::{MarketCalendar, MarketPhase};
pub use models::{Announcement, IndexValue, MarketSummary, Quote, TopMover, TopMoverKind};
pub use tick::{TickBand, TickTable};
