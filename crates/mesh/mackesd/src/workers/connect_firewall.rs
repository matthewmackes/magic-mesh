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
//! Safety: **additive only** — it never removes a rule (so it can't lock out SSH
//! or Nebula). `firewall-cmd --add-port` is idempotent. Drift *correction*
//! (closing ports the policy no longer wants, and alerting on unexpected public
//! openings) is deferred — it needs a complete allow-inventory to avoid
//! false-closing other features' ports + an operator-confirmed close (a full
//! public-deny sweep that auto-removes is too dangerous to run unattended).

#![cfg(feature = "async-services")]

use std::path::PathBuf;
use std::time::Duration;

use mackes_mesh_types::exposure::{self, ProtoMode};

use super::{ShutdownToken, Worker};

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

/// The exposure-driven enforcement worker: opens the policy's ingress firewall
/// ports (CONNECT-3), forwards raw TCP/UDP streams to the overlay (CONNECT-5),
/// AND renders/writes this node's Caddy ingress config (CONNECT-4) — one "apply
/// this node's exposure" reconcile per tick.
pub struct ConnectFirewallWorker {
    workgroup_root: PathBuf,
    hostname: String,
    tick_interval: Duration,
    /// `firewall-cmd` binary (empty disables the shell-out — for tests).
    firewall_cmd: &'static str,
    /// Dir the Caddy ingress fragment is written to (overridable for tests).
    caddy_dir: PathBuf,
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

    /// CONNECT-5 — apply raw TCP/UDP stream forwards (public port → the service's
    /// overlay endpoint) via firewalld `forward-port` (+ masquerade so replies
    /// route back). Additive + idempotent; resolves node→overlay-IP from the peer
    /// directory. Returns the number of forward rules applied (0 when none bound
    /// here / no firewalld / unresolved). Best-effort.
    fn apply_forwards(&self, cfg: &exposure::ExposureConfig) -> usize {
        if self.firewall_cmd.is_empty() {
            return 0;
        }
        let peers = mackes_mesh_types::peers::read_peers(&mackes_mesh_types::peers::peers_dir(
            &self.workgroup_root,
        ));
        let resolve = |node: &str| -> Option<String> {
            peers
                .iter()
                .find(|p| p.hostname == node)
                .and_then(|p| p.overlay_ip.clone())
        };
        let fwds = desired_forwards(cfg, &self.hostname, resolve);
        if fwds.is_empty() {
            return 0;
        }
        // forward-port to a remote (overlay) addr needs masquerade for the return path.
        let mut masq = std::process::Command::new(self.firewall_cmd);
        masq.args(["--permanent", "--zone", PUBLIC_ZONE, "--add-masquerade"]);
        let _ = crate::workers::proc::status_with_timeout(
            masq,
            crate::workers::proc::DEFAULT_CMD_TIMEOUT,
        );
        let mut applied = 0usize;
        for (port, proto, ip) in &fwds {
            let spec = format!("port={port}:proto={proto}:toaddr={ip}:toport={port}");
            let mut cmd = std::process::Command::new(self.firewall_cmd);
            cmd.args([
                "--permanent",
                "--zone",
                PUBLIC_ZONE,
                "--add-forward-port",
                &spec,
            ]);
            if crate::workers::proc::status_with_timeout(
                cmd,
                crate::workers::proc::DEFAULT_CMD_TIMEOUT,
            )
            .map(|s| s.success())
            .unwrap_or(false)
            {
                applied += 1;
            }
        }
        applied
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

    /// One enforcement tick: load the policy, compute this node's desired ingress
    /// ports, and idempotently open them on the `public` zone. Returns how many
    /// `--add-port` invocations ran (0 when nothing is bound here / no firewalld).
    pub fn tick_once(&self) -> usize {
        let cfg = exposure::load(&self.workgroup_root);
        // CONNECT-4 — render/write this node's Caddy ingress fragment (runs even
        // when no firewall ports change, e.g. to clear the fragment after an
        // unexpose). Best-effort + no-op when Caddy isn't installed here.
        let _ = self.apply_caddy(&cfg);
        // CONNECT-5 — raw TCP/UDP stream forwards (firewalld forward-port).
        let _ = self.apply_forwards(&cfg);
        let desired = desired_ingress_ports(&cfg, &self.hostname);
        if desired.is_empty() || self.firewall_cmd.is_empty() {
            return 0;
        }
        let mut added = 0usize;
        for (port, proto) in &desired {
            let mut cmd = std::process::Command::new(self.firewall_cmd);
            cmd.args([
                "--permanent",
                "--zone",
                PUBLIC_ZONE,
                "--add-port",
                &format!("{port}/{proto}"),
            ]);
            // EFF-20 — bound the shell-out so a hung firewall-cmd can't pin the tick.
            if crate::workers::proc::status_with_timeout(
                cmd,
                crate::workers::proc::DEFAULT_CMD_TIMEOUT,
            )
            .map(|s| s.success())
            .unwrap_or(false)
            {
                added += 1;
            }
        }
        if added > 0 {
            let mut reload = std::process::Command::new(self.firewall_cmd);
            reload.arg("--reload");
            let _ = crate::workers::proc::status_with_timeout(
                reload,
                crate::workers::proc::DEFAULT_CMD_TIMEOUT,
            );
            tracing::info!(
                ports = desired.len(),
                "connect_firewall: ensured {} ingress port(s) open on the public zone (CONNECT-3)",
                desired.len()
            );
        }
        added
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
}
