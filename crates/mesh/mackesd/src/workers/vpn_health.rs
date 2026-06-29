//! VPN-GW-6 — per-tunnel health + exit-IP/leak verification + auto-failover.
//!
//! A periodic worker (modelled on `presence_watch`) that, every
//! [`SWEEP_INTERVAL`], probes each configured tunnel:
//!   1. **liveness** — is `mvpn-<id>` present (the interface up),
//!   2. **exit-IP verification** — fetch the public IP *through the tunnel*
//!      (`curl --interface mvpn-<id> <provider exit-check>`) and confirm it
//!      differs from the node's WAN IP (egress really leaves via the tunnel, not
//!      direct) and, where the provider runs a first-party reflector
//!      (`vpn_providers::exit_check_target`), that the reflector confirms it,
//!   3. **DNS-leak probe** — best-effort, where the provider reflector reports it.
//!
//! It publishes the verified per-tunnel exit state ([`vpn::save_exit_state`]) so
//! DDNS (DDNS-EGRESS-4) + the Routing panel read the live truth, resolves each
//! egress route's active tunnel from the chain (VPN-GW-4), **fails over** a route
//! whose active tunnel went unhealthy to the next healthy one — or engages the
//! leak-proof **kill-switch** when the whole chain is down — and raises
//! `event/vpn/tunnel-down` (+ a Hub alert) on a health boundary crossing.
//!
//! The probe + egress I/O is gated on `spawn` (false in tests); the health
//! decision (`classify`) + the failover resolution (`EgressRoute::resolve`) +
//! the event-transition logic (`health_transitions`) are pure + unit-tested.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

use mackes_mesh_types::vpn::{
    self, EgressRoute, Method, RouteScope, TunnelDef, TunnelExit, VpnExitState,
};
use mackes_mesh_types::vpn_egress::EgressPlan;
use mackes_mesh_types::vpn_providers::{self, Provider};

/// Sweep cadence — matches the presence/heartbeat granularity. Comfortably under
/// the DDNS default 60 s TTL so a failover-driven exit-IP change is republished
/// within ~TTL (DDNS-EGRESS-4).
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(30);

/// Per-probe network budget (each `curl` through the tunnel / to the WAN).
const PROBE_TIMEOUT_SECS: u32 = 6;

/// The raw outcome of probing one tunnel — the pure inputs to [`classify`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TunnelProbe {
    /// `ip link show mvpn-<id>` succeeded (the interface is present).
    pub iface_present: bool,
    /// The public IP observed *through the tunnel* (`None` ⇒ unreachable).
    pub exit_ip: Option<String>,
    /// The node's direct WAN IP, for the leak comparison (`None` ⇒ unknown).
    pub wan_ip: Option<String>,
    /// The provider's first-party reflector positively confirmed the exit (or,
    /// with no first-party reflector, the exit IP differs from the WAN).
    pub provider_ok: bool,
    /// A DNS-leak was positively detected (resolver egressed outside the tunnel).
    pub dns_leak: bool,
}

/// A tunnel's health verdict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    /// Interface up, exit IP verified through the tunnel, no DNS leak.
    Healthy,
    /// Interface down, or up but no exit reachable through it.
    Down,
    /// Interface up + an exit reachable, but it leaks — the exit IP equals the
    /// WAN (egress went direct), the provider check failed, or DNS leaked.
    Leaking,
}

impl Health {
    /// Only [`Health::Healthy`] is trustworthy egress; a leaking tunnel is *not*
    /// healthy (failover/kill-switch treats a silent leak like a drop).
    #[must_use]
    pub const fn is_healthy(self) -> bool {
        matches!(self, Self::Healthy)
    }

    /// A short label for the published `detail` + the event payload.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Down => "down",
            Self::Leaking => "leaking",
        }
    }
}

