//! VPN-GW-6 — tunnel health + exit-IP/leak verification + auto-failover + alerts.
//!
//! A silently-leaking or down tunnel is the worst egress failure: traffic the
//! operator believes is going out the provider is instead escaping to the raw
//! WAN, deanonymizing it without any visible error. This module is the watcher
//! that catches that. Per tunnel on a route's chain it:
//!
//!   1. **liveness** — is the `mvpn-<id>` interface present + the route routable
//!      (reusing [`crate::ipc::vpn_gw`]'s `iface_up`),
//!   2. **exit-IP verification** — fetch the provider's reflector (Mullvad's
//!      first-party `am.i.mullvad.net`, else the neutral `ipinfo.io`) *through
//!      the tunnel iface* and compare the reported public IP to the raw-WAN IP:
//!      equal ⇒ the tunnel is up but traffic is **leaking** straight out the WAN
//!      (a silent leak), different ⇒ egress really exits the provider,
//!   3. **DNS-leak probe** — resolve a name *through the tunnel* and confirm the
//!      answer didn't come back via the host's WAN resolver.
//!
//! On a failure it computes the failover decision from the route's ordered chain
//! ([`EgressRoute::active_tunnel`]) — walk to the next live tunnel, or engage the
//! kill-switch when the whole chain is down — and raises a `vpn/tunnel-down`
//! alert on `event/vpn/signals`. The verified exit IP is cached so the UI's
//! `egress-health` read shows the real, confirmed exit (not just "iface up").
//!
//! The decision core ([`verdict`], [`exit_ip_from_body`], [`failover`]) is pure
//! and unit-tested; the spawn side ([`probe_through`], [`wan_ip`]) shells out to
//! `curl`/`ip` exactly like the other `mackesd` health workers (`dc_health`,
//! `surrounding_hosts`) and is reached at runtime by the vpn responder's sweep.

use std::collections::HashMap;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use serde_json::json;

use mackes_mesh_types::vpn::{self, TunnelDef};
use mackes_mesh_types::vpn_egress::{self, EgressRoute};
use mackes_mesh_types::vpn_providers::{self, Provider};

/// The bus topic VPN-GW-6 raises tunnel-health alerts on.
///
/// Mirrors the `event/<domain>/signals` shape the nebula + fleet dispatchers
/// use. The operator's alert-relay / dashboard subscribes here; the task's
/// `vpn/tunnel-down` is the alert payload's `"alert"` tag.
pub const VPN_EVENT_TOPIC: &str = "event/vpn/signals";

/// How a tunnel's egress verifies — the per-tunnel verdict the sweep computes.
///
/// Only [`Ok`] (and a neutral [`Unverifiable`] where no live account can confirm
/// the exit) keep a tunnel active; everything else fails it over + alerts.
///
/// [`Ok`]: Health::Ok
/// [`Unverifiable`]: Health::Unverifiable
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    /// Interface present, exit IP confirmed to be the provider's (≠ the WAN IP),
    /// and no DNS leak. The only fully-green state.
    Ok,
    /// The `mvpn-<id>` interface is absent — the tunnel is hard-down.
    Down,
    /// The interface is up but egress is **leaking**: the exit IP fetched through
    /// the tunnel equals the raw-WAN IP, so traffic isn't actually tunneling.
    /// The silent failure VPN-GW-6 exists to catch.
    Leaking,
    /// The interface is up + the exit IP differs from the WAN, but a DNS query
    /// resolved via the host's WAN resolver — a DNS leak that deanonymizes even
    /// when the data path tunnels.
    DnsLeak,
    /// The exit IP couldn't be fetched (no `curl`, no network, reflector down).
    /// Distinct from a leak: we can't *confirm* the provider, but we also have no
    /// evidence of a WAN leak. The interface is up, so the tunnel stays active
    /// (failing it over on an unverifiable probe would flap a healthy chain).
    Unverifiable,
}

