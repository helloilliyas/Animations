//! Market-structure history kept on the phone: confirmed swings, one
//! structure row per trading day, and every alert ever fired. Days are
//! Sri Lanka dates (`YYYY-MM-DD`); prices are plain numbers here because
//! they are analysis outputs, not money.

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::{Result, Store};

pub(crate) const V4: &str = "
CREATE TABLE swing_log (
    symbol     TEXT    NOT NULL,
    day        TEXT    NOT NULL,
    kind       TEXT    NOT NULL,
    label      TEXT,
    price      REAL    NOT NULL,
    trend      TEXT    NOT NULL,
    backfilled INTEGER NOT NULL,
    PRIMARY KEY (symbol, day, kind)
) WITHOUT ROWID;

CREATE TABLE structure_day (
    symbol     TEXT    NOT NULL,
    day        TEXT    NOT NULL,
    trend      TEXT    NOT NULL,
    weekly     TEXT,
    status     TEXT    NOT NULL,
    close      REAL    NOT NULL,
    note       TEXT,
    backfilled INTEGER NOT NULL,
    PRIMARY KEY (symbol, day)
) WITHOUT ROWID;

CREATE TABLE alert_log (
    id      INTEGER PRIMARY KEY,
    at_ms   INTEGER NOT NULL,
    symbol  TEXT    NOT NULL,
    kind    TEXT    NOT NULL,
    price   REAL    NOT NULL,
    level   REAL    NOT NULL,
    message TEXT    NOT NULL,
    entry   REAL,
    stop    REAL,
    target  REAL
);
CREATE INDEX alert_log_symbol ON alert_log (symbol);
";

/// A confirmed swing high or low.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SwingRow {
    pub symbol: String,
    pub day: String,
    /// `High` or `Low`.
    pub kind: String,
    /// `HH`, `HL`, `LH`, `LL`, or none for the first of its kind.
    pub label: Option<String>,
    pub price: f64,
    /// Trend once this swing was confirmed.
    pub trend: String,
    /// Reconstructed from history rather than seen as it happened.
    pub backfilled: bool,
}

/// A stock's structure at the close of one trading day.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructureDay {
    pub symbol: String,
    pub day: String,
    pub trend: String,
    pub weekly: Option<String>,
    /// Marker status (`look_now`, `uptrend`, `broken`, ...).
    pub status: String,
    pub close: f64,
    pub note: Option<String>,
    pub backfilled: bool,
}

/// A fired alert, kept permanently.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlertRow {
    pub id: u64,
    pub at_ms: i64,
    pub symbol: String,
    pub kind: String,
    pub price: f64,
    pub level: f64,
    pub message: String,
    pub entry: Option<f64>,
    pub stop: Option<f64>,
    pub target: Option<f64>,
}

