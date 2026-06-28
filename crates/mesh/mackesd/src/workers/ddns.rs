//! DDNS-EGRESS-4 — the dynamic-DNS reconcile worker (VPN exit + WAN → DNS).
//!
//! A periodic worker (beside `vpn_health`) that watches the per-tunnel exit
//! state (VPN-GW-6) + the node WAN IP and keeps the `[ddns]` records under the
//! configured zone pointing at the live IP via a [`DnsWriter`] (DigitalOcean in
//! v1). On **reconnect-with-a-new-IP** the record is rewritten within ~TTL (the
//! sweep cadence is under the default 60 s TTL); on **tunnel-down** the record
//! follows its `on_down` policy (`remove | sentinel | keep`) tied to the
//! VPN-GW kill-switch state — so a name never silently points at a dead/leaking
//! exit (DDNS-EGRESS-4 acceptance). A bad/expired DO token raises a clear
//! `ddns/auth` alert, not a silent no-op (§7).
//!
//! The DNS I/O is a [`DnsWriter`] trait so the reconnect-rewrite + on_down
//! decision (`plan_record` / `resolve_source_ip`) is exercised end-to-end against
//! an injected in-memory writer in tests — no network. The DigitalOcean adapter
//! ([`DigitalOceanWriter`]) shells `curl`, passing the bearer token through a
//! stdin config (never argv) so it can't appear in `ps`/logs (§3).

use std::path::{Path, PathBuf};
use std::time::Duration;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

use mackes_mesh_types::ddns::{
    self, needs_update, record_type, DdnsConfig, OnDown, PublishedRecord, RecordDef,
};
use mackes_mesh_types::vpn::{self, VpnExitState};
use mackes_mesh_types::vpn_providers;

use crate::ipc::secret_store::{self, SecretStore};

/// Sweep cadence — under the DDNS default 60 s TTL so a reconnect-with-new-IP is
/// republished "within ~TTL" (DDNS-EGRESS-4 acceptance).
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(30);

/// The sentinel address an `on_down = sentinel` record is parked at — TEST-NET-1
/// (RFC 5737), guaranteed unrouted so the name resolves to a deliberately-dead IP
/// rather than a stale/leaking exit.
pub const SENTINEL_IP: &str = "192.0.2.1";

/// What the worker should do with a record this sweep (the pure decision).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordAction {
    /// Upsert the A/AAAA record to this IP (first publish / reconnect-new-IP).
    Upsert(String),
    /// Remove the record (`on_down = remove` + the source is down).
    Remove,
    /// Park the record at [`SENTINEL_IP`] (`on_down = sentinel` + source down).
    Sentinel,
    /// Leave the record as-is (`on_down = keep`, or the IP is unchanged).
    Keep,
}

/// Resolve the IP a record's source currently presents, or `None` when the
/// source is down / unknown (which triggers the `on_down` policy). A `wan` source
/// presents the node WAN IP; a `tunnel:<id>` source presents the **verified**
/// exit IP only when the tunnel is up + verified (a leaking/down tunnel is "down"
/// here, tying the record to the kill-switch state). Pure.
#[must_use]
pub fn resolve_source_ip(source: &str, state: &VpnExitState, wan: Option<&str>) -> Option<String> {
    let s = source.trim();
    if s.eq_ignore_ascii_case("wan") {
        return wan
            .map(str::trim)
            .filter(|w| !w.is_empty())
            .map(str::to_string);
    }
    let id = s.strip_prefix("tunnel:")?.trim();
    state
        .get(id)
        .filter(|e| e.up && e.verified && !e.exit_ip.trim().is_empty())
        .map(|e| e.exit_ip.clone())
}

/// The provider label to template a record's name with: a tunnel source uses the
/// tunnel's provider (from the exit state), a WAN source is literally `wan`. Pure.
#[must_use]
pub fn provider_for(source: &str, state: &VpnExitState) -> String {
    let s = source.trim();
    if s.eq_ignore_ascii_case("wan") {
        return "wan".to_string();
    }
    s.strip_prefix("tunnel:")
        .and_then(|id| state.get(id.trim()))
        .map(|e| e.provider.clone())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| "vpn".to_string())
}

