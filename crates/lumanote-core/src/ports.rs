//! Ports: traits through which the core reaches the outside world, plus fakes.
//!
//! The Tauri shell supplies real adapters; tests use [`fakes`].

use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::schema::{Result, TaskId, Timestamp};

/// A monotonic reading: time elapsed since an arbitrary, clock-specific origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MonoInstant(Duration);

impl MonoInstant {
    /// Creates a reading `offset` after the clock origin.
    pub fn from_offset(offset: Duration) -> Self {
        Self(offset)
    }
    /// Offset from the clock origin.
    pub fn offset(self) -> Duration {
        self.0
    }
    /// Time elapsed since `earlier` (zero if `earlier` is later).
    pub fn duration_since(self, earlier: MonoInstant) -> Duration {
        self.0.saturating_sub(earlier.0)
    }
}

/// Source of time. All time-dependent logic must use this, never the OS clock.
#[async_trait]
pub trait Clock: Send + Sync {
    /// Current wall-clock time in UTC.
    fn now_utc(&self) -> Timestamp;
    /// Monotonic reading for measuring durations.
    fn monotonic(&self) -> MonoInstant;
    /// Completes after `duration` of this clock's time has passed.
    async fn sleep(&self, duration: Duration);
}

/// A user-visible notification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    /// Notification title.
    pub title: String,
    /// Notification body text.
    pub body: String,
    /// Related task, if any.
    pub task_id: Option<TaskId>,
}

/// Delivers notifications to the user (system toast, tray, ...).
#[async_trait]
pub trait Notifier: Send + Sync {
    /// Shows a notification.
    async fn notify(&self, notification: &Notification) -> Result<()>;
}

/// OS-protected credential storage; the database only keeps references.
pub trait CredentialStore: Send + Sync {
    /// Reads a secret, `None` if absent.
    fn get(&self, credential_ref: &str) -> Result<Option<String>>;
    /// Stores or replaces a secret.
    fn set(&self, credential_ref: &str, secret: &str) -> Result<()>;
    /// Removes a secret (no error if absent).
    fn delete(&self, credential_ref: &str) -> Result<()>;
}

/// One observation of user activity (only produced when the user opted in).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivitySample {
    /// When the sample was taken.
    pub at: Timestamp,
    /// Foreground application identifier, if known.
    pub app_id: Option<String>,
    /// Whether the session was idle.
    pub idle: bool,
}

/// Source of activity samples.
#[async_trait]
pub trait ActivitySource: Send + Sync {
    /// Returns the next sample, or `None` when none is available.
    async fn poll(&self) -> Result<Option<ActivitySample>>;
}

/// Header names whose values are credentials: never printed by `Debug`.
const SENSITIVE_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "x-api-key",
    "api-key",
    "x-goog-api-key",
    "cookie",
    "set-cookie",
];

/// True if the header value is a credential and must not be printed or logged.
pub fn is_sensitive_header(name: &str) -> bool {
    SENSITIVE_HEADERS
        .iter()
        .any(|h| h.eq_ignore_ascii_case(name))
}

fn find_header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// Minimal HTTP request. `Debug` redacts credential headers.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct HttpRequest {
    /// HTTP method, e.g. `POST`.
    pub method: String,
    /// Absolute URL.
    pub url: String,
    /// Header name/value pairs.
    pub headers: Vec<(String, String)>,
    /// Optional body.
    pub body: Option<Vec<u8>>,
}

impl HttpRequest {
    /// First header named `name` (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        find_header(&self.headers, name)
    }
}

impl fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let headers: Vec<(&str, &str)> = self
            .headers
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str(),
                    if is_sensitive_header(k) {
                        "[redacted]"
                    } else {
                        v.as_str()
                    },
                )
            })
            .collect();
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("headers", &headers)
            .field("body_bytes", &self.body.as_ref().map(Vec::len))
            .finish()
    }
}

/// Minimal HTTP response.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HttpResponse {
    /// Status code.
    pub status: u16,
    /// Header name/value pairs.
    pub headers: Vec<(String, String)>,
    /// Response body.
    pub body: Vec<u8>,
}

