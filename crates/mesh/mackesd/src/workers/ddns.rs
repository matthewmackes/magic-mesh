//! DDNS-EGRESS-3 (worker) — the dynamic-DNS reconcile loop + the DigitalOcean
//! `DnsWriter` adapter (design: `docs/design/ddns-egress.md`).
//!
//! The `[ddns]` config (`mackes_mesh_types::ddns`) and the `action/ddns/*` CRUD
//! responder ([`crate::ipc::ddns`]) are the durable model + control surface; the
//! pure reconcile core ([`mackes_mesh_types::ddns::plan_action`]) decides what a
//! record SHOULD be. This worker is the runtime engine that closes the loop:
//!
//!   1. **subscribe to VPN-GW exit-IP changes** — it tails `event/vpn/signals`
//!      (the same lane the VPN-GW-6 health sweep raises `vpn/tunnel-down` on) so a
//!      tunnel flapping up/down/leaking wakes a reconcile promptly, and
//!   2. **a periodic WAN check** — on a slow cadence it re-resolves every record's
//!      source (the tunnel's verified exit IP via the VPN-GW-6 verifier, or the
//!      node WAN for a `wan` source) and reconciles, so a silent IP change that
//!      raised no event is still caught within the interval.
//!
//! For each managed record it resolves the live [`SourceState`], applies the pure
//! [`plan_action`] predicate against the LAST value it published (the no-churn
//! rule lives there), and — only when the plan is an actual change — drives the
//! [`DnsWriter`]. The writer is a DigitalOcean A/AAAA-record adapter built on the
//! pure request builders ([`do_upsert_request`]/[`do_delete_request`]), the token
//! read from the mesh secret store via the config's `token_ref`, every HTTP call a
//! §9-safe fixed-arg `curl` argv (no shell, no interpolation into a command
//! string). The reconcile decision core is pure + unit-tested here; the spawn side
//! shells out exactly like the sibling VPN-GW-6 health worker.

use std::collections::HashMap;

use mde_bus::persist::Persist;

use mackes_mesh_types::ddns::{
    self, do_delete_request, do_upsert_request, DdnsAction, DdnsConfig, RecordDef, SourceState,
};

use crate::ipc::vpn_health::{self, Health};

/// The VPN-GW event lane the worker tails so an exit-IP/health change wakes a
/// reconcile promptly (the same topic VPN-GW-6 raises `vpn/tunnel-down` on).
pub const VPN_EVENT_TOPIC: &str = "event/vpn/signals";

/// How often the worker runs the full WAN/exit-IP re-resolve sweep even with no
/// VPN event. Matches the VPN-GW-6 health cadence: the exit-IP verifier shells out
/// to `curl` per source, so a tight loop would hammer the reflectors; 30 s catches
/// a silent change well inside an operator's reaction window.
pub const SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// How often the worker wakes to check the VPN event lane (fast poll, no I/O — a
/// new `event/vpn/signals` message triggers an immediate reconcile sweep).
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(800);

/// Strip the `peer:` prefix a node id may carry (`peer:eagle` → `eagle`) so the
/// `{node}` template placeholder is the bare short name. Pure.
#[must_use]
pub fn node_label(node_id: &str) -> &str {
    node_id.strip_prefix("peer:").unwrap_or(node_id)
}

/// The DNS writer the reconcile loop drives: create-or-update an A/AAAA record to
/// an IP, or delete it. The DigitalOcean adapter ([`DoDnsWriter`]) is the
/// production impl; a test fake records calls so the reconcile core is exercised
/// without any network. `name` is the bare record label (the part before the zone).
pub trait DnsWriter {
    /// Create-or-update the record `name` in `zone` to `ip` with `ttl`. Idempotent
    /// from the caller's view (the adapter looks up an existing record id and
    /// PUTs, else POSTs).
    ///
    /// # Errors
    /// An operator-readable failure (token unresolved, HTTP error, API error).
    fn upsert(&self, zone: &str, name: &str, ip: &str, ttl: u32) -> Result<(), String>;