impl Health {
    /// Does this verdict keep the tunnel as the active egress? Only a confirmed
    /// [`Ok`] or an [`Unverifiable`] (interface up, exit unconfirmable) hold the
    /// slot; a [`Down`]/[`Leaking`]/[`DnsLeak`] tunnel is failed over.
    ///
    /// [`Ok`]: Self::Ok
    /// [`Unverifiable`]: Self::Unverifiable
    /// [`Down`]: Self::Down
    /// [`Leaking`]: Self::Leaking
    /// [`DnsLeak`]: Self::DnsLeak
    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Ok | Self::Unverifiable)
    }

    /// The short tag the UI / alert payload reports.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Down => "down",
            Self::Leaking => "leaking",
            Self::DnsLeak => "dns-leak",
            Self::Unverifiable => "unverifiable",
        }
    }
}

/// The pure exit-IP + DNS-leak verdict for one tunnel.
///
/// Given the observed facts the spawn side gathers — whether the interface is
/// present, the exit IP fetched *through the tunnel* (`None` ⇒ couldn't fetch),
/// the raw-WAN IP, and whether the DNS probe resolved via the WAN resolver. No
/// I/O — this is the decision the sweep (and the unit tests) drive.
///
/// Precedence (worst wins, so a leak is never masked by a later check):
///   * iface absent ⇒ [`Health::Down`],
///   * exit IP unknown ⇒ [`Health::Unverifiable`] (can't confirm, no leak proof),
///   * exit IP == WAN IP ⇒ [`Health::Leaking`] (the silent leak),
///   * DNS resolved via WAN ⇒ [`Health::DnsLeak`],
///   * else [`Health::Ok`].
#[must_use]
pub fn verdict(
    iface_present: bool,
    exit_ip: Option<&str>,
    wan_ip: Option<&str>,
    dns_leaked: bool,
) -> Health {
    if !iface_present {
        return Health::Down;
    }
    let Some(exit) = exit_ip.map(str::trim).filter(|s| !s.is_empty()) else {
        return Health::Unverifiable;
    };
    // A leak is exit==WAN. If we couldn't read the WAN IP we can't *prove* a
    // leak, but the exit IP did come back through the tunnel, so treat the data
    // path as verified and only fall through to the DNS check.
    if let Some(wan) = wan_ip.map(str::trim).filter(|s| !s.is_empty()) {
        if exit.eq_ignore_ascii_case(wan) {
            return Health::Leaking;
        }
    }
    if dns_leaked {
        return Health::DnsLeak;
    }
    Health::Ok
}

/// Extract the public IP string from an exit-check reflector's JSON body.
///
/// Both `ipinfo.io` (`{"ip":"1.2.3.4",…}`) and Mullvad (`{"ip":"…",
/// "mullvad_exit_ip":true,…}`) report the observed IP under `"ip"`; this reads
/// that field. `None` when the body isn't the expected JSON (a captive-portal
/// HTML page, a truncated response) so the caller reports
/// [`Health::Unverifiable`], never a bogus IP.
#[must_use]
pub fn exit_ip_from_body(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body.trim()).ok()?;
    let ip = v.get("ip").and_then(serde_json::Value::as_str)?.trim();
    if ip.is_empty() {
        None
    } else {
        Some(ip.to_string())
    }
}

/// For a Mullvad reflector body, whether it *also* self-attests the exit is
/// Mullvad's (`"mullvad_exit_ip": true`). A first-party confirmation that's
/// strictly stronger than "exit ≠ WAN": used to upgrade the alert detail, never
/// to weaken the leak check. Non-Mullvad bodies simply don't carry the field.
#[must_use]
pub fn mullvad_self_attested(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body.trim())
        .ok()
        .and_then(|v| {
            v.get("mullvad_exit_ip")
                .and_then(serde_json::Value::as_bool)
        })
        .unwrap_or(false)
}

/// The exit-check reflector URL for a tunnel, resolved from its provider label
/// (Mullvad's first-party host, else the neutral reflector). Reused from the
/// VPN-GW-5 catalog so the target stays in one place.
#[must_use]
pub fn exit_check_url(t: &TunnelDef) -> &'static str {
    let provider = Provider::from_label(&t.provider).unwrap_or(Provider::GenericWg);
    vpn_providers::exit_check_target(provider)
}

