//! Orchestrator behaviour driven by the model contracts: reasoning, provider state,
//! truncation, partial-output failures, retry-after, rejected calls, and the end-to-end
//! scenario through the real protocol adapters over the replay transport.

use super::*;
use crate::model::adapters::{
    AdapterDeps, AnthropicAdapter, AnthropicConfig, OpenAiChatAdapter, OpenAiChatConfig,
};
use crate::model::testkit::{
    anthropic_profile, anthropic_sse, chat_profile, chat_sse, Item, SECRET,
};
use crate::model::{ModelError, ModelErrorKind};
use crate::ports::fakes::{InMemoryCredentialStore, ReplayResponse, ReplayTransport};
use crate::ports::CredentialStore;

fn texts_of(msgs: &[StoredMessage]) -> Vec<String> {
    msgs.iter()
        .flat_map(|m| m.parts.clone())
        .filter_map(|p| match p {
            MessagePart::Text { text } => Some(text),
            _ => None,
        })
        .collect()
}

fn mock_origin() -> crate::model::ModelOrigin {
    crate::model::ModelOrigin::from_profile(&profile())
}

// ---- reasoning and provider state -----------------------------------------------------------

#[tokio::test]
async fn reasoning_goes_out_on_its_own_event_and_is_never_stored_or_shown_as_answer() {
    let h = Harness::new();
    let id = h.task("x", Value::Null);
    let (m, adapter) = model(
        &h,
        vec![vec![
            ScriptStep::Reasoning("hidden chain of thought".into()).into(),
            ScriptStep::Text("the answer".into()).into(),
            ScriptStep::Complete(StopReason::EndTurn).into(),
        ]],
    );
    let out = h.orch.run(&id, &m).await.unwrap();
    assert!(
        matches!(out, RunOutcome::Completed { .. }) || matches!(out, RunOutcome::Failed { .. }),
        "{out:?}"
    );
    let labels = h.labels();
    assert!(
        labels.contains(&"reasoning:hidden chain of thought".to_owned()),
        "{labels:?}"
    );
    assert!(labels.contains(&"text:the answer".to_owned()));
    assert!(!labels
        .iter()
        .any(|l| l.starts_with("text:") && l.contains("hidden")));
    let stored = serde_json::to_string(&h.messages(&id)).unwrap();
    assert!(
        !stored.contains("hidden chain of thought"),
        "reasoning must not be persisted: {stored}"
    );
    assert_eq!(adapter.recorded_requests().len(), 1);
}

#[tokio::test]
async fn provider_state_is_persisted_in_order_and_replayed_to_the_same_origin_only() {
    let h = Harness::new();
    let id = h.task("todo", Value::Null);
    let (m, adapter) = model(
        &h,
        vec![
            {
                let mut t: Vec<ScriptItem> = vec![
                    ScriptStep::ProviderState {
                        kind: "mock.thinking".into(),
                        data: json!({"sig": "S1"}),
                    }
                    .into(),
                    ScriptStep::Text("working".into()).into(),
                ];
                t.extend(calls(vec![call("c1", "create_todos", todos_args("one"))]));
                t
            },
            say("done"),
        ],
    );
    let out = tokio::time::timeout(Duration::from_secs(5), h.orch.run(&id, &m))
        .await
        .unwrap_or_else(|_| {
            panic!(
                "hung: {:?} reqs={}",
                h.labels(),
                adapter.recorded_requests().len()
            )
        })
        .unwrap();
    assert!(matches!(out, RunOutcome::Completed { .. }), "{out:?}");

    // stored in arrival order, before the text and the call
    let msgs = h.messages(&id);
    let assistant = msgs.iter().find(|m| m.role == Role::Assistant).unwrap();
    assert!(
        matches!(&assistant.parts[0], MessagePart::ProviderState { kind, .. } if kind == "mock.thinking"),
        "{:?}",
        assistant.parts
    );
    assert!(matches!(&assistant.parts[1], MessagePart::Text { .. }));
    assert!(matches!(&assistant.parts[2], MessagePart::ToolCall { .. }));

    // the same origin gets it back, in place
    let second = &adapter.recorded_requests()[1];
    let replayed = second
        .messages
        .iter()
        .find(|m| m.role == Role::Assistant)
        .unwrap();
    assert!(
        matches!(&replayed.parts[0], MessagePart::ProviderState { origin, data, .. } if *origin == mock_origin() && data["sig"] == "S1")
    );

    // another origin does not: the orchestrator filters before the adapter sees it
    let mut other = profile();
    other.id = "p2".into();
    other.provider = "elsewhere".into();
    let conv = h.task_row(&id).conversation_id.unwrap();
    let history = h.orch.store().with(|s| s.list_messages(&conv)).unwrap();
    let rebuilt =
        super::history::to_messages(&history, &crate::model::ModelOrigin::from_profile(&other));
    let flat = serde_json::to_string(&rebuilt).unwrap();
    assert!(!flat.contains("mock.thinking"), "{flat}");
    assert!(flat.contains("working"), "the visible text still travels");
}

