//! Events the orchestrator pushes to the shell (handbook section 12: `task_progress`,
//! `assistant_delta`, `approval_required`, `artifact_ready`). The Tauri shell implements
//! [`EventSink`] and relays them to the WebView; the core never depends on Tauri.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::model::Usage;
use crate::schema::{ApprovalId, ConversationId, TaskId, ToolCallId};
use crate::storage::Artifact;
use crate::task::TaskStatus;

/// Version of the event schema (bump on incompatible changes).
pub const EVENT_SCHEMA_VERSION: u32 = 1;

/// What happened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEventKind {
    /// The task changed state or reached a milestone.
    TaskProgress {
        /// Task status after the change.
        status: TaskStatus,
        /// Short human-readable description (e.g. the pause reason).
        message: String,
    },
    /// Incremental assistant text.
    AssistantDelta {
        /// The new text.
        text: String,
    },
    /// Incremental reasoning text from the model. Display-only: it is never stored and
    /// never part of the answer.
    ReasoningDelta {
        /// The new text.
        text: String,
    },
    /// The provider accepted the request (which provider and model actually answer).
    ModelStarted {
        /// Provider name of the profile.
        provider: String,
        /// The model that actually answers, if the provider said.
        model: Option<String>,
        /// The provider's request id, for support.
        request_id: Option<String>,
    },
    /// The model asked for a call that cannot run (malformed or incomplete arguments, an
    /// unknown tool). Nothing was executed; the model gets an error result and continues.
    ToolCallRejected {
        /// Call identifier.
        call_id: ToolCallId,
        /// Tool name, if the stream carried one.
        tool: Option<String>,
        /// Why it was rejected.
        reason: String,
    },
    /// The model stream failed and is being retried; the UI should drop the text of the
    /// failed attempt.
    StreamRetry {
        /// 1-based retry number.
        attempt: u32,
        /// Why the stream failed.
        reason: String,
    },
    /// A tool call passed validation and policy and is about to run.
    ToolStarted {
        /// Call identifier.
        call_id: ToolCallId,
        /// Tool name.
        tool: String,
    },
    /// A tool call produced a result (success or error) that goes back to the model.
    ToolFinished {
        /// Call identifier.
        call_id: ToolCallId,
        /// Tool name.
        tool: String,
        /// Whether the model was told it failed.
        is_error: bool,
        /// Whether the output was cut at the tool's size limit.
        truncated: bool,
    },
    /// The user must decide before the task can continue.
    ApprovalRequired {
        /// Approval to answer with `respond_to_approval`.
        approval_id: ApprovalId,
        /// The waiting tool call.
        tool_call_id: ToolCallId,
        /// Tool name.
        tool: String,
        /// Concrete description of what would happen.
        preview: String,
        /// Reason code (`no_matching_grant` / `untrusted_content`).
        reason: String,
    },
    /// A tool produced a file or document.
    ArtifactReady {
        /// The stored artifact.
        artifact: Artifact,
    },
    /// Token usage reported by the model.
    Usage {
        /// The report (unknown fields stay `None`).
        usage: Usage,
    },
}

/// One event for the shell.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEvent {
    /// Schema version, see [`EVENT_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Task the event belongs to; `None` on the chat path.
    pub task_id: Option<TaskId>,
    /// Conversation, when known.
    pub conversation_id: Option<ConversationId>,
    /// Sequence number of the matching `task_events` row, for events that have one
    /// (state changes). The frontend reads authoritative state again after a reconnect.
    pub sequence: Option<i64>,
    /// What happened.
    pub kind: AgentEventKind,
}

/// Receiver of orchestrator events. Must not block.
pub trait EventSink: Send + Sync {
    /// Delivers one event.
    fn emit(&self, event: AgentEvent);
}

/// Sink that drops everything.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _event: AgentEvent) {}
}

/// Sink that records events in memory (tests, early shell development).
#[derive(Debug, Default)]
pub struct RecordingSink {
    events: Mutex<Vec<AgentEvent>>,
}

impl RecordingSink {
    /// An empty sink.
    pub fn new() -> Self {
        Self::default()
    }

    /// Everything received so far.
    pub fn events(&self) -> Vec<AgentEvent> {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Just the event kinds.
    pub fn kinds(&self) -> Vec<AgentEventKind> {
        self.events().into_iter().map(|e| e.kind).collect()
    }
}

impl EventSink for RecordingSink {
    fn emit(&self, event: AgentEvent) {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(event);
    }
}
