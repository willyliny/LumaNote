//! Model request / event types, the [`ModelAdapter`] trait and tool-call assembly.

use std::collections::HashMap;

use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::schema::{str_enum, ArtifactId, ElfError, RequestId, Result, Timestamp, ToolCallId};
use std::fmt;

str_enum!(
    /// Author of a message.
    Role {
        /// System / developer instruction.
        System => "system",
        /// The user.
        User => "user",
        /// The model.
        Assistant => "assistant",
        /// A tool result.
        Tool => "tool",
    }
);

/// One piece of message content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessagePart {
    /// Plain text.
    Text {
        /// The text.
        text: String,
    },
    /// Reference to a locally stored image (never inline bytes).
    ImageRef {
        /// Artifact holding the image.
        artifact_id: ArtifactId,
        /// MIME type, e.g. `image/png`.
        mime: String,
    },
    /// A tool call requested by the model.
    ToolCall {
        /// Call identifier, paired with the result.
        id: ToolCallId,
        /// Tool name.
        name: String,
        /// Validated JSON arguments.
        arguments: Value,
    },
    /// The result of a tool call.
    ToolResult {
        /// The call this answers.
        call_id: ToolCallId,
        /// Result text.
        content: String,
        /// Whether the tool failed.
        is_error: bool,
    },
}

/// A message in the product's own format; adapters translate it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// Author.
    pub role: Role,
    /// Content parts.
    pub parts: Vec<MessagePart>,
}

impl Message {
    /// Convenience constructor for a single text part.
    pub fn text(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            parts: vec![MessagePart::Text { text: text.into() }],
        }
    }
}

/// A tool the model may call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    /// Unique tool name.
    pub name: String,
    /// What the tool does.
    pub description: String,
    /// JSON-Schema-style description of the argument object.
    pub parameters_schema: Value,
}

/// A request to a model (handbook 7.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelRequest {
    /// Which configured model profile to use.
    pub model_profile_id: String,
    /// System instructions.
    pub instructions: String,
    /// Conversation so far.
    pub messages: Vec<Message>,
    /// Tools offered to the model.
    pub tools: Vec<ToolSpec>,
    /// Optional structured-output schema.
    pub output_contract: Option<Value>,
    /// Maximum output tokens, if limited.
    pub output_budget: Option<u32>,
    /// Absolute deadline, if any.
    pub deadline: Option<Timestamp>,
    /// Identifier for tracing and idempotency.
    pub request_id: RequestId,
}

/// Token usage. Every field is optional: unknown is never recorded as zero.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Prompt tokens, if reported.
    pub input_tokens: Option<u64>,
    /// Completion tokens, if reported.
    pub output_tokens: Option<u64>,
    /// Prompt tokens served from cache (billed separately), if reported.
    pub cached_input_tokens: Option<u64>,
    /// Reasoning tokens, if reported.
    pub reasoning_tokens: Option<u64>,
}

str_enum!(
    /// Why a model turn ended.
    FinishReason {
        /// Natural end of the answer.
        Stop => "stop",
        /// The model wants tool results.
        ToolCalls => "tool_calls",
        /// Output budget reached.
        Length => "length",
        /// Provider filtered the output.
        ContentFilter => "content_filter",
        /// Anything else the provider reported.
        Other => "other",
    }
);

/// Events produced while streaming a model turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelEvent {
    /// Incremental assistant text.
    TextDelta {
        /// The new text.
        text: String,
    },
    /// A raw fragment of a tool call; informational only, never executable.
    ToolCallDelta {
        /// Call identifier.
        call_id: ToolCallId,
        /// Tool name, when this fragment carries it.
        name: Option<String>,
        /// Raw argument text fragment.
        arguments_fragment: String,
    },
    /// A complete tool call whose arguments parsed and validated.
    ToolCallReady {
        /// Call identifier.
        call_id: ToolCallId,
        /// Tool name.
        name: String,
        /// Validated JSON object.
        arguments: Value,
    },
    /// Token usage report.
    Usage(Usage),
    /// The turn finished normally. Last event of a successful stream.
    Completed {
        /// Why it finished.
        finish_reason: FinishReason,
    },
    /// The turn failed. Last event of a failed stream.
    Failed {
        /// The typed error.
        error: ElfError,
    },
}