/// The pure health decision over a probe (VPN-GW-6 §9). Order matters: a missing
/// interface or unreachable exit is *down*; an exit that equals the WAN, fails
/// the provider check, or leaks DNS is *leaking* (a silent leak is worse than an
/// honest drop — it must fail over too).
#[must_use]
pub fn classify(p: &TunnelProbe) -> Health {
    if !p.iface_present {
        return Health::Down;
    }
    let Some(exit) = p.exit_ip.as_deref() else {
        return Health::Down;
    };
    if p.dns_leak {
        return Health::Leaking;
    }
    if let Some(wan) = p.wan_ip.as_deref() {
        if exit == wan {
            return Health::Leaking; // egress went direct — the tunnel isn't carrying.
        }
    }
    if !p.provider_ok {
        return Health::Leaking;
    }
    Health::Healthy
}

/// Build the published [`TunnelExit`] for a tunnel from its probe + the wall
/// clock. `verified` is set only when [`classify`] is [`Health::Healthy`] (so
/// DDNS only ever publishes a verified exit IP); `exit_ip` carries the observed
/// address either way so the UI can show "leaking via <ip>". Pure.
#[must_use]
pub fn exit_from_probe(t: &TunnelDef, p: &TunnelProbe, now_ms: u64) -> TunnelExit {
    let health = classify(p);
    TunnelExit {
        id: t.id.clone(),
        ifname: t.ifname(),
        provider: t.provider.clone(),
        up: p.iface_present && p.exit_ip.is_some(),
        verified: health.is_healthy(),
        dns_leak: p.dns_leak,
        exit_ip: p.exit_ip.clone().unwrap_or_default(),
        checked_ms: now_ms,
        detail: health.label().to_string(),
    }
}

/// Decide which tunnels crossed a health boundary between the previous and
/// current sweep. Returns `(tunnel_id, now_healthy)` for each crossing — a
/// healthy→unhealthy transition (`false`) raises `tunnel-down`, an
/// unhealthy→healthy transition (`true`) raises a recovery. The first sweep seeds
/// silently (an empty `prev` never fires) so a daemon restart doesn't re-alert
/// every already-down tunnel. Pure — the testable core.
#[must_use]
pub fn health_transitions(
    prev: &HashMap<String, bool>,
    cur: &HashMap<String, bool>,
) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    for (id, &healthy) in cur {
        match prev.get(id) {
            Some(&was) if was != healthy => out.push((id.clone(), healthy)),
            _ => {}
        }
    }
    out.sort();
    out
}

/// The VPN health + auto-failover worker.
#[derive(Clone, Debug)]
pub struct VpnHealthWorker {
    workgroup_root: PathBuf,
    /// This node's id — used to decide which routes' gateway egress this node is
    /// responsible for applying (a route is only enforced on its gateway node).
    node_id: String,
    /// Run the real `curl`/`ip`/`nft` shell-outs; tests disable it so the pure
    /// decision logic runs with no host mutation / network.
    spawn: bool,
}

