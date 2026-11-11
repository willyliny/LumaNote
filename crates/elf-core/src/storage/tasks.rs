//! Tasks, task steps (write-ahead rule), task events and restart recovery.

use rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{conv, get_json, get_opt_ts, get_ts, to_json, Store};
use crate::ports::Clock;
use crate::schema::{
    ts_to_sql, ConversationId, ElfError, EvidenceRef, GrantId, Result, StepId, TaskId, Timestamp,
    Trust,
};
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
    /// Task this one follows up on (completed tasks are never reopened).
    pub parent_task_id: Option<TaskId>,
    /// Raw limits (`max_tool_calls`, `max_duration_secs`); see [`crate::task::TaskLimits`].
    pub limits: Value,
    /// Tool calls that produced a result so far (all budget windows together).
    pub tool_call_count: u32,
    /// Start of the current budget window (first run, or the latest resume).
    pub started_at: Option<Timestamp>,
    /// Conversation holding the task's messages.
    pub conversation_id: Option<ConversationId>,
    /// `tool_call_count` when the current budget window began.
    pub tool_call_base: u32,
}

/// Parameters for [`Store::create_task_with`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NewTask {
    /// What the user asked for.
    pub intent: String,
    /// Scope declared at creation.
    pub scope: Value,
    /// Limits override (`{}` means the defaults of 20 tool calls and 10 minutes).
    pub limits: Value,
    /// Follow-up relationship.
    pub parent_task_id: Option<TaskId>,
    /// Conversation holding the task's messages.
    pub conversation_id: Option<ConversationId>,
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
    /// The saved tool result (what the model was told); reused instead of re-running.
    pub result: Option<Value>,
    /// The grant whose use was consumed to authorise this step, recorded in the same
    /// transaction as the step itself (see [`crate::policy::authorize_and_begin_step`]).
    pub consumed_grant_id: Option<GrantId>,
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
    "id, intent, status, scope_json, created_at, updated_at, result_ref, untrusted_content, \
     parent_task_id, limits_json, tool_call_count, started_at, conversation_id, tool_call_base";
const STEP_COLS: &str = "id, task_id, seq, tool, args_redacted, side_effect, status, idempotency_key, started_at, finished_at, evidence_ref, result_json, consumed_grant_id";

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
        parent_task_id: r.get::<_, Option<String>>(8)?.map(TaskId::from_string),
        limits: get_json(r, 9)?,
        tool_call_count: u32::try_from(r.get::<_, i64>(10)?).unwrap_or(u32::MAX),
        started_at: get_opt_ts(r, 11)?,
        conversation_id: r
            .get::<_, Option<String>>(12)?
            .map(ConversationId::from_string),
        tool_call_base: u32::try_from(r.get::<_, i64>(13)?).unwrap_or(u32::MAX),
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
        result: r
            .get::<_, Option<String>>(11)?
            .map(|s| {
                conv(
                    11,
                    serde_json::from_str(&s).map_err(|e| ElfError::Storage {
                        message: format!("bad stored step result: {e}"),
                    }),
                )
            })
            .transpose()?,
        consumed_grant_id: r.get::<_, Option<String>>(12)?.map(GrantId::from_string),
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
    // The first entry into `running` opens the first budget window.
    let started = (to == TaskStatus::Running && task.started_at.is_none()).then(|| ts_to_sql(&now));
    conn.execute(
        "UPDATE tasks SET status = ?2, updated_at = ?3, result_ref = COALESCE(?4, result_ref),
             started_at = COALESCE(?5, started_at) WHERE id = ?1",
        params![
            id.as_str(),
            to.as_str(),
            ts_to_sql(&now),
            result_ref,
            started
        ],
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
    // A finished task keeps no live grants: its task-scoped grants (the implied ones and
    // every one-shot approval) end in the same transaction as the transition, so a crash
    // cannot leave them behind. An uncertain task cannot run again either.
    if to.is_terminal() || to == TaskStatus::Uncertain {
        super::grants::revoke_task_scoped(conn, id, now)?;
        // Tool calls that never got a result belong to this task only: answer them "not
        // executed" now, so no later task of the conversation can pick them up.
        let text = match to {
            TaskStatus::Cancelled => "not executed: the task was cancelled",
            TaskStatus::Failed => "not executed: the task failed",
            TaskStatus::Uncertain => {
                "not executed: the task ended with an uncertain outcome; nothing was repeated"
            }
            _ => "not executed: the task had already finished",
        };
        super::content::close_unanswered_calls_in(conn, id, text, now)?;
    }
    insert_event(conn, id, "status_changed", &payload, now)
}

