//! Adapter for the Anthropic Messages API (`POST /v1/messages`, server-sent events).
//!
//! Wire facts come from R2 section 1 (docs/research); items R2 marks unverified are
//! listed in the module-level notes of the final report and handled conservatively here.
//!
//! * system prompt is the top-level `system` string; tool results are `tool_result`
//!   blocks at the front of a `user` message, all results of one assistant turn in the
//!   same message; `max_tokens` is mandatory.
//! * thinking / redacted-thinking blocks are captured as provider state and replayed
//!   unmodified, in their original position, to the same model only.
//! * no sampling parameters (`temperature`, `top_p`, `top_k`) are ever sent: version
//!   `2026-10-21` rejects them with 400.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use futures::stream::BoxStream;
use serde_json::{json, Map, Value};
use tokio_util::sync::CancellationToken;

use super::common::*;
use crate::model::assembler::ToolCallAssembler;
use crate::model::contracts::*;
use crate::model::sse::SseEvent;
use crate::ports::HttpRequest;
use crate::schema::{RequestId, Timestamp, ToolCallId};

/// Which header carries the API key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AnthropicAuth {
    /// `x-api-key: <key>` (the long-standing header; accepted by Anthropic-compatible servers).
    #[default]
    XApiKey,
    /// `Authorization: Bearer <key>`.
    Bearer,
}

/// Tunables of [`AnthropicAdapter`].
#[derive(Debug, Clone)]
pub struct AnthropicConfig {
    /// Value of the `anthropic-version` header.
    pub version: String,
    /// Key header style.
    pub auth: AnthropicAuth,
    /// Allow plain `http` to loopback hosts (local Anthropic-compatible servers).
    pub allow_loopback_http: bool,
    /// Default idle timeout (a request may carry its own).
    pub idle_timeout: Duration,
    /// `max_tokens` when neither the request nor the capability profile names one.
    pub default_max_tokens: u32,
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        Self {
            version: "2026-10-21".to_owned(),
            auth: AnthropicAuth::XApiKey,
            allow_loopback_http: false,
            idle_timeout: Duration::from_secs(120),
            default_max_tokens: 8192,
        }
    }
}

/// Adapter for one Anthropic Messages profile.
pub struct AnthropicAdapter {
    profile: ModelProfile,
    deps: AdapterDeps,
    config: AnthropicConfig,
}

impl std::fmt::Debug for AnthropicAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicAdapter")
            .field("profile", &self.profile.id)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

fn invalid(m: impl Into<String>) -> ModelError {
    ModelError::new(ModelErrorKind::InvalidRequest, m)
}
fn unsupported(m: impl Into<String>) -> ModelError {
    ModelError::new(ModelErrorKind::UnsupportedCapability, m)
}

fn valid_tool_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 128
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

impl AnthropicAdapter {
    /// Creates an adapter bound to `profile`.
    pub fn new(profile: ModelProfile, deps: AdapterDeps, config: AnthropicConfig) -> Self {
        Self {
            profile,
            deps,
            config,
        }
    }

    fn endpoint(&self, base: &url::Url) -> String {
        if base.path().trim_end_matches('/').ends_with("/v1") {
            join_path(base, "messages")
        } else {
            join_path(base, "v1/messages")
        }
    }