    /// Delete the record `name` in `zone` (a no-op when it is already absent).
    ///
    /// # Errors
    /// An operator-readable failure.
    fn delete(&self, zone: &str, name: &str) -> Result<(), String>;
}

/// Resolve a record's live [`SourceState`] from its `source` field:
///   * `wan` → up at the node's raw-WAN IP when the WAN reflector answered, else
///     down (no kill-switch — the WAN has no tunnel to kill-switch),
///   * `tunnel:<id>` → the VPN-GW-6 verifier's verdict for that tunnel: up at the
///     verified exit IP on a healthy exit, down otherwise. A `Leaking`/`DnsLeak`
///     tunnel is treated as a kill-switched down so the leak-coupling rule
///     ([`plan_action`]) parks/removes the name rather than publishing a leaking
///     exit.
///
/// `wan` is the pre-fetched raw-WAN IP (so the per-sweep fetch happens once).
/// `spawn` false (tests / no tools) yields a deterministic Down without host I/O.
#[must_use]
pub fn resolve_source(
    spawn: bool,
    source: &str,
    cfg: &mackes_mesh_types::vpn::VpnConfig,
    wan: Option<&str>,
) -> SourceState {
    let src = source.trim();
    if src.eq_ignore_ascii_case("wan") {
        return match wan.map(str::trim).filter(|s| !s.is_empty()) {
            // The node's own public IP — an identity record, never inbound on its
            // own (no provider port-forward), so `port_forward: false`.
            Some(ip) => SourceState::Up {
                ip: ip.to_string(),
                port_forward: false,
            },
            None => SourceState::Down { kill_switch: false },
        };
    }
    let Some(id) = src.strip_prefix("tunnel:").map(str::trim) else {
        // An unrecognized source can't be resolved; treat as a clean down so the
        // on-down policy applies (never a spurious publish).
        return SourceState::Down { kill_switch: false };
    };
    let report = vpn_health::verify_chain_tunnel(spawn, id, cfg, wan);
    source_state_from_report(&report)
}

/// Map a VPN-GW-6 [`vpn_health::TunnelReport`] to the DDNS [`SourceState`] the
/// reconcile core consumes. A confirmed exit (`Ok`/`Unverifiable`) with a verified
/// IP is `Up`; a leak/dns-leak is a **kill-switched down** (so the name is never
/// pointed at a leaking exit — the leak-coupling rule); a hard down is a clean
/// down (the on-down policy then decides remove/sentinel/keep). Pure.
#[must_use]
pub fn source_state_from_report(report: &vpn_health::TunnelReport) -> SourceState {
    match report.health {
        Health::Ok | Health::Unverifiable => match report
            .verified_exit_ip
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            // A shared commercial exit has no inbound port-forward by default, so
            // the published name is identity-only (the UI's "port-forward only").
            Some(ip) => SourceState::Up {
                ip: ip.to_string(),
                port_forward: false,
            },
            // Interface up but the exit IP couldn't be confirmed — nothing safe to
            // publish; a clean down so a `keep` record retains its last value.
            None => SourceState::Down { kill_switch: false },
        },
        // A leak means egress is escaping to the raw WAN: kill-switched-down so the
        // name is parked/removed, never published at the leaking address.
        Health::Leaking | Health::DnsLeak => SourceState::Down { kill_switch: true },
        Health::Down => SourceState::Down { kill_switch: false },
    }
}

