//! Machinery shared by the protocol adapters: endpoint rules, credential handling, error
//! helpers and the stream driver that turns an HTTP response into [`ModelEvent`]s.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use futures::stream::{BoxStream, StreamExt};
use tokio_util::sync::CancellationToken;
use url::{Host, Url};

use crate::model::contracts::*;
use crate::model::sse::{SseEvent, SseParser};
use crate::ports::{Clock, CredentialStore, HttpRequest, HttpTransport, TransportError};
use crate::schema::{Timestamp, ToolCallId};

/// Protocol identifier of [`super::AnthropicAdapter`] (`ModelProfile::protocol`).
pub const PROTOCOL_ANTHROPIC: &str = "anthropic_messages";
/// Protocol identifier of [`super::OpenAiChatAdapter`] (`ModelProfile::protocol`).
pub const PROTOCOL_OPENAI_CHAT: &str = "openai_chat_completions";

/// Largest error body read from a failing response.
pub(crate) const ERROR_BODY_LIMIT: usize = 64 * 1024;
const MAX_MESSAGE_CHARS: usize = 500;

/// What an adapter needs from the outside world.
#[derive(Clone)]
pub struct AdapterDeps {
    /// Network access.
    pub transport: Arc<dyn HttpTransport>,
    /// Where API keys live; read at request time only.
    pub credentials: Arc<dyn CredentialStore>,
    /// Time source for idle timeouts and `Retry-After` dates.
    pub clock: Arc<dyn Clock>,
}

/// Parses `base_url` and enforces the endpoint rules of handbook 7.3: HTTPS, except plain
/// HTTP to a loopback host when `allow_loopback_http` is set; no credentials, query or
/// fragment inside the URL.
pub(crate) fn parse_endpoint(base_url: &str, allow_loopback_http: bool) -> Result<Url, String> {
    let url =
        Url::parse(base_url.trim()).map_err(|e| format!("base_url is not a valid URL ({e})"))?;
    match url.scheme() {
        "https" => {}
        "http" => {
            if !allow_loopback_http {
                return Err("base_url uses plain http; https is required".into());
            }
            if !is_loopback(&url) {
                return Err(
                    "plain http is only allowed for loopback hosts (localhost, 127.0.0.0/8, ::1)"
                        .into(),
                );
            }
        }
        other => return Err(format!("base_url scheme `{other}` is not supported")),
    }
    if url.host().is_none() {
        return Err("base_url has no host".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("base_url must not contain credentials".into());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("base_url must not contain a query or fragment".into());
    }
    Ok(url)
}

/// True for `localhost`, `*.localhost`, 127.0.0.0/8 and `::1`.
pub(crate) fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(d)) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            d == "localhost" || d.ends_with(".localhost")
        }
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// `base` with `suffix` appended to its path (single slash between them).
pub(crate) fn join_path(base: &Url, suffix: &str) -> String {
    let mut s = base.as_str().trim_end_matches('/').to_owned();
    s.push('/');
    s.push_str(suffix.trim_start_matches('/'));
    s
}

/// Reads the profile's credential. `Ok(None)` when the profile has no credential reference.
pub(crate) fn read_secret(
    credentials: &dyn CredentialStore,
    profile: &ModelProfile,
) -> Result<Option<String>, ModelError> {
    let Some(reference) = &profile.credential_ref else {
        return Ok(None);
    };
    match credentials.get(reference) {
        Ok(Some(secret)) if !secret.trim().is_empty() => {
            if secret.chars().any(|c| c.is_control()) {
                return Err(ModelError::new(
                    ModelErrorKind::Auth,
                    "the stored credential contains control characters",
                ));
            }
            Ok(Some(secret.trim().to_owned()))
        }
        Ok(_) => Err(ModelError::new(
            ModelErrorKind::Auth,
            format!("no credential is stored under the reference `{reference}`"),
        )),
        Err(e) => Err(ModelError::new(
            ModelErrorKind::Auth,
            format!("the credential store failed: {e}"),
        )),
    }
}

