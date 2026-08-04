//! Bounded HTTP/1.1 transport for the dedicated guest controller.
//!
//! The service deliberately avoids ambient proxies, DNS, redirects, chunked
//! bodies, persistent connections, and content negotiation. Both ends are under
//! MCNF control, so a small strict protocol is safer than a general web client.

use crate::auth::{request_signature, response_signature, verify_response_signature};
use crate::{hex_encode, random_bytes, unix_seconds};
use anyhow::{bail, ensure, Context, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::Duration;
use zeroize::Zeroize;

pub const MAX_HEADER_BYTES: usize = 16 * 1024;
pub const MAX_JSON_BODY_BYTES: usize = 64 * 1024;
pub const MAX_WAV_BODY_BYTES: usize = 2 * 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(4);

#[derive(Clone, Debug)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl HttpRequest {
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }
}

#[derive(Clone, Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub content_type: &'static str,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    #[must_use]
    pub fn bytes(status: u16, content_type: &'static str, body: Vec<u8>) -> Self {
        Self {
            status,
            content_type,
            headers: BTreeMap::new(),
            body,
        }
    }

    pub fn json<T: Serialize>(status: u16, value: &T) -> Result<Self> {
        let body = serde_json::to_vec(value).context("encode JSON response")?;
        Ok(Self::bytes(status, "application/json", body))
    }

    #[must_use]
    pub fn error(status: u16, code: &str) -> Self {
        let safe = code
            .bytes()
            .filter(|value| value.is_ascii_alphanumeric() || *value == b'-' || *value == b'_')
            .map(char::from)
            .collect::<String>();
        Self::bytes(
            status,
            "application/json",
            format!("{{\"error\":\"{safe}\"}}").into_bytes(),
        )
    }

    pub fn add_header(&mut self, name: &str, value: String) -> Result<()> {
        validate_header_name(name)?;
        validate_header_value(&value)?;
        ensure!(
            !is_reserved_response_header(name),
            "attempted to replace reserved response header"
        );
        self.headers.insert(name.to_owned(), value);
        Ok(())
    }
}

#[derive(Debug)]
pub struct ClientResponse {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl ClientResponse {
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }
}

pub fn read_request(stream: &mut TcpStream, maximum_body: usize) -> Result<HttpRequest> {
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .context("set controller read timeout")?;
    let raw = read_message(stream, maximum_body)?;
    parse_request(&raw, maximum_body)
}

pub fn write_response(stream: &mut TcpStream, response: &HttpResponse) -> Result<()> {
    stream
        .set_write_timeout(Some(IO_TIMEOUT))
        .context("set controller write timeout")?;
    ensure!(
        response.body.len() <= MAX_WAV_BODY_BYTES,
        "response exceeds bounded body limit"
    );
    let reason = reason_phrase(response.status)?;
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nContent-Type: {}\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\n",
        response.status,
        reason,
        response.body.len(),
        response.content_type
    );
    for (name, value) in &response.headers {
        validate_header_name(name)?;
        validate_header_value(value)?;
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    stream
        .write_all(head.as_bytes())
        .context("write HTTP response headers")?;
    stream
        .write_all(&response.body)
        .context("write HTTP response body")?;
    stream.flush().context("flush HTTP response")
}

fn read_message(stream: &mut TcpStream, maximum_body: usize) -> Result<Vec<u8>> {
    let maximum = MAX_HEADER_BYTES
        .checked_add(maximum_body)
        .context("HTTP size bound overflow")?;
    let mut raw = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 4096];
    let mut expected_total = None;
    loop {
        let count = stream.read(&mut chunk).context("read HTTP message")?;
        if count == 0 {
            break;
        }
        ensure!(raw.len() + count <= maximum, "HTTP message is too large");
        raw.extend_from_slice(&chunk[..count]);
        if expected_total.is_none() {
            if let Some(header_end) = find_header_end(&raw) {
                ensure!(header_end <= MAX_HEADER_BYTES, "HTTP headers are too large");
                let headers = parse_header_lines(&raw[..header_end])?;
                let content_length = content_length(&headers)?;
                ensure!(content_length <= maximum_body, "HTTP body is too large");
                expected_total = Some(
                    header_end
                        .checked_add(4)
                        .and_then(|value| value.checked_add(content_length))
                        .context("HTTP message length overflow")?,
                );
            } else {
                ensure!(raw.len() <= MAX_HEADER_BYTES, "HTTP headers are too large");
            }
        }
        if expected_total.is_some_and(|length| raw.len() >= length) {
            break;
        }
    }
    let expected = expected_total.context("HTTP message omitted a complete header")?;
    ensure!(
        raw.len() == expected,
        "HTTP message length does not match Content-Length"
    );
    Ok(raw)
}

