//! WL-FUNC-014 — AirSonic/Subsonic gateway proxy responder.
//!
//! An AirSonic gateway row in QNM-Shared advertises:
//!
//! `http://<gateway>.mesh:4040/mde/airsonic/<source_id>`
//!
//! This worker owns that responder on the gateway node. It deliberately keeps
//! the shared read-only Subsonic credential server-side: clients send ordinary
//! Subsonic `/rest/...` requests under the gateway prefix, mackesd resolves the
//! source's `credential_ref` through the mesh secret store, strips any
//! client-supplied Subsonic auth query parameters, injects a fresh server-side
//! `u`/`t`/`s` token triplet, strips client auth / hop-by-hop headers, and
//! forwards the request to the LAN upstream.

#![cfg(feature = "async-services")]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::ipc::secret_store::{self, SecretStore};
use crate::mesh_media::{
    self, read_airsonic_gateway_sources_from_plane, AirsonicGatewaySource, GatewayHealth,
    AIRSONIC_PORT,
};

use super::{ShutdownToken, Worker};

/// Bind override for the mesh AirSonic gateway responder.
pub const AIRSONIC_GATEWAY_BIND_ENV: &str = "MDE_AIRSONIC_GATEWAY_BIND";
/// The advertised AirSonic gateway source URL uses the Subsonic default port.
pub const DEFAULT_BIND_ADDR: &str = "0.0.0.0:4040";

