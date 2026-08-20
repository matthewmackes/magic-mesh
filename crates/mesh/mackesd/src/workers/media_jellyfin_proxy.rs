//! WL-FUNC-015 — Jellyfin gateway proxy responder.
//!
//! A Jellyfin gateway row in QNM-Shared advertises:
//!
//! `http://<gateway>.mesh:8097/mde/jellyfin/<source_id>`
//!
//! This worker owns that responder on the gateway node. It deliberately keeps
//! the shared read-only Jellyfin token server-side: clients send ordinary
//! Jellyfin paths under the gateway prefix, mackesd resolves the source's
//! `credential_ref` through the mesh secret store, injects the Jellyfin
//! `Authorization` header, strips client-supplied auth / hop-by-hop headers, and
//! forwards the request to the LAN upstream.

#![cfg(feature = "async-services")]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mackes_mesh_types::media_sources::JELLYFIN_GATEWAY_USER_SENTINEL;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::ipc::secret_store::{self, SecretStore};
use crate::mesh_media::{
    self, read_jellyfin_gateway_sources_from_plane, GatewayHealth, JellyfinGatewaySource,
};

use super::{ShutdownToken, Worker};

/// Bind override for the mesh Jellyfin gateway responder.
pub const JELLYFIN_GATEWAY_BIND_ENV: &str = "MDE_JELLYFIN_GATEWAY_BIND";
/// The advertised Jellyfin gateway source URL uses a proxy-specific port, not
/// the standard direct-Jellyfin 8096 port.
pub const DEFAULT_BIND_ADDR: &str = "0.0.0.0:8097";

