//! The app's backend. The UI talks to it only through [`AppService::dispatch`],
//! a JSON command API shared by the Tauri app and the browser dev server.
//!
//! It keeps the latest CSE market data in memory, drives the paper-trading
//! engine from each refresh, and persists accounts, portfolios, watch lists
//! and price snapshots in SQLite.

mod alerts;
mod chart;
mod import_list;
mod market;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};

use chrono::{DateTime, Duration, Utc};
use cse_client::{poll_interval, CseClient};
use cse_core::{MarketPhase, Quote};
use cse_engine::{
    Account, NewTransaction, OrderId, OrderRequest, OrderType, RealPortfolio, Settings, Side,
};
use cse_storage::Store;
use rust_decimal::Decimal;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub use chart::ASPI_KEY;
pub use market::{MarketData, MarketView};

const ACCOUNT_KEY: &str = "paper_account";
const REAL_KEY: &str = "real_portfolio";
/// Price snapshots are kept at most this often, and for this long.
const SNAPSHOT_EVERY: i64 = 60;
const SNAPSHOT_DAYS: i64 = 90;
pub const DEFAULT_STARTING_CASH: Decimal = Decimal::from_parts(1_000_000, 0, 0, false, 0);

pub type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("unknown command {0:?}")]
    UnknownCommand(String),
    #[error("bad arguments: {0}")]
    BadArgs(String),
    #[error("{0}")]
    Order(#[from] cse_engine::OrderError),
    #[error("{0}")]
    Real(#[from] cse_engine::RealError),
    #[error("{0}")]
    Storage(#[from] cse_storage::Error),
    #[error("{0}")]
    Market(#[from] cse_client::Error),
}

pub type Result<T> = std::result::Result<T, ServiceError>;

struct State {
    account: Account,
    real: RealPortfolio,
    market: MarketData,
    last_snapshot: Option<DateTime<Utc>>,
    /// Day each chart key last fetched its one-year history.
    synced: HashMap<String, chrono::NaiveDate>,
    /// Day whose closing prices were last saved to daily history.
    last_daily_save: Option<chrono::NaiveDate>,
    alerts: alerts::AlertBook,
    analysis: HashMap<String, alerts::Analysis>,
    /// Day each favourite missing from the price list was looked up.
    looked_up: HashMap<String, chrono::NaiveDate>,
}

pub struct AppService {
    client: CseClient,
    store: Mutex<Store>,
    state: RwLock<State>,
    clock: Clock,
    /// File the Android background service reads alert notifications from.
    outbox: Option<std::path::PathBuf>,
}

impl AppService {
    /// Opens (or creates) the database at `db_path`.
    pub fn open(db_path: impl AsRef<Path>) -> Result<Self> {
        let outbox = alerts::outbox_beside(db_path.as_ref());
        let mut svc = Self::with(
            Store::open(db_path)?,
            CseClient::default(),
            Arc::new(Utc::now),
        )?;
        svc.outbox = Some(outbox);
        Ok(svc)
    }

    /// Writes fired alerts to `path` (one JSON object per line).
    pub fn with_outbox(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.outbox = Some(path.into());
        self
    }

    pub fn with(store: Store, client: CseClient, clock: Clock) -> Result<Self> {
        let now = clock();
        let account = store
            .load_json(ACCOUNT_KEY)?
            .unwrap_or_else(|| Account::new(DEFAULT_STARTING_CASH, Settings::default(), now));
        let real = store.load_json(REAL_KEY)?.unwrap_or_default();
        let alert_book = store.load_json(alerts::ALERTS_KEY)?.unwrap_or_default();
        Ok(Self {
            client,
            store: Mutex::new(store),
            state: RwLock::new(State {
                account,
                real,
                market: MarketData::default(),
                last_snapshot: None,
                synced: HashMap::new(),
                last_daily_save: None,
                alerts: alert_book,
                analysis: HashMap::new(),
                looked_up: HashMap::new(),
            }),
            clock,
            outbox: None,
        })
    }

    fn now(&self) -> DateTime<Utc> {
        (self.clock)()
    }

    /// Current market phase from the user's calendar settings.
    pub fn phase(&self) -> MarketPhase {
        let s = self.state.read().unwrap();
        s.account.settings.calendar.phase_at(self.now())
    }

    /// How long the poller should wait before the next refresh.
    pub fn next_refresh_in(&self) -> std::time::Duration {
        poll_interval(self.phase())
    }

    /// Runs forever: refresh, then sleep according to the market phase.
    pub async fn run_poller(self: Arc<Self>) {
        loop {
            let _ = self.refresh().await;
            tokio::time::sleep(self.next_refresh_in()).await;
        }
    }

    /// Fetches all feeds, updates the cache, fills paper orders and stores
    /// snapshots. Individual feed failures are recorded, not fatal.
    pub async fn refresh(&self) -> Result<()> {
        self.refresh_market().await?;
        self.update_alerts().await
    }

    async fn refresh_market(&self) -> Result<()> {
        let c = &self.client;
        let (status, aspi, snp, summary, quotes, gainers, losers, active) = tokio::join!(
            c.market_status(),
            c.aspi(),
            c.snp_sl20(),
            c.market_summary(),
            c.all_quotes(),
            c.top(cse_core::TopMoverKind::Gainers),
            c.top(cse_core::TopMoverKind::Losers),
            c.most_active(),
        );
        let now = self.now();
        let mut errors = Vec::new();
        let mut note = |name: &str, e: &cse_client::Error| errors.push(format!("{name}: {e}"));

        let mut guard = self.state.write().unwrap();
        let s = &mut *guard;
        let m = &mut s.market;
        match status {
            Ok(v) => m.status = Some(v),
            Err(e) => note("market status", &e),
        }
        match aspi {
            Ok(v) => m.aspi = Some(v),
            Err(e) => note("ASPI", &e),
        }
        match snp {
            Ok(v) => m.snp_sl20 = Some(v),
            Err(e) => note("S&P SL20", &e),
        }
        match summary {
            Ok(v) => m.summary = Some(v),
            Err(e) => note("market summary", &e),
        }
        match gainers {
            Ok(v) => m.gainers = v,
            Err(e) => note("top gainers", &e),
        }
        match losers {
            Ok(v) => m.losers = v,
            Err(e) => note("top losers", &e),
        }
        match active {
            Ok(v) => m.most_active = v,
            Err(e) => note("most active", &e),
        }
        let fresh_quotes = match quotes {
            // Before the open the CSE serves an empty list: keep the last prices.
            Ok(v) if v.is_empty() && !m.quotes.is_empty() => false,
            Ok(v) => {
                let mut fresh: HashMap<String, Quote> =
                    v.into_iter().map(|q| (q.symbol.clone(), q)).collect();
                // The list only has stocks that traded today; keep the last
                // known price of the others (e.g. favourites with no trades).
                for (sym, q) in m.quotes.drain() {
                    fresh.entry(sym).or_insert(q);
                }
                m.quotes = fresh;
                true
            }
            Err(e) => {
                note("prices", &e);
                false
            }
        };
        m.errors = errors;
        m.updated_at = Some(now);

        // CSE says closed (e.g. an unlisted holiday): expire but never fill.
        let exchange_closed = m
            .status
            .as_deref()
            .is_some_and(|t| t.to_ascii_lowercase().contains("close"));
        if fresh_quotes && !exchange_closed {
            s.account.on_market(now, &s.market.quotes);
        } else {
            s.account.on_market(now, &HashMap::new());
        }

        let snapshot_due = fresh_quotes
            && s.account.settings.calendar.phase_at(now) == MarketPhase::Open
            && s.last_snapshot
                .is_none_or(|t| now - t >= Duration::seconds(SNAPSHOT_EVERY));
        let mut store = self.store.lock().unwrap();
        if snapshot_due {
            let quotes: Vec<Quote> = s.market.quotes.values().cloned().collect();
            store.save_quotes(&quotes, now)?;
            for idx in [&s.market.aspi, &s.market.snp_sl20].into_iter().flatten() {
                store.save_index(idx, now)?;
            }
            store.prune_snapshots(now - Duration::days(SNAPSHOT_DAYS))?;
            s.last_snapshot = Some(now);
        }
        if fresh_quotes {
            self.save_session_closes(s, &mut store, now)?;
        }
        // Expiry or queue changes alter the account even without fills.
        store.save_json(ACCOUNT_KEY, &s.account)?;
        Ok(())
    }

    /// Replaces the market cache directly (tests and offline use).
    pub fn set_market(&self, market: MarketData) {
        self.state.write().unwrap().market = market;
    }

    /// Executes one UI command. `args` is a JSON object (or null).
    pub async fn dispatch(&self, cmd: &str, args: Value) -> Result<Value> {
        if cmd == "refresh" {
            self.refresh().await?;
            return self.dispatch_sync("market", Value::Null);
        }
        if cmd == "chart" {
            return self.chart(args).await;
        }
        if let Some(r) = self.alerts_command(cmd, args.clone()) {
            return r;
        }
        if cmd == "import_watch_list" {
            return self.import_watch_list(args).await;
        }
        if cmd == "quote" {
            // Fetch detail for symbols not in the all-share list (e.g. debt).
            let a: SymbolArg = parse(args.clone())?;
            let cached = self
                .state
                .read()
                .unwrap()
                .market
                .quotes
                .contains_key(&a.symbol);
            if !cached {
                let q = self.client.quote(&a.symbol).await?;
                self.state
                    .write()
                    .unwrap()
                    .market
                    .quotes
                    .insert(q.symbol.clone(), q);
            }
        }
        self.dispatch_sync(cmd, args)
    }

    fn dispatch_sync(&self, cmd: &str, args: Value) -> Result<Value> {
        let now = self.now();
        let out = match cmd {
            "market" => {
                let s = self.state.read().unwrap();
                let phase = s.account.settings.calendar.phase_at(now);
                to_json(MarketView::new(&s.market, phase))
            }
            "quotes" => {
                let s = self.state.read().unwrap();
                let mut q: Vec<&Quote> = s.market.quotes.values().collect();
                q.sort_by(|a, b| a.symbol.cmp(&b.symbol));
                to_json(q)
            }
            "quote" => {
                let a: SymbolArg = parse(args)?;
                let s = self.state.read().unwrap();
                let quote = s.market.quotes.get(&a.symbol);
                let history = self.store.lock().unwrap().price_history(
                    &a.symbol,
                    now - Duration::days(SNAPSHOT_DAYS),
                    now + Duration::seconds(1),
                )?;
                let acc = &s.account;
                to_json(json!({
                    "quote": quote,
                    "held": acc.portfolio().qty(&a.symbol),
                    "available_to_sell": acc.available_to_sell(&a.symbol),
                    "pending_buy": acc.pending_qty(&a.symbol, Side::Buy),
                    "pending_sell": acc.pending_qty(&a.symbol, Side::Sell),
                    "history": history.iter().map(|p| json!({
                        "at": p.at, "price": p.price, "volume": p.share_volume
                    })).collect::<Vec<_>>(),
                }))
            }
            "estimate" => {
                let a: EstimateArgs = parse(args)?;
                let s = self.state.read().unwrap();
                let acc = &s.account;
                let price = a
                    .price
                    .or_else(|| s.market.quotes.get(&a.symbol).map(|q| q.last));
                let (fees, net) = price
                    .map(|p| acc.estimate(a.side, a.qty, p))
                    .unwrap_or_default();
                let ticks = &acc.settings.ticks;
                to_json(json!({
                    "price": price,
                    "order_value": price.map(|p| Decimal::from(a.qty) * p),
                    "fees": fees,
                    "net": net,
                    "tick": price.map(|p| ticks.tick_for(p)),
                    "price_valid": price.is_some_and(|p| ticks.is_valid(p)),
                    "buying_power": acc.buying_power(),
                    "available_to_sell": acc.available_to_sell(&a.symbol),
                    "pending_buy": acc.pending_qty(&a.symbol, Side::Buy),
                    "pending_sell": acc.pending_qty(&a.symbol, Side::Sell),
                    "fee_pct": acc.settings.fees.total_pct(),
                }))
            }
            "place_order" => {
                let a: PlaceArgs = parse(args)?;
                let id = self.mutate_account(|s| {
                    let reference = a
                        .reference_price
                        .or_else(|| s.market.quotes.get(&a.order.symbol).map(|q| q.last));
                    let market_open_fill = matches!(a.order.order_type, OrderType::Market);
                    let id = s.account.place(a.order, reference, now)?;
                    if market_open_fill {
                        // Fill straight away when the market is open.
                        let quotes = s.market.quotes.clone();
                        s.account.on_market(now, &quotes);
                    }
                    Ok(id)
                })?;
                to_json(json!({ "id": id }))
            }
            "amend_order" => {
                let a: AmendArgs = parse(args)?;
                self.mutate_account(|s| Ok(s.account.amend(a.id, a.qty, a.price, now)?))?;
                Value::Null
            }
            "cancel_order" => {
                let a: IdArg = parse(args)?;
                self.mutate_account(|s| Ok(s.account.cancel(a.id, now)?))?;
                Value::Null
            }
            "orders" => {
                let s = self.state.read().unwrap();
                let mut orders: Vec<_> = s.account.orders().iter().collect();
                orders.reverse();
                to_json(orders)
            }
            "account" => {
                let s = self.state.read().unwrap();
                to_json(s.account.summary(&s.market.quotes))
            }
            "ledger" => {
                let s = self.state.read().unwrap();
                let mut l: Vec<_> = s.account.ledger().iter().collect();
                l.reverse();
                to_json(l)
            }
            "reset_account" => {
                let a: ResetArgs = parse(args)?;
                self.mutate_account(|s| {
                    s.account.reset(a.starting_cash, now);
                    Ok(())
                })?;
                Value::Null
            }
            "real_portfolio" => {
                let s = self.state.read().unwrap();
                let settings = &s.account.settings;
                let p = s.real.portfolio(&settings.fees)?;
                to_json(json!({
                    "valuation": p.valuation(&s.market.quotes, &settings.fees, &settings.ticks),
                    "transactions": s.real.transactions(),
                }))
            }
            "add_transaction" => {
                let a: TxArgs = parse(args)?;
                let id = self.mutate_real(|real, fees| Ok(real.add(a.tx, fees)?))?;
                to_json(json!({ "id": id }))
            }
            "remove_transaction" => {
                let a: IdArg = parse(args)?;
                self.mutate_real(|real, fees| Ok(real.remove(a.id, fees)?))?;
                Value::Null
            }
            "import_csv" => {
                let a: CsvArgs = parse(args)?;
                let n = self.mutate_real(|real, fees| Ok(real.import_csv(&a.csv, fees)?))?;
                to_json(json!({ "imported": n }))
            }
            "export_csv" => to_json(self.state.read().unwrap().real.export_csv()),
            "watch_lists" => to_json(self.store.lock().unwrap().watch_lists()?),
            "create_watch_list" => {
                let a: NameArg = parse(args)?;
                to_json(json!({ "id": self.store.lock().unwrap().create_watch_list(&a.name)? }))
            }
            "rename_watch_list" => {
                let a: RenameArgs = parse(args)?;
                self.store
                    .lock()
                    .unwrap()
                    .rename_watch_list(a.id, &a.name)?;
                Value::Null
            }
            "delete_watch_list" => {
                let a: WatchIdArg = parse(args)?;
                self.store.lock().unwrap().delete_watch_list(a.id)?;
                Value::Null
            }
            "add_to_watch_list" => {
                let a: WatchSymbolArgs = parse(args)?;
                self.store
                    .lock()
                    .unwrap()
                    .add_to_watch_list(a.id, &a.symbol)?;
                Value::Null
            }
            "remove_from_watch_list" => {
                let a: WatchSymbolArgs = parse(args)?;
                self.store
                    .lock()
                    .unwrap()
                    .remove_from_watch_list(a.id, &a.symbol)?;
                Value::Null
            }
            "settings" => to_json(&self.state.read().unwrap().account.settings),
            "update_settings" => {
                let a: SettingsArgs = parse(args)?;
                self.mutate_account(|s| {
                    s.account.settings = a.settings;
                    Ok(())
                })?;
                Value::Null
            }
            other => return Err(ServiceError::UnknownCommand(other.to_owned())),
        };
        Ok(out)
    }

    /// Applies `f` to the state and saves the account if it succeeded.
    fn mutate_account<T>(&self, f: impl FnOnce(&mut State) -> Result<T>) -> Result<T> {
        let mut s = self.state.write().unwrap();
        let out = f(&mut s)?;
        self.store
            .lock()
            .unwrap()
            .save_json(ACCOUNT_KEY, &s.account)?;
        Ok(out)
    }

    fn mutate_real<T>(
        &self,
        f: impl FnOnce(&mut RealPortfolio, &cse_core::FeeSchedule) -> Result<T>,
    ) -> Result<T> {
        let mut guard = self.state.write().unwrap();
        let s = &mut *guard;
        let out = f(&mut s.real, &s.account.settings.fees)?;
        self.store.lock().unwrap().save_json(REAL_KEY, &s.real)?;
        Ok(out)
    }
}

pub(crate) fn parse<T: DeserializeOwned>(args: Value) -> Result<T> {
    serde_json::from_value(args).map_err(|e| ServiceError::BadArgs(e.to_string()))
}

fn to_json<T: Serialize>(v: T) -> Value {
    serde_json::to_value(v).expect("service views serialise")
}

#[derive(Deserialize)]
struct SymbolArg {
    symbol: String,
}

#[derive(Deserialize)]
struct IdArg {
    id: OrderId,
}

#[derive(Deserialize)]
struct WatchIdArg {
    id: i64,
}

#[derive(Deserialize)]
struct NameArg {
    name: String,
}

#[derive(Deserialize)]
struct RenameArgs {
    id: i64,
    name: String,
}

#[derive(Deserialize)]
struct WatchSymbolArgs {
    id: i64,
    symbol: String,
}

#[derive(Deserialize)]
struct EstimateArgs {
    symbol: String,
    side: Side,
    qty: u64,
    price: Option<Decimal>,
}

#[derive(Deserialize)]
struct PlaceArgs {
    order: OrderRequest,
    reference_price: Option<Decimal>,
}

#[derive(Deserialize)]
struct AmendArgs {
    id: OrderId,
    qty: u64,
    price: Option<Decimal>,
}

#[derive(Deserialize)]
struct ResetArgs {
    starting_cash: Decimal,
}

#[derive(Deserialize)]
struct TxArgs {
    tx: NewTransaction,
}

#[derive(Deserialize)]
struct CsvArgs {
    csv: String,
}

#[derive(Deserialize)]
struct SettingsArgs {
    settings: Settings,
}