/// Why a streaming request could not deliver (all messages are credential-free).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    /// The connection could not be established (DNS, TCP, TLS).
    #[error("connection failed: {0}")]
    Connect(String),
    /// The transport's own policy refused the request (plain HTTP to a non-loopback host,
    /// a redirect, ...). Nothing was sent.
    #[error("blocked by transport policy: {0}")]
    Blocked(String),
    /// A connect or read timeout of the transport.
    #[error("timed out: {0}")]
    Timeout(String),
    /// The connection broke while the response was being read.
    #[error("connection lost: {0}")]
    Disconnected(String),
    /// The caller's cancellation token fired.
    #[error("cancelled")]
    Cancelled,
    /// Anything else.
    #[error("transport error: {0}")]
    Other(String),
}

/// A response whose body is still arriving. Dropping the body closes the connection.
pub struct HttpStreamResponse {
    /// Status code.
    pub status: u16,
    /// Header name/value pairs.
    pub headers: Vec<(String, String)>,
    /// Body chunks as they arrive. An `Err` item ends the stream.
    pub body: BoxStream<'static, std::result::Result<Vec<u8>, TransportError>>,
}

impl HttpStreamResponse {
    /// First header named `name` (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        find_header(&self.headers, name)
    }
}

impl fmt::Debug for HttpStreamResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpStreamResponse")
            .field("status", &self.status)
            .field("headers", &self.headers)
            .finish_non_exhaustive()
    }
}

/// Network transport used by model and search adapters.
///
/// Implementations must not follow redirects to another host with credentials attached
/// (handbook 7.3); the simplest compliant behaviour is not to follow redirects at all.
#[async_trait]
pub trait HttpTransport: Send + Sync {
    /// Performs one request and buffers the whole response.
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse>;

    /// Performs one request and returns as soon as the status line and headers are in.
    /// The body arrives as a stream; dropping it (or cancelling `cancel`) closes the
    /// connection. Non-2xx statuses are ordinary responses, not errors.
    async fn send_streaming(
        &self,
        request: HttpRequest,
        cancel: CancellationToken,
    ) -> std::result::Result<HttpStreamResponse, TransportError>;
}

/// Real-time clock for production use.
#[derive(Debug)]
pub struct SystemClock {
    origin: std::time::Instant,
}

