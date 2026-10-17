# CSE Trader

A personal Android app, written in Rust, for **paper trading** and **portfolio
tracking** on the Colombo Stock Exchange, with screens modelled on ATrad.
It uses CSE public market data only; it never connects to a broker or places
real orders.

See [`docs/atrad-spec.md`](docs/atrad-spec.md) for the feature spec.

## Workspace

| Crate | Purpose |
|---|---|
| `crates/core` (`cse-core`) | Models (quotes, indices, announcements), fee schedule, tick sizes, CSE market hours |
| `crates/cse-client` | Async client for CSE public data feeds, plus the `cse-probe` diagnostic tool |
| `crates/engine` (`cse-engine`) | Paper trading: limit/market orders, Day or N-day TIF, queuing while closed, amend/cancel, fills, cash and buying power, portfolio with average cost, B.E.S, realised/unrealised P&L |
| `crates/storage` (`cse-storage`) | SQLite: price and index snapshots for charts, watch lists, saved app state (paper account) |

Coming next: `app` (Tauri 2 Android UI).

## Paper-trading rules

- Orders fill in full at the last traded price: market orders at once, limit
  buys when last <= limit, limit sells when last >= limit.
- Orders placed outside 09:30–14:30 are **queued** and open at the next session.
- Day orders expire at that session's close; N-day orders at the close of the
  Nth trading session (weekends and holidays skipped).
- Buy orders reserve their value plus fees from buying power; sell orders
  reserve shares. No short selling.
- A market buy is rejected if the price moved so far that cash runs short.

## Development

```sh
cargo test           # all unit tests
cargo clippy --all-targets -- -D warnings
```

### Checking the CSE feeds

The CSE data feeds are undocumented. `cse-probe` fetches each one, saves the
raw JSON and reports whether our parsers understand it:

```sh
cargo run -p cse-client --bin cse-probe -- crates/cse-client/tests/fixtures JKH.N0000
```

Run it on a machine that can reach `www.cse.lk` (or allow that domain in the
cloud environment's network settings).

## Defaults to confirm

- Fee schedule: 1.12% per side (brokerage 0.64%, CSE 0.084%, CDS 0.024%,
  SEC 0.072%, STL 0.30%). Editable in `FeeSchedule`.
- Tick size: 0.10 below Rs. 100, 0.25 from Rs. 100. Editable in `TickTable`.
- Trading hours: pre-open 09:00, continuous 09:30–14:30 Sri Lanka time.
  Holidays are supplied to `MarketCalendar`.
