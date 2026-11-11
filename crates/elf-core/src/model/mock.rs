//! Deterministic scripted [`ModelAdapter`] for tests. Not a real provider.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::stream::{BoxStream, StreamExt};
use tokio_util::sync::CancellationToken;

use super::contracts::*;
use crate::ports::Clock;
use crate::schema::{ElfError, ToolCallId};

/// One scripted provider-level step.
#[derive(Debug, Clone, PartialEq)]
pub enum ScriptStep {
    /// Wait this long on the injected [`Clock`] (cancellable).
    Delay(Duration),
    /// Provider text chunk.
    Text(String),
    /// Provider tool-call argument fragment (name usually only on the first).
    ToolFragment {
        /// Call identifier.
        call_id: ToolCallId,
        /// Tool name, if this fragment carries it.
        name: Option<String>,
        /// Raw argument text.
        fragment: String,
    },
    /// Provider signals that the tool call is complete.
    ToolEnd {
        /// Call identifier.
        call_id: ToolCallId,
    },
    /// Usage report.
    Usage(Usage),
    /// Normal completion.
    Complete(FinishReason),
    /// Mid-stream failure.
    Fail(ElfError),
}

/// A step plus an optional provider event id used to drop duplicate deliveries.
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptItem {
    /// Provider event id; a repeated id is ignored.
    pub event_id: Option<String>,
    /// The step.
    pub step: ScriptStep,
}

impl ScriptItem {
    /// A step carrying an event id (use the same id twice to simulate a duplicate).
    pub fn with_id(event_id: impl Into<String>, step: ScriptStep) -> Self {
        Self {
            event_id: Some(event_id.into()),
            step,
        }
    }
}

impl From<ScriptStep> for ScriptItem {
    fn from(step: ScriptStep) -> Self {
        Self {
            event_id: None,
            step,
        }
    }
}

/// Adapter that replays a script through the same assembly rules a real adapter must follow.
pub struct MockAdapter {
    clock: Arc<dyn Clock>,
    script: Vec<ScriptItem>,
    capabilities: ModelCapabilities,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

impl MockAdapter {
    /// Creates a mock replaying `script`, with every capability marked supported.
    pub fn new(clock: Arc<dyn Clock>, script: Vec<ScriptItem>) -> Self {
        let mut capabilities = ModelCapabilities::unverified();
        capabilities.text = Support::Supported;
        capabilities.streaming = Support::Supported;
        capabilities.tool_calling = Support::Supported;
        capabilities.structured_output = Support::Supported;
        Self {
            clock,
            script,
            capabilities,
            requests: Arc::default(),
        }
    }

