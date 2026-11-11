//! Schedule and delivery repository (handbook sections 9 and 11).
//!
//! This layer only moves raw rows. Their meaning (JSON template, RRULE text,
//! misfire policy, delivery states) is owned by [`crate::scheduler`].

use rusqlite::{params, OptionalExtension, Row, TransactionBehavior};

use super::{get_opt_ts, Store};
use crate::schema::{ts_to_sql, Result, Timestamp};

/// A raw `schedules` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleRow {
    /// Primary key.
    pub id: String,
    /// JSON template (title, body, original rule, action kind, ...).
    pub task_template: String,
    /// IANA timezone name.
    pub timezone: String,
    /// UTC instant of the next unprocessed occurrence, `None` when exhausted.
    pub next_run_at: Option<Timestamp>,
    /// RRULE-subset text, `None` for a one-shot schedule.
    pub recurrence: Option<String>,
    /// Misfire policy spelling.
    pub misfire_policy: String,
    /// Whether the schedule is active.
    pub enabled: bool,
    /// When the user deleted the schedule (soft delete). A deleted schedule is invisible
    /// to listings and the engine; its deliveries remain as history. Ignored on insert
    /// and replace (use [`Store::soft_delete_schedule_row`]).
    pub deleted_at: Option<Timestamp>,
}

/// A raw `deliveries` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryRow {
    /// Primary key.
    pub id: String,
    /// Owning schedule.
    pub schedule_id: String,
    /// Identifier of the occurrence, unique per schedule.
    pub occurrence_key: String,
    /// Status spelling.
    pub status: String,
    /// When the notification was confirmed delivered.
    pub delivered_at: Option<Timestamp>,
}

/// A delivery row about to be inserted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDeliveryRow {
    /// Primary key.
    pub id: String,
    /// Occurrence key.
    pub occurrence_key: String,
    /// Initial status spelling.
    pub status: String,
}

/// Columns written on insert (everything but `deleted_at`).
const SCHEDULE_COLS: &str =
    "id, task_template, timezone, next_run_at, recurrence, misfire_policy, enabled";
/// Columns read back.
const SCHEDULE_SELECT: &str =
    "id, task_template, timezone, next_run_at, recurrence, misfire_policy, enabled, deleted_at";
const DELIVERY_COLS: &str = "id, schedule_id, occurrence_key, status, delivered_at";

fn schedule_from_row(r: &Row<'_>) -> rusqlite::Result<ScheduleRow> {
    Ok(ScheduleRow {
        id: r.get(0)?,
        task_template: r.get(1)?,
        timezone: r.get(2)?,
        next_run_at: get_opt_ts(r, 3)?,
        recurrence: r.get(4)?,
        misfire_policy: r.get(5)?,
        enabled: r.get::<_, i64>(6)? != 0,
        deleted_at: get_opt_ts(r, 7)?,
    })
}

fn delivery_from_row(r: &Row<'_>) -> rusqlite::Result<DeliveryRow> {
    Ok(DeliveryRow {
        id: r.get(0)?,
        schedule_id: r.get(1)?,
        occurrence_key: r.get(2)?,
        status: r.get(3)?,
        delivered_at: get_opt_ts(r, 4)?,
    })
}

impl Store {
    /// Inserts a new schedule row.
    pub fn insert_schedule_row(&mut self, row: &ScheduleRow) -> Result<()> {
        self.conn.execute(
            &format!("INSERT INTO schedules ({SCHEDULE_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"),
            params![
                row.id,
                row.task_template,
                row.timezone,
                row.next_run_at.as_ref().map(ts_to_sql),
                row.recurrence,
                row.misfire_policy,
                i64::from(row.enabled)
            ],
        )?;
        Ok(())
    }

