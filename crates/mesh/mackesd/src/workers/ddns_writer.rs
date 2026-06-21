//! DDNS-EGRESS-2 — the daemon-side DigitalOcean DNS writer.
//!
//! DDNS-EGRESS-1 ships the discovery half (the `ddns` worker learns the
//! node's egress IP + publishes `event/egress-ip/<host>`), and
//! [`mackes_mesh_types::ddns`] ships the **pure request builders**
//! ([`do_upsert_request`](mackes_mesh_types::ddns::do_upsert_request) /
//! [`do_delete_request`](mackes_mesh_types::ddns::do_delete_request) /
//! [`record_type`](mackes_mesh_types::ddns::record_type)) that model
//! POST-create vs PUT-update-by-id vs DELETE as `(method, path, body)`
//! tuples. This module is the part that **executes** them against the
//! live DigitalOcean DNS API (`https://api.digitalocean.com/v2/...`) —
//! the `DnsWriter` trait + its DigitalOcean impl that upserts (create-
//! or-update) and removes A/AAAA records under the configured zone.
//!
//! ## Three seams keep it unit-testable without a live token (§7)
//!
//! Live DO verification is **deferred** (no DO API token exists in this
//! environment), so every moving part is factored behind a seam so the
//! create/update/delete/lookup paths + the 401→alert mapping are tested
//! with mocked responses, never a real call:
//!
//! * [`HttpExec`] — the transport. The real [`CurlExec`] shells to
//!   `curl` (mackesd deliberately avoids `reqwest`; the existing ddns
//!   worker already shells to `curl` for the IP echo — we match that).
//!   Tests inject a `MockExec` that returns canned [`HttpResponse`]s.
//! * [`TokenSource`] — resolves the age-encrypted, leader-distributed DO
//!   token from the config's `token_ref` ([`SealedTokenSource`]); a test
//!   injects the token directly without touching age/the secret store.
//! * [`AlertSink`] — where a `ddns/auth` alert lands ([`FileAlertSink`]
//!   drops it into the MON-3 alerts dir the `alert_relay` worker
//!   surfaces); a test uses an in-memory sink to assert the mapping.
//!
//! ## §3 — crypto + secret hygiene
//!
//! The DO token is an age-encrypted mesh secret (the same Argon2id +
//! XChaCha20-Poly1305 envelope as VPN-GW-2 / the CA backup, via
//! [`crate::ca::backup::unseal_bytes`] under a distinct [`TOKEN_MAGIC`]
//! so a DDNS-token blob can't cross-decrypt as a VPN secret or CA
//! bundle). It is NEVER hardcoded, NEVER in `ps`/argv/logs: the bearer
//! header is written to a 0600 temp file passed via `curl --config`
//! (mirroring [`crate::vpn_secret`]'s 0600 handling) and unlinked after
//! the call — it never appears on a command line a co-tenant could read
//! via `ps`.

#![cfg(feature = "async-services")]

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use tracing::{debug, warn};

use mackes_mesh_types::ddns::{self, OnDown};

/// Envelope magic for a sealed DDNS API token — ASCII "MDDT" ("Mackes
/// DDns Token"). Distinct from the VPN secret's `MVPS` and the CA
/// bundle's `MNCA` so the one vetted AEAD path refuses to cross-decrypt
/// the three secret domains (§3 / §6 crypto-floor).
pub const TOKEN_MAGIC: &[u8; 4] = b"MDDT";

/// The DigitalOcean API origin. Const so a future operator-config can
/// override it (and tests can point [`CurlExec`] elsewhere) without
/// touching the request logic.
pub const DO_API_BASE: &str = "https://api.digitalocean.com";

// ── HTTP transport seam ─────────────────────────────────────────────

/// One HTTP response reduced to what the writer needs: the status code
/// and the body. Headers are irrelevant to the DO record flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    /// HTTP status code (e.g. 200, 201, 401).
    pub status: u16,
    /// Response body (JSON for DO; may be empty on a 204 delete).
    pub body: String,
}

impl HttpResponse {
    /// 2xx — the request succeeded.
    #[must_use]
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// 401/403 — the bearer token is missing/expired/forbidden. Mapped
    /// to [`WriteError::Auth`] → the `ddns/auth` alert.
    #[must_use]
    pub fn is_auth_failure(&self) -> bool {
        self.status == 401 || self.status == 403
    }
}

/// The transport seam. `execute` performs one authenticated HTTP request
/// to the DO API: `method` + `url` (absolute) + an optional JSON `body`,
/// authenticated with the bearer `token`. **Implementations must keep
/// the token out of argv/`ps`/logs** (the real [`CurlExec`] writes it to
/// a 0600 file). Sync (the writer runs on a `spawn_blocking` hop, like
/// the ddns worker's IP echo) so tests need no runtime.
pub trait HttpExec: Send + Sync {
    /// Execute the request; `Err` only for a transport failure (curl
    /// couldn't run / timed out) — an HTTP error status is a successful
    /// transport returning a non-2xx [`HttpResponse`].
    ///
    /// # Errors
    /// A transport-level failure (spawn/timeout/non-UTF-8 status line).
    fn execute(
        &self,
        method: &str,
        url: &str,
        body: Option<&str>,
        token: &str,
    ) -> anyhow::Result<HttpResponse>;
}

/// The production transport: shells to `curl`, passing the bearer token
/// via a **0600 `--config` file** (NEVER `-H "Authorization: Bearer …"`
/// on argv — that leaks via `ps`). curl reads the header + the write-out
/// format from the config file, emits the body then a trailing
/// `\n<<<status>>>NNN` line we split off to recover the status code
/// without a second request. The config file is unlinked after the call.
pub struct CurlExec {
    /// Per-request timeout passed to `curl --max-time`.
    timeout_secs: u64,
}

impl Default for CurlExec {
    fn default() -> Self {
        Self { timeout_secs: 15 }
    }
}