/// The pure reconnect-rewrite + on_down decision: given the source's current IP
/// (`None` ⇒ down), the last value published for this record, and the record's
/// `on_down` policy, decide the DNS action.
///
///   * a live IP that differs from the last published ⇒ **Upsert** (first publish
///     or reconnect-with-a-new-IP — the within-TTL rewrite),
///   * a live IP unchanged ⇒ **Keep** (no churn, the [`needs_update`] rule),
///   * source down ⇒ the `on_down` policy: **Remove** / **Sentinel** / **Keep**.
///
/// Pure — the load-bearing DDNS-EGRESS-4 logic, unit-tested.
#[must_use]
pub fn plan_record(current: Option<&str>, last: Option<&str>, on_down: OnDown) -> RecordAction {
    match current.map(str::trim).filter(|s| !s.is_empty()) {
        Some(ip) => {
            if needs_update(last, ip) {
                RecordAction::Upsert(ip.to_string())
            } else {
                RecordAction::Keep
            }
        }
        None => match on_down {
            OnDown::Remove => RecordAction::Remove,
            OnDown::Sentinel => RecordAction::Sentinel,
            OnDown::Keep => RecordAction::Keep,
        },
    }
}

/// The registrable apex of a zone — its last two dot-labels
/// (`a.b.matthewmackes.com` → `matthewmackes.com`). The DigitalOcean *domain* is
/// the apex; a record's `name` is the FQDN minus the apex. Pure.
#[must_use]
pub fn apex_domain(zone: &str) -> String {
    let labels: Vec<&str> = zone
        .trim_matches('.')
        .split('.')
        .filter(|s| !s.is_empty())
        .collect();
    if labels.len() <= 2 {
        return labels.join(".");
    }
    labels[labels.len() - 2..].join(".")
}

/// The DigitalOcean record `name` for an FQDN under `apex`: the FQDN with the
/// `.{apex}` suffix stripped (`eagle.services.matthewmackes.com` w/ apex
/// `matthewmackes.com` → `eagle.services`); the bare apex → `@`. Pure.
#[must_use]
pub fn do_record_name(fqdn: &str, apex: &str) -> String {
    let fqdn = fqdn.trim_matches('.');
    if fqdn.eq_ignore_ascii_case(apex) {
        return "@".to_string();
    }
    fqdn.strip_suffix(&format!(".{apex}"))
        .map_or_else(|| fqdn.to_string(), str::to_string)
}

/// A DNS write error — `Auth` is surfaced as the `ddns/auth` alert (§7), `Other`
/// degrades the record to `error` status without claiming success.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DnsError {
    /// The token was rejected (401/403) — actionable, alert-worthy.
    Auth(String),
    /// Any other failure (network, parse, API error).
    Other(String),
}

impl std::fmt::Display for DnsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auth(m) => write!(f, "auth: {m}"),
            Self::Other(m) => write!(f, "{m}"),
        }
    }
}

/// The DNS-host abstraction (DDNS-EGRESS-2 design): upsert / remove A/AAAA
/// records under a domain. v1 ships [`DigitalOceanWriter`]; the trait keeps
/// Cloudflare/Route53 addable without touching the worker.
pub trait DnsWriter {
    /// Upsert the `rtype` record `name` under `domain` to `ip` with `ttl`.
    ///
    /// # Errors
    /// [`DnsError`] on an API / auth / network failure.
    fn upsert(&self, domain: &str, name: &str, ip: &str, ttl: u32) -> Result<(), DnsError>;

    /// Remove the record `name` (all types) under `domain`. A name that doesn't
    /// exist is a no-op success.
    ///
    /// # Errors
    /// [`DnsError`] on an API / auth / network failure.
    fn remove(&self, domain: &str, name: &str) -> Result<(), DnsError>;
}