/// The failover outcome for a route given the live per-tunnel health map (id →
/// [`Health`]). A tunnel is "down for failover purposes" when its verdict isn't
/// [`Health::is_active`] — so a *leaking* tunnel is treated exactly like a
/// hard-down one and walked past, which is the whole point of VPN-GW-6 (a leak
/// must fail over, not silently persist). Returns the active tunnel id (the first
/// chain entry that's healthy) or `None` ⇒ the whole chain failed and the
/// kill-switch (if [`EgressRoute::kill_switch`]) blocks egress.
#[must_use]
pub fn failover(route: &EgressRoute, health: &HashMap<String, Health>) -> Option<String> {
    let down: Vec<String> = route
        .chain()
        .into_iter()
        .filter(|id| {
            health.get(id).copied().is_some_and(|h| !h.is_active())
            // A tunnel with no health entry (never probed) is treated as
            // live so an unprobed chain isn't spuriously killed; the sweep
            // probes every chain tunnel, so this is the cold-start guard.
        })
        .collect();
    route.active_tunnel(&down)
}

/// One tunnel's full health record — what the sweep caches + the UI reads. The
/// `verified_exit_ip` is the confirmed provider exit (the field DDNS-EGRESS-1
/// subscribes to), present only when the exit IP came back through the tunnel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TunnelReport {
    /// The tunnel id (`TunnelDef::id`).
    pub id: String,
    /// The derived interface name `mvpn-<id>`.
    pub ifname: String,
    /// The verdict.
    pub health: Health,
    /// The exit IP fetched through the tunnel (the verified provider exit), if
    /// the reflector answered. This is what surfaces in the UI + feeds DDNS.
    pub verified_exit_ip: Option<String>,
    /// The raw-WAN IP the exit was compared against (for the leak proof).
    pub wan_ip: Option<String>,
    /// A human detail line (provider self-attestation, the leak reason, …).
    pub detail: String,
}

impl TunnelReport {
    /// The JSON shape the `egress-health` read + the alert payload carry.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        json!({
            "id": self.id,
            "ifname": self.ifname,
            "health": self.health.tag(),
            "verified_exit_ip": self.verified_exit_ip,
            "wan_ip": self.wan_ip,
            "detail": self.detail,
        })
    }
}

