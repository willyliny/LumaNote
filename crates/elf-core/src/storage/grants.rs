//! Grant repository (handbook sections 8 and 9). Matching logic lives in [`crate::policy`].

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use super::{get_opt_ts, get_ts, Store};
use crate::schema::{ts_to_sql, ElfError, GrantId, Result, TaskId, Timestamp};

/// A stored grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    /// Identifier.
    pub id: GrantId,
    /// Action name, e.g. `read`, `write`, `delete`, `send`.
    pub action: String,
    /// Directory path (for file actions) or resource pattern (exact, or `prefix*`).
    pub resource_scope: String,
    /// If set, only calls with exactly this arguments hash are covered.
    pub arguments_hash: Option<String>,
    /// Expiry; the grant is invalid at and after this instant.
    pub expires_at: Option<Timestamp>,
    /// Maximum number of uses, unlimited if `None`.
    pub max_uses: Option<u32>,
    /// Uses consumed so far.
    pub used_count: u32,
    /// Task whose approval created the grant.
    pub source_task: Option<TaskId>,
    /// Revocation time.
    pub revoked_at: Option<Timestamp>,
    /// Creation time.
    pub created_at: Timestamp,
}

impl Grant {
    /// True if the grant can still be used at `now`.
    pub fn is_active(&self, now: Timestamp) -> bool {
        self.revoked_at.is_none()
            && self.expires_at.is_none_or(|e| now < e)
            && self.max_uses.is_none_or(|m| self.used_count < m)
    }
}

/// Parameters for a new grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewGrant {
    /// Action name.
    pub action: String,
    /// Scope (directory or resource pattern).
    pub resource_scope: String,
    /// Optional exact-arguments binding.
    pub arguments_hash: Option<String>,
    /// Optional expiry.
    pub expires_at: Option<Timestamp>,
    /// Optional use limit.
    pub max_uses: Option<u32>,
    /// Task whose approval created the grant.
    pub source_task: Option<TaskId>,
}

pub(crate) const GRANT_COLS: &str =
    "id, action, resource_scope, arguments_hash, expires_at, max_uses, used_count, source_task, revoked_at, created_at";

fn grant_from_row(r: &Row<'_>) -> rusqlite::Result<Grant> {
    Ok(Grant {
        id: GrantId::from_string(r.get::<_, String>(0)?),
        action: r.get(1)?,
        resource_scope: r.get(2)?,
        arguments_hash: r.get(3)?,
        expires_at: get_opt_ts(r, 4)?,
        max_uses: r
            .get::<_, Option<i64>>(5)?
            .and_then(|v| u32::try_from(v).ok()),
        used_count: u32::try_from(r.get::<_, i64>(6)?).unwrap_or(u32::MAX),
        source_task: r.get::<_, Option<String>>(7)?.map(TaskId::from_string),
        revoked_at: get_opt_ts(r, 8)?,
        created_at: get_ts(r, 9)?,
    })
}

/// Grants for one action in creation order.
pub(crate) fn grants_for_action(conn: &Connection, action: &str) -> Result<Vec<Grant>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {GRANT_COLS} FROM grants WHERE action = ?1 ORDER BY rowid"
    ))?;
    let rows = stmt.query_map([action], grant_from_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Atomically consumes one use if the grant is still active; returns whether it did.
pub(crate) fn consume(conn: &Connection, id: &GrantId, now: Timestamp) -> Result<bool> {
    let n = conn.execute(
        "UPDATE grants SET used_count = used_count + 1
         WHERE id = ?1 AND revoked_at IS NULL
           AND (expires_at IS NULL OR expires_at > ?2)
           AND (max_uses IS NULL OR used_count < max_uses)",
        params![id.as_str(), ts_to_sql(&now)],
    )?;
    Ok(n == 1)
}

impl Store {
    /// Stores a new grant.
    pub fn insert_grant(&mut self, new: &NewGrant, now: Timestamp) -> Result<Grant> {
        let id = GrantId::new();
        self.conn.execute(
            "INSERT INTO grants (id, action, resource_scope, arguments_hash, expires_at, max_uses, source_task, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id.as_str(),
                new.action,
                new.resource_scope,
                new.arguments_hash,
                new.expires_at.as_ref().map(ts_to_sql),
                new.max_uses,
                new.source_task.as_ref().map(TaskId::as_str),
                ts_to_sql(&now)
            ],
        )?;
        self.get_grant(&id)
    }

    /// Whether any grant for `action` is active at `now`, without consuming a use.
    /// Used to decide which tools to offer a task; the real check is still
    /// [`crate::policy::evaluate`] at execution time.
    pub fn has_active_grant_for_action(&self, action: &str, now: Timestamp) -> Result<bool> {
        Ok(grants_for_action(&self.conn, action)?
            .iter()
            .any(|g| g.is_active(now)))
    }

    /// Loads a grant.
    pub fn get_grant(&self, id: &GrantId) -> Result<Grant> {
        self.conn
            .query_row(
                &format!("SELECT {GRANT_COLS} FROM grants WHERE id = ?1"),
                [id.as_str()],
                grant_from_row,
            )
            .optional()?
            .ok_or_else(|| ElfError::invalid_input(format!("unknown grant {id}")))
    }

    /// All grants (including inactive ones) in creation order.
    pub fn list_grants(&self) -> Result<Vec<Grant>> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {GRANT_COLS} FROM grants ORDER BY rowid"))?;
        let rows = stmt.query_map([], grant_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Revokes a grant immediately (idempotent: the first revocation time is kept).
    pub fn revoke_grant(&mut self, id: &GrantId, now: Timestamp) -> Result<Grant> {
        let n = self.conn.execute(
            "UPDATE grants SET revoked_at = COALESCE(revoked_at, ?2) WHERE id = ?1",
            params![id.as_str(), ts_to_sql(&now)],
        )?;
        if n == 0 {
            return Err(ElfError::invalid_input(format!("unknown grant {id}")));
        }
        self.get_grant(id)
    }
}
