# ATrad feature spec (reference for our own app)

This document describes **what** the ATrad Android app does so that we can build
our own paper-trading and portfolio-tracking app with a similar experience. It
records screens, fields, flows and rules in our own words. It contains no ATrad
code, assets, server addresses or login details, and our app will not talk to
ATrad or broker servers.

## Source of this analysis

| Item | Value |
|---|---|
| Package | `com.ironone.atrad` (IronOne Technologies) |
| Version | 2.0.80 (code 2080), min SDK 24, target SDK 36 |
| APK SHA-256 | `5682645197212d86c5948791155afd67e1236288f26a6dd2ba2d4a5422cce3e9` (base APK only) |
| Framework | React Native + Expo, JS compiled to **Hermes bytecode v96** (`assets/index.android.bundle`) |
| UI libs seen | React Navigation (native stack, bottom tabs, material top tabs), React Native Paper, Victory / chart-kit charts, react-native-calendars, WebView (for full charts) |
| Tools | REA 6.5.0 `inspect-android-package` (JADX 0.7.1) for the manifest; `hermes-dec` for the Hermes bundle (string table + disassembly). Static analysis only; the app was never run. |

Permissions declared: internet, network state, biometric/fingerprint, vibrate,
storage read/write, overlay window, among 11 total.

## Navigation map

```
AtradNavigator
├── LoginStack
│   ├── BrokerSelection      pick broker (logo + name)
│   └── Login                username / password, biometric
├── newUser / pwExpired      first-login and password-expiry flows
└── TabStack
    ├── TabNavigator (bottom tabs)
    │   ├── Market      ── top tabs: Market Info | Indices | Trades
    │   ├── Full Watch  ── top tabs: Full Watch | Favourites
    │   ├── Orders      ── top tabs: Blotter | Recent Transactions | Account Summary
    │   ├── Portfolio   ── top tabs: Summary | Detailed | Valuation
    │   └── Buy/Sell
    ├── TopStocks       ── top tabs: Gainers | Losers | Turnover | Share Volume
    ├── Quote           ── top tabs: Quote | Market Depth | Time and Sales | Trade Summary
    ├── Announcements   ── tabs: Broker | CSE
    ├── UniversalSearch, ClientSearch
    ├── BuySellAmend, OrderList
    └── ChangePassword, AboutUs, Help
```

## Screens and fields

### Market (Market Info)
- Header: **CSE – Colombo Stock Exchange**, market status.
- Index cards: **ASPI** and **S&P SL20** with value, change, change %.
- **Turnover**, **Volume**, number of trades.
- **Market contribution / buy sentiment** bar: Positive / Negative / Neutral
  counts and totals.
- Shortcuts to Top Stocks and Announcements.

### Indices
- List of indices (incl. sector indices) with value and change, gradient tiles.
- Long-press / action: **Add to Favourites**.

### Trades (market-wide)
- Filter: **All / Positive / Negative / Neutral**, Equity.
- Row: symbol, last price, change, change %, trades, volume; tap → Quote, Buy/Sell.

### Full Watch
- Board filter: **Equity / Debt / Crossing**; "Filter by", "Sort by" sheet with
  **Symbol, Change %, Trades, Trades Time**; **Show Detailed View** toggle.
- Row: Symbol, Last, Chg, Chg %, Bid, Ask, Volume, Trades, Turnover;
  quick **BUY / SELL** buttons.

### Favourites (watch lists)
- Multiple named watch lists: **Select Watch List**, **Rename the watch list**,
  add/remove symbols, swipe actions.

### Top Stocks
- Tabs **Gainers, Losers, Turnover, Share Volume** (top ten each).

### Quote (stock detail)
- Header: symbol, company name, last traded price, change, change %.
- Fields: Open, High, Low, Prev. Closed, Last Trade, Last Quantity, Volume,
  Turnover, Trades, **Best Bid / Best Bid Qty, Best Ask / Best Ask Qty**,
  **52wk High / 52wk Low**, Issued Qty, **Security Status**.
- Chart (intraday / historic) with an option to open the full chart.
- Announcements list for the security ("No announcements to be shown.").
- Buy / Sell buttons.

### Market Depth (order book)
- Columns: **Bid Splits, Bid Qty, Bid Price | Ask Price, Ask Qty, Ask Splits**.
- Footer: **Total Bid Quantity, Total Ask Quantity**, High, Low, Turnover, Volume.

### Time and Sales
- List of trades for the security: time, price, quantity; with Open/High/Low/Close/Volume.

### Trade Summary
- Trades grouped by price: price, number of trades, volume;
  **Total Trades**, **Total Volume**.

### Buy / Sell ticket
- Side toggle **Buy / Sell** (green / red), security search.
- Live header: last price, change, High, Low, **Total Bid**, **Total Ask**.
- Inputs: **Order Type** (Limit / Market), **Quantity** (number pad),
  **Price** (decimal pad; hidden for Market), **TIF** (Day, or good-till-days
  with a **Days** field), optional **Disclose Qty**.
- Info: **Available Shares**, **Buying Power**, **Pending Buy Qty**,
  **Pending Sell Qty**.
