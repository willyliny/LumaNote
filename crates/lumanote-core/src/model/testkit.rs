//! Test support shared by the adapter tests and the orchestrator-level protocol tests:
//! profiles, rigs over the replay transport, and builders for provider event streams.

use std::sync::Arc;

use futures::StreamExt;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use super::adapters::{
    AdapterDeps, AnthropicAdapter, AnthropicConfig, OpenAiChatAdapter, OpenAiChatConfig,
    PROTOCOL_ANTHROPIC, PROTOCOL_OPENAI_CHAT,
};
use super::contracts::*;
use crate::ports::fakes::{FakeClock, InMemoryCredentialStore, ReplayTransport};
use crate::ports::CredentialStore;

/// The API key used by every test; tests grep outputs for it.
pub(crate) const SECRET: &str = "sk-test-SECRET-0123456789abcdef";

pub(crate) fn full_caps() -> ModelCapabilities {
    let mut c = ModelCapabilities::unverified();
    c.text = Support::Supported;
    c.streaming = Support::Supported;
    c.tool_calling = Support::Supported;
    c.structured_output = Support::Supported;
    c.image_input = Support::Supported;
    c.parallel_tools = Support::Supported;
    c.usage_in_stream = Support::Supported;
    c.reasoning = Support::Supported;
    c.tool_choice_modes = vec![
        ToolChoiceMode::None,
        ToolChoiceMode::Required,
        ToolChoiceMode::Tool,
    ];
    c.max_output_tokens = Some(4096);
    c
}

pub(crate) fn anthropic_profile() -> ModelProfile {
    let mut caps = full_caps();
    caps.requires_opaque_roundtrip = true;
    ModelProfile {
        id: "claude-main".into(),
        provider: "anthropic".into(),
        protocol: PROTOCOL_ANTHROPIC.into(),
        base_url: "https://api.anthropic.com".into(),
        model_id: "claude-test-1".into(),
        credential_ref: Some("anthropic-key".into()),
        capabilities: caps,
        config_version: 1,
    }
}

pub(crate) fn chat_profile() -> ModelProfile {
    ModelProfile {
        id: "gpt-main".into(),
        provider: "openai".into(),
        protocol: PROTOCOL_OPENAI_CHAT.into(),
        base_url: "https://api.openai.com/v1".into(),
        model_id: "gpt-test-1".into(),
        credential_ref: Some("openai-key".into()),
        capabilities: full_caps(),
        config_version: 1,
    }
}

/// A local OpenAI-compatible server: loopback http, no key, little declared.
pub(crate) fn local_profile() -> ModelProfile {
    let mut caps = ModelCapabilities::unverified();
    caps.text = Support::Supported;
    caps.streaming = Support::Supported;
    caps.tool_calling = Support::Supported;
    caps.usage_in_stream = Support::Unsupported;
    ModelProfile {
        id: "local-qwen".into(),
        provider: "ollama".into(),
        protocol: PROTOCOL_OPENAI_CHAT.into(),
        base_url: "http://localhost:11434/v1".into(),
        model_id: "qwen3:8b".into(),
        credential_ref: None,
        capabilities: caps,
        config_version: 1,
    }
}

pub(crate) struct Rig {
    pub transport: Arc<ReplayTransport>,
    pub creds: Arc<InMemoryCredentialStore>,
    pub clock: FakeClock,
    pub adapter: Arc<dyn ModelAdapter>,
    pub profile: ModelProfile,
}

impl Rig {
    pub fn deps(&self) -> AdapterDeps {
        AdapterDeps {
            transport: self.transport.clone(),
            credentials: self.creds.clone(),
            clock: Arc::new(self.clock.clone()),
        }
    }

    pub fn request(&self, text: &str) -> ModelRequest {
        let mut r = ModelRequest::new(
            self.profile.id.clone(),
            "You are a test assistant.",
            vec![Message::text(Role::User, text)],
        );
        r.tools = test_tools();
        r
    }

    pub async fn run(&self, req: ModelRequest) -> Vec<ModelEvent> {
        self.adapter
            .stream(req, CancellationToken::new())
            .collect()
            .await
    }
}

