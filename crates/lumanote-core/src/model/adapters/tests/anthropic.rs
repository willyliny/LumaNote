use serde_json::json;

use super::*;
use crate::model::adapters::{AnthropicAuth, AnthropicConfig};
use crate::model::contracts::*;
use crate::model::testkit::*;
use crate::ports::fakes::{Fault, ReplayResponse};
use crate::ports::CredentialStore;
use std::time::Duration;

/// Byte offset just past the first text delta event of fixture 01.
fn after_first_text() -> usize {
    let body = fx("01-text.sse").body;
    let text = String::from_utf8_lossy(&body).into_owned();
    let at = text
        .find("\"text\": \"Hello\"")
        .or_else(|| text.find("\"text\":\"Hello\""))
        .unwrap_or(0);
    at + text[at..].find("\n\n").unwrap_or(0) + 2
}

fn fx(name: &str) -> ReplayResponse {
    ReplayResponse::from_fixture(&fixture(&format!("anthropic-messages/{name}")))
}

async fn run_all(name: &str) -> Vec<(String, Vec<ModelEvent>)> {
    let mut out = Vec::new();
    for (label, resp) in variants(&fx(name)) {
        let rig = anthropic_rig();
        rig.transport.push(resp);
        let evs = rig.run(rig.request("hi")).await;
        out.push((label, evs));
    }
    out
}

#[tokio::test]
async fn plain_text_all_chunkings() {
    for (label, evs) in run_all("01-text.sse").await {
        assert_eq!(texts(&evs), "Hello!", "{label}");
        assert_eq!(stop(&evs), Some(StopReason::EndTurn), "{label}");
        assert!(well_terminated(&evs), "{label}");
        let u = last_usage(&evs).expect("usage");
        assert_eq!(
            (u.input_tokens, u.output_tokens),
            (Some(25), Some(15)),
            "{label}"
        );
        assert!(
            matches!(evs[0], ModelEvent::ResponseStarted { .. }),
            "{label}"
        );
    }
}

#[tokio::test]
async fn provisional_output_tokens_from_message_start_are_not_reported() {
    let rig = anthropic_rig();
    rig.transport.push(fx("01-text.sse"));
    let evs = rig.run(rig.request("hi")).await;
    for e in &evs {
        if let ModelEvent::Usage(u) = e {
            assert_ne!(u.output_tokens, Some(1), "provisional count leaked: {u:?}");
        }
    }
}

#[tokio::test]
async fn split_tool_call_is_assembled_once() {
    for (label, evs) in run_all("02-tool-call-split.sse").await {
        let r = readies(&evs);
        assert_eq!(r.len(), 1, "{label}");
        assert_eq!(r[0].0, "toolu_01T1x1fJ34qAmk2tNTrN7Up6");
        assert_eq!(r[0].1, "get_weather");
        assert_eq!(
            r[0].2,
            json!({"location":"San Francisco, CA","unit":"fahrenheit"}),
            "{label}"
        );
        assert_eq!(stop(&evs), Some(StopReason::ToolUse), "{label}");
        assert!(texts(&evs).starts_with("Okay, let's check"), "{label}");
        assert!(rejections(&evs).is_empty());
    }
}

#[tokio::test]
async fn parallel_tool_calls() {
    for (label, evs) in run_all("03-parallel-tools.sse").await {
        let r = readies(&evs);
        assert!(r.len() >= 2, "{label}: {r:?}");
        let ids: std::collections::HashSet<_> = r.iter().map(|c| c.0.clone()).collect();
        assert_eq!(ids.len(), r.len(), "{label}");
        assert_eq!(stop(&evs), Some(StopReason::ToolUse), "{label}");
    }
}

