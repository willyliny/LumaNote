//! Tasks, task steps (write-ahead rule), task events and restart recovery.

use rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{conv, get_json, get_opt_ts, get_ts, to_json, Store};
use crate::ports::Clock;
use crate::schema::{ts_to_sql, ElfError, EvidenceRef, Result, StepId, TaskId, Timestamp, Trust};
use crate::task::{check_transition, SideEffect, StepStatus, TaskStatus};

/// A task row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    /// Identifier.
    pub id: TaskId,
    /// What the user asked for.
    pub intent: String,
    /// Current state.
    pub status: TaskStatus,
    /// Scope declared at creation (e.g. `{"output_dirs": [...]}`).
    pub scope: Value,
    /// Creation time.
    pub created_at: Timestamp,
    /// Last change time.
    pub updated_at: Timestamp,
    /// First evidence reference recorded when the task completed.
    pub result_ref: Option<String>,
    /// Set once external (untrusted) content entered the task (handbook section 8).
    pub untrusted_content: bool,
}

/// A task step row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskStep {
    /// Identifier.
    pub id: StepId,
    /// Owning task.
    pub task_id: TaskId,
    /// 1-based order within the task.
    pub seq: i64,
    /// Tool name.
    pub tool: String,
    /// Arguments with secrets removed.
    pub args_redacted: Option<String>,
    /// Side-effect class.
    pub side_effect: SideEffect,
    /// Current status.
    pub status: StepStatus,
    /// Idempotency key for the external call, if any.
    pub idempotency_key: Option<String>,
    /// When the step was recorded (before execution).
    pub started_at: Timestamp,
    /// When it finished.
    pub finished_at: Option<Timestamp>,
    /// Evidence reference produced by the step.
    pub evidence_ref: Option<String>,
}

/// A task event row (source of frontend event sequence numbers).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskEvent {
    /// Global row id.
    pub id: i64,
    /// Owning task.
    pub task_id: TaskId,
    /// Per-task increasing sequence starting at 1.
    pub sequence: i64,
    /// Event type, e.g. `created`, `status_changed`, `transition_rejected`.
    pub event_type: String,
    /// Event details.
    pub payload: Value,
    /// When it was written.
    pub created_at: Timestamp,
}

/// How a step ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepOutcome {
    /// Completed successfully.
    Succeeded,
    /// Failed with a known result.
    Failed,
    /// External write whose result is unknown.
    Uncertain,
}

impl StepOutcome {
    fn status(self) -> StepStatus {
        match self {
            StepOutcome::Succeeded => StepStatus::Succeeded,
            StepOutcome::Failed => StepStatus::Failed,
            StepOutcome::Uncertain => StepStatus::Uncertain,
        }
    }
}

/// Result of restart recovery for one task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredTask {
    /// The task.
    pub task_id: TaskId,
    /// Always `running` (only running tasks are touched).
    pub from: TaskStatus,
    /// `paused` or `uncertain`.
    pub to: TaskStatus,
    /// Steps marked `uncertain` (external writes that were in flight).
    pub uncertain_steps: Vec<StepId>,
}

const TASK_COLS: &str =
    "id, intent, status, scope_json, created_at, updated_at, result_ref, untrusted_content";
const STEP_COLS: &str = "id, task_id, seq, tool, args_redacted, side_effect, status, idempotency_key, started_at, finished_at, evidence_ref";

fn task_from_row(r: &Row<'_>) -> rusqlite::Result<Task> {
    let status: String = r.get(2)?;
    Ok(Task {
        id: TaskId::from_string(r.get::<_, String>(0)?),
        intent: r.get(1)?,
        status: conv(2, TaskStatus::parse(&status))?,
        scope: get_json(r, 3)?,
        created_at: get_ts(r, 4)?,
        updated_at: get_ts(r, 5)?,
        result_ref: r.get(6)?,
        untrusted_content: r.get::<_, i64>(7)? != 0,
    })
}

fn step_from_row(r: &Row<'_>) -> rusqlite::Result<TaskStep> {
    let side: String = r.get(5)?;
    let status: String = r.get(6)?;
    Ok(TaskStep {
        id: StepId::from_string(r.get::<_, String>(0)?),
        task_id: TaskId::from_string(r.get::<_, String>(1)?),
        seq: r.get(2)?,
        tool: r.get(3)?,
        args_redacted: r.get(4)?,
        side_effect: conv(5, SideEffect::parse(&side))?,
        status: conv(6, StepStatus::parse(&status))?,
        idempotency_key: r.get(7)?,
        started_at: get_ts(r, 8)?,
        finished_at: get_opt_ts(r, 9)?,
        evidence_ref: r.get(10)?,
    })
}

