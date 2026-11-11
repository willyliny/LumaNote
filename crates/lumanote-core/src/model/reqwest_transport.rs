//! The real [`HttpTransport`]: reqwest over rustls (ring crypto provider, platform
//! certificate verifier, no OpenSSL).
//!
//! Policy (handbook 7.3):
//! * redirects are **never followed** (a redirect to another host must not carry the
//!   API key; the 3xx response is returned to the caller, which reports it);
//! * `https` is required, except `http` to loopback hosts when
//!   [`ReqwestTransportConfig::allow_loopback_http`] is set;
//! * connect timeout and per-read idle timeout;
//! * no system / environment proxy is used.

use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::ports::{
    is_sensitive_header, HttpRequest, HttpResponse, HttpStreamResponse, HttpTransport,
    TransportError,
};
use crate::schema::{LumaError, Result};

/// Settings of [`ReqwestTransport`].
#[derive(Debug, Clone)]
pub struct ReqwestTransportConfig {
    /// Time allowed to establish the connection (TCP and TLS).
    pub connect_timeout: Duration,
    /// Longest silence between two pieces of the response before the read fails.
    pub read_timeout: Duration,
    /// Allow plain `http` to loopback hosts (local model servers).
    pub allow_loopback_http: bool,
    /// Largest body `send` (the buffered call) accepts.
    pub max_buffered_bytes: usize,
    /// `User-Agent` header.
    pub user_agent: String,
}

impl Default for ReqwestTransportConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            read_timeout: Duration::from_secs(120),
            allow_loopback_http: false,
            max_buffered_bytes: 8 * 1024 * 1024,
            user_agent: concat!("lumanote-core/", env!("CARGO_PKG_VERSION")).to_owned(),
        }
    }
}

/// HTTP transport backed by reqwest.
#[derive(Debug, Clone)]
pub struct ReqwestTransport {
    client: reqwest::Client,
    config: ReqwestTransportConfig,
}

impl ReqwestTransport {
    /// Builds the client. Installs the ring crypto provider as the process default if no
    /// provider is installed yet.
    pub fn new(config: ReqwestTransportConfig) -> Result<Self> {
        // `Err` only means a provider was installed already, which is fine.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(config.connect_timeout)
            .read_timeout(config.read_timeout)
            .user_agent(config.user_agent.clone())
            .no_proxy()
            .build()
            .map_err(|e| LumaError::ExternalFailure {
                message: format!("cannot build the HTTP client: {}", e.without_url()),
                retryable: false,
            })?;
        Ok(Self { client, config })
    }

    fn check_url(&self, raw: &str) -> std::result::Result<Url, TransportError> {
        let url =
            Url::parse(raw).map_err(|e| TransportError::Blocked(format!("invalid URL: {e}")))?;
        match url.scheme() {
            "https" => Ok(url),
            "http" => {
                let loopback = match url.host() {
                    Some(url::Host::Domain(d)) => {
                        let d = d.trim_end_matches('.').to_ascii_lowercase();
                        d == "localhost" || d.ends_with(".localhost")
                    }
                    Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
                    Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
                    None => false,
                };
                if loopback && self.config.allow_loopback_http {
                    Ok(url)
                } else {
                    Err(TransportError::Blocked(
                        "plain http is only allowed to configured loopback hosts".into(),
                    ))
                }
            }
            other => Err(TransportError::Blocked(format!(
                "scheme `{other}` is not allowed"
            ))),
        }
    }
}

fn map_error(e: reqwest::Error) -> TransportError {
    let e = e.without_url();
    if e.is_timeout() {
        TransportError::Timeout(e.to_string())
    } else if e.is_connect() {
        TransportError::Connect(e.to_string())
    } else if e.is_body() || e.is_decode() {
        TransportError::Disconnected(e.to_string())
    } else if e.is_builder() || e.is_redirect() {
        TransportError::Blocked(e.to_string())
    } else {
        TransportError::Other(e.to_string())
    }
}

struct BodyState {
    resp: reqwest::Response,
    cancel: CancellationToken,
    done: bool,
}