#[tokio::test]
async fn midstream_error_fails_after_partial_output_and_never_runs_the_cut_call() {
    for (label, evs) in run_all("04-midstream-error.sse").await {
        let f = failure(&evs).unwrap_or_else(|| panic!("{label}: {evs:?}"));
        assert_eq!(f.kind, ModelErrorKind::Overloaded, "{label}");
        assert!(f.retryable);
        assert!(f.partial_output_emitted, "{label}");
        assert!(readies(&evs).is_empty(), "{label}");
        assert!(well_terminated(&evs));
        assert_eq!(texts(&evs), "I'll look that up");
    }
}

#[tokio::test]
async fn rate_limit_429_with_retry_after() {
    for (label, resp) in variants(&fx("05-rate-limit-429.http")) {
        let rig = anthropic_rig();
        rig.transport.push(resp);
        let evs = rig.run(rig.request("hi")).await;
        let f = failure(&evs).unwrap_or_else(|| panic!("{label}: {evs:?}"));
        assert_eq!(f.kind, ModelErrorKind::RateLimited);
        assert!(f.retryable);
        assert_eq!(f.retry_after, Some(Duration::from_secs(20)));
        assert_eq!(f.http_status, Some(429));
        assert_eq!(
            f.request_id.as_deref(),
            Some("req_011CRATELIMITFIXTURE00001")
        );
        assert!(!f.partial_output_emitted);
        assert!(texts(&evs).is_empty());
    }
}

#[tokio::test]
async fn spend_limit_429_is_billing_and_not_retryable() {
    let rig = anthropic_rig();
    rig.transport.push(
        ReplayResponse::sse(
            br#"{"type":"error","error":{"type":"rate_limit_error","message":"cap","details":{"error_code":"enforced_spend_limit_reached"}},"request_id":"req_x"}"#.to_vec(),
        )
        .with_status(429)
        .with_header("content-type", "application/json"),
    );
    let f = failure(&rig.run(rig.request("hi")).await).expect("failed");
    assert_eq!(f.kind, ModelErrorKind::Billing);
    assert!(!f.retryable);
    assert!(f.is_permanent());
}

