//! Conversations, messages and artifacts.

use std::collections::HashMap;
use std::fmt;

use rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{conv, get_opt_ts, get_ts, to_json, Store};
use crate::model::{MessagePart, Role};
use crate::schema::{
    str_enum, ts_to_sql, ArtifactId, ConversationId, LumaError, MessageId, Result, TaskId,
    Timestamp, ToolCallId, Trust,
};

/// A conversation row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conversation {
    /// Identifier.
    pub id: ConversationId,
    /// Title shown in the UI.
    pub title: String,
    /// Creation time.
    pub created_at: Timestamp,
    /// Last activity time.
    pub updated_at: Timestamp,
    /// Archive time, if archived.
    pub archived_at: Option<Timestamp>,
}

/// A stored message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredMessage {
    /// Identifier.
    pub id: MessageId,
    /// Owning conversation.
    pub conversation_id: ConversationId,
    /// Task this message belongs to, if any.
    pub task_id: Option<TaskId>,
    /// Author.
    pub role: Role,
    /// Content parts.
    pub parts: Vec<MessagePart>,
    /// Tool call this message answers (tool role), if any.
    pub tool_call_id: Option<ToolCallId>,
    /// How far the content can be trusted. Anything but `user_provided` taints the
    /// conversation: see [`Store::conversation_tainted`].
    pub trust: Trust,
    /// True if the tool-call arguments in `parts` are redacted (the complete arguments of
    /// an unanswered call are in `pending_calls`). False on messages stored before
    /// migration 0004, whose arguments are complete.
    pub args_redacted: bool,
    /// Creation time.
    pub created_at: Timestamp,
}

/// A produced file or document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    /// Identifier.
    pub id: ArtifactId,
    /// Producing task.
    pub task_id: TaskId,
    /// Kind, e.g. `markdown`.
    pub kind: String,
    /// File path.
    pub path: String,
    /// SHA-256 hex of the content.
    pub content_hash: String,
    /// Size in bytes.
    pub bytes: u64,
    /// Creation time.
    pub created_at: Timestamp,
}

fn conversation_from_row(r: &Row<'_>) -> rusqlite::Result<Conversation> {
    Ok(Conversation {
        id: ConversationId::from_string(r.get::<_, String>(0)?),
        title: r.get(1)?,
        created_at: get_ts(r, 2)?,
        updated_at: get_ts(r, 3)?,
        archived_at: get_opt_ts(r, 4)?,
    })
}

fn message_from_row(r: &Row<'_>) -> rusqlite::Result<StoredMessage> {
    let role: String = r.get(3)?;
    let content: String = r.get(4)?;
    Ok(StoredMessage {
        id: MessageId::from_string(r.get::<_, String>(0)?),
        conversation_id: ConversationId::from_string(r.get::<_, String>(1)?),
        task_id: r.get::<_, Option<String>>(2)?.map(TaskId::from_string),
        role: conv(3, Role::parse(&role))?,
        parts: conv(
            4,
            serde_json::from_str(&content).map_err(|e| LumaError::Storage {
                message: format!("bad message content: {e}"),
            }),
        )?,
        tool_call_id: r.get::<_, Option<String>>(5)?.map(ToolCallId::from_string),
        created_at: get_ts(r, 6)?,
        trust: conv(7, Trust::parse(&r.get::<_, String>(7)?))?,
        args_redacted: r.get::<_, i64>(8)? != 0,
    })
}

const MESSAGE_COLS: &str =
    "id, conversation_id, task_id, role, content_json, tool_call_id, created_at, trust, args_redacted";

str_enum!(
    /// How a tool call of a task ended (see [`Store::list_call_outcomes`]).
    CallOutcomeKind {
        /// The tool ran and succeeded.
        Succeeded => "succeeded",
        /// Policy refused it or the user said no.
        Denied => "denied",
        /// The approval it waited for expired.
        Expired => "expired",
        /// The tool ran (or was about to) and failed.
        Failed => "failed",
        /// An external write whose outcome is unknown.
        Uncertain => "uncertain",
        /// The call was malformed or unknown and never reached policy.
        Rejected => "rejected",
        /// The call was answered "not executed" when its task ended.
        NotExecuted => "not_executed",
    }
);

