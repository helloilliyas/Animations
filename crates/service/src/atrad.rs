//! Reading the user's holdings from the ATrad app's Portfolio → Detailed
//! screen (text captured on the phone by the accessibility service) and
//! bringing the Real portfolio in line with them.
//!
//! The screen's text arrives as a flat list in reading order. Each holding
//! starts with its symbol; values follow either their labels ("Holding
//! Quantity", "Avg. Cost") or, in a table, their column order.

use std::collections::{BTreeMap, HashSet};

use cse_engine::{NewTransaction, Side};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::chart::lk_day;
use crate::{parse, AppService, Result};

/// One holding as read from ATrad.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Captured {
    pub symbol: String,
    pub qty: u64,
    pub avg_cost: Option<Decimal>,
    pub last: Option<Decimal>,
    /// Values came with their labels (not guessed from position).
    pub confident: bool,
}

fn number(s: &str) -> Option<Decimal> {
    let t: String = s
        .trim()
        .trim_start_matches("Rs.")
        .trim_start_matches("LKR")
        .chars()
        .filter(|c| !matches!(c, ',' | ' ' | '%'))
        .collect();
    if t.is_empty() || !t.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    t.parse().ok()
}

fn is_whole(s: &str) -> bool {
    !s.contains('.') && number(s).is_some_and(|d| d >= Decimal::ZERO)
}

/// `JKH.N0000`-style CSE symbols: code, a dot, class letter, four digits.
fn is_cse_symbol(t: &str) -> bool {
    let Some((code, class)) = t.split_once('.') else {
        return false;
    };
    (1..=8).contains(&code.len())
        && code.chars().all(|c| c.is_ascii_alphanumeric() || c == '&')
        && code.chars().any(|c| c.is_ascii_alphabetic())
        && class.len() == 5
        && class.as_bytes()[0].is_ascii_alphabetic()
        && class[1..].chars().all(|c| c.is_ascii_digit())
}

/// A CSE symbol at the start of `s`: any full symbol (`CLND.N0000`), or a
/// bare code of a known security (`CLND`).
fn symbol_of(s: &str, known: &HashSet<String>) -> Option<String> {
    let first = s
        .split([' ', '\n'])
        .next()?
        .trim()
        .trim_end_matches([',', ';', ':'])
        .to_ascii_uppercase();
    if first.len() < 2 {
        return None;
    }
    if is_cse_symbol(&first) || known.contains(&first) {
        return Some(first);
    }
    let n = format!("{first}.N0000");
    known.contains(&n).then_some(n)
}

#[derive(Clone, Copy, PartialEq)]
enum Label {
    Qty,
    Avg,
    Last,
}

fn label_of(s: &str) -> Option<Label> {
    let l = s.trim().trim_end_matches(':').to_ascii_lowercase();
    if (l.contains("holding") && (l.contains("qty") || l.contains("quantity")))
        || l == "qty"
        || l == "quantity"
    {
        Some(Label::Qty)
    } else if (l.contains("avg") || l.contains("average"))
        && (l.contains("cost") || l.contains("price"))
    {
        Some(Label::Avg)
    } else if l.contains("last traded") || l == "ltp" {
        Some(Label::Last)
    } else {
        None
    }
}

/// A whole row read as one text, e.g. `CLND.N0000, 18,085, 5.86%, 44.10, -`
/// (cells joined by ", "; thousands separators have no space). Gives the
/// quantity (first whole number that is not a percentage); prices in such
/// rows are not labelled, so no average cost.
fn row_text(t: &str, known: &HashSet<String>) -> Option<Captured> {
    let cells: Vec<&str> = t.split(", ").map(str::trim).collect();
    if cells.len() < 2 {
        return None;
    }
    let symbol = symbol_of(cells[0], known)?;
    let qty = cells[1..]
        .iter()
        .find(|c| !c.contains('%') && is_whole(c))
        .and_then(|c| number(c))
        .and_then(|q| u64::try_from(q.trunc()).ok())?;
    Some(Captured {
        symbol,
        qty,
        avg_cost: None,
        last: None,
        confident: true,
    })
}

/// Holdings in the captured text; each symbol once (scrolled pages repeat).
/// Labelled values win, then whole-row texts, then values by position.
pub fn parse_capture(texts: &[String], known: &HashSet<String>) -> Vec<Captured> {
    let mut out = parse_segments(texts, known);
    for row in texts.iter().filter_map(|t| row_text(t, known)) {
        match out.iter_mut().find(|c| c.symbol == row.symbol) {
            // Quantity from the row, cost (if any) from the details.
            Some(c) if !c.confident => {
                c.qty = row.qty;
                c.confident = true;
            }
            Some(_) => {}
            None => out.push(row),
        }
    }
    out
}