- Calculated: **Order Value**, **Commission**, **Net Value**.
- Buttons: Buy/Sell (submit with confirmation "Are you sure you want to submit
  this order?"), **Clear**, **Cancel**; link to **Order List**.
- If the market/security is not open: "This security is not open for trading.
  Do you want to queue the order?" → order is **QUEUED**.

### Orders – Blotter
- Filter by **Order Status**; row: side, symbol, qty, price, filled qty, status, time.
- Row actions: **Amend**, **Cancel**, **Details**, **History**.
- Order Details: client order ID, exchange order ID, order date/time, TIF,
  filled qty, remaining qty, average price.
- Order History: status changes over time.

### Orders – Recent Transactions / Account Summary
- Account Summary: **Total Cost Of The Portfolio**, **Total Market Value Of The
  Portfolio**, **Total Gain/Loss**, **Cash Balance**, **Buying Power**,
  **Total Pending Buy Order Value**, Exposure / margin fields (broker margin
  accounts only).

### Portfolio – Summary
- **Total Cost**, **Market Value**, **Sales Commission**, **Sales Proceeds**,
  **Unrealized Gain/(Loss)**, **Today Gain/(Loss)**.

### Portfolio – Detailed
- Per holding: Symbol, **Holding Quantity**, **Pending Buy / Pending Sell**,
  **Avg. Cost**, **B.E.S Price** (break-even selling price), **Last Traded Price**,
  **Total Cost**, **Market Value**, **Unrealized Gain/Loss**, **Gain/Loss %**,
  **Holdings %**.

### Portfolio – Valuation
- Allocation: **Holding % (Market Value)** and **Holding % (Quantity)**, shown as a
  pie chart.

### Announcements
- **Broker** and **CSE** announcements; Latest / Today; calendar date picker
  and date range ("Maximum date range exceeded").

### Tools in the menu (web version, optional for us)
Average Cost Calculator, What-if Calculator, Alerts Manager / Set Alert,
Sector Watch, Ticker Watch, OHLC values, Statistics, Change Color Theme
(light / dark).

## Order lifecycle

Statuses seen: **NEW → OPEN / PENDING → P.FILLED → FILLED**, plus **QUEUED**
(placed while market closed), **Q.AMEND** (amend while queued), **AMENDED**,
**CANCELED**, **REJECTED**, **EXPIRED** (TIF ran out).

Amend rules: new quantity must be greater than the filled quantity.

## Validation rules seen in the app

| Rule | Message (paraphrased) |
|---|---|
| Quantity > 0 and a valid integer | "Quantity you entered is not valid." |
| Price must be a valid number | "Price you entered is not valid." |
| Price must be a multiple of the tick size (0.10 or 0.25 depending on price band) | "Price should be multiples of …" |
| Sell qty ≤ available shares (else short-sell warning) | "Order quantity can not greater than available share balance" |
| Buy order value ≤ buying power | "Order value can not exceed buying power" |
| Disclose qty ≤ order qty | "Disclose quantity can not be greater than order quantity" |
| Per-order and per-day limits (broker-set) | "Your per order limit exceeds" |
| Crossing / block board minimum value (10M / 20M) | "Order value should be greater than 10 million" |

Commission in ATrad is calculated by the broker's server, not in the app. Our
app computes it locally from an editable fee schedule (see below).

## What we build (mapping to our app)

| ATrad feature | Our app |
|---|---|
| Broker selection + login | **Not needed.** Local app, optional PIN/biometric lock |
| Market Info, Indices, Trades, Top Stocks | Same, from CSE public market data |
| Full Watch + Favourites | Same (local watch lists in SQLite) |
| Quote, Market Depth, Time & Sales, Trade Summary | Same where public data allows |
| Buy/Sell ticket | Same layout; orders go to the **paper-trading engine** |
| Blotter / order history / amend / cancel | Same, local |
| Account Summary | Paper cash, buying power, pending buy value |
| Portfolio Summary / Detailed / Valuation | Same, for **Paper** and **Real (manual)** portfolios |
| Announcements | CSE announcements only |
| Avg Cost / What-if calculators, alerts, dark theme | Phase 6 extras |
| Margin, exposure, crossing board, basket/bulk orders | Out of scope |

### Formulas (ours)
- **Fee rate** `f` (total of brokerage + CSE + SEC + CDS + STL levies),
  editable; default from the current CSE published schedule (to be confirmed).
- Buy: `net = qty × price × (1 + f)`; Sell: `net = qty × price × (1 − f)`.
- **Average cost** = total buy net cost ÷ quantity held (FIFO not needed;
  weighted average like ATrad).
- **B.E.S price** = `avg_cost / (1 − f)`, rounded **up** to the next tick.
- **Unrealized G/L** = `qty × last × (1 − f) − qty × avg_cost`.
- **Today G/L** = `qty × (last − prev_close)`.
- **Tick size**: price-band table, editable (CSE rule: 0.10 below a threshold,
  0.25 above; to be confirmed against current CSE rules).

### Market rules for the paper engine
- Trading hours 09:30–14:30 Asia/Colombo, Mon–Fri excluding CSE holidays;
  orders placed outside hours are **QUEUED** and become OPEN at the open.
- Limit buy fills when last ≤ limit; limit sell fills when last ≥ limit;
  market orders fill at best ask / best bid (or last if no depth).
- TIF: Day (expires at close) or N days.
