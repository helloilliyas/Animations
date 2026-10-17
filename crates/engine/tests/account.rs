use std::collections::HashMap;

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use cse_core::market_hours::colombo;
use cse_core::Quote;
use cse_engine::{Account, OrderError, OrderRequest, OrderStatus, OrderType, Settings, Side, Tif};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

/// Sri Lanka local time. 2026-10-15 is a Thursday.
fn lk(d: u32, h: u32, m: u32) -> DateTime<Utc> {
    colombo()
        .with_ymd_and_hms(2026, 10, d, h, m, 0)
        .unwrap()
        .with_timezone(&Utc)
}

fn account() -> Account {
    Account::new(dec!(100000), Settings::default(), lk(15, 9, 0))
}

fn quotes(rows: &[(&str, Decimal)]) -> HashMap<String, Quote> {
    rows.iter()
        .map(|(s, last)| {
            (
                s.to_string(),
                Quote {
                    symbol: s.to_string(),
                    name: String::new(),
                    last: *last,
                    change: dec!(0),
                    change_pct: dec!(0),
                    prev_close: *last,
                    open: None,
                    high: None,
                    low: None,
                    share_volume: 0,
                    trades: 0,
                    turnover: dec!(0),
                    last_traded_at: None,
                },
            )
        })
        .collect()
}

fn limit(symbol: &str, side: Side, qty: u64, price: Decimal) -> OrderRequest {
    OrderRequest {
        symbol: symbol.into(),
        side,
        order_type: OrderType::Limit(price),
        qty,
        tif: Tif::Day,
    }
}

fn market(symbol: &str, side: Side, qty: u64) -> OrderRequest {
    OrderRequest {
        symbol: symbol.into(),
        side,
        order_type: OrderType::Market,
        qty,
        tif: Tif::Day,
    }
}

#[test]
fn limit_buy_waits_for_price_then_fills_at_last() {
    let mut a = account();
    let id = a
        .place(
            limit("JKH", Side::Buy, 1000, dec!(18.00)),
            None,
            lk(15, 10, 0),
        )
        .unwrap();
    assert_eq!(a.order(id).unwrap().status, OrderStatus::Open);
    // 1000 x 18.00 = 18,000 + 1.12% = 18,201.60 reserved
    assert_eq!(a.pending_buy_value(), dec!(18201.60));
    assert_eq!(a.buying_power(), dec!(81798.40));
    assert_eq!(a.cash(), dec!(100000));

    assert!(a
        .on_market(lk(15, 10, 1), &quotes(&[("JKH", dec!(18.10))]))
        .is_empty());

    let fills = a.on_market(lk(15, 10, 2), &quotes(&[("JKH", dec!(17.90))]));
    assert_eq!(fills.len(), 1);
    let f = &fills[0];
    assert_eq!(f.price, dec!(17.90));
    assert_eq!(f.gross, dec!(17900.00));
    assert_eq!(f.fees, dec!(200.48));
    assert_eq!(f.net, dec!(18100.48));

    assert_eq!(a.order(id).unwrap().status, OrderStatus::Filled);
    assert_eq!(a.cash(), dec!(81899.52));
    assert_eq!(a.buying_power(), a.cash());
    assert_eq!(a.portfolio().qty("JKH"), 1000);
    assert_eq!(
        a.portfolio().holding("JKH").unwrap().total_cost,
        dec!(18100.48)
    );
}

#[test]
fn limit_sell_fills_when_price_reaches_limit_and_realises_gain() {
    let mut a = account();
    a.place(
        market("JKH", Side::Buy, 1000),
        Some(dec!(20)),
        lk(15, 10, 0),
    )
    .unwrap();
    a.on_market(lk(15, 10, 0), &quotes(&[("JKH", dec!(20))]));
    let cost = a.portfolio().holding("JKH").unwrap().total_cost; // 20,224.00

    let id = a
        .place(
            limit("JKH", Side::Sell, 1000, dec!(22.00)),
            None,
            lk(15, 11, 0),
        )
        .unwrap();
    assert_eq!(a.available_to_sell("JKH"), 0);
    assert!(a
        .on_market(lk(15, 11, 1), &quotes(&[("JKH", dec!(21.90))]))
        .is_empty());
    let fills = a.on_market(lk(15, 11, 2), &quotes(&[("JKH", dec!(22.10))]));

    let f = &fills[0];
    assert_eq!(f.order_id, id);
    assert_eq!(f.net, dec!(21852.48)); // 22,100 - 247.52
    assert_eq!(f.realized, Some(dec!(21852.48) - cost));
    assert_eq!(a.portfolio().qty("JKH"), 0);
    assert_eq!(a.cash(), dec!(100000) - cost + dec!(21852.48));
    assert_eq!(a.ledger().len(), 3);
    assert_eq!(a.ledger().last().unwrap().balance, a.cash());
}