const MAX_HEADER_BYTES: usize = 32 * 1024;
const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(30);
const RESPONSE_HEADERS_TO_FORWARD: &[(&str, &str)] = &[
    ("content-type", "Content-Type"),
    ("content-length", "Content-Length"),
    ("content-range", "Content-Range"),
    ("accept-ranges", "Accept-Ranges"),
    ("etag", "ETag"),
    ("last-modified", "Last-Modified"),
    ("cache-control", "Cache-Control"),
    ("expires", "Expires"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedRequest {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GatewayRoute {
    source_id: String,
    upstream_url: String,
    credential_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GatewayCredential {
    username: String,
    password: String,
}

#[derive(Debug)]
enum ProxyFailure {
    BeforeResponse { status: u16, message: String },
    AfterResponse { message: String },
}

impl ProxyFailure {
    fn before_response(status: u16, message: impl Into<String>) -> Self {
        Self::BeforeResponse {
            status,
            message: message.into(),
        }
    }

    fn after_response(message: impl Into<String>) -> Self {
        Self::AfterResponse {
            message: message.into(),
        }
    }
}

/// Test seam over the encrypted mesh secret store.
pub trait CredentialProvider: Send + Sync {
    /// Resolve one sealed credential reference into its decrypted body.
    fn get_secret(&self, name: &str) -> Result<Option<String>, String>;
}

impl CredentialProvider for SecretStore {
    fn get_secret(&self, name: &str) -> Result<Option<String>, String> {
        self.get(name)
    }
}

/// Worker handle for the AirSonic/Subsonic gateway responder.
pub struct AirsonicGatewayProxyWorker {
    node_id: String,
    hostname: String,
    workgroup_root: PathBuf,
    bind_addr: SocketAddr,
    credentials: Arc<dyn CredentialProvider>,
    http: reqwest::Client,
}

impl AirsonicGatewayProxyWorker {
    /// Construct with production defaults.
    #[must_use]
    pub fn new(node_id: String, hostname: String, workgroup_root: PathBuf) -> Self {
        let bind_addr = std::env::var(AIRSONIC_GATEWAY_BIND_ENV)
            .ok()
            .and_then(|raw| raw.parse().ok())
            .unwrap_or_else(|| {
                DEFAULT_BIND_ADDR
                    .parse()
                    .expect("default AirSonic gateway bind parses")
            });
        let secret_store = SecretStore::resolve(&secret_store::repo_root(), &workgroup_root);
        let http = reqwest::Client::builder()
            .timeout(UPSTREAM_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("reqwest client builds");
        Self {
            node_id,
            hostname,
            workgroup_root,
            bind_addr,
            credentials: Arc::new(secret_store),
            http,
        }
    }

    /// Override bind address for tests or non-standard deployments.
    #[must_use]
    pub fn with_bind_addr(mut self, bind_addr: SocketAddr) -> Self {
        self.bind_addr = bind_addr;
        self
    }

    /// Override the credential provider for tests.
    #[must_use]
    pub fn with_credential_provider(mut self, provider: Arc<dyn CredentialProvider>) -> Self {
        self.credentials = provider;
        self
    }

    fn gateway_aliases(&self) -> Vec<String> {
        let mut out = vec![self.hostname.clone(), self.node_id.clone()];
        if let Some(stripped) = self.node_id.strip_prefix("peer:") {
            out.push(stripped.to_string());
        }
        out.sort();
        out.dedup();
        out
    }

    #[allow(dead_code)]
    fn local_sources(&self) -> Vec<AirsonicGatewaySource> {
        let aliases = self.gateway_aliases();
        read_airsonic_gateway_sources_from_plane(&self.workgroup_root)
            .into_iter()
            .filter(|source| source_matches_gateway_alias(source, &aliases))
            .collect()
    }
}

#[async_trait::async_trait]
impl Worker for AirsonicGatewayProxyWorker {
    fn name(&self) -> &'static str {
        "media_airsonic_proxy"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let listener = match TcpListener::bind(self.bind_addr).await {
            Ok(listener) => listener,
            Err(error) => {
                tracing::warn!(
                    target: "mackesd::media_airsonic_proxy",
                    bind = %self.bind_addr,
                    error = %error,
                    "AirSonic gateway proxy bind failed",
                );
                return Err(anyhow::anyhow!("airsonic gateway proxy bind: {error}"));
            }
        };
        tracing::info!(
            target: "mackesd::media_airsonic_proxy",
            bind = %self.bind_addr,
            node = %self.node_id,
            host = %self.hostname,
            "AirSonic gateway proxy accepting mesh requests",
        );

        let aliases = self.gateway_aliases();
        loop {
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                accept = listener.accept() => {
                    if let Ok((stream, _)) = accept {
                        let workgroup_root = self.workgroup_root.clone();
                        let credentials = Arc::clone(&self.credentials);
                        let http = self.http.clone();
                        let aliases = aliases.clone();
                        tokio::spawn(async move {
                            handle_conn(stream, workgroup_root, aliases, credentials, http).await;
                        });
                    }
                }
            }
        }
    }
}

async fn handle_conn(
    mut stream: TcpStream,
    workgroup_root: PathBuf,
    gateway_aliases: Vec<String>,
    credentials: Arc<dyn CredentialProvider>,
    http: reqwest::Client,
) {
    let request = match read_request(&mut stream).await {
        Ok(request) => request,
        Err((status, message)) => {
            let _ = write_text_response(&mut stream, status, &message).await;
            return;
        }
    };

    let sources: Vec<AirsonicGatewaySource> =
        read_airsonic_gateway_sources_from_plane(&workgroup_root)
            .into_iter()
            .filter(|source| source_matches_gateway_alias(source, &gateway_aliases))
            .collect();

    let route = match resolve_gateway_route(&request.target, &sources) {
        Ok(route) => route,
        Err((status, message)) => {
            let _ = write_text_response(&mut stream, status, message).await;
            return;
        }
    };

    let credential = match credentials.get_secret(&route.credential_ref) {
        Ok(Some(body)) => match parse_gateway_credential(&body) {
            Some(credential) => credential,
            None => {
                let _ =
                    write_text_response(&mut stream, 503, "airsonic gateway credential is invalid")
                        .await;
                return;
            }
        },
        Ok(None) => {
            let _ = write_text_response(
                &mut stream,
                503,
                "airsonic gateway credential is not distributed",
            )
            .await;
            return;
        }
        Err(error) => {
            tracing::warn!(
                target: "mackesd::media_airsonic_proxy",
                source = %route.source_id,
                credential_ref = %route.credential_ref,
                error = %error,
                "credential resolution failed",
            );
            let _ =
                write_text_response(&mut stream, 503, "airsonic gateway credential error").await;
            return;
        }
    };

    let _ = proxy_request_to_client(&mut stream, &http, request, route, credential).await;
}

async fn proxy_request_to_client(
    stream: &mut TcpStream,
    http: &reqwest::Client,
    request: ParsedRequest,
    route: GatewayRoute,
    credential: GatewayCredential,
) -> bool {
    let source_id = route.source_id.clone();
    match proxy_request(stream, http, request, route, credential).await {
        Ok(()) => false,
        Err(ProxyFailure::BeforeResponse { status, message }) => {
            let _ = write_text_response(stream, status, &message).await;
            false
        }
        Err(ProxyFailure::AfterResponse { message }) => {
            tracing::warn!(
                target: "mackesd::media_airsonic_proxy",
                source = %source_id,
                error = %message,
                "AirSonic upstream failed after the client response was committed",
            );
            true
        }
    }
}

async fn read_request(stream: &mut TcpStream) -> Result<ParsedRequest, (u16, String)> {
    let mut buf = Vec::new();
    let head_end = loop {
        if let Some(pos) = find_header_end(&buf) {
            break pos;
        }
        if buf.len() > MAX_HEADER_BYTES {
            return Err((
                431,
                "airsonic gateway request headers too large".to_string(),
            ));
        }
        let mut chunk = [0u8; 4096];
        let n = stream
            .read(&mut chunk)
            .await
            .map_err(|e| (400, format!("airsonic gateway request read failed: {e}")))?;
        if n == 0 {
            return Err((400, "empty airsonic gateway request".to_string()));
        }
        buf.extend_from_slice(&chunk[..n]);
    };

    let head = String::from_utf8(buf[..head_end].to_vec()).map_err(|_| {
        (
            400,
            "airsonic gateway request head is not utf-8".to_string(),
        )
    })?;
    let mut lines = head.lines();
    let request_line = lines
        .next()
        .ok_or_else(|| (400, "missing airsonic gateway request line".to_string()))?;
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| (400, "missing airsonic gateway method".to_string()))?
        .to_ascii_uppercase();
    let target = parts
        .next()
        .ok_or_else(|| (400, "missing airsonic gateway target".to_string()))?
        .to_string();
    if !matches!(method.as_str(), "GET" | "HEAD") {
        return Err((405, "airsonic gateway method is not allowed".to_string()));
    }

    let mut headers = Vec::new();
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_string();
        let value = value.trim().to_string();
        if name.eq_ignore_ascii_case("content-length") {
            let content_length = value
                .parse::<usize>()
                .map_err(|_| (400, "invalid airsonic gateway content-length".to_string()))?;
            if content_length > 0 {
                return Err((
                    400,
                    "airsonic gateway Subsonic proxy accepts query-auth GET/HEAD only".to_string(),
                ));
            }
        }
        if name.eq_ignore_ascii_case("transfer-encoding") && !value.is_empty() {
            return Err((
                400,
                "airsonic gateway Subsonic proxy rejects request bodies".to_string(),
            ));
        }
        headers.push((name, value));
    }

    Ok(ParsedRequest {
        method,
        target,
        headers,
    })
}

async fn proxy_request(
    stream: &mut TcpStream,
    http: &reqwest::Client,
    request: ParsedRequest,
    route: GatewayRoute,
    credential: GatewayCredential,
) -> Result<(), ProxyFailure> {
    let upstream_url = inject_subsonic_auth_query_params(&route.upstream_url, &credential)
        .ok_or_else(|| {
            ProxyFailure::before_response(400, "invalid airsonic gateway upstream query")
        })?;
    let method = match request.method.as_str() {
        "GET" => reqwest::Method::GET,
        "HEAD" => reqwest::Method::HEAD,
        _ => {
            return Err(ProxyFailure::before_response(
                405,
                "airsonic gateway method is not allowed",
            ));
        }
    };
    let headers = forwarded_headers(&request.headers);
    let mut response = http
        .request(method, &upstream_url)
        .headers(headers)
        .send()
        .await
        .map_err(|e| {
            ProxyFailure::before_response(502, format!("airsonic upstream request failed: {e}"))
        })?;
    let status = response.status();
    let response_headers = response.headers().clone();
    let content_length = response.content_length();
    let header = render_response_head(status, &response_headers, content_length);
    stream.write_all(header.as_bytes()).await.map_err(|e| {
        ProxyFailure::after_response(format!("airsonic gateway response write failed: {e}"))
    })?;
    while let Some(chunk) = response.chunk().await.map_err(|e| {
        ProxyFailure::after_response(format!("airsonic upstream response read failed: {e}"))
    })? {
        stream.write_all(&chunk).await.map_err(|e| {
            ProxyFailure::after_response(format!("airsonic gateway response stream failed: {e}"))
        })?;
    }
    stream.flush().await.map_err(|e| {
        ProxyFailure::after_response(format!("airsonic gateway response flush failed: {e}"))
    })?;
    Ok(())
}

fn render_response_head(
    status: reqwest::StatusCode,
    headers: &HeaderMap,
    content_length: Option<u64>,
) -> String {
    let reason = status.canonical_reason().unwrap_or("Status");
    let mut out = format!("HTTP/1.1 {} {}\r\n", status.as_u16(), reason);
    let mut wrote_content_type = false;
    let mut wrote_content_length = false;
    for (name, display_name) in RESPONSE_HEADERS_TO_FORWARD {
        for value in headers.get_all(*name) {
            let Some(value) = safe_response_header_value(value) else {
                continue;
            };
            wrote_content_type |= *name == "content-type";
            wrote_content_length |= *name == "content-length";
            out.push_str(display_name);
            out.push_str(": ");
            out.push_str(value);
            out.push_str("\r\n");
        }
    }
    if !wrote_content_type {
        out.push_str("Content-Type: application/octet-stream\r\n");
    }
    if !wrote_content_length {
        if let Some(len) = content_length {
            out.push_str(&format!("Content-Length: {len}\r\n"));
        }
    }
    out.push_str("Connection: close\r\n");
    out.push_str("\r\n");
    out
}

fn safe_response_header_value(value: &HeaderValue) -> Option<&str> {
    let value = value.to_str().ok()?;
    if value.contains(['\r', '\n']) {
        None
    } else {
        Some(value)
    }
}

async fn write_text_response(
    stream: &mut TcpStream,
    status: u16,
    message: &str,
) -> std::io::Result<()> {
    let reason = match status {
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        431 => "Request Header Fields Too Large",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Error",
    };
    let body = message.as_bytes();
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=\"utf-8\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await
}

fn resolve_gateway_route(
    target: &str,
    sources: &[AirsonicGatewaySource],
) -> Result<GatewayRoute, (u16, &'static str)> {
    let Some(rest) = target.strip_prefix("/mde/airsonic/") else {
        return Err((404, "not an airsonic gateway route"));
    };
    let Some(pos) = rest.find('/') else {
        return Err((404, "missing airsonic gateway Subsonic path"));
    };
    let (source_id, upstream_suffix) = (&rest[..pos], rest[pos..].to_string());
    if source_id.is_empty() || !source_id.bytes().all(is_safe_source_id_byte) {
        return Err((400, "invalid airsonic gateway source id"));
    }
    let suffix_path = upstream_suffix.split('?').next().unwrap_or("");
    if !suffix_path.starts_with("/rest/") {
        return Err((
            404,
            "airsonic gateway only forwards Subsonic /rest API paths",
        ));
    }
    let source = sources
        .iter()
        .find(|source| source.id == source_id)
        .ok_or((404, "unknown airsonic gateway source"))?;
    if source.health != GatewayHealth::Healthy {
        return Err((503, "airsonic gateway source is degraded"));
    }
    let upstream_url = join_upstream_url(&source.upstream_url, &upstream_suffix)
        .ok_or((400, "invalid airsonic gateway upstream path"))?;
    Ok(GatewayRoute {
        source_id: source.id.clone(),
        upstream_url,
        credential_ref: source.credential_ref.clone(),
    })
}

fn join_upstream_url(base: &str, suffix: &str) -> Option<String> {
    if suffix.contains("://") || suffix.contains('\\') {
        return None;
    }
    let path_part = suffix.split('?').next().unwrap_or(suffix);
    if path_part
        .split('/')
        .any(|segment| matches!(segment, "." | ".."))
    {
        return None;
    }
    let mut out = base.trim_end_matches('/').to_string();
    if suffix.starts_with('/') {
        out.push_str(suffix);
    } else {
        out.push('/');
        out.push_str(suffix);
    }
    strip_client_auth_query_params(&out)
}

fn strip_client_auth_query_params(url: &str) -> Option<String> {
    let mut parsed = reqwest::Url::parse(url).ok()?;
    let Some(_) = parsed.query() else {
        return Some(parsed.to_string());
    };
    let retained: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(key, _)| !is_client_auth_query_key(key))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    parsed.set_query(None);
    if !retained.is_empty() {
        let mut query = parsed.query_pairs_mut();
        for (key, value) in retained {
            query.append_pair(&key, &value);
        }
    }
    Some(parsed.to_string())
}

