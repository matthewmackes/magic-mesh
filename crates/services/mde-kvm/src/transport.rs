//! The cloud-hypervisor API transport — injectable so the [`crate::Vm`] lifecycle
//! is testable without a live VMM.
//!
//! cloud-hypervisor exposes its control API as **HTTP/1.1 over a unix-domain
//! socket** (`--api-socket`), base path `/api/v1`. That is simple enough that the
//! real transport is `std::os::unix::net::UnixStream` plus a hand-rolled request/
//! response — no hyper/reqwest/ureq (none of which speak UDS cleanly), keeping
//! this a §6 glue layer. The request builder and response parser are pure +
//! unit-tested; only [`UnixSocketTransport::request`] does I/O. Tests inject a
//! recording mock through the [`ChTransport`] trait.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use crate::KvmError;

/// A parsed cloud-hypervisor API response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChResponse {
    /// HTTP status code (cloud-hypervisor returns `204 No Content` for the
    /// bodyless lifecycle verbs and `200 OK` with a body for `vm.info`).
    pub status: u16,
    /// Response body (empty for a `204`).
    pub body: String,
}

impl ChResponse {
    /// A `2xx` response.
    #[must_use]
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// The cloud-hypervisor API transport.
///
/// The real impl is [`UnixSocketTransport`]; tests inject a recording mock.
/// `method` is `"PUT"`/`"GET"`, `path` is the full API path (e.g.
/// `"/api/v1/vm.boot"`), `body` is an optional JSON request body.
pub trait ChTransport {
    /// Issue one request and return the parsed response.
    ///
    /// # Errors
    /// Connect/I/O failures, or a malformed HTTP response.
    fn request(&self, method: &str, path: &str, body: Option<&str>)
        -> Result<ChResponse, KvmError>;
}

/// HTTP/1.1-over-UDS transport to a cloud-hypervisor api-socket.
#[derive(Debug, Clone)]
pub struct UnixSocketTransport {
    socket: PathBuf,
    timeout: Duration,
}

impl UnixSocketTransport {
    /// A transport dialing the cloud-hypervisor api-socket at `socket`, with a
    /// 30 s read/write timeout.
    #[must_use]
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
            timeout: Duration::from_secs(30),
        }
    }

    /// Override the read/write timeout (builder style).
    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The api-socket path this transport dials.
    #[must_use]
    pub fn socket(&self) -> &std::path::Path {
        &self.socket
    }
}

impl ChTransport for UnixSocketTransport {
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
    ) -> Result<ChResponse, KvmError> {
        let mut stream = UnixStream::connect(&self.socket)
            .map_err(|e| KvmError::Connect(self.socket.clone(), e))?;
        // Best-effort timeouts so a wedged VMM can't hang the broker forever.
        let _ = stream.set_read_timeout(Some(self.timeout));
        let _ = stream.set_write_timeout(Some(self.timeout));

        let req = build_http_request(method, path, body);
        stream.write_all(req.as_bytes())?;
        stream.flush()?;

        // `Connection: close` (set in the request) makes cloud-hypervisor close
        // the socket after the response, so reading to EOF yields the whole
        // message — headers + body — without needing to parse chunking.
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw)?;
        parse_http_response(&raw)
    }
}

/// Build a minimal HTTP/1.1 request for the cloud-hypervisor API. Pure + tested.
///
/// `Connection: close` lets the reader slurp to EOF; `Content-Length` frames the
/// (optional) JSON body. The `Host` is a placeholder — cloud-hypervisor routes on
/// the path, not the authority.
#[must_use]
pub fn build_http_request(method: &str, path: &str, body: Option<&str>) -> String {
    let body = body.unwrap_or("");
    let mut req = String::with_capacity(128 + body.len());
    req.push_str(method);
    req.push(' ');
    req.push_str(path);
    req.push_str(" HTTP/1.1\r\n");
    req.push_str("Host: localhost\r\n");
    req.push_str("Accept: application/json\r\n");
    if !body.is_empty() {
        req.push_str("Content-Type: application/json\r\n");
    }
    req.push_str("Content-Length: ");
    req.push_str(&body.len().to_string());
    req.push_str("\r\n");
    req.push_str("Connection: close\r\n");
    req.push_str("\r\n");
    req.push_str(body);
    req
}

/// Parse an HTTP/1.1 response into [`ChResponse`] (status + body). Pure + tested.
///
/// # Errors
/// An empty response or an unparseable status line.
pub fn parse_http_response(raw: &[u8]) -> Result<ChResponse, KvmError> {
    let text = String::from_utf8_lossy(raw);
    let (head, body) = text
        .split_once("\r\n\r\n")
        .map_or_else(|| (text.as_ref(), ""), |(h, b)| (h, b));

    let status_line = head
        .lines()
        .next()
        .ok_or_else(|| KvmError::Protocol("empty HTTP response".to_string()))?;
    // "HTTP/1.1 204 No Content" → the second whitespace-token is the code.
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse::<u16>().ok())
        .ok_or_else(|| KvmError::Protocol(format!("bad HTTP status line: {status_line}")))?;

    Ok(ChResponse {
        status,
        body: body.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_has_method_path_framing_and_body() {
        let body = r#"{"cpus":{"boot_vcpus":2}}"#;
        let req = build_http_request("PUT", "/api/v1/vm.create", Some(body));
        assert!(req.starts_with("PUT /api/v1/vm.create HTTP/1.1\r\n"));
        assert!(req.contains("Content-Type: application/json\r\n"));
        assert!(req.contains(&format!("Content-Length: {}\r\n", body.len())));
        assert!(req.contains("Connection: close\r\n"));
        // The body follows the blank-line header terminator verbatim.
        let (_, sent_body) = req.split_once("\r\n\r\n").expect("header/body split");
        assert_eq!(sent_body, body);
    }

    #[test]
    fn bodyless_request_sends_zero_length_and_no_content_type() {
        let req = build_http_request("PUT", "/api/v1/vm.boot", None);
        assert!(req.starts_with("PUT /api/v1/vm.boot HTTP/1.1\r\n"));
        assert!(req.contains("Content-Length: 0\r\n"));
        assert!(!req.contains("Content-Type"));
        assert!(req.ends_with("\r\n\r\n"));
    }

    #[test]
    fn parse_204_no_content() {
        let raw = b"HTTP/1.1 204 No Content\r\nServer: micro_http\r\n\r\n";
        let resp = parse_http_response(raw).expect("parse");
        assert_eq!(resp.status, 204);
        assert!(resp.body.is_empty());
        assert!(resp.is_success());
    }

    #[test]
    fn parse_200_with_json_body() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 18\r\n\r\n{\"state\":\"Running\"}";
        let resp = parse_http_response(raw).expect("parse");
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, r#"{"state":"Running"}"#);
        assert!(resp.is_success());
    }

    #[test]
    fn parse_500_error_is_not_success() {
        let raw = b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 5\r\n\r\nboom!";
        let resp = parse_http_response(raw).expect("parse");
        assert_eq!(resp.status, 500);
        assert_eq!(resp.body, "boom!");
        assert!(!resp.is_success());
    }

    #[test]
    fn parse_rejects_empty_and_garbage() {
        assert!(parse_http_response(b"").is_err());
        assert!(parse_http_response(b"not-an-http-response").is_err());
    }
}