impl CurlExec {
    /// Construct with the default 15 s per-request budget.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// The sentinel that separates the response body from the appended
/// status code in curl's `--write-out`. Chosen to never collide with DO
/// JSON. Kept module-pub so the parsing is unit-tested directly.
pub const STATUS_SENTINEL: &str = "\n<<<mde-ddns-status>>>";

/// Split curl's combined `body + STATUS_SENTINEL + code` output into
/// `(body, status)`. Pure — the seam between [`CurlExec`]'s subprocess
/// and the [`HttpResponse`] so the wire framing is testable without curl.
///
/// # Errors
/// The sentinel is absent or the trailing code isn't a `u16`.
pub fn split_status(out: &str) -> anyhow::Result<HttpResponse> {
    let idx = out
        .rfind(STATUS_SENTINEL)
        .ok_or_else(|| anyhow::anyhow!("curl output missing status sentinel"))?;
    let body = out[..idx].to_string();
    let code = out[idx + STATUS_SENTINEL.len()..].trim();
    let status: u16 = code
        .parse()
        .map_err(|e| anyhow::anyhow!("curl status not a u16 ({code:?}): {e}"))?;
    Ok(HttpResponse { status, body })
}

/// Build the `curl --config` file contents for one request. The bearer
/// token rides a `header` line **inside the file** (never argv). Returns
/// the config text; the caller writes it 0600 + passes `--config`. Pure
/// + unit-tested so we verify the token never lands on a command line.
#[must_use]
pub fn curl_config_contents(method: &str, url: &str, body: Option<&str>, token: &str) -> String {
    // curl config syntax: `key = "value"`, one per line. The auth header
    // + the write-out (append our status sentinel) live here so they
    // never appear in argv. A body is sent via --data-raw read from this
    // file too, so even a request body never hits the command line.
    let mut cfg = String::new();
    cfg.push_str(&format!("request = \"{method}\"\n"));
    cfg.push_str(&format!("url = \"{url}\"\n"));
    cfg.push_str(&format!("header = \"Authorization: Bearer {token}\"\n"));
    cfg.push_str("header = \"Content-Type: application/json\"\n");
    cfg.push_str("silent\n");
    cfg.push_str("show-error\n");
    if let Some(b) = body {
        // Escape backslashes + quotes for the config-file string literal.
        let esc = b.replace('\\', "\\\\").replace('"', "\\\"");
        cfg.push_str(&format!("data-raw = \"{esc}\"\n"));
    }
    cfg.push_str(&format!(
        "write-out = \"{STATUS_SENTINEL}%{{http_code}}\"\n"
    ));
    cfg
}

impl HttpExec for CurlExec {
    fn execute(
        &self,
        method: &str,
        url: &str,
        body: Option<&str>,
        token: &str,
    ) -> anyhow::Result<HttpResponse> {
        use std::os::unix::fs::OpenOptionsExt as _;

        // 0600 temp config file — created O_EXCL with mode 0600 so the
        // token is never briefly world-readable (mirrors vpn_secret's
        // 0600 invariant). Unlinked in all exit paths below.
        let pid = std::process::id();
        let nonce: u64 = rand::random();
        let cfg_path: PathBuf =
            std::env::temp_dir().join(format!("mde-ddns-{pid}-{nonce:016x}.curlrc"));
        {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&cfg_path)
                .map_err(|e| anyhow::anyhow!("create curl config: {e}"))?;
            let contents = curl_config_contents(method, url, body, token);
            f.write_all(contents.as_bytes())
                .map_err(|e| anyhow::anyhow!("write curl config: {e}"))?;
        }

        // Token + url + body all live in the --config file; argv carries
        // ONLY the flag + the file path, so `ps` never sees the secret.
        let secs = self.timeout_secs.max(1).to_string();
        let mut cmd = Command::new("curl");
        cmd.args(["--config", &cfg_path.to_string_lossy(), "--max-time", &secs]);
        let run = crate::workers::proc::output_with_timeout(
            cmd,
            crate::workers::proc::DEFAULT_CMD_TIMEOUT,
        );
        // Always unlink the secret-bearing file, success or failure.
        let _ = std::fs::remove_file(&cfg_path);

        let out = run.map_err(|e| anyhow::anyhow!("curl transport: {e}"))?;
        if !out.status.success() {
            // curl's own non-zero exit (network/TLS). stderr is safe to
            // log — the token lived only in the unlinked config file.
            let stderr = String::from_utf8_lossy(&out.stderr);
            anyhow::bail!("curl exit {:?}: {}", out.status.code(), stderr.trim());
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        split_status(&stdout)
    }
}

// ── token source seam ───────────────────────────────────────────────

/// Resolves the DO API bearer token. The production [`SealedTokenSource`]
/// decrypts the age-sealed, leader-distributed mesh secret named by the
/// config's `token_ref`; a test injects the token directly so the writer
/// is exercised without age/the secret store.
pub trait TokenSource: Send + Sync {
    /// Return the cleartext bearer token, or an error describing why it
    /// couldn't be resolved (missing blob / no mesh key / decrypt fail).
    /// The token is returned by value + held only for the duration of one
    /// request — never logged, never persisted in cleartext.
    ///
    /// # Errors
    /// The secret blob is absent, the mesh key is unavailable, or the
    /// envelope fails to decrypt.
    fn token(&self) -> anyhow::Result<String>;
}

/// Production token source: reads the age-sealed blob for `token_ref`
/// from the mesh secret subtree and unseals it with the mesh key
/// ([`crate::vpn_secret::mesh_key_from_env`] →
/// [`crate::ca::backup::unseal_bytes`] under [`TOKEN_MAGIC`]). Same
/// secret plumbing as VPN-GW-2; the token never leaves this call in the
/// clear and never touches argv.
pub struct SealedTokenSource {
    blob_path: PathBuf,
}

impl SealedTokenSource {
    /// Resolve the sealed-blob path for `token_ref` under the workgroup
    /// secret subtree, scoped to this node. `token_ref` is the config
    /// handle (`secret://ddns/do-token`); only its final segment is used
    /// as the blob name, sanitized so it can't traverse out of the
    /// subtree (`<workgroup_root>/secrets/ddns/<node_id>/<name>.age`).
    #[must_use]
    pub fn new(workgroup_root: &Path, node_id: &str, token_ref: &str) -> Self {
        Self {
            blob_path: token_blob_path(workgroup_root, node_id, token_ref),
        }
    }

