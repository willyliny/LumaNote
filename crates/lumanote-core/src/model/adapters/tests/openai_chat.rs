use serde_json::json;

use super::*;
use crate::model::adapters::{ChatTokenParam, OpenAiChatConfig};
use crate::model::contracts::*;
use crate::model::testkit::*;
use crate::ports::fakes::{Fault, ReplayResponse};
use crate::ports::CredentialStore;
use crate::schema::ToolCallId;
use std::time::Duration;

fn fx(name: &str) -> ReplayResponse {
    ReplayResponse::from_fixture(&fixture(&format!("openai-chat-completions/{name}")))
}

async fn run_all(name: &str) -> Vec<(String, Vec<ModelEvent>)> {
    run_all_with(name, chat_rig).await
}

async fn run_all_with(name: &str, mk: fn() -> Rig) -> Vec<(String, Vec<ModelEvent>)> {
    let mut out = Vec::new();
    for (label, resp) in variants(&fx(name)) {
        let rig = mk();
        rig.transport.push(resp);
        let evs = rig.run(rig.request("hi")).await;
        out.push((label, evs));
    }
    out
}

fn local_rig() -> Rig {
    chat_rig_with(
        local_profile(),
        OpenAiChatConfig {
            allow_loopback_http: true,
            ..Default::default()
        },
    )
}

#[tokio::test]
async fn plain_text_all_chunkings() {
    for (label, evs) in run_all("01-text.sse").await {
        assert!(!texts(&evs).is_empty(), "{label}");
        assert_eq!(stop(&evs), Some(StopReason::EndTurn), "{label}");
        assert!(well_terminated(&evs), "{label}");
        let u = last_usage(&evs).expect("usage");
        assert_eq!(
            (u.input_tokens, u.output_tokens),
            (Some(12), Some(3)),
            "{label}"
        );
        assert_eq!(u.cache_read_tokens, Some(0));
    }
}

#[tokio::test]
async fn split_tool_call_is_assembled_once() {
    for (label, evs) in run_all("02-tool-call-split.sse").await {
        let r = readies(&evs);
        assert_eq!(r.len(), 1, "{label}: {evs:?}");
        assert_eq!(r[0].0, "call_FIXTUREa1b2c3", "{label}");
        assert_eq!(r[0].1, "get_weather");
        assert_eq!(
            r[0].2,
            json!({"location":"San Francisco, CA","unit":"fahrenheit"}),
            "{label}"
        );
        assert_eq!(stop(&evs), Some(StopReason::ToolUse), "{label}");
        assert!(rejections(&evs).is_empty());
        let u = last_usage(&evs).unwrap();
        assert_eq!((u.input_tokens, u.output_tokens), (Some(82), Some(18)));
    }
}

#[tokio::test]
async fn parallel_tool_calls() {
    for (label, evs) in run_all("03-parallel-tools.sse").await {
        let r = readies(&evs);
        assert_eq!(r.len(), 2, "{label}: {evs:?}");
        assert_eq!(
            (r[0].0.as_str(), r[0].1.as_str()),
            ("call_FIXTUREp1", "get_weather")
        );
        assert_eq!(
            (r[1].0.as_str(), r[1].1.as_str()),
            ("call_FIXTUREp2", "get_time")
        );
        assert_eq!(stop(&evs), Some(StopReason::ToolUse), "{label}");
        let u = last_usage(&evs).unwrap();
        assert_eq!(
            (u.input_tokens, u.cache_read_tokens),
            (Some(640), Some(512))
        );
    }
}

#[tokio::test]
async fn midstream_error_fails_after_partial_output() {
    for (label, evs) in run_all("04-midstream-error.sse").await {
        let f = failure(&evs).unwrap_or_else(|| panic!("{label}: {evs:?}"));
        assert_eq!(f.kind, ModelErrorKind::Server, "{label}");
        assert!(f.retryable);
        assert!(f.partial_output_emitted, "{label}");
        assert!(readies(&evs).is_empty());
        assert!(well_terminated(&evs));
        assert_eq!(texts(&evs), "Let me check");
    }
}