/// One recorded call outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallOutcome {
    /// Insertion order (later outcomes have larger ids).
    pub id: i64,
    /// Task.
    pub task_id: TaskId,
    /// The model's call.
    pub tool_call_id: ToolCallId,
    /// Tool name.
    pub tool: String,
    /// Target, when the tool could name one.
    pub resource: Option<String>,
    /// How it ended.
    pub outcome: CallOutcomeKind,
    /// The call was an attempt to write (any side effect).
    pub is_write: bool,
    /// Short explanation shown to the user.
    pub detail: Option<String>,
    /// When.
    pub created_at: Timestamp,
}

/// A message to insert.
#[derive(Debug, Clone)]
pub struct NewMessage<'a> {
    /// Conversation.
    pub conversation_id: &'a ConversationId,
    /// Owning task.
    pub task_id: Option<&'a TaskId>,
    /// Author.
    pub role: Role,
    /// Content.
    pub parts: &'a [MessagePart],
    /// Tool call answered by this message.
    pub tool_call_id: Option<&'a ToolCallId>,
    /// Trust of the content.
    pub trust: Trust,
    /// Whether tool-call arguments in `parts` are redacted.
    pub args_redacted: bool,
}

/// A tool call of an assistant turn together with its complete arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct CallToRun {
    /// Call id.
    pub id: ToolCallId,
    /// Tool name.
    pub name: String,
    /// Complete arguments (an empty object for a refused call).
    pub arguments: Value,
    /// If set, the call must not run and is answered as an error with this text.
    pub refusal: Option<String>,
}

/// The result of a tool call, stored together with everything that must change with it.
#[derive(Debug, Clone)]
pub struct ToolResultRecord<'a> {
    /// Conversation.
    pub conversation_id: &'a ConversationId,
    /// Task.
    pub task_id: &'a TaskId,
    /// Call answered.
    pub call_id: &'a ToolCallId,
    /// Tool name.
    pub tool: &'a str,
    /// Text as the model sees it (already labelled by trust).
    pub content: &'a str,
    /// Whether the model is told it failed.
    pub is_error: bool,
    /// Trust of the content; anything but `user_provided` taints the task and the
    /// conversation in this very transaction.
    pub trust: Trust,
    /// The call was an attempt to write.
    pub is_write: bool,
    /// How it ended.
    pub outcome: CallOutcomeKind,
    /// Target, if known.
    pub resource: Option<&'a str>,
    /// Short explanation.
    pub detail: Option<&'a str>,
}

/// A tool call that has no result yet.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenCall {
    /// Task the call belongs to (`None` if that task row was deleted).
    pub task_id: Option<TaskId>,
    /// Conversation holding it.
    pub conversation_id: ConversationId,
    /// Call id.
    pub call_id: ToolCallId,
    /// Tool name.
    pub name: String,
    /// Arguments as stored in the message (redacted unless `redacted` is false).
    pub arguments: Value,
    /// Whether `arguments` is redacted.
    pub redacted: bool,
}

/// Tool calls in `messages` that no later message answers. A call is identified by
/// (task, call id): the same provider id used by two tasks, or reused later by the same
/// task, never answers another call. Pure function over stored history.
pub fn open_calls(messages: &[StoredMessage]) -> Vec<OpenCall> {
    let mut open: Vec<Option<OpenCall>> = Vec::new();
    let mut index: HashMap<(Option<TaskId>, ToolCallId), usize> = HashMap::new();
    for m in messages {
        for p in &m.parts {
            match p {
                MessagePart::ToolCall {
                    id,
                    name,
                    arguments,
                } if m.role == Role::Assistant => {
                    index.insert((m.task_id.clone(), id.clone()), open.len());
                    open.push(Some(OpenCall {
                        task_id: m.task_id.clone(),
                        conversation_id: m.conversation_id.clone(),
                        call_id: id.clone(),
                        name: name.clone(),
                        arguments: arguments.clone(),
                        redacted: m.args_redacted,
                    }));
                }
                MessagePart::ToolResult { call_id, .. } => {
                    if let Some(i) = index.remove(&(m.task_id.clone(), call_id.clone())) {
                        open[i] = None;
                    }
                }
                _ => {}
            }
        }
    }
    open.into_iter().flatten().collect()
}

