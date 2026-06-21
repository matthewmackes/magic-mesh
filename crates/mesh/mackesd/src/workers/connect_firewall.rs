//! CONNECT-3 — exposure-driven firewall enforcement (additive).
//!
//! Makes `expose` (the CONNECT exposure model) actually take effect at the
//! network layer: on the lighthouse a public-via-ingress service is bound to,
//! open the ingress ports it needs on the `public` (underlay) firewalld zone, so
//! the Caddy reverse proxy / stream forwarder can accept the public traffic. The
//! foundational deny (firewalld's `public` zone already drops everything not
//! explicitly opened) + the Nebula/SSH/enroll allows are owned by
//! [`super::firewall_preset`]; this worker layers the **policy-driven** ingress
//! openings on top.
//!
//! Safety: the reconcile is **bounded to CONNECT's own openings**. The worker
//! records exactly the ports + stream forwards it last applied in a node-local
//! state file ([`CONNECT_STATE_DIR`]); on the next tick it removes only entries
//! that are in *that* file but no longer wanted by the policy (an unexpose).
//! Because it only ever removes rules it previously added, it can never close
//! SSH / Nebula / enroll — those are owned by [`super::firewall_preset`] +
//! firewalld's public-zone default and never enter CONNECT's state. The shared
//! HTTP ingress ports (80/443) are additive-only (never auto-removed) so a flap
//! with firewall_preset / Caddy is impossible; only service-specific raw stream
//! ports + their forward-ports are reclaimed on unexpose.

#![cfg(feature = "async-services")]

use std::path::PathBuf;
use std::time::Duration;

use mackes_mesh_types::exposure::{self, ProtoMode};
use serde::{Deserialize, Serialize};

use super::{ShutdownToken, Worker};

/// Node-local dir holding CONNECT's applied-firewall reconcile state (the set of
/// ports + forwards this worker last opened, so an unexpose can reclaim exactly
/// those — see the module safety note). Mirrors the `/var/lib/mackesd` local
/// state convention (node-signing key, passcode cred).
pub const CONNECT_STATE_DIR: &str = "/var/lib/mackesd/connect";

/// The shared HTTP ingress ports — additive-only, never auto-removed (they may be
/// shared with `firewall_preset` / the Caddy listener; reclaiming them is not
/// worth a flap). Service-specific raw stream ports ARE reclaimed on unexpose.
const SHARED_HTTP_PORTS: [(u16, &str); 2] = [(80, "tcp"), (443, "tcp")];

/// CONNECT-3 — the public-zone ports that are legitimately open but are NOT
/// CONNECT's to manage: the foundational always-public layer (§1/§6 — SSH/22,
/// Nebula/4242, enroll/4243, owned by [`super::firewall_preset`] + firewalld's
/// public-zone default) plus the shared HTTP ingress (80/443, owned by Caddy +
/// shared services). A port open on the public zone that is outside this set
/// **and** outside CONNECT's own openings is *unexpected* — a rogue or forgotten
/// public opening the operator should be alerted to.
const EXPECTED_FOUNDATIONAL_PORTS: [(u16, &str); 5] = [
    (22, "tcp"),
    (4242, "udp"),
    (4243, "tcp"),
    (80, "tcp"),
    (443, "tcp"),
];

/// Tick cadence — exposure policy changes are rare; a minute keeps a freshly
/// exposed service reachable quickly without polling storms.
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(60);

/// The firewalld zone the underlay (internet-facing) NIC lives in — the same
/// `public` zone [`super::firewall_preset`] binds the underlay to (W70).
pub const PUBLIC_ZONE: &str = super::firewall_preset::UNDERLAY_ZONE;