impl VpnHealthWorker {
    /// Build the worker rooted at the shared workgroup root for `node_id`.
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: String) -> Self {
        Self {
            workgroup_root,
            node_id,
            spawn: true,
        }
    }

    /// Disable the probe/egress shell-out (tests).
    #[must_use]
    pub fn without_spawn(mut self) -> Self {
        self.spawn = false;
        self
    }

    /// Run the health loop until `should_stop`. `alerts_dir` is the `alert_relay`
    /// drop-dir for the Hub toast (same lane `presence_watch` uses).
    pub fn run<F: Fn() -> bool>(&self, persist: &Persist, alerts_dir: &Path, should_stop: F) {
        // Seed silently — a restart must not re-announce every down tunnel.
        let mut prev = self.sweep_health();
        let mut applied: HashMap<(RouteScope, String), Option<String>> = HashMap::new();
        while !should_stop() {
            std::thread::sleep(SWEEP_INTERVAL);
            if should_stop() {
                break;
            }
            self.sweep(persist, alerts_dir, &mut prev, &mut applied);
        }
    }

    /// One full sweep: probe → publish exit state → emit transitions → enforce
    /// the gateway routes. Split out from [`run`](Self::run) so the loop is thin.
    fn sweep(
        &self,
        persist: &Persist,
        alerts_dir: &Path,
        prev: &mut HashMap<String, bool>,
        applied: &mut HashMap<(RouteScope, String), Option<String>>,
    ) {
        let cfg = vpn::load(&self.workgroup_root);
        let wan = self.probe_wan();
        let now = now_ms();
        let mut state = VpnExitState::default();
        let mut cur: HashMap<String, bool> = HashMap::new();
        for t in &cfg.tunnel {
            let probe = self.probe_tunnel(t, wan.as_deref());
            let exit = exit_from_probe(t, &probe, now);
            cur.insert(t.id.clone(), exit.verified);
            state.upsert(exit);
        }
        if let Err(e) = vpn::save_exit_state(&self.workgroup_root, &state) {
            tracing::warn!(target: "mackesd::vpn_health", error = %e, "exit-state write failed");
        }
        for (id, now_healthy) in health_transitions(prev, &cur) {
            let exit_ip = state
                .get(&id)
                .map_or("", |e| e.exit_ip.as_str())
                .to_string();
            let detail = state.get(&id).map_or("", |e| e.detail.as_str()).to_string();
            self.emit_transition(persist, alerts_dir, &id, now_healthy, &exit_ip, &detail);
        }
        self.enforce_routes(&cfg.route, &cfg.tunnel, &state, applied);
        *prev = cur;
    }

    /// The current sweep's health map without emitting/enforcing — used to seed
    /// `prev` so the first real sweep only fires on a genuine change.
    fn sweep_health(&self) -> HashMap<String, bool> {
        let cfg = vpn::load(&self.workgroup_root);
        let wan = self.probe_wan();
        cfg.tunnel
            .iter()
            .map(|t| {
                let probe = self.probe_tunnel(t, wan.as_deref());
                (t.id.clone(), classify(&probe).is_healthy())
            })
            .collect()
    }

    /// Enforce each route this node is the **gateway** for: move the selective
    /// egress to the route's resolved active tunnel (VPN-GW-4 chain + VPN-GW-6
    /// health), or engage the kill-switch when the whole chain is unhealthy. Only
    /// acts on a *change* from the last-applied tunnel so steady-state is no nft
    /// churn. Reuses the exact apply/kill-switch sequences from `ipc::vpn_gw`
    /// (§6 — no second egress code path).
    fn enforce_routes(
        &self,
        routes: &[EgressRoute],
        tunnels: &[TunnelDef],
        state: &VpnExitState,
        applied: &mut HashMap<(RouteScope, String), Option<String>>,
    ) {
        if !self.spawn {
            return;
        }
        let healthy = |id: &str| {
            state
                .get(id)
                .is_some_and(|e| e.up && e.verified && !e.dns_leak)
        };
        let ifname_of = |id: &str| tunnels.iter().find(|t| t.id == id).map(TunnelDef::ifname);
        for r in routes {
            if r.gateway.trim() != self.node_id.trim() {
                continue; // not this node's responsibility.
            }
            let want = r.resolve(healthy).map(str::to_string);
            let key = r.key();
            if applied.get(&key) == Some(&want) {
                continue; // unchanged — no churn.
            }
            match want.as_deref() {
                Some(id) => {
                    if let Some(ifname) = ifname_of(id) {
                        crate::ipc::vpn_gw::apply_egress(
                            &EgressPlan::for_ifname_on_default_overlay(&ifname),
                        );
                        tracing::info!(
                            target: "mackesd::vpn_health",
                            route = %r.target, tunnel = id, "egress route active tunnel (VPN-GW-4/6)"
                        );
                    }
                }
                None if r.kill_switch => {
                    // Whole chain unhealthy → block on the primary's mark (no leak).
                    if let Some(ifname) = ifname_of(r.tunnel.trim()) {
                        crate::ipc::vpn_gw::engage_kill_switch(
                            &EgressPlan::for_ifname_on_default_overlay(&ifname),
                        );
                        tracing::warn!(
                            target: "mackesd::vpn_health",
                            route = %r.target, "no healthy tunnel — kill-switch engaged (VPN-GW-8)"
                        );
                    }
                }
                None => {} // kill_switch off → leave direct (operator opted out).
            }
            applied.insert(key, want);
        }
    }

    /// Probe the node's direct WAN IP (no `--interface`, so it leaves over the
    /// default route). Best-effort; `None` when `curl`/network is unavailable.
    fn probe_wan(&self) -> Option<String> {
        if !self.spawn {
            return None;
        }
        curl_body(&[], vpn_providers::NEUTRAL_EXIT_CHECK_HOST).and_then(|b| parse_ip(&b))
    }

    /// Probe one tunnel: liveness + the exit-IP-through-the-tunnel + provider/leak
    /// checks. Best-effort; a probe that can't run yields a "down" [`TunnelProbe`].
    fn probe_tunnel(&self, t: &TunnelDef, wan: Option<&str>) -> TunnelProbe {
        let ifname = t.ifname();
        let iface_present = iface_present(self.spawn, &ifname);
        if !self.spawn || !iface_present {
            return TunnelProbe {
                iface_present,
                wan_ip: wan.map(str::to_string),
                ..Default::default()
            };
        }
        let provider = Provider::from_label(&t.provider).unwrap_or(Provider::GenericWg);
        let target = vpn_providers::exit_check_target(provider);
        let body = curl_body(&["--interface", &ifname], target);
        let exit_ip = body.as_deref().and_then(parse_ip);
        let provider_ok = match (provider, exit_ip.as_deref()) {
            // Mullvad's first-party reflector positively confirms the exit.
            (Provider::Mullvad, Some(_)) => body
                .as_deref()
                .and_then(parse_bool_field("mullvad_exit_ip"))
                .unwrap_or(false),
            // No first-party reflector: confirmed-different-from-WAN is the best
            // honest signal that egress is the provider's, not the WAN.
            (_, Some(ip)) => wan.is_none_or(|w| w != ip),
            (_, None) => false,
        };
        // DNS-leak: a real, defensible structural signal — a WireGuard tunnel
        // whose materialized config carries NO `DNS =` line resolves names via
        // the host resolver, which egresses *outside* the tunnel (the classic
        // DNS leak). We read the on-disk wg config the bring-up materialized
        // (`/etc/wireguard/<ifname>.conf`) and flag a leak only when it's present
        // AND has no DNS directive. An unreadable config ⇒ no claim (§7 — honest
        // "not detected", never a fabricated verdict). OpenVPN's resolver
        // handling is config-internal → not flagged here.
        let dns_leak = match t.method {
            Method::Wg => std::fs::read_to_string(format!("/etc/wireguard/{ifname}.conf"))
                .ok()
                .is_some_and(|conf| !wg_conf_has_dns(&conf)),
            _ => false,
        };
        TunnelProbe {
            iface_present,
            exit_ip,
            wan_ip: wan.map(str::to_string),
            provider_ok,
            dns_leak,
        }
    }

    /// Raise `event/vpn/tunnel-down` (or a recovery `event/vpn/tunnel-up`) on the
    /// Bus + a Hub alert (the `alert_relay` JSON lane `presence_watch` rides).
    fn emit_transition(
        &self,
        persist: &Persist,
        alerts_dir: &Path,
        id: &str,
        now_healthy: bool,
        exit_ip: &str,
        detail: &str,
    ) {
        let topic = if now_healthy {
            "event/vpn/tunnel-up"
        } else {
            "event/vpn/tunnel-down"
        };
        let body = serde_json::json!({
            "tunnel": id,
            "node": self.node_id,
            "healthy": now_healthy,
            "exit_ip": exit_ip,
            "detail": detail,
        })
        .to_string();
        let priority = if now_healthy {
            Priority::Default
        } else {
            Priority::High
        };
        if let Err(e) = persist.write(topic, priority, None, Some(&body)) {
            tracing::warn!(target: "mackesd::vpn_health", error = %e, topic, "event publish failed");
        }
        // Hub toast — only the down boundary is news (a recovery is info-level).
        let minute = now_ms() / 60_000;
        let alert_id = format!(
            "vpn-{id}-{}-{minute}",
            if now_healthy { "up" } else { "down" }
        );
        let (severity, summary) = if now_healthy {
            ("info", format!("VPN tunnel {id} recovered"))
        } else {
            ("warn", format!("VPN tunnel {id} down ({detail})"))
        };
        let event = serde_json::json!({
            "id": alert_id,
            "severity": severity,
            "alert": format!("vpn.tunnel.{}", if now_healthy { "up" } else { "down" }),
            "host": self.node_id,
            "summary": summary,
        });
        if std::fs::create_dir_all(alerts_dir).is_ok() {
            let path = alerts_dir.join(format!("{alert_id}.json"));
            let _ = std::fs::write(path, event.to_string());
        }
    }
}

