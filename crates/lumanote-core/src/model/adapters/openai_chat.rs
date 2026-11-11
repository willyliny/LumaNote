//! Adapter for OpenAI Chat Completions (`POST {base}/chat/completions`, `data:`-only SSE),
//! also used for "OpenAI-compatible" cloud services and local servers (Ollama, vLLM,
//! LM Studio, ...). Wire facts: R2 section 3.
//!
//! Tolerated server variations: a whole tool call in one chunk; a missing or colliding
//! tool-call id (a stable local id is made up); no usage chunk; unknown extra fields;
//! `finish_reason: "stop"` together with tool calls; several calls arriving in one chunk;
//! servers that reuse `index` 0 for successive calls; `reasoning_content` deltas.

use std::collections::HashSet;
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

/// Which request parameter limits the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChatTokenParam {
    /// `max_completion_tokens` (OpenAI; `max_tokens` is deprecated there).
    #[default]
    MaxCompletionTokens,
    /// `max_tokens` (older compatible servers).
    MaxTokens,
}

/// Role used for the system prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChatSystemRole {
    /// `system`, understood everywhere.
    #[default]
    System,
    /// `developer`, for OpenAI reasoning models.
    Developer,
}

/// Tunables of [`OpenAiChatAdapter`].
#[derive(Debug, Clone)]
pub struct OpenAiChatConfig {
    /// Allow plain `http` to loopback hosts (local servers).
    pub allow_loopback_http: bool,
    /// Default idle timeout (a request may carry its own).
    pub idle_timeout: Duration,
    /// Output-limit parameter name.
    pub max_tokens_param: ChatTokenParam,
    /// Send `stream_options.include_usage` (skipped when the profile says the server
    /// does not report usage).
    pub request_usage: bool,
    /// System prompt role.
    pub system_role: ChatSystemRole,
}

impl Default for OpenAiChatConfig {
    fn default() -> Self {
        Self {
            allow_loopback_http: false,
            idle_timeout: Duration::from_secs(120),
            max_tokens_param: ChatTokenParam::MaxCompletionTokens,
            request_usage: true,
            system_role: ChatSystemRole::System,
        }
    }
}

/// Adapter for one Chat Completions profile.
pub struct OpenAiChatAdapter {
    profile: ModelProfile,
    deps: AdapterDeps,
    config: OpenAiChatConfig,
}

impl std::fmt::Debug for OpenAiChatAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiChatAdapter")
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

const EXTRA_KIND: &str = "openai.tool_call_extra";