str_enum!(
    /// Whether a model feature is known to work (handbook 7.2: unknown means unverified).
    Support {
        /// Not verified yet; treated as unavailable.
        Unverified => "unverified",
        /// Verified not to work.
        Unsupported => "unsupported",
        /// Verified to work.
        Supported => "supported",
    }
);

/// Capability declaration of a model profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCapabilities {
    /// Text generation.
    pub text: Support,
    /// Streaming responses.
    pub streaming: Support,
    /// Native tool calling.
    pub tool_calling: Support,
    /// Native structured output.
    pub structured_output: Support,
    /// Image input.
    pub image_input: Support,
    /// Context window in tokens, if known.
    pub context_limit: Option<u32>,
    /// When the declaration was last verified.
    pub verified_at: Option<Timestamp>,
}

impl ModelCapabilities {
    /// Everything unverified.
    pub fn unverified() -> Self {
        Self {
            text: Support::Unverified,
            streaming: Support::Unverified,
            tool_calling: Support::Unverified,
            structured_output: Support::Unverified,
            image_input: Support::Unverified,
            context_limit: None,
            verified_at: None,
        }
    }
    /// True only when tool calling is verified.
    pub fn can_use_tools(&self) -> bool {
        self.tool_calling == Support::Supported
    }
}

/// A configured model endpoint (subset of the `model_profiles` row).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelProfile {
    /// Profile identifier.
    pub id: String,
    /// Provider name.
    pub provider: String,
    /// Protocol identifier, selects the adapter.
    pub protocol: String,
    /// API base URL.
    pub base_url: String,
    /// Provider model ID.
    pub model_id: String,
    /// Reference into the credential store.
    pub credential_ref: Option<String>,
    /// Declared capabilities.
    pub capabilities: ModelCapabilities,
    /// Bumped on every configuration change.
    pub config_version: u32,
}

/// Result of validating a profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationResult {
    /// True when no issue was found.
    pub ok: bool,
    /// Human-readable problems.
    pub issues: Vec<String>,
}

/// Provider adapter. Business rules (authorisation, execution) never live here.
pub trait ModelAdapter: Send + Sync {
    /// Capabilities this adapter can offer for the profile.
    fn capabilities(&self, profile: &ModelProfile) -> ModelCapabilities;
    /// Checks the profile configuration without sending user data.
    fn validate_config(&self, profile: &ModelProfile) -> ValidationResult;
    /// Streams one model turn. The stream ends after `Completed` or `Failed`;
    /// cancelling the token yields `Failed { Cancelled }`.
    fn stream(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> BoxStream<'static, ModelEvent>;
}

fn bad_tool_call(message: String) -> ElfError {
    ElfError::ExternalFailure {
        message,
        retryable: false,
    }
}

/// Checks that `args` is a JSON object containing every `required` key of the tool's schema.
pub fn validate_tool_arguments(tool: &ToolSpec, args: &Value) -> Result<()> {
    let obj = args.as_object().ok_or_else(|| {
        bad_tool_call(format!("arguments of {} are not a JSON object", tool.name))
    })?;
    if let Some(required) = tool
        .parameters_schema
        .get("required")
        .and_then(Value::as_array)
    {
        for key in required.iter().filter_map(Value::as_str) {
            if !obj.contains_key(key) {
                return Err(bad_tool_call(format!(
                    "arguments of {} miss required field {key}",
                    tool.name
                )));
            }
        }
    }
    Ok(())
}

#[derive(Debug, Default)]
struct PendingCall {
    name: Option<String>,
    buffer: String,
}

/// Reassembles streamed tool-call fragments. Adapters feed it provider fragments
/// and only emit `ToolCallReady` for what [`ToolCallAssembler::finish`] returns.
#[derive(Debug, Default)]
pub struct ToolCallAssembler {
    pending: HashMap<ToolCallId, PendingCall>,
    done: std::collections::HashSet<ToolCallId>,
}

impl ToolCallAssembler {
    /// Creates an empty assembler.
    pub fn new() -> Self {
        Self::default()
    }

    /// True if `call_id` was already finished (later events for it are duplicates).
    pub fn is_done(&self, call_id: &ToolCallId) -> bool {
        self.done.contains(call_id)
    }

