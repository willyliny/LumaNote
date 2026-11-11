//! Rebuilding model context from local history, and finding unanswered tool calls.

use std::collections::HashSet;

use serde_json::Value;

use crate::model::{Message, MessagePart, Role};
use crate::schema::ToolCallId;
use crate::storage::StoredMessage;

/// A tool call the model made that has no stored result yet.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PendingCall {
    pub id: ToolCallId,
    pub name: String,
    pub arguments: Value,
}

/// The model-facing messages: everything stored for the conversation, in order. The
/// product's local history is the only source; no provider conversation ID is involved.
pub(crate) fn to_messages(history: &[StoredMessage]) -> Vec<Message> {
    history
        .iter()
        .map(|m| Message {
            role: m.role,
            parts: m.parts.clone(),
        })
        .collect()
}

/// Tool calls of the latest assistant message that still lack a stored result.
pub(crate) fn pending_calls(history: &[StoredMessage]) -> Vec<PendingCall> {
    let Some(idx) = history.iter().rposition(|m| {
        m.role == Role::Assistant
            && m.parts
                .iter()
                .any(|p| matches!(p, MessagePart::ToolCall { .. }))
    }) else {
        return Vec::new();
    };
    let answered: HashSet<&ToolCallId> = history[idx + 1..]
        .iter()
        .flat_map(|m| m.parts.iter())
        .filter_map(|p| match p {
            MessagePart::ToolResult { call_id, .. } => Some(call_id),
            _ => None,
        })
        .collect();
    history[idx]
        .parts
        .iter()
        .filter_map(|p| match p {
            MessagePart::ToolCall {
                id,
                name,
                arguments,
            } if !answered.contains(id) => Some(PendingCall {
                id: id.clone(),
                name: name.clone(),
                arguments: arguments.clone(),
            }),
            _ => None,
        })
        .collect()
}

/// True if the newest message is an assistant message without tool calls (a final answer).
pub(crate) fn ends_with_final_answer(history: &[StoredMessage]) -> bool {
    history.last().is_some_and(|m| {
        m.role == Role::Assistant
            && !m
                .parts
                .iter()
                .any(|p| matches!(p, MessagePart::ToolCall { .. }))
    })
}
