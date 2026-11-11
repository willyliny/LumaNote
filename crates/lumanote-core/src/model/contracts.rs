//! Model request / event types and the [`ModelAdapter`] trait (handbook 7.1, 7.2).
//!
//! The contract is the product's own; it is not any provider's API schema. Adapters
//! translate it (see `model::adapters`). Tool-call fragment assembly lives in
//! [`super::assembler`].

use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::schema::{str_enum, ArtifactId, LumaError, RequestId, Result, Timestamp, ToolCallId};

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

/// Where a piece of provider-opaque state came from (provider, protocol and model).
///
/// Opaque state (thinking signatures, ...) is only valid for the model that produced it;
/// it is replayed to the same origin and dropped for any other.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelOrigin {
    /// Provider name (profile `provider`).
    pub provider: String,
    /// Protocol identifier (profile `protocol`).
    pub protocol: String,
    /// Provider model ID (profile `model_id`).
    pub model: String,
}

impl ModelOrigin {
    /// The origin a profile produces.
    pub fn from_profile(profile: &ModelProfile) -> Self {
        Self {
            provider: profile.provider.clone(),
            protocol: profile.protocol.clone(),
            model: profile.model_id.clone(),
        }
    }
}

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
    /// Provider-opaque state attached to an assistant turn (for example an Anthropic
    /// thinking block with its signature). It sits among the other parts exactly where the
    /// provider emitted it, is replayed only to the same `origin`, and is never shown.
    ProviderState {
        /// Who produced it.
        origin: ModelOrigin,
        /// Adapter-defined kind, e.g. `anthropic.thinking`.
        kind: String,
        /// The payload, stored and replayed unmodified.
        data: Value,
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

    /// A copy without provider state that does not belong to `origin`. Returns `None` if
    /// nothing is left of the message.
    pub fn for_origin(&self, origin: &ModelOrigin) -> Option<Message> {
        let parts: Vec<MessagePart> = self
            .parts
            .iter()
            .filter(|p| match p {
                MessagePart::ProviderState { origin: o, .. } => o == origin,
                _ => true,
            })
            .cloned()
            .collect();
        (!parts.is_empty()).then_some(Message {
            role: self.role,
            parts,
        })
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

str_enum!(
    /// The kinds of `tool_choice` a model may support.
    ToolChoiceMode {
        /// The model decides.
        Auto => "auto",
        /// No tool may be called.
        None => "none",
        /// Some tool must be called.
        Required => "required",
        /// One named tool must be called.
        Tool => "tool",
    }
);

/// How the model may use tools in this request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ToolChoice {
    /// The model decides (the default; adapters send nothing for it).
    #[default]
    Auto,
    /// No tool may be called.
    None,
    /// Some tool must be called.
    Required,
    /// This tool must be called.
    Tool {
        /// Tool name.
        name: String,
    },
}

impl ToolChoice {
    /// The capability mode this choice needs.
    pub fn mode(&self) -> ToolChoiceMode {
        match self {
            ToolChoice::Auto => ToolChoiceMode::Auto,
            ToolChoice::None => ToolChoiceMode::None,
            ToolChoice::Required => ToolChoiceMode::Required,
            ToolChoice::Tool { .. } => ToolChoiceMode::Tool,
        }
    }
}

str_enum!(
    /// Reasoning effort, in the vocabulary shared by the supported protocols.
    ReasoningEffort {
        /// No reasoning.
        None => "none",
        /// Minimal reasoning.
        Minimal => "minimal",
        /// Low effort.
        Low => "low",
        /// Medium effort.
        Medium => "medium",
        /// High effort.
        High => "high",
        /// Extra high effort.
        XHigh => "xhigh",
        /// Maximum effort.
        Max => "max",
    }
);

/// Reasoning ("thinking") settings. `None` on the request means provider default: nothing
/// is sent. Asking for something the model profile does not support is an error, never
/// silently ignored (handbook 7.2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReasoningConfig {
    /// Desired effort, if any.
    pub effort: Option<ReasoningEffort>,
    /// Explicit thinking-token budget, for protocols that take one.
    pub budget_tokens: Option<u32>,
}