/// Unix epoch milliseconds.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Is `ifname` a present network interface? (`ip -o link show <ifname>`.)
fn iface_present(spawn: bool, ifname: &str) -> bool {
    if !spawn {
        return false;
    }
    std::process::Command::new("ip")
        .args(["-o", "link", "show", ifname])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// `curl -s --max-time N [extra] <url>` → the response body (best-effort).
fn curl_body(extra: &[&str], url: &str) -> Option<String> {
    let mut args: Vec<String> = vec![
        "-s".into(),
        "--max-time".into(),
        PROBE_TIMEOUT_SECS.to_string(),
    ];
    args.extend(extra.iter().map(|s| (*s).to_string()));
    args.push(url.to_string());
    let out = std::process::Command::new("curl")
        .args(&args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let body = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!body.is_empty()).then_some(body)
}

/// Extract the `"ip"` field from an ipinfo/mullvad-style JSON body. Pure.
#[must_use]
pub fn parse_ip(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .get("ip")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// A closure that extracts a named boolean field from a JSON body. Pure.
fn parse_bool_field(field: &'static str) -> impl Fn(&str) -> Option<bool> {
    move |body: &str| {
        serde_json::from_str::<serde_json::Value>(body)
            .ok()?
            .get(field)
            .and_then(serde_json::Value::as_bool)
    }
}

/// Does a `wg-quick` config carry a `DNS =` directive (under `[Interface]`)? A
/// config WITHOUT one resolves names via the host resolver — a DNS leak when the
/// tunnel is the egress. Pure (tolerant of case + surrounding spaces + comments).
#[must_use]
pub fn wg_conf_has_dns(conf: &str) -> bool {
    conf.lines().any(|raw| {
        let line = raw.trim();
        if line.starts_with('#') || line.starts_with(';') {
            return false;
        }
        line.split_once('=')
            .is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case("dns"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(
        iface: bool,
        exit: Option<&str>,
        wan: Option<&str>,
        prov: bool,
        leak: bool,
    ) -> TunnelProbe {
        TunnelProbe {
            iface_present: iface,
            exit_ip: exit.map(str::to_string),
            wan_ip: wan.map(str::to_string),
            provider_ok: prov,
            dns_leak: leak,
        }
    }

    #[test]
    fn classify_down_when_iface_absent_or_no_exit() {
        assert_eq!(
            classify(&probe(false, None, Some("9.9.9.9"), true, false)),
            Health::Down
        );
        assert_eq!(
            classify(&probe(true, None, Some("9.9.9.9"), true, false)),
            Health::Down
        );
    }

    #[test]
    fn classify_leaking_on_wan_match_provider_fail_or_dns_leak() {
        // Exit == WAN ⇒ egress went direct.
        assert_eq!(
            classify(&probe(true, Some("9.9.9.9"), Some("9.9.9.9"), true, false)),
            Health::Leaking
        );
        // Provider check failed.
        assert_eq!(
            classify(&probe(true, Some("1.2.3.4"), Some("9.9.9.9"), false, false)),
            Health::Leaking
        );
        // DNS leak.
        assert_eq!(
            classify(&probe(true, Some("1.2.3.4"), Some("9.9.9.9"), true, true)),
            Health::Leaking
        );
    }

    #[test]
    fn classify_healthy_when_exit_differs_provider_ok_no_leak() {
        let h = classify(&probe(true, Some("1.2.3.4"), Some("9.9.9.9"), true, false));
        assert_eq!(h, Health::Healthy);
        assert!(h.is_healthy());
        // No WAN known but provider reflector confirmed → still healthy.
        assert_eq!(
            classify(&probe(true, Some("1.2.3.4"), None, true, false)),
            Health::Healthy
        );
    }

    #[test]
    fn exit_from_probe_publishes_verified_only_when_healthy() {
        let t = TunnelDef {
            id: "mullvad1".into(),
            provider: "mullvad".into(),
            ..Default::default()
        };
        let good = exit_from_probe(
            &t,
            &probe(true, Some("1.2.3.4"), Some("9.9.9.9"), true, false),
            100,
        );
        assert!(good.up);
        assert!(good.verified);
        assert_eq!(good.exit_ip, "1.2.3.4");
        assert_eq!(good.ifname, "mvpn-mullvad1");
        assert_eq!(good.detail, "healthy");
        // A leaking tunnel is up but NOT verified (DDNS won't publish it).
        let leak = exit_from_probe(
            &t,
            &probe(true, Some("9.9.9.9"), Some("9.9.9.9"), true, false),
            100,
        );
        assert!(leak.up);
        assert!(!leak.verified);
        assert_eq!(leak.detail, "leaking");
    }

    #[test]
    fn health_transitions_only_fire_on_a_boundary_and_seed_silently() {
        let prev: HashMap<String, bool> = [("a".into(), true), ("b".into(), false)]
            .into_iter()
            .collect();
        let cur: HashMap<String, bool> =
            [("a".into(), false), ("b".into(), false), ("c".into(), true)]
                .into_iter()
                .collect();
        // a: healthy→unhealthy (down); b: unchanged; c: new (seeds silently).
        assert_eq!(
            health_transitions(&prev, &cur),
            vec![("a".to_string(), false)]
        );
        // Recovery fires the up boundary (b was unhealthy → now healthy).
        let cur2: HashMap<String, bool> = [("a".into(), true), ("b".into(), true)]
            .into_iter()
            .collect();
        assert_eq!(
            health_transitions(&prev, &cur2),
            vec![("b".to_string(), true)]
        );
        // Empty prev seeds the whole map silently.
        assert!(health_transitions(&HashMap::new(), &cur).is_empty());
    }

    #[test]
    fn parse_ip_reads_the_ip_field() {
        assert_eq!(
            parse_ip(r#"{"ip":"1.2.3.4","country":"US"}"#),
            Some("1.2.3.4".to_string())
        );
        assert_eq!(parse_ip(r#"{"no":"ip"}"#), None);
        assert_eq!(parse_ip("garbage"), None);
    }

    #[test]
    fn parse_bool_field_reads_named_bool() {
        let f = parse_bool_field("mullvad_exit_ip");
        assert_eq!(f(r#"{"mullvad_exit_ip":true}"#), Some(true));
        assert_eq!(f(r#"{"mullvad_exit_ip":false}"#), Some(false));
        assert_eq!(f(r#"{"other":1}"#), None);
    }

    #[test]
    fn wg_conf_dns_presence_detects_the_leak_shape() {
        assert!(wg_conf_has_dns(
            "[Interface]\nPrivateKey = x\nDNS = 10.64.0.1\n"
        ));
        assert!(wg_conf_has_dns("[Interface]\ndns=1.1.1.1\n")); // case + no spaces
                                                                // No DNS line ⇒ leaks to the host resolver.
        assert!(!wg_conf_has_dns(
            "[Interface]\nPrivateKey = x\nAddress = 10/32\n"
        ));
        // A commented DNS line doesn't count.
        assert!(!wg_conf_has_dns(
            "[Interface]\n# DNS = 1.1.1.1\nAddress = 10/32\n"
        ));
    }

    #[test]
    fn worker_without_spawn_probes_down_and_writes_empty_state() {
        let tmp = tempfile::tempdir().unwrap();
        // A tunnel in config; no spawn ⇒ iface absent ⇒ down, unverified.
        let mut cfg = vpn::VpnConfig::default();
        cfg.upsert(TunnelDef {
            id: "m1".into(),
            provider: "mullvad".into(),
            ..Default::default()
        });
        vpn::save(tmp.path(), &cfg).unwrap();
        let w = VpnHealthWorker::new(tmp.path().to_path_buf(), "eagle".into()).without_spawn();
        let health = w.sweep_health();
        assert_eq!(health.get("m1"), Some(&false));
    }
}