/// Write-ahead insert of a `started` step on `conn` (normally inside a transaction).
/// The task must be `running`.
pub(crate) fn begin_step_in(
    conn: &Connection,
    task_id: &TaskId,
    tool: &str,
    args_redacted: Option<&str>,
    side_effect: SideEffect,
    idempotency_key: Option<&str>,
    now: Timestamp,
) -> Result<StepId> {
    let task = load_task(conn, task_id)?;
    if task.status != TaskStatus::Running {
        return Err(ElfError::invalid_input(format!(
            "cannot start a step while task is {}",
            task.status
        )));
    }
    let seq: i64 = conn.query_row(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM task_steps WHERE task_id = ?1",
        [task_id.as_str()],
        |r| r.get(0),
    )?;
    let id = StepId::new();
    conn.execute(
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
        rusqlite::Error::SqliteFailure(f, _)
            if f.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            ElfError::invalid_input("duplicate idempotency key for this task")
        }
        other => other.into(),
    })?;
    Ok(id)
}

/// Re-opens a failed or interrupted non-external step on `conn` (see [`Store::reopen_step`]).
pub(crate) fn reopen_step_in(conn: &Connection, step_id: &StepId, now: Timestamp) -> Result<()> {
    let step = conn
        .query_row(
            &format!("SELECT {STEP_COLS} FROM task_steps WHERE id = ?1"),
            [step_id.as_str()],
            step_from_row,
        )
        .optional()?
        .ok_or_else(|| ElfError::invalid_input(format!("unknown step {step_id}")))?;
    let allowed = matches!(step.status, StepStatus::Failed | StepStatus::Interrupted)
        && step.side_effect != SideEffect::ExternalWrite;
    if !allowed {
        return Err(ElfError::InvalidTransition {
            from: step.status.to_string(),
            to: StepStatus::Started.to_string(),
            reason: Some("only failed or interrupted non-external steps can be re-run".into()),
        });
    }
    conn.execute(
        "UPDATE task_steps SET status = 'started', started_at = ?2, finished_at = NULL,
             evidence_ref = NULL, result_json = NULL, consumed_grant_id = NULL WHERE id = ?1",
        params![step_id.as_str(), ts_to_sql(&now)],
    )?;
    Ok(())
}

/// Records which grant use authorised a step.
pub(crate) fn set_step_consumed_grant(
    conn: &Connection,
    step_id: &StepId,
    grant: &GrantId,
) -> Result<()> {
    conn.execute(
        "UPDATE task_steps SET consumed_grant_id = ?2 WHERE id = ?1",
        params![step_id.as_str(), grant.as_str()],
    )?;
    Ok(())
}

/// Sets the task's untrusted flag on `conn` (inside the caller's transaction) and records a
/// `marked_untrusted` event the first time. Returns whether it newly changed.
pub(crate) fn taint_task_in(conn: &Connection, id: &TaskId, now: Timestamp) -> Result<bool> {
    let task = load_task(conn, id)?;
    if task.untrusted_content {
        return Ok(false);
    }
    conn.execute(
        "UPDATE tasks SET untrusted_content = 1, updated_at = ?2 WHERE id = ?1",
        params![id.as_str(), ts_to_sql(&now)],
    )?;
    insert_event(conn, id, "marked_untrusted", &json!({}), now)?;
    Ok(true)
}