/// Bytes of an image the adapter may inline; the orchestrator resolves artifact references.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageData {
    /// MIME type.
    pub mime: String,
    /// Standard base64 of the bytes.
    pub base64: String,
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
    /// Absolute overall deadline, if any (enforced by the orchestrator).
    pub deadline: Option<Timestamp>,
    /// Identifier for tracing and idempotency.
    pub request_id: RequestId,
    /// Tool-choice mode.
    #[serde(default)]
    pub tool_choice: ToolChoice,
    /// `Some(false)` forbids parallel tool calls; `Some(true)`/`None` leave the default.
    #[serde(default)]
    pub parallel_tool_calls: Option<bool>,
    /// Stop sequences.
    #[serde(default)]
    pub stop_sequences: Vec<String>,
    /// Reasoning settings; `None` = provider default.
    #[serde(default)]
    pub reasoning: Option<ReasoningConfig>,
    /// Maximum silence between two pieces of stream data before the adapter gives up.
    /// Separate from the overall `deadline`. `None` = the adapter's configured default.
    #[serde(default, with = "opt_duration_ms")]
    pub idle_timeout: Option<Duration>,
    /// Bytes for the `ImageRef` parts of `messages`, keyed by artifact id.
    #[serde(default)]
    pub image_data: HashMap<ArtifactId, ImageData>,
}

impl ModelRequest {
    /// A request with every optional field at its default.
    pub fn new(
        model_profile_id: impl Into<String>,
        instructions: impl Into<String>,
        messages: Vec<Message>,
    ) -> Self {
        Self {
            model_profile_id: model_profile_id.into(),
            instructions: instructions.into(),
            messages,
            tools: Vec::new(),
            output_contract: None,
            output_budget: None,
            deadline: None,
            request_id: RequestId::new(),
            tool_choice: ToolChoice::Auto,
            parallel_tool_calls: None,
            stop_sequences: Vec::new(),
            reasoning: None,
            idle_timeout: None,
            image_data: HashMap::new(),
        }
    }
}

/// Serde helper: `Option<Duration>` as whole milliseconds.
pub(crate) mod opt_duration_ms {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &Option<Duration>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(d) => s.serialize_some(&u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Duration>, D::Error> {
        Ok(Option::<u64>::deserialize(d)?.map(Duration::from_millis))
    }
}

/// Token usage as a **cumulative snapshot**: a later `Usage` event replaces the earlier
/// one (the last one wins). Every field is optional: unknown is never recorded as zero.
///
/// `input_tokens` is the *total* prompt size including cached tokens; the cache fields are
/// the parts of it that were read from / written to the provider's cache. Uncached input is
/// therefore `input - cache_read - cache_write`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Total prompt tokens (cache reads and writes included), if reported.
    pub input_tokens: Option<u64>,
    /// Completion tokens (reasoning included), if reported.
    pub output_tokens: Option<u64>,
    /// Prompt tokens served from cache (billed at a different rate), if reported.
    pub cache_read_tokens: Option<u64>,
    /// Prompt tokens written to cache, if reported.
    pub cache_write_tokens: Option<u64>,
    /// Reasoning ("thinking") tokens, a part of the output, if reported.
    pub reasoning_tokens: Option<u64>,
}

impl Usage {
    /// True when no field is known.
    pub fn is_unknown(&self) -> bool {
        *self == Usage::default()
    }

    /// Replaces every field that `newer` knows; keeps the rest.
    pub fn overlay(&mut self, newer: &Usage) {
        fn take(dst: &mut Option<u64>, src: Option<u64>) {
            if src.is_some() {
                *dst = src;
            }
        }
        take(&mut self.input_tokens, newer.input_tokens);
        take(&mut self.output_tokens, newer.output_tokens);
        take(&mut self.cache_read_tokens, newer.cache_read_tokens);
        take(&mut self.cache_write_tokens, newer.cache_write_tokens);
        take(&mut self.reasoning_tokens, newer.reasoning_tokens);
    }
}