impl SystemClock {
    /// Creates a clock whose monotonic origin is now.
    pub fn new() -> Self {
        Self {
            origin: std::time::Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Clock for SystemClock {
    fn now_utc(&self) -> Timestamp {
        chrono::Utc::now()
    }
    fn monotonic(&self) -> MonoInstant {
        MonoInstant(self.origin.elapsed())
    }
    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}

/// In-memory fakes for tests and for the shell's early development.
pub mod fakes {
    use std::collections::{HashMap, VecDeque};
    use std::sync::{Arc, Mutex, MutexGuard};

    use futures::StreamExt;

    use super::*;
    use crate::schema::LumaError;

    fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[derive(Debug, Clone, Copy)]
    struct FakeState {
        utc: Timestamp,
        mono: Duration,
    }

    /// Manually advanced clock. Cloning shares the same time.
    #[derive(Debug, Clone)]
    pub struct FakeClock {
        tx: Arc<tokio::sync::watch::Sender<FakeState>>,
    }

    impl FakeClock {
        /// Creates a clock reading `start`, with monotonic origin zero.
        pub fn new(start: Timestamp) -> Self {
            let (tx, _rx) = tokio::sync::watch::channel(FakeState {
                utc: start,
                mono: Duration::ZERO,
            });
            Self { tx: Arc::new(tx) }
        }

        /// Creates a clock at a fixed, arbitrary date (2026-11-12T00:00:00Z).
        pub fn at_default_start() -> Self {
            Self::new(
                chrono::DateTime::parse_from_rfc3339("2026-11-12T00:00:00Z")
                    .map(|d| d.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now()),
            )
        }

        /// Moves both wall-clock and monotonic time forward and wakes sleepers.
        pub fn advance(&self, by: Duration) {
            self.tx.send_modify(|s| {
                s.mono += by;
                if let Some(utc) = chrono::Duration::from_std(by)
                    .ok()
                    .and_then(|d| s.utc.checked_add_signed(d))
                {
                    s.utc = utc;
                }
            });
        }

        /// Jumps wall-clock time (e.g. system time change) without touching the monotonic clock.
        pub fn set_utc(&self, utc: Timestamp) {
            self.tx.send_modify(|s| s.utc = utc);
        }
    }

    #[async_trait]
    impl Clock for FakeClock {
        fn now_utc(&self) -> Timestamp {
            self.tx.borrow().utc
        }
        fn monotonic(&self) -> MonoInstant {
            MonoInstant(self.tx.borrow().mono)
        }
        async fn sleep(&self, duration: Duration) {
            let deadline = self.tx.borrow().mono + duration;
            let mut rx = self.tx.subscribe();
            // The sender lives in `self`, so `wait_for` cannot fail with a closed channel.
            let _ = rx.wait_for(|s| s.mono >= deadline).await;
        }
    }

    /// Notifier that records everything it is asked to show.
    #[derive(Debug, Default)]
    pub struct FakeNotifier {
        sent: Mutex<Vec<Notification>>,
    }

    impl FakeNotifier {
        /// Creates an empty notifier.
        pub fn new() -> Self {
            Self::default()
        }
        /// Notifications received so far.
        pub fn sent(&self) -> Vec<Notification> {
            lock(&self.sent).clone()
        }
    }

    #[async_trait]
    impl Notifier for FakeNotifier {
        async fn notify(&self, notification: &Notification) -> Result<()> {
            lock(&self.sent).push(notification.clone());
            Ok(())
        }
    }

    /// Credential store backed by a map.
    #[derive(Debug, Default)]
    pub struct InMemoryCredentialStore {
        secrets: Mutex<HashMap<String, String>>,
    }

    impl InMemoryCredentialStore {
        /// Creates an empty store.
        pub fn new() -> Self {
            Self::default()
        }
    }

    impl CredentialStore for InMemoryCredentialStore {
        fn get(&self, credential_ref: &str) -> Result<Option<String>> {
            Ok(lock(&self.secrets).get(credential_ref).cloned())
        }
        fn set(&self, credential_ref: &str, secret: &str) -> Result<()> {
            lock(&self.secrets).insert(credential_ref.to_owned(), secret.to_owned());
            Ok(())
        }
        fn delete(&self, credential_ref: &str) -> Result<()> {
            lock(&self.secrets).remove(credential_ref);
            Ok(())
        }
    }

    /// Activity source that replays queued samples.
    #[derive(Debug, Default)]
    pub struct FakeActivitySource {
        queue: Mutex<VecDeque<ActivitySample>>,
    }

    impl FakeActivitySource {
        /// Creates an empty source.
        pub fn new() -> Self {
            Self::default()
        }
        /// Queues a sample for a later `poll`.
        pub fn push(&self, sample: ActivitySample) {
            lock(&self.queue).push_back(sample);
        }
    }

    #[async_trait]
    impl ActivitySource for FakeActivitySource {
        async fn poll(&self) -> Result<Option<ActivitySample>> {
            Ok(lock(&self.queue).pop_front())
        }
    }

    /// HTTP transport that returns queued responses and records requests.
    #[derive(Debug, Default)]
    pub struct FakeHttpTransport {
        responses: Mutex<VecDeque<Result<HttpResponse>>>,
        requests: Mutex<Vec<HttpRequest>>,
    }

    impl FakeHttpTransport {
        /// Creates a transport with no queued responses.
        pub fn new() -> Self {
            Self::default()
        }
        /// Queues the outcome of the next request.
        pub fn push_response(&self, response: Result<HttpResponse>) {
            lock(&self.responses).push_back(response);
        }
        /// Requests seen so far.
        pub fn requests(&self) -> Vec<HttpRequest> {
            lock(&self.requests).clone()
        }
    }

    #[async_trait]
    impl HttpTransport for FakeHttpTransport {
        async fn send(&self, request: HttpRequest) -> Result<HttpResponse> {
            lock(&self.requests).push(request);
            lock(&self.responses).pop_front().unwrap_or_else(|| {
                Err(LumaError::ExternalFailure {
                    message: "no queued fake response".into(),
                    retryable: false,
                })
            })
        }

        async fn send_streaming(
            &self,
            request: HttpRequest,
            _cancel: CancellationToken,
        ) -> std::result::Result<HttpStreamResponse, TransportError> {
            let resp = self
                .send(request)
                .await
                .map_err(|e| TransportError::Other(e.to_string()))?;
            Ok(HttpStreamResponse {
                status: resp.status,
                headers: resp.headers,
                body: futures::stream::iter(vec![Ok(resp.body)]).boxed(),
            })
        }
    }

    /// How a replayed body is cut into chunks. Chunk borders ignore UTF-8 characters and
    /// SSE lines on purpose: a parser must cope with a border anywhere.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Chunking {
        /// One chunk.
        Whole,
        /// Chunks of this many bytes (the last one may be shorter).
        Fixed(usize),
        /// These sizes in turn, repeating the last one.
        Sizes(Vec<usize>),
    }

    /// A failure injected into a replayed stream.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Fault {
        /// Deliver everything.
        None,
        /// After this many body bytes the connection breaks with a transport error.
        DisconnectAfter(usize),
        /// After this many body bytes the stream simply ends (a truncated body, clean EOF).
        TruncateAfter(usize),
        /// After this many body bytes nothing more ever arrives (idle timeout and
        /// cancellation tests).
        StallAfter(usize),
    }

    /// Line ending used when a fixture is replayed.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum LineEnding {
        /// `\n`, as stored in the fixtures.
        Lf,
        /// `\r\n`, as many real servers send.
        Crlf,
    }