#[tokio::test]
async fn thinking_then_tool_roundtrips_provider_state() {
    for (label, resp) in variants(&fx("06-thinking-then-tool.sse")) {
        let rig = anthropic_rig();
        rig.transport.push(resp);
        let evs = rig.run(rig.request("what is open?")).await;
        let states: Vec<_> = evs
            .iter()
            .filter_map(|e| match e {
                ModelEvent::ProviderState { kind, data } => Some((kind.clone(), data.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(states.len(), 1, "{label}");
        assert_eq!(states[0].0, "anthropic.thinking");
        // The state precedes the tool call in the stream.
        let ps = evs
            .iter()
            .position(|e| matches!(e, ModelEvent::ProviderState { .. }))
            .unwrap();
        let tc = evs
            .iter()
            .position(|e| matches!(e, ModelEvent::ToolCallReady { .. }))
            .unwrap();
        assert!(ps < tc, "{label}");
        assert_eq!(readies(&evs)[0].1, "list_todos");

        // Next request replays the block, in order, before the tool_use.
        let origin = ModelOrigin::from_profile(&rig.profile);
        let (id, name, args) = readies(&evs).remove(0);
        let mut req = rig.request("what is open?");
        req.messages.push(Message {
            role: Role::Assistant,
            parts: vec![
                MessagePart::ProviderState {
                    origin: origin.clone(),
                    kind: states[0].0.clone(),
                    data: states[0].1.clone(),
                },
                MessagePart::ToolCall {
                    id: crate::schema::ToolCallId::from_string(id.clone()),
                    name,
                    arguments: args,
                },
            ],
        });
        req.messages.push(Message {
            role: Role::Tool,
            parts: vec![MessagePart::ToolResult {
                call_id: crate::schema::ToolCallId::from_string(id),
                content: "[]".into(),
                is_error: false,
            }],
        });
        rig.transport.push(fx("01-text.sse"));
        let _ = rig.run(req).await;
        let body = rig.transport.request_json(1).expect("second request");
        let assistant = &body["messages"][1];
        assert_eq!(assistant["role"], "assistant");
        assert_eq!(
            assistant["content"][0]["type"], "thinking",
            "{label}: {assistant}"
        );
        assert_eq!(
            assistant["content"][0]["signature"],
            "EqQBCgIYAhIM1gbcDa9GJwZA2b3hGgxBdjrkzLoky3dl1pkiMOYds_FIXTURE_NOT_A_REAL_SIGNATURE"
        );
        assert_eq!(assistant["content"][1]["type"], "tool_use");
    }
}

#[tokio::test]
async fn provider_state_of_another_origin_is_not_sent() {
    let rig = anthropic_rig();
    rig.transport.push(fx("01-text.sse"));
    let other = ModelOrigin {
        provider: "openai".into(),
        protocol: "openai_chat_completions".into(),
        model: "gpt-test-1".into(),
    };
    let mut req = rig.request("hi");
    req.messages.push(Message {
        role: Role::Assistant,
        parts: vec![
            MessagePart::ProviderState {
                origin: other,
                kind: "anthropic.thinking".into(),
                data: json!({"type":"thinking","thinking":"x","signature":"S"}),
            },
            MessagePart::Text {
                text: "earlier answer".into(),
            },
        ],
    });
    req.messages.push(Message::text(Role::User, "and now?"));
    let _ = rig.run(req).await;
    let body = rig.transport.request_json(0).unwrap().to_string();
    assert!(!body.contains("thinking"), "{body}");
    assert!(body.contains("earlier answer"));
}

#[tokio::test]
async fn duplicated_events_do_not_duplicate_calls() {
    let base = fixture("anthropic-messages/06-thinking-then-tool.sse");
    // Replay every event twice with the same SSE id: the second copy must be ignored.
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
    let rig = anthropic_rig();
    rig.transport
        .push(ReplayResponse::sse(doubled.into_bytes()).chunked(5));
    let evs = rig.run(rig.request("x")).await;
    assert_eq!(readies(&evs).len(), 1, "{evs:?}");
    assert_eq!(
        evs.iter()
            .filter(|e| matches!(e, ModelEvent::ProviderState { .. }))
            .count(),
        1
    );
    assert_eq!(stop(&evs), Some(StopReason::ToolUse));
}

#[tokio::test]
async fn disconnect_mid_stream_fails_as_network_with_partial_flag() {
    let total = fx("02-tool-call-split.sse").body.len();
    for cut in [50, 600, total / 2] {
        let rig = anthropic_rig();
        rig.transport.push(
            fx("02-tool-call-split.sse")
                .chunked(11)
                .disconnect_after(cut),
        );
        let evs = rig.run(rig.request("hi")).await;
        let f = failure(&evs).unwrap_or_else(|| panic!("cut {cut}: {evs:?}"));
        assert_eq!(f.kind, ModelErrorKind::Network, "cut {cut}");
        assert!(
            readies(&evs).is_empty(),
            "cut {cut}: no executable call from a cut stream"
        );
        assert_eq!(
            f.partial_output_emitted,
            !texts(&evs).is_empty(),
            "cut {cut}"
        );
        assert!(well_terminated(&evs));
    }
}

#[tokio::test]
async fn clean_eof_before_message_stop_is_a_network_failure() {
    let rig = anthropic_rig();
    rig.transport
        .push(fx("01-text.sse").with_fault(Fault::TruncateAfter(700)));
    let evs = rig.run(rig.request("hi")).await;
    let f = failure(&evs).expect("failed");
    // A body that ends before `message_stop` is a cut connection: retryable network error.
    assert_eq!(f.kind, ModelErrorKind::Network);
    assert!(f.retryable);
}

fn tool_stream(args_json: &str, name: &str, stop_reason: &str) -> String {
    let esc = serde_json::to_string(args_json).unwrap();
    format!(
        "event: message_start\ndata: {{\"type\":\"message_start\",\"message\":{{\"id\":\"m\",\"usage\":{{\"input_tokens\":5,\"output_tokens\":1}}}}}}\n\n\
         event: content_block_start\ndata: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"tool_use\",\"id\":\"toolu_bad\",\"name\":\"{name}\",\"input\":{{}}}}}}\n\n\
         event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"input_json_delta\",\"partial_json\":{esc}}}}}\n\n\
         event: content_block_stop\ndata: {{\"type\":\"content_block_stop\",\"index\":0}}\n\n\
         event: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"{stop_reason}\"}},\"usage\":{{\"output_tokens\":9}}}}\n\n\
         event: message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n"
    )
}

#[tokio::test]
async fn malformed_arguments_are_rejected_not_failed() {
    let rig = anthropic_rig();
    rig.transport.push(
        ReplayResponse::sse(tool_stream("{\"path\": \"C:/Us", "read_file", "tool_use")).chunked(4),
    );
    let evs = rig.run(rig.request("hi")).await;
    let rj = rejections(&evs);
    assert_eq!(rj.len(), 1, "{evs:?}");
    assert_eq!(rj[0].0, "toolu_bad");
    assert_eq!(rj[0].1.as_deref(), Some("read_file"));
    assert!(!rj[0].2.is_empty());
    assert!(readies(&evs).is_empty());
    assert_eq!(
        stop(&evs),
        Some(StopReason::ToolUse),
        "the turn completes: {evs:?}"
    );
}

#[tokio::test]
async fn unknown_tool_and_schema_violations_are_rejected() {
    let rig = anthropic_rig();
    rig.transport.push(ReplayResponse::sse(tool_stream(
        "{}",
        "delete_everything",
        "tool_use",
    )));
    let evs = rig.run(rig.request("hi")).await;
    let rj = rejections(&evs);
    assert_eq!(rj.len(), 1, "{evs:?}");
    assert!(rj[0].2.contains("delete_everything"), "{}", rj[0].2);
    assert_eq!(stop(&evs), Some(StopReason::ToolUse));

    let rig = anthropic_rig();
    rig.transport.push(ReplayResponse::sse(tool_stream(
        "{}",
        "read_file",
        "tool_use",
    )));
    let evs = rig.run(rig.request("hi")).await;
    assert_eq!(
        rejections(&evs).len(),
        1,
        "missing required `path`: {evs:?}"
    );
}

#[tokio::test]
async fn max_tokens_cut_call_is_rejected_and_stop_reason_is_max_tokens() {
    let rig = anthropic_rig();
    rig.transport.push(ReplayResponse::sse(tool_stream(
        "{\"path\": \"a",
        "read_file",
        "max_tokens",
    )));
    let evs = rig.run(rig.request("hi")).await;
    assert_eq!(rejections(&evs).len(), 1);
    assert_eq!(stop(&evs), Some(StopReason::MaxTokens));
}

#[tokio::test]
async fn stop_reasons_map() {
    for (raw, want) in [
        ("end_turn", StopReason::EndTurn),
        ("stop_sequence", StopReason::StopSequence),
        ("max_tokens", StopReason::MaxTokens),
        ("refusal", StopReason::Refusal),
        ("pause_turn", StopReason::Paused),
        ("model_context_window_exceeded", StopReason::ContextExceeded),
    ] {
        let rig = anthropic_rig();
        rig.transport.push(ReplayResponse::sse(anthropic_sse(
            &[Item::Text("ab")],
            raw,
            3,
            4,
        )));
        let evs = rig.run(rig.request("hi")).await;
        assert_eq!(stop(&evs), Some(want), "{raw}");
    }
    let rig = anthropic_rig();
    rig.transport.push(ReplayResponse::sse(anthropic_sse(
        &[Item::Text("ab")],
        "brand_new",
        3,
        4,
    )));
    assert!(matches!(
        stop(&rig.run(rig.request("hi")).await),
        Some(StopReason::Other { .. })
    ));
}

#[tokio::test]
async fn cancellation_mid_stream_yields_cancelled() {
    use futures::StreamExt;
    use tokio_util::sync::CancellationToken;
    let rig = anthropic_rig();
    rig.transport.push(
        fx("01-text.sse")
            .chunked(40)
            .with_fault(Fault::StallAfter(after_first_text())),
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
    assert!(!f.retryable);
    assert!(f.partial_output_emitted);
    assert!(well_terminated(&evs));
}

#[tokio::test]
async fn idle_timeout_uses_the_injected_clock() {
    use futures::StreamExt;
    use tokio_util::sync::CancellationToken;
    let rig = anthropic_rig();
    rig.transport.push(
        fx("01-text.sse")
            .chunked(40)
            .with_fault(Fault::StallAfter(after_first_text())),
    );
    let mut req = rig.request("hi");
    req.idle_timeout = Some(Duration::from_secs(30));
    let mut s = rig.adapter.stream(req, CancellationToken::new());
    let mut evs = Vec::new();
    let clock = rig.clock.clone();
    let driver = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(5)).await;
            clock.advance(Duration::from_secs(5));
        }
    });
    while let Some(e) = tokio::time::timeout(Duration::from_secs(10), s.next())
        .await
        .expect("stream hung")
    {
        evs.push(e);
    }
    driver.abort();
    let f = failure(&evs).expect("failed");
    assert_eq!(f.kind, ModelErrorKind::Timeout);
    assert!(f.retryable);
}

#[tokio::test]
async fn http_errors_map_by_status() {
    for (status, body, kind, retryable) in [
        (
            401,
            r#"{"type":"error","error":{"type":"authentication_error","message":"bad key"}}"#,
            ModelErrorKind::Auth,
            false,
        ),
        (
            403,
            r#"{"type":"error","error":{"type":"permission_error","message":"no"}}"#,
            ModelErrorKind::Permission,
            false,
        ),
        (
            402,
            r#"{"type":"error","error":{"type":"billing_error","message":"pay"}}"#,
            ModelErrorKind::Billing,
            false,
        ),
        (
            400,
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 250000 tokens > 200000 maximum"}}"#,
            ModelErrorKind::ContextTooLong,
            false,
        ),
        (
            400,
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"bad field"}}"#,
            ModelErrorKind::InvalidRequest,
            false,
        ),
        (
            413,
            r#"{"type":"error","error":{"type":"request_too_large","message":"big"}}"#,
            ModelErrorKind::InvalidRequest,
            false,
        ),
        (
            500,
            r#"{"type":"error","error":{"type":"api_error","message":"oops"}}"#,
            ModelErrorKind::Server,
            true,
        ),
        (
            529,
            r#"{"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#,
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
        let rig = anthropic_rig();
        rig.transport.push(
            ReplayResponse::sse(body.as_bytes().to_vec())
                .with_status(status)
                .with_header("content-type", "application/json"),
        );
        let f = failure(&rig.run(rig.request("hi")).await).expect("failed");
        assert_eq!(f.kind, kind, "{status} {body}");
        assert_eq!(f.retryable, retryable, "{status}");
        assert_eq!(f.http_status, Some(status));
        assert!(!f.partial_output_emitted);
    }
}