    /// Appends a fragment. Returns `false` (and ignores it) if the call already finished.
    pub fn push_fragment(
        &mut self,
        call_id: &ToolCallId,
        name: Option<&str>,
        fragment: &str,
    ) -> bool {
        if self.done.contains(call_id) {
            return false;
        }
        let p = self.pending.entry(call_id.clone()).or_default();
        if let Some(n) = name {
            p.name = Some(n.to_owned());
        }
        p.buffer.push_str(fragment);
        true
    }

    /// True while some call has fragments but was never finished.
    pub fn has_incomplete(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Finishes a call: parses and validates its arguments against `tools`.
    /// Returns `Ok(None)` if the call was already finished (duplicate end event).
    /// An empty argument buffer is treated as `{}`.
    pub fn finish(
        &mut self,
        call_id: &ToolCallId,
        tools: &[ToolSpec],
    ) -> Result<Option<ModelEvent>> {
        if self.done.contains(call_id) {
            return Ok(None);
        }
        let Some(p) = self.pending.remove(call_id) else {
            return Err(bad_tool_call(format!(
                "tool call {call_id} ended without any data"
            )));
        };
        self.done.insert(call_id.clone());
        let name = p
            .name
            .ok_or_else(|| bad_tool_call(format!("tool call {call_id} has no tool name")))?;
        let tool = tools
            .iter()
            .find(|t| t.name == name)
            .ok_or_else(|| bad_tool_call(format!("model called unknown tool {name}")))?;
        let raw = if p.buffer.trim().is_empty() {
            "{}"
        } else {
            p.buffer.as_str()
        };
        let arguments: Value = serde_json::from_str(raw)
            .map_err(|e| bad_tool_call(format!("malformed JSON arguments for {name}: {e}")))?;
        validate_tool_arguments(tool, &arguments)?;
        Ok(Some(ModelEvent::ToolCallReady {
            call_id: call_id.clone(),
            name,
            arguments,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tools() -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "read_file".into(),
            description: "read".into(),
            parameters_schema: json!({"type":"object","required":["path"]}),
        }]
    }

    #[test]
    fn assembler_waits_for_complete_valid_json() {
        let mut a = ToolCallAssembler::new();
        let id = ToolCallId::from_string("c1");
        a.push_fragment(&id, Some("read_file"), "{\"pa");
        a.push_fragment(&id, None, "th\":\"/a\"}");
        let ev = a.finish(&id, &tools()).unwrap().unwrap();
        assert_eq!(
            ev,
            ModelEvent::ToolCallReady {
                call_id: id.clone(),
                name: "read_file".into(),
                arguments: json!({"path":"/a"})
            }
        );
        assert!(a.finish(&id, &tools()).unwrap().is_none());
        assert!(!a.push_fragment(&id, None, "x"));
        assert!(!a.has_incomplete());
    }

    #[test]
    fn assembler_rejects_bad_calls() {
        let t = tools();
        for (name, buf) in [
            ("read_file", "{\"path\":"),
            ("read_file", "[1]"),
            ("read_file", "{}"),
            ("nope", "{}"),
        ] {
            let mut a = ToolCallAssembler::new();
            let id = ToolCallId::from_string("c");
            a.push_fragment(&id, Some(name), buf);
            assert!(
                matches!(a.finish(&id, &t), Err(ElfError::ExternalFailure { .. })),
                "{buf}"
            );
        }
        let mut a = ToolCallAssembler::new();
        let id = ToolCallId::from_string("c");
        a.push_fragment(&id, None, "{}");
        assert!(a.finish(&id, &t).is_err());
    }

    #[test]
    fn usage_unknown_is_not_zero() {
        let u: Usage = serde_json::from_str("{}").unwrap();
        assert_eq!(u.input_tokens, None);
        assert_eq!(
            serde_json::to_value(&u).unwrap()["output_tokens"],
            Value::Null
        );
        assert!(!ModelCapabilities::unverified().can_use_tools());
    }

    #[test]
    fn events_serialise_with_type_tag() {
        let v = serde_json::to_value(ModelEvent::Usage(Usage::default())).unwrap();
        assert_eq!(v["type"], "usage");
        let v = serde_json::to_value(ModelEvent::Failed {
            error: ElfError::Cancelled,
        })
        .unwrap();
        assert_eq!(v["error"]["kind"], "cancelled");
    }
}
