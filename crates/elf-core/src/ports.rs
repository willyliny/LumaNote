//! Ports: traits through which the core reaches the outside world, plus fakes.
//!
//! The Tauri shell supplies real adapters; tests use [`fakes`].

use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

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

/// Minimal HTTP request.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
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

/// Network transport used by model and search adapters (buffered; streaming comes later).
#[async_trait]
pub trait HttpTransport: Send + Sync {
    /// Performs one request.
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse>;
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

    use super::*;
    use crate::schema::ElfError;

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
                Err(ElfError::ExternalFailure {
                    message: "no queued fake response".into(),
                    retryable: false,
                })
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
}
