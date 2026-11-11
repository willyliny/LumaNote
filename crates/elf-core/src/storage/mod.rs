//! SQLite storage: connection setup, migrations and repositories.
//!
//! All repository methods live on [`Store`]. Writes that touch several rows run
//! inside one `IMMEDIATE` transaction.

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, Row};
use serde_json::Value;

use crate::ports::Clock;
use crate::schema::{ts_from_sql, ts_to_sql, ElfError, Result, Timestamp};

mod content;
pub mod grants;
pub mod migrate;
pub mod tasks;

#[cfg(test)]
mod tests;

pub use content::{Artifact, Conversation, StoredMessage};
pub use grants::{Grant, NewGrant};
pub use migrate::{Migration, MIGRATIONS};
pub use tasks::{RecoveredTask, StepOutcome, Task, TaskEvent, TaskStep};

/// Handle to the application database (one connection).
#[derive(Debug)]
pub struct Store {
    pub(crate) conn: Connection,
}

impl Store {
    /// Opens (creating if needed) the database at `path`, enables WAL and
    /// foreign keys, and applies pending migrations.
    pub fn open(path: &Path, clock: &dyn Clock) -> Result<Self> {
        Self::open_with(path, clock, MIGRATIONS)
    }

    /// Opens a private in-memory database (tests, previews).
    pub fn open_in_memory(clock: &dyn Clock) -> Result<Self> {
        Self::init(Connection::open_in_memory()?, clock, MIGRATIONS)
    }

    pub(crate) fn open_with(
        path: &Path,
        clock: &dyn Clock,
        migrations: &[Migration],
    ) -> Result<Self> {
        Self::init(Connection::open(path)?, clock, migrations)
    }

    fn init(mut conn: Connection, clock: &dyn Clock, migrations: &[Migration]) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        // Returns the resulting mode ("wal", or "memory" for in-memory databases).
        let _mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        migrate::apply(&mut conn, migrations, &ts_to_sql(&clock.now_utc()))?;
        Ok(Self { conn })
    }

    /// Highest applied migration version (0 if none).
    pub fn schema_version(&self) -> Result<u32> {
        migrate::current_version(&self.conn)
    }

    /// Current journal mode, e.g. `wal`.
    pub fn journal_mode(&self) -> Result<String> {
        Ok(self
            .conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))?)
    }

    /// Whether foreign-key enforcement is on.
    pub fn foreign_keys_enabled(&self) -> Result<bool> {
        Ok(self
            .conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0))?
            == 1)
    }
}

/// Converts a domain error raised while mapping a column into a rusqlite error.
pub(crate) fn conv<T>(idx: usize, r: Result<T>) -> rusqlite::Result<T> {
    r.map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(idx, rusqlite::types::Type::Text, Box::new(e))
    })
}

pub(crate) fn get_ts(row: &Row<'_>, idx: usize) -> rusqlite::Result<Timestamp> {
    let s: String = row.get(idx)?;
    conv(idx, ts_from_sql(&s))
}

pub(crate) fn get_opt_ts(row: &Row<'_>, idx: usize) -> rusqlite::Result<Option<Timestamp>> {
    let s: Option<String> = row.get(idx)?;
    s.map(|s| conv(idx, ts_from_sql(&s))).transpose()
}

pub(crate) fn get_json(row: &Row<'_>, idx: usize) -> rusqlite::Result<Value> {
    let s: String = row.get(idx)?;
    conv(
        idx,
        serde_json::from_str(&s).map_err(|e| ElfError::Storage {
            message: format!("bad stored JSON: {e}"),
        }),
    )
}

pub(crate) fn to_json(v: &impl serde::Serialize) -> Result<String> {
    serde_json::to_string(v).map_err(|e| ElfError::Storage {
        message: format!("cannot serialise JSON: {e}"),
    })
}