fn rig_for(profile: ModelProfile, build: impl FnOnce(&Rig) -> Arc<dyn ModelAdapter>) -> Rig {
    let creds = Arc::new(InMemoryCredentialStore::new());
    if let Some(r) = &profile.credential_ref {
        creds.set(r, SECRET).unwrap();
    }
    let mut rig = Rig {
        transport: Arc::new(ReplayTransport::new()),
        creds,
        clock: FakeClock::at_default_start(),
        adapter: Arc::new(super::mock::MockAdapter::new(
            Arc::new(FakeClock::at_default_start()),
            vec![],
        )),
        profile,
    };
    rig.adapter = build(&rig);
    rig
}

pub(crate) fn anthropic_rig() -> Rig {
    anthropic_rig_with(anthropic_profile(), AnthropicConfig::default())
}

pub(crate) fn anthropic_rig_with(profile: ModelProfile, config: AnthropicConfig) -> Rig {
    let p = profile.clone();
    rig_for(profile, move |r| {
        Arc::new(AnthropicAdapter::new(p, r.deps(), config))
    })
}

pub(crate) fn chat_rig() -> Rig {
    chat_rig_with(chat_profile(), OpenAiChatConfig::default())
}

pub(crate) fn chat_rig_with(profile: ModelProfile, config: OpenAiChatConfig) -> Rig {
    let p = profile.clone();
    rig_for(profile, move |r| {
        Arc::new(OpenAiChatAdapter::new(p, r.deps(), config))
    })
}

pub(crate) fn test_tools() -> Vec<ToolSpec> {
    let t = |name: &str, req: &[&str]| ToolSpec {
        name: name.into(),
        description: format!("{name} tool"),
        parameters_schema: json!({"type":"object","required":req,"properties":{}}),
    };
    vec![
        t("get_weather", &["location"]),
        t("get_time", &["timezone"]),
        t("list_todos", &[]),
        t("read_file", &["path"]),
        t("add_todo", &["title"]),
        t("create_todos", &["todos"]),
        t("write_markdown", &["path", "content"]),
    ]
}

pub(crate) fn fixture(rel: &str) -> String {
    crate::ports::fakes::read_fixture(rel).unwrap()
}

// ---- stream builders --------------------------------------------------------------------

pub(crate) enum Item<'a> {
    Thinking {
        signature: &'a str,
    },
    Text(&'a str),
    Call {
        id: &'a str,
        name: &'a str,
        args: Value,
    },
}

fn sse(event: Option<&str>, data: Value) -> String {
    match event {
        Some(e) => format!("event: {e}\ndata: {data}\n\n"),
        None => format!("data: {data}\n\n"),
    }
}