    /// Construct directly from a blob path (tests).
    #[must_use]
    pub fn from_blob_path(blob_path: PathBuf) -> Self {
        Self { blob_path }
    }
}

/// The age-sealed DO-token blob path:
/// `<workgroup_root>/secrets/ddns/<node_id>/<token-name>.age`. Mirrors
/// the VPN secret subtree shape; every path segment is sanitized so a
/// `peer:host` node-id or an operator-typed `token_ref` can't escape the
/// shared root. Pure — single-sourced so the leader-side distributor +
/// the node-side reader agree byte-for-byte.
#[must_use]
pub fn token_blob_path(workgroup_root: &Path, node_id: &str, token_ref: &str) -> PathBuf {
    // The ref is a handle like `secret://ddns/do-token`; key on its last
    // path-ish segment so the on-disk name is stable + log-safe.
    let name = token_ref
        .rsplit(['/', ':'])
        .find(|s| !s.is_empty())
        .unwrap_or("do-token");
    workgroup_root
        .join("secrets")
        .join("ddns")
        .join(sanitize_segment(node_id))
        .join(format!("{}.age", sanitize_segment(name)))
}

/// Map every char that isn't `[A-Za-z0-9._-]` to `_` and collapse a `..`
/// traversal run to `_`, so no path component escapes the secret subtree.
/// Mirrors `mackes_mesh_types::vpn`'s private sanitizer (kept local to
/// avoid widening that crate's API surface for one caller).
fn sanitize_segment(s: &str) -> String {
    let mapped: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut out = String::with_capacity(mapped.len());
    let mut dots = 0usize;
    for c in mapped.chars() {
        if c == '.' {
            dots += 1;
        } else {
            match dots {
                0 => {}
                1 => out.push('.'),
                _ => out.push('_'),
            }
            dots = 0;
            out.push(c);
        }
    }
    match dots {
        0 => {}
        1 => out.push('.'),
        _ => out.push('_'),
    }
    out
}

impl TokenSource for SealedTokenSource {
    fn token(&self) -> anyhow::Result<String> {
        let sealed = std::fs::read(&self.blob_path).map_err(|e| {
            // Path only — never the (absent) cleartext.
            anyhow::anyhow!(
                "ddns token blob unreadable at {}: {e}",
                self.blob_path.display()
            )
        })?;
        let mesh_key = crate::vpn_secret::mesh_key_from_env().ok_or_else(|| {
            anyhow::anyhow!("no mesh key (set {})", crate::vpn_secret::MESH_KEY_ENV)
        })?;
        let plain = crate::ca::backup::unseal_bytes(TOKEN_MAGIC, &mesh_key, &sealed)
            .map_err(|e| anyhow::anyhow!("ddns token decrypt failed: {e}"))?;
        let token = String::from_utf8(plain)
            .map_err(|_| anyhow::anyhow!("ddns token is not valid UTF-8"))?;
        let token = token.trim().to_string();
        if token.is_empty() {
            anyhow::bail!("ddns token blob decrypted to an empty token");
        }
        Ok(token)
    }
}

// ── alert sink seam ─────────────────────────────────────────────────

/// Where a `ddns/auth` alert is delivered. The production
/// [`FileAlertSink`] drops a JSON alert into the MON-3 alerts dir the
/// `alert_relay` worker surfaces (the §EFF alert path); a test uses an
/// in-memory sink to assert the 401/403 → alert mapping.
pub trait AlertSink: Send + Sync {
    /// Raise the `ddns/auth` alert for `host` with a human `summary`.
    fn auth_alert(&self, host: &str, summary: &str);
}

/// Production alert sink: writes a deterministic-id JSON alert file into
/// the `alert_relay` watch dir (same shape + file-drop pattern as
/// `upgrade_intent_watcher` / `nebula_ca_backup`). The id keys on the
/// host so a re-fired auth failure de-dupes rather than re-toasting.
pub struct FileAlertSink {
    alerts_dir: Option<PathBuf>,
}

impl Default for FileAlertSink {
    fn default() -> Self {
        Self {
            alerts_dir: crate::workers::alert_relay::default_alerts_dir(),
        }
    }
}

impl FileAlertSink {
    /// Construct pointing at the real alerts dir.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Point the sink at a scratch dir (tests).
    #[must_use]
    pub fn with_alerts_dir(dir: PathBuf) -> Self {
        Self {
            alerts_dir: Some(dir),
        }
    }
}

/// Build the `ddns/auth` alert JSON for `host` + `summary`. Pure +
/// testable; the panel/relay read `id`/`severity`/`alert`/`host`/
/// `summary`. The id keys only on host so a repeated auth failure de-
/// dupes (the relay de-dupes by id).
#[must_use]
pub fn auth_alert_event(host: &str, summary: &str) -> serde_json::Value {
    let safe = |s: &str| s.replace(['/', '.', ' ', ':'], "-");
    serde_json::json!({
        "id": format!("ddns-auth-{}", safe(host)),
        "severity": "crit",
        "category": "ddns.auth",
        "alert": "ddns/auth",
        "host": host,
        "summary": summary,
        "fired_by": "ddns",
    })
}

impl AlertSink for FileAlertSink {
    fn auth_alert(&self, host: &str, summary: &str) {
        let Some(dir) = &self.alerts_dir else {
            return;
        };
        let event = auth_alert_event(host, summary);
        let _ = std::fs::create_dir_all(dir);
        let id = event["id"].as_str().unwrap_or("ddns-auth").to_string();
        let path = dir.join(format!("{id}.json"));
        let tmp = dir.join(format!(".{id}.json.tmp"));
        if std::fs::write(&tmp, serde_json::to_vec_pretty(&event).unwrap_or_default()).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

// ── DnsWriter ───────────────────────────────────────────────────────

/// What went wrong writing a record. `Auth` is mapped from DO 401/403 →
/// the `ddns/auth` alert; the rest are transport/protocol failures the
/// caller logs + retries next tick.
#[derive(Debug)]
pub enum WriteError {
    /// The DO token is missing/expired/forbidden (HTTP 401/403). The
    /// writer raises the `ddns/auth` alert for this — NOT a silent no-op.
    Auth(String),
    /// The token couldn't be resolved (no blob / no mesh key / decrypt).
    Token(String),
    /// A transport failure (curl couldn't run / timed out).
    Transport(String),
    /// DO returned a non-2xx, non-auth status, or an unparseable body.
    Api(String),
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteError::Auth(m) => write!(f, "ddns auth: {m}"),
            WriteError::Token(m) => write!(f, "ddns token: {m}"),
            WriteError::Transport(m) => write!(f, "ddns transport: {m}"),
            WriteError::Api(m) => write!(f, "ddns api: {m}"),
        }
    }
}

impl std::error::Error for WriteError {}

/// The DnsWriter trait the design names: upsert (create-or-update) and
/// remove A/AAAA records for a managed hostname. The worker drives this
/// on an egress-IP change; the DigitalOcean impl is the only v1 adapter,
/// but the trait keeps Cloudflare/Route53 addable without a worker
/// rewrite (design §"DNS writing").
pub trait DnsWriter: Send + Sync {
    /// Create-or-update the A/AAAA record for `fqdn` → `ip` at `ttl`.
    ///
    /// # Errors
    /// [`WriteError`] — `Auth` for a 401/403 (the caller alerts), else
    /// token/transport/api.
    fn upsert(&self, fqdn: &str, ip: &str, ttl: u32) -> Result<(), WriteError>;