#[test]
fn market_buy_needs_a_reference_price() {
    let mut a = account();
    assert_eq!(
        a.place(market("JKH", Side::Buy, 10), None, lk(15, 10, 0)),
        Err(OrderError::NoReferencePrice)
    );
}

#[test]
fn validation_rejects_bad_orders() {
    let mut a = account();
    let t = lk(15, 10, 0);
    assert_eq!(
        a.place(limit("JKH", Side::Buy, 0, dec!(18)), None, t),
        Err(OrderError::InvalidQuantity)
    );
    assert_eq!(
        a.place(limit("JKH", Side::Buy, 10, dec!(18.05)), None, t),
        Err(OrderError::InvalidPrice { tick: dec!(0.10) })
    );
    assert_eq!(
        a.place(limit("ABC", Side::Buy, 10, dec!(150.10)), None, t),
        Err(OrderError::InvalidPrice { tick: dec!(0.25) })
    );
    assert_eq!(
        a.place(limit("JKH", Side::Sell, 10, dec!(18)), None, t),
        Err(OrderError::InsufficientShares {
            symbol: "JKH".into(),
            available: 0
        })
    );
    let mut req = limit("JKH", Side::Buy, 10, dec!(18));
    req.tif = Tif::Days(0);
    assert_eq!(a.place(req, None, t), Err(OrderError::InvalidTif));
}

#[test]
fn pending_buys_reduce_buying_power() {
    let mut a = account();
    let t = lk(15, 10, 0);
    // 5,000 x 10 = 50,000 + 560 = 50,560 reserved
    a.place(limit("A", Side::Buy, 5000, dec!(10)), None, t)
        .unwrap();
    // Another 50,560 would exceed the remaining 49,440.
    assert_eq!(
        a.place(limit("B", Side::Buy, 5000, dec!(10)), None, t),
        Err(OrderError::InsufficientBuyingPower {
            needed: dec!(50560.00),
            available: dec!(49440.00)
        })
    );
}

#[test]
fn cancel_releases_reservation() {
    let mut a = account();
    let id = a
        .place(limit("A", Side::Buy, 5000, dec!(10)), None, lk(15, 10, 0))
        .unwrap();
    a.cancel(id, lk(15, 10, 5)).unwrap();
    assert_eq!(a.order(id).unwrap().status, OrderStatus::Cancelled);
    assert_eq!(a.buying_power(), dec!(100000));
    assert_eq!(a.cancel(id, lk(15, 10, 6)), Err(OrderError::NotActive(id)));
    assert_eq!(a.cancel(99, lk(15, 10, 6)), Err(OrderError::NotFound(99)));
}

#[test]
fn amend_rechecks_against_own_reservation() {
    let mut a = account();
    let t = lk(15, 10, 0);
    let id = a
        .place(limit("A", Side::Buy, 5000, dec!(10)), None, t)
        .unwrap();
    // Growing to 9,000 needs 91,008 of the 100,000 once our own 50,560 is released.
    a.amend(id, 9000, Some(dec!(10)), t).unwrap();
    assert_eq!(a.order(id).unwrap().qty, 9000);
    assert!(matches!(
        a.amend(id, 10000, None, t),
        Err(OrderError::InsufficientBuyingPower { .. })
    ));
    a.amend(id, 9000, Some(dec!(9.50)), t).unwrap();
    assert_eq!(a.order(id).unwrap().limit_price(), Some(dec!(9.50)));
    assert!(a
        .order(id)
        .unwrap()
        .history
        .last()
        .unwrap()
        .note
        .contains("9000 @ 9.50"));
    assert!(matches!(
        a.amend(id, 9000, Some(dec!(9.55)), t),
        Err(OrderError::InvalidPrice { .. })
    ));
}

#[test]
fn market_order_price_cannot_be_amended() {
    let mut a = account();
    let t = lk(15, 10, 0);
    let id = a
        .place(market("A", Side::Buy, 10), Some(dec!(10)), t)
        .unwrap();
    assert_eq!(
        a.amend(id, 10, Some(dec!(11)), t),
        Err(OrderError::PriceOnMarketOrder)
    );
}