/// CONNECT-3 — the set of ingress ports (`(port, proto)`) the `public` zone must
/// open on `hostname` for every public-via-ingress service bound to it:
///   * `http` services → the shared Caddy `80`/`443` (auto-TLS) — added once.
///   * `tcp`/`udp` services → the service's own port as a raw stream.
/// Only services whose `ingress.lighthouse == hostname` count (the ingress
/// terminates there). Deduped + sorted for stable comparison/tests. Pure.
#[must_use]
pub fn desired_ingress_ports(cfg: &exposure::ExposureConfig, hostname: &str) -> Vec<(u16, String)> {
    let mut ports: Vec<(u16, String)> = Vec::new();
    for s in cfg.public_services() {
        let bound_here = s.ingress.as_ref().is_some_and(|b| b.lighthouse == hostname);
        if !bound_here {
            continue;
        }
        match s.mode {
            ProtoMode::Http => {
                ports.push((80, "tcp".to_string()));
                ports.push((443, "tcp".to_string()));
            }
            ProtoMode::Tcp => ports.push((s.source.port, "tcp".to_string())),
            ProtoMode::Udp => ports.push((s.source.port, "udp".to_string())),
        }
    }
    ports.sort();
    ports.dedup();
    ports
}

/// CONNECT-4 — default dir for the rendered Caddy ingress fragment. The lighthouse
/// Caddy setup `import`s `*.caddy` from here (the install/import wiring is the
/// packaging follow-on); writing the fragment + reloading Caddy is this worker.
pub const CADDY_FRAGMENT_DIR: &str = "/etc/caddy/Caddyfile.d";

/// CONNECT-5 — raw TCP/UDP stream forwards for this lighthouse: `(public_port,
/// proto, service_overlay_ip)` for every public **tcp/udp** service bound here,
/// resolving the hosting node's overlay IP via `resolve`. Streams are forwarded
/// at the firewall (firewalld `forward-port` → the overlay endpoint) rather than
/// through Caddy — no `caddy-l4` plugin needed. A node whose overlay IP can't be
/// resolved yet is skipped (it reconciles on a later tick). Deterministic. Pure.
#[must_use]
pub fn desired_forwards(
    cfg: &exposure::ExposureConfig,
    lighthouse: &str,
    resolve: impl Fn(&str) -> Option<String>,
) -> Vec<(u16, String, String)> {
    let mut fwds: Vec<(u16, String, String)> = cfg
        .public_services()
        .into_iter()
        .filter(|s| matches!(s.mode, ProtoMode::Tcp | ProtoMode::Udp))
        .filter(|s| {
            s.ingress
                .as_ref()
                .is_some_and(|b| b.lighthouse == lighthouse)
        })
        .filter_map(|s| {
            let proto = if s.mode == ProtoMode::Udp {
                "udp"
            } else {
                "tcp"
            };
            let ip = resolve(&s.source.node)?;
            Some((s.source.port, proto.to_string(), ip))
        })
        .collect();
    fwds.sort();
    fwds.dedup();
    fwds
}

/// CONNECT's node-local record of the firewall rules it last applied, so the next
/// tick can reclaim exactly those on an unexpose (see the module safety note).
/// Serialized as JSON at `<state_dir>/applied-firewall.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedState {
    /// `(port, proto)` raw openings this worker added via `--add-port`.
    #[serde(default)]
    pub ports: Vec<(u16, String)>,
    /// `(port, proto, toaddr)` stream forwards this worker added.
    #[serde(default)]
    pub forwards: Vec<(u16, String, String)>,
    /// CONNECT-3 — the unexpected public-zone openings last alerted on, so the
    /// alert is **edge-triggered**: it fires when the set changes and clears when
    /// the rogue port is removed, instead of spamming every tick.
    #[serde(default)]
    pub alerted_unexpected: Vec<(u16, String)>,
}

/// Ports CONNECT previously opened that the policy no longer wants — the unexpose
/// reclaim set. Excludes the shared HTTP ports (80/443), which stay additive-only.
/// Pure + testable; the bounded-removal core of the drift-correction.
#[must_use]
pub fn ports_to_remove(prev: &[(u16, String)], desired: &[(u16, String)]) -> Vec<(u16, String)> {
    prev.iter()
        .filter(|p| !desired.contains(p))
        .filter(|(port, proto)| {
            !SHARED_HTTP_PORTS
                .iter()
                .any(|(sp, spr)| *sp == *port && *spr == proto.as_str())
        })
        .cloned()
        .collect()
}

