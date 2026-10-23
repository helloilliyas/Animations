//! US stock market hours (New York time, with daylight saving). Exchange
//! holidays are not listed; the price feed's own "market open" flag covers
//! them.

use chrono::{
    DateTime, Datelike, Duration, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc,
    Weekday,
};

use crate::MarketPhase;

/// Symbols of US stocks end with this suffix, e.g. `AAPL.US`.
pub const US_SUFFIX: &str = ".US";

pub fn is_us(symbol: &str) -> bool {
    symbol.ends_with(US_SUFFIX)
}

/// `AAPL.US` -> `AAPL`.
pub fn ticker(symbol: &str) -> &str {
    symbol.strip_suffix(US_SUFFIX).unwrap_or(symbol)
}

/// The `n`th `weekday` of a month (1-based).
fn nth_weekday(year: i32, month: u32, weekday: Weekday, n: u32) -> NaiveDate {
    let first = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
    let shift = (7 + weekday.num_days_from_monday() - first.weekday().num_days_from_monday()) % 7;
    first + Duration::days((shift + 7 * (n - 1)) as i64)
}

/// New York's UTC offset at `at`: daylight time (UTC-4) from 2:00 on the
/// second Sunday of March to 2:00 on the first Sunday of November,
/// otherwise UTC-5.
pub fn new_york_offset(at: DateTime<Utc>) -> FixedOffset {
    let y = at.year();
    let start = nth_weekday(y, 3, Weekday::Sun, 2)
        .and_hms_opt(7, 0, 0)
        .unwrap()
        .and_utc();
    let end = nth_weekday(y, 11, Weekday::Sun, 1)
        .and_hms_opt(6, 0, 0)
        .unwrap()
        .and_utc();
    let hours = if at >= start && at < end { -4 } else { -5 };
    FixedOffset::east_opt(hours * 3600).unwrap()
}

/// New York calendar date at `at`.
pub fn new_york_date(at: DateTime<Utc>) -> NaiveDate {
    at.with_timezone(&new_york_offset(at)).date_naive()
}

/// A New York wall-clock time as UTC.
pub fn from_new_york(local: NaiveDateTime) -> DateTime<Utc> {
    // The offset of the moment itself; a guess from the naive time is right
    // except within the hour of a switch, which never falls in a session.
    let guess = local.and_utc() + Duration::hours(5);
    let off = new_york_offset(guess);
    off.from_local_datetime(&local)
        .single()
        .unwrap_or_else(|| guess.with_timezone(&off))
        .with_timezone(&Utc)
}

fn open_time() -> NaiveTime {
    NaiveTime::from_hms_opt(9, 30, 0).unwrap()
}

fn close_time() -> NaiveTime {
    NaiveTime::from_hms_opt(16, 0, 0).unwrap()
}

pub fn is_weekday(date: NaiveDate) -> bool {
    !matches!(date.weekday(), Weekday::Sat | Weekday::Sun)
}

/// Regular session 09:30–16:00 New York time, weekdays.
pub fn us_phase(at: DateTime<Utc>) -> MarketPhase {
    let local = at.with_timezone(&new_york_offset(at));
    let t = local.time();
    if is_weekday(local.date_naive()) && t >= open_time() && t < close_time() {
        MarketPhase::Open
    } else {
        MarketPhase::Closed
    }
}

/// The session's close on a New York date, if it is a weekday.
pub fn us_close_on(date: NaiveDate) -> Option<DateTime<Utc>> {
    is_weekday(date).then(|| from_new_york(date.and_time(close_time())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    #[test]
    fn daylight_saving_switches() {
        // 2026: DST from Sunday 8 March to Sunday 1 November.
        assert_eq!(
            new_york_offset(utc(2026, 3, 8, 6, 59)).local_minus_utc(),
            -5 * 3600
        );
        assert_eq!(
            new_york_offset(utc(2026, 3, 8, 7, 0)).local_minus_utc(),
            -4 * 3600
        );
        assert_eq!(
            new_york_offset(utc(2026, 10, 23, 12, 0)).local_minus_utc(),
            -4 * 3600
        );
        assert_eq!(
            new_york_offset(utc(2026, 11, 1, 5, 59)).local_minus_utc(),
            -4 * 3600
        );
        assert_eq!(
            new_york_offset(utc(2026, 11, 1, 6, 0)).local_minus_utc(),
            -5 * 3600
        );
    }

    #[test]
    fn session_in_new_york_and_sri_lanka_time() {
        // Friday 23 Oct 2026, daylight time: 09:30 NY = 13:30 UTC = 19:00 LK.
        assert_eq!(us_phase(utc(2026, 10, 23, 13, 29)), MarketPhase::Closed);
        assert_eq!(us_phase(utc(2026, 10, 23, 13, 30)), MarketPhase::Open);
        assert_eq!(us_phase(utc(2026, 10, 23, 19, 59)), MarketPhase::Open);
        assert_eq!(us_phase(utc(2026, 10, 23, 20, 0)), MarketPhase::Closed);
        // After the switch, 09:30 NY = 14:30 UTC.
        assert_eq!(us_phase(utc(2026, 11, 2, 14, 29)), MarketPhase::Closed);
        assert_eq!(us_phase(utc(2026, 11, 2, 14, 30)), MarketPhase::Open);
        // Saturday.
        assert_eq!(us_phase(utc(2026, 10, 24, 15, 0)), MarketPhase::Closed);
        let d = NaiveDate::from_ymd_opt(2026, 10, 23).unwrap();
        assert_eq!(us_close_on(d), Some(utc(2026, 10, 23, 20, 0)));
        assert_eq!(new_york_date(utc(2026, 10, 24, 1, 0)), d);
    }

    #[test]
    fn tickers() {
        assert!(is_us("AAPL.US"));
        assert!(!is_us("JKH.N0000"));
        assert_eq!(ticker("AAPL.US"), "AAPL");
    }
}