const MAX_HEADER_BYTES: usize = 32 * 1024;
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
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
    body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GatewayRoute {
    source_id: String,
    upstream_url: String,
    credential_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GatewayCredential {
    access_token: String,
    user_id: Option<String>,
}

impl GatewayCredential {
    fn authorization_header(&self) -> String {
        let token = sanitize_header_value(&self.access_token);
        format!(
            "MediaBrowser Client=\"mde-media-gateway\", Device=\"mackesd\", \
             DeviceId=\"mesh-gateway\", Version=\"{}\", Token=\"{}\"",
            env!("CARGO_PKG_VERSION"),
            token
        )
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

/// Worker handle for the Jellyfin gateway responder.
pub struct JellyfinGatewayProxyWorker {
    node_id: String,
    hostname: String,
    workgroup_root: PathBuf,
    bind_addr: SocketAddr,
    credentials: Arc<dyn CredentialProvider>,
    http: reqwest::Client,
}

impl JellyfinGatewayProxyWorker {
    /// Construct with production defaults.
    #[must_use]
    pub fn new(node_id: String, hostname: String, workgroup_root: PathBuf) -> Self {
        let bind_addr = std::env::var(JELLYFIN_GATEWAY_BIND_ENV)
            .ok()
            .and_then(|raw| raw.parse().ok())
            .unwrap_or_else(|| {
                DEFAULT_BIND_ADDR
                    .parse()
                    .expect("default Jellyfin gateway bind parses")
            });
        let secret_store = SecretStore::resolve(&secret_store::repo_root(), &workgroup_root);
        let http = reqwest::Client::builder()
            .timeout(UPSTREAM_TIMEOUT)
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
    fn local_sources(&self) -> Vec<JellyfinGatewaySource> {
        let aliases = self.gateway_aliases();
        read_jellyfin_gateway_sources_from_plane(&self.workgroup_root)
            .into_iter()
            .filter(|source| source_matches_gateway_alias(source, &aliases))
            .collect()
    }
}

#[async_trait::async_trait]
impl Worker for JellyfinGatewayProxyWorker {
    fn name(&self) -> &'static str {
        "media_jellyfin_proxy"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let listener = match TcpListener::bind(self.bind_addr).await {
            Ok(listener) => listener,
            Err(error) => {
                tracing::warn!(
                    target: "mackesd::media_jellyfin_proxy",
                    bind = %self.bind_addr,
                    error = %error,
                    "Jellyfin gateway proxy bind failed",
                );
                return Err(anyhow::anyhow!("jellyfin gateway proxy bind: {error}"));
            }
        };
        tracing::info!(
            target: "mackesd::media_jellyfin_proxy",
            bind = %self.bind_addr,
            node = %self.node_id,
            host = %self.hostname,
            "Jellyfin gateway proxy accepting mesh requests",
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

    let sources: Vec<JellyfinGatewaySource> =
        read_jellyfin_gateway_sources_from_plane(&workgroup_root)
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
                    write_text_response(&mut stream, 503, "jellyfin gateway credential is invalid")
                        .await;
                return;
            }
        },
        Ok(None) => {
            let _ = write_text_response(
                &mut stream,
                503,
                "jellyfin gateway credential is not distributed",
            )
            .await;
            return;
        }
        Err(error) => {
            tracing::warn!(
                target: "mackesd::media_jellyfin_proxy",
                source = %route.source_id,
                credential_ref = %route.credential_ref,
                error = %error,
                "credential resolution failed",
            );
            let _ =
                write_text_response(&mut stream, 503, "jellyfin gateway credential error").await;
            return;
        }
    };

    // Credential lookup is deliberately outside the replicated source plane and
    // can span a Bus replacement.  Do not let an in-flight request retain the
    // old endpoint/credential binding after that declaration has been revoked,
    // degraded, or corrected forward.
    let current_sources: Vec<JellyfinGatewaySource> =
        read_jellyfin_gateway_sources_from_plane(&workgroup_root)
            .into_iter()
            .filter(|source| source_matches_gateway_alias(source, &gateway_aliases))
            .collect();
    if !route_matches_current_authority(&request.target, &route, &current_sources) {
        let _ = write_text_response(
            &mut stream,
            503,
            "jellyfin gateway source authority changed",
        )
        .await;
        return;
    }

    if let Err((status, message)) =
        proxy_request(&mut stream, &http, request, route, credential).await
    {
        let _ = write_text_response(&mut stream, status, &message).await;
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
                "jellyfin gateway request headers too large".to_string(),
            ));
        }
        let mut chunk = [0u8; 4096];
        let n = stream
            .read(&mut chunk)
            .await
            .map_err(|e| (400, format!("jellyfin gateway request read failed: {e}")))?;
        if n == 0 {
            return Err((400, "empty jellyfin gateway request".to_string()));
        }
        buf.extend_from_slice(&chunk[..n]);
    };

    let head = String::from_utf8(buf[..head_end].to_vec()).map_err(|_| {
        (
            400,
            "jellyfin gateway request head is not utf-8".to_string(),
        )
    })?;
    let mut lines = head.lines();
    let request_line = lines
        .next()
        .ok_or_else(|| (400, "missing jellyfin gateway request line".to_string()))?;
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| (400, "missing jellyfin gateway method".to_string()))?
        .to_ascii_uppercase();
    let target = parts
        .next()
        .ok_or_else(|| (400, "missing jellyfin gateway target".to_string()))?
        .to_string();
    if !matches!(method.as_str(), "GET" | "POST" | "DELETE" | "HEAD") {
        return Err((405, "jellyfin gateway method is not allowed".to_string()));
    }

    let mut headers = Vec::new();
    let mut content_length = 0usize;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_string();
        let value = value.trim().to_string();
        if name.eq_ignore_ascii_case("content-length") {
            content_length = value
                .parse::<usize>()
                .map_err(|_| (400, "invalid jellyfin gateway content-length".to_string()))?;
            if content_length > MAX_BODY_BYTES {
                return Err((413, "jellyfin gateway request body too large".to_string()));
            }
        }
        headers.push((name, value));
    }

    let body_start = head_end + 4;
    let mut body = buf.get(body_start..).unwrap_or_default().to_vec();
    while body.len() < content_length {
        let remaining = content_length - body.len();
        let mut chunk = vec![0u8; remaining.min(4096)];
        let n = stream
            .read(&mut chunk)
            .await
            .map_err(|e| (400, format!("jellyfin gateway body read failed: {e}")))?;
        if n == 0 {
            return Err((400, "truncated jellyfin gateway request body".to_string()));
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(content_length);

    Ok(ParsedRequest {
        method,
        target,
        headers,
        body,
    })
}

async fn proxy_request(
    stream: &mut TcpStream,
    http: &reqwest::Client,
    request: ParsedRequest,
    route: GatewayRoute,
    credential: GatewayCredential,
) -> Result<(), (u16, String)> {
    let upstream_url =
        materialize_gateway_user_text(&route.upstream_url, credential.user_id.as_deref())?;
    let body = materialize_gateway_user_body(request.body, credential.user_id.as_deref())?;
    let method = match request.method.as_str() {
        "GET" => reqwest::Method::GET,
        "POST" => reqwest::Method::POST,
        "DELETE" => reqwest::Method::DELETE,
        "HEAD" => reqwest::Method::HEAD,
        _ => return Err((405, "jellyfin gateway method is not allowed".to_string())),
    };
    let headers = forwarded_headers(&request.headers, &credential)?;
    let mut builder = http.request(method, &upstream_url).headers(headers);
    if !body.is_empty() {
        builder = builder.body(body);
    }
    let mut response = builder
        .send()
        .await
        .map_err(|e| (502, format!("jellyfin upstream request failed: {e}")))?;
    let status = response.status();
    let response_headers = response.headers().clone();
    let content_length = response.content_length();
    let header = render_response_head(status, &response_headers, content_length);
    stream
        .write_all(header.as_bytes())
        .await
        .map_err(|e| (502, format!("jellyfin gateway response write failed: {e}")))?;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| (502, format!("jellyfin upstream response read failed: {e}")))?
    {
        stream
            .write_all(&chunk)
            .await
            .map_err(|e| (502, format!("jellyfin gateway response stream failed: {e}")))?;
    }
    stream
        .flush()
        .await
        .map_err(|e| (502, format!("jellyfin gateway response flush failed: {e}")))?;
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
        413 => "Payload Too Large",
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
    sources: &[JellyfinGatewaySource],
) -> Result<GatewayRoute, (u16, &'static str)> {
    let Some(rest) = target.strip_prefix("/mde/jellyfin/") else {
        return Err((404, "not a jellyfin gateway route"));
    };
    let (source_id, upstream_suffix): (&str, String) = match rest.find('/') {
        Some(pos) => (&rest[..pos], rest[pos..].to_string()),
        None => rest
            .split_once('?')
            .map_or((rest, "/".to_string()), |(id, query)| {
                (id, format!("/?{query}"))
            }),
    };
    if source_id.is_empty() || !source_id.bytes().all(is_safe_source_id_byte) {
        return Err((400, "invalid jellyfin gateway source id"));
    }
    let source = sources
        .iter()
        .find(|source| source.id == source_id)
        .ok_or((404, "unknown jellyfin gateway source"))?;
    if source.health != GatewayHealth::Healthy {
        return Err((503, "jellyfin gateway source is degraded"));
    }
    let upstream_url = join_upstream_url(&source.upstream_url, &upstream_suffix)
        .ok_or((400, "invalid jellyfin gateway upstream path"))?;
    Ok(GatewayRoute {
        source_id: source.id.clone(),
        upstream_url,
        credential_ref: source.credential_ref.clone(),
    })
}