    /// Builds the JSON request body (pure; no secrets involved).
    pub(crate) fn build_body(&self, req: &ModelRequest) -> Result<Value, ModelError> {
        let caps = &self.profile.capabilities;
        if req.model_profile_id != self.profile.id {
            return Err(invalid(format!(
                "request is for profile `{}` but this adapter serves `{}`",
                req.model_profile_id, self.profile.id
            )));
        }
        validate_tool_pairing(&req.messages)?;
        if !req.tools.is_empty() && caps.tool_calling != Support::Supported {
            return Err(unsupported(
                "tool calling is not verified for this model profile",
            ));
        }
        let mut tools = Vec::new();
        for t in &req.tools {
            if !valid_tool_name(&t.name) {
                return Err(invalid(format!(
                    "tool name `{}` does not match ^[a-zA-Z0-9_-]{{1,128}}$",
                    t.name
                )));
            }
            if !t.parameters_schema.is_object() {
                return Err(invalid(format!(
                    "schema of tool `{}` is not a JSON object",
                    t.name
                )));
            }
            tools.push(json!({
                "name": t.name,
                "description": t.description,
                "input_schema": t.parameters_schema,
            }));
        }

        let max_tokens = {
            let wanted = req
                .output_budget
                .or(caps.max_output_tokens)
                .unwrap_or(self.config.default_max_tokens);
            caps.max_output_tokens.map_or(wanted, |cap| wanted.min(cap))
        };

        let origin = ModelOrigin::from_profile(&self.profile);
        let mut system: Vec<String> = Vec::new();
        if !req.instructions.trim().is_empty() {
            system.push(req.instructions.clone());
        }
        let mut messages: Vec<(&'static str, Vec<Value>)> = Vec::new();
        let mut push = |role: &'static str, blocks: Vec<Value>| {
            if blocks.is_empty() {
                return;
            }
            match messages.last_mut() {
                Some((r, existing)) if *r == role => existing.extend(blocks),
                _ => messages.push((role, blocks)),
            }
        };
        for msg in &req.messages {
            match msg.role {
                Role::System => {
                    let t = joined_text(&msg.parts);
                    if !t.trim().is_empty() {
                        system.push(t);
                    }
                }
                Role::User | Role::Tool => {
                    let mut blocks = Vec::new();
                    for p in &msg.parts {
                        match p {
                            MessagePart::Text { text } => {
                                if !text.trim().is_empty() {
                                    blocks.push(json!({"type":"text","text":text}));
                                }
                            }
                            MessagePart::ImageRef { artifact_id, mime } => {
                                if caps.image_input != Support::Supported {
                                    return Err(unsupported(
                                        "image input is not supported by this model profile",
                                    ));
                                }
                                if msg.role == Role::Tool {
                                    return Err(invalid(
                                        "images inside tool results are not supported",
                                    ));
                                }
                                let data = req.image_data.get(artifact_id).ok_or_else(|| {
                                    invalid(format!(
                                        "no image bytes supplied for artifact {artifact_id}"
                                    ))
                                })?;
                                blocks.push(json!({"type":"image","source":{
                                    "type":"base64",
                                    "media_type": if data.mime.is_empty() { mime } else { &data.mime },
                                    "data": data.base64,
                                }}));
                            }
                            MessagePart::ToolResult {
                                call_id,
                                content,
                                is_error,
                            } => {
                                let mut b = Map::new();
                                b.insert("type".into(), json!("tool_result"));
                                b.insert("tool_use_id".into(), json!(call_id.as_str()));
                                if !content.is_empty() {
                                    b.insert("content".into(), json!(content));
                                }
                                if *is_error {
                                    b.insert("is_error".into(), json!(true));
                                }
                                blocks.push(Value::Object(b));
                            }
                            MessagePart::ToolCall { .. } => {
                                return Err(invalid(
                                    "tool calls are only valid in assistant messages",
                                ))
                            }
                            MessagePart::ProviderState { .. } => {}
                        }
                    }
                    // tool_result blocks must come first in the user message
                    blocks.sort_by_key(|b| b["type"] != "tool_result");
                    push("user", blocks);
                }
                Role::Assistant => {
                    let mut blocks = Vec::new();
                    for p in &msg.parts {
                        match p {
                            MessagePart::Text { text } => {
                                if !text.trim().is_empty() {
                                    blocks.push(json!({"type":"text","text":text}));
                                }
                            }
                            MessagePart::ToolCall {
                                id,
                                name,
                                arguments,
                            } => blocks.push(json!({
                                "type":"tool_use",
                                "id": id.as_str(),
                                "name": name,
                                "input": if arguments.is_object() { arguments.clone() } else { json!({}) },
                            })),
                            MessagePart::ProviderState {
                                origin: o,
                                kind,
                                data,
                            } => {
                                if *o == origin && kind.starts_with("anthropic.") {
                                    blocks.push(data.clone());
                                }
                            }
                            MessagePart::ImageRef { .. } | MessagePart::ToolResult { .. } => {
                                return Err(invalid("assistant messages cannot contain images or tool results"))
                            }
                        }
                    }
                    push("assistant", blocks);
                }
            }
        }
        if messages.first().map(|(r, _)| *r) != Some("user") {
            return Err(invalid("the conversation must start with a user message"));
        }
        if messages.last().map(|(r, _)| *r) != Some("user") {
            return Err(invalid(
                "the conversation must end with a user or tool message (no prefill)",
            ));
        }

        let mut body = Map::new();
        body.insert("model".into(), json!(self.profile.model_id));
        body.insert("max_tokens".into(), json!(max_tokens));
        body.insert("stream".into(), json!(true));
        if !system.is_empty() {
            body.insert("system".into(), json!(system.join("\n\n")));
        }
        body.insert(
            "messages".into(),
            Value::Array(
                messages
                    .into_iter()
                    .map(|(role, content)| json!({"role": role, "content": content}))
                    .collect(),
            ),
        );
        if !tools.is_empty() {
            body.insert("tools".into(), Value::Array(tools));
        }
        // tool_choice
        let mode = req.tool_choice.mode();
        if !req.tools.is_empty() {
            if !caps.allows_tool_choice(mode) {
                return Err(unsupported(format!(
                    "tool_choice `{mode}` is not supported by this model profile"
                )));
            }
            let no_parallel = req.parallel_tool_calls == Some(false)
                && caps.parallel_tools != Support::Unsupported;
            let mut tc = match &req.tool_choice {
                ToolChoice::Auto => {
                    no_parallel.then(|| Map::from_iter([("type".to_owned(), json!("auto"))]))
                }
                ToolChoice::None => Some(Map::from_iter([("type".to_owned(), json!("none"))])),
                ToolChoice::Required => Some(Map::from_iter([("type".to_owned(), json!("any"))])),
                ToolChoice::Tool { name } => {
                    if !req.tools.iter().any(|t| &t.name == name) {
                        return Err(invalid(format!("tool_choice names unknown tool `{name}`")));
                    }
                    Some(Map::from_iter([
                        ("type".to_owned(), json!("tool")),
                        ("name".to_owned(), json!(name)),
                    ]))
                }
            };
            if let Some(m) = tc.as_mut() {
                if no_parallel && !matches!(req.tool_choice, ToolChoice::None) {
                    m.insert("disable_parallel_tool_use".into(), json!(true));
                }
            }
            if let Some(m) = tc {
                body.insert("tool_choice".into(), Value::Object(m));
            }
        } else if mode != ToolChoiceMode::Auto {
            return Err(invalid("tool_choice was set but no tools are offered"));
        }
        if !req.stop_sequences.is_empty() {
            body.insert("stop_sequences".into(), json!(req.stop_sequences));
        }
        // reasoning / structured output
        let mut output_config = Map::new();
        if let Some(r) = &req.reasoning {
            if caps.reasoning != Support::Supported {
                return Err(unsupported(
                    "reasoning settings are not supported by this model profile",
                ));
            }
            let thinking = match r.budget_tokens {
                Some(b) => {
                    if b >= max_tokens {
                        return Err(invalid("thinking budget must be below max_tokens"));
                    }
                    json!({"type":"enabled","budget_tokens":b})
                }
                None => json!({"type":"adaptive"}),
            };
            body.insert("thinking".into(), thinking);
            if let Some(e) = r.effort {
                match e {
                    ReasoningEffort::Low
                    | ReasoningEffort::Medium
                    | ReasoningEffort::High
                    | ReasoningEffort::XHigh
                    | ReasoningEffort::Max => {
                        output_config.insert("effort".into(), json!(e.as_str()));
                    }
                    ReasoningEffort::None | ReasoningEffort::Minimal => {
                        return Err(unsupported(format!(
                            "reasoning effort `{e}` does not exist for this protocol"
                        )))
                    }
                }
            }
        }
        if let Some(schema) = &req.output_contract {
            if caps.structured_output != Support::Supported {
                return Err(unsupported(
                    "structured output is not supported by this model profile",
                ));
            }
            output_config.insert(
                "format".into(),
                json!({"type":"json_schema","schema":schema}),
            );
        }
        if !output_config.is_empty() {
            body.insert("output_config".into(), Value::Object(output_config));
        }
        Ok(Value::Object(body))
    }