/// Stream forwards CONNECT previously added that the policy no longer wants.
/// Forwards are always CONNECT-owned (firewall_preset never makes forward-ports),
/// so all stale entries are reclaimed. Pure + testable.
#[must_use]
pub fn forwards_to_remove(
    prev: &[(u16, String, String)],
    desired: &[(u16, String, String)],
) -> Vec<(u16, String, String)> {
    prev.iter()
        .filter(|f| !desired.contains(f))
        .cloned()
        .collect()
}

/// CONNECT-3 — parse `firewall-cmd --zone=<z> --list-ports` output: a
/// whitespace-separated list of `port/proto` tokens (e.g. `"80/tcp 443/tcp
/// 4242/udp"`). Malformed tokens are skipped. Pure + testable.
#[must_use]
pub fn parse_open_ports(list_output: &str) -> Vec<(u16, String)> {
    list_output
        .split_whitespace()
        .filter_map(|tok| {
            let (port, proto) = tok.split_once('/')?;
            let port: u16 = port.parse().ok()?;
            if proto.is_empty() {
                return None;
            }
            Some((port, proto.to_string()))
        })
        .collect()
}

/// CONNECT-3 — the public-zone ports that are open but neither foundational
/// ([`EXPECTED_FOUNDATIONAL_PORTS`]) nor opened by CONNECT itself (`connect_owned`,
/// this node's desired ingress). These are *unexpected* — a rogue or forgotten
/// public opening the operator should see. Detection only — this never mutates
/// the firewall (closing a port stays bounded to CONNECT's own [`AppliedState`]).
/// Pure + testable; the result is sorted for a stable edge-trigger comparison.
#[must_use]
pub fn unexpected_open_ports(
    open: &[(u16, String)],
    connect_owned: &[(u16, String)],
) -> Vec<(u16, String)> {
    let mut out: Vec<(u16, String)> = open
        .iter()
        .filter(|(port, proto)| {
            let foundational = EXPECTED_FOUNDATIONAL_PORTS
                .iter()
                .any(|(p, pr)| *p == *port && *pr == proto.as_str());
            let owned = connect_owned.iter().any(|(p, pr)| p == port && pr == proto);
            !foundational && !owned
        })
        .cloned()
        .collect();
    out.sort();
    out.dedup();
    out
}

/// CONNECT-3 — publish a Hub alert naming unexpected public-zone openings, on the
/// same `event/firewall/<host>` alert lane the firewall monitor uses (so it
/// surfaces in the Notification Hub). Best-effort fire-and-reap.
fn publish_unexpected_ports_alert(host: &str, unexpected: &[(u16, String)]) {
    let topic = format!("event/firewall/{host}");
    let list = unexpected
        .iter()
        .map(|(p, pr)| format!("{p}/{pr}"))
        .collect::<Vec<_>>()
        .join(",");
    let body = format!(
        r#"{{"host":"{host}","unexpected_public_ports":"{list}","count":{},"alert":true}}"#,
        unexpected.len()
    );
    let mut cmd = std::process::Command::new("mde-bus");
    cmd.args(["publish", &topic, "--body-flag", &body]);
    crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
}

/// The exposure-driven enforcement worker: opens the policy's ingress firewall
/// ports (CONNECT-3), forwards raw TCP/UDP streams to the overlay (CONNECT-5),
/// AND renders/writes this node's Caddy ingress config (CONNECT-4) — one "apply
/// this node's exposure" reconcile per tick. Removals are bounded to CONNECT's
/// own prior openings via [`AppliedState`].
pub struct ConnectFirewallWorker {
    workgroup_root: PathBuf,
    hostname: String,
    tick_interval: Duration,
    /// `firewall-cmd` binary (empty disables the shell-out — for tests).
    firewall_cmd: &'static str,
    /// Dir the Caddy ingress fragment is written to (overridable for tests).
    caddy_dir: PathBuf,
    /// Dir holding the applied-firewall reconcile state (overridable for tests).
    state_dir: PathBuf,
}

