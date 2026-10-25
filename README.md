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
| `crates/signals` (`cse-signals`) | Market structure (zigzag swings, trend, higher-low and breakout setups with trade plans) plus indicator signals; filters and one-year track records |
| `crates/cli` (`cse-cli`) | `cse` command-line interface with JSON output (see `docs/cli.md`) |
| `crates/service` (`cse-service`) | App backend: market data cache and poller, paper engine, real portfolio, alerts, JSON command API |
| `crates/dev-server` | Runs the app in a desktop browser at http://127.0.0.1:8787 |
| `app/ui` | Mobile UI (HTML/CSS/JS): Market, Watch, Quote, Buy/Sell, Orders, Portfolio, Settings |
| `app/src-tauri` | Tauri 2 Android/desktop shell (built separately, see [`docs/android-build.md`](docs/android-build.md)) |

## Try it

```sh
cargo run -p cse-dev-server    # then open http://127.0.0.1:8787
```

To build the Android APK, follow [`docs/android-build.md`](docs/android-build.md).

**Command line:** `cse` runs the same engine from a terminal with JSON
output, for scripts and AI agents (`cargo install --path crates/cli`). See
[`docs/cli.md`](docs/cli.md).

## Paper-trading rules

- Orders fill in full at the last traded price: market orders at once, limit
  buys when last <= limit, limit sells when last >= limit.
- Orders placed outside 09:30–14:30 are **queued** and open at the next session.
- Day orders expire at that session's close; N-day orders at the close of the
  Nth trading session (weekends and holidays skipped).
- Buy orders reserve their value plus fees from buying power; sell orders
  reserve shares. No short selling.
- A market buy is rejected if the price moved so far that cash runs short.

## Market structure alerts

