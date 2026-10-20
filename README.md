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
| `crates/signals` (`cse-signals`) | Alert signals: breakout, pullback, trend turn, oversold bounce (daily); opening-range breakout, volume spike (15-min); filters and one-year track records |
| `crates/service` (`cse-service`) | App backend: market data cache and poller, paper engine, real portfolio, alerts, JSON command API |
| `crates/dev-server` | Runs the app in a desktop browser at http://127.0.0.1:8787 |
| `app/ui` | Mobile UI (HTML/CSS/JS): Market, Watch, Quote, Buy/Sell, Orders, Portfolio, Settings |
| `app/src-tauri` | Tauri 2 Android/desktop shell (built separately, see [`docs/android-build.md`](docs/android-build.md)) |

## Try it

```sh
cargo run -p cse-dev-server    # then open http://127.0.0.1:8787
```

To build the Android APK, follow [`docs/android-build.md`](docs/android-build.md).

## Paper-trading rules

- Orders fill in full at the last traded price: market orders at once, limit
  buys when last <= limit, limit sells when last >= limit.
- Orders placed outside 09:30–14:30 are **queued** and open at the next session.
- Day orders expire at that session's close; N-day orders at the close of the
  Nth trading session (weekends and holidays skipped).
- Buy orders reserve their value plus fees from buying power; sell orders
  reserve shares. No short selling.
- A market buy is rejected if the price moved so far that cash runs short.

## Price alerts

Tap 🔔 to see alert levels for every stock in your watch lists. Each level has
a plain-English reason and that signal's one-year track record on the stock
(share of signals that beat the ~2.24% round-trip fees 10 trading days later).
Signals are skipped for thinly traded stocks; intraday signals are skipped when
a stock's typical daily move is smaller than the fees. You can switch each
signal off, add your own one-off price levels, and see the alert history.

**Background mode** (Android, off by default) keeps checking prices during
market hours when the app is closed. It runs as a foreground service with a
permanent notification and a Stop button, holds a CPU wake lock only from
09:25 to 14:40 on weekdays, and turns alerts written by the Rust service into
notifications. Some phone brands stop background apps aggressively: allow
CSE Trader to run without battery restrictions when Android asks.

Signals are prompts to look at a stock, not buy recommendations.

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