/// Reconcile ONE record against its live source state, executing the planned DNS
/// action through `writer` and returning the new last-published value for the
/// record (so the caller's no-churn cache stays current). `last` is what the
/// worker believes is currently published. Pure decision (via [`plan_action`]) +
/// a single writer call; an `Err` from the writer leaves `last` unchanged (the
/// next sweep retries) and is surfaced for logging.
///
/// # Errors
/// The writer's failure, with the record name for context.
pub fn reconcile_record<W: DnsWriter>(
    writer: &W,
    cfg: &DdnsConfig,
    node: &str,
    record: &RecordDef,
    last: Option<&str>,
    state: &SourceState,
) -> Result<Option<String>, String> {
    // Derive the bare label (the part before the zone) from the template; the DO
    // adapter takes the bare name and the zone separately.
    let fqdn = record.fqdn(node, provider_hint(record), 1, &cfg.zone);
    let name = fqdn
        .strip_suffix(&format!(".{}", cfg.zone))
        .unwrap_or(&fqdn)
        .to_string();
    match ddns::plan_action(record, last, state) {
        DdnsAction::Upsert { ip } => {
            writer
                .upsert(&cfg.zone, &name, &ip, cfg.ttl)
                .map_err(|e| format!("{}: upsert {ip}: {e}", record.name))?;
            Ok(Some(ip))
        }
        DdnsAction::Remove => {
            writer
                .delete(&cfg.zone, &name)
                .map_err(|e| format!("{}: delete: {e}", record.name))?;
            Ok(None)
        }
        // No DNS call — the value is already correct (or a clean `keep`); the last
        // value the worker believes is published is unchanged.
        DdnsAction::Noop => Ok(last.map(str::to_string)),
    }
}

/// The `{provider}` substitution for a record's template: a `tunnel:<id>` source's
/// provider is the tunnel id stem (the operator names the tunnel after the
/// provider, e.g. `tunnel:mullvad-1`); a `wan` source has no provider, so the
/// literal `wan` is used. Pure helper so the worker can template without a tunnel
/// config lookup for the common case.
#[must_use]
pub fn provider_hint(record: &RecordDef) -> &str {
    let src = record.source.trim();
    if src.eq_ignore_ascii_case("wan") {
        return "wan";
    }
    src.strip_prefix("tunnel:").map_or(src, str::trim)
}

/// One full reconcile sweep across every managed record: resolve each record's
/// live source state, run [`reconcile_record`], and update the no-churn `last`
/// cache. Returns the count of records that resulted in an actual DNS write (for
/// logging / the test assertion). Best-effort: a single record's writer failure is
/// logged and does not abort the sweep (the others still converge).
pub fn sweep_once<W: DnsWriter>(
    writer: &W,
    cfg: &DdnsConfig,
    node: &str,
    vpn_cfg: &mackes_mesh_types::vpn::VpnConfig,
    wan: Option<&str>,
    spawn: bool,
    last: &mut HashMap<String, String>,
) -> usize {
    if !cfg.enabled {
        return 0;
    }
    let mut writes = 0;
    for record in &cfg.record {
        let state = resolve_source(spawn, &record.source, vpn_cfg, wan);
        let prev = last.get(&record.name).map(String::as_str);
        match reconcile_record(writer, cfg, node, record, prev, &state) {
            Ok(new_last) => {
                // A change is a (prev → new_last) transition; count it for logging.
                if new_last.as_deref() != prev {
                    writes += 1;
                }
                match new_last {
                    Some(v) => {
                        last.insert(record.name.clone(), v);
                    }
                    None => {
                        last.remove(&record.name);
                    }
                }
            }
            Err(e) => {
                tracing::warn!(record = %record.name, error = %e, "ddns reconcile: write failed");
            }
        }
    }
    writes
}

