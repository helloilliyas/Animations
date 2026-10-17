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
| `crates/storage` (`cse-storage`) | SQLite: price and index snapshots for charts, watch lists |

Coming next: `engine` (paper-trading orders and portfolio) and `app` (Tauri 2
Android UI).

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