    fn prepare(&self, req: &ModelRequest) -> Result<(HttpRequest, Option<String>), ModelError> {
        let base = parse_endpoint(&self.profile.base_url, self.config.allow_loopback_http)
            .map_err(invalid)?;
        let body = self.build_body(req)?;
        let secret = read_secret(&*self.deps.credentials, &self.profile)?;
        if secret.is_none() && !is_loopback(&base) {
            return Err(ModelError::new(
                ModelErrorKind::Auth,
                "this profile has no credential_ref; the endpoint requires an API key",
            ));
        }
        let mut headers = vec![
            ("content-type".to_owned(), "application/json".to_owned()),
            ("accept".to_owned(), "text/event-stream".to_owned()),
            ("anthropic-version".to_owned(), self.config.version.clone()),
        ];
        if let Some(key) = &secret {
            match self.config.auth {
                AnthropicAuth::XApiKey => headers.push(("x-api-key".to_owned(), key.clone())),
                AnthropicAuth::Bearer => {
                    headers.push(("authorization".to_owned(), format!("Bearer {key}")))
                }
            }
        }
        let bytes = serde_json::to_vec(&body)
            .map_err(|e| invalid(format!("cannot serialise the request: {e}")))?;
        Ok((
            HttpRequest {
                method: "POST".to_owned(),
                url: self.endpoint(&base),
                headers,
                body: Some(bytes),
            },
            secret,
        ))
    }
}