/// The DigitalOcean DNS adapter. Shells `curl`, passing the bearer token via a
/// stdin config (`curl -K -`) so it never lands in argv (`ps`)/logs (§3).
pub struct DigitalOceanWriter {
    token: String,
}

impl DigitalOceanWriter {
    /// Build the adapter from the (already-resolved) DO API token.
    #[must_use]
    pub fn new(token: String) -> Self {
        Self { token }
    }
}

/// The DigitalOcean API base.
const DO_API: &str = "https://api.digitalocean.com";

impl DnsWriter for DigitalOceanWriter {
    fn upsert(&self, domain: &str, name: &str, ip: &str, ttl: u32) -> Result<(), DnsError> {
        let rtype = record_type(ip);
        let existing = self.find_record_id(domain, name, rtype)?;
        let (method, path, body) =
            ddns::do_upsert_request(domain, name, ip, ttl, existing.as_deref());
        let (code, resp) = self.curl(method, &path, Some(&body))?;
        check_code(code, &resp)
    }

    fn remove(&self, domain: &str, name: &str) -> Result<(), DnsError> {
        // Remove every A/AAAA record for the name (idempotent: none ⇒ ok).
        for rtype in ["A", "AAAA"] {
            while let Some(id) = self.find_record_id(domain, name, rtype)? {
                let (method, path) = ddns::do_delete_request(domain, &id);
                let (code, resp) = self.curl(method, &path, None)?;
                check_code(code, &resp)?;
            }
        }
        Ok(())
    }
}

impl DigitalOceanWriter {
    /// Find the id of the `name`/`rtype` record under `domain`, if any.
    fn find_record_id(
        &self,
        domain: &str,
        name: &str,
        rtype: &str,
    ) -> Result<Option<String>, DnsError> {
        let fqdn = if name == "@" {
            domain.to_string()
        } else {
            format!("{name}.{domain}")
        };
        let path = format!("/v2/domains/{domain}/records?type={rtype}&name={fqdn}&per_page=200");
        let (code, resp) = self.curl("GET", &path, None)?;
        check_code(code, &resp)?;
        Ok(parse_first_record_id(&resp, name))
    }

    /// Run one `curl` against the DO API. The URL/method/body go on argv; the
    /// **token rides a stdin config** (`-K -`) so it's never in `ps`. Returns
    /// `(http_code, body)`.
    fn curl(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
    ) -> Result<(u16, String), DnsError> {
        use std::fmt::Write as _;
        use std::io::Write as _;
        use std::process::{Command, Stdio};

        let mut cfg = String::new();
        // header lines carry the secret — kept out of argv.
        let _ = writeln!(cfg, "header = \"Authorization: Bearer {}\"", self.token);
        cfg.push_str("header = \"Content-Type: application/json\"\n");
        let mut child = Command::new("curl")
            .args([
                "-sS",
                "--max-time",
                "15",
                "-o",
                "-",
                "-w",
                "\n%{http_code}",
                "-X",
                method,
                "-K",
                "-",
                &format!("{DO_API}{path}"),
            ])
            .args(
                body.iter()
                    .flat_map(|b| ["-d".to_string(), (*b).to_string()]),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| DnsError::Other(format!("curl spawn: {e}")))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(cfg.as_bytes())
                .map_err(|e| DnsError::Other(format!("curl stdin: {e}")))?;
        }
        let out = child
            .wait_with_output()
            .map_err(|e| DnsError::Other(format!("curl wait: {e}")))?;
        if !out.status.success() && out.stdout.is_empty() {
            return Err(DnsError::Other(format!(
                "curl exited {:?}",
                out.status.code()
            )));
        }
        let full = String::from_utf8_lossy(&out.stdout);
        let (body, code) = full
            .rsplit_once('\n')
            .unwrap_or_else(|| (full.as_ref(), "0"));
        let code: u16 = code.trim().parse().unwrap_or(0);
        Ok((code, body.to_string()))
    }
}

