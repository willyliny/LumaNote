//! One streamed model turn with deadline, cancellation and bounded retries.

use futures::StreamExt;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::history::PendingCall;
use super::{AgentEventKind, ModelBinding, Orchestrator};
use crate::model::{FinishReason, ModelEvent, ModelRequest};
use crate::schema::{ConversationId, ElfError, RequestId, TaskId, Timestamp, ToolCallId};

/// What the model produced in a completed turn.
#[derive(Debug)]
pub(crate) struct TurnOutput {
    pub text: String,
    pub calls: Vec<PendingCall>,
    pub finish: FinishReason,
}

/// How a turn ended.
#[derive(Debug)]
pub(crate) enum TurnEnd {
    /// The stream completed normally.
    Done(TurnOutput),
    /// The caller's token was cancelled.
    Cancelled,
    /// The request deadline passed before the stream finished.
    Deadline,
    /// The stream failed for good (non-retryable, or retries exhausted).
    Error { error: ElfError, retryable: bool },
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

fn is_retryable(e: &ElfError) -> bool {
    matches!(
        e,
        ElfError::ExternalFailure {
            retryable: true,
            ..
        } | ElfError::Timeout { .. }
    )
}

impl Orchestrator {
    fn remaining(&self, deadline: Option<Timestamp>) -> Option<std::time::Duration> {
        deadline.map(|d| (d - self.clock.now_utc()).to_std().unwrap_or_default())
    }

    /// Streams one turn, retrying retryable failures with backoff on the injected clock.
    /// Text deltas are forwarded to the sink as they arrive; tool calls are collected
    /// (never executed here). A failed attempt's partial output is discarded.
    pub(crate) async fn stream_turn(&self, p: TurnParams<'_>) -> TurnEnd {
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
            let mut calls: Vec<PendingCall> = Vec::new();
            let mut seen: Vec<ToolCallId> = Vec::new();
            let failure: ElfError = loop {
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
                    Some(ModelEvent::TextDelta { text: t }) => {
                        self.emit(
                            p.task_id,
                            p.conversation_id,
                            None,
                            AgentEventKind::AssistantDelta { text: t.clone() },
                        );
                        text.push_str(&t);
                    }
                    Some(ModelEvent::ToolCallDelta { .. }) => {}
                    Some(ModelEvent::ToolCallReady {
                        call_id,
                        name,
                        arguments,
                    }) => {
                        if !seen.contains(&call_id) {
                            seen.push(call_id.clone());
                            calls.push(PendingCall {
                                id: call_id,
                                name,
                                arguments: normalise_args(arguments),
                            });
                        }
                    }
                    Some(ModelEvent::Usage(usage)) => self.emit(
                        p.task_id,
                        p.conversation_id,
                        None,
                        AgentEventKind::Usage { usage },
                    ),
                    Some(ModelEvent::Completed { finish_reason }) => {
                        return TurnEnd::Done(TurnOutput {
                            text,
                            calls,
                            finish: finish_reason,
                        });
                    }
                    Some(ModelEvent::Failed { error }) => break error,
                    None => {
                        break ElfError::ExternalFailure {
                            message: "model stream ended without a final event".into(),
                            retryable: true,
                        }
                    }
                }
            };

            if failure == ElfError::Cancelled {
                return TurnEnd::Cancelled;
            }
            let retryable = is_retryable(&failure);
            if !retryable || attempt >= self.config.max_stream_retries {
                return TurnEnd::Error {
                    error: failure,
                    retryable,
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
            let backoff = self.config.retry_backoff * 2u32.saturating_pow(attempt - 1);
            tokio::select! {
                biased;
                () = p.token.cancelled() => return TurnEnd::Cancelled,
                () = self.clock.sleep(backoff) => {}
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