/// Why a model turn ended. Truncation and refusal are not success.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum StopReason {
    /// Natural end of the answer.
    EndTurn,
    /// The model wants tool results.
    ToolUse,
    /// The output budget was reached: the output is cut off.
    MaxTokens,
    /// A configured stop sequence matched.
    StopSequence,
    /// The context window was filled: the output is cut off.
    ContextExceeded,
    /// The model refused.
    Refusal,
    /// The provider filtered the output.
    ContentFilter,
    /// The provider paused the turn and wants it continued (server-side tool loops).
    Paused,
    /// Anything else the provider reported, verbatim.
    Other {
        /// The provider's own value.
        raw: String,
    },
}

impl StopReason {
    /// Stable spelling.
    pub fn as_str(&self) -> &str {
        match self {
            StopReason::EndTurn => "end_turn",
            StopReason::ToolUse => "tool_use",
            StopReason::MaxTokens => "max_tokens",
            StopReason::StopSequence => "stop_sequence",
            StopReason::ContextExceeded => "context_exceeded",
            StopReason::Refusal => "refusal",
            StopReason::ContentFilter => "content_filter",
            StopReason::Paused => "paused",
            StopReason::Other { raw } => raw,
        }
    }

    /// True when the output is known to be cut off (budget or context window).
    pub fn is_truncation(&self) -> bool {
        matches!(self, StopReason::MaxTokens | StopReason::ContextExceeded)
    }
}

impl fmt::Display for StopReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

str_enum!(
    /// Classification of a model failure (handbook 7.1 "Failed 帶錯誤類別").
    ModelErrorKind {
        /// Credential rejected (401).
        Auth => "auth",
        /// Credential valid but not allowed (403).
        Permission => "permission",
        /// Quota, credit or spend limit exhausted: never retried.
        Billing => "billing",
        /// Request-rate limit.
        RateLimited => "rate_limited",
        /// Provider temporarily overloaded.
        Overloaded => "overloaded",
        /// The input does not fit the context window.
        ContextTooLong => "context_too_long",
        /// The request itself is wrong.
        InvalidRequest => "invalid_request",
        /// The request needs something the model profile does not support.
        UnsupportedCapability => "unsupported_capability",
        /// Provider-side error.
        Server => "server",
        /// A timeout (idle, connect or provider side).
        Timeout => "timeout",
        /// Connection failed or dropped.
        Network => "network",
        /// Cancelled by the caller.
        Cancelled => "cancelled",
        /// Blocked for safety reasons.
        SafetyBlocked => "safety_blocked",
        /// The stream broke the protocol.
        MalformedStream => "malformed_stream",
        /// Unclassified.
        Other => "other",
    }
);

impl ModelErrorKind {
    /// Whether a retry can help by default.
    pub fn default_retryable(self) -> bool {
        matches!(
            self,
            ModelErrorKind::RateLimited
                | ModelErrorKind::Overloaded
                | ModelErrorKind::Server
                | ModelErrorKind::Timeout
                | ModelErrorKind::Network
        )
    }
}

/// A model failure. The message never contains credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelError {
    /// Classification.
    pub kind: ModelErrorKind,
    /// Human-readable, credential-free description.
    pub message: String,
    /// Whether a retry may help.
    pub retryable: bool,
    /// How long the provider asked us to wait before retrying.
    #[serde(default, with = "opt_duration_ms")]
    pub retry_after: Option<Duration>,
    /// HTTP status, if the failure came from a response.
    pub http_status: Option<u16>,
    /// The provider's own error code / type.
    pub provider_code: Option<String>,
    /// The provider's request ID, for support.
    pub request_id: Option<String>,
    /// True once content was already delivered to the consumer: such a stream is never
    /// replayed automatically.
    pub partial_output_emitted: bool,
}