/// Map a DO HTTP status to a [`DnsError`]: 401/403 → `Auth`, 2xx → ok, else
/// `Other`. Pure.
fn check_code(code: u16, resp: &str) -> Result<(), DnsError> {
    match code {
        200..=299 => Ok(()),
        401 | 403 => Err(DnsError::Auth(format!(
            "DigitalOcean rejected the token (HTTP {code})"
        ))),
        _ => Err(DnsError::Other(format!(
            "DigitalOcean API HTTP {code}: {}",
            resp.trim()
        ))),
    }
}

/// Pull the first record id from a DO `{"domain_records":[{id,name,...}]}` list
/// whose `name` matches `want` (the DO label). Pure.
#[must_use]
fn parse_first_record_id(resp: &str, want: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(resp).ok()?;
    v.get("domain_records")?
        .as_array()?
        .iter()
        .find(|r| r.get("name").and_then(serde_json::Value::as_str) == Some(want))
        .and_then(|r| r.get("id"))
        .map(|id| match id {
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        })
}

/// The DDNS reconcile worker.
#[derive(Clone, Debug)]
pub struct DdnsWorker {
    workgroup_root: PathBuf,
    node_id: String,
    /// Run the real `curl` probes/writes; tests disable it + inject a writer.
    spawn: bool,
}