#[async_trait::async_trait]
impl ModelAdapter for AnthropicAdapter {
    fn capabilities(&self, profile: &ModelProfile) -> ModelCapabilities {
        profile.capabilities.clone()
    }

    fn validate_config(&self, profile: &ModelProfile) -> ValidationResult {
        let mut issues = Vec::new();
        if profile.protocol != PROTOCOL_ANTHROPIC {
            issues.push(format!(
                "protocol `{}` is not `{PROTOCOL_ANTHROPIC}`",
                profile.protocol
            ));
        }
        if profile.model_id.trim().is_empty() {
            issues.push("model_id is empty".to_owned());
        }
        let local = match parse_endpoint(&profile.base_url, self.config.allow_loopback_http) {
            Ok(u) => is_loopback(&u),
            Err(e) => {
                issues.push(e);
                false
            }
        };
        match &profile.credential_ref {
            Some(r) => match self.deps.credentials.get(r) {
                Ok(Some(s)) if !s.trim().is_empty() => {}
                Ok(_) => issues.push(format!("no credential is stored under `{r}`")),
                Err(e) => issues.push(format!("credential store error: {e}")),
            },
            None if !local => issues.push("credential_ref is missing".to_owned()),
            None => {}
        }
        if self.config.version.trim().is_empty() {
            issues.push("anthropic-version is empty".to_owned());
        }
        ValidationResult {
            ok: issues.is_empty(),
            issues,
        }
    }

    fn stream(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> BoxStream<'static, ModelEvent> {
        let (http, secret) = match self.prepare(&request) {
            Ok(x) => x,
            Err(e) => return failed_stream(e),
        };
        let protocol = AnthropicStream::new(
            request.tools.clone(),
            request.request_id.clone(),
            used_call_ids(&request.messages),
            secret,
        );
        run(StreamJob {
            transport: self.deps.transport.clone(),
            clock: self.deps.clock.clone(),
            request: http,
            protocol: Box::new(protocol),
            idle: request.idle_timeout.unwrap_or(self.config.idle_timeout),
            cancel,
        })
    }

    async fn probe(&self, profile: &ModelProfile) -> ProbeReport {
        let bound = Self::new(profile.clone(), self.deps.clone(), self.config.clone());
        let req = probe_request(profile);
        probe_with(bound.stream(req, CancellationToken::new())).await
    }
}