// ---- truncation and refusal are not success --------------------------------------------------

#[tokio::test]
async fn max_tokens_and_context_exceeded_fail_the_task_and_store_nothing() {
    for (stop, needle) in [
        (StopReason::MaxTokens, "model_output_truncated"),
        (StopReason::ContextExceeded, "model_context_exceeded"),
    ] {
        let h = Harness::new();
        let id = h.task("x", Value::Null);
        let (m, _) = model(
            &h,
            vec![vec![
                ScriptStep::Text("half an ans".into()).into(),
                ScriptStep::Complete(stop.clone()).into(),
            ]],
        );
        let out = h.orch.run(&id, &m).await.unwrap();
        assert!(
            matches!(&out, RunOutcome::Failed { reason } if reason.contains(needle)),
            "{stop:?}: {out:?}"
        );
        assert_eq!(h.task_row(&id).status, TaskStatus::Failed);
        assert!(
            texts_of(&h.messages(&id))
                .iter()
                .all(|t| !t.contains("half an ans")),
            "truncated text is not stored as the answer"
        );
    }
}

#[tokio::test]
async fn a_cut_tool_call_is_never_executed() {
    let h = Harness::new();
    let id = h.task("x", Value::Null);
    let cid = ToolCallId::from_string("cut");
    let (m, _) = model(
        &h,
        vec![vec![
            ScriptStep::ToolFragment {
                call_id: cid,
                name: Some("create_todos".into()),
                fragment: "{\"todos\":[{\"title\":\"hal".into(),
            }
            .into(),
            ScriptStep::Complete(StopReason::MaxTokens).into(),
        ]],
    );
    let out = h.orch.run(&id, &m).await.unwrap();
    assert!(matches!(out, RunOutcome::Failed { .. }), "{out:?}");
    assert!(h.todos().is_empty());
    assert!(h.steps(&id).is_empty());
}

#[tokio::test]
async fn a_refusal_without_calls_fails_instead_of_completing() {
    let h = Harness::new();
    let id = h.task("x", Value::Null);
    let (m, _) = model(
        &h,
        vec![vec![
            ScriptStep::Text("no".into()).into(),
            ScriptStep::Complete(StopReason::Refusal).into(),
        ]],
    );
    let out = h.orch.run(&id, &m).await.unwrap();
    assert!(matches!(out, RunOutcome::Failed { .. }), "{out:?}");
}

// ---- failure policy ----------------------------------------------------------------------------

fn fail_turn(text: &str, error: ModelError) -> Vec<ScriptItem> {
    let mut v: Vec<ScriptItem> = Vec::new();
    if !text.is_empty() {
        v.push(ScriptStep::Text(text.into()).into());
    }
    v.push(ScriptStep::Fail(error).into());
    v
}

#[tokio::test]
async fn a_stream_that_failed_after_partial_output_is_not_replayed() {
    let h = Harness::new();
    let id = h.task("x", Value::Null);
    let net = ModelError::new(ModelErrorKind::Network, "reset");
    let (m, adapter) = model(
        &h,
        vec![fail_turn("partial ", net), say("would be a duplicate")],
    );
    let out = h.orch.run(&id, &m).await.unwrap();
    assert_eq!(
        out,
        RunOutcome::Paused {
            reason: PauseReason::ModelUnavailable
        },
        "{out:?}"
    );
    assert_eq!(
        adapter.recorded_requests().len(),
        1,
        "no automatic replay after output was shown"
    );
    assert!(!h.labels().iter().any(|l| l.starts_with("retry:")));
    assert!(texts_of(&h.messages(&id))
        .iter()
        .all(|t| !t.contains("partial")));
}