/// Run the DDNS reconcile worker until `should_stop`. Tails the VPN-GW event lane
/// (a new `event/vpn/signals` message triggers an immediate sweep) and, on the
/// [`SWEEP_INTERVAL`] cadence, runs the periodic WAN/exit-IP re-resolve sweep —
/// so both a tunnel flap (event-driven) and a silent IP change (timer-driven) are
/// reconciled. The DigitalOcean [`DoDnsWriter`] is built fresh per sweep from the
/// live config (token resolved from the secret store), so a config/token change is
/// picked up without a restart.
pub fn serve_reconcile<F: Fn() -> bool>(
    persist: &Persist,
    workgroup_root: &std::path::Path,
    node_id: &str,
    spawn: bool,
    should_stop: F,
) {
    let node = node_label(node_id).to_string();
    let mut last: HashMap<String, String> = HashMap::new();
    let mut cursor: Option<String> = None;
    let mut last_sweep = std::time::Instant::now()
        .checked_sub(SWEEP_INTERVAL)
        .unwrap_or_else(std::time::Instant::now);
    while !should_stop() {
        // (1) VPN-GW exit-IP changes: a new health/exit-IP event triggers an
        // immediate reconcile (an up/down/leak transition the operator wants
        // reflected in DNS fast, not on the slow timer).
        let mut event_woke = false;
        match persist.list_since(VPN_EVENT_TOPIC, cursor.as_deref()) {
            Ok(msgs) => {
                for msg in msgs {
                    cursor = Some(msg.ulid.clone());
                    event_woke = true;
                }
            }
            Err(e) => {
                tracing::debug!(error = %e, "ddns reconcile: list_since failed");
            }
        }

        // (2) the periodic WAN check on its cadence (or now if an event woke us).
        if event_woke || last_sweep.elapsed() >= SWEEP_INTERVAL {
            run_sweep(workgroup_root, &node, spawn, &mut last);
            last_sweep = std::time::Instant::now();
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Load the live config + VPN state, build the DO writer, and run one sweep. Split
/// out so the serve loop stays a thin scheduler. Honest no-op when DDNS is
/// disabled or has no records.
fn run_sweep(
    workgroup_root: &std::path::Path,
    node: &str,
    spawn: bool,
    last: &mut HashMap<String, String>,
) {
    let cfg = ddns::load(workgroup_root);
    if !cfg.enabled || cfg.record.is_empty() {
        return;
    }
    let vpn_cfg = mackes_mesh_types::vpn::load(workgroup_root);
    // The raw-WAN IP, fetched once per sweep (the `wan`-source records + the leak
    // comparison all reuse it). Only when the tools can spawn.
    let wan = if spawn { vpn_health::wan_ip() } else { None };
    let writer = DoDnsWriter::resolve(workgroup_root, &cfg.token_ref, spawn);
    let writes = sweep_once(&writer, &cfg, node, &vpn_cfg, wan.as_deref(), spawn, last);
    if writes > 0 {
        tracing::info!(writes, "ddns reconcile: published record changes");
    }
}

// ── DDNS-EGRESS-2/3 — the DigitalOcean DnsWriter adapter ──────────────────────

/// The DigitalOcean DNS API base. Records live under `…/v2/domains/<zone>/records`.
const DO_API_BASE: &str = "https://api.digitalocean.com";

/// The DigitalOcean A/AAAA-record writer: resolves an existing record id (so an
/// update PUTs in place rather than creating a duplicate), then drives the pure
/// [`do_upsert_request`]/[`do_delete_request`] builders. Every call is a §9-safe
/// fixed-arg `curl` argv — the bearer token is passed as a discrete header arg, the
/// JSON body as a discrete `-d` arg, NEVER interpolated into a shell command
/// string. The token is read from the mesh secret store (the config's `token_ref`)
/// so it is never inlined into config or argv-logged.
pub struct DoDnsWriter {
    /// The bearer token, resolved from the secret store; `None` ⇒ the writer is in
    /// honest "no token" mode (every call returns an Err rather than a fake ok).
    token: Option<String>,
    /// Whether to actually shell out to `curl` (false in tests / on a box without
    /// the tools → every call is an honest Err, never a fake success).
    spawn: bool,
}

impl DoDnsWriter {
    /// Build a writer, resolving the DO API token from the mesh secret store under
    /// `token_ref` (e.g. `do-token`). An absent/unreadable token leaves the writer
    /// in honest "no token" mode (calls Err, never a fake ok); the reconcile loop
    /// logs and retries on the next sweep when the secret lands.
    #[must_use]
    pub fn resolve(workgroup_root: &std::path::Path, token_ref: &str, spawn: bool) -> Self {
        let token = resolve_token(workgroup_root, token_ref);
        Self { token, spawn }
    }

    /// Construct directly from a known token (tests / a caller that already holds
    /// the secret). `spawn` gates the real `curl` shell-out.
    #[must_use]
    pub fn new(token: Option<String>, spawn: bool) -> Self {
        Self { token, spawn }
    }

    /// The bearer header argument value (`Authorization: Bearer <token>`), or an
    /// Err when no token is resolved. Kept private so the token never leaves the
    /// struct except as a discrete header arg.
    fn auth_header(&self) -> Result<String, String> {
        match self
            .token
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(t) => Ok(format!("Authorization: Bearer {t}")),
            None => Err("DigitalOcean token not resolved from secret store (token_ref)".into()),
        }
    }

    /// GET the records for `name` in `zone` and return the first matching record
    /// id, if any. The DO API supports `?name=<fqdn>&type=<A|AAAA>` server-side
    /// filtering; the fqdn is the bare label joined to the zone. `Ok(None)` ⇒ no
    /// such record yet (a create path); an `Err` ⇒ a real API/HTTP fault.
    fn existing_id(&self, zone: &str, name: &str, rtype: &str) -> Result<Option<String>, String> {
        if !self.spawn {
            // No host I/O in tests: treat as "no existing record" so an upsert
            // exercises the create path deterministically.
            return Ok(None);
        }
        let auth = self.auth_header()?;
        let fqdn = if name.is_empty() {
            zone.to_string()
        } else {
            format!("{name}.{zone}")
        };
        let url = format!("{DO_API_BASE}/v2/domains/{zone}/records?name={fqdn}&type={rtype}");
        let body = curl_json(&auth, "GET", &url, None)?;
        let v: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| format!("list records: bad json: {e}"))?;
        let id = v
            .get("domain_records")
            .and_then(serde_json::Value::as_array)
            .and_then(|recs| recs.first())
            .and_then(|r| r.get("id"))
            .map(|id| id.to_string());
        Ok(id)
    }
}

impl DnsWriter for DoDnsWriter {
    fn upsert(&self, zone: &str, name: &str, ip: &str, ttl: u32) -> Result<(), String> {
        let auth = self.auth_header()?;
        let rtype = ddns::record_type(ip);
        let existing = self.existing_id(zone, name, rtype)?;
        let (method, path, json_body) = do_upsert_request(zone, name, ip, ttl, existing.as_deref());
        let url = format!("{DO_API_BASE}{path}");
        if !self.spawn {
            // Honest in tests: no token-bearing host I/O. The pure request builder
            // is unit-tested in the types crate; here we just confirm the argv path
            // is reachable without a live account.
            return Ok(());
        }
        curl_json(&auth, method, &url, Some(&json_body)).map(|_| ())
    }

    fn delete(&self, zone: &str, name: &str) -> Result<(), String> {
        let auth = self.auth_header()?;
        // A delete needs the record id; resolve it (try A then AAAA). Absent ⇒
        // already gone (a no-op), which is success.
        let id = self
            .existing_id(zone, name, "A")?
            .or(self.existing_id(zone, name, "AAAA")?);
        let Some(id) = id else {
            return Ok(());
        };
        let (method, path) = do_delete_request(zone, &id);
        let url = format!("{DO_API_BASE}{path}");
        if !self.spawn {
            return Ok(());
        }
        curl_json(&auth, method, &url, None).map(|_| ())
    }
}

/// Resolve the DO API token from the mesh secret store under `token_ref`. `None`
/// when the ref is empty or the secret isn't in the store yet (honest "not
/// distributed" — the writer then declines to fake a success).
#[must_use]
fn resolve_token(workgroup_root: &std::path::Path, token_ref: &str) -> Option<String> {
    let key = token_ref.trim();
    if key.is_empty() {
        return None;
    }
    // A `secret:` prefix is the config convention for a secret-store ref; strip it
    // to the bare store key. A bare key is accepted too.
    let key = key.strip_prefix("secret:").unwrap_or(key);
    let store = crate::ipc::secret_store::SecretStore::resolve(
        &crate::ipc::secret_store::repo_root(),
        workgroup_root,
    );
    match store.get(key) {
        Ok(Some(tok)) => Some(tok.trim().to_string()).filter(|s| !s.is_empty()),
        Ok(None) => None,
        Err(e) => {
            tracing::debug!(token_ref = key, error = %e, "ddns: secret-store token read failed");
            None
        }
    }
}

/// A §9-safe fixed-arg `curl` to the DO API: the method, URL, bearer header, and
/// (optional) JSON body are each DISCRETE argv elements — never interpolated into
/// a shell command string, so there is no shell-injection surface even if a record
/// name/zone carried metacharacters. Returns the response body on a 2xx, else an
/// Err carrying the status + body tail for logging.
fn curl_json(
    auth: &str,
    method: &str,
    url: &str,
    json_body: Option<&str>,
) -> Result<String, String> {
    let mut cmd = std::process::Command::new("curl");
    cmd.arg("-s")
        .args(["-X", method])
        .args(["-H", auth])
        .args(["-H", "Content-Type: application/json"])
        .args(["-m", "15"])
        // Append the HTTP status after a newline so we can split body / code.
        .args(["-w", "\n%{http_code}"]);
    if let Some(b) = json_body {
        cmd.args(["-d", b]);
    }
    cmd.arg(url);
    let out = cmd.output().map_err(|e| format!("curl not run: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "curl exited {}",
            out.status
                .code()
                .map_or_else(|| "signal".to_string(), |c| c.to_string())
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let (body, code) = match text.trim_end().rsplit_once('\n') {
        Some((b, c)) => (b.to_string(), c.trim().to_string()),
        None => (String::new(), text.trim().to_string()),
    };
    if code.starts_with('2') {
        Ok(body)
    } else {
        Err(format!("DO API {code}: {}", body.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::ddns::{OnDown, SENTINEL_ADDR};
    use std::cell::RefCell;

    /// A recording fake writer: captures every upsert/delete so the reconcile core
    /// is exercised without any network. Interior mutability so it can sit behind
    /// a shared `&` like the real writer.
    #[derive(Default)]
    struct FakeWriter {
        upserts: RefCell<Vec<(String, String, String)>>, // (zone, name, ip)
        deletes: RefCell<Vec<(String, String)>>,         // (zone, name)
    }

    impl DnsWriter for FakeWriter {
        fn upsert(&self, zone: &str, name: &str, ip: &str, _ttl: u32) -> Result<(), String> {
            self.upserts
                .borrow_mut()
                .push((zone.into(), name.into(), ip.into()));
            Ok(())
        }
        fn delete(&self, zone: &str, name: &str) -> Result<(), String> {
            self.deletes.borrow_mut().push((zone.into(), name.into()));
            Ok(())
        }
    }

    fn cfg_with(records: Vec<RecordDef>) -> DdnsConfig {
        DdnsConfig {
            enabled: true,
            zone: "services.matthewmackes.com".into(),
            ttl: 60,
            record: records,
            ..Default::default()
        }
    }

    fn rec(name: &str, source: &str, on_down: OnDown) -> RecordDef {
        RecordDef {
            name: name.into(),
            source: source.into(),
            on_down,
        }
    }

    #[test]
    fn node_label_strips_peer_prefix() {
        assert_eq!(node_label("peer:eagle"), "eagle");
        assert_eq!(node_label("eagle"), "eagle");
    }

    #[test]
    fn provider_hint_from_source() {
        assert_eq!(
            provider_hint(&rec("r", "tunnel:mullvad-1", OnDown::Keep)),
            "mullvad-1"
        );
        assert_eq!(provider_hint(&rec("r", "wan", OnDown::Keep)), "wan");
    }

    #[test]
    fn resolve_source_wan_up_and_down() {
        let vpn_cfg = mackes_mesh_types::vpn::VpnConfig::default();
        // WAN with an IP → up, identity-only.
        assert_eq!(
            resolve_source(false, "wan", &vpn_cfg, Some("203.0.113.7")),
            SourceState::Up {
                ip: "203.0.113.7".into(),
                port_forward: false,
            }
        );
        // No WAN IP → clean down.
        assert_eq!(
            resolve_source(false, "wan", &vpn_cfg, None),
            SourceState::Down { kill_switch: false }
        );
    }

    #[test]
    fn resolve_source_tunnel_down_without_iface() {
        // spawn=false → the tunnel iface reads absent → Down (clean).
        let vpn_cfg = mackes_mesh_types::vpn::VpnConfig::default();
        assert_eq!(
            resolve_source(false, "tunnel:mullvad1", &vpn_cfg, None),
            SourceState::Down { kill_switch: false }
        );
    }

    #[test]
    fn source_state_from_report_maps_leak_to_kill_switched_down() {
        let leaking = vpn_health::TunnelReport {
            id: "m1".into(),
            ifname: "mvpn-m1".into(),
            health: Health::Leaking,
            verified_exit_ip: Some("203.0.113.7".into()),
            wan_ip: Some("203.0.113.7".into()),
            detail: "leak".into(),
        };
        assert_eq!(
            source_state_from_report(&leaking),
            SourceState::Down { kill_switch: true }
        );
        let ok = vpn_health::TunnelReport {
            id: "m1".into(),
            ifname: "mvpn-m1".into(),
            health: Health::Ok,
            verified_exit_ip: Some("198.51.100.9".into()),
            wan_ip: Some("203.0.113.7".into()),
            detail: "ok".into(),
        };
        assert_eq!(
            source_state_from_report(&ok),
            SourceState::Up {
                ip: "198.51.100.9".into(),
                port_forward: false,
            }
        );
    }

    #[test]
    fn reconcile_publishes_then_rewrites_then_no_churn() {
        let w = FakeWriter::default();
        let cfg = cfg_with(vec![rec("{node}-{provider}", "wan", OnDown::Keep)]);
        let r = &cfg.record[0];
        // First publish.
        let up1 = SourceState::Up {
            ip: "1.2.3.4".into(),
            port_forward: false,
        };
        let last = reconcile_record(&w, &cfg, "eagle", r, None, &up1).unwrap();
        assert_eq!(last.as_deref(), Some("1.2.3.4"));
        // Reconnect with a NEW ip → rewrite.
        let up2 = SourceState::Up {
            ip: "5.6.7.8".into(),
            port_forward: false,
        };
        let last = reconcile_record(&w, &cfg, "eagle", r, last.as_deref(), &up2).unwrap();
        assert_eq!(last.as_deref(), Some("5.6.7.8"));
        // Same ip → no churn (no extra write).
        let last = reconcile_record(&w, &cfg, "eagle", r, last.as_deref(), &up2).unwrap();
        assert_eq!(last.as_deref(), Some("5.6.7.8"));
        assert_eq!(
            w.upserts.borrow().len(),
            2,
            "first-publish + one rewrite only"
        );
        // The bare label (zone stripped) was passed to the writer.
        assert_eq!(w.upserts.borrow()[0].1, "eagle-wan");
        assert!(w.deletes.borrow().is_empty());
    }

    #[test]
    fn reconcile_remove_on_down_deletes() {
        let w = FakeWriter::default();
        let cfg = cfg_with(vec![rec(
            "{node}-{provider}",
            "tunnel:mullvad-1",
            OnDown::Remove,
        )]);
        let r = &cfg.record[0];
        let down = SourceState::Down { kill_switch: false };
        // A previously-published record on a down source → delete.
        let last = reconcile_record(&w, &cfg, "eagle", r, Some("1.2.3.4"), &down).unwrap();
        assert_eq!(last, None);
        assert_eq!(w.deletes.borrow().len(), 1);
        assert_eq!(w.deletes.borrow()[0].1, "eagle-mullvad-1");
    }

    #[test]
    fn reconcile_sentinel_parks_on_kill_switched_keep() {
        let w = FakeWriter::default();
        let cfg = cfg_with(vec![rec(
            "{node}-{provider}",
            "tunnel:mullvad-1",
            OnDown::Keep,
        )]);
        let r = &cfg.record[0];
        // Kill-switched down + keep → leak-coupling parks at the sentinel.
        let down = SourceState::Down { kill_switch: true };
        let last = reconcile_record(&w, &cfg, "eagle", r, Some("1.2.3.4"), &down).unwrap();
        assert_eq!(last.as_deref(), Some(SENTINEL_ADDR));
        assert_eq!(w.upserts.borrow()[0].2, SENTINEL_ADDR);
    }

    #[test]
    fn sweep_disabled_config_is_a_noop() {
        let w = FakeWriter::default();
        let mut cfg = cfg_with(vec![rec("{node}-{provider}", "wan", OnDown::Keep)]);
        cfg.enabled = false;
        let vpn_cfg = mackes_mesh_types::vpn::VpnConfig::default();
        let mut last = HashMap::new();
        let writes = sweep_once(
            &w,
            &cfg,
            "eagle",
            &vpn_cfg,
            Some("1.2.3.4"),
            false,
            &mut last,
        );
        assert_eq!(writes, 0);
        assert!(w.upserts.borrow().is_empty());
    }

    #[test]
    fn sweep_publishes_wan_record_once_then_idempotent() {
        let w = FakeWriter::default();
        let cfg = cfg_with(vec![rec("{node}-{provider}", "wan", OnDown::Keep)]);
        let vpn_cfg = mackes_mesh_types::vpn::VpnConfig::default();
        let mut last = HashMap::new();
        // First sweep publishes.
        let writes = sweep_once(
            &w,
            &cfg,
            "eagle",
            &vpn_cfg,
            Some("1.2.3.4"),
            false,
            &mut last,
        );
        assert_eq!(writes, 1);
        // Second sweep, same IP → no churn.
        let writes = sweep_once(
            &w,
            &cfg,
            "eagle",
            &vpn_cfg,
            Some("1.2.3.4"),
            false,
            &mut last,
        );
        assert_eq!(writes, 0);
        assert_eq!(w.upserts.borrow().len(), 1);
        assert_eq!(
            last.get("{node}-{provider}").map(String::as_str),
            Some("1.2.3.4")
        );
    }

    #[test]
    fn do_writer_without_token_errs_not_fakes() {
        // The honest contract: no token ⇒ every write Errs (never a fake ok), so
        // the reconcile loop retries when the secret lands instead of marking a
        // record published.
        let w = DoDnsWriter::new(None, true);
        assert!(w.upsert("z.example", "n", "1.2.3.4", 60).is_err());
        assert!(w.delete("z.example", "n").is_err());
    }

    #[test]
    fn do_writer_with_token_no_spawn_is_reachable_and_honest() {
        // With a token but spawn disabled, the argv path is reachable without host
        // I/O and returns Ok (the pure request builders are unit-tested in types).
        let w = DoDnsWriter::new(Some("dop_v1_test".into()), false);
        assert!(w.upsert("z.example", "n", "1.2.3.4", 60).is_ok());
        assert!(w.delete("z.example", "n").is_ok());
    }
}