fn list_messages_where(conn: &Connection, clause: &str, arg: &str) -> Result<Vec<StoredMessage>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {MESSAGE_COLS} FROM messages WHERE {clause} ORDER BY rowid"
    ))?;
    let rows = stmt.query_map([arg], message_from_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub(crate) fn insert_message_in(
    conn: &Connection,
    m: &NewMessage<'_>,
    now: Timestamp,
) -> Result<StoredMessage> {
    let id = MessageId::new();
    let ts = ts_to_sql(&now);
    conn.execute(
        "INSERT INTO messages (id, conversation_id, task_id, role, content_json, tool_call_id,
                               created_at, trust, args_redacted)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            id.as_str(),
            m.conversation_id.as_str(),
            m.task_id.map(TaskId::as_str),
            m.role.as_str(),
            to_json(&m.parts)?,
            m.tool_call_id.map(ToolCallId::as_str),
            ts,
            m.trust.as_str(),
            i64::from(m.args_redacted)
        ],
    )?;
    conn.execute(
        "UPDATE conversations SET updated_at = ?2 WHERE id = ?1",
        params![m.conversation_id.as_str(), ts],
    )?;
    Ok(StoredMessage {
        id,
        conversation_id: m.conversation_id.clone(),
        task_id: m.task_id.cloned(),
        role: m.role,
        parts: m.parts.to_vec(),
        tool_call_id: m.tool_call_id.cloned(),
        trust: m.trust,
        args_redacted: m.args_redacted,
        created_at: now,
    })
}