#[tokio::test]
async fn billing_and_auth_errors_are_not_retried() {
    for kind in [
        ModelErrorKind::Billing,
        ModelErrorKind::Auth,
        ModelErrorKind::Permission,
        ModelErrorKind::InvalidRequest,
    ] {
        let h = Harness::new();
        let id = h.task("x", Value::Null);
        // even a (wrongly) retryable flag from a provider must not cause retries
        let err = ModelError::new(kind, "nope").with_retryable(true);
        let (m, adapter) = model(&h, vec![fail_turn("", err), say("unreachable")]);
        let out = h.orch.run(&id, &m).await.unwrap();
        assert!(
            matches!(out, RunOutcome::Failed { .. }),
            "{kind:?}: {out:?}"
        );
        assert_eq!(adapter.recorded_requests().len(), 1, "{kind:?}");
    }
}

#[tokio::test]
async fn retry_after_is_honoured_on_the_injected_clock() {
    let h = Harness::new();
    let id = h.task("x", Value::Null);
    let err = ModelError::new(ModelErrorKind::RateLimited, "slow down")
        .with_retry_after(Some(Duration::from_secs(30)));
    let (m, adapter) = model(&h, vec![fail_turn("", err), say("fine")]);
    let orch = h.orch.clone();
    let id2 = id.clone();
    let run = tokio::spawn(async move { orch.run(&id2, &m).await });
    settle().await;
    assert_eq!(
        adapter.recorded_requests().len(),
        1,
        "waiting for retry-after"
    );
    assert!(h.labels().contains(&"retry:1".to_owned()));
    h.clock.advance(Duration::from_secs(29));
    settle().await;
    assert_eq!(adapter.recorded_requests().len(), 1, "29s is not enough");
    h.clock.advance(Duration::from_secs(2));
    let out = tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .expect("run finished")
        .unwrap()
        .unwrap();
    assert!(
        matches!(out, RunOutcome::Completed { .. }) || matches!(out, RunOutcome::Failed { .. }),
        "{out:?}"
    );
    assert_eq!(adapter.recorded_requests().len(), 2);
}

#[tokio::test]
async fn a_retry_after_beyond_the_cap_pauses_instead_of_sleeping() {
    let h = Harness::new();
    let id = h.task("x", Value::Null);
    let err = ModelError::new(ModelErrorKind::RateLimited, "later")
        .with_retry_after(Some(Duration::from_secs(3600)));
    let (m, adapter) = model(&h, vec![fail_turn("", err), say("later")]);
    let out = h.orch.run(&id, &m).await.unwrap();
    assert_eq!(
        out,
        RunOutcome::Paused {
            reason: PauseReason::ModelUnavailable
        }
    );
    assert_eq!(adapter.recorded_requests().len(), 1);
}

// ---- rejected calls ------------------------------------------------------------------------------

fn broken_call(id: &str, name: &str, raw: &str) -> Vec<ScriptItem> {
    let cid = ToolCallId::from_string(id);
    vec![
        ScriptStep::ToolFragment {
            call_id: cid.clone(),
            name: Some(name.into()),
            fragment: raw.into(),
        }
        .into(),
        ScriptStep::ToolEnd { call_id: cid }.into(),
    ]
}

#[tokio::test]
async fn rejected_calls_are_answered_as_error_results_without_execution() {
    let h = Harness::new();
    let id = h.task("x", Value::Null);
    let (m, adapter) = model(
        &h,
        vec![
            calls(vec![
                broken_call(
                    "bad1",
                    "create_todos",
                    "{\"todos\": [{\"title\": \"unterminated",
                ),
                broken_call("bad2", "no_such_tool", "{}"),
                call("good", "create_todos", todos_args("kept")),
            ]),
            say("recovered"),
        ],
    );
    let out = h.orch.run(&id, &m).await.unwrap();
    assert!(matches!(out, RunOutcome::Completed { .. }), "{out:?}");
    assert_eq!(h.todos().len(), 1);
    assert_eq!(
        h.steps(&id).len(),
        1,
        "rejected calls never reach the write-ahead log"
    );
    let results = tool_results(&adapter.recorded_requests()[1].messages);
    assert_eq!(results.len(), 3);
    let by_id = |i: &str| results.iter().find(|r| r.0.as_str() == i).unwrap().clone();
    assert!(
        by_id("bad1").2 && by_id("bad1").1.contains("nothing was executed"),
        "{results:?}"
    );
    assert!(by_id("bad2").2);
    assert!(!by_id("good").2);
    let labels = h.labels();
    assert!(
        labels.contains(&"rejected:create_todos".to_owned()),
        "{labels:?}"
    );
    assert!(
        labels.contains(&"rejected:no_such_tool".to_owned()),
        "{labels:?}"
    );
}

