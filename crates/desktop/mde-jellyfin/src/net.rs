//! The injectable HTTP seam.
//!
//! Every Jellyfin endpoint in this crate is built as a pure [`HttpRequest`] and
//! executed through the [`HttpTransport`] trait. The real [`ReqwestTransport`]
//! is the one egress point; tests implement the same trait over recorded bytes,
//! so no request-builder or response-parser needs a live network to be tested.

/// The HTTP method of a Jellyfin request. Jellyfin's browse + auth surface only
/// needs `GET` (browse, Quick Connect poll) and `POST` (login, Quick Connect
/// exchange).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    /// A read request (browse, Quick Connect initiate/poll).
    Get,
    /// A write request with a JSON body (username/password + Quick Connect
    /// exchange).
    Post,
}

/// A fully-formed HTTP request — the pure output of every request builder.
///
/// The [`url`](Self::url) is complete (scheme, host, path, and encoded query),
/// the [`headers`](Self::headers) carry the Jellyfin `Authorization` line, and
/// [`body`](Self::body) holds the JSON bytes for a `POST`. Building this needs
/// no transport, so it is unit-testable directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    /// The request method.
    pub method: HttpMethod,
    /// The absolute request URL, query included and percent-encoded.
    pub url: String,
    /// Ordered `(name, value)` header pairs (e.g. the `Authorization` line).
    pub headers: Vec<(String, String)>,
    /// The request body (JSON bytes) for a `POST`, if any.
    pub body: Option<Vec<u8>>,
}

impl HttpRequest {
    /// A `GET` for `url` carrying `headers`.
    #[must_use]
    pub fn get(url: impl Into<String>, headers: Vec<(String, String)>) -> Self {
        Self {
            method: HttpMethod::Get,
            url: url.into(),
            headers,
            body: None,
        }
    }

    /// A `POST` for `url` carrying `headers` and a JSON `body`.
    #[must_use]
    pub fn post(url: impl Into<String>, headers: Vec<(String, String)>, body: Vec<u8>) -> Self {
        Self {
            method: HttpMethod::Post,
            url: url.into(),
            headers,
            body: Some(body),
        }
    }
}

/// The response a transport hands back: the HTTP status and the raw body bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    /// The HTTP status code.
    pub status: u16,
    /// The raw response body (parsed by the typed endpoint).
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// Whether the status is a 2xx success.
    #[must_use]
    pub const fn is_success(&self) -> bool {
        self.status >= 200 && self.status < 300
    }
}

/// A transport-level failure (connect / TLS / timeout / read). Distinct from a
/// non-2xx HTTP status, which the client surfaces as
/// [`JellyfinError::Http`](crate::JellyfinError::Http).
#[derive(Debug, thiserror::Error)]
#[error("jellyfin transport error: {0}")]
pub struct TransportError(pub String);

/// The one seam between the typed Jellyfin endpoints and the wire.
///
/// The real [`ReqwestTransport`] performs a blocking HTTPS round-trip; tests
/// implement this over recorded fixture bytes. Blocking (not async) to match
/// the immediate-mode `mde-media-egui` consumer and the `reqwest::blocking`
/// stack the media core already chose.
pub trait HttpTransport {
    /// Execute `request` and return the raw [`HttpResponse`].
    ///
    /// # Errors
    /// Returns [`TransportError`] on any connect / TLS / timeout / read failure
    /// (a non-2xx status is *not* an error here — it is returned in the
    /// [`HttpResponse::status`]).
    fn execute(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError>;
}

/// The real egress transport: a single `reqwest::blocking` client over rustls.
///
/// Airgap-safe to compile (rustls pulls no system OpenSSL); only exercising it
/// against a live Jellyfin server needs egress, which is out of scope for this
/// crate's fixture tests.
#[derive(Debug, Clone)]
pub struct ReqwestTransport {
    http: reqwest::blocking::Client,
}

impl ReqwestTransport {
    /// Build a transport with a default `reqwest::blocking` client.
    ///
    /// # Errors
    /// Returns [`TransportError`] if the underlying HTTP client cannot be built.
    pub fn new() -> Result<Self, TransportError> {
        let http = reqwest::blocking::Client::builder()
            .build()
            .map_err(|e| TransportError(e.to_string()))?;
        Ok(Self { http })
    }

    /// Wrap a pre-built `reqwest::blocking` client (so the caller can set
    /// timeouts / a proxy / a custom root store).
    #[must_use]
    pub const fn with_client(http: reqwest::blocking::Client) -> Self {
        Self { http }
    }
}

impl HttpTransport for ReqwestTransport {
    fn execute(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        let mut builder = match request.method {
            HttpMethod::Get => self.http.get(&request.url),
            HttpMethod::Post => self.http.post(&request.url),
        };
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        if let Some(body) = &request.body {
            builder = builder.body(body.clone());
        }
        let response = builder.send().map_err(|e| TransportError(e.to_string()))?;
        let status = response.status().as_u16();
        let body = response
            .bytes()
            .map_err(|e| TransportError(e.to_string()))?
            .to_vec();
        Ok(HttpResponse { status, body })
    }
}

/// Percent-encode one query-string component per RFC 3986: everything outside
/// the unreserved set (`A-Z a-z 0-9 - _ . ~`) becomes `%XX`.
///
/// Kept in-crate (a tiny, pure helper) so the request builders form complete,
/// testable URLs without pulling the `url` crate as a direct dependency; it is
/// exercised by the browse-request tests (search terms, ids).
#[must_use]
pub fn encode_query_component(value: &str) -> String {
    /// The uppercase hex digit for a nibble `0..=15`.
    const fn hex_digit(nibble: u8) -> char {
        (if nibble < 10 {
            b'0' + nibble
        } else {
            b'A' + nibble - 10
        }) as char
    }

    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        let unreserved = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~');
        if unreserved {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(hex_digit(byte >> 4));
            out.push(hex_digit(byte & 0x0f));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_request_get_has_no_body() {
        let req = HttpRequest::get("https://jelly.mesh/x", vec![]);
        assert_eq!(req.method, HttpMethod::Get);
        assert!(req.body.is_none());
    }

    #[test]
    fn http_request_post_carries_body() {
        let req = HttpRequest::post(
            "https://jelly.mesh/x",
            vec![("Content-Type".into(), "application/json".into())],
            b"{}".to_vec(),
        );
        assert_eq!(req.method, HttpMethod::Post);
        assert_eq!(req.body.as_deref(), Some(b"{}".as_slice()));
    }

    #[test]
    fn response_success_range() {
        assert!(HttpResponse {
            status: 200,
            body: vec![]
        }
        .is_success());
        assert!(HttpResponse {
            status: 204,
            body: vec![]
        }
        .is_success());
        assert!(!HttpResponse {
            status: 401,
            body: vec![]
        }
        .is_success());
        assert!(!HttpResponse {
            status: 500,
            body: vec![]
        }
        .is_success());
    }

    #[test]
    fn encode_leaves_unreserved_intact() {
        assert_eq!(encode_query_component("Abc-1_2.3~x"), "Abc-1_2.3~x");
    }

    #[test]
    fn encode_escapes_reserved_and_space() {
        // Space, ampersand, equals, and a multibyte char all encode.
        assert_eq!(encode_query_component("a b&c=d"), "a%20b%26c%3Dd");
        assert_eq!(encode_query_component("é"), "%C3%A9");
    }
}