#[tokio::test]
async fn transport_connect_failures_are_typed() {
    use crate::ports::TransportError;
    for (err, kind) in [
        (TransportError::Timeout("t".into()), ModelErrorKind::Timeout),
        (TransportError::Connect("c".into()), ModelErrorKind::Network),
    ] {
        let rig = anthropic_rig();
        rig.transport.push(ReplayResponse::connect_failure(err));
        let f = failure(&rig.run(rig.request("hi")).await).expect("failed");
        assert_eq!(f.kind, kind);
        assert!(f.retryable);
    }
}

// ---- request building ---------------------------------------------------------------------

fn conversation(origin: &ModelOrigin) -> Vec<Message> {
    let _ = origin;
    let tc = |id: &str| crate::schema::ToolCallId::from_string(id);
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
                    id: tc("toolu_1"),
                    name: "get_weather".into(),
                    arguments: json!({"location":"Paris"}),
                },
                MessagePart::ToolCall {
                    id: tc("toolu_2"),
                    name: "get_time".into(),
                    arguments: json!({"timezone":"UTC"}),
                },
            ],
        },
        Message {
            role: Role::Tool,
            parts: vec![
                MessagePart::ToolResult {
                    call_id: tc("toolu_1"),
                    content: "sunny".into(),
                    is_error: false,
                },
                MessagePart::ToolResult {
                    call_id: tc("toolu_2"),
                    content: "boom".into(),
                    is_error: true,
                },
            ],
        },
    ]
}