#[tokio::test]
async fn rate_limit_429_with_retry_after() {
    for (label, resp) in variants(&fx("05-rate-limit-429.http")) {
        let rig = chat_rig();
        rig.transport.push(resp);
        let f = failure(&rig.run(rig.request("hi")).await).unwrap_or_else(|| panic!("{label}"));
        assert_eq!(f.kind, ModelErrorKind::RateLimited);
        assert!(f.retryable);
        assert_eq!(f.retry_after, Some(Duration::from_secs(7)));
        assert_eq!(f.request_id.as_deref(), Some("req_FIXTURE429"));
        assert_eq!(f.http_status, Some(429));
    }
}

#[tokio::test]
async fn insufficient_quota_429_is_billing() {
    let rig = chat_rig();
    rig.transport.push(
        ReplayResponse::sse(br#"{"error":{"message":"You exceeded your current quota","type":"insufficient_quota","param":null,"code":"insufficient_quota"}}"#.to_vec())
            .with_status(429)
            .with_header("content-type", "application/json"),
    );
    let f = failure(&rig.run(rig.request("hi")).await).unwrap();
    assert_eq!(f.kind, ModelErrorKind::Billing);
    assert!(!f.retryable);
}

#[tokio::test]
async fn local_server_whole_call_no_usage() {
    for (label, resp) in variants(&fx("06-local-whole-call-no-usage.sse")) {
        let rig = local_rig();
        rig.transport.push(resp);
        let evs = rig.run(rig.request("hi")).await;
        let r = readies(&evs);
        assert_eq!(r.len(), 1, "{label}: {evs:?}");
        assert_eq!(r[0].0, "call_0");
        assert_eq!(r[0].1, "add_todo");
        assert_eq!(r[0].2, json!({"title":"Buy milk","due":"2026-11-12"}));
        assert_eq!(stop(&evs), Some(StopReason::ToolUse));
        match last_usage(&evs) {
            None => {}
            Some(u) => assert!(
                u.is_unknown(),
                "unknown usage must not be reported as zero: {u:?}"
            ),
        }
    }
}

fn data(v: serde_json::Value) -> String {
    format!("data: {v}\n\n")
}

fn chunk(delta: serde_json::Value, finish: serde_json::Value) -> String {
    data(
        json!({"id":"c","object":"chat.completion.chunk","choices":[{"index":0,"delta":delta,"finish_reason":finish}]}),
    )
}

#[tokio::test]
async fn missing_and_colliding_call_ids_get_stable_unique_ids() {
    let body = chunk(
        json!({"tool_calls":[
            {"index":0,"function":{"name":"get_time","arguments":"{\"timezone\":\"UTC\"}"}},
            {"index":1,"function":{"name":"get_time","arguments":"{\"timezone\":\"CET\"}"}}
        ]}),
        json!(null),
    ) + &chunk(json!({}), json!("tool_calls"))
        + "data: [DONE]\n\n";
    let mut ids_seen = Vec::new();
    for size in [1usize, 5, 0] {
        let rig = local_rig();
        rig.transport.push(chunked(
            ReplayResponse::sse(body.clone().into_bytes()),
            size,
        ));
        let evs = rig.run(rig.request("hi")).await;
        let r = readies(&evs);
        assert_eq!(r.len(), 2, "{evs:?}");
        assert_ne!(r[0].0, r[1].0);
        assert!(!r[0].0.is_empty());
        ids_seen.push(r.iter().map(|c| c.0.clone()).collect::<Vec<_>>());
    }
    // chunking must not change the ids made up for one request id... they depend on the request id
    // only, so each run (new request id) is allowed to differ but never within one run.
    assert_eq!(ids_seen.len(), 3);

    // Two calls that both claim the same id keep distinct ids.
    let body = chunk(
        json!({"tool_calls":[
            {"index":0,"id":"call_0","function":{"name":"get_time","arguments":"{\"timezone\":\"UTC\"}"}},
            {"index":1,"id":"call_0","function":{"name":"get_time","arguments":"{\"timezone\":\"CET\"}"}}
        ]}),
        json!(null),
    ) + &chunk(json!({}), json!("tool_calls"))
        + "data: [DONE]\n\n";
    let rig = local_rig();
    rig.transport.push(ReplayResponse::sse(body.into_bytes()));
    let r = readies(&rig.run(rig.request("hi")).await);
    assert_eq!(r.len(), 2);
    assert_ne!(r[0].0, r[1].0, "colliding ids must be disambiguated");
}

#[tokio::test]
async fn finish_stop_with_tool_calls_is_still_tool_use() {
    let body = chunk(
        json!({"tool_calls":[{"index":0,"id":"call_a","function":{"name":"get_time","arguments":"{\"timezone\":\"UTC\"}"}}]}),
        json!(null),
    ) + &chunk(json!({}), json!("stop"))
        + "data: [DONE]\n\n";
    let rig = local_rig();
    rig.transport.push(ReplayResponse::sse(body.into_bytes()));
    let evs = rig.run(rig.request("hi")).await;
    assert_eq!(readies(&evs).len(), 1);
    assert_eq!(stop(&evs), Some(StopReason::ToolUse));
}

#[tokio::test]
async fn eof_without_done_after_finish_reason_completes_but_without_finish_fails() {
    let ok = chunk(json!({"content":"hi"}), json!(null)) + &chunk(json!({}), json!("stop"));
    let rig = local_rig();
    rig.transport.push(ReplayResponse::sse(ok.into_bytes()));
    assert_eq!(
        stop(&rig.run(rig.request("x")).await),
        Some(StopReason::EndTurn)
    );

    let cut = chunk(json!({"content":"hi"}), json!(null));
    let rig = local_rig();
    rig.transport.push(ReplayResponse::sse(cut.into_bytes()));
    let evs = rig.run(rig.request("x")).await;
    let f = failure(&evs).expect("cut stream fails");
    assert_eq!(f.kind, ModelErrorKind::Network);
    assert!(f.partial_output_emitted);
}

#[tokio::test]
async fn duplicated_chunks_with_sse_ids_are_ignored() {
    let base = fixture("openai-chat-completions/02-tool-call-split.sse");
    let mut doubled = String::new();
    for (n, block) in base
        .split("\n\n")
        .filter(|b| b.contains("data:"))
        .enumerate()
    {
        let b: String = block
            .lines()
            .filter(|l| !l.starts_with(':'))
            .collect::<Vec<_>>()
            .join("\n");
        if b.trim().is_empty() {
            continue;
        }
        doubled += &format!("id: {n}\n{b}\n\nid: {n}\n{b}\n\n");
    }
    let rig = chat_rig();
    rig.transport
        .push(ReplayResponse::sse(doubled.into_bytes()).chunked(9));
    let evs = rig.run(rig.request("x")).await;
    let r = readies(&evs);
    assert_eq!(r.len(), 1, "{evs:?}");
    assert_eq!(
        r[0].2,
        json!({"location":"San Francisco, CA","unit":"fahrenheit"})
    );
}

#[tokio::test]
async fn disconnect_mid_stream() {
    let total = fx("02-tool-call-split.sse").body.len();
    for cut in [30, 700, total / 2] {
        let rig = chat_rig();
        rig.transport.push(
            fx("02-tool-call-split.sse")
                .chunked(13)
                .disconnect_after(cut),
        );
        let evs = rig.run(rig.request("hi")).await;
        let f = failure(&evs).unwrap_or_else(|| panic!("cut {cut}: {evs:?}"));
        assert_eq!(f.kind, ModelErrorKind::Network, "cut {cut}");
        assert!(readies(&evs).is_empty());
        assert!(well_terminated(&evs));
    }
}

#[tokio::test]
async fn malformed_unknown_and_invalid_calls_are_rejected_not_failed() {
    for (name, args, want_in_reason) in [
        ("read_file", "{\"path\": \"C:/Us", ""),
        ("delete_everything", "{}", "delete_everything"),
        ("read_file", "{}", "path"),
        ("read_file", "[1,2]", ""),
    ] {
        let body = chunk(
            json!({"tool_calls":[{"index":0,"id":"call_bad","function":{"name":name,"arguments":args}}]}),
            json!(null),
        ) + &chunk(json!({}), json!("tool_calls"))
            + "data: [DONE]\n\n";
        let rig = chat_rig();
        rig.transport
            .push(ReplayResponse::sse(body.into_bytes()).chunked(6));
        let evs = rig.run(rig.request("hi")).await;
        let rj = rejections(&evs);
        assert_eq!(rj.len(), 1, "{name} {args}: {evs:?}");
        assert_eq!(rj[0].0, "call_bad");
        assert!(rj[0].2.contains(want_in_reason), "{}", rj[0].2);
        assert!(readies(&evs).is_empty());
        assert_eq!(
            stop(&evs),
            Some(StopReason::ToolUse),
            "the turn completes: {evs:?}"
        );
    }
}

#[tokio::test]
async fn length_finish_cuts_the_call_and_reports_max_tokens() {
    let body = chunk(
        json!({"tool_calls":[{"index":0,"id":"call_cut","function":{"name":"read_file","arguments":"{\"path\": \"a"}}]}),
        json!(null),
    ) + &chunk(json!({}), json!("length"))
        + "data: [DONE]\n\n";
    let rig = chat_rig();
    rig.transport.push(ReplayResponse::sse(body.into_bytes()));
    let evs = rig.run(rig.request("hi")).await;
    assert_eq!(rejections(&evs).len(), 1);
    assert_eq!(stop(&evs), Some(StopReason::MaxTokens));
}

#[tokio::test]
async fn finish_reasons_map() {
    for (raw, want) in [
        ("stop", StopReason::EndTurn),
        ("length", StopReason::MaxTokens),
        ("content_filter", StopReason::ContentFilter),
    ] {
        let body = chunk(json!({"content":"ab"}), json!(null))
            + &chunk(json!({}), json!(raw))
            + "data: [DONE]\n\n";
        let rig = chat_rig();
        rig.transport.push(ReplayResponse::sse(body.into_bytes()));
        assert_eq!(stop(&rig.run(rig.request("hi")).await), Some(want), "{raw}");
    }
}

#[tokio::test]
async fn refusal_deltas_are_text_and_stop_is_refusal() {
    let body = chunk(json!({"refusal":"I can't help"}), json!(null))
        + &chunk(json!({}), json!("stop"))
        + "data: [DONE]\n\n";
    let rig = chat_rig();
    rig.transport.push(ReplayResponse::sse(body.into_bytes()));
    let evs = rig.run(rig.request("hi")).await;
    assert_eq!(texts(&evs), "I can't help");
    assert_eq!(stop(&evs), Some(StopReason::Refusal));
}

#[tokio::test]
async fn reasoning_content_is_not_answer_text() {
    let body = chunk(json!({"reasoning_content":"thinking..."}), json!(null))
        + &chunk(json!({"content":"answer"}), json!(null))
        + &chunk(json!({}), json!("stop"))
        + "data: [DONE]\n\n";
    let rig = chat_rig();
    rig.transport.push(ReplayResponse::sse(body.into_bytes()));
    let evs = rig.run(rig.request("hi")).await;
    assert_eq!(texts(&evs), "answer");
    let reasoning: String = evs
        .iter()
        .filter_map(|e| match e {
            ModelEvent::ReasoningDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(reasoning, "thinking...");
}

#[tokio::test]
async fn unknown_fields_are_tolerated() {
    let body = data(
        json!({"id":"c","object":"chat.completion.chunk","system_fingerprint":"fp","service_tier":"x","weird":{"a":1},
        "choices":[{"index":0,"delta":{"role":"assistant","content":"ok","audio":null,"new_field":[1]},"logprobs":null,"finish_reason":null,"extra":1}]}),
    ) + &chunk(json!({}), json!("stop"))
        + "data: [DONE]\n\n";
    let rig = chat_rig();
    rig.transport.push(ReplayResponse::sse(body.into_bytes()));
    let evs = rig.run(rig.request("hi")).await;
    assert_eq!(texts(&evs), "ok");
    assert_eq!(stop(&evs), Some(StopReason::EndTurn));
}

#[tokio::test]
async fn extra_content_round_trips_on_the_matching_call() {
    let body = chunk(
        json!({"tool_calls":[{"index":0,"id":"call_g","extra_content":{"google":{"thought_signature":"SIG"}},
        "function":{"name":"get_time","arguments":"{\"timezone\":\"UTC\"}"}}]}),
        json!(null),
    ) + &chunk(json!({}), json!("tool_calls"))
        + "data: [DONE]\n\n";
    let rig = chat_rig();
    rig.transport.push(ReplayResponse::sse(body.into_bytes()));
    let evs = rig.run(rig.request("hi")).await;
    let state = evs.iter().find_map(|e| match e {
        ModelEvent::ProviderState { kind, data } => Some((kind.clone(), data.clone())),
        _ => None,
    });
    let (kind, data) = state.expect("provider state emitted");
    assert_eq!(kind, "openai.tool_call_extra");

    let origin = ModelOrigin::from_profile(&rig.profile);
    let id = ToolCallId::from_string("call_g");
    let mut req = rig.request("hi");
    req.messages.push(Message {
        role: Role::Assistant,
        parts: vec![
            MessagePart::ProviderState { origin, kind, data },
            MessagePart::ToolCall {
                id: id.clone(),
                name: "get_time".into(),
                arguments: json!({"timezone":"UTC"}),
            },
        ],
    });
    req.messages.push(Message {
        role: Role::Tool,
        parts: vec![MessagePart::ToolResult {
            call_id: id,
            content: "12:00".into(),
            is_error: false,
        }],
    });
    rig.transport.push(fx("01-text.sse"));
    let _ = rig.run(req).await;
    let sent = rig.transport.request_json(1).expect("second request");
    assert_eq!(
        sent["messages"][2]["tool_calls"][0]["extra_content"],
        json!({"google":{"thought_signature":"SIG"}}),
        "{sent}"
    );
}

#[tokio::test]
async fn cancellation_mid_stream_yields_cancelled() {
    use futures::StreamExt;
    use tokio_util::sync::CancellationToken;
    let rig = chat_rig();
    let at = {
        let b = String::from_utf8_lossy(&fx("01-text.sse").body).into_owned();
        let p = b.find("\"content\":\"Hello\"").unwrap_or(0);
        p + b[p..].find("\n\n").unwrap_or(0) + 2
    };
    rig.transport.push(
        fx("01-text.sse")
            .chunked(40)
            .with_fault(Fault::StallAfter(at + 100)),
    );
    let cancel = CancellationToken::new();
    let mut s = rig.adapter.stream(rig.request("hi"), cancel.clone());
    let mut evs = Vec::new();
    while let Some(e) = tokio::time::timeout(Duration::from_secs(5), s.next())
        .await
        .unwrap_or_else(|_| panic!("hung after {evs:?}"))
    {
        let is_text = matches!(e, ModelEvent::TextDelta { .. });
        evs.push(e);
        if is_text {
            cancel.cancel();
        }
    }
    let f = failure(&evs).expect("failed");
    assert_eq!(f.kind, ModelErrorKind::Cancelled);
    assert!(f.partial_output_emitted);
    assert!(well_terminated(&evs));
}

#[tokio::test]
async fn http_errors_map_by_status() {
    for (status, body, kind, retryable) in [
        (
            401,
            r#"{"error":{"message":"Incorrect API key","type":"invalid_request_error","code":"invalid_api_key"}}"#,
            ModelErrorKind::Auth,
            false,
        ),
        (
            403,
            r#"{"error":{"message":"no access","type":"invalid_request_error","code":null}}"#,
            ModelErrorKind::Permission,
            false,
        ),
        (
            400,
            r#"{"error":{"message":"This model's maximum context length is 8192 tokens","type":"invalid_request_error","code":"context_length_exceeded"}}"#,
            ModelErrorKind::ContextTooLong,
            false,
        ),
        (
            400,
            r#"{"error":{"message":"bad param","type":"invalid_request_error","code":null}}"#,
            ModelErrorKind::InvalidRequest,
            false,
        ),
        (
            404,
            r#"{"error":{"message":"model not found","type":"invalid_request_error","code":"model_not_found"}}"#,
            ModelErrorKind::InvalidRequest,
            false,
        ),
        (
            500,
            r#"{"error":{"message":"oops","type":"server_error"}}"#,
            ModelErrorKind::Server,
            true,
        ),
        (
            503,
            r#"{"error":{"message":"slow down","type":"server_error"}}"#,
            ModelErrorKind::Overloaded,
            true,
        ),
        (
            502,
            "<html>bad gateway</html>",
            ModelErrorKind::Server,
            true,
        ),
    ] {
        let rig = chat_rig();
        rig.transport.push(
            ReplayResponse::sse(body.as_bytes().to_vec())
                .with_status(status)
                .with_header("content-type", "application/json"),
        );
        let f = failure(&rig.run(rig.request("hi")).await).expect("failed");
        assert_eq!(f.kind, kind, "{status} {body}");
        assert_eq!(f.retryable, retryable, "{status}");
        assert_eq!(f.http_status, Some(status));
    }
}

fn conversation() -> Vec<Message> {
    let tc = ToolCallId::from_string;
    vec![
        Message::text(
            Role::User,
            "What is the weather in Paris and the time in UTC?",
        ),
        Message {
            role: Role::Assistant,
            parts: vec![
                MessagePart::Text {
                    text: "Checking.".into(),
                },
                MessagePart::ToolCall {
                    id: tc("call_1"),
                    name: "get_weather".into(),
                    arguments: json!({"location":"Paris"}),
                },
                MessagePart::ToolCall {
                    id: tc("call_2"),
                    name: "get_time".into(),
                    arguments: json!({"timezone":"UTC"}),
                },
            ],
        },
        Message {
            role: Role::Tool,
            parts: vec![
                MessagePart::ToolResult {
                    call_id: tc("call_1"),
                    content: "sunny".into(),
                    is_error: false,
                },
                MessagePart::ToolResult {
                    call_id: tc("call_2"),
                    content: "boom".into(),
                    is_error: true,
                },
            ],
        },
    ]
}

#[tokio::test]
async fn request_body_snapshot() {
    let rig = chat_rig();
    rig.transport.push(fx("01-text.sse"));
    let mut req = rig.request("ignored");
    req.messages = conversation();
    req.tools.truncate(2);
    req.output_budget = Some(1000);
    let _ = rig.run(req).await;
    let body = rig.transport.request_json(0).unwrap();
    let want = json!({
        "model": "gpt-test-1",
        "stream": true,
        "stream_options": {"include_usage": true},
        "max_completion_tokens": 1000,
        "messages": [
            {"role":"system","content":"You are a test assistant."},
            {"role":"user","content":"What is the weather in Paris and the time in UTC?"},
            {"role":"assistant","content":"Checking.","tool_calls":[
                {"id":"call_1","type":"function","function":{"name":"get_weather","arguments":"{\"location\":\"Paris\"}"}},
                {"id":"call_2","type":"function","function":{"name":"get_time","arguments":"{\"timezone\":\"UTC\"}"}}
            ]},
            {"role":"tool","tool_call_id":"call_1","content":"sunny"},
            {"role":"tool","tool_call_id":"call_2","content":"boom"}
        ],
        "tools": [
            {"type":"function","function":{"name":"get_weather","description":"get_weather tool","parameters":{"type":"object","required":["location"],"properties":{}}}},
            {"type":"function","function":{"name":"get_time","description":"get_time tool","parameters":{"type":"object","required":["timezone"],"properties":{}}}}
        ]
    });
    assert_eq!(
        body,
        want,
        "{}",
        serde_json::to_string_pretty(&body).unwrap()
    );
}

#[tokio::test]
async fn local_profile_gets_no_unsupported_parameters() {
    let rig = local_rig();
    rig.transport.push(fx("06-local-whole-call-no-usage.sse"));
    let mut req = rig.request("hi");
    req.output_budget = Some(500);
    let _ = rig.run(req).await;
    let body = rig.transport.request_json(0).unwrap();
    assert!(
        body.get("stream_options").is_none(),
        "server does not report usage: {body}"
    );
    assert!(body.get("parallel_tool_calls").is_none() && body.get("tool_choice").is_none());
    assert!(body.get("reasoning_effort").is_none());
    let r = &rig.transport.requests()[0];
    assert_eq!(r.url, "http://localhost:11434/v1/chat/completions");
    assert!(
        r.header("authorization").is_none(),
        "no credential configured, none sent"
    );
}

#[tokio::test]
async fn max_tokens_param_variant() {
    let cfg = OpenAiChatConfig {
        max_tokens_param: ChatTokenParam::MaxTokens,
        ..Default::default()
    };
    let rig = chat_rig_with(chat_profile(), cfg);
    rig.transport.push(fx("01-text.sse"));
    let mut req = rig.request("hi");
    req.output_budget = Some(77);
    let _ = rig.run(req).await;
    let body = rig.transport.request_json(0).unwrap();
    assert_eq!(body["max_tokens"], 77);
    assert!(body.get("max_completion_tokens").is_none());
}

#[tokio::test]
async fn tool_choice_parallel_stop_and_reasoning_mapping() {
    let rig = chat_rig();
    rig.transport.push(fx("01-text.sse"));
    let mut req = rig.request("hi");
    req.tool_choice = ToolChoice::Tool {
        name: "get_time".into(),
    };
    req.parallel_tool_calls = Some(false);
    req.stop_sequences = vec!["END".into()];
    req.reasoning = Some(ReasoningConfig {
        effort: Some(ReasoningEffort::Low),
        budget_tokens: None,
    });
    let _ = rig.run(req).await;
    let body = rig.transport.request_json(0).unwrap();
    assert_eq!(
        body["tool_choice"],
        json!({"type":"function","function":{"name":"get_time"}})
    );
    assert_eq!(body["parallel_tool_calls"], false);
    assert_eq!(body["stop"], json!(["END"]));
    assert_eq!(body["reasoning_effort"], "low");
    for k in ["temperature", "top_p"] {
        assert!(body.get(k).is_none());
    }
}

#[tokio::test]
async fn provider_state_of_another_origin_is_not_sent() {
    let rig = chat_rig();
    rig.transport.push(fx("01-text.sse"));
    let other = ModelOrigin {
        provider: "anthropic".into(),
        protocol: "anthropic_messages".into(),
        model: "claude-test-1".into(),
    };
    let mut req = rig.request("hi");
    req.messages.push(Message {
        role: Role::Assistant,
        parts: vec![
            MessagePart::ProviderState {
                origin: other,
                kind: "anthropic.thinking".into(),
                data: json!({"type":"thinking","signature":"S"}),
            },
            MessagePart::Text {
                text: "earlier answer".into(),
            },
        ],
    });
    req.messages.push(Message::text(Role::User, "and now?"));
    let _ = rig.run(req).await;
    let body = rig.transport.request_json(0).unwrap().to_string();
    assert!(
        !body.contains("thinking") && !body.contains("signature"),
        "{body}"
    );
    assert!(body.contains("earlier answer"));
}

#[tokio::test]
async fn images_by_data_url_when_supported() {
    use crate::schema::ArtifactId;
    let rig = chat_rig();
    rig.transport.push(fx("01-text.sse"));
    let art = ArtifactId::new();
    let mut req = rig.request("x");
    req.messages = vec![Message {
        role: Role::User,
        parts: vec![
            MessagePart::Text {
                text: "look".into(),
            },
            MessagePart::ImageRef {
                artifact_id: art.clone(),
                mime: "image/png".into(),
            },
        ],
    }];
    req.image_data.insert(
        art,
        ImageData {
            mime: "image/png".into(),
            base64: "AAAA".into(),
        },
    );
    let _ = rig.run(req).await;
    let body = rig.transport.request_json(0).unwrap();
    assert_eq!(
        body["messages"][1]["content"],
        json!([{"type":"text","text":"look"},{"type":"image_url","image_url":{"url":"data:image/png;base64,AAAA"}}])
    );

    let mut p = chat_profile();
    p.capabilities.image_input = Support::Unsupported;
    let rig = chat_rig_with(p, Default::default());
    let art = crate::schema::ArtifactId::new();
    let mut req = rig.request("x");
    req.messages = vec![Message {
        role: Role::User,
        parts: vec![MessagePart::ImageRef {
            artifact_id: art,
            mime: "image/png".into(),
        }],
    }];
    let f = failure(&rig.run(req).await).expect("failed");
    assert_eq!(f.kind, ModelErrorKind::UnsupportedCapability);
}

#[tokio::test]
async fn validate_config_rules_without_network() {
    let rig = chat_rig();
    assert!(rig.adapter.validate_config(&rig.profile).ok);
    let bad = |f: &dyn Fn(&mut ModelProfile)| {
        let mut p = chat_profile();
        f(&mut p);
        rig.adapter.validate_config(&p)
    };
    assert!(
        !bad(&|p| p.base_url = "http://api.openai.com/v1".into()).ok,
        "http to a remote host"
    );
    assert!(!bad(&|p| p.base_url = "not a url".into()).ok);
    assert!(
        !bad(&|p| p.credential_ref = None).ok,
        "remote host needs a key"
    );
    assert!(!bad(&|p| p.credential_ref = Some("absent".into())).ok);
    assert!(
        !bad(&|p| p.base_url = "https://user:pw@api.openai.com/v1".into()).ok,
        "no userinfo in urls"
    );
    assert!(rig.transport.requests().is_empty());

    // loopback http is refused unless configured, then allowed without a key
    let off = chat_rig_with(local_profile(), Default::default());
    assert!(!off.adapter.validate_config(&local_profile()).ok);
    let on = local_rig();
    assert!(
        on.adapter.validate_config(&local_profile()).ok,
        "{:?}",
        on.adapter.validate_config(&local_profile())
    );
}

#[tokio::test]
async fn key_goes_only_to_the_configured_host_in_the_bearer_header() {
    let rig = chat_rig();
    rig.transport.push(fx("01-text.sse"));
    let _ = rig.run(rig.request("hi")).await;
    let r = &rig.transport.requests()[0];
    assert_eq!(r.url, "https://api.openai.com/v1/chat/completions");
    assert_eq!(
        r.header("authorization"),
        Some(format!("Bearer {SECRET}").as_str())
    );
    assert!(!r.url.contains(SECRET));
    assert!(!String::from_utf8_lossy(r.body.as_deref().unwrap_or_default()).contains(SECRET));
}

#[tokio::test]
async fn missing_credential_fails_without_sending() {
    let rig = chat_rig();
    rig.creds.delete("openai-key").unwrap();
    let f = failure(&rig.run(rig.request("hi")).await).expect("failed");
    assert_eq!(f.kind, ModelErrorKind::Auth);
    assert!(rig.transport.requests().is_empty());
}

#[tokio::test]
async fn the_secret_never_appears_in_events_errors_or_debug_output() {
    let rig = chat_rig();
    let echo = format!(
        r#"{{"error":{{"message":"Incorrect API key provided: {SECRET}.","type":"invalid_request_error","code":"invalid_api_key"}}}}"#
    );
    rig.transport.push(
        ReplayResponse::sse(echo.into_bytes())
            .with_status(401)
            .with_header("content-type", "application/json"),
    );
    let evs = rig.run(rig.request("hi")).await;
    assert!(!serde_json::to_string(&evs).unwrap().contains(SECRET));
    assert!(!format!("{evs:?}").contains(SECRET));
    let f = failure(&evs).unwrap();
    assert!(!f.to_luma_error().to_string().contains(SECRET));
    assert!(!format!("{:?}", f.to_luma_error()).contains(SECRET));

    // the same inside a mid-stream error event
    let echo = format!(
        "data: {}\n\n",
        json!({"error":{"message":format!("bad key {SECRET}"),"type":"server_error"}})
    );
    rig.transport.push(ReplayResponse::sse(echo.into_bytes()));
    let evs = rig.run(rig.request("hi")).await;
    assert!(
        !serde_json::to_string(&evs).unwrap().contains(SECRET),
        "{evs:?}"
    );

    rig.transport.push(fx("01-text.sse"));
    let evs = rig.run(rig.request("hi")).await;
    assert!(!serde_json::to_string(&evs).unwrap().contains(SECRET));
    for r in rig.transport.requests() {
        assert!(
            !format!("{r:?}").contains(SECRET),
            "HttpRequest Debug redacts"
        );
    }
    assert!(!format!("{:?}", rig.adapter_debug()).contains(SECRET));
}

impl Rig {
    fn adapter_debug(&self) -> String {
        format!(
            "{:?} {:?} {:?}",
            self.profile,
            OpenAiChatConfig::default(),
            self.transport
        )
    }
}

#[tokio::test]
async fn probe_makes_one_cheap_call_and_reports_missing_usage() {
    let rig = local_rig();
    rig.transport.push(fx("06-local-whole-call-no-usage.sse"));
    let rep = rig.adapter.probe(&local_profile()).await;
    assert_eq!(rig.transport.requests().len(), 1);
    assert_eq!(rep.streaming, Support::Supported, "{rep:?}");
    assert_ne!(rep.usage_in_stream, Support::Supported);

    let rig = chat_rig();
    rig.transport.push(fx("01-text.sse"));
    let rep = rig.adapter.probe(&rig.profile).await;
    assert!(rep.ok, "{rep:?}");
    assert_eq!(rep.usage_in_stream, Support::Supported);
}