// ---------------------------------------------------------------------------------------
// response side

#[derive(Debug, Default, Clone)]
struct RawUsage {
    input: Option<u64>,
    output: Option<u64>,
    cache_read: Option<u64>,
    cache_write: Option<u64>,
    thinking: Option<u64>,
}

impl RawUsage {
    fn overlay_json(&mut self, u: &Value) {
        let n = |v: &Value, k: &str| v.get(k).and_then(Value::as_u64);
        let set = |dst: &mut Option<u64>, v: Option<u64>| {
            if v.is_some() {
                *dst = v;
            }
        };
        set(&mut self.input, n(u, "input_tokens"));
        set(&mut self.output, n(u, "output_tokens"));
        set(&mut self.cache_read, n(u, "cache_read_input_tokens"));
        let write = n(u, "cache_creation_input_tokens").or_else(|| {
            let c = u.get("cache_creation")?;
            let parts = [
                n(c, "ephemeral_5m_input_tokens"),
                n(c, "ephemeral_1h_input_tokens"),
            ];
            parts
                .iter()
                .any(Option::is_some)
                .then(|| parts.iter().flatten().sum())
        });
        set(&mut self.cache_write, write);
        set(
            &mut self.thinking,
            u.get("output_tokens_details")
                .and_then(|d| n(d, "thinking_tokens")),
        );
    }

    fn snapshot(&self) -> Usage {
        Usage {
            input_tokens: self
                .input
                .map(|i| i + self.cache_read.unwrap_or(0) + self.cache_write.unwrap_or(0)),
            output_tokens: self.output,
            cache_read_tokens: self.cache_read,
            cache_write_tokens: self.cache_write,
            reasoning_tokens: self.thinking,
        }
    }
}

enum Block {
    Text,
    Tool {
        call_id: ToolCallId,
        start_input: Option<Value>,
    },
    Thinking {
        thinking: String,
        signature: String,
    },
    Redacted {
        data: Value,
    },
    Ignored,
}

struct AnthropicStream {
    tools: Vec<ToolSpec>,
    request_id: RequestId,
    used_ids: HashSet<ToolCallId>,
    secret: Option<String>,
    header_request_id: Option<String>,
    assembler: ToolCallAssembler,
    blocks: HashMap<u64, Block>,
    closed: HashSet<u64>,
    started: bool,
    raw: RawUsage,
    last_usage: Option<Usage>,
    stop: Option<StopReason>,
}

impl AnthropicStream {
    fn new(
        tools: Vec<ToolSpec>,
        request_id: RequestId,
        used_ids: HashSet<ToolCallId>,
        secret: Option<String>,
    ) -> Self {
        Self {
            tools,
            request_id,
            used_ids,
            secret,
            header_request_id: None,
            assembler: ToolCallAssembler::new(),
            blocks: HashMap::new(),
            closed: HashSet::new(),
            started: false,
            raw: RawUsage::default(),
            last_usage: None,
            stop: None,
        }
    }

    fn emit_usage(&mut self, out: &mut Vec<ModelEvent>) {
        let snap = self.raw.snapshot();
        if !snap.is_unknown() && self.last_usage.as_ref() != Some(&snap) {
            self.last_usage = Some(snap.clone());
            out.push(ModelEvent::Usage(snap));
        }
    }

    fn malformed(&self, what: &str) -> ModelEvent {
        ModelEvent::Failed {
            error: ModelError::new(ModelErrorKind::MalformedStream, what.to_owned())
                .with_request_id(self.header_request_id.clone()),
        }
    }