#[tokio::test]
async fn request_body_snapshot() {
    let rig = anthropic_rig();
    rig.transport.push(fx("01-text.sse"));
    let origin = ModelOrigin::from_profile(&rig.profile);
    let mut req = rig.request("ignored");
    req.messages = conversation(&origin);
    req.tools.truncate(2);
    req.output_budget = Some(1000);
    req.tool_choice = ToolChoice::Auto;
    let _ = rig.run(req).await;
    let body = rig.transport.request_json(0).unwrap();
    let want = json!({
        "model": "claude-test-1",
        "max_tokens": 1000,
        "stream": true,
        "system": "You are a test assistant.",
        "messages": [
            {"role":"user","content":[{"type":"text","text":"What is the weather in Paris and the time in UTC?"}]},
            {"role":"assistant","content":[
                {"type":"text","text":"Checking."},
                {"type":"tool_use","id":"toolu_1","name":"get_weather","input":{"location":"Paris"}},
                {"type":"tool_use","id":"toolu_2","name":"get_time","input":{"timezone":"UTC"}}
            ]},
            {"role":"user","content":[
                {"type":"tool_result","tool_use_id":"toolu_1","content":"sunny"},
                {"type":"tool_result","tool_use_id":"toolu_2","content":"boom","is_error":true}
            ]}
        ],
        "tools": [
            {"name":"get_weather","description":"get_weather tool","input_schema":{"type":"object","required":["location"],"properties":{}}},
            {"name":"get_time","description":"get_time tool","input_schema":{"type":"object","required":["timezone"],"properties":{}}}
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
async fn tool_result_text_after_results_and_role_merging() {
    let rig = anthropic_rig();
    rig.transport.push(fx("01-text.sse"));
    let origin = ModelOrigin::from_profile(&rig.profile);
    let mut msgs = conversation(&origin);
    msgs.push(Message::text(Role::User, "thanks, now summarise"));
    let mut req = rig.request("x");
    req.messages = msgs;
    let _ = rig.run(req).await;
    let body = rig.transport.request_json(0).unwrap();
    let m = body["messages"].as_array().unwrap();
    assert_eq!(m.len(), 3, "consecutive user messages merge: {body}");
    let last = m[2]["content"].as_array().unwrap();
    assert_eq!(last[0]["type"], "tool_result");
    assert_eq!(last[1]["type"], "tool_result");
    assert_eq!(last[2]["type"], "text", "tool results first, text after");
}

#[tokio::test]
async fn unpaired_tool_result_is_rejected_before_any_request() {
    let rig = anthropic_rig();
    let mut req = rig.request("x");
    req.messages.push(Message {
        role: Role::Tool,
        parts: vec![MessagePart::ToolResult {
            call_id: crate::schema::ToolCallId::from_string("nope"),
            content: "x".into(),
            is_error: false,
        }],
    });
    let f = failure(&rig.run(req).await).expect("failed");
    assert_eq!(f.kind, ModelErrorKind::InvalidRequest);
    assert!(rig.transport.requests().is_empty());
}

#[tokio::test]
async fn headers_and_host_and_no_sampling_parameters() {
    let rig = anthropic_rig();
    rig.transport.push(fx("01-text.sse"));
    let mut req = rig.request("hi");
    req.reasoning = Some(ReasoningConfig {
        effort: Some(ReasoningEffort::High),
        budget_tokens: None,
    });
    let _ = rig.run(req).await;
    let r = &rig.transport.requests()[0];
    assert_eq!(r.url, "https://api.anthropic.com/v1/messages");
    assert_eq!(r.header("x-api-key"), Some(SECRET));
    assert!(r.header("anthropic-version").is_some());
    let body = rig.transport.request_json(0).unwrap();
    for k in ["temperature", "top_p", "top_k"] {
        assert!(body.get(k).is_none(), "{k} must not be sent");
    }
}

#[tokio::test]
async fn capability_gating_drops_what_the_profile_rejects() {
    let mut p = anthropic_profile();
    p.capabilities.reasoning = Support::Unsupported;
    p.capabilities.tool_calling = Support::Unsupported;
    p.capabilities.parallel_tools = Support::Unsupported;
    p.capabilities.image_input = Support::Unsupported;
    let rig = anthropic_rig_with(p, Default::default());
    rig.transport.push(fx("01-text.sse"));
    let mut req = rig.request("hi");
    req.reasoning = Some(ReasoningConfig {
        effort: Some(ReasoningEffort::High),
        budget_tokens: Some(2000),
    });
    req.parallel_tool_calls = Some(false);
    req.tool_choice = ToolChoice::Required;
    // Offering tools to a model that cannot call them is an error, not a silent drop.
    let f = failure(&rig.run(req.clone()).await).expect("failed");
    assert_eq!(f.kind, ModelErrorKind::UnsupportedCapability);
    assert!(rig.transport.requests().is_empty());
    req.tools.clear();
    req.tool_choice = ToolChoice::Auto;
    let f = failure(&rig.run(req.clone()).await).expect("failed");
    assert_eq!(
        f.kind,
        ModelErrorKind::UnsupportedCapability,
        "reasoning on a non-reasoning profile"
    );
    req.reasoning = None;
    let evs = rig.run(req).await;
    let body = rig
        .transport
        .request_json(0)
        .unwrap_or_else(|| panic!("{evs:?}"));
    assert!(
        body.get("thinking").is_none() && body.get("output_config").is_none(),
        "{body}"
    );
    assert!(
        body.get("tools").is_none() && body.get("tool_choice").is_none(),
        "{body}"
    );
}

#[tokio::test]
async fn tool_choice_and_parallel_flag_are_mapped() {
    let rig = anthropic_rig();
    rig.transport.push(fx("01-text.sse"));
    let mut req = rig.request("hi");
    req.tool_choice = ToolChoice::Tool {
        name: "get_time".into(),
    };
    req.parallel_tool_calls = Some(false);
    req.stop_sequences = vec!["END".into()];
    let _ = rig.run(req).await;
    let body = rig.transport.request_json(0).unwrap();
    assert_eq!(
        body["tool_choice"],
        json!({"type":"tool","name":"get_time","disable_parallel_tool_use":true})
    );
    assert_eq!(body["stop_sequences"], json!(["END"]));
}

#[tokio::test]
async fn images_are_sent_only_when_supported_and_by_stored_data() {
    use crate::schema::ArtifactId;
    let rig = anthropic_rig();
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
        body["messages"][0]["content"][1],
        json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":"AAAA"}})
    );
}

// ---- config, secrets, probe ---------------------------------------------------------------

#[tokio::test]
async fn validate_config_checks_url_and_credential_without_network() {
    let rig = anthropic_rig();
    assert!(rig.adapter.validate_config(&rig.profile).ok);
    let r = rig.adapter.validate_config(&{
        let mut p = anthropic_profile();
        p.base_url = "http://api.anthropic.com".into();
        p
    });
    assert!(!r.ok, "plain http to a remote host is refused: {r:?}");
    let r = rig.adapter.validate_config(&{
        let mut p = anthropic_profile();
        p.credential_ref = None;
        p
    });
    assert!(!r.ok, "remote host needs a credential");
    let r = rig.adapter.validate_config(&{
        let mut p = anthropic_profile();
        p.credential_ref = Some("missing-ref".into());
        p
    });
    assert!(!r.ok, "credential must exist");
    assert!(rig.transport.requests().is_empty(), "no network call");
}

#[tokio::test]
async fn missing_credential_fails_without_sending_anything() {
    let rig = anthropic_rig();
    rig.creds.delete("anthropic-key").unwrap();
    let f = failure(&rig.run(rig.request("hi")).await).expect("failed");
    assert_eq!(f.kind, ModelErrorKind::Auth);
    assert!(rig.transport.requests().is_empty());
}

#[tokio::test]
async fn the_secret_never_appears_in_events_errors_or_debug_output() {
    // A server that echoes the key into its error body must not get it into our errors.
    let rig = anthropic_rig();
    let echo = format!(
        r#"{{"type":"error","error":{{"type":"authentication_error","message":"invalid x-api-key {SECRET}"}}}}"#
    );
    rig.transport.push(
        ReplayResponse::sse(echo.into_bytes())
            .with_status(401)
            .with_header("content-type", "application/json"),
    );
    let evs = rig.run(rig.request("hi")).await;
    let ser = serde_json::to_string(&evs).unwrap();
    assert!(!ser.contains(SECRET), "events: {ser}");
    assert!(!format!("{evs:?}").contains(SECRET));
    let f = failure(&evs).unwrap();
    assert!(!f.to_luma_error().to_string().contains(SECRET));
    assert!(!format!("{:?}", f.to_luma_error()).contains(SECRET));

    // Successful streams, requests, adapter and config Debug output.
    rig.transport.push(fx("01-text.sse"));
    let evs = rig.run(rig.request("hi")).await;
    assert!(!serde_json::to_string(&evs).unwrap().contains(SECRET));
    let reqs = rig.transport.requests();
    assert!(
        reqs.iter().all(|r| !format!("{r:?}").contains(SECRET)),
        "HttpRequest Debug redacts the key"
    );
    assert!(!format!("{:?}", rig.profile).contains(SECRET));
    assert!(!format!("{:?}", AnthropicConfig::default()).contains(SECRET));
    // The body never carries it, only the one auth header does.
    for r in &reqs {
        let body = String::from_utf8_lossy(r.body.as_deref().unwrap_or_default()).into_owned();
        assert!(!body.contains(SECRET));
        assert!(!r.url.contains(SECRET));
    }
}

#[tokio::test]
async fn bearer_auth_variant_and_loopback_gateway() {
    let mut p = anthropic_profile();
    p.base_url = "http://127.0.0.1:8080".into();
    let cfg = AnthropicConfig {
        auth: AnthropicAuth::Bearer,
        allow_loopback_http: true,
        ..Default::default()
    };
    let rig = anthropic_rig_with(p, cfg);
    rig.transport.push(fx("01-text.sse"));
    let _ = rig.run(rig.request("hi")).await;
    let r = &rig.transport.requests()[0];
    assert!(r.url.starts_with("http://127.0.0.1:8080/"));
    assert_eq!(
        r.header("authorization"),
        Some(format!("Bearer {SECRET}").as_str())
    );
    assert!(r.header("x-api-key").is_none());
}

#[tokio::test]
async fn probe_makes_one_cheap_call() {
    let rig = anthropic_rig();
    rig.transport.push(fx("01-text.sse"));
    let rep = rig.adapter.probe(&rig.profile).await;
    assert!(rep.ok, "{rep:?}");
    assert_eq!(rep.streaming, Support::Supported);
    assert_eq!(rep.usage_in_stream, Support::Supported);
    assert_eq!(rig.transport.requests().len(), 1);
    let body = rig.transport.request_json(0).unwrap();
    assert!(body["max_tokens"].as_u64().unwrap() <= 256);
    assert!(body.get("tools").is_none());
}