#[tokio::test]
async fn endless_rejections_are_bounded_by_the_tool_call_limit() {
    let h = Harness::new();
    let id = h.task("x", json!({"max_tool_calls": 3}));
    let turn = || calls(vec![broken_call("b", "create_todos", "{oops")]);
    let (m, adapter) = model(
        &h,
        (0..20)
            .map(|_| {
                // a fresh id each turn is not needed: the request is rebuilt from history
                turn()
            })
            .collect(),
    );
    let out = tokio::time::timeout(Duration::from_secs(5), h.orch.run(&id, &m))
        .await
        .expect("bounded")
        .unwrap();
    assert_eq!(
        out,
        RunOutcome::Paused {
            reason: PauseReason::ToolCallLimit
        },
        "{out:?}"
    );
    assert!(
        adapter.recorded_requests().len() <= 5,
        "{}",
        adapter.recorded_requests().len()
    );
    assert!(h.todos().is_empty());
    assert!(h.steps(&id).is_empty());
}

// ---- real adapters through the orchestrator ------------------------------------------------------

struct Net {
    transport: Arc<ReplayTransport>,
    binding: ModelBinding,
}

fn anthropic_binding(h: &Harness) -> Net {
    let transport = Arc::new(ReplayTransport::new());
    let creds = Arc::new(InMemoryCredentialStore::new());
    creds.set("anthropic-key", SECRET).unwrap();
    let profile = anthropic_profile();
    let deps = AdapterDeps {
        transport: transport.clone(),
        credentials: creds,
        clock: Arc::new(h.clock.clone()),
    };
    Net {
        transport,
        binding: ModelBinding {
            adapter: Arc::new(AnthropicAdapter::new(
                profile.clone(),
                deps,
                AnthropicConfig::default(),
            )),
            profile,
        },
    }
}

fn chat_binding(h: &Harness) -> Net {
    let transport = Arc::new(ReplayTransport::new());
    let creds = Arc::new(InMemoryCredentialStore::new());
    creds.set("openai-key", SECRET).unwrap();
    let profile = chat_profile();
    let deps = AdapterDeps {
        transport: transport.clone(),
        credentials: creds,
        clock: Arc::new(h.clock.clone()),
    };
    Net {
        transport,
        binding: ModelBinding {
            adapter: Arc::new(OpenAiChatAdapter::new(
                profile.clone(),
                deps,
                OpenAiChatConfig::default(),
            )),
            profile,
        },
    }
}

fn notes_args() -> (Value, Value) {
    let todos = json!({"todos": [
        {"title": "Send the budget to Amy", "due_at": "2026-11-20T09:00:00+08:00", "source_ref": "notes:line 3"},
        {"title": "Book the venue", "due_uncertain": true, "due_text": "sometime next week?", "source_ref": "notes:line 5"}
    ]});
    let md = json!({"path": "meeting-summary", "content": "# Meeting summary\n\n- Budget to Amy by Fri\n- Book the venue (date unclear)\n"});
    (todos, md)
}

fn assert_notes_outcome(h: &Harness, id: &TaskId, out: &RunOutcome) {
    let task = completed(out);
    assert_eq!(task.status, TaskStatus::Completed);
    let artifacts = h.orch.store().with(|s| s.list_artifacts(id)).unwrap();
    assert_eq!(artifacts.len(), 1);
    assert_eq!(
        task.result_ref.as_deref(),
        Some(format!("artifact:{}", artifacts[0].id).as_str())
    );
    let on_disk = std::fs::read(&artifacts[0].path).unwrap();
    assert_eq!(artifacts[0].content_hash, sha256_hex(&on_disk));
    assert_eq!(h.todos().len(), 2);
    let steps = h.steps(id);
    assert_eq!(
        steps
            .iter()
            .map(|s| (s.tool.as_str(), s.status))
            .collect::<Vec<_>>(),
        vec![
            ("create_todos", StepStatus::Succeeded),
            ("write_markdown", StepStatus::Succeeded)
        ]
    );
}