fn parse_segments(texts: &[String], known: &HashSet<String>) -> Vec<Captured> {
    let starts: Vec<(usize, String)> = texts
        .iter()
        .enumerate()
        .filter(|(_, t)| !t.contains(", "))
        .filter_map(|(i, t)| symbol_of(t, known).map(|s| (i, s)))
        .collect();
    let mut out: Vec<Captured> = Vec::new();
    for (k, (i, sym)) in starts.iter().enumerate() {
        let end = starts.get(k + 1).map_or(texts.len(), |(j, _)| *j);
        let seg = &texts[i + 1..end];
        let (mut qty, mut avg, mut last) = (None, None, None);
        let mut j = 0;
        while j < seg.len() {
            let t = &seg[j];
            // "Label: value" in one text, or the label then its value.
            let (label, inline) = match t.split_once(':') {
                Some((l, v)) if label_of(l).is_some() => (label_of(l), number(v)),
                _ => (label_of(t), None),
            };
            if let Some(label) = label {
                let value = inline.or_else(|| seg.get(j + 1).and_then(|v| number(v)));
                match label {
                    Label::Qty => qty = qty.or(value),
                    Label::Avg => avg = avg.or(value),
                    Label::Last => last = last.or(value),
                }
            }
            j += 1;
        }
        let confident = qty.is_some();
        if qty.is_none() {
            // Table rows: the quantity is the first whole number. Unlabelled
            // prices are left alone: in ATrad's list that is the last price,
            // not the average cost.
            qty = seg
                .iter()
                .find(|t| !t.contains('%') && is_whole(t))
                .and_then(|t| number(t));
        }
        let Some(q) = qty.and_then(|q| u64::try_from(q.trunc()).ok()) else {
            continue;
        };
        let avg = avg.filter(|a| *a > Decimal::ZERO);
        // The same holding again (an opened row, or a scrolled page): fill
        // in what the earlier reading lacked.
        match out.iter_mut().find(|c| &c.symbol == sym) {
            Some(c) => {
                c.avg_cost = c.avg_cost.or(avg);
                c.last = c.last.or(last);
                if confident && !c.confident {
                    c.qty = q;
                    c.confident = true;
                }
            }
            None => out.push(Captured {
                symbol: sym.clone(),
                qty: q,
                avg_cost: avg,
                last,
                confident,
            }),
        }
    }
    out
}

#[derive(Deserialize)]
struct ParseArgs {
    texts: Vec<String>,
}

#[derive(Deserialize)]
struct ApplyRow {
    symbol: String,
    qty: u64,
    avg_cost: Option<Decimal>,
}

#[derive(Deserialize)]
struct ApplyArgs {
    rows: Vec<ApplyRow>,
    /// Holdings to close because ATrad no longer shows them.
    #[serde(default)]
    remove: Vec<String>,
}

/// Average costs within this fraction count as the same.
const COST_TOLERANCE: Decimal = Decimal::from_parts(1, 0, 0, false, 2);

impl AppService {
    pub(crate) fn atrad_command(&self, cmd: &str, args: Value) -> Option<Result<Value>> {
        let r = match cmd {
            "atrad_parse" => parse::<ParseArgs>(args).and_then(|a| self.atrad_parse(&a.texts)),
            "atrad_apply" => parse::<ApplyArgs>(args).and_then(|a| self.atrad_apply(a)),
            _ => return None,
        };
        Some(r)
    }

    /// Holdings read from ATrad, compared with the Real portfolio.
    fn atrad_parse(&self, texts: &[String]) -> Result<Value> {
        let s = self.state.read().unwrap();
        let known: HashSet<String> = s.market.quotes.keys().cloned().collect();
        let captured = parse_capture(texts, &known);
        let ours = s.real.portfolio(&s.account.settings.fees)?;
        let mut seen = HashSet::new();
        let rows: Vec<Value> = captured
            .iter()
            .map(|c| {
                seen.insert(c.symbol.clone());
                let h = ours.holding(&c.symbol);
                let (oq, oa) = h.map_or((0, None), |h| (h.qty, Some(h.avg_cost())));
                let change = match (oq, oa, c.avg_cost) {
                    (0, _, _) => "new",
                    (q, _, _) if c.qty > q => "more",
                    (q, _, _) if c.qty < q => "less",
                    (_, Some(a), Some(b))
                        if b > Decimal::ZERO && ((a - b) / b).abs() > COST_TOLERANCE =>
                    {
                        "cost"
                    }
                    _ => "same",
                };
                json!({
                    "symbol": c.symbol,
                    "name": s.market.quotes.get(&c.symbol).map(|q| q.name.clone()),
                    "qty": c.qty,
                    "avg_cost": c.avg_cost,
                    "last": c.last,
                    "confident": c.confident,
                    "ours_qty": oq,
                    "ours_avg": oa.map(|a| a.round_dp(2)),
                    "change": change,
                })
            })
            .collect();
        let missing: Vec<Value> = ours
            .holdings()
            .filter(|h| h.qty > 0 && !seen.contains(&h.symbol))
            .map(|h| json!({"symbol": h.symbol, "qty": h.qty}))
            .collect();
        Ok(json!({ "rows": rows, "missing": missing, "texts_read": texts.len() }))
    }