impl ModelError {
    /// A new error with the default retryability of `kind`.
    pub fn new(kind: ModelErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            retryable: kind.default_retryable(),
            retry_after: None,
            http_status: None,
            provider_code: None,
            request_id: None,
            partial_output_emitted: false,
        }
    }

    /// The cancellation error.
    pub fn cancelled() -> Self {
        Self::new(ModelErrorKind::Cancelled, "cancelled")
    }

    /// Overrides retryability.
    pub fn with_retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }

    /// Sets the HTTP status.
    pub fn with_status(mut self, status: u16) -> Self {
        self.http_status = Some(status);
        self
    }

    /// Sets the provider error code.
    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.provider_code = Some(code.into());
        self
    }

    /// Sets the provider request id.
    pub fn with_request_id(mut self, id: Option<String>) -> Self {
        self.request_id = id;
        self
    }

    /// Sets `retry_after`.
    pub fn with_retry_after(mut self, d: Option<Duration>) -> Self {
        self.retry_after = d;
        self
    }

    /// True if retrying can never help: the failure is about money, identity, the request
    /// itself or a broken protocol, or the provider said not to retry.
    pub fn is_permanent(&self) -> bool {
        !self.retryable
            || matches!(
                self.kind,
                ModelErrorKind::Billing
                    | ModelErrorKind::Auth
                    | ModelErrorKind::Permission
                    | ModelErrorKind::Cancelled
                    | ModelErrorKind::InvalidRequest
                    | ModelErrorKind::ContextTooLong
                    | ModelErrorKind::UnsupportedCapability
                    | ModelErrorKind::SafetyBlocked
                    | ModelErrorKind::MalformedStream
            )
    }

    /// True if the stream must not be replayed automatically: the failure is permanent,
    /// or the consumer already saw output of the failed attempt.
    pub fn is_final(&self) -> bool {
        self.partial_output_emitted || self.is_permanent()
    }

    /// The shared application error for callers that do not care about the details.
    pub fn to_luma_error(&self) -> LumaError {
        match self.kind {
            ModelErrorKind::Cancelled => LumaError::Cancelled,
            ModelErrorKind::Timeout => LumaError::Timeout {
                operation: self.message.clone(),
            },
            _ => LumaError::ExternalFailure {
                message: self.to_string(),
                retryable: self.retryable && !self.partial_output_emitted,
            },
        }
    }
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

impl std::error::Error for ModelError {}

impl From<LumaError> for ModelError {
    fn from(e: LumaError) -> Self {
        match e {
            LumaError::Cancelled => ModelError::cancelled(),
            LumaError::Timeout { operation } => ModelError::new(ModelErrorKind::Timeout, operation),
            LumaError::ExternalFailure { message, retryable } => {
                let kind = if retryable {
                    ModelErrorKind::Server
                } else {
                    ModelErrorKind::Other
                };
                ModelError::new(kind, message).with_retryable(retryable)
            }
            other => ModelError::new(ModelErrorKind::Other, other.to_string()),
        }
    }
}