Every stock in your watch lists is checked for **market structure**: price
swings of at least max(5%, 2× the stock's typical daily range) are marked as
higher highs (HH), higher lows (HL), lower highs (LH) and lower lows (LL).
Higher highs plus higher lows make an uptrend.

| Signal | Fires when | Default |
| --- | --- | --- |
| Higher low entry | In an uptrend, a pullback holds above the last low and bounces half a swing | on |
| Uptrend confirmed | After a higher low, price clears the last high by 1% | on |
| Downtrend broken | In a downtrend, price clears the last lower high | off |

Each signal comes with a plan: entry, stop just below the swing low, target
at the last high (or the prior advance for breakouts), and fires only when
reward/risk is at least 2× and the target covers the ~2.24% round-trip fees.
It fires once per swing, and only for stocks trading at least Rs. 1M a day
on average. ⭐ means the weekly chart is in an uptrend too.

**Watch list markers** show which favourites to look at, next to the name:
● Look now (a setup triggered), ◐ Get ready (within 2% of a trigger),
↗ uptrend, ↔ range, ↘ downtrend, and "thin" for thinly traded stocks. Sort by
"Look first" and filter with the chips. The stock screen shows the structure,
the plan, the one-year track record, and swing dots on the chart.

**Uptrend broken.** As soon as a stock in an uptrend trades below its last
higher low, it is marked ⚠ Uptrend broken, before the next swing confirms
it. An optional notification (off by default) says so once per swing. A
downtrend rising above its last lower high shows as "Downtrend breaking".
A range or downtrend falling below its last swing low shows as ⚠ Breaking
down ("avoid until a higher low forms"). Each swing in the card shows its
date.

**History.** The app keeps, on the phone:

- a swing log: every confirmed swing with its date, price, label and the
  trend after it (the first run reconstructs the past year from CSE prices);
- one structure row per trading day (trend, weekly trend, marker), shown on
  the stock screen as trend periods, e.g. "⚠ Uptrend broken 13–21 Oct";
- every alert, with its result: target or stop hit first (a day touching
  both counts as stopped), best and worst move, and the return after 5, 10
  and 20 trading days, after fees. 🔔 → Alert results summarises each signal.

Everything can be exported as CSV (🔔 → Export history).

**Prices before the open.** The CSE serves empty lists for about 30 minutes
before the session. The app keeps a copy of the whole market view (every
quote, the indices and the top lists), saved every 5 minutes in the session
and once after the close. On start it shows that copy, labelled "saved
prices", until the CSE serves the new session's prices. Paper orders never
fill on saved prices.

The earlier indicator signals (breakout, pullback, trend turn, oversold
bounce, opening range, volume spike) are still available under 🔔 → Other
indicators, off by default. You can also add one-off price levels.

**Background checks** (Android) when the app is closed:

- **Every 15, 30 (default) or 60 minutes** during trading hours (09:30–14:30,
  weekdays), plus one check after the close on the closing prices. Uses
  Android WorkManager; no permanent notification.
- **Live** keeps checking every few seconds as a foreground service with a
  permanent notification and a Stop button.

Each check sends one notification: the setup in full for one stock, or
"🔔 N stocks to look at" for several. Some phone brands stop background work
aggressively: allow CSE Trader to run without battery restrictions when
Android asks.

Signals are prompts to look at a stock, not buy recommendations.

## Orders and portfolio sync with ATrad

**Buy / Sell** on a CSE stock opens a ticket for your real broker app,
ATrad: the price is filled in (last price, the plan's entry, or target /
stop / break-even for sells), you type the quantity, and it shows the cost
or proceeds after fees. **Place in ATrad** copies the order (e.g. `BUY DIPD
1,000 @ 56.50`), opens ATrad and, with the ATrad reader on, goes to Buy Sell,
picks exactly that symbol (ACL.N0000, never ACL.X0000) in ATrad's stock
search and checks it is selected, sets the Buy/Sell switch (only when it
can be told apart from the submit button, which shares its word), fills in
quantity and price, and checks ATrad's Order Value equals quantity x price.
It never taps the submit button and never enters a PIN: you confirm the
order in ATrad. Any step that cannot be confirmed stops the filling.
Back in CSE Trader it asks whether the order was placed and records the fill
in the Real portfolio, optionally with alerts at the plan's stop and target.
"Practise on paper instead" opens the paper ticket.

**Sync from ATrad** (Portfolio → Real) reads your holdings from ATrad's
Portfolio → Detailed screen with an Android accessibility service that only
works inside ATrad (`res/xml/atrad_accessibility.xml`). It taps Sign in when
your login is already filled in, opens Portfolio → Detailed, scrolls and
reads the holdings (opening each row for its average cost), then returns. You review what was read (symbol, quantity, average cost)
against the Real portfolio and choose what to update; changes are added as
adjustment trades dated today with no fees, so history stays. Turn it on
once in Android Settings → Accessibility → Installed apps; for sideloaded
apps Android may first require Settings → Apps → CSE Trader → ⋮ → Allow
restricted settings.

## US stocks (watching and alerts)

US stocks come from [Twelve Data](https://twelvedata.com) with your own free
API key (Settings → US stocks; it is stored only on the phone). Tap **+ US**
on the Watch tab, search by ticker or name, and add stocks to any watch
list. They show as e.g. "AAPL 🇺🇸" with USD prices and get the same market
structure markers, alerts, history and charts as CSE stocks. Paper orders
are for CSE stocks only.

The free plan allows 8 requests a minute and 800 a day, and the app keeps
within that: one daily-history download per stock per New York trading day,
and quotes during the US session (09:30–16:00 New York, 7:00 pm–1:30 am Sri
Lanka time in summer, 8:00 pm–2:30 am in winter) plus one after the close,
in batches of 8. While the app is open (or in Live mode) the day's remaining
requests are spread over the rest of the session: about every 11 minutes per
stock for 20 stocks, never more often than every 5 minutes. Settings shows how many requests were
used today.

**US checks** (🔔, separate from the CSE checks) set how often US stocks are
checked when the app is closed: Off, 1 h, 30, 15 (default), 10 or 5 minutes,
during US hours plus one check after the US close. Each choice shows its
cost for your number of US stocks (20 stocks: about 300 requests a day at
30 minutes, 560 at 15); choices the free plan cannot support are greyed
out. 15 minutes and up use Android's job scheduler; 10 and 5 minutes keep a
"Watching US stocks" notification and the phone awake during the session
only. Live mode is for the CSE.



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