fn route_matches_current_authority(
    target: &str,
    resolved: &GatewayRoute,
    current_sources: &[JellyfinGatewaySource],
) -> bool {
    resolve_gateway_route(target, current_sources).is_ok_and(|current| current == *resolved)
}

fn join_upstream_url(base: &str, suffix: &str) -> Option<String> {
    if suffix.contains("://") || suffix.contains('\\') {
        return None;
    }
    let suffix = if suffix.is_empty() { "/" } else { suffix };
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

fn is_client_auth_query_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "api_key" | "apikey" | "x-emby-token" | "x-mediabrowser-token"
    )
}

fn materialize_gateway_user_text(
    value: &str,
    user_id: Option<&str>,
) -> Result<String, (u16, String)> {
    if !value.contains(JELLYFIN_GATEWAY_USER_SENTINEL) {
        return Ok(value.to_string());
    }
    let user_id = gateway_user_id(user_id)?;
    Ok(value.replace(JELLYFIN_GATEWAY_USER_SENTINEL, user_id))
}

fn materialize_gateway_user_body(
    body: Vec<u8>,
    user_id: Option<&str>,
) -> Result<Vec<u8>, (u16, String)> {
    if !body
        .windows(JELLYFIN_GATEWAY_USER_SENTINEL.len())
        .any(|window| window == JELLYFIN_GATEWAY_USER_SENTINEL.as_bytes())
    {
        return Ok(body);
    }
    let user_id = gateway_user_id(user_id)?;
    let body = String::from_utf8(body).map_err(|_| {
        (
            400,
            "jellyfin gateway sentinel body is not utf-8".to_string(),
        )
    })?;
    Ok(body
        .replace(JELLYFIN_GATEWAY_USER_SENTINEL, user_id)
        .into_bytes())
}