    fn mid_stream_error(&self, v: &Value) -> ModelError {
        let err = v.get("error").unwrap_or(&Value::Null);
        let mut e = classify_error(
            None,
            err.get("type").and_then(Value::as_str),
            err.get("message").and_then(Value::as_str),
            err.pointer("/details/error_code").and_then(Value::as_str),
            self.secret.as_deref(),
        );
        e.request_id = v
            .get("request_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| self.header_request_id.clone());
        e
    }

    fn block_start(&mut self, v: &Value, out: &mut Vec<ModelEvent>) {
        let Some(index) = v.get("index").and_then(Value::as_u64) else {
            return;
        };
        if self.blocks.contains_key(&index) || self.closed.contains(&index) {
            return; // duplicate start
        }
        let cb = v.get("content_block").unwrap_or(&Value::Null);
        let block = match cb.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(t) = cb
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|t| !t.is_empty())
                {
                    out.push(ModelEvent::TextDelta { text: t.to_owned() });
                }
                Block::Text
            }
            Some("tool_use") => {
                let call_id = stable_call_id(
                    cb.get("id").and_then(Value::as_str),
                    &self.used_ids,
                    &self.request_id,
                    index,
                );
                self.used_ids.insert(call_id.clone());
                let name = cb.get("name").and_then(Value::as_str);
                self.assembler.push_fragment(&call_id, name, "");
                let start_input = cb
                    .get("input")
                    .filter(|i| i.as_object().is_some_and(|o| !o.is_empty()))
                    .cloned();
                if name.is_some() {
                    out.push(ModelEvent::ToolCallDelta {
                        call_id: call_id.clone(),
                        name: name.map(str::to_owned),
                        arguments_fragment: String::new(),
                    });
                }
                Block::Tool {
                    call_id,
                    start_input,
                }
            }
            Some("thinking") => Block::Thinking {
                thinking: cb
                    .get("thinking")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                signature: cb
                    .get("signature")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            },
            Some("redacted_thinking") => Block::Redacted {
                data: cb.get("data").cloned().unwrap_or(Value::Null),
            },
            _ => Block::Ignored,
        };
        self.blocks.insert(index, block);
    }

    fn block_delta(&mut self, v: &Value, out: &mut Vec<ModelEvent>) {
        let Some(index) = v.get("index").and_then(Value::as_u64) else {
            return;
        };
        let delta = v.get("delta").unwrap_or(&Value::Null);
        let Some(block) = self.blocks.get_mut(&index) else {
            return; // delta for a block that never started or is closed
        };
        match (delta.get("type").and_then(Value::as_str), block) {
            (Some("text_delta"), Block::Text) => {
                if let Some(t) = delta
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|t| !t.is_empty())
                {
                    out.push(ModelEvent::TextDelta { text: t.to_owned() });
                }
            }
            (Some("input_json_delta"), Block::Tool { call_id, .. }) => {
                let frag = delta
                    .get("partial_json")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if self.assembler.push_fragment(call_id, None, frag) {
                    out.push(ModelEvent::ToolCallDelta {
                        call_id: call_id.clone(),
                        name: None,
                        arguments_fragment: frag.to_owned(),
                    });
                }
            }
            (Some("thinking_delta"), Block::Thinking { thinking, .. }) => {
                if let Some(t) = delta.get("thinking").and_then(Value::as_str) {
                    thinking.push_str(t);
                    if !t.is_empty() {
                        out.push(ModelEvent::ReasoningDelta { text: t.to_owned() });
                    }
                }
            }
            (Some("signature_delta"), Block::Thinking { signature, .. }) => {
                if let Some(s) = delta.get("signature").and_then(Value::as_str) {
                    signature.push_str(s);
                }
            }
            _ => {} // citations_delta and future delta types
        }
    }

    fn block_stop(&mut self, v: &Value, out: &mut Vec<ModelEvent>) {
        let Some(index) = v.get("index").and_then(Value::as_u64) else {
            return;
        };
        let Some(block) = self.blocks.remove(&index) else {
            return; // duplicate stop
        };
        self.closed.insert(index);
        match block {
            Block::Tool {
                call_id,
                start_input,
            } => {
                if let Some(ev) =
                    self.assembler
                        .finish_with(&call_id, &self.tools, start_input.as_ref())
                {
                    out.push(ev);
                }
            }
            Block::Thinking {
                thinking,
                signature,
            } => {
                // an unsigned block (cut off before the signature) cannot be replayed
                if !signature.is_empty() {
                    out.push(ModelEvent::ProviderState {
                        kind: "anthropic.thinking".into(),
                        data: json!({"type":"thinking","thinking":thinking,"signature":signature}),
                    });
                }
            }
            Block::Redacted { data } => out.push(ModelEvent::ProviderState {
                kind: "anthropic.redacted_thinking".into(),
                data: json!({"type":"redacted_thinking","data":data}),
            }),
            Block::Text | Block::Ignored => {}
        }
    }
}