/// Fetch a URL *through a specific interface* via `curl --interface <ifname>`,
/// returning the body on a 2xx. Binding to the tunnel iface is what makes this an
/// exit-IP check: the request can only succeed (and report the provider's IP) if
/// egress actually routes out the tunnel. `None` on any failure (no `curl`,
/// timeout, non-2xx) — the caller maps that to [`Health::Unverifiable`].
#[must_use]
pub fn probe_through(ifname: &str, url: &str) -> Option<String> {
    let out = std::process::Command::new("curl")
        .args([
            "-s",
            "--interface",
            ifname,
            "-m",
            "10",
            "-o",
            "/dev/stdout",
            "-w",
            "\n%{http_code}",
            url,
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let (body, code) = match text.trim_end().rsplit_once('\n') {
        Some((b, c)) => (b.to_string(), c.trim().to_string()),
        None => (String::new(), text.trim().to_string()),
    };
    if code.starts_with('2') && !body.trim().is_empty() {
        Some(body)
    } else {
        None
    }
}

/// The raw-WAN public IP — fetched on the **default** route (NOT through any
/// tunnel) so it's the IP a leak would expose. Uses the neutral reflector. `None`
/// when unreachable; the verdict then can't prove a leak but still verifies the
/// data path (see [`verdict`]).
#[must_use]
pub fn wan_ip() -> Option<String> {
    let out = std::process::Command::new("curl")
        .args([
            "-s",
            "-m",
            "10",
            "-o",
            "/dev/stdout",
            "-w",
            "\n%{http_code}",
            vpn_providers::NEUTRAL_EXIT_CHECK_HOST,
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let body = text.trim_end().rsplit_once('\n').map_or("", |(b, _)| b);
    exit_ip_from_body(body)
}

/// A DNS-leak probe: resolve a name *binding to the tunnel iface* and report
/// whether the lookup leaked to the WAN. We can only positively detect a leak
/// when the tunnel-bound resolution *fails* while the WAN-bound one *succeeds*
/// for the same name — i.e. DNS only works off-tunnel, so any resolution the box
/// does is escaping the tunnel. A conservative, false-positive-free probe: it
/// returns `true` (leak) only on that asymmetry, never on a transient failure of
/// both paths. `spawn == false` (tests) short-circuits to "no leak".
#[must_use]
pub fn dns_leaked(spawn: bool, ifname: &str) -> bool {
    if !spawn {
        return false;
    }
    let resolves = |bind: Option<&str>| -> bool {
        let mut cmd = std::process::Command::new("getent");
        cmd.arg("ahostsv4").arg("one.one.one.one");
        // getent can't bind an interface; use `ping -I <ifname> -c1` resolution
        // as the tunnel-bound probe and a plain getent as the WAN-bound one.
        match bind {
            Some(iface) => std::process::Command::new("ping")
                .args(["-I", iface, "-c", "1", "-W", "3", "one.one.one.one"])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false),
            None => cmd
                .output()
                .map(|o| o.status.success() && !o.stdout.is_empty())
                .unwrap_or(false),
        }
    };
    let wan_resolves = resolves(None);
    let tunnel_resolves = resolves(Some(ifname));
    // Leak ⇔ DNS only works off the tunnel (WAN yes, tunnel no): the name path
    // is escaping the tunnel. Both-up or both-down is not a provable leak.
    wan_resolves && !tunnel_resolves
}

/// Verify a tunnel **named on a chain** (`id`), resolving its [`TunnelDef`] from
/// the durable config: the real tunnel via [`verify_tunnel`], or a synthetic
/// [`Health::Down`] report when the chain references a tunnel the config no
/// longer holds (so the route fails over off it). The single place both the
/// periodic [`sweep`] and the operator's `egress-health` read build a report,
/// so the "missing-tunnel ⇒ Down" rule lives once.
#[must_use]
pub fn verify_chain_tunnel(
    spawn: bool,
    id: &str,
    cfg: &vpn::VpnConfig,
    wan: Option<&str>,
) -> TunnelReport {
    cfg.get(id).map_or_else(
        || TunnelReport {
            id: id.to_string(),
            ifname: format!("mvpn-{id}"),
            health: Health::Down,
            verified_exit_ip: None,
            wan_ip: wan
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            detail: "no such tunnel in config".to_string(),
        },
        |t| verify_tunnel(spawn, t, wan),
    )
}

/// Verify ONE tunnel end to end (spawn side): liveness, exit IP through the
/// tunnel, the WAN IP, and the DNS-leak probe → a [`TunnelReport`]. `spawn`
/// false (tests) yields a deterministic `Down`/`Unverifiable` without touching
/// the host.
#[must_use]
pub fn verify_tunnel(spawn: bool, t: &TunnelDef, wan: Option<&str>) -> TunnelReport {
    let ifname = t.ifname();
    let iface_present = crate::ipc::vpn_gw::iface_up_public(spawn, &ifname);
    let (exit, attested) = if spawn && iface_present {
        probe_through(&ifname, exit_check_url(t)).map_or((None, false), |body| {
            (exit_ip_from_body(&body), mullvad_self_attested(&body))
        })
    } else {
        (None, false)
    };
    let dns_leak = iface_present && exit.is_some() && dns_leaked(spawn, &ifname);
    let health = verdict(iface_present, exit.as_deref(), wan, dns_leak);
    let detail = match health {
        Health::Ok if attested => "exit confirmed (provider self-attested)".to_string(),
        Health::Ok => "exit confirmed (≠ WAN)".to_string(),
        Health::Down => format!("interface {ifname} absent"),
        Health::Leaking => "LEAK: exit IP equals the WAN IP — egress is not tunneling".to_string(),
        Health::DnsLeak => "DNS leak: lookups resolve via the WAN resolver".to_string(),
        Health::Unverifiable => "exit IP unverifiable (reflector unreachable)".to_string(),
    };
    TunnelReport {
        id: t.id.clone(),
        ifname,
        health,
        verified_exit_ip: exit,
        wan_ip: wan
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        detail,
    }
}

/// Verify every tunnel on every durable route's chain, decide failover per route,
/// and raise a `vpn/tunnel-down` alert for each unhealthy tunnel. Returns the
/// per-tunnel reports (cached by the sweep so `egress-health` can read the
/// verified exit IPs). The alert is best-effort (a write failure is logged, never
/// fatal) — the same persist the responder already holds.
pub fn sweep(persist: &Persist, root: &std::path::Path, spawn: bool) -> Vec<TunnelReport> {
    let routing = vpn_egress::load_routing(root);
    let cfg = vpn::load(root);
    let wan = if spawn { wan_ip() } else { None };

    // Probe each tunnel that appears on some chain exactly once (a tunnel can be
    // a primary for one route and a failover for another).
    let mut reports: HashMap<String, TunnelReport> = HashMap::new();
    for route in &routing.route {
        for id in route.chain() {
            if reports.contains_key(&id) {
                continue;
            }
            let report = verify_chain_tunnel(spawn, &id, &cfg, wan.as_deref());
            reports.insert(id, report);
        }
    }

    let health_map: HashMap<String, Health> = reports
        .iter()
        .map(|(id, r)| (id.clone(), r.health))
        .collect();

    // Per route, decide the active tunnel + alert on the chain's failures.
    for route in &routing.route {
        let active = failover(route, &health_map);
        for id in route.chain() {
            let Some(report) = reports.get(&id) else {
                continue;
            };
            if report.health.is_active() {
                continue;
            }
            raise_tunnel_down(persist, route, report, active.as_deref());
        }
    }

    let mut out: Vec<TunnelReport> = reports.into_values().collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Raise the `vpn/tunnel-down` alert on `event/vpn/signals` for one failed tunnel
/// on a route: the kind, the failing tunnel + its verdict, the route target +
/// gateway, the tunnel it failed over to (or `null` ⇒ the kill-switch engaged).
fn raise_tunnel_down(
    persist: &Persist,
    route: &EgressRoute,
    report: &TunnelReport,
    active: Option<&str>,
) {
    let body = json!({
        "alert": "vpn/tunnel-down",
        "target": route.target.key(),
        "gateway": route.gateway,
        "tunnel": report.id,
        "health": report.health.tag(),
        "detail": report.detail,
        // What the chain did about it: the tunnel now carrying egress, or null
        // when the whole chain failed and the kill-switch (if set) blocks.
        "failed_over_to": active,
        "kill_switch_engaged": active.is_none() && route.kill_switch,
    });
    if let Err(e) = persist.write(
        VPN_EVENT_TOPIC,
        Priority::Default,
        None,
        Some(&body.to_string()),
    ) {
        tracing::warn!(error = %e, tunnel = %report.id, "vpn health: alert write failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::vpn::Method;
    use mackes_mesh_types::vpn_egress::RouteTarget;

    fn tdef(id: &str, provider: &str) -> TunnelDef {
        TunnelDef {
            id: id.into(),
            provider: provider.into(),
            method: Method::Wg,
            server: String::new(),
            protocol: "udp".into(),
            creds_ref: String::new(),
        }
    }

    fn route(primary: &str, failover: &[&str], kill_switch: bool) -> EgressRoute {
        EgressRoute {
            target: RouteTarget::Any,
            gateway: "gw1".into(),
            primary: primary.into(),
            failover: failover.iter().map(|s| (*s).to_string()).collect(),
            kill_switch,
        }
    }

    #[test]
    fn topic_locks() {
        assert_eq!(VPN_EVENT_TOPIC, "event/vpn/signals");
    }

    #[test]
    fn verdict_down_when_iface_absent() {
        assert_eq!(
            verdict(false, Some("1.2.3.4"), Some("5.6.7.8"), false),
            Health::Down
        );
    }

    #[test]
    fn verdict_leaking_when_exit_equals_wan() {
        // The silent leak: iface up, but the exit IP IS the WAN IP.
        assert_eq!(
            verdict(true, Some("203.0.113.7"), Some("203.0.113.7"), false),
            Health::Leaking
        );
        // Case-insensitive (IPv6 hex).
        assert_eq!(
            verdict(true, Some("2001:DB8::1"), Some("2001:db8::1"), false),
            Health::Leaking
        );
    }

    #[test]
    fn verdict_ok_when_exit_differs_from_wan() {
        assert_eq!(
            verdict(true, Some("198.51.100.9"), Some("203.0.113.7"), false),
            Health::Ok
        );
    }

    #[test]
    fn verdict_dns_leak_only_when_data_path_is_clean() {
        // Exit differs from WAN (data path fine) but DNS leaked → DnsLeak.
        assert_eq!(
            verdict(true, Some("198.51.100.9"), Some("203.0.113.7"), true),
            Health::DnsLeak
        );
        // A data-path leak takes precedence over the DNS flag (worst wins).
        assert_eq!(
            verdict(true, Some("203.0.113.7"), Some("203.0.113.7"), true),
            Health::Leaking
        );
    }

    #[test]
    fn verdict_unverifiable_when_exit_unknown() {
        assert_eq!(
            verdict(true, None, Some("203.0.113.7"), false),
            Health::Unverifiable
        );
        assert_eq!(
            verdict(true, Some("   "), None, false),
            Health::Unverifiable
        );
    }

    #[test]
    fn verdict_ok_without_wan_when_exit_came_through_tunnel() {
        // The exit IP came back through the tunnel but we couldn't read the WAN
        // IP — we can't *prove* a leak, so the data path is treated as verified.
        assert_eq!(verdict(true, Some("198.51.100.9"), None, false), Health::Ok);
    }

    #[test]
    fn active_health_keeps_only_ok_and_unverifiable() {
        assert!(Health::Ok.is_active());
        assert!(Health::Unverifiable.is_active());
        assert!(!Health::Down.is_active());
        assert!(!Health::Leaking.is_active());
        assert!(!Health::DnsLeak.is_active());
    }

    #[test]
    fn exit_ip_parses_ipinfo_and_mullvad_bodies() {
        assert_eq!(
            exit_ip_from_body(r#"{"ip":"198.51.100.9","city":"NYC"}"#),
            Some("198.51.100.9".into())
        );
        assert_eq!(
            exit_ip_from_body(r#"{"ip":"203.0.113.7","mullvad_exit_ip":true}"#),
            Some("203.0.113.7".into())
        );
        // A captive-portal HTML page → no IP (Unverifiable, not a bogus IP).
        assert_eq!(exit_ip_from_body("<html>nope</html>"), None);
        assert_eq!(exit_ip_from_body(r#"{"ip":""}"#), None);
    }

    #[test]
    fn mullvad_self_attestation_reads_the_flag() {
        assert!(mullvad_self_attested(
            r#"{"ip":"203.0.113.7","mullvad_exit_ip":true}"#
        ));
        assert!(!mullvad_self_attested(
            r#"{"ip":"203.0.113.7","mullvad_exit_ip":false}"#
        ));
        // ipinfo body has no such field → not attested.
        assert!(!mullvad_self_attested(r#"{"ip":"198.51.100.9"}"#));
    }

    #[test]
    fn exit_check_url_resolves_per_provider() {
        assert_eq!(
            exit_check_url(&tdef("m1", "mullvad")),
            "https://am.i.mullvad.net/json"
        );
        // A non-first-party provider → the neutral reflector.
        assert_eq!(
            exit_check_url(&tdef("p1", "proton")),
            "https://ipinfo.io/json"
        );
        // An unknown label still resolves (generic-wg → neutral), never panics.
        assert_eq!(
            exit_check_url(&tdef("x1", "weird")),
            "https://ipinfo.io/json"
        );
    }

    #[test]
    fn failover_walks_past_a_leaking_tunnel() {
        // The core VPN-GW-6 behavior: a LEAKING primary is failed over exactly
        // like a down one — the next healthy tunnel becomes active.
        let r = route("mullvad1", &["proton1", "ivpn1"], true);
        let mut health = HashMap::new();
        health.insert("mullvad1".to_string(), Health::Leaking);
        health.insert("proton1".to_string(), Health::Ok);
        health.insert("ivpn1".to_string(), Health::Ok);
        assert_eq!(failover(&r, &health), Some("proton1".into()));

        // Primary leaking + first failover down → the second failover wins.
        health.insert("proton1".to_string(), Health::Down);
        assert_eq!(failover(&r, &health), Some("ivpn1".into()));
    }

    #[test]
    fn failover_none_engages_kill_switch_when_whole_chain_fails() {
        let r = route("mullvad1", &["proton1"], true);
        let mut health = HashMap::new();
        health.insert("mullvad1".to_string(), Health::Leaking);
        health.insert("proton1".to_string(), Health::Down);
        // Whole chain unhealthy → no active tunnel → caller engages kill-switch.
        assert_eq!(failover(&r, &health), None);
    }

    #[test]
    fn failover_treats_unprobed_tunnel_as_live() {
        // Cold-start guard: a chain tunnel with no health entry is not killed.
        let r = route("mullvad1", &[], true);
        let health: HashMap<String, Health> = HashMap::new();
        assert_eq!(failover(&r, &health), Some("mullvad1".into()));
    }

    #[test]
    fn verify_tunnel_without_spawn_is_down_and_does_not_touch_host() {
        let t = tdef("mullvad1", "mullvad");
        let report = verify_tunnel(false, &t, Some("203.0.113.7"));
        assert_eq!(report.health, Health::Down);
        assert_eq!(report.ifname, "mvpn-mullvad1");
        assert_eq!(report.verified_exit_ip, None);
        // The compared WAN IP is carried for the leak proof.
        assert_eq!(report.wan_ip.as_deref(), Some("203.0.113.7"));
    }

    #[test]
    fn report_json_shape_carries_the_verified_exit_ip() {
        let report = TunnelReport {
            id: "mullvad1".into(),
            ifname: "mvpn-mullvad1".into(),
            health: Health::Ok,
            verified_exit_ip: Some("198.51.100.9".into()),
            wan_ip: Some("203.0.113.7".into()),
            detail: "exit confirmed".into(),
        };
        let v = report.to_json();
        assert_eq!(v["health"], "ok");
        assert_eq!(v["verified_exit_ip"], "198.51.100.9");
        assert_eq!(v["wan_ip"], "203.0.113.7");
    }

    #[test]
    fn sweep_without_spawn_reports_each_chain_tunnel_down_once() {
        // A persist-backed end-to-end sweep with spawn off: every chain tunnel
        // reports Down (no iface), deduped across routes, no host I/O.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Two routes sharing a tunnel ("proton1" is failover for both).
        let mut routing = vpn_egress::EgressRouting::default();
        routing.set(EgressRoute {
            target: RouteTarget::Any,
            gateway: "gw1".into(),
            primary: "mullvad1".into(),
            failover: vec!["proton1".into()],
            kill_switch: true,
        });
        routing.set(EgressRoute {
            target: RouteTarget::Node {
                name: "anvil".into(),
            },
            gateway: "gw1".into(),
            primary: "ivpn1".into(),
            failover: vec!["proton1".into()],
            kill_switch: true,
        });
        vpn_egress::save_routing(root, &routing).unwrap();

        let bus = tempfile::tempdir().unwrap();
        let persist = Persist::open(bus.path().to_path_buf()).unwrap();
        let reports = sweep(&persist, root, false);
        // mullvad1, proton1, ivpn1 — three distinct tunnels, proton1 deduped.
        let ids: Vec<&str> = reports.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["ivpn1", "mullvad1", "proton1"], "{ids:?}");
        assert!(reports.iter().all(|r| r.health == Health::Down));

        // The alert really landed on the bus topic for a failed tunnel.
        let msgs = persist.list_since(VPN_EVENT_TOPIC, None).unwrap();
        assert!(!msgs.is_empty(), "expected vpn/tunnel-down alerts");
        let body = msgs[0].body.as_deref().unwrap();
        let v: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(v["alert"], "vpn/tunnel-down");
        // Whole chain down → no failover target → kill-switch engaged.
        assert_eq!(v["failed_over_to"], serde_json::Value::Null);
        assert_eq!(v["kill_switch_engaged"], serde_json::Value::Bool(true));
    }
}
