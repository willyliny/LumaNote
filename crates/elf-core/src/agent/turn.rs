//! One streamed model turn with deadline, cancellation and bounded retries.
//!
//! Rules enforced here (handbook 7.1, 7.4):
//! * reasoning deltas go out on their own event and are never stored or shown as answer text;
//! * provider-opaque state is kept with the assistant turn in the order it arrived;
//! * a call the adapter rejected (malformed / incomplete / unknown tool) is collected for
//!   the caller to answer as an error result; it never fails the turn;
//! * a stream that failed after output was delivered is not replayed automatically;
//!   billing / auth / request errors are not retried; `retry_after` is waited out on the
//!   injected clock (and a wait that cannot fit the budget ends the attempt instead).

use futures::StreamExt;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use super::history::PendingCall;
use super::{AgentEventKind, ModelBinding, Orchestrator};
use crate::model::{
    MessagePart, ModelError, ModelErrorKind, ModelEvent, ModelOrigin, ModelRequest, StopReason,
};
use crate::schema::{ConversationId, RequestId, TaskId, Timestamp, ToolCallId};

/// A tool call the adapter refused to hand out for execution.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RejectedCall {
    pub id: ToolCallId,
    pub name: Option<String>,
    pub reason: String,
}

/// What the model produced in a completed turn.
#[derive(Debug)]
pub(crate) struct TurnOutput {
    /// All answer text, concatenated.
    pub text: String,
    /// The assistant message in arrival order: text, provider state, tool calls (a
    /// rejected call appears as a call with empty arguments so it can be answered).
    pub parts: Vec<MessagePart>,
    /// Calls that may be executed.
    pub calls: Vec<PendingCall>,
    /// Calls that must be answered with an error instead.
    pub rejected: Vec<RejectedCall>,
    /// Why the model stopped.
    pub stop: StopReason,
}

/// How a turn ended.
#[derive(Debug)]
pub(crate) enum TurnEnd {
    /// The stream completed.
    Done(TurnOutput),
    /// The caller's token was cancelled.
    Cancelled,
    /// The request deadline passed before the stream finished.
    Deadline,
    /// The stream failed for good (permanent error, or retries exhausted / not allowed).
    /// `retryable` is true when waiting and asking again later may help.
    Error { error: ModelError, retryable: bool },
}

/// Inputs of one turn.
pub(crate) struct TurnParams<'a> {
    pub model: &'a ModelBinding,
    pub request: ModelRequest,
    pub token: &'a CancellationToken,
    pub deadline: Option<Timestamp>,
    pub task_id: Option<&'a TaskId>,
    pub conversation_id: Option<&'a ConversationId>,
}

impl Orchestrator {
    fn remaining(&self, deadline: Option<Timestamp>) -> Option<std::time::Duration> {
        deadline.map(|d| (d - self.clock.now_utc()).to_std().unwrap_or_default())
    }