impl ConnectFirewallWorker {
    /// Build the worker for `hostname`, reading policy from `workgroup_root`.
    #[must_use]
    pub fn new(workgroup_root: PathBuf, hostname: String) -> Self {
        Self {
            workgroup_root,
            hostname,
            tick_interval: DEFAULT_TICK_INTERVAL,
            firewall_cmd: "firewall-cmd",
            caddy_dir: PathBuf::from(CADDY_FRAGMENT_DIR),
            state_dir: PathBuf::from(CONNECT_STATE_DIR),
        }
    }

    /// Disable the `firewall-cmd` shell-out (tests drive the pure plan instead).
    #[must_use]
    pub fn without_firewall_cmd(mut self) -> Self {
        self.firewall_cmd = "";
        self
    }

    /// Override the Caddy fragment dir (tests).
    #[must_use]
    pub fn with_caddy_dir(mut self, dir: PathBuf) -> Self {
        self.caddy_dir = dir;
        self
    }

    /// Override the reconcile-state dir (tests).
    #[must_use]
    pub fn with_state_dir(mut self, dir: PathBuf) -> Self {
        self.state_dir = dir;
        self
    }

    /// Path of the applied-firewall reconcile state file.
    fn state_path(&self) -> PathBuf {
        self.state_dir.join("applied-firewall.json")
    }

