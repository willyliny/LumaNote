//! Versioned SQL migrations. Each runs in its own transaction, so a failing
//! migration leaves the database exactly as it was.

use rusqlite::{Connection, OptionalExtension};

use crate::schema::{LumaError, Result};

/// One schema migration.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    /// Strictly increasing version number.
    pub version: u32,
    /// Short name recorded in `schema_version`.
    pub name: &'static str,
    /// SQL batch.
    pub sql: &'static str,
}

/// All migrations shipped with this build, in order.
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "init",
        sql: include_str!("../../migrations/0001_init.sql"),
    },
    Migration {
        version: 2,
        name: "agent",
        sql: include_str!("../../migrations/0002_agent.sql"),
    },
    Migration {
        version: 3,
        name: "policy_schedules",
        sql: include_str!("../../migrations/0003_policy_schedules.sql"),
    },
    Migration {
        version: 4,
        name: "security_review",
        sql: include_str!("../../migrations/0004_security_review.sql"),
    },
];

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let found: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [name],
            |r| r.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

/// Highest applied version (0 if the version table does not exist yet).
pub fn current_version(conn: &Connection) -> Result<u32> {
    if !table_exists(conn, "schema_version")? {
        return Ok(0);
    }
    let v: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |r| r.get(0),
    )?;
    u32::try_from(v).map_err(|_| LumaError::Storage {
        message: format!("invalid schema version {v}"),
    })
}

/// Applies every migration newer than the database's version; returns the versions applied.
pub fn apply(conn: &mut Connection, migrations: &[Migration], now: &str) -> Result<Vec<u32>> {
    if migrations.windows(2).any(|w| w[0].version >= w[1].version) {
        return Err(LumaError::Storage {
            message: "migration versions must be strictly increasing".into(),
        });
    }
    let current = current_version(conn)?;
    if let Some(last) = migrations.last() {
        if current > last.version {
            return Err(LumaError::Storage {
                message: format!(
                    "database schema version {current} is newer than this build supports ({})",
                    last.version
                ),
            });
        }
    }
    let mut applied = Vec::new();
    for m in migrations.iter().filter(|m| m.version > current) {
        let tx = conn.transaction()?;
        let run = || -> rusqlite::Result<()> {
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS schema_version (
                     version INTEGER PRIMARY KEY, name TEXT NOT NULL, applied_at TEXT NOT NULL)",
            )?;
            tx.execute_batch(m.sql)?;
            tx.execute(
                "INSERT INTO schema_version (version, name, applied_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![m.version, m.name, now],
            )?;
            Ok(())
        };
        run().map_err(|e| LumaError::Storage {
            message: format!("migration {} ({}) failed: {e}", m.version, m.name),
        })?;
        tx.commit()?;
        applied.push(m.version);
    }
    Ok(applied)
}