fn map_stop_reason(raw: &str) -> StopReason {
    match raw {
        "end_turn" => StopReason::EndTurn,
        "tool_use" => StopReason::ToolUse,
        "max_tokens" => StopReason::MaxTokens,
        "stop_sequence" => StopReason::StopSequence,
        "pause_turn" => StopReason::Paused,
        "refusal" => StopReason::Refusal,
        "model_context_window_exceeded" => StopReason::ContextExceeded,
        other => StopReason::Other {
            raw: other.to_owned(),
        },
    }
}

fn kind_for_type(t: &str) -> Option<ModelErrorKind> {
    Some(match t {
        "authentication_error" => ModelErrorKind::Auth,
        "permission_error" => ModelErrorKind::Permission,
        "billing_error" => ModelErrorKind::Billing,
        "rate_limit_error" => ModelErrorKind::RateLimited,
        "overloaded_error" => ModelErrorKind::Overloaded,
        "api_error" => ModelErrorKind::Server,
        "timeout_error" => ModelErrorKind::Timeout,
        "invalid_request_error" | "request_too_large" | "not_found_error" => {
            ModelErrorKind::InvalidRequest
        }
        _ => return None,
    })
}

/// Maps an Anthropic error (HTTP status and/or body `error` object, or a mid-stream
/// `error` event) to a [`ModelError`].
fn classify_error(
    status: Option<u16>,
    err_type: Option<&str>,
    message: Option<&str>,
    detail_code: Option<&str>,
    secret: Option<&str>,
) -> ModelError {
    let from_status = status.map(|s| match s {
        401 => ModelErrorKind::Auth,
        402 => ModelErrorKind::Billing,
        403 => ModelErrorKind::Permission,
        408 | 504 => ModelErrorKind::Timeout,
        429 => ModelErrorKind::RateLimited,
        529 => ModelErrorKind::Overloaded,
        500..=599 => ModelErrorKind::Server,
        _ => ModelErrorKind::InvalidRequest,
    });
    let mut kind = err_type
        .and_then(kind_for_type)
        .or(from_status)
        .unwrap_or(ModelErrorKind::Other);
    let text = message.unwrap_or("");
    // Spend limits arrive as 400 (R2 1.5) or as a 429 without retry-after.
    let lower = text.to_ascii_lowercase();
    if detail_code == Some("enforced_spend_limit_reached")
        || (kind == ModelErrorKind::InvalidRequest
            && (lower.contains("spend limit")
                || lower.contains("usage limit")
                || lower.contains("credit balance")))
    {
        kind = ModelErrorKind::Billing;
    } else if kind == ModelErrorKind::InvalidRequest && looks_like_context_overflow(text) {
        kind = ModelErrorKind::ContextTooLong;
    }
    // the status is authoritative for the common failures even if the type is odd
    if let Some(s) = status {
        if matches!(s, 401..=403) {
            if let Some(k) = from_status {
                kind = k;
            }
        }
    }
    let shown = if text.is_empty() {
        err_type.unwrap_or("error").to_owned()
    } else {
        scrub(text, secret)
    };
    let mut e = ModelError::new(
        kind,
        match status {
            Some(s) => format!("HTTP {s}: {shown}"),
            None => shown,
        },
    );
    e.http_status = status;
    e.provider_code = err_type
        .map(str::to_owned)
        .or_else(|| detail_code.map(str::to_owned));
    e
}

impl Protocol for AnthropicStream {
    fn begin(&mut self, headers: &[(String, String)]) {
        self.header_request_id = request_id_header(headers);
    }