fn event_from_row(r: &Row<'_>) -> rusqlite::Result<TaskEvent> {
    Ok(TaskEvent {
        id: r.get(0)?,
        task_id: TaskId::from_string(r.get::<_, String>(1)?),
        sequence: r.get(2)?,
        event_type: r.get(3)?,
        payload: get_json(r, 4)?,
        created_at: get_ts(r, 5)?,
    })
}

pub(crate) fn load_task(conn: &Connection, id: &TaskId) -> Result<Task> {
    conn.query_row(
        &format!("SELECT {TASK_COLS} FROM tasks WHERE id = ?1"),
        [id.as_str()],
        task_from_row,
    )
    .optional()?
    .ok_or_else(|| ElfError::invalid_input(format!("unknown task {id}")))
}

fn insert_event(
    conn: &Connection,
    task_id: &TaskId,
    event_type: &str,
    payload: &Value,
    now: Timestamp,
) -> Result<TaskEvent> {
    let sequence: i64 = conn.query_row(
        "SELECT COALESCE(MAX(sequence), 0) + 1 FROM task_events WHERE task_id = ?1",
        [task_id.as_str()],
        |r| r.get(0),
    )?;
    conn.execute(
        "INSERT INTO task_events (task_id, sequence, type, payload_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![task_id.as_str(), sequence, event_type, to_json(payload)?, ts_to_sql(&now)],
    )?;
    Ok(TaskEvent {
        id: conn.last_insert_rowid(),
        task_id: task_id.clone(),
        sequence,
        event_type: event_type.to_owned(),
        payload: payload.clone(),
        created_at: now,
    })
}

fn rejected(from: TaskStatus, to: TaskStatus, reason: &str) -> ElfError {
    ElfError::InvalidTransition {
        from: from.to_string(),
        to: to.to_string(),
        reason: Some(reason.to_owned()),
    }
}

fn check_evidence(conn: &Connection, task: &Task, evidence: &[EvidenceRef]) -> Result<()> {
    if evidence.is_empty() {
        return Err(rejected(
            task.status,
            TaskStatus::Completed,
            "completed requires at least one evidence reference",
        ));
    }
    for e in evidence {
        let ok = match e {
            EvidenceRef::Artifact { id } => conn
                .query_row(
                    "SELECT 1 FROM artifacts WHERE id = ?1 AND task_id = ?2",
                    params![id.as_str(), task.id.as_str()],
                    |_| Ok(()),
                )
                .optional()?
                .is_some(),
            EvidenceRef::Step { id } => conn
                .query_row(
                    "SELECT 1 FROM task_steps WHERE id = ?1 AND task_id = ?2 AND status = 'succeeded'",
                    params![id.as_str(), task.id.as_str()],
                    |_| Ok(()),
                )
                .optional()?
                .is_some(),
        };
        if !ok {
            return Err(rejected(
                task.status,
                TaskStatus::Completed,
                &format!(
                    "evidence {} does not exist for this task",
                    e.to_ref_string()
                ),
            ));
        }
    }
    Ok(())
}