    /// Adds adjustment trades (dated today, no fees) so each holding's
    /// quantity and average cost match ATrad. Selling at the current
    /// average cost keeps realised profit unchanged.
    fn atrad_apply(&self, a: ApplyArgs) -> Result<Value> {
        let today = lk_day(self.now());
        let mut changed = 0;
        let last_prices: BTreeMap<String, Decimal> = {
            let s = self.state.read().unwrap();
            a.rows
                .iter()
                .filter_map(|r| {
                    s.market
                        .quotes
                        .get(&r.symbol)
                        .map(|q| (r.symbol.clone(), q.last))
                })
                .collect()
        };
        self.mutate_real(|real, fees| {
            let mut targets: BTreeMap<String, (u64, Option<Decimal>)> = a
                .rows
                .iter()
                .map(|r| (r.symbol.clone(), (r.qty, r.avg_cost)))
                .collect();
            for sym in &a.remove {
                targets.insert(sym.clone(), (0, None));
            }
            for (sym, (q1, a1)) in targets {
                let p = real.portfolio(fees)?;
                let (q0, c0) = p
                    .holding(&sym)
                    .map_or((0, Decimal::ZERO), |h| (h.qty, h.total_cost));
                let avg0 = if q0 > 0 {
                    c0 / Decimal::from(q0)
                } else {
                    Decimal::ZERO
                };
                // No cost on ATrad's screen: keep ours, or use the last price
                // for a holding new to this app.
                let a1 = a1
                    .or((avg0 > Decimal::ZERO).then_some(avg0))
                    .or_else(|| last_prices.get(&sym).copied())
                    .unwrap_or(avg0);
                let mut tx = |side, qty: u64, price: Decimal| {
                    if qty == 0 || price <= Decimal::ZERO {
                        return Ok::<(), crate::ServiceError>(());
                    }
                    real.add(
                        NewTransaction {
                            date: today,
                            symbol: sym.clone(),
                            side,
                            qty,
                            price: price.round_dp(4),
                            fees: Some(Decimal::ZERO),
                        },
                        fees,
                    )?;
                    changed += 1;
                    Ok(())
                };
                let cost_differs = a1 > Decimal::ZERO
                    && avg0 > Decimal::ZERO
                    && ((avg0 - a1) / a1).abs() > COST_TOLERANCE;
                if q1 > q0 {
                    if cost_differs || q0 == 0 {
                        // Rebuild at ATrad's average cost.
                        tx(Side::Sell, q0, avg0)?;
                        tx(Side::Buy, q1, a1)?;
                    } else {
                        // Top up so the average stays as it is.
                        tx(Side::Buy, q1 - q0, avg0)?;
                    }
                } else if q1 < q0 {
                    tx(Side::Sell, q0 - q1, avg0)?;
                    if q1 > 0 && cost_differs {
                        tx(Side::Sell, q1, avg0)?;
                        tx(Side::Buy, q1, a1)?;
                    }
                } else if q1 > 0 && cost_differs {
                    tx(Side::Sell, q1, avg0)?;
                    tx(Side::Buy, q1, a1)?;
                }
            }
            Ok(())
        })?;
        Ok(json!({ "changed": changed }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn known() -> HashSet<String> {
        ["JKH.N0000", "COMB.N0000", "WATA.N0000"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    fn t(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn labelled_cards() {
        let texts = t(&[
            "Portfolio",
            "Summary",
            "Detailed",
            "Valuation",
            "JKH.N0000",
            "Holding Quantity",
            "2,000",
            "Avg. Cost",
            "18.4567",
            "Last Traded Price",
            "19.20",
            "Unrealized Gain/Loss",
            "1,450.00",
            "WATA",
            "Holding Qty: 1,500",
            "Avg Cost: 44.10",
            "B.E.S Price",
            "44.60",
        ]);
        let c = parse_capture(&texts, &known());
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].symbol, "JKH.N0000");
        assert_eq!(c[0].qty, 2000);
        assert_eq!(c[0].avg_cost, Some(dec!(18.4567)));
        assert_eq!(c[0].last, Some(dec!(19.20)));
        assert!(c[0].confident);
        assert_eq!(c[1].symbol, "WATA.N0000");
        assert_eq!((c[1].qty, c[1].avg_cost), (1500, Some(dec!(44.10))));
    }

    #[test]
    fn whole_row_texts_from_atrad() {
        // As read on the phone (Portfolio list: Symbol, Quantity, Last
        // Price, Holdings %, Change); CLND is not in `known`.
        let texts = t(&[
            "Symbol",
            "Quantity",
            "Last Price",
            "Holdings %",
            "Change",
            "CLND.N0000, 18,085, 5.86%, 44.10, -",
            "CLND.N0000",
            "18,085",
            "5.86%",
            "44.10",
            "-",
            "JKH.N0000, 2,000, 3.10%, 19.20, 0.10",
            "JKH.N0000",
            "2,000",
            "3.10%",
            "19.20",
            "0.10",
        ]);
        let c = parse_capture(&texts, &known());
        assert_eq!(c.len(), 2, "{c:?}");
        assert_eq!((c[0].symbol.as_str(), c[0].qty), ("CLND.N0000", 18085));
        assert_eq!(c[0].avg_cost, None, "last price is not the average cost");
        assert!(c[0].confident);
        assert_eq!((c[1].symbol.as_str(), c[1].qty), ("JKH.N0000", 2000));
    }

    #[test]
    fn opened_rows_give_the_average_cost() {
        // ATrad's Detailed list; each row opened in turn shows its details.
        let texts = t(&[
            "Symbol",
            "Quantity",
            "Last Price",
            "Holdings %",
            "Change",
            "CLND.N0000, 18,085, 5.86%, 44.10, -",
            "CLND.N0000",
            "18,085",
            "5.86%",
            "44.10",
            "-",
            "Avg. Cost",
            "42.3512",
            "B.E.S Price",
            "42.85",
            "Total Cost",
            "765,925.48",
            "RHL.N0000, 122,206, 39.57%, 34.40, -",
            "RHL.N0000",
            "122,206",
            "39.57%",
            "34.40",
            "-",
            // Next page: CLND closed again, RHL opened.
            "CLND.N0000, 18,085, 5.86%, 44.10, -",
            "CLND.N0000",
            "18,085",
            "5.86%",
            "44.10",
            "-",
            "RHL.N0000, 122,206, 39.57%, 34.40, -",
            "RHL.N0000",
            "122,206",
            "39.57%",
            "34.40",
            "-",
            "Avg. Cost",
            "31.2045",
            "B.E.S Price",
            "31.55",
        ]);
        let c = parse_capture(&texts, &known());
        assert_eq!(c.len(), 2, "{c:?}");
        assert_eq!((c[0].symbol.as_str(), c[0].qty), ("CLND.N0000", 18085));
        assert_eq!(c[0].avg_cost, Some(dec!(42.3512)));
        assert_eq!((c[1].symbol.as_str(), c[1].qty), ("RHL.N0000", 122206));
        assert_eq!(c[1].avg_cost, Some(dec!(31.2045)));
        assert!(c.iter().all(|x| x.confident));
    }

    #[test]
    fn table_rows_and_repeated_pages() {
        // Column headers once, then symbol, qty, pending buy, pending sell,
        // avg cost, ...; the second page repeats COMB after scrolling.
        let texts = t(&[
            "Symbol",
            "Holding",
            "P.Buy",
            "P.Sell",
            "Avg",
            "LTP",
            "COMB.N0000",
            "300",
            "0",
            "0",
            "98.25",
            "101.00",
            "JKH.N0000",
            "2000",
            "0",
            "0",
            "18.46",
            "19.20",
            "COMB.N0000",
            "300",
            "0",
            "0",
            "98.25",
            "101.00",
        ]);
        let c = parse_capture(&texts, &known());
        assert_eq!(c.len(), 2);
        assert_eq!((c[0].symbol.as_str(), c[0].qty), ("COMB.N0000", 300));
        // Unlabelled prices are not taken as the average cost.
        assert_eq!(c[0].avg_cost, None);
        assert!(!c[0].confident);
        // Unknown words are not symbols.
        assert!(parse_capture(&t(&["TOTAL", "5000"]), &known()).is_empty());
    }
}