fn parse_request(raw: &[u8], maximum_body: usize) -> Result<HttpRequest> {
    let header_end = find_header_end(raw).context("request header terminator is missing")?;
    let header_text =
        std::str::from_utf8(&raw[..header_end]).context("request headers not UTF-8")?;
    let mut lines = header_text.split("\r\n");
    let request_line = lines.next().context("request line is missing")?;
    let fields = request_line.split(' ').collect::<Vec<_>>();
    ensure!(fields.len() == 3, "malformed request line");
    let method = fields[0];
    let path = fields[1];
    ensure!(
        matches!(method, "GET" | "POST" | "DELETE"),
        "unsupported HTTP method"
    );
    validate_path(path)?;
    ensure!(fields[2] == "HTTP/1.1", "controller requires HTTP/1.1");
    let headers = parse_headers(lines)?;
    ensure!(
        !headers.contains_key("transfer-encoding"),
        "chunked/encoded request bodies are forbidden"
    );
    let length = content_length(&headers)?;
    ensure!(length <= maximum_body, "request body is too large");
    let body = &raw[header_end + 4..];
    ensure!(body.len() == length, "request body length mismatch");
    Ok(HttpRequest {
        method: method.to_owned(),
        path: path.to_owned(),
        headers,
        body: body.to_vec(),
    })
}

fn parse_response(raw: &[u8], maximum_body: usize) -> Result<ClientResponse> {
    let header_end = find_header_end(raw).context("response header terminator is missing")?;
    let header_text =
        std::str::from_utf8(&raw[..header_end]).context("response headers not UTF-8")?;
    let mut lines = header_text.split("\r\n");
    let status_line = lines.next().context("status line is missing")?;
    let mut fields = status_line.splitn(3, ' ');
    ensure!(
        fields.next() == Some("HTTP/1.1"),
        "controller returned non-HTTP/1.1 response"
    );
    let status = fields
        .next()
        .context("response status is missing")?
        .parse::<u16>()
        .context("response status is invalid")?;
    ensure!(
        (100..=599).contains(&status),
        "response status is outside HTTP range"
    );
    let headers = parse_headers(lines)?;
    ensure!(
        !headers.contains_key("transfer-encoding"),
        "chunked/encoded response bodies are forbidden"
    );
    let length = content_length(&headers)?;
    ensure!(length <= maximum_body, "response body is too large");
    let body = &raw[header_end + 4..];
    ensure!(body.len() == length, "response body length mismatch");
    Ok(ClientResponse {
        status,
        headers,
        body: body.to_vec(),
    })
}

fn parse_header_lines(raw: &[u8]) -> Result<BTreeMap<String, String>> {
    let text = std::str::from_utf8(raw).context("HTTP headers not UTF-8")?;
    let mut lines = text.split("\r\n");
    let _start_line = lines.next().context("HTTP start line is missing")?;
    parse_headers(lines)
}

fn parse_headers<'a>(lines: impl Iterator<Item = &'a str>) -> Result<BTreeMap<String, String>> {
    let mut headers = BTreeMap::new();
    for line in lines {
        ensure!(!line.is_empty(), "unexpected empty header line");
        ensure!(
            !line.starts_with([' ', '\t']),
            "folded HTTP headers are forbidden"
        );
        let (name, value) = line.split_once(':').context("malformed HTTP header")?;
        validate_header_name(name)?;
        let value = value.trim();
        validate_header_value(value)?;
        let key = name.to_ascii_lowercase();
        ensure!(!headers.contains_key(&key), "duplicate HTTP header");
        headers.insert(key, value.to_owned());
    }
    Ok(headers)
}

fn content_length(headers: &BTreeMap<String, String>) -> Result<usize> {
    match headers.get("content-length") {
        Some(value) => value
            .parse::<usize>()
            .context("invalid Content-Length header"),
        None => Ok(0),
    }
}

fn validate_path(path: &str) -> Result<()> {
    ensure!(
        path.starts_with('/')
            && path.len() <= 1024
            && path.is_ascii()
            && !path
                .bytes()
                .any(|value| value.is_ascii_control() || value == b' ')
            && !path.contains('?')
            && !path.contains('#'),
        "invalid origin-form request path"
    );
    Ok(())
}

fn validate_header_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= 128
            && name
                .bytes()
                .all(|value| { value.is_ascii_alphanumeric() || matches!(value, b'-' | b'_') }),
        "invalid HTTP header name"
    );
    Ok(())
}

fn validate_header_value(value: &str) -> Result<()> {
    ensure!(
        value.len() <= 4096
            && value.is_ascii()
            && !value.bytes().any(|byte| matches!(byte, b'\r' | b'\n' | 0)),
        "invalid HTTP header value"
    );
    Ok(())
}

fn is_reserved_response_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "content-length" | "content-type" | "connection" | "x-content-type-options"
    )
}

fn find_header_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4).position(|window| window == b"\r\n\r\n")
}

fn reason_phrase(status: u16) -> Result<&'static str> {
    match status {
        200 => Ok("OK"),
        201 => Ok("Created"),
        204 => Ok("No Content"),
        400 => Ok("Bad Request"),
        401 => Ok("Unauthorized"),
        403 => Ok("Forbidden"),
        404 => Ok("Not Found"),
        409 => Ok("Conflict"),
        413 => Ok("Payload Too Large"),
        422 => Ok("Unprocessable Content"),
        429 => Ok("Too Many Requests"),
        500 => Ok("Internal Server Error"),
        _ => bail!("unsupported response status {status}"),
    }
}

