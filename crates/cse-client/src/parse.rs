//! Tolerant JSON field extraction.
//!
//! The CSE website's data feeds are undocumented and their field names have
//! varied over time, so each model field is read from a list of candidate keys
//! and numbers are accepted as JSON numbers or numeric strings.

use std::str::FromStr;

use chrono::{DateTime, TimeZone, Utc};
use rust_decimal::Decimal;
use serde_json::Value;

use crate::Error;

/// Value of the first key present and non-null.
pub fn field<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().filter_map(|k| v.get(*k)).find(|x| !x.is_null())
}

pub fn decimal(v: &Value, keys: &[&str]) -> Option<Decimal> {
    field(v, keys).and_then(to_decimal)
}

pub fn req_decimal(v: &Value, keys: &[&str]) -> Result<Decimal, Error> {
    decimal(v, keys).ok_or_else(|| missing(keys))
}

pub fn decimal_or_zero(v: &Value, keys: &[&str]) -> Decimal {
    decimal(v, keys).unwrap_or_default()
}

pub fn count(v: &Value, keys: &[&str]) -> u64 {
    decimal(v, keys)
        .and_then(|d| u64::try_from(d.trunc()).ok())
        .unwrap_or(0)
}

/// A non-negative whole number, or `None` when absent.
pub fn count_opt(v: &Value, keys: &[&str]) -> Option<u64> {
    decimal(v, keys).and_then(|d| u64::try_from(d.trunc()).ok())
}

pub fn string(v: &Value, keys: &[&str]) -> Option<String> {
    match field(v, keys)? {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_owned()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

pub fn req_string(v: &Value, keys: &[&str]) -> Result<String, Error> {
    string(v, keys).ok_or_else(|| missing(keys))
}

/// Accepts epoch milliseconds, epoch seconds, or RFC 3339 strings.
pub fn timestamp(v: &Value, keys: &[&str]) -> Option<DateTime<Utc>> {
    match field(v, keys)? {
        Value::String(s) => DateTime::parse_from_rfc3339(s)
            .map(|d| d.with_timezone(&Utc))
            .ok()
            .or_else(|| i64::from_str(s).ok().and_then(epoch)),
        Value::Number(n) => n.as_i64().and_then(epoch),
        _ => None,
    }
}

/// The list inside a response: the value itself if it is an array, otherwise
/// the first of `keys` holding an array, otherwise the first array field.
pub fn list<'a>(v: &'a Value, keys: &[&str]) -> &'a [Value] {
    if let Value::Array(items) = v {
        return items;
    }
    keys.iter()
        .filter_map(|k| v.get(*k))
        .chain(v.as_object().into_iter().flat_map(|o| o.values()))
        .find_map(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// The object inside a response: the first of `keys` holding an object, or the
/// value itself.
pub fn object<'a>(v: &'a Value, keys: &[&str]) -> &'a Value {
    keys.iter()
        .filter_map(|k| v.get(*k))
        .find(|x| x.is_object())
        .unwrap_or(v)
}

fn to_decimal(v: &Value) -> Option<Decimal> {
    match v {
        Value::Number(n) => Decimal::from_str(&n.to_string())
            .or_else(|_| Decimal::from_scientific(&n.to_string()))
            .ok(),
        Value::String(s) => {
            let cleaned: String = s
                .chars()
                .filter(|c| !matches!(c, ',' | '%' | ' '))
                .collect();
            Decimal::from_str(&cleaned).ok()
        }
        _ => None,
    }
}

fn epoch(n: i64) -> Option<DateTime<Utc>> {
    // Values above ~year 2286 in seconds are treated as milliseconds.
    if n > 10_000_000_000 {
        Utc.timestamp_millis_opt(n).single()
    } else {
        Utc.timestamp_opt(n, 0).single()
    }
}

fn missing(keys: &[&str]) -> Error {
    Error::MissingField(keys.join("|"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;
    use serde_json::json;

    #[test]
    fn first_present_key_wins_and_null_is_skipped() {
        let v = json!({"a": null, "b": 1.5, "c": 2});
        assert_eq!(decimal(&v, &["a", "b", "c"]), Some(dec!(1.5)));
    }

    #[test]
    fn numbers_from_strings() {
        let v = json!({"x": "1,234.50", "p": "-2.5%"});
        assert_eq!(decimal(&v, &["x"]), Some(dec!(1234.50)));
        assert_eq!(decimal(&v, &["p"]), Some(dec!(-2.5)));
    }

    #[test]
    fn counts_truncate_and_default_to_zero() {
        let v = json!({"n": 12.0, "neg": -3});
        assert_eq!(count(&v, &["n"]), 12);
        assert_eq!(count(&v, &["neg"]), 0);
        assert_eq!(count(&v, &["none"]), 0);
    }

    #[test]
    fn required_fields_report_candidates() {
        let err = req_decimal(&json!({}), &["price", "lastPrice"]).unwrap_err();
        assert_eq!(err.to_string(), "missing field (tried: price|lastPrice)");
    }

    #[test]
    fn timestamps() {
        let ms = json!({"t": 1_760_500_000_000_i64});
        let s = json!({"t": 1_760_500_000});
        let iso = json!({"t": "2025-10-15T03:46:40Z"});
        let expected = Utc.timestamp_opt(1_760_500_000, 0).unwrap();
        assert_eq!(timestamp(&ms, &["t"]), Some(expected));
        assert_eq!(timestamp(&s, &["t"]), Some(expected));
        assert_eq!(timestamp(&iso, &["t"]), Some(expected));
    }

    #[test]
    fn list_finds_nested_arrays() {
        let wrapped = json!({"meta": 1, "rows": [1, 2]});
        assert_eq!(list(&wrapped, &["data"]).len(), 2);
        assert_eq!(list(&json!([1, 2, 3]), &[]).len(), 3);
        assert!(list(&json!({"x": 1}), &[]).is_empty());
    }
}