impl OpenAiChatAdapter {
    /// Creates an adapter bound to `profile`.
    pub fn new(profile: ModelProfile, deps: AdapterDeps, config: OpenAiChatConfig) -> Self {
        Self {
            profile,
            deps,
            config,
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
        let origin = ModelOrigin::from_profile(&self.profile);
        // provider extras (Gemini thought signatures) by tool call id, replayed to the same origin
        let extras: Vec<(&str, &Value)> = req
            .messages
            .iter()
            .flat_map(|m| m.parts.iter())
            .filter_map(|p| match p {
                MessagePart::ProviderState {
                    origin: o,
                    kind,
                    data,
                } if *o == origin && kind == EXTRA_KIND => {
                    Some((data.get("call_id")?.as_str()?, data.get("extra_content")?))
                }
                _ => None,
            })
            .collect();

        let mut messages: Vec<Value> = Vec::new();
        if !req.instructions.trim().is_empty() {
            let role = match self.config.system_role {
                ChatSystemRole::System => "system",
                ChatSystemRole::Developer => "developer",
            };
            messages.push(json!({"role": role, "content": req.instructions}));
        }
        for msg in &req.messages {
            match msg.role {
                Role::System => {
                    let t = joined_text(&msg.parts);
                    if !t.trim().is_empty() {
                        messages.push(json!({"role":"system","content":t}));
                    }
                }
                Role::User => {
                    let mut parts: Vec<Value> = Vec::new();
                    let mut has_image = false;
                    for p in &msg.parts {
                        match p {
                            MessagePart::Text { text } => {
                                parts.push(json!({"type":"text","text":text}))
                            }
                            MessagePart::ImageRef { artifact_id, mime } => {
                                if caps.image_input != Support::Supported {
                                    return Err(unsupported(
                                        "image input is not supported by this model profile",
                                    ));
                                }
                                let data = req.image_data.get(artifact_id).ok_or_else(|| {
                                    invalid(format!(
                                        "no image bytes supplied for artifact {artifact_id}"
                                    ))
                                })?;
                                let m = if data.mime.is_empty() {
                                    mime
                                } else {
                                    &data.mime
                                };
                                has_image = true;
                                parts.push(json!({"type":"image_url","image_url":{
                                    "url": format!("data:{m};base64,{}", data.base64)}}));
                            }
                            MessagePart::ProviderState { .. } => {}
                            _ => {
                                return Err(invalid(
                                    "user messages can only contain text and images",
                                ))
                            }
                        }
                    }
                    if parts.is_empty() {
                        continue;
                    }
                    if has_image {
                        messages.push(json!({"role":"user","content":parts}));
                    } else {
                        messages.push(json!({"role":"user","content":joined_text(&msg.parts)}));
                    }
                }
                Role::Assistant => {
                    let text = joined_text(&msg.parts);
                    let mut calls = Vec::new();
                    for p in &msg.parts {
                        match p {
                            MessagePart::ToolCall {
                                id,
                                name,
                                arguments,
                            } => {
                                let mut call = json!({
                                    "id": id.as_str(),
                                    "type": "function",
                                    "function": {
                                        "name": name,
                                        "arguments": if arguments.is_object() { arguments.to_string() } else { "{}".to_owned() },
                                    },
                                });
                                if let Some((_, extra)) =
                                    extras.iter().find(|(c, _)| *c == id.as_str())
                                {
                                    call["extra_content"] = (*extra).clone();
                                }
                                calls.push(call);
                            }
                            MessagePart::Text { .. } | MessagePart::ProviderState { .. } => {}
                            _ => {
                                return Err(invalid(
                                    "assistant messages cannot contain images or tool results",
                                ))
                            }
                        }
                    }
                    if text.is_empty() && calls.is_empty() {
                        continue;
                    }
                    let mut m = Map::new();
                    m.insert("role".into(), json!("assistant"));
                    m.insert(
                        "content".into(),
                        if text.is_empty() {
                            Value::Null
                        } else {
                            json!(text)
                        },
                    );
                    if !calls.is_empty() {
                        m.insert("tool_calls".into(), Value::Array(calls));
                    }
                    messages.push(Value::Object(m));
                }
                Role::Tool => {
                    for p in &msg.parts {
                        match p {
                            MessagePart::ToolResult {
                                call_id, content, ..
                            } => messages.push(json!({
                                "role":"tool","tool_call_id": call_id.as_str(), "content": content,
                            })),
                            MessagePart::ProviderState { .. } => {}
                            _ => {
                                return Err(invalid("tool messages can only contain tool results"))
                            }
                        }
                    }
                }
            }
        }
        if !messages
            .iter()
            .any(|m| m["role"] == "user" || m["role"] == "tool")
        {
            return Err(invalid("the conversation has no user message"));
        }

        let mut body = Map::new();
        body.insert("model".into(), json!(self.profile.model_id));
        body.insert("stream".into(), json!(true));
        if self.config.request_usage && caps.usage_in_stream != Support::Unsupported {
            body.insert("stream_options".into(), json!({"include_usage": true}));
        }
        body.insert("messages".into(), Value::Array(messages));
        if let Some(limit) = req.output_budget {
            let limit = caps.max_output_tokens.map_or(limit, |cap| limit.min(cap));
            let key = match self.config.max_tokens_param {
                ChatTokenParam::MaxCompletionTokens => "max_completion_tokens",
                ChatTokenParam::MaxTokens => "max_tokens",
            };
            body.insert(key.into(), json!(limit));
        }
        if !req.tools.is_empty() {
            let mut tools = Vec::new();
            for t in &req.tools {
                if !t.parameters_schema.is_object() {
                    return Err(invalid(format!(
                        "schema of tool `{}` is not a JSON object",
                        t.name
                    )));
                }
                tools.push(json!({"type":"function","function":{
                    "name": t.name, "description": t.description, "parameters": t.parameters_schema}}));
            }
            body.insert("tools".into(), Value::Array(tools));
            let mode = req.tool_choice.mode();
            if !caps.allows_tool_choice(mode) {
                return Err(unsupported(format!(
                    "tool_choice `{mode}` is not supported by this model profile"
                )));
            }
            match &req.tool_choice {
                ToolChoice::Auto => {}
                ToolChoice::None => {
                    body.insert("tool_choice".into(), json!("none"));
                }
                ToolChoice::Required => {
                    body.insert("tool_choice".into(), json!("required"));
                }
                ToolChoice::Tool { name } => {
                    if !req.tools.iter().any(|t| &t.name == name) {
                        return Err(invalid(format!("tool_choice names unknown tool `{name}`")));
                    }
                    body.insert(
                        "tool_choice".into(),
                        json!({"type":"function","function":{"name":name}}),
                    );
                }
            }
            if req.parallel_tool_calls == Some(false) && caps.parallel_tools != Support::Unsupported
            {
                body.insert("parallel_tool_calls".into(), json!(false));
            }
        } else if req.tool_choice.mode() != ToolChoiceMode::Auto {
            return Err(invalid("tool_choice was set but no tools are offered"));
        }
        if !req.stop_sequences.is_empty() {
            if req.stop_sequences.len() > 4 {
                return Err(invalid("at most 4 stop sequences are supported"));
            }
            body.insert("stop".into(), json!(req.stop_sequences));
        }
        if let Some(r) = &req.reasoning {
            if caps.reasoning != Support::Supported {
                return Err(unsupported(
                    "reasoning settings are not supported by this model profile",
                ));
            }
            if r.budget_tokens.is_some() {
                return Err(unsupported("this protocol has no thinking-token budget"));
            }
            if let Some(e) = r.effort {
                body.insert("reasoning_effort".into(), json!(e.as_str()));
            }
        }
        if let Some(schema) = &req.output_contract {
            if caps.structured_output != Support::Supported {
                return Err(unsupported(
                    "structured output is not supported by this model profile",
                ));
            }
            body.insert(
                "response_format".into(),
                json!({"type":"json_schema","json_schema":{"name":"result","schema":schema,"strict":true}}),
            );
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
        ];
        if let Some(key) = &secret {
            headers.push(("authorization".to_owned(), format!("Bearer {key}")));
        }
        let bytes = serde_json::to_vec(&body)
            .map_err(|e| invalid(format!("cannot serialise the request: {e}")))?;
        Ok((
            HttpRequest {
                method: "POST".to_owned(),
                url: join_path(&base, "chat/completions"),
                headers,
                body: Some(bytes),
            },
            secret,
        ))
    }
}

#[async_trait::async_trait]
impl ModelAdapter for OpenAiChatAdapter {
    fn capabilities(&self, profile: &ModelProfile) -> ModelCapabilities {
        profile.capabilities.clone()
    }

