//! Conversations, messages and artifacts.

use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use super::{conv, get_opt_ts, get_ts, to_json, Store};
use crate::model::{MessagePart, Role};
use crate::schema::{
    ts_to_sql, ArtifactId, ConversationId, ElfError, MessageId, Result, TaskId, Timestamp,
    ToolCallId,
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
            serde_json::from_str(&content).map_err(|e| ElfError::Storage {
                message: format!("bad message content: {e}"),
            }),
        )?,
        tool_call_id: r.get::<_, Option<String>>(5)?.map(ToolCallId::from_string),
        created_at: get_ts(r, 6)?,
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
            .ok_or_else(|| ElfError::invalid_input(format!("unknown conversation {id}")))
    }

    /// Appends a message and bumps the conversation's `updated_at`.
    pub fn append_message(
        &mut self,
        conversation_id: &ConversationId,
        task_id: Option<&TaskId>,
        role: Role,
        parts: &[MessagePart],
        tool_call_id: Option<&ToolCallId>,
        now: Timestamp,
    ) -> Result<StoredMessage> {
        let id = MessageId::new();
        let ts = ts_to_sql(&now);
        let content = to_json(&parts)?;
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO messages (id, conversation_id, task_id, role, content_json, tool_call_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id.as_str(),
                conversation_id.as_str(),
                task_id.map(TaskId::as_str),
                role.as_str(),
                content,
                tool_call_id.map(ToolCallId::as_str),
                ts
            ],
        )?;
        tx.execute(
            "UPDATE conversations SET updated_at = ?2 WHERE id = ?1",
            params![conversation_id.as_str(), ts],
        )?;
        tx.commit()?;
        Ok(StoredMessage {
            id,
            conversation_id: conversation_id.clone(),
            task_id: task_id.cloned(),
            role,
            parts: parts.to_vec(),
            tool_call_id: tool_call_id.cloned(),
            created_at: now,
        })
    }

    /// Messages of a conversation in insertion order.
    pub fn list_messages(&self, conversation_id: &ConversationId) -> Result<Vec<StoredMessage>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, conversation_id, task_id, role, content_json, tool_call_id, created_at
             FROM messages WHERE conversation_id = ?1 ORDER BY rowid",
        )?;
        let rows = stmt.query_map([conversation_id.as_str()], message_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
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
            i64::try_from(bytes).map_err(|_| ElfError::invalid_input("artifact too large"))?;
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
            .ok_or_else(|| ElfError::invalid_input(format!("unknown artifact {id}")))
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
