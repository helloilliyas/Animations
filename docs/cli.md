# `cse` — CSE Trader on the command line

`cse` runs the same engine as the phone app (market data, market-structure
markers, alerts, history, paper and real portfolios, US stocks) from a
terminal. Every command prints **JSON**, so it works in scripts and for AI
coding agents such as Claude Code; errors go to stderr as
`{"error": "..."}` with exit code 1.

It keeps its own database on the computer (not the phone's):
`~/.local/share/cse-trader/cse-trader.sqlite`, or `--db FILE`, or the
`CSE_TRADER_DB` environment variable.

## Install

```sh
cargo install --path crates/cli     # installs the `cse` command
cse --help
```

## Quick start

```sh
cse list-import "Shariah" --codes "ABL WATA DIPD CLND RHL RIL COCO ACL"
cse markers                     # look now / get ready / uptrend / broken / ...
cse markers --status uptrend    # just one status
cse structure WATA              # trend, swings, setup plan, track record
cse history WATA                # trend periods, swing log, alerts with results
cse quotes WATA DIPD
cse chart ASPI --range 1W
cse check                       # refresh + alerts, like a background check
```

Symbols: a bare code means the normal share class (`wata` → `WATA.N0000`);
give the full symbol for others (`ACL.X0000`) and US stocks (`AAPL.US`).

## Commands

| Area | Commands |
|---|---|
| Data | `refresh`, `check`, `market`, `quotes [SYMBOL…]`, `quote SYMBOL`, `chart SYMBOL\|ASPI --range 1D\|1W\|1M\|3M\|1Y\|All` |
| Watch lists | `lists`, `list-create NAME`, `list-add LIST SYMBOL…`, `list-remove LIST SYMBOL…`, `list-import NAME --codes "…" \| --file F [--add]` (LIST is an id or name) |
| Structure | `markers [--status S]`, `structure SYMBOL`, `history SYMBOL` |
| Alerts | `alerts`, `alert-add SYMBOL --above P \| --below P`, `results`, `export-history [--out F]` |
| Real portfolio | `real`, `real-add SYMBOL buy\|sell QTY PRICE --date YYYY-MM-DD [--fees F]`, `real-import FILE`, `real-export [--out F]` |
| Paper trading | `account`, `orders`, `order SYMBOL buy\|sell QTY [--limit P] [--days N]`, `cancel ID` |
| US stocks | `us-key KEY`, `us-search TEXT`, `us-status` |
| Anything else | `call COMMAND ['{"json":"args"}']`, `commands` (lists them) |

Global options: `--db FILE`, `--compact` (one-line JSON), `--refresh`
(fetch the latest data first). Commands that need prices fetch them
automatically when none are stored yet; `markers` and `structure` keep
refreshing until every favourite's history is downloaded.

## For AI agents

- Start with `cse markers --compact` for the state of every favourite, then
  `cse structure SYMBOL --compact` for the reasons and the plan.
- `markers[sym].status` is one of `look_now`, `get_ready`, `uptrend`,
  `broken`, `range`, `downtrend`, `breaking_down`, `thin`, `pending`,
  `no_data`; `label` and `detail` explain it.
- `structure` → `structure` (trend, weekly trend, swing size, last swings,
  liquidity), `now.setups[]` (kind, trigger, `triggered`, `distance_pct`,
  `plan.entry/stop/target/reward_risk/passes`), `track_record[]`.
- `history` → `periods[]` (state, from, to, days, closes), `swings[]`
  (day, kind, label, price, trend after), `alerts[]` with `outcome`
  (target/stop/open, returns after 5/10/20 days net of fees).
- Paper orders never touch real money; `real-add` only records trades.
  The CLI cannot place real orders.