fn body_stream(
    resp: reqwest::Response,
    cancel: CancellationToken,
) -> BoxStream<'static, std::result::Result<Vec<u8>, TransportError>> {
    futures::stream::unfold(
        BodyState {
            resp,
            cancel,
            done: false,
        },
        |mut st| async move {
            if st.done {
                return None;
            }
            let next = tokio::select! {
                biased;
                () = st.cancel.cancelled() => {
                    st.done = true;
                    return Some((Err(TransportError::Cancelled), st));
                }
                r = st.resp.chunk() => r,
            };
            match next {
                Ok(Some(bytes)) => Some((Ok(bytes.to_vec()), st)),
                Ok(None) => None,
                Err(e) => {
                    st.done = true;
                    Some((Err(map_error(e)), st))
                }
            }
        },
    )
    .boxed()
}

#[async_trait]
impl HttpTransport for ReqwestTransport {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse> {
        let fail = |e: TransportError| LumaError::ExternalFailure {
            retryable: !matches!(e, TransportError::Blocked(_) | TransportError::Cancelled),
            message: e.to_string(),
        };
        let mut resp = self
            .send_streaming(request, CancellationToken::new())
            .await
            .map_err(fail)?;
        let mut body = Vec::new();
        while let Some(chunk) = resp.body.next().await {
            body.extend(chunk.map_err(fail)?);
            if body.len() > self.config.max_buffered_bytes {
                return Err(LumaError::ExternalFailure {
                    message: "response body exceeds the buffering limit".into(),
                    retryable: false,
                });
            }
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
        let url = self.check_url(&request.url)?;
        let method = reqwest::Method::from_bytes(request.method.as_bytes())
            .map_err(|_| TransportError::Blocked(format!("invalid method `{}`", request.method)))?;
        let mut builder = self.client.request(method, url);
        for (name, value) in &request.headers {
            let n = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| TransportError::Blocked(format!("invalid header name `{name}`")))?;
            let mut v = reqwest::header::HeaderValue::from_str(value).map_err(|_| {
                TransportError::Blocked(format!("invalid value for header `{name}`"))
            })?;
            v.set_sensitive(is_sensitive_header(name));
            builder = builder.header(n, v);
        }
        if let Some(body) = request.body {
            builder = builder.body(body);
        }
        let resp = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(TransportError::Cancelled),
            r = builder.send() => r.map_err(map_error)?,
        };
        let status = resp.status().as_u16();
        let headers = resp
            .headers()
            .iter()
            .filter_map(|(k, v)| Some((k.as_str().to_owned(), v.to_str().ok()?.to_owned())))
            .collect();
        Ok(HttpStreamResponse {
            status,
            headers,
            body: body_stream(resp, cancel),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn transport(allow: bool, read_timeout: Duration) -> ReqwestTransport {
        ReqwestTransport::new(ReqwestTransportConfig {
            allow_loopback_http: allow,
            read_timeout,
            connect_timeout: Duration::from_secs(2),
            ..Default::default()
        })
        .unwrap()
    }

    fn req(url: &str) -> HttpRequest {
        HttpRequest {
            method: "POST".into(),
            url: url.into(),
            headers: vec![
                ("authorization".into(), "Bearer sk-test-secret".into()),
                ("content-type".into(), "application/json".into()),
            ],
            body: Some(b"{\"a\":1}".to_vec()),
        }
    }

    /// Reads one request (headers and body) and returns the raw bytes.
    async fn read_request(sock: &mut tokio::net::TcpStream) -> String {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 1024];
        loop {
            let n = sock.read(&mut tmp).await.unwrap();
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n")
                && String::from_utf8_lossy(&buf).contains("{\"a\":1}")
            {
                break;
            }
        }
        String::from_utf8_lossy(&buf).into_owned()
    }

    #[tokio::test]
    async fn streams_a_chunked_body_and_sends_headers() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let request = read_request(&mut sock).await;
            sock.write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nx-request-id: req_1\r\ntransfer-encoding: chunked\r\n\r\n",
            )
            .await
            .unwrap();
            for piece in ["data: one\n\n", "data: two\n\n"] {
                sock.write_all(format!("{:x}\r\n{piece}\r\n", piece.len()).as_bytes())
                    .await
                    .unwrap();
                sock.flush().await.unwrap();
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
            sock.write_all(b"0\r\n\r\n").await.unwrap();
            request
        });
        let t = transport(true, Duration::from_secs(5));
        let mut resp = t
            .send_streaming(
                req(&format!("http://{addr}/v1/x")),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.header("x-request-id"), Some("req_1"));
        let mut all = Vec::new();
        while let Some(c) = resp.body.next().await {
            all.extend(c.unwrap());
        }
        assert_eq!(
            String::from_utf8(all).unwrap(),
            "data: one\n\ndata: two\n\n"
        );
        let seen = server.await.unwrap().to_ascii_lowercase();
        assert!(
            seen.contains("authorization: bearer sk-test-secret"),
            "{seen}"
        );
        assert!(seen.contains("post /v1/x"));
    }

    #[tokio::test]
    async fn redirects_are_not_followed_and_credentials_stay_put() {
        let other = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let other_addr = other.local_addr().unwrap();
        let first = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let first_addr = first.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut sock, _) = first.accept().await.unwrap();
            let _ = read_request(&mut sock).await;
            let resp = format!(
                "HTTP/1.1 307 Temporary Redirect\r\nlocation: http://{other_addr}/steal\r\ncontent-length: 0\r\n\r\n"
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
        });
        let t = transport(true, Duration::from_secs(5));
        let resp = t
            .send_streaming(
                req(&format!("http://{first_addr}/v1/x")),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status, 307);
        server.await.unwrap();
        // nothing ever connected to the redirect target
        let accepted = tokio::time::timeout(Duration::from_millis(200), other.accept()).await;
        assert!(accepted.is_err(), "the redirect target was contacted");
    }

    #[tokio::test]
    async fn scheme_and_host_policy() {
        let strict = transport(false, Duration::from_secs(1));
        for url in [
            "http://example.com/x",
            "http://localhost:1/x",
            "ftp://example.com/x",
            "not a url",
        ] {
            let e = strict
                .send_streaming(req(url), CancellationToken::new())
                .await
                .unwrap_err();
            assert!(matches!(e, TransportError::Blocked(_)), "{url}: {e:?}");
        }
        let loopback = transport(true, Duration::from_secs(1));
        let e = loopback
            .send_streaming(req("http://192.168.1.10/x"), CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(e, TransportError::Blocked(_)));
        // a closed loopback port is a connect failure, not a policy block
        let e = loopback
            .send_streaming(req("http://127.0.0.1:1/x"), CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(e, TransportError::Connect(_)), "{e:?}");
        // header injection is refused before anything is sent
        let mut bad = req("http://127.0.0.1:1/x");
        bad.headers.push(("x-evil".into(), "a\r\nb: c".into()));
        assert!(matches!(
            loopback.send_streaming(bad, CancellationToken::new()).await,
            Err(TransportError::Blocked(_))
        ));
    }

    #[tokio::test]
    async fn read_timeout_and_cancellation() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut sock, _) = listener.accept().await.unwrap();
                let _ = read_request(&mut sock).await;
                sock.write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n9\r\ndata: x\n\n\r\n",
                )
                .await
                .unwrap();
                // then stall; the client must hang up
                let mut tmp = [0u8; 16];
                let _ = tokio::time::timeout(Duration::from_secs(5), sock.read(&mut tmp)).await;
            }
        });
        let t = transport(true, Duration::from_millis(300));
        let mut resp = t
            .send_streaming(req(&format!("http://{addr}/")), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(resp.body.next().await.unwrap().unwrap(), b"data: x\n\n");
        let e = resp.body.next().await.unwrap().unwrap_err();
        assert!(
            matches!(
                e,
                TransportError::Timeout(_) | TransportError::Disconnected(_)
            ),
            "{e:?}"
        );

        let t = transport(true, Duration::from_secs(30));
        let cancel = CancellationToken::new();
        let mut resp = t
            .send_streaming(req(&format!("http://{addr}/")), cancel.clone())
            .await
            .unwrap();
        assert!(resp.body.next().await.unwrap().is_ok());
        cancel.cancel();
        assert_eq!(
            resp.body.next().await.unwrap().unwrap_err(),
            TransportError::Cancelled
        );
        drop(resp);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn buffered_send_returns_status_headers_body() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let _ = read_request(&mut sock).await;
            sock.write_all(
                b"HTTP/1.1 429 Too Many Requests\r\nretry-after: 7\r\ncontent-length: 2\r\n\r\n{}",
            )
            .await
            .unwrap();
        });
        let r = transport(true, Duration::from_secs(5))
            .send(req(&format!("http://{addr}/")))
            .await
            .unwrap();
        assert_eq!(r.status, 429);
        assert_eq!(r.body, b"{}");
        assert!(r
            .headers
            .iter()
            .any(|(k, v)| k == "retry-after" && v == "7"));
    }
}
