//! Command definitions for the `cse` CLI, mapped onto the app's JSON
//! command API (`AppService::dispatch`).

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use cse_service::AppService;
use serde_json::{json, Value};

#[derive(Parser, Debug)]
#[command(
    name = "cse",
    version,
    about = "CSE Trader from the command line (JSON output)"
)]
pub struct Cli {
    /// Database file (default: ~/.local/share/cse-trader/cse-trader.sqlite).
    #[arg(long, global = true, env = "CSE_TRADER_DB")]
    pub db: Option<PathBuf>,
    /// One-line JSON instead of pretty JSON.
    #[arg(long, global = true)]
    pub compact: bool,
    /// Fetch the latest CSE (and US) data before the command.
    #[arg(long, global = true)]
    pub refresh: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Fetch the latest market data once.
    Refresh,
    /// One full check: refresh, update every favourite, fire alerts.
    Check,
    /// Market overview: status, ASPI, S&P SL20, breadth, top movers.
    Market,
    /// Latest quotes (all, or the given symbols).
    Quotes { symbols: Vec<String> },
    /// One quote with paper-holding details.
    Quote { symbol: String },
    /// Price history: ASPI or a symbol.
    Chart {
        /// Symbol, or ASPI.
        symbol: String,
        #[arg(long, value_enum, default_value = "1M")]
        range: Range,
    },
    /// Watch lists.
    Lists,
    /// Create a watch list.
    ListCreate { name: String },
    /// Add symbols to a watch list (by id or name).
    ListAdd { list: String, symbols: Vec<String> },
    /// Remove symbols from a watch list.
    ListRemove { list: String, symbols: Vec<String> },
    /// Import company codes into a watch list (replaces it unless --add).
    ListImport {
        name: String,
        /// Codes separated by spaces, commas or new lines (e.g. "JKH COMB").
        #[arg(long, conflicts_with = "file")]
        codes: Option<String>,
        /// File with codes.
        #[arg(long)]
        file: Option<PathBuf>,
        /// Add to the list instead of replacing it.
        #[arg(long)]
        add: bool,
    },
    /// Market-structure marker for every favourite (look now, get ready, ...).
    Markers {
        /// Only this status (look_now, get_ready, uptrend, broken, range,
        /// downtrend, breaking_down, thin).
        #[arg(long)]
        status: Option<String>,
        /// Refresh rounds allowed to finish downloading history.
        #[arg(long, default_value_t = 15)]
        rounds: u32,
    },
    /// Structure, setups, plan and track record for one favourite.
    Structure { symbol: String },
    /// Swing log, trend periods and alerts for one symbol.
    History { symbol: String },
    /// Alert settings, favourites' levels, custom alerts and history.
    Alerts,
    /// Alert results: target/stop hits and returns after fees.
    Results,
    /// All history (swings, daily structure, alerts) as CSV.
    ExportHistory {
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// A one-off price alert.
    AlertAdd {
        symbol: String,
        #[arg(long, conflicts_with = "below")]
        above: Option<String>,
        #[arg(long)]
        below: Option<String>,
    },
    /// Real portfolio valuation and transactions.
    Real,
    /// Record a real trade.
    RealAdd {
        symbol: String,
        #[arg(value_enum)]
        side: Side,
        qty: u64,
        price: String,
        /// Trade date (YYYY-MM-DD).
        #[arg(long)]
        date: String,
        /// Actual fees (default: the fee schedule).
        #[arg(long)]
        fees: Option<String>,
    },
    /// Import real trades from CSV (date,symbol,side,qty,price,fees).
    RealImport { file: PathBuf },
    /// Export real trades as CSV.
    RealExport {
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Paper account: cash, buying power, holdings.
    Account,
    /// Paper orders.
    Orders,
    /// Place a paper order (market unless --limit).
    Order {
        symbol: String,
        #[arg(value_enum)]
        side: Side,
        qty: u64,
        #[arg(long)]
        limit: Option<String>,
        /// Good for N trading sessions (default: today only).
        #[arg(long)]
        days: Option<u32>,
    },
    /// Cancel a paper order.
    Cancel { id: u64 },
    /// US stocks: save a Twelve Data API key.
    UsKey { key: String },
    /// US stocks: search by ticker or name.
    UsSearch { text: String },
    /// US stocks: key status and requests used today.
    UsStatus,
    /// Any app command with JSON arguments (see `cse commands`).
    Call {
        cmd: String,
        /// JSON arguments, e.g. '{"symbol":"JKH.N0000"}'.
        args: Option<String>,
    },
    /// Every app command name, for `cse call`.
    Commands,
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum Range {
    #[value(name = "1D")]
    Day,
    #[value(name = "1W")]
    Week,
    #[value(name = "1M")]
    Month,
    #[value(name = "3M")]
    Quarter,
    #[value(name = "1Y")]
    Year,
    #[value(name = "All")]
    All,
}

impl Range {
    fn name(self) -> &'static str {
        match self {
            Range::Day => "1D",
            Range::Week => "1W",
            Range::Month => "1M",
            Range::Quarter => "3M",
            Range::Year => "1Y",
            Range::All => "All",
        }
    }
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    fn name(self) -> &'static str {
        match self {
            Side::Buy => "Buy",
            Side::Sell => "Sell",
        }
    }
}

/// App command names accepted by `cse call`.
pub const COMMANDS: &[&str] = &[
    "refresh",
    "market",
    "quotes",
    "quote",
    "chart",
    "estimate",
    "place_order",
    "amend_order",
    "cancel_order",
    "orders",
    "account",
    "ledger",
    "reset_account",
    "real_portfolio",
    "add_transaction",
    "remove_transaction",
    "import_csv",
    "export_csv",
    "watch_lists",
    "create_watch_list",
    "rename_watch_list",
    "delete_watch_list",
    "add_to_watch_list",
    "remove_from_watch_list",
    "import_watch_list",
    "settings",
    "update_settings",
    "alerts",
    "update_alert_config",
    "add_custom_alert",
    "remove_custom_alert",
    "clear_alert_history",
    "alerts_pending",
    "favourite_markers",
    "structure_history",
    "alert_results",
    "export_history",
    "us_settings",
    "set_us_key",
    "set_us_checks",
    "us_search",
    "atrad_parse",
    "atrad_apply",
];

/// `jkh` -> `JKH.N0000`; full symbols (`JKH.X0000`, `AAPL.US`) and `ASPI`
/// as given.
pub fn symbol(s: &str) -> String {
    let t = s.trim().to_ascii_uppercase();
    if t.contains('.') || t == "ASPI" {
        t
    } else {
        format!("{t}.N0000")
    }
}

async fn call(svc: &AppService, cmd: &str, args: Value) -> Result<Value, String> {
    svc.dispatch(cmd, args).await.map_err(|e| e.to_string())
}

/// A watch list id from an id or a name.
async fn list_id(svc: &AppService, list: &str) -> Result<i64, String> {
    if let Ok(id) = list.parse::<i64>() {
        return Ok(id);
    }
    let lists = call(svc, "watch_lists", Value::Null).await?;
    lists
        .as_array()
        .into_iter()
        .flatten()
        .find(|l| {
            l["name"]
                .as_str()
                .is_some_and(|n| n.eq_ignore_ascii_case(list))
        })
        .and_then(|l| l["id"].as_i64())
        .ok_or_else(|| format!("no watch list named {list:?}"))
}

/// Refreshes until every favourite has its history and analysis (each
/// refresh prepares a batch), at most `rounds` times.
async fn ensure_markers(svc: &AppService, rounds: u32) -> Result<Value, String> {
    let mut markers = call(svc, "favourite_markers", Value::Null).await?;
    for _ in 0..rounds {
        if markers["counts"]["pending"].as_u64().unwrap_or(0) == 0
            && markers["counts"].as_object().is_some_and(|c| !c.is_empty())
        {
            break;
        }
        call(svc, "refresh", Value::Null).await?;
        markers = call(svc, "favourite_markers", Value::Null).await?;
    }
    Ok(markers)
}

fn write_or_return(text: &str, out: &Option<PathBuf>) -> Result<Value, String> {
    match out {
        Some(path) => {
            std::fs::write(path, text).map_err(|e| e.to_string())?;
            Ok(json!({ "written": path, "bytes": text.len() }))
        }
        None => Ok(Value::String(text.to_owned())),
    }
}

/// Prices are needed but none are stored yet (a new database, or no
/// session saved since): fetch them first.
async fn ensure_prices(svc: &AppService) -> Result<(), String> {
    let q = call(svc, "quotes", Value::Null).await?;
    if q.as_array().is_none_or(|a| a.is_empty()) {
        call(svc, "refresh", Value::Null).await?;
    }
    Ok(())
}

/// Rounds analysis floats (e.g. 48.480000000000004) to 4 decimals.
pub fn tidy(v: Value) -> Value {
    match v {
        Value::Number(n) if n.is_f64() => {
            let x = (n.as_f64().unwrap_or(0.0) * 10_000.0).round() / 10_000.0;
            json!(x)
        }
        Value::Array(a) => Value::Array(a.into_iter().map(tidy).collect()),
        Value::Object(o) => Value::Object(o.into_iter().map(|(k, v)| (k, tidy(v))).collect()),
        other => other,
    }
}

/// Runs one parsed command against the service.
pub async fn run(svc: &AppService, cli: &Cli) -> Result<Value, String> {
    run_inner(svc, cli).await.map(tidy)
}

async fn run_inner(svc: &AppService, cli: &Cli) -> Result<Value, String> {
    if cli.refresh && !matches!(cli.command, Command::Refresh | Command::Check) {
        call(svc, "refresh", Value::Null).await?;
    } else if matches!(
        cli.command,
        Command::Market
            | Command::Quotes { .. }
            | Command::Quote { .. }
            | Command::Real
            | Command::Account
            | Command::Orders
            | Command::Order { .. }
    ) {
        ensure_prices(svc).await?;
    }
    match &cli.command {
        Command::Refresh => call(svc, "refresh", Value::Null).await,
        Command::Check => {
            let before = call(svc, "alerts_pending", json!({})).await?;
            let after_id = before["latest_id"].as_u64().unwrap_or(0);
            let n = svc.run_check().await.map_err(|e| e.to_string())?;
            let alerts = call(svc, "alerts_pending", json!({ "after_id": after_id })).await?;
            Ok(json!({ "new_alerts": n, "alerts": alerts["alerts"] }))
        }
        Command::Market => call(svc, "market", Value::Null).await,
        Command::Quotes { symbols } => {
            let all = call(svc, "quotes", Value::Null).await?;
            if symbols.is_empty() {
                return Ok(all);
            }
            let want: Vec<String> = symbols.iter().map(|s| symbol(s)).collect();
            Ok(Value::Array(
                all.as_array()
                    .into_iter()
                    .flatten()
                    .filter(|q| {
                        q["symbol"]
                            .as_str()
                            .is_some_and(|s| want.iter().any(|w| w == s))
                    })
                    .cloned()
                    .collect(),
            ))
        }
        Command::Quote { symbol: s } => call(svc, "quote", json!({ "symbol": symbol(s) })).await,
        Command::Chart { symbol: s, range } => {
            let sym = symbol(s);
            let args = if sym == "ASPI" {
                json!({ "range": range.name() })
            } else {
                json!({ "symbol": sym, "range": range.name() })
            };
            call(svc, "chart", args).await
        }
        Command::Lists => call(svc, "watch_lists", Value::Null).await,
        Command::ListCreate { name } => {
            call(svc, "create_watch_list", json!({ "name": name })).await
        }
        Command::ListAdd { list, symbols } | Command::ListRemove { list, symbols } => {
            let id = list_id(svc, list).await?;
            let cmd = if matches!(cli.command, Command::ListAdd { .. }) {
                "add_to_watch_list"
            } else {
                "remove_from_watch_list"
            };
            for s in symbols {
                call(svc, cmd, json!({ "id": id, "symbol": symbol(s) })).await?;
            }
            let lists = call(svc, "watch_lists", Value::Null).await?;
            Ok(lists
                .as_array()
                .into_iter()
                .flatten()
                .find(|l| l["id"].as_i64() == Some(id))
                .cloned()
                .unwrap_or(Value::Null))
        }
        Command::ListImport {
            name,
            codes,
            file,
            add,
        } => {
            let text = match (codes, file) {
                (Some(c), _) => c.clone(),
                (None, Some(f)) => std::fs::read_to_string(f).map_err(|e| e.to_string())?,
                (None, None) => return Err("give --codes or --file".into()),
            };
            call(
                svc,
                "import_watch_list",
                json!({ "name": name, "text": text, "replace": !add }),
            )
            .await
        }
        Command::Markers { status, rounds } => {
            let mut m = ensure_markers(svc, *rounds).await?;
            if let Some(want) = status {
                if let Some(map) = m["markers"].as_object_mut() {
                    map.retain(|_, v| v["status"].as_str() == Some(want.as_str()));
                }
            }
            Ok(m)
        }
        Command::Structure { symbol: s } => {
            let sym = symbol(s);
            ensure_markers(svc, 15).await?;
            let view = call(svc, "alerts", Value::Null).await?;
            view["favourites"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|f| f["symbol"] == sym.as_str())
                .map(|f| {
                    json!({
                        "symbol": sym,
                        "name": f["name"],
                        "price": f["price"],
                        "structure": f["structure"],
                        "now": f["structure_view"],
                        "track_record": f["structure_records"],
                        "error": f["error"],
                    })
                })
                .ok_or_else(|| format!("{sym} is not in a watch list; add it with `cse list-add`"))
        }
        Command::History { symbol: s } => {
            call(svc, "structure_history", json!({ "symbol": symbol(s) })).await
        }
        Command::Alerts => call(svc, "alerts", Value::Null).await,
        Command::Results => call(svc, "alert_results", Value::Null).await,
        Command::ExportHistory { out } => {
            let v = call(svc, "export_history", Value::Null).await?;
            write_or_return(v["csv"].as_str().unwrap_or(""), out)
        }
        Command::AlertAdd {
            symbol: s,
            above,
            below,
        } => {
            let (is_above, price) = match (above, below) {
                (Some(p), None) => (true, p),
                (None, Some(p)) => (false, p),
                _ => return Err("give --above PRICE or --below PRICE".into()),
            };
            call(
                svc,
                "add_custom_alert",
                json!({ "symbol": symbol(s), "above": is_above, "price": price }),
            )
            .await
        }
        Command::Real => call(svc, "real_portfolio", Value::Null).await,
        Command::RealAdd {
            symbol: s,
            side,
            qty,
            price,
            date,
            fees,
        } => {
            call(
                svc,
                "add_transaction",
                json!({ "tx": { "date": date, "symbol": symbol(s), "side": side.name(),
                                "qty": qty, "price": price, "fees": fees } }),
            )
            .await
        }
        Command::RealImport { file } => {
            let csv = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
            call(svc, "import_csv", json!({ "csv": csv })).await
        }
        Command::RealExport { out } => {
            let v = call(svc, "export_csv", Value::Null).await?;
            write_or_return(v.as_str().unwrap_or(""), out)
        }
        Command::Account => call(svc, "account", Value::Null).await,
        Command::Orders => call(svc, "orders", Value::Null).await,
        Command::Order {
            symbol: s,
            side,
            qty,
            limit,
            days,
        } => {
            let order_type = match limit {
                Some(p) => json!({ "Limit": p }),
                None => json!("Market"),
            };
            let tif = match days {
                Some(n) => json!({ "Days": n }),
                None => json!("Day"),
            };
            call(
                svc,
                "place_order",
                json!({ "order": { "symbol": symbol(s), "side": side.name(), "qty": qty,
                                   "order_type": order_type, "tif": tif } }),
            )
            .await
        }
        Command::Cancel { id } => call(svc, "cancel_order", json!({ "id": id })).await,
        Command::UsKey { key } => call(svc, "set_us_key", json!({ "api_key": key })).await,
        Command::UsSearch { text } => call(svc, "us_search", json!({ "text": text })).await,
        Command::UsStatus => call(svc, "us_settings", Value::Null).await,
        Command::Call { cmd, args } => {
            let args = match args {
                Some(a) => {
                    serde_json::from_str(a).map_err(|e| format!("bad JSON arguments: {e}"))?
                }
                None => Value::Null,
            };
            call(svc, cmd, args).await
        }
        Command::Commands => Ok(json!(COMMANDS)),
    }
}