    /// One scripted response of a [`ReplayTransport`].
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ReplayResponse {
        /// Status code.
        pub status: u16,
        /// Response headers.
        pub headers: Vec<(String, String)>,
        /// Body bytes.
        pub body: Vec<u8>,
        /// How to chunk the body.
        pub chunking: Chunking,
        /// Injected fault.
        pub fault: Fault,
        /// If set, the request fails before any response with this error.
        pub connect_error: Option<TransportError>,
    }

    impl ReplayResponse {
        /// A `200` event stream with this body.
        pub fn sse(body: impl Into<Vec<u8>>) -> Self {
            Self {
                status: 200,
                headers: vec![(
                    "content-type".into(),
                    "text/event-stream; charset=utf-8".into(),
                )],
                body: body.into(),
                chunking: Chunking::Whole,
                fault: Fault::None,
                connect_error: None,
            }
        }

        /// Builds a response from a fixture file's text. A `.http` fixture (a human
        /// preface, a `# ---` line, then a raw HTTP response) yields that response; any other
        /// text is replayed as the body of a `200` event stream.
        pub fn from_fixture(text: &str) -> Self {
            let Some(idx) = text.find("\n# ---\n") else {
                return Self::sse(text.as_bytes());
            };
            let raw = &text[idx + "\n# ---\n".len()..];
            let (head, body) = raw.split_once("\n\n").unwrap_or((raw, ""));
            let mut lines = head.lines();
            let status = lines
                .next()
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|c| c.parse::<u16>().ok())
                .unwrap_or(500);
            let headers = lines
                .filter_map(|l| l.split_once(':'))
                .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
                .collect();
            Self {
                status,
                headers,
                body: body.as_bytes().to_vec(),
                chunking: Chunking::Whole,
                fault: Fault::None,
                connect_error: None,
            }
        }

        /// A request that fails before a response arrives.
        pub fn connect_failure(error: TransportError) -> Self {
            Self {
                connect_error: Some(error),
                ..Self::sse(Vec::new())
            }
        }

        /// Adds a header.
        pub fn with_header(mut self, name: &str, value: &str) -> Self {
            self.headers.push((name.to_owned(), value.to_owned()));
            self
        }

        /// Sets the status.
        pub fn with_status(mut self, status: u16) -> Self {
            self.status = status;
            self
        }

        /// Sets the chunking.
        pub fn with_chunking(mut self, chunking: Chunking) -> Self {
            self.chunking = chunking;
            self
        }

        /// Shorthand for `Chunking::Fixed(n)`.
        pub fn chunked(self, n: usize) -> Self {
            self.with_chunking(Chunking::Fixed(n.max(1)))
        }

        /// Rewrites every `\n` of the body (not already part of `\r\n`) to the given ending.
        pub fn with_line_ending(mut self, ending: LineEnding) -> Self {
            if ending == LineEnding::Crlf {
                let mut out = Vec::with_capacity(self.body.len() + self.body.len() / 16);
                let mut prev = 0u8;
                for &b in &self.body {
                    if b == b'\n' && prev != b'\r' {
                        out.push(b'\r');
                    }
                    out.push(b);
                    prev = b;
                }
                self.body = out;
            }
            self
        }

