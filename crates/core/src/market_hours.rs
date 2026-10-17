//! CSE trading calendar in Sri Lanka time (UTC+05:30, no daylight saving).

use std::collections::BTreeSet;

use chrono::{
    DateTime, Datelike, Duration, FixedOffset, NaiveDate, NaiveTime, TimeZone, Utc, Weekday,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarketPhase {
    /// Order entry before the open (09:00–09:30).
    PreOpen,
    /// Continuous trading (09:30–14:30).
    Open,
    Closed,
}

/// Trading days and sessions. Holidays are supplied by the user or app data
/// because the CSE publishes them yearly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarketCalendar {
    pub pre_open: NaiveTime,
    pub open: NaiveTime,
    pub close: NaiveTime,
    pub holidays: BTreeSet<NaiveDate>,
}

impl Default for MarketCalendar {
    fn default() -> Self {
        Self {
            pre_open: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            open: NaiveTime::from_hms_opt(9, 30, 0).unwrap(),
            close: NaiveTime::from_hms_opt(14, 30, 0).unwrap(),
            holidays: BTreeSet::new(),
        }
    }
}

/// Asia/Colombo offset.
pub fn colombo() -> FixedOffset {
    FixedOffset::east_opt(5 * 3600 + 30 * 60).unwrap()
}

impl MarketCalendar {
    pub fn is_trading_day(&self, date: NaiveDate) -> bool {
        !matches!(date.weekday(), Weekday::Sat | Weekday::Sun) && !self.holidays.contains(&date)
    }

    pub fn phase_at(&self, at: DateTime<Utc>) -> MarketPhase {
        let local = at.with_timezone(&colombo());
        if !self.is_trading_day(local.date_naive()) {
            return MarketPhase::Closed;
        }
        let t = local.time();
        if t >= self.open && t < self.close {
            MarketPhase::Open
        } else if t >= self.pre_open && t < self.open {
            MarketPhase::PreOpen
        } else {
            MarketPhase::Closed
        }
    }

    /// The next moment continuous trading starts, strictly after `at` unless
    /// the market is already open (then the current session's open).
    pub fn next_open(&self, at: DateTime<Utc>) -> DateTime<Utc> {
        let local = at.with_timezone(&colombo());
        let mut date = local.date_naive();
        if self.phase_at(at) != MarketPhase::Open && local.time() >= self.open {
            date += Duration::days(1);
        }
        while !self.is_trading_day(date) {
            date += Duration::days(1);
        }
        colombo()
            .from_local_datetime(&date.and_time(self.open))
            .unwrap()
            .with_timezone(&Utc)
    }

    /// Today's close if trading today, otherwise `None`.
    pub fn close_on(&self, date: NaiveDate) -> Option<DateTime<Utc>> {
        self.is_trading_day(date).then(|| {
            colombo()
                .from_local_datetime(&date.and_time(self.close))
                .unwrap()
                .with_timezone(&Utc)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lk(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        colombo()
            .with_ymd_and_hms(y, m, d, h, min, 0)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn phases_on_a_weekday() {
        let cal = MarketCalendar::default();
        // 2026-10-15 is a Thursday.
        assert_eq!(cal.phase_at(lk(2026, 10, 15, 8, 59)), MarketPhase::Closed);
        assert_eq!(cal.phase_at(lk(2026, 10, 15, 9, 0)), MarketPhase::PreOpen);
        assert_eq!(cal.phase_at(lk(2026, 10, 15, 9, 30)), MarketPhase::Open);
        assert_eq!(cal.phase_at(lk(2026, 10, 15, 14, 29)), MarketPhase::Open);
        assert_eq!(cal.phase_at(lk(2026, 10, 15, 14, 30)), MarketPhase::Closed);
    }

    #[test]
    fn weekends_and_holidays_are_closed() {
        let mut cal = MarketCalendar::default();
        assert_eq!(cal.phase_at(lk(2026, 10, 17, 10, 0)), MarketPhase::Closed); // Saturday
        cal.holidays
            .insert(NaiveDate::from_ymd_opt(2026, 10, 15).unwrap());
        assert_eq!(cal.phase_at(lk(2026, 10, 15, 10, 0)), MarketPhase::Closed);
    }

    #[test]
    fn next_open_skips_weekend_and_holiday() {
        let mut cal = MarketCalendar::default();
        // Friday after close -> Monday 09:30.
        assert_eq!(
            cal.next_open(lk(2026, 10, 16, 15, 0)),
            lk(2026, 10, 19, 9, 30)
        );
        // Monday is a holiday -> Tuesday.
        cal.holidays
            .insert(NaiveDate::from_ymd_opt(2026, 10, 19).unwrap());
        assert_eq!(
            cal.next_open(lk(2026, 10, 16, 15, 0)),
            lk(2026, 10, 20, 9, 30)
        );
        // Before the open on a trading day -> same day.
        assert_eq!(
            cal.next_open(lk(2026, 10, 15, 8, 0)),
            lk(2026, 10, 15, 9, 30)
        );
        // During the session -> this session's open.
        assert_eq!(
            cal.next_open(lk(2026, 10, 15, 11, 0)),
            lk(2026, 10, 15, 9, 30)
        );
    }

    #[test]
    fn utc_conversion_uses_plus_0530() {
        let cal = MarketCalendar::default();
        let close = cal
            .close_on(NaiveDate::from_ymd_opt(2026, 10, 15).unwrap())
            .unwrap();
        assert_eq!(close.to_rfc3339(), "2026-10-15T09:00:00+00:00");
    }
}