    /// Overrides the advertised capabilities.
    pub fn with_capabilities(mut self, capabilities: ModelCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// Requests received so far.
    pub fn recorded_requests(&self) -> Vec<ModelRequest> {
        self.requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

struct State {
    items: VecDeque<ScriptItem>,
    assembler: ToolCallAssembler,
    seen_ids: HashSet<String>,
    tools: Vec<ToolSpec>,
    clock: Arc<dyn Clock>,
    cancel: CancellationToken,
    queue: VecDeque<ModelEvent>,
    finished: bool,
}

impl State {
    fn fail(&mut self, error: ElfError) {
        self.queue.push_back(ModelEvent::Failed { error });
        self.finished = true;
    }

    async fn advance(&mut self) {
        if self.cancel.is_cancelled() {
            return self.fail(ElfError::Cancelled);
        }
        let Some(item) = self.items.pop_front() else {
            return self.fail(ElfError::ExternalFailure {
                message: "stream ended without completion".into(),
                retryable: true,
            });
        };
        if let Some(id) = &item.event_id {
            if !self.seen_ids.insert(id.clone()) {
                return;
            }
        }
        match item.step {
            ScriptStep::Delay(d) => {
                let clock = self.clock.clone();
                tokio::select! {
                    _ = self.cancel.cancelled() => self.fail(ElfError::Cancelled),
                    _ = clock.sleep(d) => {}
                }
            }
            ScriptStep::Text(text) => self.queue.push_back(ModelEvent::TextDelta { text }),
            ScriptStep::ToolFragment {
                call_id,
                name,
                fragment,
            } => {
                if self
                    .assembler
                    .push_fragment(&call_id, name.as_deref(), &fragment)
                {
                    self.queue.push_back(ModelEvent::ToolCallDelta {
                        call_id,
                        name,
                        arguments_fragment: fragment,
                    });
                }
            }
            ScriptStep::ToolEnd { call_id } => match self.assembler.finish(&call_id, &self.tools) {
                Ok(Some(ready)) => self.queue.push_back(ready),
                Ok(None) => {}
                Err(error) => self.fail(error),
            },
            ScriptStep::Usage(u) => self.queue.push_back(ModelEvent::Usage(u)),
            ScriptStep::Complete(finish_reason) => {
                if self.assembler.has_incomplete() {
                    self.fail(ElfError::ExternalFailure {
                        message: "stream completed with an unfinished tool call".into(),
                        retryable: false,
                    });
                } else {
                    self.queue
                        .push_back(ModelEvent::Completed { finish_reason });
                    self.finished = true;
                }
            }
            ScriptStep::Fail(error) => self.fail(error),
        }
    }
}

impl ModelAdapter for MockAdapter {
    fn capabilities(&self, _profile: &ModelProfile) -> ModelCapabilities {
        self.capabilities.clone()
    }

    fn validate_config(&self, profile: &ModelProfile) -> ValidationResult {
        let mut issues = Vec::new();
        if profile.model_id.trim().is_empty() {
            issues.push("model_id is empty".to_owned());
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
        let tools = request.tools.clone();
        self.requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(request);
        let state = State {
            items: self.script.iter().cloned().collect(),
            assembler: ToolCallAssembler::new(),
            seen_ids: HashSet::new(),
            tools,
            clock: self.clock.clone(),
            cancel,
            queue: VecDeque::new(),
            finished: false,
        };
        futures::stream::unfold(state, |mut st| async move {
            loop {
                if let Some(ev) = st.queue.pop_front() {
                    return Some((ev, st));
                }
                if st.finished {
                    return None;
                }
                st.advance().await;
            }
        })
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::fakes::FakeClock;
    use crate::schema::RequestId;
    use futures::StreamExt;
    use serde_json::json;

    fn request() -> ModelRequest {
        ModelRequest {
            model_profile_id: "p".into(),
            instructions: "i".into(),
            messages: vec![Message::text(Role::User, "hi")],
            tools: vec![ToolSpec {
                name: "write_note".into(),
                description: "d".into(),
                parameters_schema: json!({"type":"object","required":["title"]}),
            }],
            output_contract: None,
            output_budget: None,
            deadline: None,
            request_id: RequestId::new(),
        }
    }

    fn frag(id: &str, name: Option<&str>, f: &str) -> ScriptItem {
        ScriptStep::ToolFragment {
            call_id: ToolCallId::from_string(id),
            name: name.map(str::to_owned),
            fragment: f.into(),
        }
        .into()
    }
    fn end(id: &str) -> ScriptItem {
        ScriptStep::ToolEnd {
            call_id: ToolCallId::from_string(id),
        }
        .into()
    }
    fn adapter(script: Vec<ScriptItem>) -> MockAdapter {
        MockAdapter::new(Arc::new(FakeClock::at_default_start()), script)
    }
    async fn collect(a: &MockAdapter) -> Vec<ModelEvent> {
        a.stream(request(), CancellationToken::new())
            .collect()
            .await
    }

    #[tokio::test]
    async fn tool_call_ready_only_after_complete_valid_json() {
        let a = adapter(vec![
            ScriptStep::Text("hello".into()).into(),
            frag("c1", Some("write_note"), "{\"ti"),
            frag("c1", None, "tle\":\"A\""),
            frag("c1", None, "}"),
            end("c1"),
            ScriptStep::Usage(Usage {
                input_tokens: Some(3),
                ..Default::default()
            })
            .into(),
            ScriptStep::Complete(FinishReason::ToolCalls).into(),
        ]);
        let evs = collect(&a).await;
        let ready_pos = evs
            .iter()
            .position(|e| matches!(e, ModelEvent::ToolCallReady { .. }))
            .unwrap();
        let deltas = evs
            .iter()
            .filter(|e| matches!(e, ModelEvent::ToolCallDelta { .. }))
            .count();
        assert_eq!(deltas, 3);
        // every delta precedes Ready, and Ready carries the full parsed object
        assert!(
            evs[..ready_pos]
                .iter()
                .filter(|e| matches!(e, ModelEvent::ToolCallDelta { .. }))
                .count()
                == 3
        );
        assert_eq!(
            evs[ready_pos],
            ModelEvent::ToolCallReady {
                call_id: ToolCallId::from_string("c1"),
                name: "write_note".into(),
                arguments: json!({"title":"A"})
            }
        );
        assert!(matches!(
            evs.last(),
            Some(ModelEvent::Completed {
                finish_reason: FinishReason::ToolCalls
            })
        ));
        assert_eq!(a.recorded_requests().len(), 1);
    }

    #[tokio::test]
    async fn no_ready_for_truncated_or_malformed_arguments() {
        // malformed JSON at end marker
        let evs = collect(&adapter(vec![
            frag("c1", Some("write_note"), "{\"title\":"),
            end("c1"),
            ScriptStep::Complete(FinishReason::ToolCalls).into(),
        ]))
        .await;
        assert!(!evs
            .iter()
            .any(|e| matches!(e, ModelEvent::ToolCallReady { .. })));
        assert!(matches!(
            evs.last(),
            Some(ModelEvent::Failed {
                error: ElfError::ExternalFailure { .. }
            })
        ));

        // completion arrives while the call is still unfinished
        let evs = collect(&adapter(vec![
            frag("c1", Some("write_note"), "{\"title\":\"x\"}"),
            ScriptStep::Complete(FinishReason::Stop).into(),
        ]))
        .await;
        assert!(!evs
            .iter()
            .any(|e| matches!(e, ModelEvent::ToolCallReady { .. })));
        assert!(matches!(evs.last(), Some(ModelEvent::Failed { .. })));
    }

    #[tokio::test]
    async fn duplicate_events_are_dropped() {
        let a = adapter(vec![
            ScriptItem::with_id("e1", ScriptStep::Text("a".into())),
            ScriptItem::with_id("e1", ScriptStep::Text("a".into())),
            ScriptItem::with_id(
                "e2",
                ScriptStep::ToolFragment {
                    call_id: ToolCallId::from_string("c1"),
                    name: Some("write_note".into()),
                    fragment: "{\"title\":".into(),
                },
            ),
            ScriptItem::with_id(
                "e2",
                ScriptStep::ToolFragment {
                    call_id: ToolCallId::from_string("c1"),
                    name: Some("write_note".into()),
                    fragment: "{\"title\":".into(),
                },
            ),
            frag("c1", None, "\"T\"}"),
            end("c1"),
            end("c1"),              // duplicate end without event id
            frag("c1", None, "{}"), // late fragment for a finished call
            ScriptStep::Complete(FinishReason::ToolCalls).into(),
        ]);
        let evs = collect(&a).await;
        assert_eq!(
            evs.iter()
                .filter(|e| matches!(e, ModelEvent::TextDelta { .. }))
                .count(),
            1
        );
        assert_eq!(
            evs.iter()
                .filter(|e| matches!(e, ModelEvent::ToolCallReady { .. }))
                .count(),
            1
        );
        assert!(evs.iter().any(|e| matches!(e, ModelEvent::ToolCallReady { arguments, .. } if *arguments == json!({"title":"T"}))));
        assert!(matches!(evs.last(), Some(ModelEvent::Completed { .. })));
    }

    #[tokio::test]
    async fn mid_stream_failure_and_missing_completion() {
        let evs = collect(&adapter(vec![
            ScriptStep::Text("par".into()).into(),
            ScriptStep::Fail(ElfError::Timeout {
                operation: "stream".into(),
            })
            .into(),
            ScriptStep::Text("never".into()).into(),
        ]))
        .await;
        assert_eq!(evs.len(), 2);
        assert!(matches!(
            &evs[1],
            ModelEvent::Failed {
                error: ElfError::Timeout { .. }
            }
        ));

        let evs = collect(&adapter(vec![ScriptStep::Text("x".into()).into()])).await;
        assert!(matches!(
            evs.last(),
            Some(ModelEvent::Failed {
                error: ElfError::ExternalFailure {
                    retryable: true,
                    ..
                }
            })
        ));
    }

    #[tokio::test]
    async fn delays_follow_the_fake_clock_and_can_be_cancelled() {
        let clock = Arc::new(FakeClock::at_default_start());
        let a = MockAdapter::new(
            clock.clone(),
            vec![
                ScriptStep::Text("a".into()).into(),
                ScriptStep::Delay(Duration::from_secs(30)).into(),
                ScriptStep::Text("b".into()).into(),
                ScriptStep::Complete(FinishReason::Stop).into(),
            ],
        );
        let mut s = a.stream(request(), CancellationToken::new());
        assert!(matches!(s.next().await, Some(ModelEvent::TextDelta { .. })));
        let h = tokio::spawn(async move { s.collect::<Vec<_>>().await });
        tokio::task::yield_now().await;
        assert!(!h.is_finished());
        clock.advance(Duration::from_secs(30));
        let rest = h.await.unwrap();
        assert_eq!(rest.len(), 2);

        // cancellation during a delay
        let cancel = CancellationToken::new();
        let a = MockAdapter::new(
            clock.clone(),
            vec![ScriptStep::Delay(Duration::from_secs(600)).into()],
        );
        let s = a.stream(request(), cancel.clone());
        let h = tokio::spawn(async move { s.collect::<Vec<_>>().await });
        tokio::task::yield_now().await;
        cancel.cancel();
        let evs = h.await.unwrap();
        assert_eq!(
            evs,
            vec![ModelEvent::Failed {
                error: ElfError::Cancelled
            }]
        );
    }

    #[tokio::test]
    async fn unknown_tool_is_rejected_and_capabilities_exposed() {
        let evs = collect(&adapter(vec![frag("c1", Some("rm_rf"), "{}"), end("c1")])).await;
        assert!(!evs
            .iter()
            .any(|e| matches!(e, ModelEvent::ToolCallReady { .. })));
        let a = adapter(vec![]);
        let profile = ModelProfile {
            id: "p".into(),
            provider: "mock".into(),
            protocol: "mock".into(),
            base_url: "".into(),
            model_id: " ".into(),
            credential_ref: None,
            capabilities: ModelCapabilities::unverified(),
            config_version: 1,
        };
        assert!(a.capabilities(&profile).can_use_tools());
        assert!(!a.validate_config(&profile).ok);
    }
}