#[tokio::test]
async fn meeting_notes_scenario_completes_through_the_anthropic_adapter() {
    let h = Harness::new();
    let id = h.task(
        "Turn these meeting notes into todos and a summary document",
        Value::Null,
    );
    let net = anthropic_binding(&h);
    let (todos, md) = notes_args();
    for (body, size) in [
        (
            anthropic_sse(
                &[
                    Item::Thinking { signature: "SIG-1" },
                    Item::Text("Extracting todos. "),
                    Item::Call {
                        id: "toolu_c1",
                        name: "create_todos",
                        args: todos,
                    },
                ],
                "tool_use",
                100,
                40,
            ),
            5usize,
        ),
        (
            anthropic_sse(
                &[Item::Call {
                    id: "toolu_c2",
                    name: "write_markdown",
                    args: md,
                }],
                "tool_use",
                200,
                30,
            ),
            17,
        ),
        (
            anthropic_sse(
                &[Item::Text("Done: 2 todos saved and a summary written.")],
                "end_turn",
                300,
                12,
            ),
            1,
        ),
    ] {
        net.transport
            .push(ReplayResponse::sse(body.into_bytes()).chunked(size));
    }
    let out = h.orch.run(&id, &net.binding).await.unwrap();
    assert_notes_outcome(&h, &id, &out);

    // the thinking block travelled with the turn and came back on the next requests
    let r2 = net.transport.request_json(1).unwrap();
    assert_eq!(r2["messages"][1]["content"][0]["type"], "thinking", "{r2}");
    assert_eq!(r2["messages"][1]["content"][0]["signature"], "SIG-1");
    assert_eq!(r2["messages"][2]["content"][0]["type"], "tool_result");
    let r3 = net.transport.request_json(2).unwrap();
    assert_eq!(r3["messages"][1]["content"][0]["type"], "thinking");
    assert_eq!(net.transport.requests().len(), 3);
    // the key went only to the configured host
    assert!(net
        .transport
        .requests()
        .iter()
        .all(|r| r.url.starts_with("https://api.anthropic.com/")));
    // answer text arrived as deltas; reasoning content never became answer text
    let streamed: String = h
        .labels()
        .iter()
        .filter_map(|l| l.strip_prefix("text:"))
        .collect();
    assert_eq!(
        streamed,
        "Extracting todos. Done: 2 todos saved and a summary written."
    );
    assert!(!h
        .labels()
        .iter()
        .any(|l| l.starts_with("reasoning:") && l.contains("SIG-1")));
    assert!(!serde_json::to_string(&h.sink.events())
        .unwrap()
        .contains(SECRET));
}

#[tokio::test]
async fn meeting_notes_scenario_completes_through_the_chat_completions_adapter() {
    let h = Harness::new();
    let id = h.task(
        "Turn these meeting notes into todos and a summary document",
        Value::Null,
    );
    let net = chat_binding(&h);
    let (todos, md) = notes_args();
    for (body, size) in [
        (
            chat_sse(
                Some("Extracting todos. "),
                &[("call_c1", "create_todos", todos)],
                "tool_calls",
                Some((100, 40)),
            ),
            5usize,
        ),
        (
            chat_sse(
                None,
                &[("call_c2", "write_markdown", md)],
                "tool_calls",
                Some((200, 30)),
            ),
            17,
        ),
        (
            chat_sse(
                Some("Done: 2 todos saved and a summary written."),
                &[],
                "stop",
                Some((300, 12)),
            ),
            1,
        ),
    ] {
        net.transport
            .push(ReplayResponse::sse(body.into_bytes()).chunked(size));
    }
    let out = h.orch.run(&id, &net.binding).await.unwrap();
    assert_notes_outcome(&h, &id, &out);

    let r2 = net.transport.request_json(1).unwrap();
    let msgs = r2["messages"].as_array().unwrap();
    assert_eq!(msgs[0]["role"], "system");
    assert_eq!(msgs[2]["tool_calls"][0]["id"], "call_c1");
    assert_eq!(msgs[3]["role"], "tool");
    assert_eq!(msgs[3]["tool_call_id"], "call_c1");
    assert_eq!(net.transport.requests().len(), 3);
    assert!(!serde_json::to_string(&h.sink.events())
        .unwrap()
        .contains(SECRET));
}