    fn validate_config(&self, profile: &ModelProfile) -> ValidationResult {
        let mut issues = Vec::new();
        if profile.protocol != PROTOCOL_OPENAI_CHAT {
            issues.push(format!(
                "protocol `{}` is not `{PROTOCOL_OPENAI_CHAT}`",
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
        let protocol = ChatStream::new(
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

/// One tool call being assembled.
struct Slot {
    index: u64,
    id: ToolCallId,
    provider_id: Option<String>,
    extra: Option<Value>,
    finished: bool,
}

struct ChatStream {
    tools: Vec<ToolSpec>,
    request_id: RequestId,
    used_ids: HashSet<ToolCallId>,
    secret: Option<String>,
    header_request_id: Option<String>,
    assembler: ToolCallAssembler,
    slots: Vec<Slot>,
    started: bool,
    usage: Usage,
    last_usage: Option<Usage>,
    finish: Option<String>,
    calls_seen: usize,
    refused: bool,
    calls_emitted: bool,
}

impl ChatStream {
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
            slots: Vec::new(),
            started: false,
            usage: Usage::default(),
            last_usage: None,
            finish: None,
            calls_seen: 0,
            refused: false,
            calls_emitted: false,
        }
    }

    fn malformed(&self, what: &str) -> ModelEvent {
        ModelEvent::Failed {
            error: ModelError::new(ModelErrorKind::MalformedStream, what.to_owned())
                .with_request_id(self.header_request_id.clone()),
        }
    }

    fn overlay_usage(&mut self, u: &Value, out: &mut Vec<ModelEvent>) {
        if !u.is_object() {
            return;
        }
        let n = |v: &Value, p: &str| v.pointer(p).and_then(Value::as_u64);
        let newer = Usage {
            input_tokens: n(u, "/prompt_tokens"),
            output_tokens: n(u, "/completion_tokens"),
            cache_read_tokens: n(u, "/prompt_tokens_details/cached_tokens"),
            cache_write_tokens: n(u, "/prompt_tokens_details/cache_write_tokens"),
            reasoning_tokens: n(u, "/completion_tokens_details/reasoning_tokens"),
        };
        self.usage.overlay(&newer);
        if !self.usage.is_unknown() && self.last_usage.as_ref() != Some(&self.usage) {
            self.last_usage = Some(self.usage.clone());
            out.push(ModelEvent::Usage(self.usage.clone()));
        }
    }

    fn tool_call_delta(&mut self, tc: &Value, out: &mut Vec<ModelEvent>) {
        let index = tc.get("index").and_then(Value::as_u64).unwrap_or(0);
        let provider_id = tc
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        let func = tc.get("function").unwrap_or(&Value::Null);
        let name = func
            .get("name")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        let args = match func.get("arguments") {
            Some(Value::String(s)) => s.clone(),
            Some(v) if v.is_object() => v.to_string(),
            _ => String::new(),
        };

        // The latest open slot of this index continues; a different provider id (or no slot
        // yet, or a finished one) starts a new call.
        let reuse = self.slots.iter().rposition(|s| s.index == index).filter(|&p| {
            let slot = &self.slots[p];
            let other_id = matches!((provider_id, slot.provider_id.as_deref()), (Some(n), Some(o)) if n != o);
            !slot.finished && !other_id
        });
        let p = match reuse {
            Some(p) => p,
            None => {
                let position = self.calls_seen as u64;
                self.calls_seen += 1;
                let id = stable_call_id(provider_id, &self.used_ids, &self.request_id, position);
                self.used_ids.insert(id.clone());
                self.slots.push(Slot {
                    index,
                    id,
                    provider_id: provider_id.map(str::to_owned),
                    extra: None,
                    finished: false,
                });
                self.slots.len() - 1
            }
        };
        if let Some(extra) = tc.get("extra_content").filter(|e| !e.is_null()) {
            self.slots[p].extra = Some(extra.clone());
        }
        let id = self.slots[p].id.clone();
        if self.assembler.push_fragment(&id, name, &args) && (name.is_some() || !args.is_empty()) {
            out.push(ModelEvent::ToolCallDelta {
                call_id: id,
                name: name.map(str::to_owned),
                arguments_fragment: args,
            });
        }
    }

    /// Finishes every open call (the protocol has no per-call end marker).
    fn finish_calls(&mut self, out: &mut Vec<ModelEvent>) {
        for i in 0..self.slots.len() {
            if self.slots[i].finished {
                continue;
            }
            self.slots[i].finished = true;
            let id = self.slots[i].id.clone();
            if let Some(extra) = self.slots[i].extra.take() {
                out.push(ModelEvent::ProviderState {
                    kind: EXTRA_KIND.into(),
                    data: json!({"call_id": id.as_str(), "extra_content": extra}),
                });
            }
            if let Some(ev) = self.assembler.finish(&id, &self.tools) {
                out.push(ev);
                self.calls_emitted = true;
            }
        }
    }

    fn stop_reason(&self, finish: &str) -> StopReason {
        match finish {
            "length" => StopReason::MaxTokens,
            "content_filter" => StopReason::ContentFilter,
            // some local servers say `stop` although they returned tool calls
            _ if self.calls_emitted => StopReason::ToolUse,
            "tool_calls" | "function_call" => StopReason::ToolUse,
            "stop" if self.refused => StopReason::Refusal,
            "stop" => StopReason::EndTurn,
            other => StopReason::Other {
                raw: other.to_owned(),
            },
        }
    }

    fn error_from(&self, v: &Value) -> ModelError {
        let err = v.get("error").unwrap_or(&Value::Null);
        let message = err
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| err.as_str());
        let mut e = classify_error(
            None,
            err.get("type").and_then(Value::as_str),
            err.get("code").map(code_to_string).as_deref(),
            message,
            self.secret.as_deref(),
        );
        e.request_id = self.header_request_id.clone();
        e
    }
}

fn code_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Maps a Chat Completions error (HTTP status and/or body `error` object, or a mid-stream
/// error payload) to a [`ModelError`].
fn classify_error(
    status: Option<u16>,
    err_type: Option<&str>,
    code: Option<&str>,
    message: Option<&str>,
    secret: Option<&str>,
) -> ModelError {
    const BILLING_CODES: &[&str] = &[
        "insufficient_quota",
        "credit_balance_exhausted",
        "organization_spend_limit_exceeded",
        "project_spend_limit_exceeded",
        "organization_usage_limit_exceeded",
        "billing_hard_limit_reached",
    ];
    let text = message.unwrap_or("");
    let code_l = code.unwrap_or("").to_ascii_lowercase();
    let type_l = err_type.unwrap_or("").to_ascii_lowercase();
    let billing =
        BILLING_CODES.contains(&code_l.as_str()) || BILLING_CODES.contains(&type_l.as_str());
    let kind = if billing {
        ModelErrorKind::Billing
    } else if code_l == "context_length_exceeded"
        || looks_like_context_overflow(text) && status.is_none_or(|s| s == 400 || s == 413)
    {
        ModelErrorKind::ContextTooLong
    } else {
        match status {
            Some(401) => ModelErrorKind::Auth,
            Some(402) => ModelErrorKind::Billing,
            Some(403) => ModelErrorKind::Permission,
            Some(408) | Some(504) => ModelErrorKind::Timeout,
            Some(429) => ModelErrorKind::RateLimited,
            Some(503) => ModelErrorKind::Overloaded,
            Some(500..=599) => ModelErrorKind::Server,
            Some(_) => ModelErrorKind::InvalidRequest,
            None => match (type_l.as_str(), code_l.as_str()) {
                ("server_error", _) | (_, "server_error") => ModelErrorKind::Server,
                ("rate_limit_error", _)
                | (_, "rate_limit_exceeded")
                | ("requests", _)
                | ("tokens", _) => ModelErrorKind::RateLimited,
                ("service_unavailable_error", _) | (_, "server_is_overloaded") => {
                    ModelErrorKind::Overloaded
                }
                ("invalid_request_error", _) => ModelErrorKind::InvalidRequest,
                ("authentication_error", _) | (_, "invalid_api_key") => ModelErrorKind::Auth,
                _ => ModelErrorKind::Other,
            },
        }
    };
    let shown = if text.is_empty() {
        code.or(err_type).unwrap_or("error").to_owned()
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
    e.provider_code = code.or(err_type).map(str::to_owned);
    e
}

impl Protocol for ChatStream {
    fn begin(&mut self, headers: &[(String, String)]) {
        self.header_request_id = request_id_header(headers);
    }

    fn on_event(&mut self, event: &SseEvent, out: &mut Vec<ModelEvent>) -> bool {
        let data = event.data.trim();
        if data.is_empty() {
            return false;
        }
        if event.is_done() {
            return self.complete(out);
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            out.push(self.malformed("a stream chunk was not valid JSON"));
            return true;
        };
        if v.get("error").is_some_and(|e| !e.is_null())
            && v.get("choices")
                .is_none_or(|c| c.as_array().is_none_or(Vec::is_empty))
        {
            out.push(ModelEvent::Failed {
                error: self.error_from(&v),
            });
            return true;
        }
        if !self.started {
            self.started = true;
            out.push(ModelEvent::ResponseStarted {
                request_id: self.header_request_id.clone(),
                response_id: v.get("id").and_then(Value::as_str).map(str::to_owned),
                model: v.get("model").and_then(Value::as_str).map(str::to_owned),
            });
        }
        if let Some(u) = v.get("usage") {
            self.overlay_usage(u, out);
        }
        let choice = v.get("choices").and_then(Value::as_array).and_then(|c| {
            c.iter()
                .find(|c| c.get("index").and_then(Value::as_u64).unwrap_or(0) == 0)
        });
        let Some(choice) = choice else {
            return false; // usage-only chunk or a keep-alive
        };
        if let Some(delta) = choice.get("delta") {
            for key in ["reasoning_content", "reasoning"] {
                if let Some(t) = delta
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|t| !t.is_empty())
                {
                    out.push(ModelEvent::ReasoningDelta { text: t.to_owned() });
                }
            }
            if let Some(t) = delta
                .get("content")
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
            {
                out.push(ModelEvent::TextDelta { text: t.to_owned() });
            }
            if let Some(t) = delta
                .get("refusal")
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
            {
                self.refused = true;
                out.push(ModelEvent::TextDelta { text: t.to_owned() });
            }
            if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                for tc in calls {
                    self.tool_call_delta(tc, out);
                }
            }
        }
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            if reason == "error" {
                out.push(ModelEvent::Failed {
                    error: if v.get("error").is_some() {
                        self.error_from(&v)
                    } else if let Some(e) = choice.get("error") {
                        self.error_from(&json!({"error": e}))
                    } else {
                        ModelError::new(
                            ModelErrorKind::Server,
                            "the server reported finish_reason `error`",
                        )
                        .with_request_id(self.header_request_id.clone())
                    },
                });
                return true;
            }
            if self.finish.is_none() {
                self.finish = Some(reason.to_owned());
                self.finish_calls(out);
            }
        }
        false
    }

    fn on_eof(&mut self, out: &mut Vec<ModelEvent>) {
        // A server that ends the stream without `[DONE]` is tolerated once it has said why
        // it stopped; without a finish_reason the answer may be cut off.
        if self.finish.is_some() {
            self.complete(out);
        } else {
            out.push(ModelEvent::Failed {
                error: ModelError::new(
                    ModelErrorKind::Network,
                    "the stream ended before a finish_reason arrived",
                )
                .with_request_id(self.header_request_id.clone()),
            });
        }
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
                .or_else(|| err.as_str())
                .or_else(|| v.get("message").and_then(Value::as_str))
                .map(str::to_owned)
                .or_else(|| {
                    (!body.is_empty() && err.is_null())
                        .then(|| String::from_utf8_lossy(body).chars().take(200).collect())
                });
            classify_error(
                Some(status),
                err.get("type").and_then(Value::as_str),
                err.get("code").map(code_to_string).as_deref(),
                message.as_deref(),
                self.secret.as_deref(),
            )
        };
        e.request_id = request_id_header(headers);
        if e.kind == ModelErrorKind::RateLimited || e.kind == ModelErrorKind::Overloaded {
            e.retry_after = parse_retry_after(headers, now);
        }
        e
    }
}

impl ChatStream {
    fn complete(&mut self, out: &mut Vec<ModelEvent>) -> bool {
        let Some(finish) = self.finish.clone() else {
            out.push(self.malformed("the stream ended without a finish_reason"));
            return true;
        };
        self.finish_calls(out);
        out.push(ModelEvent::Completed {
            stop_reason: self.stop_reason(&finish),
        });
        true
    }
}
