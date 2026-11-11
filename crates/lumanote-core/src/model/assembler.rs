//! Reassembly of streamed tool-call fragments (handbook 7.1: "工具參數片段必須完整組裝、
//! 解析並驗證後，才能產生 ToolCallReady").
//!
//! A call that cannot be used (malformed JSON, not an object, missing required field,
//! unknown tool, cut off) does **not** fail the stream: [`ToolCallAssembler::finish`]
//! returns a `ToolCallRejected` event, which the orchestrator answers to the model as an
//! error result while the turn carries on.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::contracts::{validate_tool_arguments, ModelEvent, ToolSpec};
use crate::schema::ToolCallId;

const MAX_REASON_CHARS: usize = 300;

#[derive(Debug, Default)]
struct PendingCall {
    name: Option<String>,
    buffer: String,
}

/// Reassembles streamed tool-call fragments. Adapters feed it provider fragments and only
/// emit what [`ToolCallAssembler::finish`] / [`ToolCallAssembler::reject_incomplete`] return.
#[derive(Debug, Default)]
pub struct ToolCallAssembler {
    pending: HashMap<ToolCallId, PendingCall>,
    order: Vec<ToolCallId>,
    done: HashSet<ToolCallId>,
}

fn clip(s: &str) -> String {
    if s.chars().count() <= MAX_REASON_CHARS {
        return s.to_owned();
    }
    let cut: String = s.chars().take(MAX_REASON_CHARS).collect();
    format!("{cut}...")
}

fn rejected(call_id: &ToolCallId, name: Option<String>, reason: String) -> ModelEvent {
    ModelEvent::ToolCallRejected {
        call_id: call_id.clone(),
        name,
        reason: clip(&reason),
    }
}

impl ToolCallAssembler {
    /// Creates an empty assembler.
    pub fn new() -> Self {
        Self::default()
    }

    /// True if `call_id` was already finished (later events for it are duplicates).
    pub fn is_done(&self, call_id: &ToolCallId) -> bool {
        self.done.contains(call_id)
    }

    /// Appends a fragment. Returns `false` (and ignores it) if the call already finished.
    pub fn push_fragment(
        &mut self,
        call_id: &ToolCallId,
        name: Option<&str>,
        fragment: &str,
    ) -> bool {
        if self.done.contains(call_id) {
            return false;
        }
        if !self.pending.contains_key(call_id) {
            self.order.push(call_id.clone());
        }
        let p = self.pending.entry(call_id.clone()).or_default();
        if let Some(n) = name {
            if !n.is_empty() {
                p.name = Some(n.to_owned());
            }
        }
        p.buffer.push_str(fragment);
        true
    }

    /// True if fragments (not just a name) arrived for `call_id`.
    pub fn has_arguments(&self, call_id: &ToolCallId) -> bool {
        self.pending
            .get(call_id)
            .is_some_and(|p| !p.buffer.is_empty())
    }

