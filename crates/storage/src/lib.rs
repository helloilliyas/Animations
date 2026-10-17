//! SQLite persistence: price/index snapshots for charts, and watch lists.
//!
//! Decimals are stored as TEXT to keep exact values; timestamps as Unix
//! milliseconds (UTC).

use std::path::Path;
use std::str::FromStr;

use chrono::{DateTime, TimeZone, Utc};
use cse_core::{IndexValue, Quote};
use rusqlite::{params, Connection, OptionalExtension};
use rust_decimal::Decimal;
use serde::de::DeserializeOwned;
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("corrupt decimal in database: {0}")]
    Decimal(String),
    #[error("watch list {0} not found")]
    WatchListNotFound(i64),
    #[error("a watch list named {0:?} already exists")]
    DuplicateWatchList(String),
    #[error("stored {key:?} could not be read: {source}")]
    Json {
        key: String,
        source: serde_json::Error,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

/// One stored price point.
#[derive(Debug, Clone, PartialEq)]
pub struct PricePoint {
    pub at: DateTime<Utc>,
    pub price: Decimal,
    pub share_volume: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WatchList {
    pub id: i64,
    pub name: String,
    pub symbols: Vec<String>,
}

pub struct Store {
    conn: Connection,
}

/// Schema migrations; entry N upgrades `user_version` N to N + 1.
const MIGRATIONS: &[&str] = &[V1, V2];

const V1: &str = "
CREATE TABLE quote_snapshot (
    symbol       TEXT    NOT NULL,
    at_ms        INTEGER NOT NULL,
    last         TEXT    NOT NULL,
    change       TEXT    NOT NULL,
    change_pct   TEXT    NOT NULL,
    share_volume INTEGER NOT NULL,
    trades       INTEGER NOT NULL,
    turnover     TEXT    NOT NULL,
    PRIMARY KEY (symbol, at_ms)
) WITHOUT ROWID;

CREATE TABLE index_snapshot (
    name       TEXT    NOT NULL,
    at_ms      INTEGER NOT NULL,
    value      TEXT    NOT NULL,
    change     TEXT    NOT NULL,
    change_pct TEXT    NOT NULL,
    PRIMARY KEY (name, at_ms)
) WITHOUT ROWID;

CREATE TABLE watch_list (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);

CREATE TABLE watch_list_item (
    watch_list_id INTEGER NOT NULL REFERENCES watch_list(id) ON DELETE CASCADE,
    symbol        TEXT    NOT NULL,
    position      INTEGER NOT NULL,
    PRIMARY KEY (watch_list_id, symbol)
);
";

/// Whole objects stored as JSON, such as the paper account.
const V2: &str = "
CREATE TABLE app_state (
    key        TEXT    PRIMARY KEY,
    json       TEXT    NOT NULL,
    updated_ms INTEGER NOT NULL
);
";

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
        let version: usize = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(version) {
            conn.execute_batch(&format!(
                "BEGIN; {sql} PRAGMA user_version = {}; COMMIT;",
                i + 1
            ))?;
        }
        Ok(Self { conn })
    }

    /// Saves one snapshot of each quote. A repeat at the same instant replaces
    /// the earlier row.
    pub fn save_quotes(&mut self, quotes: &[Quote], at: DateTime<Utc>) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT OR REPLACE INTO quote_snapshot
                 (symbol, at_ms, last, change, change_pct, share_volume, trades, turnover)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for q in quotes {
                stmt.execute(params![
                    q.symbol,
                    at.timestamp_millis(),
                    q.last.to_string(),
                    q.change.to_string(),
                    q.change_pct.to_string(),
                    q.share_volume as i64,
                    q.trades as i64,
                    q.turnover.to_string(),
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn save_index(&mut self, index: &IndexValue, at: DateTime<Utc>) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO index_snapshot (name, at_ms, value, change, change_pct)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                index.name,
                at.timestamp_millis(),
                index.value.to_string(),
                index.change.to_string(),
                index.change_pct.to_string(),
            ],
        )?;
        Ok(())
    }

    /// Price points for `symbol` in `[from, to)`, oldest first.
    pub fn price_history(
        &self,
        symbol: &str,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<PricePoint>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT at_ms, last, share_volume FROM quote_snapshot
             WHERE symbol = ?1 AND at_ms >= ?2 AND at_ms < ?3 ORDER BY at_ms",
        )?;
        let rows = stmt.query_map(
            params![symbol, from.timestamp_millis(), to.timestamp_millis()],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            },
        )?;
        rows.map(|row| {
            let (ms, price, vol) = row?;
            Ok(PricePoint {
                at: from_ms(ms),
                price: dec(&price)?,
                share_volume: vol.max(0) as u64,
            })
        })
        .collect()
    }

    /// Index values for `name` in `[from, to)`, oldest first.
    pub fn index_history(
        &self,
        name: &str,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<(DateTime<Utc>, Decimal)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT at_ms, value FROM index_snapshot
             WHERE name = ?1 AND at_ms >= ?2 AND at_ms < ?3 ORDER BY at_ms",
        )?;
        let rows = stmt.query_map(
            params![name, from.timestamp_millis(), to.timestamp_millis()],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
        )?;
        rows.map(|row| {
            let (ms, v) = row?;
            Ok((from_ms(ms), dec(&v)?))
        })
        .collect()
    }

    /// Deletes snapshots older than `before`; returns rows removed.
    pub fn prune_snapshots(&mut self, before: DateTime<Utc>) -> Result<usize> {
        let ms = before.timestamp_millis();
        let tx = self.conn.transaction()?;
        let n = tx.execute("DELETE FROM quote_snapshot WHERE at_ms < ?1", [ms])?
            + tx.execute("DELETE FROM index_snapshot WHERE at_ms < ?1", [ms])?;
        tx.commit()?;
        Ok(n)
    }

    /// Saves any serialisable value under `key`, replacing the previous one.
    pub fn save_json<T: Serialize>(&mut self, key: &str, value: &T) -> Result<()> {
        let json = serde_json::to_string(value).map_err(|source| Error::Json {
            key: key.to_owned(),
            source,
        })?;
        self.conn.execute(
            "INSERT OR REPLACE INTO app_state (key, json, updated_ms) VALUES (?1, ?2, ?3)",
            params![key, json, Utc::now().timestamp_millis()],
        )?;
        Ok(())
    }

    /// Loads the value saved under `key`, if any.
    pub fn load_json<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let json: Option<String> = self
            .conn
            .query_row("SELECT json FROM app_state WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()?;
        json.map(|j| {
            serde_json::from_str(&j).map_err(|source| Error::Json {
                key: key.to_owned(),
                source,
            })
        })
        .transpose()
    }

    pub fn create_watch_list(&mut self, name: &str) -> Result<i64> {
        match self
            .conn
            .execute("INSERT INTO watch_list (name) VALUES (?1)", [name])
        {
            Ok(_) => Ok(self.conn.last_insert_rowid()),
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Err(Error::DuplicateWatchList(name.to_owned()))
            }
            Err(e) => Err(e.into()),
        }
    }

    pub fn rename_watch_list(&mut self, id: i64, name: &str) -> Result<()> {
        let n = self
            .conn
            .execute(
                "UPDATE watch_list SET name = ?2 WHERE id = ?1",
                params![id, name],
            )
            .map_err(|e| match e {
                rusqlite::Error::SqliteFailure(f, _)
                    if f.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    Error::DuplicateWatchList(name.to_owned())
                }
                e => e.into(),
            })?;
        if n == 0 {
            return Err(Error::WatchListNotFound(id));
        }
        Ok(())
    }

    pub fn delete_watch_list(&mut self, id: i64) -> Result<()> {
        if self
            .conn
            .execute("DELETE FROM watch_list WHERE id = ?1", [id])?
            == 0
        {
            return Err(Error::WatchListNotFound(id));
        }
        Ok(())
    }

    /// Appends `symbol`; adding a symbol already in the list is a no-op.
    pub fn add_to_watch_list(&mut self, id: i64, symbol: &str) -> Result<()> {
        self.ensure_watch_list(id)?;
        self.conn.execute(
            "INSERT OR IGNORE INTO watch_list_item (watch_list_id, symbol, position)
             VALUES (?1, ?2,
                     (SELECT COALESCE(MAX(position), -1) + 1
                      FROM watch_list_item WHERE watch_list_id = ?1))",
            params![id, symbol],
        )?;
        Ok(())
    }

    pub fn remove_from_watch_list(&mut self, id: i64, symbol: &str) -> Result<()> {
        self.ensure_watch_list(id)?;
        self.conn.execute(
            "DELETE FROM watch_list_item WHERE watch_list_id = ?1 AND symbol = ?2",
            params![id, symbol],
        )?;
        Ok(())
    }

    pub fn watch_lists(&self) -> Result<Vec<WatchList>> {
        let mut lists: Vec<WatchList> = self
            .conn
            .prepare("SELECT id, name FROM watch_list ORDER BY id")?
            .query_map([], |r| {
                Ok(WatchList {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    symbols: Vec::new(),
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut stmt = self.conn.prepare(
            "SELECT symbol FROM watch_list_item WHERE watch_list_id = ?1 ORDER BY position",
        )?;
        for list in &mut lists {
            list.symbols = stmt
                .query_map([list.id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
        }
        Ok(lists)
    }

    fn ensure_watch_list(&self, id: i64) -> Result<()> {
        self.conn
            .query_row("SELECT 1 FROM watch_list WHERE id = ?1", [id], |_| Ok(()))
            .optional()?
            .ok_or(Error::WatchListNotFound(id))
    }
}

fn from_ms(ms: i64) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(ms).single().unwrap_or_default()
}

fn dec(s: &str) -> Result<Decimal> {
    Decimal::from_str(s).map_err(|_| Error::Decimal(s.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use rust_decimal_macros::dec;

    fn quote(symbol: &str, last: Decimal, vol: u64) -> Quote {
        Quote {
            symbol: symbol.into(),
            name: String::new(),
            last,
            change: dec!(0.5),
            change_pct: dec!(1.2),
            prev_close: last,
            open: None,
            high: None,
            low: None,
            share_volume: vol,
            trades: 3,
            turnover: dec!(1000.25),
            last_traded_at: None,
        }
    }

    #[test]
    fn price_history_round_trips_exact_decimals_in_order() {
        let mut s = Store::open_in_memory().unwrap();
        let t0 = Utc.with_ymd_and_hms(2026, 10, 15, 4, 0, 0).unwrap();
        s.save_quotes(
            &[quote("JKH.N0000", dec!(20.10), 5)],
            t0 + Duration::minutes(1),
        )
        .unwrap();
        s.save_quotes(
            &[
                quote("JKH.N0000", dec!(20.00), 2),
                quote("COMB.N0000", dec!(99.9), 1),
            ],
            t0,
        )
        .unwrap();
        let h = s
            .price_history("JKH.N0000", t0, t0 + Duration::hours(1))
            .unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].price, dec!(20.00));
        assert_eq!(h[1].price.to_string(), "20.10");
        assert_eq!(h[1].share_volume, 5);
        assert_eq!(h[0].at, t0);
    }

    #[test]
    fn same_instant_replaces() {
        let mut s = Store::open_in_memory().unwrap();
        let t = Utc::now();
        s.save_quotes(&[quote("A", dec!(1), 1)], t).unwrap();
        s.save_quotes(&[quote("A", dec!(2), 1)], t).unwrap();
        let h = s
            .price_history("A", t - Duration::seconds(1), t + Duration::seconds(1))
            .unwrap();
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].price, dec!(2));
    }

    #[test]
    fn index_history_and_prune() {
        let mut s = Store::open_in_memory().unwrap();
        let t0 = Utc.with_ymd_and_hms(2026, 10, 15, 4, 0, 0).unwrap();
        let idx = |v| IndexValue {
            name: "ASPI".into(),
            value: v,
            change: dec!(0),
            change_pct: dec!(0),
            high: None,
            low: None,
            as_of: None,
        };
        s.save_index(&idx(dec!(16000.5)), t0).unwrap();
        s.save_index(&idx(dec!(16010.25)), t0 + Duration::days(1))
            .unwrap();
        s.save_quotes(&[quote("A", dec!(1), 1)], t0).unwrap();

        assert_eq!(s.prune_snapshots(t0 + Duration::hours(1)).unwrap(), 2);
        let h = s.index_history("ASPI", t0, t0 + Duration::days(2)).unwrap();
        assert_eq!(h, vec![(t0 + Duration::days(1), dec!(16010.25))]);
    }

    #[test]
    fn watch_lists_crud() {
        let mut s = Store::open_in_memory().unwrap();
        let id = s.create_watch_list("Banks").unwrap();
        s.add_to_watch_list(id, "COMB.N0000").unwrap();
        s.add_to_watch_list(id, "HNB.N0000").unwrap();
        s.add_to_watch_list(id, "COMB.N0000").unwrap(); // duplicate ignored
        s.rename_watch_list(id, "My Banks").unwrap();

        let lists = s.watch_lists().unwrap();
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0].name, "My Banks");
        assert_eq!(lists[0].symbols, ["COMB.N0000", "HNB.N0000"]);

        s.remove_from_watch_list(id, "COMB.N0000").unwrap();
        assert_eq!(s.watch_lists().unwrap()[0].symbols, ["HNB.N0000"]);

        assert!(matches!(
            s.create_watch_list("My Banks"),
            Err(Error::DuplicateWatchList(_))
        ));
        s.delete_watch_list(id).unwrap();
        assert!(s.watch_lists().unwrap().is_empty());
        assert!(matches!(
            s.add_to_watch_list(id, "X"),
            Err(Error::WatchListNotFound(_))
        ));
    }

    #[test]
    fn json_state_round_trips() {
        let mut s = Store::open_in_memory().unwrap();
        assert_eq!(s.load_json::<Vec<u32>>("k").unwrap(), None);
        s.save_json("k", &vec![1u32, 2]).unwrap();
        s.save_json("k", &vec![3u32]).unwrap();
        assert_eq!(s.load_json::<Vec<u32>>("k").unwrap(), Some(vec![3]));
        assert!(matches!(
            s.load_json::<String>("k"),
            Err(Error::Json { .. })
        ));
    }

    #[test]
    fn upgrades_a_version_1_database() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&format!("{V1} PRAGMA user_version = 1;"))
            .unwrap();
        conn.execute("INSERT INTO watch_list (name) VALUES ('Old')", [])
            .unwrap();
        let mut s = Store::init(conn).unwrap();
        assert_eq!(s.watch_lists().unwrap()[0].name, "Old");
        s.save_json("k", &1).unwrap();
    }

    #[test]
    fn reopening_a_file_keeps_data() {
        let dir = std::env::temp_dir().join(format!("cse-storage-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("db.sqlite");
        {
            let mut s = Store::open(&path).unwrap();
            s.create_watch_list("Keep").unwrap();
        }
        let s = Store::open(&path).unwrap();
        assert_eq!(s.watch_lists().unwrap()[0].name, "Keep");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