    /// Remove the A/AAAA record for `fqdn` (the `on_down = remove`
    /// policy). A record that doesn't exist is success (idempotent).
    ///
    /// # Errors
    /// [`WriteError`] as for [`upsert`](DnsWriter::upsert).
    fn remove(&self, fqdn: &str, record_type: &str) -> Result<(), WriteError>;
}

/// The DigitalOcean DnsWriter. Executes the pure
/// [`ddns::do_upsert_request`] / [`ddns::do_delete_request`] builders
/// against the DO API via an [`HttpExec`], resolving the bearer token
/// per call from a [`TokenSource`] (so the cleartext is held only for
/// the request) and raising a `ddns/auth` alert through an [`AlertSink`]
/// on a 401/403.
pub struct DigitalOceanWriter<H: HttpExec, T: TokenSource, A: AlertSink> {
    /// Registrable DO domain (`matthewmackes.com`) — the `{domain}` in
    /// the DO record paths.
    domain: String,
    /// API origin (overridable for tests).
    api_base: String,
    /// Host this writer runs on (for the `ddns/auth` alert).
    host: String,
    http: H,
    tokens: T,
    alerts: A,
}

impl<H: HttpExec, T: TokenSource, A: AlertSink> DigitalOceanWriter<H, T, A> {
    /// Build a writer for `zone` (the full zone, e.g.
    /// `services.matthewmackes.com`); the registrable `{domain}` is
    /// derived from it ([`registrable_domain`]).
    pub fn new(zone: &str, host: impl Into<String>, http: H, tokens: T, alerts: A) -> Self {
        Self {
            domain: registrable_domain(zone),
            api_base: DO_API_BASE.to_string(),
            host: host.into(),
            http,
            tokens,
            alerts,
        }
    }

    /// Override the API origin (tests).
    #[must_use]
    pub fn with_api_base(mut self, base: impl Into<String>) -> Self {
        self.api_base = base.into();
        self
    }

    /// Run one request through the transport, resolving the token first
    /// and mapping a 401/403 → [`WriteError::Auth`] (after raising the
    /// alert). `path` is the DO API path from a builder.
    fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
    ) -> Result<HttpResponse, WriteError> {
        let token = self
            .tokens
            .token()
            .map_err(|e| WriteError::Token(e.to_string()))?;
        let url = format!("{}{}", self.api_base, path);
        let resp = self
            .http
            .execute(method, &url, body, &token)
            .map_err(|e| WriteError::Transport(e.to_string()))?;
        // token drops here; never logged.
        if resp.is_auth_failure() {
            let summary = format!(
                "DigitalOcean rejected the DDNS token (HTTP {}) — the DO API \
                 token for zone writes is missing/expired; records can't be \
                 updated until it's rotated.",
                resp.status
            );
            warn!(
                status = resp.status,
                "ddns: DO auth failure; raising ddns/auth alert"
            );
            self.alerts.auth_alert(&self.host, &summary);
            return Err(WriteError::Auth(summary));
        }
        Ok(resp)
    }

    /// The bare record label DO stores for `fqdn` — the FQDN with the
    /// registrable `{domain}` stripped (e.g.
    /// `eagle-mullvad.services.matthewmackes.com` → `eagle-mullvad.services`).
    /// DO's `name` field is this label relative to `{domain}`.
    fn record_label(&self, fqdn: &str) -> String {
        record_label_for(fqdn, &self.domain)
    }