    /// True while some call has fragments but was never finished.
    pub fn has_incomplete(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Finishes a call: parses and validates its arguments against `tools`. Returns
    /// `ToolCallReady` or `ToolCallRejected`, or `None` if the call was already finished
    /// (duplicate end event). An empty argument buffer is treated as `{}`.
    pub fn finish(&mut self, call_id: &ToolCallId, tools: &[ToolSpec]) -> Option<ModelEvent> {
        self.finish_with(call_id, tools, None)
    }

    /// Like [`ToolCallAssembler::finish`], but when no argument fragment ever arrived the
    /// arguments are taken from `fallback` (a provider that sends the whole input up front).
    pub fn finish_with(
        &mut self,
        call_id: &ToolCallId,
        tools: &[ToolSpec],
        fallback: Option<&Value>,
    ) -> Option<ModelEvent> {
        if self.done.contains(call_id) {
            return None;
        }
        self.done.insert(call_id.clone());
        self.order.retain(|c| c != call_id);
        let Some(p) = self.pending.remove(call_id) else {
            return Some(rejected(
                call_id,
                None,
                "the tool call ended without a name or arguments".into(),
            ));
        };
        let Some(name) = p.name else {
            return Some(rejected(
                call_id,
                None,
                "the tool call has no tool name".into(),
            ));
        };
        let Some(tool) = tools.iter().find(|t| t.name == name) else {
            let available: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
            return Some(rejected(
                call_id,
                Some(name.clone()),
                format!(
                    "unknown tool `{name}`; available tools: {}",
                    if available.is_empty() {
                        "(none)".to_owned()
                    } else {
                        available.join(", ")
                    }
                ),
            ));
        };
        let arguments: Value = if p.buffer.trim().is_empty() {
            match fallback {
                Some(v) if !v.is_null() => v.clone(),
                _ => Value::Object(serde_json::Map::new()),
            }
        } else {
            match serde_json::from_str(&p.buffer) {
                Ok(v) => v,
                Err(e) => {
                    return Some(rejected(
                        call_id,
                        Some(name.clone()),
                        format!("the arguments of `{name}` are not valid JSON ({e})"),
                    ))
                }
            }
        };
        if let Err(e) = validate_tool_arguments(tool, &arguments) {
            return Some(rejected(
                call_id,
                Some(name),
                e.to_string()
                    .trim_start_matches("external failure: ")
                    .to_owned(),
            ));
        }
        Some(ModelEvent::ToolCallReady {
            call_id: call_id.clone(),
            name,
            arguments,
        })
    }

    /// Rejects every call that never ended (the stream was cut off by an output limit),
    /// in the order they started.
    pub fn reject_incomplete(&mut self, why: &str) -> Vec<ModelEvent> {
        let ids: Vec<ToolCallId> = self.order.drain(..).collect();
        ids.into_iter()
            .filter_map(|id| {
                let p = self.pending.remove(&id)?;
                self.done.insert(id.clone());
                Some(rejected(
                    &id,
                    p.name.clone(),
                    format!("the tool call was not completed: {why}"),
                ))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tools() -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "read_file".into(),
            description: "read".into(),
            parameters_schema: json!({"type":"object","required":["path"]}),
        }]
    }

    #[test]
    fn assembler_waits_for_complete_valid_json() {
        let mut a = ToolCallAssembler::new();
        let id = ToolCallId::from_string("c1");
        a.push_fragment(&id, Some("read_file"), "{\"pa");
        a.push_fragment(&id, None, "th\":\"/a\"}");
        let ev = a.finish(&id, &tools()).unwrap();
        assert_eq!(
            ev,
            ModelEvent::ToolCallReady {
                call_id: id.clone(),
                name: "read_file".into(),
                arguments: json!({"path":"/a"})
            }
        );
        assert!(a.finish(&id, &tools()).is_none());
        assert!(!a.push_fragment(&id, None, "x"));
        assert!(!a.has_incomplete());
    }

    #[test]
    fn bad_calls_are_rejected_not_fatal() {
        let t = tools();
        for (name, buf, needle) in [
            ("read_file", "{\"path\":", "not valid JSON"),
            ("read_file", "[1]", "not a JSON object"),
            ("read_file", "{}", "path"),
            ("nope", "{}", "unknown tool `nope`"),
        ] {
            let mut a = ToolCallAssembler::new();
            let id = ToolCallId::from_string("c");
            a.push_fragment(&id, Some(name), buf);
            match a.finish(&id, &t) {
                Some(ModelEvent::ToolCallRejected {
                    call_id,
                    name: n,
                    reason,
                }) => {
                    assert_eq!(call_id, id);
                    assert_eq!(n.as_deref(), Some(name));
                    assert!(reason.contains(needle), "{reason}");
                }
                other => panic!("{buf}: {other:?}"),
            }
            assert!(!a.has_incomplete());
        }
        let mut a = ToolCallAssembler::new();
        let id = ToolCallId::from_string("c");
        a.push_fragment(&id, None, "{}");
        assert!(matches!(
            a.finish(&id, &t),
            Some(ModelEvent::ToolCallRejected { name: None, .. })
        ));
        // a call that ended without any data at all
        let id2 = ToolCallId::from_string("never-started");
        assert!(matches!(
            a.finish(&id2, &t),
            Some(ModelEvent::ToolCallRejected { .. })
        ));
    }

    #[test]
    fn fallback_input_used_only_without_fragments() {
        let t = tools();
        let mut a = ToolCallAssembler::new();
        let id = ToolCallId::from_string("c");
        a.push_fragment(&id, Some("read_file"), "");
        let ev = a
            .finish_with(&id, &t, Some(&json!({"path": "/x"})))
            .unwrap();
        assert!(
            matches!(ev, ModelEvent::ToolCallReady { arguments, .. } if arguments == json!({"path":"/x"}))
        );
        let mut a = ToolCallAssembler::new();
        a.push_fragment(&id, Some("read_file"), "{\"path\":\"/y\"}");
        let ev = a
            .finish_with(&id, &t, Some(&json!({"path": "/x"})))
            .unwrap();
        assert!(
            matches!(ev, ModelEvent::ToolCallReady { arguments, .. } if arguments == json!({"path":"/y"}))
        );
    }

    #[test]
    fn incomplete_calls_are_rejected_in_start_order() {
        let mut a = ToolCallAssembler::new();
        let (c1, c2) = (ToolCallId::from_string("c1"), ToolCallId::from_string("c2"));
        a.push_fragment(&c1, Some("read_file"), "{\"pa");
        a.push_fragment(&c2, Some("read_file"), "{");
        let evs = a.reject_incomplete("output limit reached");
        assert_eq!(evs.len(), 2);
        assert!(matches!(&evs[0], ModelEvent::ToolCallRejected { call_id, .. } if *call_id == c1));
        assert!(matches!(&evs[1], ModelEvent::ToolCallRejected { call_id, .. } if *call_id == c2));
        assert!(!a.has_incomplete());
        assert!(a.is_done(&c1));
    }

    #[test]
    fn long_reasons_are_clipped() {
        let mut a = ToolCallAssembler::new();
        let id = ToolCallId::from_string("c");
        a.push_fragment(&id, Some(&"x".repeat(1000)), "{}");
        match a.finish(&id, &tools()) {
            Some(ModelEvent::ToolCallRejected { reason, .. }) => {
                assert!(reason.chars().count() <= MAX_REASON_CHARS + 3)
            }
            other => panic!("{other:?}"),
        }
    }
}