#[test]
fn orders_placed_while_closed_queue_until_the_open() {
    let mut a = account();
    // Friday 16th after the close.
    let id = a
        .place(limit("A", Side::Buy, 100, dec!(10)), None, lk(16, 16, 0))
        .unwrap();
    assert_eq!(a.order(id).unwrap().status, OrderStatus::Queued);
    // Day order placed after Friday's close lasts for Monday's session.
    assert_eq!(a.order(id).unwrap().expires_at, lk(19, 14, 30));

    // Weekend quotes do nothing.
    assert!(a
        .on_market(lk(17, 10, 0), &quotes(&[("A", dec!(9))]))
        .is_empty());
    assert_eq!(a.order(id).unwrap().status, OrderStatus::Queued);

    let fills = a.on_market(lk(19, 9, 30), &quotes(&[("A", dec!(9.90))]));
    assert_eq!(fills.len(), 1);
    let history: Vec<_> = a
        .order(id)
        .unwrap()
        .history
        .iter()
        .map(|e| e.status.clone())
        .collect();
    assert_eq!(
        history,
        [OrderStatus::Queued, OrderStatus::Open, OrderStatus::Filled]
    );
}

#[test]
fn day_orders_expire_at_the_close() {
    let mut a = account();
    let id = a
        .place(limit("A", Side::Buy, 100, dec!(10)), None, lk(15, 10, 0))
        .unwrap();
    a.on_market(lk(15, 14, 29), &quotes(&[("A", dec!(11))]));
    assert_eq!(a.order(id).unwrap().status, OrderStatus::Open);
    a.on_market(lk(15, 14, 30), &quotes(&[("A", dec!(9))]));
    assert_eq!(a.order(id).unwrap().status, OrderStatus::Expired);
    assert_eq!(a.buying_power(), dec!(100000));
}

#[test]
fn multi_day_tif_skips_weekends_and_holidays() {
    let mut settings = Settings::default();
    settings
        .calendar
        .holidays
        .insert(NaiveDate::from_ymd_opt(2026, 10, 19).unwrap());
    let mut a = Account::new(dec!(100000), settings, lk(15, 9, 0));
    let mut req = limit("A", Side::Buy, 100, dec!(10));
    req.tif = Tif::Days(3);
    // Thu 15, Fri 16, (weekend, Mon 19 holiday), Tue 20.
    let id = a.place(req, None, lk(15, 10, 0)).unwrap();
    assert_eq!(a.order(id).unwrap().expires_at, lk(20, 14, 30));
}

#[test]
fn market_buy_is_rejected_if_cash_runs_short_at_fill() {
    let mut a = Account::new(dec!(1100), Settings::default(), lk(15, 9, 0));
    let id = a
        .place(market("A", Side::Buy, 100), Some(dec!(10)), lk(15, 10, 0))
        .unwrap();
    // Price jumped: 100 x 12 + fees > 1,100.
    let fills = a.on_market(lk(15, 10, 1), &quotes(&[("A", dec!(12))]));
    assert!(fills.is_empty());
    assert!(matches!(
        a.order(id).unwrap().status,
        OrderStatus::Rejected(_)
    ));
    assert_eq!(a.cash(), dec!(1100));
}

#[test]
fn summary_reports_equity_and_return() {
    let mut a = account();
    a.place(market("A", Side::Buy, 1000), Some(dec!(50)), lk(15, 10, 0))
        .unwrap();
    a.on_market(lk(15, 10, 0), &quotes(&[("A", dec!(50))]));
    // Paid 50,560. Price rises to 55.
    let s = a.summary(&quotes(&[("A", dec!(55))]));
    assert_eq!(s.cash, dec!(49440.00));
    assert_eq!(s.portfolio.market_value, dec!(55000.00));
    assert_eq!(s.equity, dec!(104440.00));
    assert_eq!(s.total_return, dec!(4440.00));
    assert_eq!(s.total_return_pct, dec!(4.44));
}

#[test]
fn account_round_trips_through_json_and_resets() {
    let mut a = account();
    a.place(limit("A", Side::Buy, 100, dec!(10)), None, lk(15, 10, 0))
        .unwrap();
    let json = serde_json::to_string(&a).unwrap();
    let restored: Account = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, a);

    a.reset(dec!(50000), lk(15, 11, 0));
    assert_eq!(a.cash(), dec!(50000));
    assert!(a.orders().is_empty());
    assert_eq!(a.ledger().len(), 1);
}

#[test]
fn account_persists_in_sqlite() {
    let mut store = cse_storage::Store::open_in_memory().unwrap();
    let mut a = account();
    a.place(market("A", Side::Buy, 100), Some(dec!(10)), lk(15, 10, 0))
        .unwrap();
    a.on_market(lk(15, 10, 0), &quotes(&[("A", dec!(10))]));
    store.save_json("paper_account", &a).unwrap();

    let loaded: Account = store.load_json("paper_account").unwrap().unwrap();
    assert_eq!(loaded, a);
    assert_eq!(loaded.portfolio().qty("A"), 100);
}