/// An Anthropic event stream for one assistant turn.
pub(crate) fn anthropic_sse(items: &[Item<'_>], stop: &str, input: u64, output: u64) -> String {
    let mut s = String::new();
    s += &sse(
        Some("message_start"),
        json!({"type":"message_start","message":{"id":"msg_test","type":"message","role":"assistant",
            "model":"claude-test-1","content":[],"stop_reason":null,"usage":{"input_tokens":input,"output_tokens":1}}}),
    );
    for (i, item) in items.iter().enumerate() {
        match item {
            Item::Thinking { signature } => {
                s += &sse(
                    Some("content_block_start"),
                    json!({"type":"content_block_start","index":i,
                    "content_block":{"type":"thinking","thinking":"","signature":""}}),
                );
                s += &sse(
                    Some("content_block_delta"),
                    json!({"type":"content_block_delta","index":i,
                    "delta":{"type":"signature_delta","signature":signature}}),
                );
            }
            Item::Text(t) => {
                s += &sse(
                    Some("content_block_start"),
                    json!({"type":"content_block_start","index":i,
                    "content_block":{"type":"text","text":""}}),
                );
                let (a, b) = t.split_at(t.len() / 2);
                for part in [a, b] {
                    s += &sse(
                        Some("content_block_delta"),
                        json!({"type":"content_block_delta","index":i,
                        "delta":{"type":"text_delta","text":part}}),
                    );
                }
            }
            Item::Call { id, name, args } => {
                s += &sse(
                    Some("content_block_start"),
                    json!({"type":"content_block_start","index":i,
                    "content_block":{"type":"tool_use","id":id,"name":name,"input":{}}}),
                );
                let raw = args.to_string();
                let (a, b) = raw.split_at(raw.len() / 2);
                for part in [a, b] {
                    s += &sse(
                        Some("content_block_delta"),
                        json!({"type":"content_block_delta","index":i,
                        "delta":{"type":"input_json_delta","partial_json":part}}),
                    );
                }
            }
        }
        s += &sse(
            Some("content_block_stop"),
            json!({"type":"content_block_stop","index":i}),
        );
    }
    s += &sse(
        Some("message_delta"),
        json!({"type":"message_delta","delta":{"stop_reason":stop,"stop_sequence":null},"usage":{"output_tokens":output}}),
    );
    s += &sse(Some("message_stop"), json!({"type":"message_stop"}));
    s
}

/// A Chat Completions event stream for one assistant turn.
pub(crate) fn chat_sse(
    text: Option<&str>,
    calls: &[(&str, &str, Value)],
    finish: &str,
    usage: Option<(u64, u64)>,
) -> String {
    let chunk = |delta: Value, finish: Value| {
        json!({"id":"chatcmpl-test","object":"chat.completion.chunk","created":1,"model":"gpt-test-1",
            "choices":[{"index":0,"delta":delta,"finish_reason":finish}]})
    };
    let mut s = sse(
        None,
        chunk(json!({"role":"assistant","content":""}), Value::Null),
    );
    if let Some(t) = text {
        let (a, b) = t.split_at(t.len() / 2);
        for part in [a, b] {
            s += &sse(None, chunk(json!({"content":part}), Value::Null));
        }
    }
    for (i, (id, name, args)) in calls.iter().enumerate() {
        let raw = args.to_string();
        let (a, b) = raw.split_at(raw.len() / 2);
        s += &sse(
            None,
            chunk(
                json!({"tool_calls":[{"index":i,"id":id,"type":"function",
            "function":{"name":name,"arguments":a}}]}),
                Value::Null,
            ),
        );
        s += &sse(
            None,
            chunk(
                json!({"tool_calls":[{"index":i,"function":{"arguments":b}}]}),
                Value::Null,
            ),
        );
    }
    s += &sse(None, chunk(json!({}), json!(finish)));
    if let Some((i, o)) = usage {
        s += &sse(
            None,
            json!({"id":"chatcmpl-test","object":"chat.completion.chunk","created":1,
            "model":"gpt-test-1","choices":[],"usage":{"prompt_tokens":i,"completion_tokens":o,"total_tokens":i+o}}),
        );
    }
    s += "data: [DONE]\n\n";
    s
}

// ---- event helpers ----------------------------------------------------------------------

pub(crate) fn texts(evs: &[ModelEvent]) -> String {
    evs.iter()
        .filter_map(|e| match e {
            ModelEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

pub(crate) fn readies(evs: &[ModelEvent]) -> Vec<(String, String, Value)> {
    evs.iter()
        .filter_map(|e| match e {
            ModelEvent::ToolCallReady {
                call_id,
                name,
                arguments,
            } => Some((call_id.to_string(), name.clone(), arguments.clone())),
            _ => None,
        })
        .collect()
}

pub(crate) fn rejections(evs: &[ModelEvent]) -> Vec<(String, Option<String>, String)> {
    evs.iter()
        .filter_map(|e| match e {
            ModelEvent::ToolCallRejected {
                call_id,
                name,
                reason,
            } => Some((call_id.to_string(), name.clone(), reason.clone())),
            _ => None,
        })
        .collect()
}

pub(crate) fn last_usage(evs: &[ModelEvent]) -> Option<Usage> {
    evs.iter()
        .filter_map(|e| match e {
            ModelEvent::Usage(u) => Some(u.clone()),
            _ => None,
        })
        .next_back()
}

pub(crate) fn failure(evs: &[ModelEvent]) -> Option<ModelError> {
    match evs.last() {
        Some(ModelEvent::Failed { error }) => Some(error.clone()),
        _ => None,
    }
}

pub(crate) fn stop(evs: &[ModelEvent]) -> Option<StopReason> {
    match evs.last() {
        Some(ModelEvent::Completed { stop_reason }) => Some(stop_reason.clone()),
        _ => None,
    }
}

/// True if no event follows a terminal event and exactly one terminal event exists.
pub(crate) fn well_terminated(evs: &[ModelEvent]) -> bool {
    let terminal =
        |e: &ModelEvent| matches!(e, ModelEvent::Completed { .. } | ModelEvent::Failed { .. });
    evs.iter().filter(|e| terminal(e)).count() == 1 && evs.last().is_some_and(terminal)
}