    /// Loads one schedule row, deleted or not (check `deleted_at`).
    pub fn get_schedule_row(&self, id: &str) -> Result<Option<ScheduleRow>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {SCHEDULE_SELECT} FROM schedules WHERE id = ?1"),
                [id],
                schedule_from_row,
            )
            .optional()?)
    }

    /// Schedule rows in creation order; soft-deleted ones only with `include_deleted`.
    pub fn list_schedule_rows(&self, include_deleted: bool) -> Result<Vec<ScheduleRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {SCHEDULE_SELECT} FROM schedules
             WHERE (?1 OR deleted_at IS NULL) ORDER BY rowid"
        ))?;
        let rows = stmt.query_map([include_deleted], schedule_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Replaces every mutable column of an existing, not deleted schedule; false if it
    /// does not exist or was deleted.
    pub fn replace_schedule_row(&mut self, row: &ScheduleRow) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE schedules SET task_template = ?2, timezone = ?3, next_run_at = ?4,
                 recurrence = ?5, misfire_policy = ?6, enabled = ?7
             WHERE id = ?1 AND deleted_at IS NULL",
            params![
                row.id,
                row.task_template,
                row.timezone,
                row.next_run_at.as_ref().map(ts_to_sql),
                row.recurrence,
                row.misfire_policy,
                i64::from(row.enabled)
            ],
        )?;
        Ok(n == 1)
    }

    /// Soft-deletes a schedule (handbook 9: trigger records are kept). In ONE transaction:
    /// sets `deleted_at`, disables it and clears `next_run_at`, and cancels deliveries that
    /// were still `pending` so they are never notified. All other delivery rows stay
    /// untouched. Returns false if the schedule does not exist or is already deleted.
    pub fn soft_delete_schedule_row(&mut self, id: &str, now: Timestamp) -> Result<bool> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let n = tx.execute(
            "UPDATE schedules SET deleted_at = ?2, enabled = 0, next_run_at = NULL
             WHERE id = ?1 AND deleted_at IS NULL",
            params![id, ts_to_sql(&now)],
        )?;
        if n == 0 {
            return Ok(false);
        }
        tx.execute(
            "UPDATE deliveries SET status = 'cancelled' WHERE schedule_id = ?1 AND status = 'pending'",
            [id],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Earliest `next_run_at` among enabled schedules.
    pub fn earliest_next_run(&self) -> Result<Option<Timestamp>> {
        let min: Option<String> = self.conn.query_row(
            "SELECT MIN(next_run_at) FROM schedules
             WHERE enabled = 1 AND deleted_at IS NULL AND next_run_at IS NOT NULL",
            [],
            |r| r.get(0),
        )?;
        min.map(|s| crate::schema::ts_from_sql(&s)).transpose()
    }

    /// Ids of enabled schedules whose `next_run_at` is at or before `now`,
    /// oldest first (ties broken by id so processing order is deterministic).
    pub fn due_schedule_ids(&self, now: Timestamp) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM schedules
             WHERE enabled = 1 AND deleted_at IS NULL
               AND next_run_at IS NOT NULL AND next_run_at <= ?1
             ORDER BY next_run_at, id",
        )?;
        let rows = stmt.query_map([ts_to_sql(&now)], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// In ONE transaction: inserts the delivery rows (ignoring any whose
    /// `(schedule_id, occurrence_key)` already exists) and moves the schedule's
    /// `next_run_at`. Returns the occurrence keys that were newly inserted, so a
    /// caller never acts on an occurrence that was already handled.
    pub fn commit_occurrences(
        &mut self,
        schedule_id: &str,
        rows: &[NewDeliveryRow],
        next_run_at: Option<Timestamp>,
    ) -> Result<Vec<String>> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let live: i64 = tx.query_row(
            "SELECT COUNT(*) FROM schedules WHERE id = ?1 AND deleted_at IS NULL",
            [schedule_id],
            |r| r.get(0),
        )?;
        if live == 0 {
            // deleted (or gone) schedules record nothing
            return Ok(Vec::new());
        }
        let mut inserted = Vec::new();
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO deliveries (id, schedule_id, occurrence_key, status, delivered_at)
                 VALUES (?1, ?2, ?3, ?4, NULL)",
            )?;
            for r in rows {
                let n = stmt.execute(params![r.id, schedule_id, r.occurrence_key, r.status])?;
                if n == 1 {
                    inserted.push(r.occurrence_key.clone());
                }
            }
        }
        tx.execute(
            "UPDATE schedules SET next_run_at = ?2 WHERE id = ?1",
            params![schedule_id, next_run_at.as_ref().map(ts_to_sql)],
        )?;
        tx.commit()?;
        Ok(inserted)
    }

    /// Deliveries still waiting for a confirmed notification, grouped by schedule.
    /// Deliveries of soft-deleted schedules are never returned.
    pub fn pending_delivery_rows(&self) -> Result<Vec<DeliveryRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {DELIVERY_COLS} FROM deliveries
             WHERE status = 'pending'
               AND schedule_id IN (SELECT id FROM schedules WHERE deleted_at IS NULL)
             ORDER BY schedule_id, occurrence_key"
        ))?;
        let rows = stmt.query_map([], delivery_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Marks pending deliveries as delivered at `at`; returns how many changed.
    pub fn mark_deliveries_delivered(&mut self, ids: &[String], at: Timestamp) -> Result<usize> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut changed = 0;
        {
            let mut stmt = tx.prepare(
                "UPDATE deliveries SET status = 'delivered', delivered_at = ?2
                 WHERE id = ?1 AND status = 'pending'",
            )?;
            for id in ids {
                changed += stmt.execute(params![id, ts_to_sql(&at)])?;
            }
        }
        tx.commit()?;
        Ok(changed)
    }

    /// Loads one delivery row.
    pub fn get_delivery_row(&self, id: &str) -> Result<Option<DeliveryRow>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {DELIVERY_COLS} FROM deliveries WHERE id = ?1"),
                [id],
                delivery_from_row,
            )
            .optional()?)
    }

    /// Delivery history, newest occurrence first, optionally filtered.
    pub fn list_delivery_rows(
        &self,
        schedule_id: Option<&str>,
        status: Option<&str>,
        limit: u32,
    ) -> Result<Vec<DeliveryRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {DELIVERY_COLS} FROM deliveries
             WHERE (?1 IS NULL OR schedule_id = ?1) AND (?2 IS NULL OR status = ?2)
             ORDER BY occurrence_key DESC, rowid DESC LIMIT ?3"
        ))?;
        let rows = stmt.query_map(params![schedule_id, status, limit], delivery_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Moves a delivery to `to` only if it currently has one of the `from`
    /// statuses; returns whether it changed.
    pub fn transition_delivery(&mut self, id: &str, from: &[&str], to: &str) -> Result<bool> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: Option<String> = tx
            .query_row("SELECT status FROM deliveries WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()?;
        let ok = current.as_deref().is_some_and(|c| from.contains(&c));
        if ok {
            tx.execute(
                "UPDATE deliveries SET status = ?2 WHERE id = ?1",
                params![id, to],
            )?;
        }
        tx.commit()?;
        Ok(ok)
    }

    /// In ONE transaction: marks a `delivered` delivery as `snoozed` and inserts
    /// the follow-up schedule. Returns false (and changes nothing) if the
    /// delivery is not in the `delivered` state.
    pub fn snooze_delivery(&mut self, delivery_id: &str, follow_up: &ScheduleRow) -> Result<bool> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let n = tx.execute(
            "UPDATE deliveries SET status = 'snoozed'
             WHERE id = ?1 AND status = 'delivered'
               AND schedule_id IN (SELECT id FROM schedules WHERE deleted_at IS NULL)",
            [delivery_id],
        )?;
        if n == 0 {
            return Ok(false);
        }
        tx.execute(
            &format!("INSERT INTO schedules ({SCHEDULE_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"),
            params![
                follow_up.id,
                follow_up.task_template,
                follow_up.timezone,
                follow_up.next_run_at.as_ref().map(ts_to_sql),
                follow_up.recurrence,
                follow_up.misfire_policy,
                i64::from(follow_up.enabled)
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }
}