/// Events produced while streaming a model turn.
///
/// Order guarantees: `ResponseStarted` (if any) comes first; a `Completed` or `Failed` is
/// always the last event. `ToolCallReady` and `ToolCallRejected` are only produced for
/// calls whose argument stream ended; nothing executable is ever derived from a fragment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelEvent {
    /// The provider accepted the request and started answering.
    ResponseStarted {
        /// Provider request ID (from a header), for support.
        request_id: Option<String>,
        /// Provider response / message ID.
        response_id: Option<String>,
        /// The model that actually answers (aliases resolve to dated IDs).
        model: Option<String>,
    },
    /// Incremental assistant text.
    TextDelta {
        /// The new text.
        text: String,
    },
    /// Incremental reasoning text. Never mixed into answer text, never stored as an answer.
    ReasoningDelta {
        /// The new text.
        text: String,
    },
    /// Provider-opaque state that must travel with this assistant turn (see
    /// [`MessagePart::ProviderState`]); emitted at the position it occupies in the turn.
    ProviderState {
        /// Adapter-defined kind, e.g. `anthropic.thinking`.
        kind: String,
        /// The payload.
        data: Value,
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
    /// A tool call that cannot be executed (malformed or incomplete arguments, unknown
    /// tool). The orchestrator answers it to the model as an error result; the turn goes on.
    ToolCallRejected {
        /// Call identifier.
        call_id: ToolCallId,
        /// Tool name, if the stream carried one.
        name: Option<String>,
        /// Why, in words the model can act on.
        reason: String,
    },
    /// Cumulative token usage snapshot; the last one wins.
    Usage(Usage),
    /// The turn finished. Last event of a stream that did not fail.
    Completed {
        /// Why it finished.
        stop_reason: StopReason,
    },
    /// The turn failed. Last event of a failed stream.
    Failed {
        /// The typed error.
        error: ModelError,
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

#[allow(clippy::derivable_impls)] // `Support` is generated by `str_enum!`, which has no `#[default]`
impl Default for Support {
    fn default() -> Self {
        Support::Unverified
    }
}

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
    /// Largest output the model can produce, if known.
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    /// Whether the model may call several tools in one turn.
    #[serde(default)]
    pub parallel_tools: Support,
    /// `tool_choice` modes besides `auto` that the model accepts.
    #[serde(default)]
    pub tool_choice_modes: Vec<ToolChoiceMode>,
    /// True when the provider needs its opaque state replayed (e.g. thinking signatures).
    #[serde(default)]
    pub requires_opaque_roundtrip: bool,
    /// Whether the stream reliably reports token usage.
    #[serde(default)]
    pub usage_in_stream: Support,
    /// Whether reasoning settings are accepted.
    #[serde(default)]
    pub reasoning: Support,
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
            max_output_tokens: None,
            parallel_tools: Support::Unverified,
            tool_choice_modes: Vec::new(),
            requires_opaque_roundtrip: false,
            usage_in_stream: Support::Unverified,
            reasoning: Support::Unverified,
        }
    }

    /// True only when tool calling is verified.
    pub fn can_use_tools(&self) -> bool {
        self.tool_calling == Support::Supported
    }

    /// True if `mode` may be requested. `auto` is the baseline and always allowed.
    pub fn allows_tool_choice(&self, mode: ToolChoiceMode) -> bool {
        mode == ToolChoiceMode::Auto || self.tool_choice_modes.contains(&mode)
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

/// Outcome of an explicit connectivity probe (one cheap call; never part of a task).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeReport {
    /// True when the endpoint answered a minimal streaming request.
    pub ok: bool,
    /// The model that answered, if reported.
    pub model: Option<String>,
    /// Observed: text generation works.
    pub text: Support,
    /// Observed: the answer arrived as a stream.
    pub streaming: Support,
    /// Observed: the stream carried token usage.
    pub usage_in_stream: Support,
    /// What went wrong, if anything.
    pub error: Option<ModelError>,
}

impl ProbeReport {
    /// A report for adapters that cannot probe.
    pub fn unsupported() -> Self {
        Self {
            ok: false,
            model: None,
            text: Support::Unverified,
            streaming: Support::Unverified,
            usage_in_stream: Support::Unverified,
            error: Some(ModelError::new(
                ModelErrorKind::UnsupportedCapability,
                "this adapter has no probe",
            )),
        }
    }
}