    /// Load the last-applied reconcile state (default/empty on missing/garbage).
    fn load_applied(&self) -> AppliedState {
        std::fs::read_to_string(self.state_path())
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    /// Persist the reconcile state (best-effort; a write failure just means the
    /// next tick re-derives from firewalld's idempotent add — never a wrong remove).
    fn save_applied(&self, state: &AppliedState) {
        if std::fs::create_dir_all(&self.state_dir).is_err() {
            return;
        }
        if let Ok(json) = serde_json::to_string_pretty(state) {
            let _ = std::fs::write(self.state_path(), json);
        }
    }

    /// Resolve this node's desired stream forwards from the peer directory.
    fn compute_forwards(&self, cfg: &exposure::ExposureConfig) -> Vec<(u16, String, String)> {
        let peers = mackes_mesh_types::peers::read_peers(&mackes_mesh_types::peers::peers_dir(
            &self.workgroup_root,
        ));
        let resolve = |node: &str| -> Option<String> {
            peers
                .iter()
                .find(|p| p.hostname == node)
                .and_then(|p| p.overlay_ip.clone())
        };
        desired_forwards(cfg, &self.hostname, resolve)
    }

    /// Run one `firewall-cmd --permanent --zone public <args…>`, bounded; returns
    /// success.
    fn fw(&self, args: &[&str]) -> bool {
        let mut cmd = std::process::Command::new(self.firewall_cmd);
        cmd.arg("--permanent")
            .args(["--zone", PUBLIC_ZONE])
            .args(args);
        crate::workers::proc::status_with_timeout(cmd, crate::workers::proc::DEFAULT_CMD_TIMEOUT)
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// CONNECT-3 — list the `public` zone's **runtime** open ports via
    /// `firewall-cmd --zone <z> --list-ports`. Returns `[]` when firewall-cmd is
    /// disabled (tests) or the call fails. Detection-only — never mutates.
    fn list_public_ports(&self) -> Vec<(u16, String)> {
        if self.firewall_cmd.is_empty() {
            return Vec::new();
        }
        let mut cmd = std::process::Command::new(self.firewall_cmd);
        cmd.args(["--zone", PUBLIC_ZONE, "--list-ports"]);
        match crate::workers::proc::output_with_timeout(
            cmd,
            crate::workers::proc::DEFAULT_CMD_TIMEOUT,
        ) {
            Ok(out) if out.status.success() => {
                parse_open_ports(&String::from_utf8_lossy(&out.stdout))
            }
            _ => Vec::new(),
        }
    }

    /// CONNECT-4 — render this node's Caddy ingress fragment from the policy and
    /// write it (on-change) to `<caddy_dir>/mcnf-ingress.caddy`, then reload Caddy
    /// if it's installed + the fragment changed. Best-effort + safe: only writes
    /// MCNF's own managed fragment, never the operator's Caddyfile. Returns `true`
    /// if the fragment was (re)written. Skips entirely when the dir's parent
    /// doesn't exist (no Caddy on this node).
    fn apply_caddy(&self, cfg: &exposure::ExposureConfig) -> bool {
        // Only manage Caddy where its config dir exists (i.e. Caddy is installed).
        if self.caddy_dir.parent().is_some_and(|p| !p.exists()) && !self.caddy_dir.exists() {
            return false;
        }
        let rendered = exposure::render_caddyfile(cfg, &self.hostname);
        let path = self.caddy_dir.join("mcnf-ingress.caddy");
        let current = std::fs::read_to_string(&path).unwrap_or_default();
        if current == rendered {
            return false; // unchanged
        }
        if std::fs::create_dir_all(&self.caddy_dir).is_err() {
            return false;
        }
        if std::fs::write(&path, &rendered).is_err() {
            return false;
        }
        // Reload Caddy if present (best-effort; bounded).
        let mut reload = std::process::Command::new("systemctl");
        reload.args(["reload", "caddy"]);
        let _ = crate::workers::proc::status_with_timeout(
            reload,
            crate::workers::proc::DEFAULT_CMD_TIMEOUT,
        );
        tracing::info!(path = %path.display(), "connect_firewall: wrote Caddy ingress fragment (CONNECT-4)");
        true
    }

    /// One enforcement tick: load the policy, render Caddy (CONNECT-4), then
    /// reconcile this node's firewalld openings — **add** the policy's desired
    /// ingress ports (CONNECT-3) + stream forwards (CONNECT-5), and **remove**
    /// exactly the ports/forwards CONNECT itself opened on a prior tick that the
    /// policy no longer wants (the unexpose reclaim, bounded by [`AppliedState`]).
    /// Returns the number of `firewall-cmd` mutations (adds + removes) that ran
    /// (0 when nothing changed / Caddy-only / no firewalld).
    pub fn tick_once(&self) -> usize {
        let cfg = exposure::load(&self.workgroup_root);
        // CONNECT-4 — render/write this node's Caddy ingress fragment (runs even
        // when no firewall ports change, e.g. to clear the fragment after an
        // unexpose). Best-effort + no-op when Caddy isn't installed here.
        let _ = self.apply_caddy(&cfg);

        let desired_ports = desired_ingress_ports(&cfg, &self.hostname);
        let desired_fwds = self.compute_forwards(&cfg);
        if self.firewall_cmd.is_empty() {
            return 0;
        }

        let prev = self.load_applied();
        let rm_ports = ports_to_remove(&prev.ports, &desired_ports);
        let rm_fwds = forwards_to_remove(&prev.forwards, &desired_fwds);
        let mut changed = 0usize;

        // forward-port to a remote (overlay) addr needs masquerade for the return
        // path — ensure it whenever any forward is desired (idempotent).
        if !desired_fwds.is_empty() && self.fw(&["--add-masquerade"]) {
            // masquerade toggle isn't counted as a port change, but a successful
            // first-time add still warrants the reload below.
            changed = changed.max(0);
        }

        // ── Removals first (reclaim what the policy dropped) ───────────
        for (port, proto, ip) in &rm_fwds {
            let spec = format!("port={port}:proto={proto}:toaddr={ip}:toport={port}");
            if self.fw(&["--remove-forward-port", &spec]) {
                changed += 1;
            }
        }
        for (port, proto) in &rm_ports {
            if self.fw(&["--remove-port", &format!("{port}/{proto}")]) {
                changed += 1;
            }
        }

        // ── Additions (idempotent — re-running is a no-op at firewalld) ─
        for (port, proto, ip) in &desired_fwds {
            let spec = format!("port={port}:proto={proto}:toaddr={ip}:toport={port}");
            if self.fw(&["--add-forward-port", &spec]) {
                changed += 1;
            }
        }
        for (port, proto) in &desired_ports {
            if self.fw(&["--add-port", &format!("{port}/{proto}")]) {
                changed += 1;
            }
        }

        // CONNECT-3 — detect + alert on UNEXPECTED public openings: anything open
        // on the public zone that is neither foundational (SSH/Nebula/enroll/HTTP)
        // nor CONNECT's own ingress. Edge-triggered off the persisted set so it
        // fires on change + clears when resolved (no per-tick spam). Detection
        // only — it never closes a port (removal stays bounded to CONNECT's own
        // state, so it can't fight firewall_preset or lock out the foundation).
        let unexpected = unexpected_open_ports(&self.list_public_ports(), &desired_ports);
        if !unexpected.is_empty() && unexpected != prev.alerted_unexpected {
            publish_unexpected_ports_alert(&self.hostname, &unexpected);
            tracing::warn!(
                ports = ?unexpected,
                "connect_firewall: UNEXPECTED public-zone openings — raised event/firewall alert (CONNECT-3)"
            );
        }

        // Persist what we now intend to own so the next tick can reclaim drift.
        self.save_applied(&AppliedState {
            ports: desired_ports.clone(),
            forwards: desired_fwds.clone(),
            alerted_unexpected: unexpected,
        });

        if changed > 0 || !rm_fwds.is_empty() || !rm_ports.is_empty() {
            let mut reload = std::process::Command::new(self.firewall_cmd);
            reload.arg("--reload");
            let _ = crate::workers::proc::status_with_timeout(
                reload,
                crate::workers::proc::DEFAULT_CMD_TIMEOUT,
            );
            tracing::info!(
                added_ports = desired_ports.len(),
                added_forwards = desired_fwds.len(),
                removed_ports = rm_ports.len(),
                removed_forwards = rm_fwds.len(),
                "connect_firewall: reconciled the public-zone ingress from policy (CONNECT-3/5)"
            );
        }
        changed
    }
}

#[async_trait::async_trait]
impl Worker for ConnectFirewallWorker {
    fn name(&self) -> &'static str {
        "connect_firewall"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            let _ = self.tick_once();
            tokio::select! {
                _ = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(self.tick_interval) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::exposure::{
        ExposureConfig, ExposurePolicy, IngressBinding, ServiceSource, Tier,
    };

    fn public_svc(id: &str, lh: &str, port: u16, mode: ProtoMode) -> ExposurePolicy {
        ExposurePolicy {
            id: id.into(),
            source: ServiceSource {
                node: "n".into(),
                port,
                ..Default::default()
            },
            tier: Tier::PublicViaIngress,
            ingress: Some(IngressBinding {
                lighthouse: lh.into(),
                hostname: format!("{id}.example"),
            }),
            mode,
            template: None,
        }
    }

    #[test]
    fn http_service_opens_80_443_only_on_its_lighthouse() {
        let mut cfg = ExposureConfig::default();
        cfg.upsert(public_svc("web", "LH-01", 3000, ProtoMode::Http));
        // Bound to LH-01 → 80/443 here…
        assert_eq!(
            desired_ingress_ports(&cfg, "LH-01"),
            vec![(80, "tcp".into()), (443, "tcp".into())]
        );
        // …nothing on a different node.
        assert!(desired_ingress_ports(&cfg, "LH-02").is_empty());
    }

    #[test]
    fn tcp_udp_services_open_their_own_port() {
        let mut cfg = ExposureConfig::default();
        cfg.upsert(public_svc("game", "LH-01", 25565, ProtoMode::Tcp));
        cfg.upsert(public_svc("voice", "LH-01", 10000, ProtoMode::Udp));
        let mut got = desired_ingress_ports(&cfg, "LH-01");
        got.sort();
        assert_eq!(got, vec![(10000, "udp".into()), (25565, "tcp".into())]);
    }

    #[test]
    fn mesh_only_services_open_nothing() {
        let mut cfg = ExposureConfig::default();
        cfg.upsert(ExposurePolicy {
            id: "internal".into(),
            tier: Tier::MeshOnly,
            ..Default::default()
        });
        assert!(desired_ingress_ports(&cfg, "LH-01").is_empty());
    }

    #[test]
    fn desired_forwards_resolves_overlay_ip_for_streams_only() {
        // CONNECT-5 — tcp/udp public services bound here get a forward to the
        // node's overlay IP; http services + unresolved nodes are excluded.
        let mut cfg = ExposureConfig::default();
        cfg.upsert(public_svc("game", "LH-01", 25565, ProtoMode::Tcp)); // node "n"
        cfg.upsert(public_svc("web", "LH-01", 3000, ProtoMode::Http)); // http → not a forward
        let resolve = |node: &str| (node == "n").then(|| "10.42.0.9".to_string());
        let fwds = desired_forwards(&cfg, "LH-01", resolve);
        assert_eq!(fwds, vec![(25565, "tcp".into(), "10.42.0.9".into())]);
        // Unresolvable node → skipped (reconciles later).
        let none = desired_forwards(&cfg, "LH-01", |_| None);
        assert!(none.is_empty());
    }

    #[test]
    fn default_deny_no_public_surface_without_explicit_policy() {
        // CONNECT-10 — the public-boundary default-deny invariant FROM OUR CODE:
        // with no exposure policy at all, this worker opens ZERO public ports on
        // any node. The internet surface only widens for an explicitly-exposed
        // service; the foundational SSH/Nebula/enroll allows are owned by
        // firewall_preset + firewalld's public-zone default, never removed here.
        let empty = ExposureConfig::default();
        assert!(desired_ingress_ports(&empty, "LH-01").is_empty());
        assert!(desired_ingress_ports(&empty, "any-node").is_empty());
    }

    #[test]
    fn apply_caddy_writes_fragment_for_bound_http_service() {
        // CONNECT-4 — the worker renders + writes this node's Caddy fragment.
        let tmp = tempfile::tempdir().unwrap();
        let caddy = tempfile::tempdir().unwrap(); // exists ⇒ "Caddy installed"
        let mut cfg = ExposureConfig::default();
        cfg.upsert(public_svc("web", "LH-01", 3000, ProtoMode::Http));
        exposure::save(tmp.path(), &cfg).unwrap();
        let w = ConnectFirewallWorker::new(tmp.path().to_path_buf(), "LH-01".into())
            .without_firewall_cmd()
            .with_caddy_dir(caddy.path().to_path_buf());
        let _ = w.tick_once();
        let frag = std::fs::read_to_string(caddy.path().join("mcnf-ingress.caddy")).unwrap();
        assert!(frag.contains("web.example {"), "{frag}");
        assert!(frag.contains("reverse_proxy n.mesh:3000"), "{frag}");
    }

    #[test]
    fn tick_is_noop_without_firewall_cmd() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = ExposureConfig::default();
        cfg.upsert(public_svc("web", "LH-01", 3000, ProtoMode::Http));
        exposure::save(tmp.path(), &cfg).unwrap();
        let w = ConnectFirewallWorker::new(tmp.path().to_path_buf(), "LH-01".into())
            .without_firewall_cmd();
        assert_eq!(w.tick_once(), 0); // no shell-out → no adds, but no panic
    }

    #[test]
    fn ports_to_remove_drops_stale_keeps_shared_http() {
        // Previously opened: a raw stream port + the shared 80/443. Desired now
        // wants only 80/443 (the stream was unexposed).
        let prev = vec![
            (80, "tcp".to_string()),
            (443, "tcp".to_string()),
            (25565, "tcp".to_string()),
        ];
        let desired = vec![(80, "tcp".to_string()), (443, "tcp".to_string())];
        // Only the raw stream port is reclaimed; 80/443 stay additive-only even if
        // they ever fell out of `desired`.
        assert_eq!(
            ports_to_remove(&prev, &desired),
            vec![(25565, "tcp".to_string())]
        );
        // Shared HTTP ports are never returned even when desired is empty.
        assert!(ports_to_remove(&[(80, "tcp".into()), (443, "tcp".into())], &[]).is_empty());
    }

    #[test]
    fn forwards_to_remove_reclaims_all_stale() {
        let prev = vec![
            (25565, "tcp".to_string(), "10.42.0.9".to_string()),
            (10000, "udp".to_string(), "10.42.0.9".to_string()),
        ];
        // Only the tcp stream is still wanted.
        let desired = vec![(25565, "tcp".to_string(), "10.42.0.9".to_string())];
        assert_eq!(
            forwards_to_remove(&prev, &desired),
            vec![(10000, "udp".to_string(), "10.42.0.9".to_string())]
        );
    }

    #[test]
    fn applied_state_round_trips_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let w = ConnectFirewallWorker::new(tmp.path().to_path_buf(), "LH-01".into())
            .with_state_dir(state.path().to_path_buf());
        // Missing file → default empty.
        assert_eq!(w.load_applied(), AppliedState::default());
        let s = AppliedState {
            ports: vec![(25565, "tcp".into())],
            forwards: vec![(25565, "tcp".into(), "10.42.0.9".into())],
            ..Default::default()
        };
        w.save_applied(&s);
        assert_eq!(w.load_applied(), s);
    }

    #[test]
    fn parse_open_ports_parses_tokens_and_skips_garbage() {
        // firewall-cmd --list-ports emits space-separated `port/proto` tokens.
        let got = parse_open_ports("80/tcp 443/tcp 4242/udp\n");
        assert_eq!(
            got,
            vec![
                (80, "tcp".to_string()),
                (443, "tcp".to_string()),
                (4242, "udp".to_string()),
            ]
        );
        // Malformed tokens (no proto / non-numeric / bare) are skipped.
        let messy = parse_open_ports("8080/tcp garbage 99999/tcp 70000/tcp /tcp");
        // 8080 ok; "garbage" skipped; 99999 > u16::MAX skipped; 70000 > u16 skipped.
        assert_eq!(messy, vec![(8080, "tcp".to_string())]);
        assert!(parse_open_ports("").is_empty());
    }

    #[test]
    fn unexpected_excludes_foundational_and_connect_owned() {
        // Open: the whole foundational layer + a CONNECT-owned service + two rogue
        // ports. Only the rogue ports are unexpected.
        let open = vec![
            (22, "tcp".to_string()),   // ssh — foundational
            (4242, "udp".to_string()), // nebula — foundational
            (4243, "tcp".to_string()), // enroll — foundational
            (80, "tcp".to_string()),   // shared http
            (443, "tcp".to_string()),  // shared http
            (8080, "tcp".to_string()), // CONNECT-owned (exposed service)
            (9090, "tcp".to_string()), // ROGUE
            (3000, "udp".to_string()), // ROGUE
        ];
        let connect_owned = vec![(8080, "tcp".to_string())];
        let unexpected = unexpected_open_ports(&open, &connect_owned);
        // Sorted: (3000,udp) then (9090,tcp).
        assert_eq!(
            unexpected,
            vec![(3000, "udp".to_string()), (9090, "tcp".to_string())]
        );
    }

    #[test]
    fn unexpected_is_empty_when_everything_is_accounted_for() {
        let open = vec![
            (4242, "udp".to_string()),
            (443, "tcp".to_string()),
            (25565, "tcp".to_string()),
        ];
        let connect_owned = vec![(25565, "tcp".to_string())];
        assert!(unexpected_open_ports(&open, &connect_owned).is_empty());
    }

    #[test]
    fn tick_persists_desired_state_for_later_reclaim() {
        // With a real (echo-style noop) firewall_cmd we can't run here, but the
        // state persistence runs whenever firewall_cmd is set. Use `true` as a
        // harmless always-succeeds binary so the shell-outs no-op successfully.
        let tmp = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let mut cfg = ExposureConfig::default();
        cfg.upsert(public_svc("web", "LH-01", 3000, ProtoMode::Http));
        exposure::save(tmp.path(), &cfg).unwrap();
        let mut w = ConnectFirewallWorker::new(tmp.path().to_path_buf(), "LH-01".into())
            .with_state_dir(state.path().to_path_buf());
        w.firewall_cmd = "true"; // always-succeeds noop
        let _ = w.tick_once();
        // The http service's 80/443 were recorded as CONNECT-owned.
        let applied = w.load_applied();
        assert_eq!(
            applied.ports,
            vec![(80, "tcp".to_string()), (443, "tcp".to_string())]
        );
    }
}