fn gateway_user_id(user_id: Option<&str>) -> Result<&str, (u16, String)> {
    let Some(user_id) = user_id.map(str::trim).filter(|s| !s.is_empty()) else {
        return Err((
            503,
            "jellyfin gateway credential is missing user_id".to_string(),
        ));
    };
    if !user_id.bytes().all(is_safe_gateway_user_id_byte) {
        return Err((
            503,
            "jellyfin gateway credential user_id is invalid".to_string(),
        ));
    }
    Ok(user_id)
}

fn source_matches_gateway_alias(source: &JellyfinGatewaySource, aliases: &[String]) -> bool {
    aliases.iter().any(|alias| {
        let alias = alias.trim();
        !alias.is_empty()
            && (source.gateway_node.eq_ignore_ascii_case(alias)
                || mesh_media::jellyfin_gateway_source_id(alias, &source.upstream_url).as_deref()
                    == Some(source.id.as_str()))
    })
}

fn forwarded_headers(
    incoming: &[(String, String)],
    credential: &GatewayCredential,
) -> Result<HeaderMap, (u16, String)> {
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
    let auth = HeaderValue::from_str(&credential.authorization_header()).map_err(|_| {
        (
            503,
            "invalid jellyfin gateway credential header".to_string(),
        )
    })?;
    out.insert(reqwest::header::AUTHORIZATION, auth);
    Ok(out)
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
            | "x-emby-authorization"
            | "x-emby-token"
            | "x-mediabrowser-token"
    )
}

fn parse_gateway_credential(body: &str) -> Option<GatewayCredential> {
    let body = body.trim();
    if body.is_empty() {
        return None;
    }
    if body.starts_with('{') {
        #[derive(serde::Deserialize)]
        struct JsonCredential {
            access_token: Option<String>,
            token: Option<String>,
            api_key: Option<String>,
            user_id: Option<String>,
        }
        let parsed: JsonCredential = serde_json::from_str(body).ok()?;
        let access_token = parsed
            .access_token
            .or(parsed.token)
            .or(parsed.api_key)?
            .trim()
            .to_string();
        if access_token.is_empty() {
            return None;
        }
        return Some(GatewayCredential {
            access_token,
            user_id: parsed.user_id.filter(|s| !s.trim().is_empty()),
        });
    }
    let mut access_token = None;
    let mut user_id = None;
    for line in body.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_matches('"').to_string();
        match key {
            "JELLYFIN_ACCESS_TOKEN" | "JELLYFIN_TOKEN" | "JELLYFIN_API_KEY" => {
                access_token = Some(value);
            }
            "JELLYFIN_USER_ID" => user_id = Some(value),
            _ => {}
        }
    }
    let access_token = access_token?.trim().to_string();
    if access_token.is_empty() {
        return None;
    }
    Some(GatewayCredential {
        access_token,
        user_id: user_id.filter(|s| !s.trim().is_empty()),
    })
}