/// Removes every occurrence of `secret` from `text` and shortens it.
pub(crate) fn scrub(text: &str, secret: Option<&str>) -> String {
    let mut out = match secret {
        Some(s) if !s.is_empty() => text.replace(s, "[redacted]"),
        _ => text.to_owned(),
    };
    if out.chars().count() > MAX_MESSAGE_CHARS {
        out = out.chars().take(MAX_MESSAGE_CHARS).collect::<String>() + "...";
    }
    out
}

/// First header named `name` (case-insensitive).
pub(crate) fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// `Retry-After` (seconds or an HTTP date), or the `retry-after-ms` variant some servers send.
pub(crate) fn parse_retry_after(headers: &[(String, String)], now: Timestamp) -> Option<Duration> {
    const CAP: Duration = Duration::from_secs(24 * 3600);
    if let Some(ms) = header(headers, "retry-after-ms").and_then(|v| v.trim().parse::<f64>().ok()) {
        if ms.is_finite() && ms >= 0.0 {
            return Some(Duration::from_secs_f64(ms / 1000.0).min(CAP));
        }
    }
    let v = header(headers, "retry-after")?.trim();
    if let Ok(secs) = v.parse::<f64>() {
        return (secs.is_finite() && secs >= 0.0).then(|| Duration::from_secs_f64(secs).min(CAP));
    }
    let date = chrono::DateTime::parse_from_rfc2822(v).ok()?;
    let wait = date.with_timezone(&chrono::Utc) - now;
    Some(wait.to_std().unwrap_or(Duration::ZERO).min(CAP))
}

/// The provider's request id from the usual headers.
pub(crate) fn request_id_header(headers: &[(String, String)]) -> Option<String> {
    ["request-id", "x-request-id"]
        .iter()
        .find_map(|n| header(headers, n))
        .map(str::to_owned)
}

/// A case-insensitive "does the message talk about the context window" test. This is a
/// heuristic: providers do not document a stable code for it (R2 sections 1.5, 3.5).
pub(crate) fn looks_like_context_overflow(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    m.contains("prompt is too long")
        || m.contains("context length")
        || m.contains("context window")
        || m.contains("context_length")
        || m.contains("maximum context")
        || m.contains("too many tokens")
}

