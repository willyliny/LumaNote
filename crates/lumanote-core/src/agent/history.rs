//! Rebuilding model context from local history.

use std::collections::HashSet;

use serde_json::Value;

use crate::model::{Message, MessagePart, ModelOrigin, Role};
use crate::schema::{TaskId, ToolCallId};
use crate::storage::{open_calls, StoredMessage};

/// A tool call the model made that has no stored result yet.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PendingCall {
    pub id: ToolCallId,
    pub name: String,
    pub arguments: Value,
    /// Set when the call must not run; it is answered as an error with this text.
    pub refusal: Option<String>,
}

/// The model-facing messages: everything stored for the conversation, in order. The
/// product's local history is the only source; no provider conversation ID is involved.
/// Provider-opaque state (thinking signatures, ...) is kept only where it was produced by
/// `origin`; for any other model it is dropped here, before an adapter ever sees it.
///
/// A tool call that has no result and does not belong to `current` (the task being run, or
/// none on the chat path) is left out: it is another task's business, the provider must not
/// receive a dangling call, and nothing here can ever make it run.
pub(crate) fn to_messages(
    history: &[StoredMessage],
    origin: &ModelOrigin,
    current: Option<&TaskId>,
) -> Vec<Message> {
    let foreign: HashSet<(Option<TaskId>, ToolCallId)> = open_calls(history)
        .into_iter()
        .filter(|c| c.task_id.as_ref() != current || current.is_none())
        .map(|c| (c.task_id, c.call_id))
        .collect();
    history
        .iter()
        .filter_map(|m| {
            let parts: Vec<MessagePart> = m
                .parts
                .iter()
                .filter(|p| match p {
                    MessagePart::ToolCall { id, .. } if m.role == Role::Assistant => {
                        !foreign.contains(&(m.task_id.clone(), id.clone()))
                    }
                    _ => true,
                })
                .cloned()
                .collect();
            if parts.is_empty() && !m.parts.is_empty() {
                return None;
            }
            Message {
                role: m.role,
                parts,
            }
            .for_origin(origin)
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