fn sanitize_header_value(value: &str) -> String {
    value
        .chars()
        .filter(|c| !matches!(c, '"' | '\\' | '\r' | '\n'))
        .collect()
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

const fn is_safe_source_id_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.')
}

const fn is_safe_gateway_user_id_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
}

#[allow(dead_code)]
fn has_local_gateway_source(workgroup_root: &Path, aliases: &[String]) -> bool {
    read_jellyfin_gateway_sources_from_plane(workgroup_root)
        .iter()
        .any(|source| source_matches_gateway_alias(source, aliases))
}

#[cfg(test)]
mod tests {
    use crate::mesh_media::JELLYFIN_GATEWAY_PROXY_PORT;

    use super::*;
    use crate::mesh_media::{GatewayHealth, JellyfinGatewayRegistration};
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
        let head_end = loop {
            if let Some(pos) = find_header_end(&buf) {
                break pos;
            }
            let mut chunk = [0u8; 1024];
            let n = stream.read(&mut chunk).await.expect("read request");
            assert_ne!(n, 0, "request ended before header terminator");
            buf.extend_from_slice(&chunk[..n]);
        };
        let head = String::from_utf8_lossy(&buf[..head_end]);
        let content_length = head
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("Content-Length"))
            .and_then(|(_, value)| value.trim().parse::<usize>().ok())
            .unwrap_or(0);
        let body_end = head_end + 4 + content_length;
        while buf.len() < body_end {
            let mut chunk = [0u8; 1024];
            let n = stream.read(&mut chunk).await.expect("read request body");
            assert_ne!(n, 0, "request ended before declared body");
            buf.extend_from_slice(&chunk[..n]);
        }
        buf.truncate(body_end);
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
    ) -> JellyfinGatewaySource {
        let reg = JellyfinGatewayRegistration::new(
            gateway_node,
            upstream_url,
            "media/jellyfin/shared-readonly",
            health,
            true,
        )
        .unwrap();
        mesh_media::source_from_jellyfin_gateway(&reg).unwrap()
    }

    #[test]
    fn worker_name_is_media_jellyfin_proxy() {
        let worker = JellyfinGatewayProxyWorker::new(
            "peer:seat-15".to_string(),
            "seat-15".to_string(),
            PathBuf::from("/tmp/no-plane"),
        )
        .with_bind_addr("127.0.0.1:0".parse().unwrap());
        assert_eq!(worker.name(), "media_jellyfin_proxy");
        assert_eq!(worker.bind_addr.port(), 0);
        assert_eq!(JELLYFIN_GATEWAY_PROXY_PORT, 8097);
    }

    #[test]
    fn route_rewrites_prefixed_path_and_query_to_upstream() {
        let src = source(
            "seat-15",
            "http://192.168.1.60:8096/base",
            GatewayHealth::Healthy,
        );
        let target = format!("/mde/jellyfin/{}/Users/u/Items?Limit=20", src.id);
        let route = resolve_gateway_route(&target, &[src]).unwrap();
        assert_eq!(
            route.upstream_url,
            "http://192.168.1.60:8096/base/Users/u/Items?Limit=20"
        );
        assert_eq!(route.credential_ref, "media/jellyfin/shared-readonly");
    }

    #[test]
    fn route_strips_client_auth_query_params_before_upstream() {
        let src = source(
            "seat-15",
            "http://192.168.1.60:8096/base",
            GatewayHealth::Healthy,
        );
        let target = format!(
            "/mde/jellyfin/{}/Videos/movie/stream?static=true&api_key=LOCAL&X-Emby-Token=LOCAL2&mediaSourceId=s1",
            src.id
        );
        let route = resolve_gateway_route(&target, &[src]).unwrap();
        assert_eq!(
            route.upstream_url,
            "http://192.168.1.60:8096/base/Videos/movie/stream?static=true&mediaSourceId=s1"
        );
    }

    #[test]
    fn route_rejects_unknown_or_degraded_sources() {
        let degraded = source(
            "seat-15",
            "http://jellyfin.lan:8096",
            GatewayHealth::Degraded,
        );
        let target = format!("/mde/jellyfin/{}/Users/u/Items", degraded.id);
        assert_eq!(
            resolve_gateway_route(&target, &[degraded]).unwrap_err().0,
            503
        );
        assert_eq!(
            resolve_gateway_route("/mde/jellyfin/missing/Users/u/Items", &[])
                .unwrap_err()
                .0,
            404
        );
    }

    #[test]
    fn bus_replacement_revokes_inflight_route_until_exact_corrected_forward_authority() {
        let initial = source(
            "seat-15",
            "http://jellyfin.lan:8096",
            GatewayHealth::Healthy,
        );
        let target = format!("/mde/jellyfin/{}/Users/u/Items", initial.id);
        let resolved = resolve_gateway_route(&target, std::slice::from_ref(&initial)).unwrap();

        let mut rotated_credential = initial.clone();
        rotated_credential.credential_ref = "media/jellyfin/rotated-readonly".to_string();
        assert!(!route_matches_current_authority(
            &target,
            &resolved,
            &[rotated_credential]
        ));

        let mut degraded = initial.clone();
        degraded.health = GatewayHealth::Degraded;
        assert!(!route_matches_current_authority(
            &target,
            &resolved,
            &[degraded]
        ));
        assert!(!route_matches_current_authority(&target, &resolved, &[]));

        assert!(route_matches_current_authority(
            &target,
            &resolved,
            &[initial]
        ));
    }

    #[test]
    fn route_rejects_path_escape_and_absolute_url_suffixes() {
        assert!(join_upstream_url("http://jellyfin.lan:8096", "/../admin").is_none());
        assert!(join_upstream_url("http://jellyfin.lan:8096", "/http://x").is_none());
    }

    #[test]
    fn aliases_identify_local_gateway_source() {
        let src = source(
            "Seat-15",
            "http://jellyfin.lan:8096",
            GatewayHealth::Healthy,
        );
        assert!(source_matches_gateway_alias(
            &src,
            &["seat-15".to_string(), "peer:seat-15".to_string()]
        ));
        assert!(!source_matches_gateway_alias(&src, &["eagle".to_string()]));
    }

    #[test]
    fn credential_accepts_json_and_env_shapes() {
        let json = parse_gateway_credential(
            r#"{"access_token":"TOKEN-1","user_id":"user-1","ignored":"x"}"#,
        )
        .unwrap();
        assert_eq!(json.access_token, "TOKEN-1");
        assert_eq!(json.user_id.as_deref(), Some("user-1"));

        let env = parse_gateway_credential(
            "JELLYFIN_ACCESS_TOKEN=\"TOKEN-2\"\nJELLYFIN_USER_ID=user-2\n",
        )
        .unwrap();
        assert_eq!(env.access_token, "TOKEN-2");
        assert_eq!(env.user_id.as_deref(), Some("user-2"));
        assert!(parse_gateway_credential("{}").is_none());
    }

    #[test]
    fn gateway_user_sentinel_rewrites_only_from_sealed_credential_material() {
        let upstream = format!(
            "http://jellyfin.lan:8096/Users/{}/Items?userId={}&Limit=20",
            JELLYFIN_GATEWAY_USER_SENTINEL, JELLYFIN_GATEWAY_USER_SENTINEL
        );
        assert_eq!(
            materialize_gateway_user_text(&upstream, Some("user-1")).unwrap(),
            "http://jellyfin.lan:8096/Users/user-1/Items?userId=user-1&Limit=20"
        );

        let body = format!(
            r#"{{"UserId":"{}","ItemId":"movie-1"}}"#,
            JELLYFIN_GATEWAY_USER_SENTINEL
        )
        .into_bytes();
        assert_eq!(
            String::from_utf8(materialize_gateway_user_body(body, Some("user-1")).unwrap())
                .unwrap(),
            r#"{"UserId":"user-1","ItemId":"movie-1"}"#
        );

        let missing = materialize_gateway_user_text(&upstream, None).unwrap_err();
        assert_eq!(missing.0, 503);
        assert!(missing.1.contains("missing user_id"));
        let invalid = materialize_gateway_user_text(&upstream, Some("bad/user")).unwrap_err();
        assert_eq!(invalid.0, 503);
        assert!(invalid.1.contains("invalid"));
    }

    #[test]
    fn forwarded_headers_strip_client_auth_and_hop_by_hop_then_inject_gateway_auth() {
        let credential = GatewayCredential {
            access_token: "TOKEN\"bad\r\n".to_string(),
            user_id: None,
        };
        let headers = forwarded_headers(
            &[
                ("Host".to_string(), "gateway.mesh".to_string()),
                ("Connection".to_string(), "keep-alive".to_string()),
                ("Authorization".to_string(), "client-token".to_string()),
                ("Accept".to_string(), "application/json".to_string()),
                ("Content-Type".to_string(), "application/json".to_string()),
            ],
            &credential,
        )
        .unwrap();
        assert!(headers.get("host").is_none());
        assert_eq!(
            headers.get("accept").unwrap().to_str().unwrap(),
            "application/json"
        );
        let auth = headers.get("authorization").unwrap().to_str().unwrap();
        assert!(auth.contains("Token=\"TOKENbad\""));
        assert!(!auth.contains("client-token"));
    }

    #[tokio::test]
    async fn proxy_preserves_range_stream_status_headers_and_body() {
        let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream.local_addr().unwrap();
        let upstream_task = tokio::spawn(async move {
            let (mut stream, _) = upstream.accept().await.unwrap();
            let request = read_http_request_bytes(&mut stream).await;
            let request_text = String::from_utf8_lossy(&request);
            assert!(
                request_text.starts_with(
                    "GET /Videos/movie-1/stream?static=true&mediaSourceId=src-1 HTTP/1.1"
                ),
                "{request_text}"
            );
            let request_lower = request_text.to_ascii_lowercase();
            assert!(request_lower.contains("\r\nrange: bytes=4-9\r\n"));
            assert!(request_lower.contains("\r\nauthorization: mediabrowser "));
            assert!(request_text.contains("Token=\"SERVER-TOKEN\""));
            assert!(!request_text.contains("CLIENT-TOKEN"));
            stream
                .write_all(
                    b"HTTP/1.1 206 Partial Content\r\n\
                      Content-Type: video/mp4\r\n\
                      Content-Length: 6\r\n\
                      Content-Range: bytes 4-9/20\r\n\
                      Accept-Ranges: bytes\r\n\
                      ETag: \"media-etag\"\r\n\
                      Connection: close\r\n\
                      \r\n\
                      EFGHIJ",
                )
                .await
                .unwrap();
        });

        let source = source(
            "seat-15",
            &format!("http://{upstream_addr}"),
            GatewayHealth::Healthy,
        );
        let target = format!(
            "/mde/jellyfin/{}/Videos/movie-1/stream?static=true&api_key=CLIENT-TOKEN&mediaSourceId=src-1",
            source.id
        );
        let route = resolve_gateway_route(&target, &[source]).unwrap();
        let response = proxy_response_for(
            ParsedRequest {
                method: "GET".to_string(),
                target,
                headers: vec![
                    ("Host".to_string(), "seat-15.mesh:8097".to_string()),
                    ("Authorization".to_string(), "client-token".to_string()),
                    ("Range".to_string(), "bytes=4-9".to_string()),
                    ("Accept".to_string(), "video/*".to_string()),
                ],
                body: Vec::new(),
            },
            route,
            GatewayCredential {
                access_token: "SERVER-TOKEN".to_string(),
                user_id: Some("gateway-user".to_string()),
            },
        )
        .await;
        upstream_task.await.unwrap();

        let head_end = find_header_end(&response).expect("response head");
        let head = String::from_utf8_lossy(&response[..head_end]);
        assert!(head.starts_with("HTTP/1.1 206 Partial Content"), "{head}");
        assert_eq!(header_value(&head, "Content-Type"), Some("video/mp4"));
        assert_eq!(header_value(&head, "Content-Length"), Some("6"));
        assert_eq!(header_value(&head, "Content-Range"), Some("bytes 4-9/20"));
        assert_eq!(header_value(&head, "Accept-Ranges"), Some("bytes"));
        assert_eq!(header_value(&head, "ETag"), Some("\"media-etag\""));
        assert_eq!(&response[head_end + 4..], b"EFGHIJ");
    }

    #[tokio::test]
    async fn proxy_forwards_playback_progress_body_and_status_without_client_auth() {
        let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream.local_addr().unwrap();
        let upstream_task = tokio::spawn(async move {
            let (mut stream, _) = upstream.accept().await.unwrap();
            let request = read_http_request_bytes(&mut stream).await;
            let head_end = find_header_end(&request).expect("request head");
            let request_head = String::from_utf8_lossy(&request[..head_end]);
            assert!(
                request_head.starts_with("POST /Sessions/Playing/Progress HTTP/1.1"),
                "{request_head}"
            );
            let request_lower = request_head.to_ascii_lowercase();
            assert!(request_lower.contains("\r\ncontent-type: application/json\r\n"));
            assert!(request_lower.contains("\r\nauthorization: mediabrowser "));
            assert!(request_head.contains("Token=\"SERVER-TOKEN\""));
            assert!(!request_head.contains("CLIENT-TOKEN"));
            let body: serde_json::Value =
                serde_json::from_slice(&request[head_end + 4..]).expect("progress body");
            assert_eq!(body["UserId"], "gateway-user");
            assert_eq!(body["ItemId"], "movie-1");
            assert_eq!(body["PositionTicks"], 420_000_000_i64);
            stream
                .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
        });

        let source = source(
            "seat-15",
            &format!("http://{upstream_addr}"),
            GatewayHealth::Healthy,
        );
        let target = format!("/mde/jellyfin/{}/Sessions/Playing/Progress", source.id);
        let route = resolve_gateway_route(&target, &[source]).unwrap();
        let body = format!(
            r#"{{"UserId":"{}","ItemId":"movie-1","PositionTicks":420000000}}"#,
            JELLYFIN_GATEWAY_USER_SENTINEL
        )
        .into_bytes();
        let response = proxy_response_for(
            ParsedRequest {
                method: "POST".to_string(),
                target,
                headers: vec![
                    ("Authorization".to_string(), "CLIENT-TOKEN".to_string()),
                    ("Content-Type".to_string(), "application/json".to_string()),
                    ("Content-Length".to_string(), body.len().to_string()),
                ],
                body,
            },
            route,
            GatewayCredential {
                access_token: "SERVER-TOKEN".to_string(),
                user_id: Some("gateway-user".to_string()),
            },
        )
        .await;
        upstream_task.await.unwrap();

        let head_end = find_header_end(&response).expect("response head");
        let head = String::from_utf8_lossy(&response[..head_end]);
        assert!(head.starts_with("HTTP/1.1 204 No Content"), "{head}");
        assert_eq!(&response[head_end + 4..], b"");
    }

    #[test]
    fn static_credential_provider_is_a_test_seam() {
        let mut provider = StaticCredentials::default();
        provider.values.insert(
            "media/jellyfin/shared-readonly".to_string(),
            r#"{"access_token":"T"}"#.to_string(),
        );
        assert_eq!(
            provider
                .get_secret("media/jellyfin/shared-readonly")
                .unwrap()
                .as_deref(),
            Some(r#"{"access_token":"T"}"#)
        );
    }
}