impl DdnsWorker {
    /// Build the worker rooted at the shared workgroup root for `node_id`.
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: String) -> Self {
        Self {
            workgroup_root,
            node_id,
            spawn: true,
        }
    }

    /// Disable the WAN probe shell-out (tests).
    #[must_use]
    pub fn without_spawn(mut self) -> Self {
        self.spawn = false;
        self
    }

    /// Run the reconcile loop until `should_stop`.
    pub fn run<F: Fn() -> bool>(&self, persist: &Persist, alerts_dir: &Path, should_stop: F) {
        let mut auth_alerted = false;
        while !should_stop() {
            self.sweep_live(persist, alerts_dir, &mut auth_alerted);
            // Sleep in small slices so shutdown is prompt.
            let mut slept = Duration::ZERO;
            while slept < SWEEP_INTERVAL && !should_stop() {
                std::thread::sleep(Duration::from_millis(500));
                slept += Duration::from_millis(500);
            }
        }
    }

    /// One live sweep: build the DO writer from the encrypted token, then run the
    /// pure reconcile against it. Splits the I/O (writer build + WAN probe) from
    /// the writer-agnostic [`reconcile`](Self::reconcile) so the latter is tested
    /// with an in-memory writer.
    fn sweep_live(&self, persist: &Persist, alerts_dir: &Path, auth_alerted: &mut bool) {
        let cfg = ddns::load(&self.workgroup_root);
        if !cfg.enabled {
            return; // master switch off — do nothing (no churn, no alerts).
        }
        let token = self.resolve_token(&cfg);
        let Some(token) = token else {
            // No token distributed yet → can't write. Honest alert, edge-triggered.
            self.raise_auth(
                persist,
                alerts_dir,
                auth_alerted,
                "DO API token not in the secret store",
            );
            return;
        };
        let writer = DigitalOceanWriter::new(token);
        let wan = self.probe_wan();
        let outcome = self.reconcile(
            &cfg,
            &vpn::load_exit_state(&self.workgroup_root),
            wan.as_deref(),
            &writer,
        );
        if outcome.auth_failed {
            self.raise_auth(
                persist,
                alerts_dir,
                auth_alerted,
                "DigitalOcean rejected the DDNS token",
            );
        } else {
            *auth_alerted = false; // recovered — re-arm the edge.
        }
    }

    /// The writer-agnostic reconcile: for each record resolve the source IP, plan
    /// the action ([`plan_record`]), apply it via `writer`, and persist the
    /// published state. Returns whether any write hit an auth failure. Pure of the
    /// token/WAN I/O (those are the caller's), so it is unit-tested with a fake
    /// [`DnsWriter`].
    fn reconcile<W: DnsWriter>(
        &self,
        cfg: &DdnsConfig,
        exit_state: &VpnExitState,
        wan: Option<&str>,
        writer: &W,
    ) -> ReconcileOutcome {
        let apex = apex_domain(&cfg.zone);
        let now = now_ms();
        let mut published = ddns::load_published(&self.workgroup_root);
        let mut auth_failed = false;
        for rec in &cfg.record {
            let fqdn = self.fqdn_for(rec, exit_state, &cfg.zone);
            let name = do_record_name(&fqdn, &apex);
            let current = resolve_source_ip(&rec.source, exit_state, wan);
            let last = published
                .get(&fqdn)
                .map(|p| p.ip.clone())
                .filter(|ip| !ip.is_empty());
            let action = plan_record(current.as_deref(), last.as_deref(), rec.on_down);
            let result = apply_action(writer, &apex, &name, &action, cfg.ttl);
            if matches!(result, Err(DnsError::Auth(_))) {
                auth_failed = true;
            }
            published.upsert(published_after(rec, &fqdn, &action, cfg.ttl, now, &result));
        }
        if let Err(e) = ddns::save_published(&self.workgroup_root, &published) {
            tracing::warn!(target: "mackesd::ddns", error = %e, "published-state write failed");
        }
        ReconcileOutcome { auth_failed }
    }

    /// The FQDN for a record: template `{node}`/`{provider}`/`{n}` from this node
    /// + the source's provider.
    fn fqdn_for(&self, rec: &RecordDef, state: &VpnExitState, zone: &str) -> String {
        let provider = provider_for(&rec.source, state);
        rec.fqdn(&self.node_id, &provider, 1, zone)
    }

    /// Resolve the DO API token from the encrypted secret store via `token_ref`
    /// (a `secret://<key>` or a bare store key). `None` ⇒ not distributed yet.
    fn resolve_token(&self, cfg: &DdnsConfig) -> Option<String> {
        let key = cfg
            .token_ref
            .trim()
            .strip_prefix("secret://")
            .unwrap_or(cfg.token_ref.trim());
        if key.is_empty() {
            return None;
        }
        let store = SecretStore::resolve(&secret_store::repo_root(), &self.workgroup_root);
        match store.get(key) {
            Ok(Some(t)) if !t.trim().is_empty() => Some(t.trim().to_string()),
            _ => None,
        }
    }

    /// Probe the node's direct WAN IP (best-effort).
    fn probe_wan(&self) -> Option<String> {
        if !self.spawn {
            return None;
        }
        let out = std::process::Command::new("curl")
            .args([
                "-s",
                "--max-time",
                "6",
                vpn_providers::NEUTRAL_EXIT_CHECK_HOST,
            ])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        crate::workers::vpn_health::parse_ip(&String::from_utf8_lossy(&out.stdout))
    }

    /// Raise the `ddns/auth` alert (Bus event + Hub toast), edge-triggered so it
    /// fires once per failure episode, not every sweep.
    fn raise_auth(&self, persist: &Persist, alerts_dir: &Path, alerted: &mut bool, why: &str) {
        if *alerted {
            return;
        }
        *alerted = true;
        let body = serde_json::json!({ "node": self.node_id, "detail": why }).to_string();
        if let Err(e) = persist.write("event/ddns/auth", Priority::High, None, Some(&body)) {
            tracing::warn!(target: "mackesd::ddns", error = %e, "ddns/auth event publish failed");
        }
        let minute = now_ms() / 60_000;
        let id = format!("ddns-auth-{}-{minute}", self.node_id);
        let event = serde_json::json!({
            "id": id,
            "severity": "warn",
            "alert": "ddns.auth",
            "host": self.node_id,
            "summary": format!("DDNS token error — {why}"),
        });
        if std::fs::create_dir_all(alerts_dir).is_ok() {
            let _ = std::fs::write(alerts_dir.join(format!("{id}.json")), event.to_string());
        }
    }
}

/// The result of one [`DdnsWorker::reconcile`] pass.
struct ReconcileOutcome {
    auth_failed: bool,
}