    /// Streams one turn, retrying transient failures with backoff on the injected clock.
    /// Text deltas are forwarded to the sink as they arrive; tool calls are collected
    /// (never executed here). A failed attempt's partial output is discarded.
    pub(crate) async fn stream_turn(&self, p: TurnParams<'_>) -> TurnEnd {
        let origin = ModelOrigin::from_profile(&p.model.profile);
        let mut attempt: u32 = 0;
        loop {
            if self.remaining(p.deadline).is_some_and(|r| r.is_zero()) {
                return TurnEnd::Deadline;
            }
            let stream_token = p.token.child_token();
            let mut request = p.request.clone();
            request.request_id = RequestId::new();
            request.deadline = p.deadline;
            let mut stream = p.model.adapter.stream(request, stream_token.clone());

            let remaining = self.remaining(p.deadline);
            let clock = self.clock.clone();
            let deadline_hit = async move {
                match remaining {
                    Some(r) => clock.sleep(r).await,
                    None => std::future::pending::<()>().await,
                }
            };
            tokio::pin!(deadline_hit);

            let mut text = String::new();
            let mut parts: Vec<MessagePart> = Vec::new();
            let mut calls: Vec<PendingCall> = Vec::new();
            let mut rejected: Vec<RejectedCall> = Vec::new();
            let mut seen: Vec<ToolCallId> = Vec::new();
            let mut content_seen = false;
            let mut failure: ModelError = loop {
                let ev = tokio::select! {
                    biased;
                    () = p.token.cancelled() => {
                        stream_token.cancel();
                        return TurnEnd::Cancelled;
                    }
                    () = &mut deadline_hit => {
                        stream_token.cancel();
                        return TurnEnd::Deadline;
                    }
                    ev = stream.next() => ev,
                };
                match ev {
                    Some(ModelEvent::ResponseStarted {
                        request_id, model, ..
                    }) => self.emit(
                        p.task_id,
                        p.conversation_id,
                        None,
                        AgentEventKind::ModelStarted {
                            provider: p.model.profile.provider.clone(),
                            model,
                            request_id,
                        },
                    ),
                    Some(ModelEvent::TextDelta { text: t }) => {
                        content_seen = true;
                        self.emit(
                            p.task_id,
                            p.conversation_id,
                            None,
                            AgentEventKind::AssistantDelta { text: t.clone() },
                        );
                        text.push_str(&t);
                        match parts.last_mut() {
                            Some(MessagePart::Text { text: last }) => last.push_str(&t),
                            _ => parts.push(MessagePart::Text { text: t }),
                        }
                    }
                    Some(ModelEvent::ReasoningDelta { text: t }) => {
                        content_seen = true;
                        self.emit(
                            p.task_id,
                            p.conversation_id,
                            None,
                            AgentEventKind::ReasoningDelta { text: t },
                        );
                    }
                    Some(ModelEvent::ProviderState { kind, data }) => {
                        content_seen = true;
                        parts.push(MessagePart::ProviderState {
                            origin: origin.clone(),
                            kind,
                            data,
                        });
                    }
                    Some(ModelEvent::ToolCallDelta { .. }) => content_seen = true,
                    Some(ModelEvent::ToolCallReady {
                        call_id,
                        name,
                        arguments,
                    }) => {
                        content_seen = true;
                        if !seen.contains(&call_id) {
                            seen.push(call_id.clone());
                            let arguments = normalise_args(arguments);
                            parts.push(MessagePart::ToolCall {
                                id: call_id.clone(),
                                name: name.clone(),
                                arguments: arguments.clone(),
                            });
                            calls.push(PendingCall {
                                id: call_id,
                                name,
                                arguments,
                            });
                        }
                    }
                    Some(ModelEvent::ToolCallRejected {
                        call_id,
                        name,
                        reason,
                    }) => {
                        content_seen = true;
                        if !seen.contains(&call_id) {
                            seen.push(call_id.clone());
                            self.emit(
                                p.task_id,
                                p.conversation_id,
                                None,
                                AgentEventKind::ToolCallRejected {
                                    call_id: call_id.clone(),
                                    tool: name.clone(),
                                    reason: reason.clone(),
                                },
                            );
                            parts.push(MessagePart::ToolCall {
                                id: call_id.clone(),
                                name: name.clone().unwrap_or_else(|| "unknown".to_owned()),
                                arguments: json!({}),
                            });
                            rejected.push(RejectedCall {
                                id: call_id,
                                name,
                                reason,
                            });
                        }
                    }
                    Some(ModelEvent::Usage(usage)) => {
                        // a cumulative snapshot: consumers keep only the newest one
                        self.emit(
                            p.task_id,
                            p.conversation_id,
                            None,
                            AgentEventKind::Usage { usage },
                        );
                    }
                    Some(ModelEvent::Completed { stop_reason }) => {
                        return TurnEnd::Done(TurnOutput {
                            text,
                            parts,
                            calls,
                            rejected,
                            stop: stop_reason,
                        });
                    }
                    Some(ModelEvent::Failed { error }) => break error,
                    None => {
                        break ModelError::new(
                            ModelErrorKind::Network,
                            "model stream ended without a final event",
                        )
                    }
                }
            };

            if failure.kind == ModelErrorKind::Cancelled || p.token.is_cancelled() {
                return TurnEnd::Cancelled;
            }
            failure.partial_output_emitted |= content_seen;
            let transient = !failure.is_permanent();
            if failure.is_final() || attempt >= self.config.max_stream_retries {
                return TurnEnd::Error {
                    error: failure,
                    retryable: transient,
                };
            }
            // wait for whichever is longer: our backoff or the provider's retry-after
            let backoff = self.config.retry_backoff * 2u32.saturating_pow(attempt);
            let wait = failure.retry_after.map_or(backoff, |ra| ra.max(backoff));
            let fits_budget = wait <= self.config.max_retry_wait
                && self.remaining(p.deadline).is_none_or(|r| wait < r);
            if !fits_budget {
                return TurnEnd::Error {
                    error: failure,
                    retryable: true,
                };
            }
            attempt += 1;
            self.emit(
                p.task_id,
                p.conversation_id,
                None,
                AgentEventKind::StreamRetry {
                    attempt,
                    reason: failure.to_string(),
                },
            );
            tokio::select! {
                biased;
                () = p.token.cancelled() => return TurnEnd::Cancelled,
                () = self.clock.sleep(wait) => {}
            }
        }
    }
}

/// A provider may send `null` for "no arguments".
fn normalise_args(v: Value) -> Value {
    if v.is_null() {
        Value::Object(serde_json::Map::new())
    } else {
        v
    }
}