/// Checks the tool-call / tool-result pairing rules both protocols share: every call of an
/// assistant message is answered by tool results that directly follow it, and no result
/// exists without its call.
pub(crate) fn validate_tool_pairing(messages: &[Message]) -> Result<(), ModelError> {
    let bad = |m: String| ModelError::new(ModelErrorKind::InvalidRequest, m);
    let mut awaiting: HashSet<ToolCallId> = HashSet::new();
    let mut seen: HashSet<ToolCallId> = HashSet::new();
    for msg in messages {
        let results: Vec<&ToolCallId> = msg
            .parts
            .iter()
            .filter_map(|p| match p {
                MessagePart::ToolResult { call_id, .. } => Some(call_id),
                _ => None,
            })
            .collect();
        let calls: Vec<&ToolCallId> = msg
            .parts
            .iter()
            .filter_map(|p| match p {
                MessagePart::ToolCall { id, .. } => Some(id),
                _ => None,
            })
            .collect();
        if !results.is_empty() {
            if msg.role != Role::Tool {
                return Err(bad("tool results must be in tool messages".into()));
            }
            for id in results {
                if !awaiting.remove(id) {
                    return Err(bad(format!(
                        "tool result for unknown or answered call {id}"
                    )));
                }
            }
            continue;
        }
        if !awaiting.is_empty() {
            return Err(bad(format!(
                "tool call(s) without a result before the next message: {}",
                awaiting
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        if !calls.is_empty() {
            if msg.role != Role::Assistant {
                return Err(bad("tool calls must be in assistant messages".into()));
            }
            for id in calls {
                if !seen.insert(id.clone()) {
                    return Err(bad(format!("tool call id {id} is used twice")));
                }
                awaiting.insert(id.clone());
            }
        }
    }
    if awaiting.is_empty() {
        Ok(())
    } else {
        Err(bad(format!(
            "tool call(s) without a result at the end of the conversation: {}",
            awaiting
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )))
    }
}

/// A stream consisting of one `Failed` event.
pub(crate) fn failed_stream(error: ModelError) -> BoxStream<'static, ModelEvent> {
    futures::stream::iter(vec![ModelEvent::Failed { error }]).boxed()
}

/// Per-protocol half of the stream driver.
pub(crate) trait Protocol: Send + 'static {
    /// Response headers arrived with a 2xx status.
    fn begin(&mut self, headers: &[(String, String)]);
    /// One SSE event. Returns `true` when the stream is finished (a final event was pushed).
    fn on_event(&mut self, event: &SseEvent, out: &mut Vec<ModelEvent>) -> bool;
    /// The body ended without a final event having been pushed: push `Failed`, or a
    /// `Completed` if the protocol tolerates a missing terminator.
    fn on_eof(&mut self, out: &mut Vec<ModelEvent>);
    /// Maps an HTTP error response (or a 2xx response that is not a stream) to an error.
    fn http_error(
        &self,
        status: u16,
        headers: &[(String, String)],
        body: &[u8],
        now: Timestamp,
    ) -> ModelError;
}

/// Everything the driver needs.
pub(crate) struct StreamJob {
    pub transport: Arc<dyn HttpTransport>,
    pub clock: Arc<dyn Clock>,
    pub request: HttpRequest,
    pub protocol: Box<dyn Protocol>,
    pub idle: Duration,
    pub cancel: CancellationToken,
}

enum Phase {
    Connect(Box<HttpRequest>),
    Body,
    Done,
}

struct Driver {
    transport: Arc<dyn HttpTransport>,
    clock: Arc<dyn Clock>,
    protocol: Box<dyn Protocol>,
    idle: Duration,
    cancel: CancellationToken,
    phase: Phase,
    body: Option<BoxStream<'static, Result<Vec<u8>, TransportError>>>,
    parser: SseParser,
    last_event_id: Option<String>,
    queue: VecDeque<ModelEvent>,
    emitted: bool,
}

fn is_content(ev: &ModelEvent) -> bool {
    !matches!(
        ev,
        ModelEvent::ResponseStarted { .. }
            | ModelEvent::Usage(_)
            | ModelEvent::Completed { .. }
            | ModelEvent::Failed { .. }
    )
}

fn transport_error(e: TransportError, phase_connecting: bool) -> ModelError {
    match e {
        TransportError::Cancelled => ModelError::cancelled(),
        TransportError::Blocked(m) => ModelError::new(ModelErrorKind::InvalidRequest, m),
        TransportError::Timeout(m) => ModelError::new(ModelErrorKind::Timeout, m),
        TransportError::Connect(m) => ModelError::new(ModelErrorKind::Network, m),
        TransportError::Disconnected(m) => ModelError::new(
            ModelErrorKind::Network,
            if phase_connecting {
                m
            } else {
                format!("the connection was lost during the answer: {m}")
            },
        ),
        TransportError::Other(m) => ModelError::new(ModelErrorKind::Network, m),
    }
}

impl Driver {
    fn push(&mut self, mut ev: ModelEvent) {
        if is_content(&ev) {
            self.emitted = true;
        }
        if let ModelEvent::Failed { error } = &mut ev {
            error.partial_output_emitted = self.emitted;
            self.phase = Phase::Done;
            self.body = None;
        }
        if matches!(ev, ModelEvent::Completed { .. }) {
            self.phase = Phase::Done;
            self.body = None;
        }
        self.queue.push_back(ev);
    }

    fn fail(&mut self, error: ModelError) {
        self.push(ModelEvent::Failed { error });
    }

    fn feed(&mut self, events: Vec<SseEvent>) {
        for ev in events {
            if matches!(self.phase, Phase::Done) {
                return;
            }
            if let Some(id) = &ev.id {
                if self.last_event_id.as_ref() == Some(id) {
                    continue; // the server resent an event with an id we already handled
                }
                self.last_event_id = Some(id.clone());
            }
            let mut out = Vec::new();
            let finished = self.protocol.on_event(&ev, &mut out);
            for o in out {
                self.push(o);
            }
            if finished {
                self.phase = Phase::Done;
                self.body = None;
                return;
            }
        }
    }

    async fn read_error_body(&mut self) -> Vec<u8> {
        let mut buf = Vec::new();
        let Some(mut body) = self.body.take() else {
            return buf;
        };
        while buf.len() < ERROR_BODY_LIMIT {
            let next = tokio::select! {
                biased;
                () = self.cancel.cancelled() => break,
                () = self.clock.sleep(self.idle) => break,
                n = body.next() => n,
            };
            match next {
                Some(Ok(chunk)) => buf.extend(chunk),
                _ => break,
            }
        }
        buf.truncate(ERROR_BODY_LIMIT);
        buf
    }

    async fn connect(&mut self, request: HttpRequest) {
        let sending = self.transport.send_streaming(request, self.cancel.clone());
        let result = tokio::select! {
            biased;
            () = self.cancel.cancelled() => {
                self.fail(ModelError::cancelled());
                return;
            }
            () = self.clock.sleep(self.idle) => {
                self.fail(ModelError::new(
                    ModelErrorKind::Timeout,
                    format!("no response within {}s", self.idle.as_secs()),
                ));
                return;
            }
            r = sending => r,
        };
        let resp = match result {
            Ok(r) => r,
            Err(e) => return self.fail(transport_error(e, true)),
        };
        let status = resp.status;
        let headers = resp.headers.clone();
        self.body = Some(resp.body);
        let is_stream = header(&headers, "content-type")
            .map(|c| c.to_ascii_lowercase().contains("text/event-stream"))
            .unwrap_or(true);
        if !(200..300).contains(&status) || !is_stream {
            let body = self.read_error_body().await;
            let now = self.clock.now_utc();
            let mut err = self.protocol.http_error(status, &headers, &body, now);
            if err.request_id.is_none() {
                err.request_id = request_id_header(&headers);
            }
            return self.fail(err);
        }
        self.protocol.begin(&headers);
        self.phase = Phase::Body;
    }

    async fn read(&mut self) {
        let Some(body) = self.body.as_mut() else {
            self.phase = Phase::Done;
            return;
        };
        let next = tokio::select! {
            biased;
            () = self.cancel.cancelled() => {
                self.fail(ModelError::cancelled());
                return;
            }
            () = self.clock.sleep(self.idle) => {
                self.fail(ModelError::new(
                    ModelErrorKind::Timeout,
                    format!("no data from the model for {}s", self.idle.as_secs()),
                ));
                return;
            }
            n = body.next() => n,
        };
        match next {
            Some(Ok(chunk)) => match self.parser.push(&chunk) {
                Ok(events) => self.feed(events),
                Err(e) => self.fail(ModelError::new(
                    ModelErrorKind::MalformedStream,
                    e.to_string(),
                )),
            },
            Some(Err(e)) => self.fail(transport_error(e, false)),
            None => {
                let events = self.parser.finish();
                self.feed(events);
                if !matches!(self.phase, Phase::Done) {
                    let mut out = Vec::new();
                    self.protocol.on_eof(&mut out);
                    for o in out {
                        self.push(o);
                    }
                    self.phase = Phase::Done;
                    self.body = None;
                }
            }
        }
    }

    async fn advance(&mut self) {
        match std::mem::replace(&mut self.phase, Phase::Done) {
            Phase::Connect(req) => {
                if self.cancel.is_cancelled() {
                    return self.fail(ModelError::cancelled());
                }
                self.phase = Phase::Body; // replaced by `connect` on failure
                self.connect(*req).await;
            }
            Phase::Body => {
                self.phase = Phase::Body;
                self.read().await;
            }
            Phase::Done => {}
        }
    }
}

/// Runs the request and translates the answer. The returned stream ends after one
/// `Completed` or `Failed` event; dropping it closes the connection.
pub(crate) fn run(job: StreamJob) -> BoxStream<'static, ModelEvent> {
    let driver = Driver {
        transport: job.transport,
        clock: job.clock,
        protocol: job.protocol,
        idle: job.idle,
        cancel: job.cancel,
        phase: Phase::Connect(Box::new(job.request)),
        body: None,
        parser: SseParser::new(),
        last_event_id: None,
        queue: VecDeque::new(),
        emitted: false,
    };
    futures::stream::unfold(driver, |mut d| async move {
        loop {
            if let Some(ev) = d.queue.pop_front() {
                return Some((ev, d));
            }
            if matches!(d.phase, Phase::Done) {
                return None;
            }
            d.advance().await;
        }
    })
    .boxed()
}

/// Shared implementation of [`ModelAdapter::probe`]: one tiny streaming request.
pub(crate) async fn probe_with(mut events: BoxStream<'static, ModelEvent>) -> ProbeReport {
    let mut model = None;
    let mut saw_text = false;
    let mut usage = false;
    let mut completed = false;
    let mut error = None;
    while let Some(ev) = events.next().await {
        match ev {
            ModelEvent::ResponseStarted { model: m, .. } => model = m,
            ModelEvent::TextDelta { .. } => saw_text = true,
            ModelEvent::Usage(u) => usage |= u.input_tokens.is_some() || u.output_tokens.is_some(),
            ModelEvent::Completed { .. } => completed = true,
            ModelEvent::Failed { error: e } => error = Some(e),
            _ => {}
        }
    }
    ProbeReport {
        ok: completed && error.is_none(),
        model,
        text: if saw_text {
            Support::Supported
        } else {
            Support::Unverified
        },
        streaming: if completed {
            Support::Supported
        } else {
            Support::Unverified
        },
        usage_in_stream: if !completed {
            Support::Unverified
        } else if usage {
            Support::Supported
        } else {
            Support::Unsupported
        },
        error,
    }
}

/// The request used by `probe`.
pub(crate) fn probe_request(profile: &ModelProfile) -> ModelRequest {
    let mut req = ModelRequest::new(
        profile.id.clone(),
        "You are a connectivity check.",
        vec![Message::text(Role::User, "Reply with the single word OK.")],
    );
    req.output_budget = Some(256);
    req
}

/// A call id the conversation can rely on: the provider's own if it sent a usable one that
/// no earlier call in this conversation used, otherwise a deterministic local id derived
/// from the request id and the call's position. The same input always yields the same id.
pub(crate) fn stable_call_id(
    provider_id: Option<&str>,
    used: &HashSet<ToolCallId>,
    request_id: &crate::schema::RequestId,
    position: u64,
) -> ToolCallId {
    if let Some(id) = provider_id.map(str::trim).filter(|s| !s.is_empty()) {
        let candidate = ToolCallId::from_string(id);
        if !used.contains(&candidate) {
            return candidate;
        }
    }
    ToolCallId::from_string(format!("call_{request_id}_{position}"))
}

/// Ids of every tool call already present in `messages`.
pub(crate) fn used_call_ids(messages: &[Message]) -> HashSet<ToolCallId> {
    messages
        .iter()
        .flat_map(|m| m.parts.iter())
        .filter_map(|p| match p {
            MessagePart::ToolCall { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect()
}

/// Splits a text-only view of a message part list.
pub(crate) fn joined_text(parts: &[MessagePart]) -> String {
    parts
        .iter()
        .filter_map(|p| match p {
            MessagePart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_rules() {
        assert!(parse_endpoint("https://api.example.com/v1", false).is_ok());
        assert!(parse_endpoint("http://api.example.com/v1", true)
            .unwrap_err()
            .contains("loopback"));
        assert!(parse_endpoint("http://api.example.com", false)
            .unwrap_err()
            .contains("https"));
        assert!(parse_endpoint("http://localhost:11434/v1", false).is_err());
        for ok in [
            "http://localhost:11434/v1",
            "http://127.0.0.1:8000/v1",
            "http://127.1.2.3/v1",
            "http://[::1]:1234/v1",
            "http://foo.localhost/v1",
        ] {
            assert!(parse_endpoint(ok, true).is_ok(), "{ok}");
        }
        for bad in [
            "http://192.168.1.5/v1",
            "http://localhost.evil.com/v1",
            "ftp://localhost/v1",
            "https://user:pw@api.example.com",
            "https://api.example.com/v1?key=abc",
            "https://api.example.com/v1#x",
            "not a url",
            "",
        ] {
            assert!(parse_endpoint(bad, true).is_err(), "{bad}");
        }
    }

    #[test]
    fn join_path_has_single_slash() {
        let base = parse_endpoint("https://h.example/v1/", false).unwrap();
        assert_eq!(
            join_path(&base, "/chat/completions"),
            "https://h.example/v1/chat/completions"
        );
        let base = parse_endpoint("https://h.example", false).unwrap();
        assert_eq!(
            join_path(&base, "v1/messages"),
            "https://h.example/v1/messages"
        );
    }

    #[test]
    fn retry_after_forms() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-12T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let h = |k: &str, v: &str| vec![(k.to_owned(), v.to_owned())];
        assert_eq!(
            parse_retry_after(&h("Retry-After", "20"), now),
            Some(Duration::from_secs(20))
        );
        assert_eq!(
            parse_retry_after(&h("retry-after", "1.5"), now),
            Some(Duration::from_millis(1500))
        );
        assert_eq!(
            parse_retry_after(&h("retry-after-ms", "250"), now),
            Some(Duration::from_millis(250))
        );
        assert_eq!(
            parse_retry_after(&h("retry-after", "Thu, 12 Nov 2026 00:01:00 GMT"), now),
            Some(Duration::from_secs(60))
        );
        assert_eq!(
            parse_retry_after(&h("retry-after", "Wed, 11 Nov 2026 00:01:00 GMT"), now),
            Some(Duration::ZERO)
        );
        assert_eq!(parse_retry_after(&h("retry-after", "soon"), now), None);
        assert_eq!(parse_retry_after(&h("retry-after", "-5"), now), None);
        assert_eq!(parse_retry_after(&[], now), None);
        assert_eq!(
            parse_retry_after(&h("retry-after", "99999999"), now),
            Some(Duration::from_secs(24 * 3600))
        );
    }

    #[test]
    fn scrub_removes_secret_and_clips() {
        assert_eq!(
            scrub("bad key sk-abc here", Some("sk-abc")),
            "bad key [redacted] here"
        );
        let long = "x".repeat(2000);
        assert!(scrub(&long, None).chars().count() <= MAX_MESSAGE_CHARS + 3);
    }

    fn assistant_calls(ids: &[&str]) -> Message {
        Message {
            role: Role::Assistant,
            parts: ids
                .iter()
                .map(|i| MessagePart::ToolCall {
                    id: ToolCallId::from_string(*i),
                    name: "t".into(),
                    arguments: serde_json::json!({}),
                })
                .collect(),
        }
    }
    fn result(id: &str) -> Message {
        Message {
            role: Role::Tool,
            parts: vec![MessagePart::ToolResult {
                call_id: ToolCallId::from_string(id),
                content: "r".into(),
                is_error: false,
            }],
        }
    }

    #[test]
    fn tool_pairing_rules() {
        let user = Message::text(Role::User, "go");
        assert!(validate_tool_pairing(&[
            user.clone(),
            assistant_calls(&["a", "b"]),
            result("b"),
            result("a")
        ])
        .is_ok());
        // missing result at the end
        assert!(validate_tool_pairing(&[user.clone(), assistant_calls(&["a"])]).is_err());
        // a user message between call and result
        assert!(validate_tool_pairing(&[
            user.clone(),
            assistant_calls(&["a"]),
            user.clone(),
            result("a")
        ])
        .is_err());
        // result without call, duplicate answer, duplicate id
        assert!(validate_tool_pairing(&[user.clone(), result("a")]).is_err());
        assert!(validate_tool_pairing(&[
            user.clone(),
            assistant_calls(&["a"]),
            result("a"),
            result("a")
        ])
        .is_err());
        assert!(
            validate_tool_pairing(&[user.clone(), assistant_calls(&["a", "a"]), result("a")])
                .is_err()
        );
        // only some answered
        assert!(validate_tool_pairing(&[user, assistant_calls(&["a", "b"]), result("a")]).is_err());
    }
}