#[tokio::test]
async fn a_malformed_call_from_a_real_adapter_is_answered_to_the_model_and_the_task_goes_on() {
    let h = Harness::new();
    let id = h.task("x", Value::Null);
    let net = chat_binding(&h);
    // arguments cut off mid-string, then a corrected call, then the answer
    let bad = "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_bad\",\"function\":{\"name\":\"create_todos\",\"arguments\":\"{\\\"todos\\\": [{\\\"ti\"}}]},\"finish_reason\":null}]}\n\n\
               data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n";
    net.transport
        .push(ReplayResponse::sse(bad.as_bytes().to_vec()).chunked(7));
    net.transport.push(ReplayResponse::sse(
        chat_sse(
            None,
            &[("call_ok", "create_todos", todos_args("valid"))],
            "tool_calls",
            None,
        )
        .into_bytes(),
    ));
    net.transport.push(ReplayResponse::sse(
        chat_sse(Some("done"), &[], "stop", None).into_bytes(),
    ));
    let out = h.orch.run(&id, &net.binding).await.unwrap();
    assert!(matches!(out, RunOutcome::Completed { .. }), "{out:?}");
    assert_eq!(h.todos().len(), 1);
    assert_eq!(h.steps(&id).len(), 1);
    let r2 = net.transport.request_json(1).unwrap();
    let tool_msg = r2["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "tool")
        .unwrap()
        .clone();
    assert_eq!(tool_msg["tool_call_id"], "call_bad");
    assert!(
        tool_msg["content"]
            .as_str()
            .unwrap()
            .contains("nothing was executed"),
        "{tool_msg}"
    );
}

#[tokio::test]
async fn switching_provider_at_a_step_boundary_drops_provider_state_and_reruns_nothing() {
    let h = Harness::new();
    // one tool call per budget window: the second call stays pending across the switch
    let id = h.task("two todos", json!({"max_tool_calls": 1}));
    let a = anthropic_binding(&h);
    a.transport.push(ReplayResponse::sse(
        anthropic_sse(
            &[
                Item::Thinking {
                    signature: "SIG-ANTHROPIC-ONLY",
                },
                Item::Call {
                    id: "toolu_a",
                    name: "create_todos",
                    args: todos_args("first"),
                },
                Item::Call {
                    id: "toolu_b",
                    name: "create_todos",
                    args: todos_args("second"),
                },
            ],
            "tool_use",
            50,
            20,
        )
        .into_bytes(),
    ));
    let out = h.orch.run(&id, &a.binding).await.unwrap();
    assert_eq!(
        out,
        RunOutcome::Paused {
            reason: PauseReason::ToolCallLimit
        }
    );
    assert_eq!(h.todos().len(), 1);

    // the thinking block is in local history, tagged with its origin
    let stored = serde_json::to_string(&h.messages(&id)).unwrap();
    assert!(
        stored.contains("SIG-ANTHROPIC-ONLY") && stored.contains("anthropic.thinking"),
        "{stored}"
    );

    // resume on another provider
    let c = chat_binding(&h);
    c.transport.push(ReplayResponse::sse(
        chat_sse(Some("both saved"), &[], "stop", Some((10, 3))).into_bytes(),
    ));
    let out = h.orch.run(&id, &c.binding).await.unwrap();
    assert!(matches!(out, RunOutcome::Completed { .. }), "{out:?}");

    // the pending call ran locally exactly once; the first was not repeated
    assert_eq!(
        h.todos()
            .iter()
            .map(|t| t.title.as_str())
            .collect::<Vec<_>>(),
        vec!["first", "second"]
    );
    assert_eq!(h.steps(&id).len(), 2);
    assert_eq!(
        a.transport.requests().len(),
        1,
        "the old provider is not contacted again"
    );

    // the other provider got the conversation, but none of the opaque state
    let reqs = c.transport.requests();
    assert_eq!(reqs.len(), 1);
    assert!(reqs[0].url.starts_with("https://api.openai.com/"));
    let body = c.transport.request_json(0).unwrap().to_string();
    assert!(
        !body.contains("SIG-ANTHROPIC-ONLY") && !body.contains("thinking"),
        "{body}"
    );
    let tool_ids: Vec<_> = tool_results(&{
        let conv = h.task_row(&id).conversation_id.unwrap();
        let hist = h.orch.store().with(|s| s.list_messages(&conv)).unwrap();
        super::history::to_messages(
            &hist,
            &crate::model::ModelOrigin::from_profile(&chat_profile()),
        )
    })
    .iter()
    .map(|r| r.0.to_string())
    .collect();
    assert_eq!(tool_ids, vec!["toolu_a", "toolu_b"]);
    assert!(body.contains("toolu_a") && body.contains("toolu_b"));
    // each provider only ever saw its own key
    assert!(!serde_json::to_string(&h.sink.events())
        .unwrap()
        .contains(SECRET));
}