        /// Injects a fault.
        pub fn with_fault(mut self, fault: Fault) -> Self {
            self.fault = fault;
            self
        }

        /// Shorthand: break the connection after `n` body bytes.
        pub fn disconnect_after(self, n: usize) -> Self {
            self.with_fault(Fault::DisconnectAfter(n))
        }

        /// Replaces the body.
        pub fn with_body(mut self, body: impl Into<Vec<u8>>) -> Self {
            self.body = body.into();
            self
        }
    }

    /// Reads a file below the crate's `fixtures/` directory.
    pub fn read_fixture(relative: &str) -> Result<String> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join(relative);
        std::fs::read_to_string(&path).map_err(|e| LumaError::ExternalFailure {
            message: format!("cannot read fixture {}: {e}", path.display()),
            retryable: false,
        })
    }

    /// Transport that replays scripted responses: protocol fixtures cut into arbitrary
    /// chunks, with optional injected disconnects. It records every request, so tests can
    /// assert exactly what an adapter sent. No network is involved.
    #[derive(Debug, Default)]
    pub struct ReplayTransport {
        queue: Mutex<VecDeque<ReplayResponse>>,
        requests: Mutex<Vec<HttpRequest>>,
    }

    impl ReplayTransport {
        /// A transport with nothing queued.
        pub fn new() -> Self {
            Self::default()
        }

        /// Queues the response for the next request.
        pub fn push(&self, response: ReplayResponse) {
            lock(&self.queue).push_back(response);
        }

        /// All requests seen so far.
        pub fn requests(&self) -> Vec<HttpRequest> {
            lock(&self.requests).clone()
        }

        /// The JSON body of request number `index` (0-based), if it parses.
        pub fn request_json(&self, index: usize) -> Option<serde_json::Value> {
            let reqs = lock(&self.requests);
            let body = reqs.get(index)?.body.as_ref()?;
            serde_json::from_slice(body).ok()
        }
    }

    struct ReplayState {
        body: Vec<u8>,
        pos: usize,
        sizes: Chunking,
        size_idx: usize,
        fault: Fault,
        cancel: CancellationToken,
        ended: bool,
    }

    impl ReplayState {
        fn next_size(&mut self) -> usize {
            let n = match &self.sizes {
                Chunking::Whole => usize::MAX,
                Chunking::Fixed(n) => *n,
                Chunking::Sizes(v) => v
                    .get(self.size_idx)
                    .or_else(|| v.last())
                    .copied()
                    .unwrap_or(usize::MAX),
            };
            self.size_idx += 1;
            n.max(1)
        }

        fn limit(&self) -> usize {
            match self.fault {
                Fault::DisconnectAfter(n) | Fault::TruncateAfter(n) | Fault::StallAfter(n) => {
                    n.min(self.body.len())
                }
                Fault::None => self.body.len(),
            }
        }
    }

    async fn replay_next(
        mut st: ReplayState,
    ) -> Option<(std::result::Result<Vec<u8>, TransportError>, ReplayState)> {
        if st.ended {
            return None;
        }
        tokio::task::yield_now().await;
        if st.cancel.is_cancelled() {
            st.ended = true;
            return Some((Err(TransportError::Cancelled), st));
        }
        let limit = st.limit();
        if st.pos >= limit {
            st.ended = true;
            return match st.fault {
                Fault::DisconnectAfter(_) => Some((
                    Err(TransportError::Disconnected("injected disconnect".into())),
                    st,
                )),
                Fault::StallAfter(_) => {
                    let cancel = st.cancel.clone();
                    cancel.cancelled().await;
                    Some((Err(TransportError::Cancelled), st))
                }
                Fault::TruncateAfter(_) | Fault::None => None,
            };
        }
        let n = st.next_size().min(limit - st.pos);
        let chunk = st.body[st.pos..st.pos + n].to_vec();
        st.pos += n;
        Some((Ok(chunk), st))
    }

    #[async_trait]
    impl HttpTransport for ReplayTransport {
        async fn send(&self, request: HttpRequest) -> Result<HttpResponse> {
            let mut resp = self
                .send_streaming(request, CancellationToken::new())
                .await
                .map_err(|e| LumaError::ExternalFailure {
                    message: e.to_string(),
                    retryable: true,
                })?;
            let mut body = Vec::new();
            while let Some(chunk) = resp.body.next().await {
                body.extend(chunk.map_err(|e| LumaError::ExternalFailure {
                    message: e.to_string(),
                    retryable: true,
                })?);
            }
            Ok(HttpResponse {
                status: resp.status,
                headers: resp.headers,
                body,
            })
        }

        async fn send_streaming(
            &self,
            request: HttpRequest,
            cancel: CancellationToken,
        ) -> std::result::Result<HttpStreamResponse, TransportError> {
            lock(&self.requests).push(request);
            let Some(r) = lock(&self.queue).pop_front() else {
                return Err(TransportError::Other("no queued replay response".into()));
            };
            if let Some(e) = r.connect_error {
                return Err(e);
            }
            let st = ReplayState {
                body: r.body,
                pos: 0,
                sizes: r.chunking,
                size_idx: 0,
                fault: r.fault,
                cancel,
                ended: false,
            };
            Ok(HttpStreamResponse {
                status: r.status,
                headers: r.headers,
                body: futures::stream::unfold(st, replay_next).boxed(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fakes::*;
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn fake_clock_advances_only_when_told() {
        let c = FakeClock::at_default_start();
        let t0 = c.now_utc();
        let m0 = c.monotonic();
        assert_eq!(c.now_utc(), t0);
        c.advance(Duration::from_secs(90));
        assert_eq!((c.now_utc() - t0).num_seconds(), 90);
        assert_eq!(c.monotonic().duration_since(m0), Duration::from_secs(90));
        // wall clock jump does not move the monotonic clock
        c.set_utc(t0);
        assert_eq!(c.now_utc(), t0);
        assert_eq!(c.monotonic().duration_since(m0), Duration::from_secs(90));
    }

    #[tokio::test]
    async fn fake_clock_sleep_wakes_on_advance() {
        let c = Arc::new(FakeClock::at_default_start());
        let c2 = c.clone();
        let h = tokio::spawn(async move { c2.sleep(Duration::from_secs(10)).await });
        tokio::task::yield_now().await;
        assert!(!h.is_finished());
        c.advance(Duration::from_secs(5));
        tokio::task::yield_now().await;
        assert!(!h.is_finished());
        c.advance(Duration::from_secs(5));
        h.await.unwrap();
    }

    #[tokio::test]
    async fn other_fakes_behave() {
        let n = FakeNotifier::new();
        n.notify(&Notification {
            title: "t".into(),
            body: "b".into(),
            task_id: None,
        })
        .await
        .unwrap();
        assert_eq!(n.sent().len(), 1);

        let cs = InMemoryCredentialStore::new();
        assert_eq!(cs.get("k").unwrap(), None);
        cs.set("k", "v").unwrap();
        assert_eq!(cs.get("k").unwrap().as_deref(), Some("v"));
        cs.delete("k").unwrap();
        assert_eq!(cs.get("k").unwrap(), None);

        let a = FakeActivitySource::new();
        assert!(a.poll().await.unwrap().is_none());
        let s = ActivitySample {
            at: FakeClock::at_default_start().now_utc(),
            app_id: Some("x".into()),
            idle: false,
        };
        a.push(s.clone());
        assert_eq!(a.poll().await.unwrap(), Some(s));

        let h = FakeHttpTransport::new();
        assert!(h.send(HttpRequest::default()).await.is_err());
        h.push_response(Ok(HttpResponse {
            status: 200,
            ..Default::default()
        }));
        assert_eq!(h.send(HttpRequest::default()).await.unwrap().status, 200);
        assert_eq!(h.requests().len(), 2);
    }

    async fn drain(mut r: HttpStreamResponse) -> (Vec<Vec<u8>>, Option<TransportError>) {
        use futures::StreamExt;
        let mut chunks = Vec::new();
        while let Some(item) = r.body.next().await {
            match item {
                Ok(c) => chunks.push(c),
                Err(e) => return (chunks, Some(e)),
            }
        }
        (chunks, None)
    }

    #[test]
    fn debug_output_redacts_credentials() {
        let req = HttpRequest {
            method: "POST".into(),
            url: "https://example.test/v1".into(),
            headers: vec![
                ("Authorization".into(), "Bearer sk-secret-123".into()),
                ("X-Api-Key".into(), "sk-secret-123".into()),
                ("content-type".into(), "application/json".into()),
            ],
            body: Some(b"{}".to_vec()),
        };
        let dbg = format!("{req:?}");
        assert!(!dbg.contains("sk-secret-123"), "{dbg}");
        assert!(dbg.contains("application/json"));
        assert_eq!(req.header("authorization"), Some("Bearer sk-secret-123"));
    }

    #[tokio::test]
    async fn replay_transport_chunks_and_records() {
        let t = ReplayTransport::new();
        let body = "data: \u{4f60}\u{597d}\n\n";
        t.push(ReplayResponse::sse(body).chunked(1));
        let resp = t
            .send_streaming(HttpRequest::default(), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(resp.status, 200);
        let (chunks, err) = drain(resp).await;
        assert!(err.is_none());
        assert_eq!(chunks.len(), body.len());
        assert_eq!(chunks.concat(), body.as_bytes());
        assert_eq!(t.requests().len(), 1);

        // CRLF conversion, and sizes that repeat the last entry
        t.push(
            ReplayResponse::sse("a\nb\n")
                .with_line_ending(LineEnding::Crlf)
                .with_chunking(Chunking::Sizes(vec![1, 3])),
        );
        let resp = t
            .send_streaming(HttpRequest::default(), CancellationToken::new())
            .await
            .unwrap();
        let (chunks, _) = drain(resp).await;
        assert_eq!(chunks.concat(), b"a\r\nb\r\n");
        assert_eq!(chunks[0], b"a");
        assert_eq!(chunks[1], b"\r\nb");
    }

    #[tokio::test]
    async fn replay_transport_faults() {
        use futures::StreamExt;
        let t = ReplayTransport::new();
        t.push(
            ReplayResponse::sse("0123456789")
                .chunked(4)
                .disconnect_after(6),
        );
        let (chunks, err) = drain(
            t.send_streaming(HttpRequest::default(), CancellationToken::new())
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(chunks.concat(), b"012345");
        assert!(matches!(err, Some(TransportError::Disconnected(_))));

        t.push(ReplayResponse::sse("0123456789").with_fault(Fault::TruncateAfter(3)));
        let (chunks, err) = drain(
            t.send_streaming(HttpRequest::default(), CancellationToken::new())
                .await
                .unwrap(),
        )
        .await;
        assert_eq!((chunks.concat(), err), (b"012".to_vec(), None));

        // a stalled stream only ends when the caller cancels
        t.push(ReplayResponse::sse("0123").with_fault(Fault::StallAfter(2)));
        let cancel = CancellationToken::new();
        let mut resp = t
            .send_streaming(HttpRequest::default(), cancel.clone())
            .await
            .unwrap();
        assert_eq!(resp.body.next().await.unwrap().unwrap(), b"01");
        let h = tokio::spawn(async move { resp.body.next().await });
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        assert!(!h.is_finished());
        cancel.cancel();
        assert_eq!(
            h.await.unwrap().unwrap().unwrap_err(),
            TransportError::Cancelled
        );

        t.push(ReplayResponse::connect_failure(TransportError::Connect(
            "refused".into(),
        )));
        assert!(matches!(
            t.send_streaming(HttpRequest::default(), CancellationToken::new())
                .await,
            Err(TransportError::Connect(_))
        ));
    }

    #[tokio::test]
    async fn http_fixtures_are_parsed() {
        let text = "# comment\n# ---\nHTTP/1.1 429 Too Many Requests\nretry-after: 20\ncontent-type: application/json\n\n{\"a\":1}\n";
        let r = ReplayResponse::from_fixture(text);
        assert_eq!(r.status, 429);
        assert!(r.headers.contains(&("retry-after".into(), "20".into())));
        assert_eq!(r.body, b"{\"a\":1}\n");
        assert_eq!(
            ReplayResponse::from_fixture(": c\n\ndata: x\n\n").status,
            200
        );
        assert!(read_fixture("anthropic-messages/01-text.sse")
            .unwrap()
            .contains("message_start"));
        assert!(read_fixture("nope/missing").is_err());
    }
}