/// Provider adapter. Business rules (authorisation, execution) never live here.
#[async_trait]
pub trait ModelAdapter: Send + Sync {
    /// Capabilities this adapter can offer for the profile.
    fn capabilities(&self, profile: &ModelProfile) -> ModelCapabilities;
    /// Checks the profile configuration without sending anything over the network.
    fn validate_config(&self, profile: &ModelProfile) -> ValidationResult;
    /// Streams one model turn. The stream ends after `Completed` or `Failed`;
    /// cancelling the token yields `Failed { kind: Cancelled }` and drops the connection.
    fn stream(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> BoxStream<'static, ModelEvent>;
    /// One explicit, cheap call that checks that the endpoint answers (settings page
    /// "connection test"). Never called implicitly.
    async fn probe(&self, _profile: &ModelProfile) -> ProbeReport {
        ProbeReport::unsupported()
    }
}

/// Checks that `args` is a JSON object containing every `required` key of the tool's schema.
pub fn validate_tool_arguments(tool: &ToolSpec, args: &Value) -> Result<()> {
    let bad = |message: String| LumaError::ExternalFailure {
        message,
        retryable: false,
    };
    let obj = args
        .as_object()
        .ok_or_else(|| bad(format!("arguments of {} are not a JSON object", tool.name)))?;
    if let Some(required) = tool
        .parameters_schema
        .get("required")
        .and_then(Value::as_array)
    {
        for key in required.iter().filter_map(Value::as_str) {
            if !obj.contains_key(key) {
                return Err(bad(format!(
                    "arguments of {} miss required field {key}",
                    tool.name
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn usage_unknown_is_not_zero_and_overlay_keeps_known_fields() {
        let u: Usage = serde_json::from_str("{}").unwrap();
        assert!(u.is_unknown());
        assert_eq!(
            serde_json::to_value(&u).unwrap()["output_tokens"],
            Value::Null
        );
        let mut snap = Usage {
            input_tokens: Some(25),
            output_tokens: Some(1),
            ..Default::default()
        };
        snap.overlay(&Usage {
            output_tokens: Some(15),
            ..Default::default()
        });
        assert_eq!(snap.input_tokens, Some(25));
        assert_eq!(snap.output_tokens, Some(15));
        assert_eq!(snap.cache_read_tokens, None);
        assert!(!ModelCapabilities::unverified().can_use_tools());
    }

    #[test]
    fn events_serialise_with_type_tag() {
        let v = serde_json::to_value(ModelEvent::Usage(Usage::default())).unwrap();
        assert_eq!(v["type"], "usage");
        let v = serde_json::to_value(ModelEvent::Failed {
            error: ModelError::cancelled(),
        })
        .unwrap();
        assert_eq!(v["error"]["kind"], "cancelled");
        let v = serde_json::to_value(ModelEvent::Completed {
            stop_reason: StopReason::Other { raw: "x".into() },
        })
        .unwrap();
        assert_eq!(v["stop_reason"]["reason"], "other");
        assert_eq!(v["stop_reason"]["raw"], "x");
    }

    #[test]
    fn error_finality_rules() {
        let rl = ModelError::new(ModelErrorKind::RateLimited, "slow down");
        assert!(rl.retryable && !rl.is_final());
        let mut partial = rl.clone();
        partial.partial_output_emitted = true;
        assert!(partial.is_final());
        let billing = ModelError::new(ModelErrorKind::Billing, "no credit").with_retryable(true);
        assert!(billing.is_final());
        assert!(ModelError::new(ModelErrorKind::Auth, "x").is_final());
        assert_eq!(ModelError::cancelled().to_luma_error(), LumaError::Cancelled);
    }

    #[test]
    fn provider_state_is_dropped_for_other_origins() {
        let a = ModelOrigin {
            provider: "anthropic".into(),
            protocol: "anthropic_messages".into(),
            model: "m1".into(),
        };
        let b = ModelOrigin {
            model: "m2".into(),
            ..a.clone()
        };
        let msg = Message {
            role: Role::Assistant,
            parts: vec![
                MessagePart::ProviderState {
                    origin: a.clone(),
                    kind: "anthropic.thinking".into(),
                    data: json!({"signature": "s"}),
                },
                MessagePart::Text { text: "hi".into() },
            ],
        };
        assert_eq!(msg.for_origin(&a).unwrap(), msg);
        assert_eq!(msg.for_origin(&b).unwrap().parts.len(), 1);
        let only_state = Message {
            role: Role::Assistant,
            parts: vec![msg.parts[0].clone()],
        };
        assert!(only_state.for_origin(&b).is_none());
    }

    #[test]
    fn capabilities_json_without_new_fields_still_loads() {
        let old = json!({
            "text":"supported","streaming":"supported","tool_calling":"supported",
            "structured_output":"unverified","image_input":"unverified",
            "context_limit":null,"verified_at":null
        });
        let caps: ModelCapabilities = serde_json::from_value(old).unwrap();
        assert!(caps.can_use_tools());
        assert!(caps.allows_tool_choice(ToolChoiceMode::Auto));
        assert!(!caps.allows_tool_choice(ToolChoiceMode::Required));
    }
}