fn inject_subsonic_auth_query_params(url: &str, credential: &GatewayCredential) -> Option<String> {
    let salt = subsonic_auth_salt();
    inject_subsonic_auth_query_params_with_salt(url, credential, &salt)
}

fn inject_subsonic_auth_query_params_with_salt(
    url: &str,
    credential: &GatewayCredential,
    salt: &str,
) -> Option<String> {
    let mut parsed = reqwest::Url::parse(&strip_client_auth_query_params(url)?).ok()?;
    let token = subsonic_auth_token(&credential.password, salt);
    {
        let mut query = parsed.query_pairs_mut();
        query.append_pair("u", &credential.username);
        query.append_pair("t", &token);
        query.append_pair("s", salt);
    }
    Some(parsed.to_string())
}

fn is_client_auth_query_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "u" | "user" | "username" | "p" | "password" | "t" | "token" | "s" | "salt"
    )
}

fn source_matches_gateway_alias(source: &AirsonicGatewaySource, aliases: &[String]) -> bool {
    aliases.iter().any(|alias| {
        let alias = alias.trim();
        !alias.is_empty()
            && (source.gateway_node.eq_ignore_ascii_case(alias)
                || mesh_media::airsonic_gateway_source_id(alias, &source.upstream_url).as_deref()
                    == Some(source.id.as_str()))
    })
}