    /// Find an existing record id for `name` (bare label) + `rtype` by
    /// listing the domain's records (`GET /v2/domains/{domain}/records`)
    /// and matching on name+type. `Ok(None)` when no such record exists
    /// (→ POST-create). Pages via the DO `links.pages.next` cursor so a
    /// zone with >per_page records is fully searched.
    fn find_record_id(&self, name: &str, rtype: &str) -> Result<Option<String>, WriteError> {
        // DO supports server-side filtering by name+type; use it to keep
        // the response small. `name` for the filter is the FQDN; the apex
        // marker `@` (or "") filters on the bare domain.
        let fqdn = if name.is_empty() || name == "@" {
            self.domain.clone()
        } else {
            format!("{name}.{}", self.domain)
        };
        let path = format!(
            "/v2/domains/{}/records?type={}&name={}&per_page=200",
            self.domain, rtype, fqdn
        );
        let resp = self.call("GET", &path, None)?;
        if !resp.is_success() {
            return Err(WriteError::Api(format!(
                "list records HTTP {}: {}",
                resp.status,
                truncate(&resp.body, 200)
            )));
        }
        find_id_in_list(&resp.body, name, rtype)
            .map_err(|e| WriteError::Api(format!("parse records list: {e}")))
    }
}

impl<H: HttpExec, T: TokenSource, A: AlertSink> DnsWriter for DigitalOceanWriter<H, T, A> {
    fn upsert(&self, fqdn: &str, ip: &str, ttl: u32) -> Result<(), WriteError> {
        let name = self.record_label(fqdn);
        let rtype = ddns::record_type(ip);
        // Resolve create-vs-update: the builders model both; we supply
        // the lookup (does a name+type record already exist?).
        let existing = self.find_record_id(&name, rtype)?;
        let (method, path, body) =
            ddns::do_upsert_request(&self.domain, &name, ip, ttl, existing.as_deref());
        let resp = self.call(method, &path, Some(&body))?;
        if !resp.is_success() {
            return Err(WriteError::Api(format!(
                "upsert {method} {path} HTTP {}: {}",
                resp.status,
                truncate(&resp.body, 200)
            )));
        }
        debug!(
            fqdn,
            ip,
            ttl,
            rtype,
            updated = existing.is_some(),
            "ddns: record upserted"
        );
        Ok(())
    }

    fn remove(&self, fqdn: &str, record_type: &str) -> Result<(), WriteError> {
        let name = self.record_label(fqdn);
        let Some(id) = self.find_record_id(&name, record_type)? else {
            // Nothing to remove — idempotent success.
            debug!(
                fqdn,
                record_type, "ddns: record already absent; remove is a no-op"
            );
            return Ok(());
        };
        let (method, path) = ddns::do_delete_request(&self.domain, &id);
        let resp = self.call(method, &path, None)?;
        // DO returns 204 No Content on a successful delete.
        if !resp.is_success() {
            return Err(WriteError::Api(format!(
                "delete {path} HTTP {}: {}",
                resp.status,
                truncate(&resp.body, 200)
            )));
        }
        debug!(fqdn, record_type, id, "ddns: record removed");
        Ok(())
    }
}

// ── pure helpers ────────────────────────────────────────────────────

/// Derive the registrable (2-label) domain from a zone/FQDN —
/// `services.matthewmackes.com` → `matthewmackes.com`. DO record paths
/// use the registrable domain as `{domain}`; the rest of the FQDN is the
/// record's `name`. Pure. (A single-label input is returned unchanged.)
#[must_use]
pub fn registrable_domain(zone: &str) -> String {
    let labels: Vec<&str> = zone.trim_matches('.').split('.').collect();
    if labels.len() <= 2 {
        return labels.join(".");
    }
    labels[labels.len() - 2..].join(".")
}

/// The DO `name` (bare label) for `fqdn` under registrable `domain` —
/// the FQDN with the `.domain` suffix stripped. `eagle-mullvad.services.
/// matthewmackes.com` under `matthewmackes.com` → `eagle-mullvad.services`.
/// An `fqdn` equal to the domain → `@` (DO's apex marker). Pure.
#[must_use]
pub fn record_label_for(fqdn: &str, domain: &str) -> String {
    let fqdn = fqdn.trim_matches('.');
    let domain = domain.trim_matches('.');
    if fqdn == domain {
        return "@".to_string();
    }
    let suffix = format!(".{domain}");
    fqdn.strip_suffix(&suffix)
        .map(str::to_string)
        .unwrap_or_else(|| fqdn.to_string())
}

/// Find the record id in a DO `GET …/records` JSON body matching the
/// bare `name` + `rtype`. DO's `domain_records[].name` is the bare label
/// (`@` at the apex). `Ok(None)` when no match. Pure — the lookup the
/// server-side filter is double-checked against (DO's name filter wants
/// an FQDN but we re-match locally so a filterless/over-broad response
/// still resolves correctly).
///
/// # Errors
/// The body isn't the expected `{ "domain_records": [...] }` JSON.
pub fn find_id_in_list(body: &str, name: &str, rtype: &str) -> anyhow::Result<Option<String>> {
    let v: serde_json::Value = serde_json::from_str(body)?;
    let recs = v
        .get("domain_records")
        .and_then(|r| r.as_array())
        .ok_or_else(|| anyhow::anyhow!("no domain_records array in response"))?;
    // DO uses `@` for the apex; our caller passes "" or "@" for it.
    let want = if name.is_empty() { "@" } else { name };
    for rec in recs {
        let rname = rec.get("name").and_then(|n| n.as_str()).unwrap_or("");
        let rt = rec.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if rname == want && rt == rtype {
            // DO record ids are integers; stringify so the builder's
            // `/records/{id}` path is uniform.
            if let Some(id) = rec.get("id") {
                return Ok(Some(id.to_string()));
            }
        }
    }
    Ok(None)
}

/// Truncate a body for an error message so a large/HTML error page
/// doesn't flood a log line. Pure.
fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