impl Store {
    /// Day of the latest logged swing for `symbol`.
    pub fn last_swing_day(&self, symbol: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT MAX(day) FROM swing_log WHERE symbol = ?1",
                [symbol],
                |r| r.get(0),
            )
            .optional()?
            .flatten())
    }

    /// Adds swings; ones already logged for the same day and kind are kept.
    pub fn log_swings(&mut self, rows: &[SwingRow]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT OR IGNORE INTO swing_log
                 (symbol, day, kind, label, price, trend, backfilled)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for w in rows {
                stmt.execute(params![
                    w.symbol,
                    w.day,
                    w.kind,
                    w.label,
                    w.price,
                    w.trend,
                    w.backfilled
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Swings for one symbol, or all when `symbol` is `None`; oldest first.
    pub fn swings(&self, symbol: Option<&str>) -> Result<Vec<SwingRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT symbol, day, kind, label, price, trend, backfilled FROM swing_log
             WHERE ?1 IS NULL OR symbol = ?1 ORDER BY symbol, day",
        )?;
        let rows = stmt.query_map([symbol], |r| {
            Ok(SwingRow {
                symbol: r.get(0)?,
                day: r.get(1)?,
                kind: r.get(2)?,
                label: r.get(3)?,
                price: r.get(4)?,
                trend: r.get(5)?,
                backfilled: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Day of the latest structure row for `symbol`.
    pub fn last_structure_day(&self, symbol: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT MAX(day) FROM structure_day WHERE symbol = ?1",
                [symbol],
                |r| r.get(0),
            )
            .optional()?
            .flatten())
    }

    /// Saves daily structure rows, replacing any for the same day.
    pub fn save_structure_days(&mut self, rows: &[StructureDay]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT OR REPLACE INTO structure_day
                 (symbol, day, trend, weekly, status, close, note, backfilled)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for d in rows {
                stmt.execute(params![
                    d.symbol,
                    d.day,
                    d.trend,
                    d.weekly,
                    d.status,
                    d.close,
                    d.note,
                    d.backfilled
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Daily structure for one symbol, or all; oldest first.
    pub fn structure_days(&self, symbol: Option<&str>) -> Result<Vec<StructureDay>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT symbol, day, trend, weekly, status, close, note, backfilled
             FROM structure_day WHERE ?1 IS NULL OR symbol = ?1 ORDER BY symbol, day",
        )?;
        let rows = stmt.query_map([symbol], |r| {
            Ok(StructureDay {
                symbol: r.get(0)?,
                day: r.get(1)?,
                trend: r.get(2)?,
                weekly: r.get(3)?,
                status: r.get(4)?,
                close: r.get(5)?,
                note: r.get(6)?,
                backfilled: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Records alerts; an id already logged is left as it is.
    pub fn log_alerts(&mut self, rows: &[AlertRow]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT OR IGNORE INTO alert_log
                 (id, at_ms, symbol, kind, price, level, message, entry, stop, target)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?;
            for a in rows {
                stmt.execute(params![
                    a.id as i64,
                    a.at_ms,
                    a.symbol,
                    a.kind,
                    a.price,
                    a.level,
                    a.message,
                    a.entry,
                    a.stop,
                    a.target
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Every logged alert for one symbol, or all; newest first.
    pub fn alert_log(&self, symbol: Option<&str>) -> Result<Vec<AlertRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, at_ms, symbol, kind, price, level, message, entry, stop, target
             FROM alert_log WHERE ?1 IS NULL OR symbol = ?1 ORDER BY at_ms DESC, id DESC",
        )?;
        let rows = stmt.query_map([symbol], |r| {
            Ok(AlertRow {
                id: r.get::<_, i64>(0)? as u64,
                at_ms: r.get(1)?,
                symbol: r.get(2)?,
                kind: r.get(3)?,
                price: r.get(4)?,
                level: r.get(5)?,
                message: r.get(6)?,
                entry: r.get(7)?,
                stop: r.get(8)?,
                target: r.get(9)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_tables_round_trip() {
        let mut s = Store::open_in_memory().unwrap();
        let w = |day: &str, kind: &str, price: f64| SwingRow {
            symbol: "WATA.N0000".into(),
            day: day.into(),
            kind: kind.into(),
            label: Some("HL".into()),
            price,
            trend: "Up".into(),
            backfilled: false,
        };
        s.log_swings(&[w("2026-10-14", "Low", 44.4), w("2026-10-08", "High", 48.0)])
            .unwrap();
        // Logged swings never change.
        s.log_swings(&[w("2026-10-14", "Low", 99.0)]).unwrap();
        let all = s.swings(Some("WATA.N0000")).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[1].price, 44.4);
        assert_eq!(
            s.last_swing_day("WATA.N0000").unwrap().as_deref(),
            Some("2026-10-14")
        );
        assert_eq!(s.last_swing_day("JKH.N0000").unwrap(), None);
        assert!(s.swings(Some("JKH.N0000")).unwrap().is_empty());
        assert_eq!(s.swings(None).unwrap().len(), 2);

        let d = |day: &str, status: &str| StructureDay {
            symbol: "WATA.N0000".into(),
            day: day.into(),
            trend: "Up".into(),
            weekly: None,
            status: status.into(),
            close: 46.6,
            note: None,
            backfilled: true,
        };
        s.save_structure_days(&[d("2026-10-21", "uptrend"), d("2026-10-22", "uptrend")])
            .unwrap();
        s.save_structure_days(&[d("2026-10-22", "broken")]).unwrap();
        let days = s.structure_days(Some("WATA.N0000")).unwrap();
        assert_eq!(days.len(), 2);
        assert_eq!(days[1].status, "broken");
        assert_eq!(
            s.last_structure_day("WATA.N0000").unwrap().as_deref(),
            Some("2026-10-22")
        );

        let a = |id: u64, at_ms: i64| AlertRow {
            id,
            at_ms,
            symbol: "WATA.N0000".into(),
            kind: "Higher low entry".into(),
            price: 46.0,
            level: 45.5,
            message: "m".into(),
            entry: Some(45.5),
            stop: Some(43.9),
            target: Some(48.0),
        };
        s.log_alerts(&[a(1, 10), a(2, 20)]).unwrap();
        s.log_alerts(&[a(2, 20)]).unwrap();
        let log = s.alert_log(None).unwrap();
        assert_eq!(log.iter().map(|x| x.id).collect::<Vec<_>>(), vec![2, 1]);
        assert_eq!(log[0].stop, Some(43.9));
        assert!(s.alert_log(Some("JKH.N0000")).unwrap().is_empty());
    }
}