    fn on_event(&mut self, event: &SseEvent, out: &mut Vec<ModelEvent>) -> bool {
        if event.data.trim().is_empty() {
            return false;
        }
        let Ok(v) = serde_json::from_str::<Value>(&event.data) else {
            out.push(self.malformed("a stream event was not valid JSON"));
            return true;
        };
        let kind = v
            .get("type")
            .and_then(Value::as_str)
            .or(event.event.as_deref())
            .unwrap_or("");
        match kind {
            "message_start" => {
                let m = v.get("message").unwrap_or(&Value::Null);
                if !self.started {
                    self.started = true;
                    out.push(ModelEvent::ResponseStarted {
                        request_id: self.header_request_id.clone(),
                        response_id: m.get("id").and_then(Value::as_str).map(str::to_owned),
                        model: m.get("model").and_then(Value::as_str).map(str::to_owned),
                    });
                }
                if let Some(u) = m.get("usage") {
                    // `output_tokens` here is a provisional count (R2 1.4); the cumulative
                    // value arrives with `message_delta`, so the early one is not reported.
                    let mut early = u.clone();
                    if let Some(o) = early.as_object_mut() {
                        o.remove("output_tokens");
                    }
                    self.raw.overlay_json(&early);
                    self.emit_usage(out);
                }
            }
            "content_block_start" => self.block_start(&v, out),
            "content_block_delta" => self.block_delta(&v, out),
            "content_block_stop" => self.block_stop(&v, out),
            "message_delta" => {
                if let Some(r) = v.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.stop = Some(map_stop_reason(r));
                }
                if let Some(u) = v.get("usage") {
                    self.raw.overlay_json(u);
                    self.emit_usage(out);
                }
            }
            "message_stop" => {
                let Some(stop) = self.stop.take() else {
                    out.push(self.malformed("message_stop arrived without a stop_reason"));
                    return true;
                };
                if self.assembler.has_incomplete() {
                    if stop.is_truncation() {
                        out.extend(
                            self.assembler
                                .reject_incomplete("the output limit was reached"),
                        );
                    } else {
                        out.push(
                            self.malformed("the message ended while a tool call was still open"),
                        );
                        return true;
                    }
                }
                out.push(ModelEvent::Completed { stop_reason: stop });
                return true;
            }
            "error" => {
                let error = self.mid_stream_error(&v);
                out.push(ModelEvent::Failed { error });
                return true;
            }
            _ => {} // ping and event types added later
        }
        false
    }

    fn on_eof(&mut self, out: &mut Vec<ModelEvent>) {
        out.push(ModelEvent::Failed {
            error: ModelError::new(
                ModelErrorKind::Network,
                "the stream ended before message_stop",
            )
            .with_request_id(self.header_request_id.clone()),
        });
    }

    fn http_error(
        &self,
        status: u16,
        headers: &[(String, String)],
        body: &[u8],
        now: Timestamp,
    ) -> ModelError {
        let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        let err = v.get("error").unwrap_or(&Value::Null);
        let mut e = if (200..300).contains(&status) && err.is_null() {
            ModelError::new(
                ModelErrorKind::MalformedStream,
                "the server answered with something other than an event stream",
            )
            .with_status(status)
        } else if (300..400).contains(&status) {
            ModelError::new(
                ModelErrorKind::InvalidRequest,
                format!("HTTP {status}: the endpoint redirected; redirects are not followed, check base_url"),
            )
            .with_status(status)
        } else {
            let message = err
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| {
                    (!body.is_empty() && err.is_null())
                        .then(|| String::from_utf8_lossy(body).chars().take(200).collect())
                });
            classify_error(
                Some(status),
                err.get("type").and_then(Value::as_str),
                message.as_deref(),
                err.pointer("/details/error_code").and_then(Value::as_str),
                self.secret.as_deref(),
            )
        };
        e.request_id = v
            .get("request_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| request_id_header(headers));
        if e.kind == ModelErrorKind::RateLimited || e.kind == ModelErrorKind::Overloaded {
            e.retry_after = parse_retry_after(headers, now);
        }
        e
    }
}