/// Authenticated, proxy-free client for one exact guest controller address.
pub struct ControllerClient {
    address: SocketAddr,
    secret: [u8; 32],
}

impl ControllerClient {
    #[must_use]
    pub const fn new(host: IpAddr, port: u16, secret: [u8; 32]) -> Self {
        Self {
            address: SocketAddr::new(host, port),
            secret,
        }
    }

    pub fn json<T: Serialize>(
        &self,
        method: &str,
        path: &str,
        value: &T,
    ) -> Result<ClientResponse> {
        let body = serde_json::to_vec(value).context("encode controller request")?;
        self.request(method, path, "application/json", &body, MAX_JSON_BODY_BYTES)
    }

    pub fn empty(&self, method: &str, path: &str, maximum: usize) -> Result<ClientResponse> {
        self.request(method, path, "application/octet-stream", &[], maximum)
    }

    fn request(
        &self,
        method: &str,
        path: &str,
        content_type: &str,
        body: &[u8],
        maximum_response: usize,
    ) -> Result<ClientResponse> {
        ensure!(
            matches!(method, "GET" | "POST" | "DELETE"),
            "invalid client method"
        );
        validate_path(path)?;
        validate_header_value(content_type)?;
        ensure!(
            body.len() <= MAX_JSON_BODY_BYTES,
            "controller request body is too large"
        );
        let nonce = hex_encode(&random_bytes::<32>()?);
        let timestamp = unix_seconds()?;
        let signature = request_signature(&self.secret, method, path, timestamp, &nonce, body)?;
        let mut stream = TcpStream::connect_timeout(&self.address, IO_TIMEOUT)
            .with_context(|| format!("connect Browser controller at {}", self.address))?;
        stream
            .set_write_timeout(Some(IO_TIMEOUT))
            .context("set controller client write timeout")?;
        stream
            .set_read_timeout(Some(IO_TIMEOUT))
            .context("set controller client read timeout")?;
        let host = match self.address.ip() {
            IpAddr::V4(value) => format!("{value}:{}", self.address.port()),
            IpAddr::V6(value) => format!("[{value}]:{}", self.address.port()),
        };
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\nContent-Type: {content_type}\r\nConnection: close\r\nX-MCNF-Time: {timestamp}\r\nX-MCNF-Nonce: {nonce}\r\nX-MCNF-Signature: {signature}\r\n\r\n",
            body.len()
        );
        stream
            .write_all(head.as_bytes())
            .context("write controller request headers")?;
        stream
            .write_all(body)
            .context("write controller request body")?;
        stream.flush().context("flush controller request")?;

        let raw = read_message(&mut stream, maximum_response)?;
        let response = parse_response(&raw, maximum_response)?;
        let response_signature = response
            .header("x-mcnf-response-signature")
            .context("controller response omitted authentication")?;
        verify_response_signature(
            &self.secret,
            &nonce,
            response.status,
            &response.body,
            response_signature,
        )?;
        Ok(response)
    }
}

impl Drop for ControllerClient {
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}

pub fn authenticate_response(
    response: &mut HttpResponse,
    secret: &[u8; 32],
    request_nonce: &str,
) -> Result<()> {
    let signature = response_signature(secret, request_nonce, response.status, &response.body)?;
    response.add_header("X-MCNF-Response-Signature", signature)
}

#[cfg(test)]
mod tests {
    use super::{parse_request, parse_response, HttpResponse};

    #[test]
    fn strict_request_parser_accepts_one_bounded_body() {
        let raw = b"POST /v1/jobs HTTP/1.1\r\nHost: 192.0.2.2\r\nContent-Length: 2\r\n\r\n{}";
        let request = parse_request(raw, 10).ok();
        assert_eq!(
            request.as_ref().map(|value| value.method.as_str()),
            Some("POST")
        );
        assert_eq!(
            request.as_ref().map(|value| value.body.as_slice()),
            Some(&b"{}"[..])
        );
    }

    #[test]
    fn parser_rejects_chunked_and_duplicate_headers() {
        let chunked = b"POST /v1/jobs HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n";
        assert!(parse_request(chunked, 10).is_err());
        let duplicate = b"GET /v1/jobs/a HTTP/1.1\r\nHost: a\r\nHost: b\r\n\r\n";
        assert!(parse_request(duplicate, 10).is_err());
    }

    #[test]
    fn strict_response_parser_obeys_content_length() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nX-MCNF-Response-Signature: deadbeef\r\n\r\n{}";
        let response = parse_response(raw, 10).ok();
        assert_eq!(response.as_ref().map(|value| value.status), Some(200));
        assert_eq!(
            response.as_ref().map(|value| value.body.as_slice()),
            Some(&b"{}"[..])
        );
    }

    #[test]
    fn response_error_codes_are_json_safe() {
        let response = HttpResponse::error(400, "bad\r\nInjected: x");
        assert!(!response.body.windows(2).any(|window| window == b"\r\n"));
    }
}