/// Apply one planned [`RecordAction`] via the writer. A `Keep` is a no-op success.
fn apply_action<W: DnsWriter>(
    writer: &W,
    domain: &str,
    name: &str,
    action: &RecordAction,
    ttl: u32,
) -> Result<(), DnsError> {
    match action {
        RecordAction::Upsert(ip) => writer.upsert(domain, name, ip, ttl),
        RecordAction::Sentinel => writer.upsert(domain, name, SENTINEL_IP, ttl),
        RecordAction::Remove => writer.remove(domain, name),
        RecordAction::Keep => Ok(()),
    }
}

/// Build the [`PublishedRecord`] reflecting an applied action + its result. Pure.
fn published_after(
    rec: &RecordDef,
    fqdn: &str,
    action: &RecordAction,
    ttl: u32,
    now_ms: u64,
    result: &Result<(), DnsError>,
) -> PublishedRecord {
    let (ip, status) = match (action, result) {
        (_, Err(DnsError::Auth(_))) => (last_ip(action), "error".to_string()),
        (_, Err(DnsError::Other(_))) => (last_ip(action), "stale".to_string()),
        (RecordAction::Upsert(ip), Ok(())) => (ip.clone(), "synced".to_string()),
        (RecordAction::Sentinel, Ok(())) => (SENTINEL_IP.to_string(), "sentinel".to_string()),
        (RecordAction::Remove, Ok(())) => (String::new(), "removed".to_string()),
        (RecordAction::Keep, Ok(())) => (last_ip(action), "synced".to_string()),
    };
    let detail = match result {
        Ok(()) => String::new(),
        Err(e) => e.to_string(),
    };
    PublishedRecord {
        fqdn: fqdn.to_string(),
        source: rec.source.clone(),
        ip,
        updated_ms: now_ms,
        ttl,
        status,
        detail,
    }
}

/// The IP an action carries for the published record on a non-write path.
fn last_ip(action: &RecordAction) -> String {
    match action {
        RecordAction::Upsert(ip) => ip.clone(),
        RecordAction::Sentinel => SENTINEL_IP.to_string(),
        RecordAction::Remove | RecordAction::Keep => String::new(),
    }
}