impl Store {
    /// Creates a queued task and its first event (`created`, sequence 1).
    pub fn create_task(&mut self, intent: &str, scope: &Value, now: Timestamp) -> Result<Task> {
        self.create_task_with(
            &NewTask {
                intent: intent.to_owned(),
                scope: scope.clone(),
                limits: json!({}),
                ..NewTask::default()
            },
            now,
        )
    }

    /// Like [`Store::create_task`] with limits, a parent task and a conversation.
    pub fn create_task_with(&mut self, new: &NewTask, now: Timestamp) -> Result<Task> {
        let id = TaskId::new();
        let ts = ts_to_sql(&now);
        let limits = if new.limits.is_null() {
            json!({})
        } else {
            new.limits.clone()
        };
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(parent) = &new.parent_task_id {
            load_task(&tx, parent)?;
        }
        // The untrusted flag belongs to the conversation: a task that continues a
        // conversation whose context holds content the user did not type starts flagged.
        let inherited = match &new.conversation_id {
            Some(c) => super::content::conversation_tainted_in(&tx, c)?,
            None => false,
        };
        tx.execute(
            "INSERT INTO tasks (id, intent, status, scope_json, created_at, updated_at,
                                limits_json, parent_task_id, conversation_id, untrusted_content)
             VALUES (?1, ?2, 'queued', ?3, ?4, ?4, ?5, ?6, ?7, ?8)",
            params![
                id.as_str(),
                new.intent,
                to_json(&new.scope)?,
                ts,
                to_json(&limits)?,
                new.parent_task_id.as_ref().map(TaskId::as_str),
                new.conversation_id.as_ref().map(ConversationId::as_str),
                i64::from(inherited)
            ],
        )?;
        insert_event(&tx, &id, "created", &json!({"status": "queued"}), now)?;
        if inherited {
            insert_event(
                &tx,
                &id,
                "marked_untrusted",
                &json!({"inherited_from_conversation": true}),
                now,
            )?;
        }
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
        self.transition_task_with(id, to, evidence, reason, json!({}), clock)
    }

    /// Like [`Store::transition_task`], adding the fields of the JSON object `extra` to the
    /// `status_changed` event payload (for example the list of unresolved items).
    pub fn transition_task_with(
        &mut self,
        id: &TaskId,
        to: TaskStatus,
        evidence: &[EvidenceRef],
        reason: Option<&str>,
        extra: Value,
        clock: &dyn Clock,
    ) -> Result<TaskEvent> {
        let now = clock.now_utc();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        match apply_transition(&tx, id, to, evidence, reason, extra, now) {
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
        let changed = taint_task_in(&tx, id, now)?;
        tx.commit()?;
        Ok(changed)
    }

    /// Notes the trust level of a tool result: anything the user did not type themselves
    /// (`local_granted` as well as `external`; handbook 8.1) sets the untrusted flag.
    pub fn record_tool_result_trust(
        &mut self,
        id: &TaskId,
        trust: Trust,
        now: Timestamp,
    ) -> Result<bool> {
        if trust == Trust::UserProvided {
            Ok(false)
        } else {
            self.mark_task_untrusted(id, now)
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
        let id = begin_step_in(
            &tx,
            task_id,
            tool,
            args_redacted,
            side_effect,
            idempotency_key,
            now,
        )?;
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
        self.finish_step_with_result(step_id, outcome, evidence, None, now)
    }

    /// Like [`Store::finish_step`], also saving the tool result in the same transaction
    /// so a later resume can return it without executing the step again.
    pub fn finish_step_with_result(
        &mut self,
        step_id: &StepId,
        outcome: StepOutcome,
        evidence: Option<&EvidenceRef>,
        result: Option<&Value>,
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
            "UPDATE task_steps SET status = ?2, finished_at = ?3, evidence_ref = ?4, result_json = ?5 WHERE id = ?1",
            params![
                step_id.as_str(),
                outcome.status().as_str(),
                ts_to_sql(&now),
                evidence.map(EvidenceRef::to_ref_string),
                result.map(to_json).transpose()?
            ],
        )?;
        // A saved result whose content the user did not type taints the task in the same
        // transaction that saves it (handbook 8.1): there is no window in which the result
        // exists, and may be replayed into the context, while the flag is not yet set.
        let content_is_untrusted = result
            .and_then(|v| v.get("trust"))
            .and_then(Value::as_str)
            .is_some_and(|t| t != Trust::UserProvided.as_str());
        if content_is_untrusted {
            taint_task_in(&tx, &step.task_id, now)?;
        }
        tx.commit()?;
        self.get_step(step_id)
    }

    /// Re-opens a `failed` or `interrupted` step that is not an external write so the
    /// same idempotency key can be executed again (write-ahead: back to `started`).
    /// Succeeded steps are never re-opened and external writes are never re-run.
    pub fn reopen_step(&mut self, step_id: &StepId, now: Timestamp) -> Result<TaskStep> {
        reopen_step_in(&self.conn, step_id, now)?;
        self.get_step(step_id)
    }

    /// Appends a custom event (e.g. `tool_started`) to the task's event log and
    /// returns it; its `sequence` continues the per-task numbering.
    pub fn append_task_event(
        &mut self,
        task_id: &TaskId,
        event_type: &str,
        payload: &Value,
        now: Timestamp,
    ) -> Result<TaskEvent> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        load_task(&tx, task_id)?;
        let ev = insert_event(&tx, task_id, event_type, payload, now)?;
        tx.commit()?;
        Ok(ev)
    }

    /// Counts one tool call that produced a result; returns the new total.
    pub fn increment_tool_calls(&mut self, id: &TaskId, now: Timestamp) -> Result<u32> {
        let n = self.conn.execute(
            "UPDATE tasks SET tool_call_count = tool_call_count + 1, updated_at = ?2 WHERE id = ?1",
            params![id.as_str(), ts_to_sql(&now)],
        )?;
        if n == 0 {
            return Err(ElfError::invalid_input(format!("unknown task {id}")));
        }
        Ok(self.get_task(id)?.tool_call_count)
    }

    /// Opens a fresh budget window: the time limit counts from `now` and the tool-call
    /// limit counts from the calls made so far. Used when a paused task is resumed.
    pub fn begin_budget_window(&mut self, id: &TaskId, now: Timestamp) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE tasks SET started_at = ?2, tool_call_base = tool_call_count WHERE id = ?1",
            params![id.as_str(), ts_to_sql(&now)],
        )?;
        if n == 0 {
            return Err(ElfError::invalid_input(format!("unknown task {id}")));
        }
        Ok(())
    }

    /// Moves the budget window start forward, so time spent waiting for the user does
    /// not count against the task's time limit.
    pub fn shift_budget_window(&mut self, id: &TaskId, by: std::time::Duration) -> Result<()> {
        let task = self.get_task(id)?;
        let (Some(start), Ok(by)) = (task.started_at, chrono::Duration::from_std(by)) else {
            return Ok(());
        };
        let shifted = start.checked_add_signed(by).unwrap_or(start);
        self.conn.execute(
            "UPDATE tasks SET started_at = ?2 WHERE id = ?1",
            params![id.as_str(), ts_to_sql(&shifted)],
        )?;
        Ok(())
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
            // A non-external step cut off mid-run may be re-run after resume, and the re-run
            // is authorised again. Give back the use its first attempt consumed so a
            // one-shot approval is not spent twice for one operation. External steps keep
            // theirs: they are never re-run.
            tx.execute(
                "UPDATE grants SET used_count = used_count - 1
                 WHERE used_count > 0 AND id IN (
                     SELECT consumed_grant_id FROM task_steps
                     WHERE task_id = ?1 AND status = 'started' AND side_effect != 'external_write'
                       AND consumed_grant_id IS NOT NULL)",
                [id.as_str()],
            )?;
            tx.execute(
                "UPDATE task_steps SET consumed_grant_id = NULL
                 WHERE task_id = ?1 AND status = 'started' AND side_effect != 'external_write'",
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
        // Crash repair: no finished task may keep live task-scoped grants.
        super::grants::revoke_task_scoped_of_finished_tasks(&tx, now)?;
        tx.commit()?;
        Ok(out)
    }
}