/// Performs one transition inside an open transaction (no commit).
fn apply_transition(
    conn: &Connection,
    id: &TaskId,
    to: TaskStatus,
    evidence: &[EvidenceRef],
    reason: Option<&str>,
    extra: Value,
    now: Timestamp,
) -> Result<TaskEvent> {
    let task = load_task(conn, id)?;
    check_transition(task.status, to)?;
    let mut result_ref = None;
    if to == TaskStatus::Completed {
        check_evidence(conn, &task, evidence)?;
        result_ref = evidence.first().map(EvidenceRef::to_ref_string);
    }
    conn.execute(
        "UPDATE tasks SET status = ?2, updated_at = ?3, result_ref = COALESCE(?4, result_ref) WHERE id = ?1",
        params![id.as_str(), to.as_str(), ts_to_sql(&now), result_ref],
    )?;
    let mut payload = json!({
        "from": task.status.as_str(),
        "to": to.as_str(),
        "reason": reason,
        "evidence": evidence.iter().map(EvidenceRef::to_ref_string).collect::<Vec<_>>(),
    });
    if let (Some(p), Some(x)) = (payload.as_object_mut(), extra.as_object()) {
        p.extend(x.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    insert_event(conn, id, "status_changed", &payload, now)
}

impl Store {
    /// Creates a queued task and its first event (`created`, sequence 1).
    pub fn create_task(&mut self, intent: &str, scope: &Value, now: Timestamp) -> Result<Task> {
        let id = TaskId::new();
        let ts = ts_to_sql(&now);
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO tasks (id, intent, status, scope_json, created_at, updated_at)
             VALUES (?1, ?2, 'queued', ?3, ?4, ?4)",
            params![id.as_str(), intent, to_json(scope)?, ts],
        )?;
        insert_event(&tx, &id, "created", &json!({"status": "queued"}), now)?;
        tx.commit()?;
        self.get_task(&id)
    }

    /// Loads a task; unknown IDs are an `InvalidInput` error.
    pub fn get_task(&self, id: &TaskId) -> Result<Task> {
        load_task(&self.conn, id)
    }

    /// Lists tasks oldest first, optionally filtered by status.
    pub fn list_tasks(&self, status: Option<TaskStatus>) -> Result<Vec<Task>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {TASK_COLS} FROM tasks WHERE (?1 IS NULL OR status = ?1) ORDER BY created_at, rowid"
        ))?;
        let rows = stmt.query_map([status.map(TaskStatus::as_str)], task_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Moves a task along the transition table and appends a `status_changed` event.
    ///
    /// `completed` additionally requires at least one existing evidence reference
    /// (an artifact of the task or a succeeded step of the task). A rejected
    /// transition changes nothing except a `transition_rejected` event, and the
    /// error is returned.
    pub fn transition_task(
        &mut self,
        id: &TaskId,
        to: TaskStatus,
        evidence: &[EvidenceRef],
        reason: Option<&str>,
        clock: &dyn Clock,
    ) -> Result<TaskEvent> {
        let now = clock.now_utc();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        match apply_transition(&tx, id, to, evidence, reason, json!({}), now) {
            Ok(ev) => {
                tx.commit()?;
                Ok(ev)
            }
            Err(e @ ElfError::InvalidTransition { .. }) => {
                let payload = json!({"to": to.as_str(), "reason": reason, "error": e.to_string()});
                insert_event(&tx, id, "transition_rejected", &payload, now)?;
                tx.commit()?;
                Err(e)
            }
            Err(e) => Err(e),
        }
    }

    /// Task events with `sequence > after_sequence`, in order.
    pub fn list_task_events(
        &self,
        task_id: &TaskId,
        after_sequence: i64,
    ) -> Result<Vec<TaskEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, sequence, type, payload_json, created_at
             FROM task_events WHERE task_id = ?1 AND sequence > ?2 ORDER BY sequence",
        )?;
        let rows = stmt.query_map(params![task_id.as_str(), after_sequence], event_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Sets the task-level untrusted-content flag (idempotent). Returns true if it newly changed.
    pub fn mark_task_untrusted(&mut self, id: &TaskId, now: Timestamp) -> Result<bool> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task = load_task(&tx, id)?;
        if task.untrusted_content {
            return Ok(false);
        }
        tx.execute(
            "UPDATE tasks SET untrusted_content = 1, updated_at = ?2 WHERE id = ?1",
            params![id.as_str(), ts_to_sql(&now)],
        )?;
        insert_event(&tx, id, "marked_untrusted", &json!({}), now)?;
        tx.commit()?;
        Ok(true)
    }

    /// Notes the trust level of a tool result; `External` sets the untrusted flag.
    pub fn record_tool_result_trust(
        &mut self,
        id: &TaskId,
        trust: Trust,
        now: Timestamp,
    ) -> Result<bool> {
        if trust == Trust::External {
            self.mark_task_untrusted(id, now)
        } else {
            Ok(false)
        }
    }

    /// Write-ahead: records a step as `started` *before* it is executed.
    /// The task must be `running`.
    pub fn begin_step(
        &mut self,
        task_id: &TaskId,
        tool: &str,
        args_redacted: Option<&str>,
        side_effect: SideEffect,
        idempotency_key: Option<&str>,
        now: Timestamp,
    ) -> Result<TaskStep> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let task = load_task(&tx, task_id)?;
        if task.status != TaskStatus::Running {
            return Err(ElfError::invalid_input(format!(
                "cannot start a step while task is {}",
                task.status
            )));
        }
        let seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM task_steps WHERE task_id = ?1",
            [task_id.as_str()],
            |r| r.get(0),
        )?;
        let id = StepId::new();
        tx.execute(
            "INSERT INTO task_steps (id, task_id, seq, tool, args_redacted, side_effect, status, idempotency_key, started_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'started', ?7, ?8)",
            params![
                id.as_str(),
                task_id.as_str(),
                seq,
                tool,
                args_redacted,
                side_effect.as_str(),
                idempotency_key,
                ts_to_sql(&now)
            ],
        )
        .map_err(|e| match e {
            rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation => {
                ElfError::invalid_input("duplicate idempotency key for this task")
            }
            other => other.into(),
        })?;
        tx.commit()?;
        self.get_step(&id)
    }

    /// Completes a `started` step with its outcome and optional evidence.
    pub fn finish_step(
        &mut self,
        step_id: &StepId,
        outcome: StepOutcome,
        evidence: Option<&EvidenceRef>,
        now: Timestamp,
    ) -> Result<TaskStep> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let step = tx
            .query_row(
                &format!("SELECT {STEP_COLS} FROM task_steps WHERE id = ?1"),
                [step_id.as_str()],
                step_from_row,
            )
            .optional()?
            .ok_or_else(|| ElfError::invalid_input(format!("unknown step {step_id}")))?;
        if step.status != StepStatus::Started {
            return Err(ElfError::InvalidTransition {
                from: step.status.to_string(),
                to: outcome.status().to_string(),
                reason: Some("step already finished".into()),
            });
        }
        tx.execute(
            "UPDATE task_steps SET status = ?2, finished_at = ?3, evidence_ref = ?4 WHERE id = ?1",
            params![
                step_id.as_str(),
                outcome.status().as_str(),
                ts_to_sql(&now),
                evidence.map(EvidenceRef::to_ref_string)
            ],
        )?;
        tx.commit()?;
        self.get_step(step_id)
    }

    /// Loads a step.
    pub fn get_step(&self, id: &StepId) -> Result<TaskStep> {
        self.conn
            .query_row(
                &format!("SELECT {STEP_COLS} FROM task_steps WHERE id = ?1"),
                [id.as_str()],
                step_from_row,
            )
            .optional()?
            .ok_or_else(|| ElfError::invalid_input(format!("unknown step {id}")))
    }

    /// Steps of a task in order.
    pub fn list_steps(&self, task_id: &TaskId) -> Result<Vec<TaskStep>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {STEP_COLS} FROM task_steps WHERE task_id = ?1 ORDER BY seq"
        ))?;
        let rows = stmt.query_map([task_id.as_str()], step_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Finds a step by its idempotency key (used to avoid re-running external writes).
    pub fn find_step_by_idempotency_key(
        &self,
        task_id: &TaskId,
        key: &str,
    ) -> Result<Option<TaskStep>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {STEP_COLS} FROM task_steps WHERE task_id = ?1 AND idempotency_key = ?2"),
                params![task_id.as_str(), key],
                step_from_row,
            )
            .optional()?)
    }

    /// Restart recovery (handbook 7.5 / 9). For every `running` task:
    /// steps still `started` with `external_write` become `uncertain` and the task
    /// goes to `uncertain`; otherwise leftover `started` steps become `interrupted`
    /// and the task goes to `paused`. One transaction; nothing is ever re-run.
    pub fn recover_after_restart(&mut self, clock: &dyn Clock) -> Result<Vec<RecoveredTask>> {
        let now = clock.now_utc();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ids: Vec<TaskId> = {
            let mut stmt = tx.prepare(
                "SELECT id FROM tasks WHERE status = 'running' ORDER BY created_at, rowid",
            )?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.map(|r| r.map(TaskId::from_string))
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut out = Vec::new();
        for id in ids {
            let uncertain: Vec<StepId> = {
                let mut stmt = tx.prepare(
                    "SELECT id FROM task_steps WHERE task_id = ?1 AND status = 'started' AND side_effect = 'external_write' ORDER BY seq",
                )?;
                let rows = stmt.query_map([id.as_str()], |r| r.get::<_, String>(0))?;
                rows.map(|r| r.map(StepId::from_string))
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };
            tx.execute(
                "UPDATE task_steps SET status = 'uncertain' WHERE task_id = ?1 AND status = 'started' AND side_effect = 'external_write'",
                [id.as_str()],
            )?;
            let interrupted = tx.execute(
                "UPDATE task_steps SET status = 'interrupted' WHERE task_id = ?1 AND status = 'started'",
                [id.as_str()],
            )?;
            let to = if uncertain.is_empty() {
                TaskStatus::Paused
            } else {
                TaskStatus::Uncertain
            };
            let extra = json!({
                "uncertain_steps": uncertain.iter().map(StepId::as_str).collect::<Vec<_>>(),
                "interrupted_steps": interrupted,
            });
            apply_transition(&tx, &id, to, &[], Some("restart_recovery"), extra, now)?;
            out.push(RecoveredTask {
                task_id: id,
                from: TaskStatus::Running,
                to,
                uncertain_steps: uncertain,
            });
        }
        tx.commit()?;
        Ok(out)
    }
}
