//! Outbound HTTP POST to the local ntfy broker.
//!
//! The broker spawned by [`crate::broker`] listens on
//! `<overlay_ip>:8443`. The publisher receives a
//! [`RenderedPublish`] from the matcher, formats it into the
//! ntfy HTTP API contract (POST `/`<topic> with `X-Title` +
//! `X-Priority` headers + raw body), and ships it.
//!
//! Plain HTTP per `docs/design/v6.x-mackes-bus.md` line 210
//! ("Encryption: None — Nebula transport is enough"). The
//! `reqwest` dep is built without any TLS feature so we don't
//! drag rustls or native-tls into the workspace.

use thiserror::Error;

use super::matcher::RenderedPublish;

/// Errors talking to the local ntfy broker.
#[derive(Debug, Error)]
pub enum PublisherError {
    /// `reqwest` transport error (connection refused, DNS, etc.).
    /// Most commonly seen when the broker hasn't spawned yet
    /// (pre-enrollment) or is restarting under supervision.
    #[error("transport: {0}")]
    Transport(String),
    /// ntfy responded with a non-2xx status.
    #[error("ntfy returned {status} for topic {topic}: {body}")]
    BadStatus {
        /// HTTP status code as reported by ntfy.
        status: u16,
        /// Topic we were publishing to (helps log triage).
        topic: String,
        /// Response body (truncated to 1 KiB).
        body: String,
    },
}

/// POST the rendered publish to `http://<broker_host>:<broker_port>/<topic>`.
///
/// `broker_base` is typically `format!("http://{overlay_ip}:8443")`.
/// We accept it as a base so tests can point at a stub server on
/// `127.0.0.1`.
///
/// # Errors
/// See [`PublisherError`].
pub async fn publish_to_ntfy(
    client: &reqwest::Client,
    broker_base: &str,
    rendered: &RenderedPublish,
) -> Result<(), PublisherError> {
    let url = format!("{}/{}", broker_base.trim_end_matches('/'), rendered.topic);
    let resp = client
        .post(&url)
        .header("X-Title", &rendered.title)
        .header("X-Priority", rendered.priority.ntfy_header())
        .body(rendered.body.clone())
        .send()
        .await
        .map_err(|e| PublisherError::Transport(e.to_string()))?;
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        let body = resp
            .text()
            .await
            .unwrap_or_default()
            .chars()
            .take(1024)
            .collect::<String>();
        return Err(PublisherError::BadStatus {
            status,
            topic: rendered.topic.clone(),
            body,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::config::Priority;
    use super::*;
    use std::sync::Arc;
    use std::sync::Mutex;

    /// Spin up a tokio TCP server that emits HTTP/1.1 200 OK on
    /// every connection, captures the raw request, and lets the
    /// test read it back. Returns `(handle, addr, captured)`.
    async fn stub_server(
        respond_with: Vec<u8>,
    ) -> (
        tokio::task::JoinHandle<()>,
        std::net::SocketAddr,
        Arc<Mutex<Vec<u8>>>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let captured = Arc::new(Mutex::new(Vec::<u8>::new()));
        let captured_clone = captured.clone();
        let handle = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            use tokio::io::AsyncWriteExt;
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                // Read once — short request fits comfortably.
                if let Ok(n) = stream.read(&mut buf).await {
                    captured_clone.lock().unwrap().extend_from_slice(&buf[..n]);
                }
                stream.write_all(&respond_with).await.ok();
                stream.shutdown().await.ok();
            }
        });
        (handle, addr, captured)
    }

    #[tokio::test]
    async fn publish_sends_expected_headers_and_body() {
        let response = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec();
        let (_handle, addr, captured) = stub_server(response).await;
        let client = reqwest::Client::new();
        let base = format!("http://{addr}");
        let rendered = RenderedPublish {
            rule_name: "test".to_string(),
            topic: "gh/push".to_string(),
            priority: Priority::High,
            title: "hello world".to_string(),
            body: "payload body".to_string(),
            quiet_hours: crate::dnd::TopicQuietHours::default(),
        };
        publish_to_ntfy(&client, &base, &rendered)
            .await
            .expect("publish ok");
        let req = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
        assert!(req.contains("POST /gh/push"));
        assert!(req.contains("x-title: hello world") || req.contains("X-Title: hello world"));
        assert!(
            req.contains("x-priority: 4") || req.contains("X-Priority: 4"),
            "expected High → 4 header in request, got:\n{req}"
        );
        assert!(req.ends_with("payload body") || req.contains("\r\n\r\npayload body"));
    }

    #[tokio::test]
    async fn non_2xx_status_propagates() {
        let response =
            b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 12\r\n\r\nbroker down!".to_vec();
        let (_handle, addr, _captured) = stub_server(response).await;
        let client = reqwest::Client::new();
        let base = format!("http://{addr}");
        let rendered = RenderedPublish {
            rule_name: "t".to_string(),
            topic: "x/y".to_string(),
            priority: Priority::Default,
            title: "t".to_string(),
            body: "b".to_string(),
            quiet_hours: crate::dnd::TopicQuietHours::default(),
        };
        let err = publish_to_ntfy(&client, &base, &rendered)
            .await
            .expect_err("503 should propagate");
        match err {
            PublisherError::BadStatus { status, topic, .. } => {
                assert_eq!(status, 503);
                assert_eq!(topic, "x/y");
            }
            PublisherError::Transport(_) => panic!("expected BadStatus, got Transport"),
        }
    }

    #[tokio::test]
    async fn transport_error_on_unreachable_broker() {
        let client = reqwest::Client::new();
        // 127.0.0.1:1 — TCP/1 is reserved + not listening anywhere.
        let rendered = RenderedPublish {
            rule_name: "t".to_string(),
            topic: "x/y".to_string(),
            priority: Priority::Default,
            title: "t".to_string(),
            body: "b".to_string(),
            quiet_hours: crate::dnd::TopicQuietHours::default(),
        };
        let err = publish_to_ntfy(&client, "http://127.0.0.1:1", &rendered)
            .await
            .expect_err("connection refused expected");
        assert!(matches!(err, PublisherError::Transport(_)));
    }
}