/// Whether any message of the conversation carries content that was not typed by the user.
pub(crate) fn conversation_tainted_in(conn: &Connection, id: &ConversationId) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM messages WHERE conversation_id = ?1 AND trust != 'user_provided' LIMIT 1",
            [id.as_str()],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Answers every unanswered tool call of `task` with an error result `text` ("not
/// executed ...") and drops its stored arguments, so no later task can pick the call up.
/// Runs inside the caller's transaction. Returns how many calls were answered.
pub(crate) fn close_unanswered_calls_in(
    conn: &Connection,
    task: &TaskId,
    text: &str,
    now: Timestamp,
) -> Result<usize> {
    let msgs = list_messages_where(conn, "task_id = ?1", task.as_str())?;
    let open: Vec<OpenCall> = open_calls(&msgs);
    close_calls_in(conn, &open, text, now)
}

fn close_calls_in(
    conn: &Connection,
    open: &[OpenCall],
    text: &str,
    now: Timestamp,
) -> Result<usize> {
    for c in open {
        let part = MessagePart::ToolResult {
            call_id: c.call_id.clone(),
            content: text.to_owned(),
            is_error: true,
        };
        insert_message_in(
            conn,
            &NewMessage {
                conversation_id: &c.conversation_id,
                task_id: c.task_id.as_ref(),
                role: Role::Tool,
                parts: &[part],
                tool_call_id: Some(&c.call_id),
                trust: Trust::UserProvided,
                args_redacted: false,
            },
            now,
        )?;
        if let Some(t) = &c.task_id {
            conn.execute(
                "DELETE FROM pending_calls WHERE task_id = ?1 AND tool_call_id = ?2",
                params![t.as_str(), c.call_id.as_str()],
            )?;
            conn.execute(
                "INSERT INTO call_outcomes (task_id, tool_call_id, tool, resource, outcome, is_write, detail, created_at)
                 VALUES (?1, ?2, ?3, NULL, 'not_executed', 0, ?4, ?5)",
                params![t.as_str(), c.call_id.as_str(), c.name, text, ts_to_sql(&now)],
            )?;
        }
    }
    Ok(open.len())
}

fn outcome_from_row(r: &Row<'_>) -> rusqlite::Result<CallOutcome> {
    Ok(CallOutcome {
        id: r.get(0)?,
        task_id: TaskId::from_string(r.get::<_, String>(1)?),
        tool_call_id: ToolCallId::from_string(r.get::<_, String>(2)?),
        tool: r.get(3)?,
        resource: r.get(4)?,
        outcome: conv(5, CallOutcomeKind::parse(&r.get::<_, String>(5)?))?,
        is_write: r.get::<_, i64>(6)? != 0,
        detail: r.get(7)?,
        created_at: get_ts(r, 8)?,
    })
}

fn artifact_from_row(r: &Row<'_>) -> rusqlite::Result<Artifact> {
    let bytes: i64 = r.get(5)?;
    Ok(Artifact {
        id: ArtifactId::from_string(r.get::<_, String>(0)?),
        task_id: TaskId::from_string(r.get::<_, String>(1)?),
        kind: r.get(2)?,
        path: r.get(3)?,
        content_hash: r.get(4)?,
        bytes: u64::try_from(bytes).unwrap_or(0),
        created_at: get_ts(r, 6)?,
    })
}

impl Store {
    /// Creates a conversation.
    pub fn create_conversation(&mut self, title: &str, now: Timestamp) -> Result<Conversation> {
        let id = ConversationId::new();
        let ts = ts_to_sql(&now);
        self.conn.execute(
            "INSERT INTO conversations (id, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
            params![id.as_str(), title, ts],
        )?;
        self.get_conversation(&id)
    }

    /// Loads a conversation.
    pub fn get_conversation(&self, id: &ConversationId) -> Result<Conversation> {
        self.conn
            .query_row(
                "SELECT id, title, created_at, updated_at, archived_at FROM conversations WHERE id = ?1",
                [id.as_str()],
                conversation_from_row,
            )
            .optional()?
            .ok_or_else(|| LumaError::invalid_input(format!("unknown conversation {id}")))
    }

    /// Appends a message and bumps the conversation's `updated_at`. The message is
    /// `user_provided` and its tool-call arguments (if any) are stored as given; see
    /// [`Store::append_message_with`] for the other cases.
    pub fn append_message(
        &mut self,
        conversation_id: &ConversationId,
        task_id: Option<&TaskId>,
        role: Role,
        parts: &[MessagePart],
        tool_call_id: Option<&ToolCallId>,
        now: Timestamp,
    ) -> Result<StoredMessage> {
        self.append_message_with(
            &NewMessage {
                conversation_id,
                task_id,
                role,
                parts,
                tool_call_id,
                trust: Trust::UserProvided,
                args_redacted: false,
            },
            now,
        )
    }

    /// Appends a message with an explicit trust level and redaction flag. A message that
    /// carries content not typed by the user (`trust` other than `user_provided`) also
    /// flags its task as untrusted, in the same transaction.
    pub fn append_message_with(
        &mut self,
        m: &NewMessage<'_>,
        now: Timestamp,
    ) -> Result<StoredMessage> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let msg = insert_message_in(&tx, m, now)?;
        if let (Some(task), true) = (m.task_id, m.trust != Trust::UserProvided) {
            super::tasks::taint_task_in(&tx, task, now)?;
        }
        tx.commit()?;
        Ok(msg)
    }

    /// Stores an assistant turn that contains tool calls: the message (with redacted
    /// arguments, `parts`) and the complete arguments of each executable call, in one
    /// transaction. The complete arguments are removed when the call's result is stored.
    pub fn append_assistant_turn(
        &mut self,
        conversation_id: &ConversationId,
        task_id: &TaskId,
        parts: &[MessagePart],
        calls: &[CallToRun],
        now: Timestamp,
    ) -> Result<StoredMessage> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let msg = insert_message_in(
            &tx,
            &NewMessage {
                conversation_id,
                task_id: Some(task_id),
                role: Role::Assistant,
                parts,
                tool_call_id: None,
                trust: Trust::UserProvided,
                args_redacted: true,
            },
            now,
        )?;
        for c in calls {
            tx.execute(
                "INSERT OR REPLACE INTO pending_calls (task_id, tool_call_id, name, arguments_json, refusal, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    task_id.as_str(),
                    c.id.as_str(),
                    c.name,
                    to_json(&c.arguments)?,
                    c.refusal,
                    ts_to_sql(&now)
                ],
            )?;
        }
        tx.commit()?;
        Ok(msg)
    }

    /// The complete arguments of an unanswered call of `task`, if they are still stored.
    pub fn get_pending_call(
        &self,
        task_id: &TaskId,
        call_id: &ToolCallId,
    ) -> Result<Option<CallToRun>> {
        let row: Option<(String, String, Option<String>)> = self
            .conn
            .query_row(
                "SELECT name, arguments_json, refusal FROM pending_calls WHERE task_id = ?1 AND tool_call_id = ?2",
                params![task_id.as_str(), call_id.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        row.map(|(name, json, refusal)| {
            Ok(CallToRun {
                id: call_id.clone(),
                refusal,
                name,
                arguments: serde_json::from_str(&json).map_err(|e| LumaError::Storage {
                    message: format!("bad stored call arguments: {e}"),
                })?,
            })
        })
        .transpose()
    }

    /// Stores the result of a tool call and everything that depends on it in ONE
    /// transaction: the tool message, removal of the call's stored arguments, the task's
    /// tool-call counter, the call outcome, and, if the content is not `user_provided`, the
    /// untrusted flag of the task. A crash can therefore never leave content in the model's
    /// context without the flag, and replaying a saved result sets the flag again.
    pub fn record_tool_result(&mut self, r: &ToolResultRecord<'_>, now: Timestamp) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let part = MessagePart::ToolResult {
            call_id: r.call_id.clone(),
            content: r.content.to_owned(),
            is_error: r.is_error,
        };
        insert_message_in(
            &tx,
            &NewMessage {
                conversation_id: r.conversation_id,
                task_id: Some(r.task_id),
                role: Role::Tool,
                parts: &[part],
                tool_call_id: Some(r.call_id),
                trust: r.trust,
                args_redacted: false,
            },
            now,
        )?;
        tx.execute(
            "DELETE FROM pending_calls WHERE task_id = ?1 AND tool_call_id = ?2",
            params![r.task_id.as_str(), r.call_id.as_str()],
        )?;
        let n = tx.execute(
            "UPDATE tasks SET tool_call_count = tool_call_count + 1, updated_at = ?2 WHERE id = ?1",
            params![r.task_id.as_str(), ts_to_sql(&now)],
        )?;
        if n == 0 {
            return Err(LumaError::invalid_input(format!(
                "unknown task {}",
                r.task_id
            )));
        }
        if r.trust != Trust::UserProvided {
            super::tasks::taint_task_in(&tx, r.task_id, now)?;
        }
        tx.execute(
            "INSERT INTO call_outcomes (task_id, tool_call_id, tool, resource, outcome, is_write, detail, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                r.task_id.as_str(),
                r.call_id.as_str(),
                r.tool,
                r.resource,
                r.outcome.as_str(),
                i64::from(r.is_write),
                r.detail,
                ts_to_sql(&now)
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Call outcomes of a task in the order they were recorded.
    pub fn list_call_outcomes(&self, task_id: &TaskId) -> Result<Vec<CallOutcome>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, tool_call_id, tool, resource, outcome, is_write, detail, created_at
             FROM call_outcomes WHERE task_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map([task_id.as_str()], outcome_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Answers every unanswered tool call of `task` with the error result `text`.
    /// Task transitions to cancelled, failed, uncertain and completed do this themselves;
    /// this is the same operation for callers that need it separately.
    pub fn close_unanswered_calls(
        &mut self,
        task_id: &TaskId,
        text: &str,
        now: Timestamp,
    ) -> Result<usize> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let n = close_unanswered_calls_in(&tx, task_id, text, now)?;
        tx.commit()?;
        Ok(n)
    }

    /// Answers exactly these unanswered calls with the error result `text`.
    pub fn close_calls(&mut self, calls: &[OpenCall], text: &str, now: Timestamp) -> Result<usize> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let n = close_calls_in(&tx, calls, text, now)?;
        tx.commit()?;
        Ok(n)
    }

    /// Finds unanswered tool calls in a conversation that belong to a task that has ended
    /// (cancelled, failed, completed, uncertain) or to no task at all, and answers them
    /// "not executed". Healing for histories written before these calls were closed at
    /// the end of their task. Calls of tasks that can still run are left alone. Returns
    /// how many calls were closed.
    pub fn close_orphan_calls(
        &mut self,
        conversation_id: &ConversationId,
        text: &str,
        now: Timestamp,
    ) -> Result<usize> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let msgs = list_messages_where(&tx, "conversation_id = ?1", conversation_id.as_str())?;
        let mut orphans = Vec::new();
        for c in open_calls(&msgs) {
            let ended = match &c.task_id {
                None => true,
                Some(t) => {
                    let st: Option<String> = tx
                        .query_row(
                            "SELECT status FROM tasks WHERE id = ?1",
                            [t.as_str()],
                            |r| r.get(0),
                        )
                        .optional()?;
                    st.is_none_or(|s| {
                        matches!(
                            s.as_str(),
                            "completed" | "failed" | "cancelled" | "uncertain"
                        )
                    })
                }
            };
            if ended {
                orphans.push(c);
            }
        }
        let n = close_calls_in(&tx, &orphans, text, now)?;
        tx.commit()?;
        Ok(n)
    }

    /// True if anything in the conversation was not typed by the user (a tool result of
    /// trust `local_granted` or `external`). The flag is derived from the stored
    /// messages, so it always agrees with what is sent to the model.
    pub fn conversation_tainted(&self, id: &ConversationId) -> Result<bool> {
        conversation_tainted_in(&self.conn, id)
    }

    /// Messages of a conversation in insertion order.
    pub fn list_messages(&self, conversation_id: &ConversationId) -> Result<Vec<StoredMessage>> {
        list_messages_where(&self.conn, "conversation_id = ?1", conversation_id.as_str())
    }

    /// Records an artifact for a task.
    pub fn add_artifact(
        &mut self,
        task_id: &TaskId,
        kind: &str,
        path: &str,
        content_hash: &str,
        bytes: u64,
        now: Timestamp,
    ) -> Result<Artifact> {
        let id = ArtifactId::new();
        let size =
            i64::try_from(bytes).map_err(|_| LumaError::invalid_input("artifact too large"))?;
        self.conn.execute(
            "INSERT INTO artifacts (id, task_id, kind, path, content_hash, bytes, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id.as_str(),
                task_id.as_str(),
                kind,
                path,
                content_hash,
                size,
                ts_to_sql(&now)
            ],
        )?;
        self.get_artifact(&id)
    }

    /// Loads an artifact.
    pub fn get_artifact(&self, id: &ArtifactId) -> Result<Artifact> {
        self.conn
            .query_row(
                "SELECT id, task_id, kind, path, content_hash, bytes, created_at FROM artifacts WHERE id = ?1",
                [id.as_str()],
                artifact_from_row,
            )
            .optional()?
            .ok_or_else(|| LumaError::invalid_input(format!("unknown artifact {id}")))
    }

    /// Artifacts of a task, oldest first.
    pub fn list_artifacts(&self, task_id: &TaskId) -> Result<Vec<Artifact>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, kind, path, content_hash, bytes, created_at
             FROM artifacts WHERE task_id = ?1 ORDER BY rowid",
        )?;
        let rows = stmt.query_map([task_id.as_str()], artifact_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}