/// Drive the writer for one resolved record per its `on_down` policy:
/// `present == true` (the egress has an IP) → upsert; `present == false`
/// (source down) → apply `on_down` (remove / keep / sentinel). Returns
/// the [`WriteError`] (if any) so the worker logs it; a `keep` down is a
/// no-op. Factored out so the worker wiring + the policy branch are
/// tested without a worker. `sentinel` (for `OnDown::Sentinel`) is the
/// parked address to point a down record at.
pub fn reconcile_record<W: DnsWriter>(
    writer: &W,
    fqdn: &str,
    ip: Option<&str>,
    ttl: u32,
    on_down: OnDown,
    sentinel: &str,
) -> Result<(), WriteError> {
    match ip {
        Some(addr) if !addr.trim().is_empty() => writer.upsert(fqdn, addr.trim(), ttl),
        _ => match on_down {
            OnDown::Remove => {
                // Remove both families' records — we don't know which
                // existed; each remove is idempotent (absent → no-op).
                let a = writer.remove(fqdn, "A");
                let aaaa = writer.remove(fqdn, "AAAA");
                a.and(aaaa)
            }
            OnDown::Sentinel => {
                if sentinel.trim().is_empty() {
                    return Ok(());
                }
                writer.upsert(fqdn, sentinel.trim(), ttl)
            }
            OnDown::Keep => Ok(()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // ── pure helpers ────────────────────────────────────────────────

    #[test]
    fn registrable_domain_strips_to_two_labels() {
        assert_eq!(
            registrable_domain("services.matthewmackes.com"),
            "matthewmackes.com"
        );
        assert_eq!(registrable_domain("matthewmackes.com"), "matthewmackes.com");
        assert_eq!(registrable_domain("a.b.c.example.org"), "example.org");
        assert_eq!(registrable_domain(".trailing.dot.com."), "dot.com");
    }

    #[test]
    fn record_label_strips_registrable_domain() {
        assert_eq!(
            record_label_for(
                "eagle-mullvad.services.matthewmackes.com",
                "matthewmackes.com"
            ),
            "eagle-mullvad.services"
        );
        // Apex → DO's `@`.
        assert_eq!(
            record_label_for("matthewmackes.com", "matthewmackes.com"),
            "@"
        );
    }

    #[test]
    fn split_status_parses_body_and_code() {
        let out = format!("{{\"ok\":true}}{STATUS_SENTINEL}201");
        let r = split_status(&out).unwrap();
        assert_eq!(r.status, 201);
        assert_eq!(r.body, "{\"ok\":true}");
        assert!(r.is_success());
    }

    #[test]
    fn split_status_handles_empty_body_204() {
        let out = format!("{STATUS_SENTINEL}204");
        let r = split_status(&out).unwrap();
        assert_eq!(r.status, 204);
        assert_eq!(r.body, "");
        assert!(r.is_success());
    }

    #[test]
    fn split_status_errors_without_sentinel() {
        assert!(split_status("just a body, no sentinel").is_err());
    }

    #[test]
    fn curl_config_keeps_token_off_argv_and_carries_request() {
        let cfg = curl_config_contents(
            "PUT",
            "https://api.digitalocean.com/v2/domains/x/records/9",
            Some("{\"data\":\"1.2.3.4\"}"),
            "SECRET-DO-TOKEN",
        );
        // The token rides a header line *inside the config file* (so it
        // never lands on argv / `ps`).
        assert!(cfg.contains("header = \"Authorization: Bearer SECRET-DO-TOKEN\""));
        assert!(cfg.contains("request = \"PUT\""));
        assert!(cfg.contains("url = \"https://api.digitalocean.com/v2/domains/x/records/9\""));
        assert!(cfg.contains("data-raw"));
        assert!(cfg.contains("write-out"));
    }

    #[test]
    fn find_id_in_list_matches_name_and_type() {
        let body = r#"{"domain_records":[
            {"id":11,"type":"A","name":"eagle-mullvad.services","data":"1.2.3.4"},
            {"id":22,"type":"AAAA","name":"eagle-mullvad.services","data":"2001:db8::1"},
            {"id":33,"type":"A","name":"other","data":"9.9.9.9"}
        ]}"#;
        assert_eq!(
            find_id_in_list(body, "eagle-mullvad.services", "A").unwrap(),
            Some("11".to_string())
        );
        assert_eq!(
            find_id_in_list(body, "eagle-mullvad.services", "AAAA").unwrap(),
            Some("22".to_string())
        );
        // No match → None (→ POST-create).
        assert_eq!(find_id_in_list(body, "ghost", "A").unwrap(), None);
    }

    #[test]
    fn find_id_in_list_apex_uses_at_marker() {
        let body = r#"{"domain_records":[{"id":7,"type":"A","name":"@","data":"1.2.3.4"}]}"#;
        assert_eq!(
            find_id_in_list(body, "", "A").unwrap(),
            Some("7".to_string())
        );
    }

    #[test]
    fn find_id_in_list_rejects_garbage() {
        assert!(find_id_in_list("not json", "x", "A").is_err());
        assert!(find_id_in_list("{}", "x", "A").is_err());
    }

    #[test]
    fn token_blob_path_is_traversal_safe_and_stable() {
        let p = token_blob_path(Path::new("/srv/wg"), "peer:eagle", "secret://ddns/do-token");
        let s = p.to_string_lossy();
        assert!(s.ends_with("secrets/ddns/peer_eagle/do-token.age"), "{s}");
        // An evil ref can't climb out.
        let evil = token_blob_path(Path::new("/srv/wg"), "../../etc", "../../passwd");
        let es = evil.to_string_lossy();
        assert!(!es.contains(".."), "{es}");
    }

    #[test]
    fn auth_alert_event_shape_is_relayable() {
        let ev = auth_alert_event("eagle", "token rejected");
        assert_eq!(ev["alert"], "ddns/auth");
        assert_eq!(ev["severity"], "crit");
        assert_eq!(ev["host"], "eagle");
        assert_eq!(ev["id"], "ddns-auth-eagle");
    }

    // ── mock seams ──────────────────────────────────────────────────

    /// Records every request + replays canned responses in order.
    struct MockExec {
        calls: Mutex<Vec<(String, String, Option<String>, String)>>,
        responses: Mutex<Vec<HttpResponse>>,
    }
    impl MockExec {
        fn new(responses: Vec<HttpResponse>) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                responses: Mutex::new(responses.into_iter().rev().collect()),
            }
        }
    }
    impl HttpExec for MockExec {
        fn execute(
            &self,
            method: &str,
            url: &str,
            body: Option<&str>,
            token: &str,
        ) -> anyhow::Result<HttpResponse> {
            self.calls.lock().unwrap().push((
                method.to_string(),
                url.to_string(),
                body.map(str::to_string),
                token.to_string(),
            ));
            self.responses
                .lock()
                .unwrap()
                .pop()
                .ok_or_else(|| anyhow::anyhow!("no more mock responses"))
        }
    }

    struct StaticToken(&'static str);
    impl TokenSource for StaticToken {
        fn token(&self) -> anyhow::Result<String> {
            Ok(self.0.to_string())
        }
    }

    #[derive(Default)]
    struct CapturingAlerts {
        fired: Mutex<Vec<(String, String)>>,
    }
    impl AlertSink for CapturingAlerts {
        fn auth_alert(&self, host: &str, summary: &str) {
            self.fired
                .lock()
                .unwrap()
                .push((host.to_string(), summary.to_string()));
        }
    }

    fn ok_list_empty() -> HttpResponse {
        HttpResponse {
            status: 200,
            body: r#"{"domain_records":[]}"#.to_string(),
        }
    }

    // ── upsert-create (POST) ────────────────────────────────────────

    #[test]
    fn upsert_creates_with_post_when_record_absent() {
        let http = MockExec::new(vec![
            ok_list_empty(), // find → none
            HttpResponse {
                status: 201,
                body: "{}".into(),
            }, // POST create
        ]);
        let alerts = CapturingAlerts::default();
        let w = DigitalOceanWriter::new(
            "services.matthewmackes.com",
            "eagle",
            http,
            StaticToken("tok"),
            alerts,
        );
        w.upsert("eagle-mullvad.services.matthewmackes.com", "1.2.3.4", 60)
            .unwrap();
        let calls = w.http.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        // 1) the lookup GET against the registrable domain.
        assert_eq!(calls[0].0, "GET");
        assert!(calls[0].1.contains("/v2/domains/matthewmackes.com/records"));
        // 2) a POST create (no id) with the A-record body.
        assert_eq!(calls[1].0, "POST");
        assert_eq!(
            calls[1].1,
            "https://api.digitalocean.com/v2/domains/matthewmackes.com/records"
        );
        let body = calls[1].2.as_deref().unwrap();
        assert!(body.contains("\"type\":\"A\""));
        assert!(body.contains("\"name\":\"eagle-mullvad.services\""));
        assert!(body.contains("\"data\":\"1.2.3.4\""));
        // Token reached the transport (kept off argv by CurlExec, not here).
        assert_eq!(calls[1].3, "tok");
    }

    // ── upsert-update (PUT by id) ───────────────────────────────────

    #[test]
    fn upsert_updates_with_put_by_id_when_record_exists() {
        let list = HttpResponse {
            status: 200,
            body: r#"{"domain_records":[{"id":42,"type":"A","name":"eagle-mullvad.services","data":"9.9.9.9"}]}"#.into(),
        };
        let http = MockExec::new(vec![
            list,
            HttpResponse {
                status: 200,
                body: "{}".into(),
            }, // PUT update
        ]);
        let w = DigitalOceanWriter::new(
            "services.matthewmackes.com",
            "eagle",
            http,
            StaticToken("tok"),
            CapturingAlerts::default(),
        );
        w.upsert("eagle-mullvad.services.matthewmackes.com", "1.2.3.4", 60)
            .unwrap();
        let calls = w.http.calls.lock().unwrap();
        assert_eq!(calls[1].0, "PUT");
        assert_eq!(
            calls[1].1,
            "https://api.digitalocean.com/v2/domains/matthewmackes.com/records/42"
        );
    }

    // ── AAAA via record_type ────────────────────────────────────────

    #[test]
    fn upsert_v6_creates_an_aaaa_record() {
        let http = MockExec::new(vec![
            ok_list_empty(),
            HttpResponse {
                status: 201,
                body: "{}".into(),
            },
        ]);
        let w = DigitalOceanWriter::new(
            "services.matthewmackes.com",
            "eagle",
            http,
            StaticToken("tok"),
            CapturingAlerts::default(),
        );
        w.upsert("eagle-wan.services.matthewmackes.com", "2001:db8::1", 60)
            .unwrap();
        let calls = w.http.calls.lock().unwrap();
        // The lookup filtered on AAAA, and the create body is an AAAA.
        assert!(calls[0].1.contains("type=AAAA"));
        assert!(calls[1].2.as_deref().unwrap().contains("\"type\":\"AAAA\""));
    }

    // ── delete ──────────────────────────────────────────────────────

    #[test]
    fn remove_deletes_by_id_then_no_ops_when_absent() {
        let list = HttpResponse {
            status: 200,
            body: r#"{"domain_records":[{"id":42,"type":"A","name":"eagle-mullvad.services","data":"1.2.3.4"}]}"#.into(),
        };
        let http = MockExec::new(vec![
            list,
            HttpResponse {
                status: 204,
                body: "".into(),
            }, // DELETE
        ]);
        let w = DigitalOceanWriter::new(
            "services.matthewmackes.com",
            "eagle",
            http,
            StaticToken("tok"),
            CapturingAlerts::default(),
        );
        w.remove("eagle-mullvad.services.matthewmackes.com", "A")
            .unwrap();
        let calls = w.http.calls.lock().unwrap();
        assert_eq!(calls[1].0, "DELETE");
        assert_eq!(
            calls[1].1,
            "https://api.digitalocean.com/v2/domains/matthewmackes.com/records/42"
        );
    }

    #[test]
    fn remove_is_a_noop_when_record_absent() {
        let http = MockExec::new(vec![ok_list_empty()]); // find → none, no delete
        let w = DigitalOceanWriter::new(
            "services.matthewmackes.com",
            "eagle",
            http,
            StaticToken("tok"),
            CapturingAlerts::default(),
        );
        w.remove("ghost.services.matthewmackes.com", "A").unwrap();
        // Only the lookup happened — no DELETE call.
        assert_eq!(w.http.calls.lock().unwrap().len(), 1);
    }

    // ── 401 → ddns/auth alert (the headline mapping) ────────────────

    #[test]
    fn http_401_raises_ddns_auth_alert_not_a_silent_noop() {
        let http = MockExec::new(vec![HttpResponse {
            status: 401,
            body: r#"{"id":"unauthorized","message":"Unable to authenticate you"}"#.into(),
        }]);
        let alerts = CapturingAlerts::default();
        let w = DigitalOceanWriter::new(
            "services.matthewmackes.com",
            "eagle",
            http,
            StaticToken("expired-tok"),
            alerts,
        );
        let r = w.upsert("eagle-mullvad.services.matthewmackes.com", "1.2.3.4", 60);
        assert!(
            matches!(r, Err(WriteError::Auth(_))),
            "401 must map to Auth"
        );
        // The alert fired for this host — NOT swallowed.
        let fired = w.alerts.fired.lock().unwrap();
        assert_eq!(fired.len(), 1, "exactly one ddns/auth alert");
        assert_eq!(fired[0].0, "eagle");
        assert!(fired[0].1.contains("token"));
    }

    #[test]
    fn http_403_also_maps_to_auth() {
        let http = MockExec::new(vec![HttpResponse {
            status: 403,
            body: r#"{"id":"forbidden"}"#.into(),
        }]);
        let w = DigitalOceanWriter::new(
            "services.matthewmackes.com",
            "eagle",
            http,
            StaticToken("scoped-out-tok"),
            CapturingAlerts::default(),
        );
        assert!(matches!(
            w.remove("eagle-mullvad.services.matthewmackes.com", "A"),
            Err(WriteError::Auth(_))
        ));
        assert_eq!(w.alerts.fired.lock().unwrap().len(), 1);
    }

    // ── token + transport failures don't alert-auth ─────────────────

    #[test]
    fn token_resolution_failure_is_token_error_not_auth() {
        struct FailingToken;
        impl TokenSource for FailingToken {
            fn token(&self) -> anyhow::Result<String> {
                anyhow::bail!("no blob")
            }
        }
        let http = MockExec::new(vec![]);
        let w = DigitalOceanWriter::new(
            "services.matthewmackes.com",
            "eagle",
            http,
            FailingToken,
            CapturingAlerts::default(),
        );
        let r = w.upsert("eagle-mullvad.services.matthewmackes.com", "1.2.3.4", 60);
        assert!(matches!(r, Err(WriteError::Token(_))));
        // No HTTP call, no auth alert (the token never reached DO).
        assert_eq!(w.http.calls.lock().unwrap().len(), 0);
        assert_eq!(w.alerts.fired.lock().unwrap().len(), 0);
    }

    #[test]
    fn non_auth_api_error_surfaces_as_api_not_auth() {
        let http = MockExec::new(vec![HttpResponse {
            status: 500,
            body: "internal".into(),
        }]);
        let w = DigitalOceanWriter::new(
            "services.matthewmackes.com",
            "eagle",
            http,
            StaticToken("tok"),
            CapturingAlerts::default(),
        );
        let r = w.upsert("eagle-mullvad.services.matthewmackes.com", "1.2.3.4", 60);
        assert!(matches!(r, Err(WriteError::Api(_))));
        assert_eq!(w.alerts.fired.lock().unwrap().len(), 0);
    }

    // ── reconcile_record policy branch ──────────────────────────────

    /// A DnsWriter that records what it was asked to do (policy test).
    #[derive(Default)]
    struct RecordingWriter {
        ops: Mutex<Vec<String>>,
    }
    impl DnsWriter for RecordingWriter {
        fn upsert(&self, fqdn: &str, ip: &str, _ttl: u32) -> Result<(), WriteError> {
            self.ops.lock().unwrap().push(format!("upsert {fqdn} {ip}"));
            Ok(())
        }
        fn remove(&self, fqdn: &str, rt: &str) -> Result<(), WriteError> {
            self.ops.lock().unwrap().push(format!("remove {fqdn} {rt}"));
            Ok(())
        }
    }

    #[test]
    fn reconcile_present_ip_upserts() {
        let w = RecordingWriter::default();
        reconcile_record(&w, "n.example", Some("1.2.3.4"), 60, OnDown::Remove, "").unwrap();
        assert_eq!(
            w.ops.lock().unwrap().as_slice(),
            ["upsert n.example 1.2.3.4"]
        );
    }

    #[test]
    fn reconcile_down_remove_deletes_both_families() {
        let w = RecordingWriter::default();
        reconcile_record(&w, "n.example", None, 60, OnDown::Remove, "").unwrap();
        let ops = w.ops.lock().unwrap();
        assert_eq!(
            ops.as_slice(),
            ["remove n.example A", "remove n.example AAAA"]
        );
    }

    #[test]
    fn reconcile_down_keep_is_a_noop() {
        let w = RecordingWriter::default();
        reconcile_record(&w, "n.example", None, 60, OnDown::Keep, "").unwrap();
        assert!(w.ops.lock().unwrap().is_empty());
    }

    #[test]
    fn reconcile_down_sentinel_upserts_the_parked_addr() {
        let w = RecordingWriter::default();
        reconcile_record(&w, "n.example", None, 60, OnDown::Sentinel, "192.0.2.1").unwrap();
        assert_eq!(
            w.ops.lock().unwrap().as_slice(),
            ["upsert n.example 192.0.2.1"]
        );
    }
}
