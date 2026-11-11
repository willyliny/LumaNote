//! Approval requests: a tool call that policy could not allow waits here for the user.

use std::fmt;

use rusqlite::{params, OptionalExtension, Row, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::grants::NewGrant;
use super::{conv, get_json, get_opt_ts, get_ts, to_json, Store};
use crate::schema::{
    str_enum, ts_to_sql, ApprovalId, ElfError, GrantId, Result, TaskId, Timestamp, ToolCallId,
};

str_enum!(
    /// State of an approval request.
    ApprovalStatus {
        /// Waiting for the user.
        Pending => "pending",
        /// The user said yes; a one-shot grant was created.
        Approved => "approved",
        /// The user said no.
        Denied => "denied",
        /// Withdrawn without an answer (task cancelled).
        Expired => "expired",
    }
);

/// A stored approval request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Approval {
    /// Identifier.
    pub id: ApprovalId,
    /// Task that is waiting.
    pub task_id: TaskId,
    /// The tool call that needs approval.
    pub tool_call_id: ToolCallId,
    /// Tool name.
    pub tool: String,
    /// Policy action name (`write`, `send`, ...).
    pub action: String,
    /// Normalised target the user is asked about.
    pub resource: String,
    /// The exact arguments the approval covers.
    pub arguments: Value,
    /// Hash of those arguments; the one-shot grant is bound to it.
    pub arguments_hash: String,
    /// Concrete, human-readable description of what will happen.
    pub preview: String,
    /// Why approval is needed (`no_matching_grant` / `untrusted_content`).
    pub reason: String,
    /// Current state.
    pub status: ApprovalStatus,
    /// When the request was created.
    pub created_at: Timestamp,
    /// When the user answered.
    pub responded_at: Option<Timestamp>,
    /// The one-shot grant created by an approval.
    pub grant_id: Option<GrantId>,
}

/// Parameters for a new approval request.
#[derive(Debug, Clone, PartialEq)]
pub struct NewApproval {
    /// Waiting task.
    pub task_id: TaskId,
    /// Tool call that needs approval.
    pub tool_call_id: ToolCallId,
    /// Tool name.
    pub tool: String,
    /// Policy action name.
    pub action: String,
    /// Normalised target.
    pub resource: String,
    /// Exact arguments.
    pub arguments: Value,
    /// Hash of the arguments.
    pub arguments_hash: String,
    /// Concrete preview.
    pub preview: String,
    /// Reason code.
    pub reason: String,
}

const APPROVAL_COLS: &str =
    "id, task_id, tool_call_id, tool, action, resource, arguments_json, arguments_hash, \
     preview, reason, status, created_at, responded_at, grant_id";

fn approval_from_row(r: &Row<'_>) -> rusqlite::Result<Approval> {
    let status: String = r.get(10)?;
    Ok(Approval {
        id: ApprovalId::from_string(r.get::<_, String>(0)?),
        task_id: TaskId::from_string(r.get::<_, String>(1)?),
        tool_call_id: ToolCallId::from_string(r.get::<_, String>(2)?),
        tool: r.get(3)?,
        action: r.get(4)?,
        resource: r.get(5)?,
        arguments: get_json(r, 6)?,
        arguments_hash: r.get(7)?,
        preview: r.get(8)?,
        reason: r.get(9)?,
        status: conv(10, ApprovalStatus::parse(&status))?,
        created_at: get_ts(r, 11)?,
        responded_at: get_opt_ts(r, 12)?,
        grant_id: r.get::<_, Option<String>>(13)?.map(GrantId::from_string),
    })
}

impl Store {
    /// Records a pending approval request (one per task and tool call).
    pub fn create_approval(&mut self, new: &NewApproval, now: Timestamp) -> Result<Approval> {
        let id = ApprovalId::new();
        self.conn.execute(
            "INSERT INTO approvals (id, task_id, tool_call_id, tool, action, resource, arguments_json,
                                    arguments_hash, preview, reason, status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'pending', ?11)",
            params![
                id.as_str(),
                new.task_id.as_str(),
                new.tool_call_id.as_str(),
                new.tool,
                new.action,
                new.resource,
                to_json(&new.arguments)?,
                new.arguments_hash,
                new.preview,
                new.reason,
                ts_to_sql(&now)
            ],
        )?;
        self.get_approval(&id)
    }

    /// Loads an approval.
    pub fn get_approval(&self, id: &ApprovalId) -> Result<Approval> {
        self.conn
            .query_row(
                &format!("SELECT {APPROVAL_COLS} FROM approvals WHERE id = ?1"),
                [id.as_str()],
                approval_from_row,
            )
            .optional()?
            .ok_or_else(|| ElfError::invalid_input(format!("unknown approval {id}")))
    }

    /// The approval for one tool call of a task, if any.
    pub fn find_approval(
        &self,
        task_id: &TaskId,
        tool_call_id: &ToolCallId,
    ) -> Result<Option<Approval>> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {APPROVAL_COLS} FROM approvals WHERE task_id = ?1 AND tool_call_id = ?2"
                ),
                params![task_id.as_str(), tool_call_id.as_str()],
                approval_from_row,
            )
            .optional()?)
    }

    /// All approvals of a task in creation order.
    pub fn list_approvals(&self, task_id: &TaskId) -> Result<Vec<Approval>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {APPROVAL_COLS} FROM approvals WHERE task_id = ?1 ORDER BY rowid"
        ))?;
        let rows = stmt.query_map([task_id.as_str()], approval_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Answers a pending approval. `grant` (for an approval) is stored in the same
    /// transaction. Answering twice is an error: the first answer stands.
    pub fn resolve_approval(
        &mut self,
        id: &ApprovalId,
        grant: Option<&NewGrant>,
        now: Timestamp,
    ) -> Result<Approval> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (current, approval_task): (String, String) = tx
            .query_row(
                "SELECT status, task_id FROM approvals WHERE id = ?1",
                [id.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| ElfError::invalid_input(format!("unknown approval {id}")))?;
        if current != ApprovalStatus::Pending.as_str() {
            return Err(ElfError::invalid_input(format!(
                "approval {id} is already {current}"
            )));
        }
        let (status, grant_id) = match grant {
            Some(g) => {
                // An approval answers one task's request: its grant is that task's, never
                // a standing grant for everyone.
                if g.scope != super::grants::GrantScope::Task
                    || g.source_task.as_ref().map(TaskId::as_str) != Some(approval_task.as_str())
                {
                    return Err(ElfError::invalid_input(
                        "an approval can only create a grant scoped to its own task",
                    ));
                }
                let gid = super::grants::insert_grant_row(&tx, g, now)?;
                (ApprovalStatus::Approved, Some(gid))
            }
            None => (ApprovalStatus::Denied, None),
        };
        tx.execute(
            "UPDATE approvals SET status = ?2, responded_at = ?3, grant_id = ?4 WHERE id = ?1",
            params![
                id.as_str(),
                status.as_str(),
                ts_to_sql(&now),
                grant_id.as_ref().map(GrantId::as_str)
            ],
        )?;
        tx.commit()?;
        self.get_approval(id)
    }

    /// Withdraws every pending approval of a task (it was cancelled). Returns how many.
    pub fn expire_pending_approvals(&mut self, task_id: &TaskId, now: Timestamp) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE approvals SET status = 'expired', responded_at = ?2 WHERE task_id = ?1 AND status = 'pending'",
            params![task_id.as_str(), ts_to_sql(&now)],
        )?)
    }
}