/// Unix epoch milliseconds.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    use mackes_mesh_types::vpn::TunnelExit;

    fn state_with(id: &str, up: bool, verified: bool, ip: &str, provider: &str) -> VpnExitState {
        let mut s = VpnExitState::default();
        s.upsert(TunnelExit {
            id: id.into(),
            up,
            verified,
            exit_ip: ip.into(),
            provider: provider.into(),
            ..Default::default()
        });
        s
    }

    #[test]
    fn resolve_source_ip_tunnel_verified_wan_and_down() {
        let st = state_with("mullvad1", true, true, "1.2.3.4", "mullvad");
        assert_eq!(
            resolve_source_ip("tunnel:mullvad1", &st, None),
            Some("1.2.3.4".into())
        );
        // WAN source.
        assert_eq!(
            resolve_source_ip("wan", &st, Some("9.9.9.9")),
            Some("9.9.9.9".into())
        );
        // A down/unverified tunnel ⇒ None (triggers on_down).
        let down = state_with("mullvad1", false, false, "1.2.3.4", "mullvad");
        assert_eq!(resolve_source_ip("tunnel:mullvad1", &down, None), None);
        // Unknown tunnel ⇒ None.
        assert_eq!(resolve_source_ip("tunnel:ghost", &st, None), None);
    }

    #[test]
    fn plan_record_reconnect_rewrite_and_on_down() {
        // First publish (no last).
        assert_eq!(
            plan_record(Some("1.2.3.4"), None, OnDown::Keep),
            RecordAction::Upsert("1.2.3.4".into())
        );
        // Reconnect with a NEW ip ⇒ rewrite.
        assert_eq!(
            plan_record(Some("5.6.7.8"), Some("1.2.3.4"), OnDown::Keep),
            RecordAction::Upsert("5.6.7.8".into())
        );
        // Unchanged ⇒ keep (no churn).
        assert_eq!(
            plan_record(Some("1.2.3.4"), Some("1.2.3.4"), OnDown::Keep),
            RecordAction::Keep
        );
        // Down + remove / sentinel / keep.
        assert_eq!(
            plan_record(None, Some("1.2.3.4"), OnDown::Remove),
            RecordAction::Remove
        );
        assert_eq!(
            plan_record(None, Some("1.2.3.4"), OnDown::Sentinel),
            RecordAction::Sentinel
        );
        assert_eq!(
            plan_record(None, Some("1.2.3.4"), OnDown::Keep),
            RecordAction::Keep
        );
    }

    #[test]
    fn apex_and_record_name_split() {
        assert_eq!(
            apex_domain("services.matthewmackes.com"),
            "matthewmackes.com"
        );
        assert_eq!(apex_domain("matthewmackes.com"), "matthewmackes.com");
        assert_eq!(
            do_record_name(
                "eagle-mullvad.services.matthewmackes.com",
                "matthewmackes.com"
            ),
            "eagle-mullvad.services"
        );
        assert_eq!(
            do_record_name("matthewmackes.com", "matthewmackes.com"),
            "@"
        );
    }

    #[test]
    fn parse_first_record_id_matches_by_name() {
        let resp = r#"{"domain_records":[
            {"id":111,"type":"A","name":"other.services"},
            {"id":222,"type":"A","name":"eagle-mullvad.services"}
        ]}"#;
        assert_eq!(
            parse_first_record_id(resp, "eagle-mullvad.services"),
            Some("222".into())
        );
        assert_eq!(parse_first_record_id(resp, "absent"), None);
    }

    #[test]
    fn check_code_maps_auth_and_errors() {
        assert!(check_code(200, "{}").is_ok());
        assert_eq!(
            check_code(401, "no"),
            Err(DnsError::Auth(
                "DigitalOcean rejected the token (HTTP 401)".into()
            ))
        );
        assert!(matches!(check_code(500, "boom"), Err(DnsError::Other(_))));
    }

    // ── end-to-end reconcile against an in-memory writer ──

    #[derive(Default)]
    struct FakeWriter {
        records: RefCell<std::collections::HashMap<String, String>>, // name -> ip
        auth_fail: bool,
        calls: RefCell<Vec<String>>,
    }
    impl DnsWriter for FakeWriter {
        fn upsert(&self, _domain: &str, name: &str, ip: &str, _ttl: u32) -> Result<(), DnsError> {
            if self.auth_fail {
                return Err(DnsError::Auth("bad token".into()));
            }
            self.calls.borrow_mut().push(format!("upsert {name} {ip}"));
            self.records
                .borrow_mut()
                .insert(name.to_string(), ip.to_string());
            Ok(())
        }
        fn remove(&self, _domain: &str, name: &str) -> Result<(), DnsError> {
            self.calls.borrow_mut().push(format!("remove {name}"));
            self.records.borrow_mut().remove(name);
            Ok(())
        }
    }

    fn worker() -> (tempfile::TempDir, DdnsWorker) {
        let tmp = tempfile::tempdir().unwrap();
        let w = DdnsWorker::new(tmp.path().to_path_buf(), "eagle".into()).without_spawn();
        (tmp, w)
    }

    fn cfg_with(record: RecordDef) -> DdnsConfig {
        DdnsConfig {
            enabled: true,
            zone: "services.matthewmackes.com".into(),
            ttl: 60,
            record: vec![record],
            ..Default::default()
        }
    }

    #[test]
    fn reconcile_publishes_then_rewrites_on_reconnect_then_applies_on_down() {
        let (_t, w) = worker();
        let rec = RecordDef {
            name: "{node}-{provider}".into(),
            source: "tunnel:mullvad1".into(),
            on_down: OnDown::Remove,
        };
        let cfg = cfg_with(rec);
        let writer = FakeWriter::default();

        // 1) First publish: tunnel up + verified at 1.2.3.4.
        let st = state_with("mullvad1", true, true, "1.2.3.4", "mullvad");
        w.reconcile(&cfg, &st, None, &writer);
        let pub1 = ddns::load_published(&w.workgroup_root);
        let r1 = pub1
            .get("eagle-mullvad.services.matthewmackes.com")
            .unwrap();
        assert_eq!(r1.ip, "1.2.3.4");
        assert_eq!(r1.status, "synced");
        assert_eq!(
            *writer
                .records
                .borrow()
                .get("eagle-mullvad.services")
                .unwrap(),
            "1.2.3.4"
        );

        // 2) Reconnect with a NEW IP → the record is rewritten within TTL.
        let st2 = state_with("mullvad1", true, true, "5.6.7.8", "mullvad");
        writer.calls.borrow_mut().clear();
        w.reconcile(&cfg, &st2, None, &writer);
        assert!(writer
            .calls
            .borrow()
            .iter()
            .any(|c| c == "upsert eagle-mullvad.services 5.6.7.8"));
        let r2 = ddns::load_published(&w.workgroup_root);
        assert_eq!(
            r2.get("eagle-mullvad.services.matthewmackes.com")
                .unwrap()
                .ip,
            "5.6.7.8"
        );

        // 3) Unchanged → no churn (no new write call).
        writer.calls.borrow_mut().clear();
        w.reconcile(&cfg, &st2, None, &writer);
        assert!(
            writer.calls.borrow().is_empty(),
            "unchanged IP must not re-write"
        );

        // 4) Tunnel down + on_down=remove → the record is removed.
        let down = state_with("mullvad1", false, false, "", "mullvad");
        w.reconcile(&cfg, &down, None, &writer);
        assert!(!writer
            .records
            .borrow()
            .contains_key("eagle-mullvad.services"));
        let r4 = ddns::load_published(&w.workgroup_root);
        assert_eq!(
            r4.get("eagle-mullvad.services.matthewmackes.com")
                .unwrap()
                .status,
            "removed"
        );
    }

    #[test]
    fn reconcile_on_down_sentinel_parks_the_record() {
        let (_t, w) = worker();
        let cfg = cfg_with(RecordDef {
            name: "{node}-{provider}".into(),
            source: "tunnel:mullvad1".into(),
            on_down: OnDown::Sentinel,
        });
        let writer = FakeWriter::default();
        let down = state_with("mullvad1", false, false, "", "mullvad");
        w.reconcile(&cfg, &down, None, &writer);
        assert_eq!(
            *writer
                .records
                .borrow()
                .get("eagle-mullvad.services")
                .unwrap(),
            SENTINEL_IP
        );
        let p = ddns::load_published(&w.workgroup_root);
        assert_eq!(
            p.get("eagle-mullvad.services.matthewmackes.com")
                .unwrap()
                .status,
            "sentinel"
        );
    }

    #[test]
    fn reconcile_wan_record_publishes_wan_ip() {
        let (_t, w) = worker();
        let cfg = cfg_with(RecordDef {
            name: "{node}-wan".into(),
            source: "wan".into(),
            on_down: OnDown::Keep,
        });
        let writer = FakeWriter::default();
        w.reconcile(&cfg, &VpnExitState::default(), Some("9.9.9.9"), &writer);
        assert_eq!(
            *writer.records.borrow().get("eagle-wan.services").unwrap(),
            "9.9.9.9"
        );
    }

    #[test]
    fn reconcile_surfaces_auth_failure() {
        let (_t, w) = worker();
        let cfg = cfg_with(RecordDef {
            name: "{node}-{provider}".into(),
            source: "tunnel:mullvad1".into(),
            on_down: OnDown::Keep,
        });
        let writer = FakeWriter {
            auth_fail: true,
            ..Default::default()
        };
        let st = state_with("mullvad1", true, true, "1.2.3.4", "mullvad");
        let outcome = w.reconcile(&cfg, &st, None, &writer);
        assert!(outcome.auth_failed);
        // The published record reflects the error, not a fake success.
        let p = ddns::load_published(&w.workgroup_root);
        assert_eq!(
            p.get("eagle-mullvad.services.matthewmackes.com")
                .unwrap()
                .status,
            "error"
        );
    }
}