fn forwarded_headers(incoming: &[(String, String)]) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (name, value) in incoming {
        if should_strip_header(name) {
            continue;
        }
        let Ok(header_name) = HeaderName::from_bytes(name.as_bytes()) else {
            continue;
        };
        let Ok(header_value) = HeaderValue::from_str(value) else {
            continue;
        };
        out.append(header_name, header_value);
    }
    out
}

fn should_strip_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization"
            | "cookie"
            | "connection"
            | "content-length"
            | "host"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

fn parse_gateway_credential(body: &str) -> Option<GatewayCredential> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct JsonCredential {
        username: String,
        password: String,
    }

    let parsed: JsonCredential = serde_json::from_str(body.trim()).ok()?;
    let username = parsed.username.trim();
    if username.is_empty() || username != parsed.username {
        return None;
    }
    Some(GatewayCredential {
        username: username.to_string(),
        password: parsed.password,
    })
}

fn subsonic_auth_token(password: &str, salt: &str) -> String {
    let digest = md5::compute(format!("{password}{salt}").as_bytes());
    format!("{digest:x}")
}

fn subsonic_auth_salt() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{nanos:x}{:016x}", rand::random::<u64>())
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

const fn is_safe_source_id_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.')
}

#[allow(dead_code)]
fn has_local_gateway_source(workgroup_root: &Path, aliases: &[String]) -> bool {
    read_airsonic_gateway_sources_from_plane(workgroup_root)
        .iter()
        .any(|source| source_matches_gateway_alias(source, aliases))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh_media::{AirsonicGatewayRegistration, GatewayHealth};
    use std::collections::HashMap;

    #[derive(Default)]
    struct StaticCredentials {
        values: HashMap<String, String>,
    }

    impl CredentialProvider for StaticCredentials {
        fn get_secret(&self, name: &str) -> Result<Option<String>, String> {
            Ok(self.values.get(name).cloned())
        }
    }

    async fn read_http_request_bytes(stream: &mut TcpStream) -> Vec<u8> {
        let mut buf = Vec::new();
        loop {
            if find_header_end(&buf).is_some() {
                break;
            }
            let mut chunk = [0u8; 1024];
            let n = stream.read(&mut chunk).await.expect("read request");
            assert_ne!(n, 0, "request ended before header terminator");
            buf.extend_from_slice(&chunk[..n]);
        }
        buf
    }

    async fn proxy_response_for(
        request: ParsedRequest,
        route: GatewayRoute,
        credential: GatewayCredential,
    ) -> Vec<u8> {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let client = tokio::spawn(async move {
            let mut stream = TcpStream::connect(addr).await.unwrap();
            let mut response = Vec::new();
            stream.read_to_end(&mut response).await.unwrap();
            response
        });
        let (mut gateway_stream, _) = listener.accept().await.unwrap();
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        proxy_request(&mut gateway_stream, &http, request, route, credential)
            .await
            .unwrap();
        drop(gateway_stream);
        client.await.unwrap()
    }

    fn header_value<'a>(head: &'a str, name: &str) -> Option<&'a str> {
        head.lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.trim())
    }

    fn source(
        gateway_node: &str,
        upstream_url: &str,
        health: GatewayHealth,
    ) -> AirsonicGatewaySource {
        let reg = AirsonicGatewayRegistration::new(
            gateway_node,
            upstream_url,
            "media/airsonic/shared-readonly",
            health,
            true,
        )
        .unwrap();
        mesh_media::source_from_airsonic_gateway(&reg).unwrap()
    }

    #[test]
    fn worker_name_is_media_airsonic_proxy() {
        let worker = AirsonicGatewayProxyWorker::new(
            "peer:seat-15".to_string(),
            "seat-15".to_string(),
            PathBuf::from("/tmp/no-plane"),
        )
        .with_bind_addr("127.0.0.1:0".parse().unwrap());
        assert_eq!(worker.name(), "media_airsonic_proxy");
        assert_eq!(worker.bind_addr.port(), 0);
        assert_eq!(AIRSONIC_PORT, 4040);
    }

    #[test]
    fn route_rewrites_prefixed_rest_path_and_strips_client_auth() {
        let src = source(
            "seat-15",
            "http://192.168.1.60:4040/base",
            GatewayHealth::Healthy,
        );
        let target = format!(
            "/mde/airsonic/{}/rest/stream?id=song-7&u=client&t=CLIENT&s=CLIENTSALT&v=1.16.1&c=mde-music&f=json",
            src.id
        );
        let route = resolve_gateway_route(&target, &[src]).unwrap();
        assert_eq!(
            route.upstream_url,
            "http://192.168.1.60:4040/base/rest/stream?id=song-7&v=1.16.1&c=mde-music&f=json"
        );
        assert_eq!(route.credential_ref, "media/airsonic/shared-readonly");
    }

    #[test]
    fn route_rejects_non_rest_unknown_degraded_and_path_escape() {
        let degraded = source(
            "seat-15",
            "http://airsonic.lan:4040",
            GatewayHealth::Degraded,
        );
        let degraded_target = format!("/mde/airsonic/{}/rest/ping?v=1.16.1", degraded.id);
        assert_eq!(
            resolve_gateway_route(&degraded_target, &[degraded])
                .unwrap_err()
                .0,
            503
        );
        assert_eq!(
            resolve_gateway_route("/mde/airsonic/missing/rest/ping", &[])
                .unwrap_err()
                .0,
            404
        );
        assert_eq!(
            resolve_gateway_route("/mde/airsonic/source-id/api/ping", &[])
                .unwrap_err()
                .0,
            404
        );
        assert!(join_upstream_url("http://airsonic.lan:4040", "/rest/../admin").is_none());
        assert!(join_upstream_url("http://airsonic.lan:4040", "/rest/http://x").is_none());
    }

    #[test]
    fn aliases_identify_local_gateway_source() {
        let src = source(
            "Seat-15",
            "http://airsonic.lan:4040",
            GatewayHealth::Healthy,
        );
        assert!(source_matches_gateway_alias(
            &src,
            &["seat-15".to_string(), "peer:seat-15".to_string()]
        ));
        assert!(!source_matches_gateway_alias(&src, &["eagle".to_string()]));
    }

    #[test]
    fn credential_accepts_strict_subsonic_auth_pair_only() {
        let credential =
            parse_gateway_credential(r#"{"username":"mesh-readonly","password":"sesame"}"#)
                .unwrap();
        assert_eq!(credential.username, "mesh-readonly");
        assert_eq!(credential.password, "sesame");
        assert!(parse_gateway_credential(
            r#"{"username":"mesh-readonly","password":"sesame","server_url":"http://music.mesh"}"#
        )
        .is_none());
        assert!(parse_gateway_credential(r#"{"username":" mesh ","password":"sesame"}"#).is_none());
        assert!(parse_gateway_credential(r#"{"username":"","password":"sesame"}"#).is_none());
    }

    #[test]
    fn injected_subsonic_auth_replaces_all_client_credentials() {
        let credential = GatewayCredential {
            username: "mesh-readonly".to_string(),
            password: "sesame".to_string(),
        };
        let url = inject_subsonic_auth_query_params_with_salt(
            "http://airsonic.lan:4040/rest/ping?u=client&p=secret&t=CLIENT&s=CLIENTSALT&v=1.16.1&c=mde-music&f=json",
            &credential,
            "c19b2d",
        )
        .unwrap();
        assert_eq!(
            url,
            "http://airsonic.lan:4040/rest/ping?v=1.16.1&c=mde-music&f=json&u=mesh-readonly&t=26719a1196d2a940705a59634eb18eab&s=c19b2d"
        );
        assert!(!url.contains("client"));
        assert!(!url.contains("secret"));
        assert_eq!(
            subsonic_auth_token("sesame", "c19b2d"),
            "26719a1196d2a940705a59634eb18eab"
        );
    }

    #[test]
    fn forwarded_headers_strip_client_auth_and_hop_by_hop() {
        let headers = forwarded_headers(&[
            ("Host".to_string(), "seat-15.mesh:4040".to_string()),
            ("Connection".to_string(), "keep-alive".to_string()),
            ("Authorization".to_string(), "client-token".to_string()),
            ("Range".to_string(), "bytes=4-9".to_string()),
            ("Accept".to_string(), "audio/*".to_string()),
        ]);
        assert!(headers.get("host").is_none());
        assert!(headers.get("authorization").is_none());
        assert_eq!(headers.get("range").unwrap().to_str().unwrap(), "bytes=4-9");
        assert_eq!(headers.get("accept").unwrap().to_str().unwrap(), "audio/*");
    }

    #[tokio::test]
    async fn read_request_rejects_transfer_encoded_bodies() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let client = tokio::spawn(async move {
            let mut stream = TcpStream::connect(addr).await.unwrap();
            stream
                .write_all(
                    b"GET /mde/airsonic/source/rest/ping HTTP/1.1\r\n\
                      Host: seat-15.mesh:4040\r\n\
                      Transfer-Encoding: chunked\r\n\
                      \r\n\
                      0\r\n\r\n",
                )
                .await
                .unwrap();
        });
        let (mut stream, _) = listener.accept().await.unwrap();
        let err = read_request(&mut stream).await.unwrap_err();
        client.await.unwrap();
        assert_eq!(err.0, 400);
        assert!(err.1.contains("rejects request bodies"));
    }

    #[tokio::test]
    async fn proxy_preserves_range_stream_status_headers_body_and_server_auth() {
        let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream.local_addr().unwrap();
        let upstream_task = tokio::spawn(async move {
            let (mut stream, _) = upstream.accept().await.unwrap();
            let request = read_http_request_bytes(&mut stream).await;
            let request_text = String::from_utf8_lossy(&request);
            assert!(
                request_text.starts_with("GET /base/rest/stream?"),
                "{request_text}"
            );
            let request_lower = request_text.to_ascii_lowercase();
            assert!(request_lower.contains("\r\nrange: bytes=4-9\r\n"));
            assert!(!request_text.contains("CLIENT-TOKEN"));
            assert!(!request_text.contains("CLIENTSALT"));
            assert!(!request_text.contains("client-user"));

            let first_line = request_text.lines().next().unwrap();
            let target = first_line.split_whitespace().nth(1).unwrap();
            let parsed = reqwest::Url::parse(&format!("http://upstream{target}")).unwrap();
            let pairs: HashMap<String, String> = parsed.query_pairs().into_owned().collect();
            assert_eq!(pairs.get("id").map(String::as_str), Some("song-7"));
            assert_eq!(pairs.get("v").map(String::as_str), Some("1.16.1"));
            assert_eq!(pairs.get("c").map(String::as_str), Some("mde-music"));
            assert_eq!(pairs.get("f").map(String::as_str), Some("json"));
            assert_eq!(pairs.get("u").map(String::as_str), Some("mesh-readonly"));
            assert!(pairs.get("t").is_some_and(|token| token.len() == 32));
            assert!(pairs.get("s").is_some_and(|salt| !salt.is_empty()));

            stream
                .write_all(
                    b"HTTP/1.1 206 Partial Content\r\n\
                      Content-Type: audio/mpeg\r\n\
                      Content-Length: 6\r\n\
                      Content-Range: bytes 4-9/20\r\n\
                      Accept-Ranges: bytes\r\n\
                      ETag: \"song-etag\"\r\n\
                      Connection: close\r\n\
                      \r\n\
                      EFGHIJ",
                )
                .await
                .unwrap();
        });

        let source = source(
            "seat-15",
            &format!("http://{upstream_addr}/base"),
            GatewayHealth::Healthy,
        );
        let target = format!(
            "/mde/airsonic/{}/rest/stream?id=song-7&u=client-user&t=CLIENT-TOKEN&s=CLIENTSALT&v=1.16.1&c=mde-music&f=json",
            source.id
        );
        let route = resolve_gateway_route(&target, &[source]).unwrap();
        let response = proxy_response_for(
            ParsedRequest {
                method: "GET".to_string(),
                target,
                headers: vec![
                    ("Host".to_string(), "seat-15.mesh:4040".to_string()),
                    ("Authorization".to_string(), "client-token".to_string()),
                    ("Range".to_string(), "bytes=4-9".to_string()),
                    ("Accept".to_string(), "audio/*".to_string()),
                ],
            },
            route,
            GatewayCredential {
                username: "mesh-readonly".to_string(),
                password: "sesame".to_string(),
            },
        )
        .await;
        upstream_task.await.unwrap();

        let head_end = find_header_end(&response).expect("response head");
        let head = String::from_utf8_lossy(&response[..head_end]);
        assert!(head.starts_with("HTTP/1.1 206 Partial Content"), "{head}");
        assert_eq!(header_value(&head, "Content-Type"), Some("audio/mpeg"));
        assert_eq!(header_value(&head, "Content-Length"), Some("6"));
        assert_eq!(header_value(&head, "Content-Range"), Some("bytes 4-9/20"));
        assert_eq!(header_value(&head, "Accept-Ranges"), Some("bytes"));
        assert_eq!(header_value(&head, "ETag"), Some("\"song-etag\""));
        assert_eq!(&response[head_end + 4..], b"EFGHIJ");
    }

    #[tokio::test]
    async fn truncated_provider_response_cannot_append_a_second_http_reply() {
        let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream.local_addr().unwrap();
        let upstream_task = tokio::spawn(async move {
            let (mut stream, _) = upstream.accept().await.unwrap();
            let _request = read_http_request_bytes(&mut stream).await;
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\n\
                      Content-Type: audio/mpeg\r\n\
                      Content-Length: 12\r\n\
                      Connection: close\r\n\
                      \r\n\
                      short",
                )
                .await
                .unwrap();
        });

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let client = tokio::spawn(async move {
            let mut stream = TcpStream::connect(addr).await.unwrap();
            let mut response = Vec::new();
            stream.read_to_end(&mut response).await.unwrap();
            response
        });
        let (mut gateway_stream, _) = listener.accept().await.unwrap();
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let committed_failure = proxy_request_to_client(
            &mut gateway_stream,
            &http,
            ParsedRequest {
                method: "GET".to_string(),
                target: "/mde/airsonic/source/rest/stream?id=song-7".to_string(),
                headers: Vec::new(),
            },
            GatewayRoute {
                source_id: "source".to_string(),
                upstream_url: format!("http://{upstream_addr}/rest/stream?id=song-7"),
                credential_ref: "media/airsonic/shared-readonly".to_string(),
            },
            GatewayCredential {
                username: "mesh-readonly".to_string(),
                password: "sesame".to_string(),
            },
        )
        .await;
        drop(gateway_stream);
        upstream_task.await.unwrap();
        let response = client.await.unwrap();
        let response_text = String::from_utf8_lossy(&response);

        assert!(committed_failure, "truncated body must be detected");
        assert!(
            response_text.starts_with("HTTP/1.1 200 OK\r\n"),
            "{response_text}"
        );
        assert_eq!(
            response_text.matches("HTTP/1.1 ").count(),
            1,
            "{response_text}"
        );
        assert!(
            !response_text.contains("502 Bad Gateway"),
            "{response_text}"
        );
    }

    #[test]
    fn static_credential_provider_is_a_test_seam() {
        let mut provider = StaticCredentials::default();
        provider.values.insert(
            "media/airsonic/shared-readonly".to_string(),
            r#"{"username":"mesh-readonly","password":"sesame"}"#.to_string(),
        );
        assert_eq!(
            provider
                .get_secret("media/airsonic/shared-readonly")
                .unwrap()
                .as_deref(),
            Some(r#"{"username":"mesh-readonly","password":"sesame"}"#)
        );
    }
}
