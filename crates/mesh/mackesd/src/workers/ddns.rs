//! DDNS-EGRESS-1 — `ddns` worker + egress-IP change detection.
//!
//! Per `docs/design/ddns-egress.md`, dynamic DNS for the mesh has two
//! halves: an **IP-discovery** layer that learns the node's current
//! egress address(es), and a **DNS-writer** layer (DigitalOcean adapter,
//! DDNS-EGRESS-2) that publishes those addresses as A/AAAA records under
//! `services.matthewmackes.com`. This worker is the discovery half: on a
//! periodic tick it determines the node's current **WAN / public egress
//! IP** and detects a CHANGE versus the last-seen value, persisting that
//! value so a change *across daemon restarts* is also caught. On a real
//! change it publishes an `event/egress-ip/<host>` Bus event the DDNS
//! writer (DDNS-EGRESS-2) consumes — and records it in the persisted
//! state so the very next writer tick reconciles even if it missed the
//! transient Bus event.
//!
//! ## The IP-source seam (DDNS-EGRESS-3 deferred)
//!
//! The design tracks two kinds of egress: the node WAN IP (classic home
//! DDNS, implemented here) and the **per-VPN-tunnel exit IP**
//! (DDNS-EGRESS-3, which depends on VPN-GW and is NOT in this worktree).
//! Both are "what address does traffic leave by" — so the IP source sits
//! behind a small [`EgressIpSource`] trait. This task ships the
//! [`WanEgressSource`]; the VPN-tunnel source is a future `impl
//! EgressIpSource` that reads VPN-GW-6's verified exit IP. The worker
//! loop is written against the trait + a `Vec<Box<dyn EgressIpSource>>`,
//! so adding the VPN source later is additive — no worker rewrite.
//! Per §7 the VPN source is **deferred, not stubbed**: it simply doesn't
//! exist yet, and the worker spawns with only the WAN source.
//!
//! ## Degrading gracefully when offline
//!
//! WAN discovery has two independent probes (design "IP discovery"):
//!   * the **default-route local egress address** — the source IP the
//!     kernel would use for an off-link destination (a UDP socket
//!     `connect()` with no packet sent — purely a routing-table lookup),
//!     which works fully offline behind a router, and
//!   * a **public IP echo** (`curl https://ipinfo.io/json`, reusing the
//!     [`super::netassess`] parser) — the address the *internet* sees,
//!     which is the true WAN IP behind NAT but needs connectivity.
//! When both fail the tick yields `None` (offline) — and an offline tick
//! is **not** treated as a change to a sentinel; the last-known IP is
//! retained so a transient outage doesn't churn a DNS record to nothing.
//!
//! ## Pure, unit-tested core
//!
//! All parse + change-detection + persistence-shape logic is in pure
//! functions ([`detect_change`], [`EgressState`] (de)serialization,
//! [`local_egress_addr`] given a socket factory) so it is unit-tested
//! without a network, a Bus, or a running daemon.

#![cfg(feature = "async-services")]

use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use super::{ShutdownToken, Worker};

/// Default discovery cadence. The design wants a short DNS TTL (~60 s) so
/// a reconnect propagates fast; the *discovery* tick is the floor on how
/// quickly we notice a change, so 60 s matches the record TTL without
/// hammering the public IP-echo endpoint. Only a real change writes
/// anything downstream, so a steady WAN IP costs one cheap probe/minute.
pub const DEFAULT_TICK: Duration = Duration::from_secs(60);

/// Canonical persisted-state path: `/var/lib/mackesd/egress-ip.json`.
/// Survives daemon restart so a WAN IP that changed *while mackesd was
/// down* is still detected on the next boot (design: "diff the last-
/// published value … persisted").
pub const DEFAULT_STATE_PATH: &str = "/var/lib/mackesd/egress-ip.json";

/// Public IP-echo endpoint (reuses the [`super::netassess`] ipinfo
/// parser). IP-echo gives the post-NAT address the internet sees — the
/// true dynamic WAN IP. Kept as a const so a future operator-config can
/// override it without touching the probe logic.
pub const IP_ECHO_URL: &str = "https://ipinfo.io/json";

/// Off-link probe target for the routing-table lookup. We never send a
/// packet — `connect()` on a UDP socket only resolves the route and binds
/// the local source address — so the target need not be reachable; it
/// only has to be a routable off-link address that selects the default
/// route. Cloudflare's `1.1.1.1` / its v6 sibling are stable, well-known
/// off-link anchors.
const ROUTE_PROBE_V4: &str = "1.1.1.1:80";
const ROUTE_PROBE_V6: &str = "[2606:4700:4700::1111]:80";

/// Bus topic for an egress-IP change: `event/egress-ip/<host>`. Mirrors
/// the `event/firewall/<host>` / `event/printers/<host>` family.
#[must_use]
pub fn egress_topic(host: &str) -> String {
    format!("event/egress-ip/{host}")
}

/// Which kind of egress an [`EgressReading`] describes. WAN is the node's
/// public/dynamic address; `Tunnel` is the per-VPN-tunnel exit IP that
/// DDNS-EGRESS-3 will add behind the same [`EgressIpSource`] trait (the
/// variant exists now so the persisted-state schema + the change-key are
/// forward-compatible — no migration when the VPN source lands).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum EgressKind {
    /// The node's WAN / public egress IP (classic home DDNS).
    Wan,
    /// A VPN tunnel's verified exit IP. `id` is the VPN-GW tunnel id
    /// (e.g. `mullvad-1`). Reserved for DDNS-EGRESS-3.
    Tunnel {
        /// VPN-GW tunnel identifier.
        id: String,
    },
}

impl EgressKind {
    /// Stable record key for change-tracking + the persisted map. WAN is
    /// `"wan"`; a tunnel is `"tunnel:<id>"`. Matches the design's
    /// `source = "wan"` / `source = "tunnel:mullvad-1"` config shape.
    #[must_use]
    pub fn key(&self) -> String {
        match self {
            EgressKind::Wan => "wan".to_owned(),
            EgressKind::Tunnel { id } => format!("tunnel:{id}"),
        }
    }
}

/// One egress observation from a source: the kind + the address(es) it
/// currently presents. Either or both family addresses may be present;
/// a fully-empty reading means "this source has no address right now"
/// (offline / tunnel down) and is distinct from "source errored".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EgressReading {
    /// Current IPv4 egress address, if any.
    pub v4: Option<String>,
    /// Current IPv6 egress address, if any.
    pub v6: Option<String>,
}

impl EgressReading {
    /// True when the source presented no address at all (offline / down).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.v4.is_none() && self.v6.is_none()
    }
}

/// The seam DDNS-EGRESS-3 plugs into. A source knows what *kind* of
/// egress it tracks and can produce a current [`EgressReading`]. Async so
/// the WAN source's IP-echo (a shell-out to `curl`) — and the future VPN
/// source's exit-IP verification — don't block the tokio scheduler.
///
/// DDNS-EGRESS-3 (deferred, VPN-GW-dependent) will add a
/// `TunnelEgressSource { tunnel_id }` implementing this trait against
/// VPN-GW-6's verified exit IP. No worker change is needed to adopt it —
/// the worker iterates `Vec<Box<dyn EgressIpSource>>`.
#[async_trait::async_trait]
pub trait EgressIpSource: Send + Sync {
    /// The egress this source tracks (its persisted-state key).
    fn kind(&self) -> EgressKind;

    /// Discover the current egress address(es). `Ok(reading)` where the
    /// reading may be empty (offline / down — *not* an error); `Err`
    /// only for a genuine probe failure the caller should log. The WAN
    /// source treats "no connectivity" as an empty reading, not an error,
    /// so a transient outage doesn't churn the record.
    async fn current(&self) -> anyhow::Result<EgressReading>;
}

// ── WAN source ──────────────────────────────────────────────────────

/// The node's WAN / public egress IP source (the only source this task
/// ships). Combines two probes — the routing-table local source address
/// (offline-safe) and the public IP echo (true post-NAT WAN IP) —
/// preferring the public echo when both are available.
pub struct WanEgressSource {
    echo_url: String,
    echo_timeout: Duration,
}

impl Default for WanEgressSource {
    fn default() -> Self {
        Self {
            echo_url: IP_ECHO_URL.to_owned(),
            echo_timeout: Duration::from_secs(5),
        }
    }
}

impl WanEgressSource {
    /// Construct with the default IP-echo endpoint + 5 s probe budget.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Override the IP-echo URL (operator-config seam / tests).
    #[must_use]
    pub fn with_echo_url(mut self, url: impl Into<String>) -> Self {
        self.echo_url = url.into();
        self
    }
}

#[async_trait::async_trait]
impl EgressIpSource for WanEgressSource {
    fn kind(&self) -> EgressKind {
        EgressKind::Wan
    }

    async fn current(&self) -> anyhow::Result<EgressReading> {
        // Probe 1 (offline-safe): the kernel's chosen source address for
        // the default route. A blocking routing-table lookup → hop onto a
        // blocking task so we never pin the scheduler.
        let local = tokio::task::spawn_blocking(local_egress_reading)
            .await
            .unwrap_or_default();

        // Probe 2 (needs connectivity): the post-NAT public IP the
        // internet sees. This is the *true* WAN IP behind a NAT router;
        // it overrides probe 1's v4 (probe 1 sees the RFC-1918 LAN
        // address behind NAT). Reuses netassess's ipinfo parser.
        let public = self.public_echo().await;

        let mut reading = local;
        if let Some(pub_ip) = public {
            match pub_ip {
                IpAddr::V4(v4) => reading.v4 = Some(v4.to_string()),
                IpAddr::V6(v6) => reading.v6 = Some(v6.to_string()),
            }
        }
        Ok(reading)
    }
}

impl WanEgressSource {
    /// Run the public IP echo, returning the parsed address or `None`
    /// when offline / the endpoint is unreachable (graceful degrade — an
    /// offline echo is not an error). Reuses [`super::netassess`]'s
    /// ipinfo parser so the JSON shape lives in exactly one place.
    async fn public_echo(&self) -> Option<IpAddr> {
        let url = self.echo_url.clone();
        let secs = self.echo_timeout.as_secs().max(1).to_string();
        let out = tokio::task::spawn_blocking(move || {
            let mut cmd = Command::new("curl");
            cmd.args(["-s", "--max-time", &secs, &url]);
            crate::workers::proc::output_with_timeout(
                cmd,
                crate::workers::proc::DEFAULT_CMD_TIMEOUT,
            )
            .ok()
        })
        .await
        .ok()
        .flatten()?;
        if !out.status.success() {
            return None;
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        let info = super::netassess::parse_ipinfo_json(&stdout)?;
        parse_ip(&info.ip)
    }
}

/// Parse a string as an [`IpAddr`], trimming whitespace. Pure.
#[must_use]
pub fn parse_ip(s: &str) -> Option<IpAddr> {
    s.trim().parse::<IpAddr>().ok()
}

/// Resolve the local source address the kernel would use for the default
/// route, per family, by `connect()`-ing a UDP socket to an off-link
/// anchor (no packet is sent — `connect` on a datagram socket only does
/// the routing-table lookup + binds the source address). Offline-safe
/// behind a router: returns the LAN/ULA source address even with no
/// internet. Returns an empty reading when there is no route at all.
#[must_use]
pub fn local_egress_reading() -> EgressReading {
    EgressReading {
        v4: local_egress_addr(ROUTE_PROBE_V4).map(|ip| ip.to_string()),
        v6: local_egress_addr(ROUTE_PROBE_V6).map(|ip| ip.to_string()),
    }
}

/// The routing-table source-address lookup for one off-link `target`
/// (`ip:port`). Binds an unspecified-address UDP socket of the matching
/// family, `connect()`s it (route lookup only — no datagram leaves the
/// host), and reads back the bound `local_addr`. `None` when the family
/// has no route (e.g. an IPv4-only host probing a v6 target). Pure given
/// the OS routing table; takes a `&str` so tests drive it directly.
#[must_use]
pub fn local_egress_addr(target: &str) -> Option<IpAddr> {
    let remote: SocketAddr = target.parse().ok()?;
    let bind: &str = if remote.is_ipv6() {
        "[::]:0"
    } else {
        "0.0.0.0:0"
    };
    let socket = UdpSocket::bind(bind).ok()?;
    socket.connect(remote).ok()?;
    let local = socket.local_addr().ok()?.ip();
    if local.is_unspecified() {
        return None;
    }
    Some(local)
}

// ── VPN-tunnel source (DDNS-EGRESS-3) ───────────────────────────────

/// DDNS-EGRESS-3 — the per-VPN-tunnel exit-IP source. Reads VPN-GW-6's
/// published `vpn/tunnel-health.json` (the `vpn_gateway` worker's
/// [`HealthState`](crate::workers::vpn_gateway::HealthState)) and yields the
/// **verified** exit IP for one tunnel under the existing
/// [`EgressKind::Tunnel`] / `tunnel:<id>` key — so a tunnel's verified exit IP
/// flows into the SAME change-detect → publish → DnsWriter path the WAN IP
/// uses. Adding this to the worker's `Vec<Box<dyn EgressIpSource>>` is purely
/// additive (DDNS-EGRESS-1 designed the seam for exactly this) — no worker
/// rewrite.
///
/// Only a **healthy** tunnel's exit IP is published: VPN-GW-6's verdict already
/// folds in liveness + "the exit IP is the provider's, not the plaintext WAN" +
/// the DNS-leak probe, so a `Leaking`/`Down` tunnel yields an *empty* reading
/// (the source is "down" for DDNS) rather than a stale/leaking address. That
/// keeps DDNS from ever publishing a record that points at a dead or leaking
/// exit — the same no-stale-record guarantee the WAN source has for an outage.
pub struct TunnelEgressSource {
    tunnel_id: String,
    health_path: PathBuf,
}

impl TunnelEgressSource {
    /// Build a source for `tunnel_id`, reading the gateway worker's published
    /// health under `workgroup_root` (`<root>/vpn/tunnel-health.json`).
    #[must_use]
    pub fn new(tunnel_id: impl Into<String>, workgroup_root: &Path) -> Self {
        Self {
            tunnel_id: tunnel_id.into(),
            health_path: crate::workers::vpn_gateway::default_health_path(workgroup_root),
        }
    }

    /// Override the health-state path (tests use a tempdir fixture).
    #[must_use]
    pub fn with_health_path(mut self, path: PathBuf) -> Self {
        self.health_path = path;
        self
    }

    /// Resolve the verified exit-IP reading from a loaded health state. Pure
    /// over the state so it is unit-tested from a fixture: an empty reading
    /// when the tunnel is absent / not `Healthy` / has no observed exit IP
    /// (each of which means "no address to publish right now"), else the
    /// verified exit IP routed into the matching v4/v6 family.
    #[must_use]
    pub fn reading_from(&self, state: &crate::workers::vpn_gateway::HealthState) -> EgressReading {
        let Some(health) = state.tunnel.get(&self.tunnel_id) else {
            return EgressReading::default();
        };
        // Only a Healthy verdict yields a publishable IP — Leaking/Down are
        // "down" for DDNS so we never publish a stale/leaking exit.
        if !health.verdict.is_up() {
            return EgressReading::default();
        }
        let Some(exit_ip) = health.exit_ip.as_deref().and_then(parse_ip) else {
            return EgressReading::default();
        };
        match exit_ip {
            IpAddr::V4(v4) => EgressReading {
                v4: Some(v4.to_string()),
                v6: None,
            },
            IpAddr::V6(v6) => EgressReading {
                v4: None,
                v6: Some(v6.to_string()),
            },
        }
    }
}

#[async_trait::async_trait]
impl EgressIpSource for TunnelEgressSource {
    fn kind(&self) -> EgressKind {
        EgressKind::Tunnel {
            id: self.tunnel_id.clone(),
        }
    }

    async fn current(&self) -> anyhow::Result<EgressReading> {
        // A plain JSON read; hop onto a blocking task so a slow fs read never
        // pins the tokio scheduler (mirrors the WAN source's blocking hops).
        let path = self.health_path.clone();
        let state = tokio::task::spawn_blocking(move || {
            crate::workers::vpn_gateway::HealthState::load(&path)
        })
        .await
        .unwrap_or_default();
        Ok(self.reading_from(&state))
    }
}

/// DDNS-EGRESS-3 — build one [`TunnelEgressSource`] per tunnel the gateway
/// worker has published health for, so every tracked tunnel's verified exit IP
/// flows through the worker. Reads `<workgroup_root>/vpn/tunnel-health.json`;
/// an absent file (no VPN-GW worker has run yet) yields no sources — additive
/// and graceful. Returned boxed so the worker appends them to its source `Vec`.
#[must_use]
pub fn tunnel_sources(workgroup_root: &Path) -> Vec<Box<dyn EgressIpSource>> {
    let health_path = crate::workers::vpn_gateway::default_health_path(workgroup_root);
    let state = crate::workers::vpn_gateway::HealthState::load(&health_path);
    state
        .tunnel
        .keys()
        .map(|id| {
            Box::new(TunnelEgressSource::new(id.clone(), workgroup_root)) as Box<dyn EgressIpSource>
        })
        .collect()
}

// ── Persisted state + change detection (pure) ───────────────────────

/// The persisted last-seen egress map: record-key → last reading. Stored
/// as JSON at [`DEFAULT_STATE_PATH`] so a change across daemon restart is
/// still detected (load → compare → on diff, publish + persist).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressState {
    /// Map of record key (`"wan"`, `"tunnel:<id>"`) → last reading.
    #[serde(default)]
    pub last: std::collections::BTreeMap<String, EgressReading>,
}

impl EgressState {
    /// Parse persisted state from JSON. A missing/corrupt file → an
    /// empty state (fail-open: a parse failure must not wedge discovery;
    /// the next reading simply registers as a first-seen change).
    #[must_use]
    pub fn from_json(s: &str) -> Self {
        serde_json::from_str(s).unwrap_or_default()
    }

    /// Serialize to pretty JSON for atomic write-back.
    ///
    /// # Errors
    /// Propagates a `serde_json` serialization failure (effectively
    /// never for this plain map, but surfaced rather than swallowed).
    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    /// Load from `path`, fail-open to empty when absent/unreadable.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(s) => Self::from_json(&s),
            Err(_) => Self::default(),
        }
    }

    /// Atomically persist to `path` (write a temp sibling + rename), so a
    /// crash mid-write can't leave a truncated state file. Creates the
    /// parent dir if needed.
    ///
    /// # Errors
    /// I/O failures creating the dir, writing the temp file, or renaming.
    pub fn store(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = self
            .to_json()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json.as_bytes())?;
        std::fs::rename(&tmp, path)
    }
}

/// The verdict of comparing a fresh reading against the last-seen one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeOutcome {
    /// First time we've seen this record at all (no prior state) — counts
    /// as a change so the DNS record gets created on first boot.
    FirstSeen,
    /// The address(es) changed versus the last-seen non-empty reading.
    Changed,
    /// Same address(es) as last seen — no DNS write needed.
    Unchanged,
    /// The fresh reading is empty (offline / tunnel down) while we have a
    /// prior non-empty value. We deliberately do **not** treat this as a
    /// change to a sentinel: a transient outage must not churn a live
    /// record to nothing. The DDNS writer's `on_down` policy
    /// (DDNS-EGRESS-2) owns down-handling, not this discovery tick — so
    /// the last-seen value is retained and we report `WentOffline` for
    /// observability without overwriting state.
    WentOffline,
}

impl ChangeOutcome {
    /// Whether this outcome should fire the change event + persist the new
    /// reading. `FirstSeen` and `Changed` do; `Unchanged` and
    /// `WentOffline` do not (the latter retains the last-seen value).
    #[must_use]
    pub fn should_publish(&self) -> bool {
        matches!(self, ChangeOutcome::FirstSeen | ChangeOutcome::Changed)
    }
}

/// Pure change-detection: compare a `fresh` reading for `key` against the
/// persisted `state`. Returns the outcome — the worker uses
/// [`ChangeOutcome::should_publish`] to decide whether to emit the Bus
/// event + write back state. Fully unit-testable without I/O.
#[must_use]
pub fn detect_change(state: &EgressState, key: &str, fresh: &EgressReading) -> ChangeOutcome {
    match state.last.get(key) {
        None => {
            if fresh.is_empty() {
                // Never seen + still offline: nothing to publish yet.
                ChangeOutcome::WentOffline
            } else {
                ChangeOutcome::FirstSeen
            }
        }
        Some(prev) => {
            if fresh.is_empty() {
                // Had a value, now offline → retain, don't churn.
                if prev.is_empty() {
                    ChangeOutcome::Unchanged
                } else {
                    ChangeOutcome::WentOffline
                }
            } else if prev == fresh {
                ChangeOutcome::Unchanged
            } else {
                ChangeOutcome::Changed
            }
        }
    }
}

/// JSON body for the `event/egress-ip/<host>` Bus event. Carries enough
/// for the DDNS writer (DDNS-EGRESS-2) to reconcile without re-probing:
/// the host, the record key, the kind, the new reading, and whether it
/// was a first-seen create. Pure — built from owned values, no I/O.
#[must_use]
pub fn change_payload(
    host: &str,
    kind: &EgressKind,
    reading: &EgressReading,
    first_seen: bool,
) -> String {
    let body = serde_json::json!({
        "host": host,
        "source": kind.key(),
        "kind": kind,
        "reading": reading,
        "first_seen": first_seen,
    });
    body.to_string()
}

// ── Worker ──────────────────────────────────────────────────────────

/// The supervised `ddns` worker. On each tick it polls every configured
/// [`EgressIpSource`], runs [`detect_change`] against the persisted
/// [`EgressState`], and on a real change publishes `event/egress-ip/
/// <host>` + persists the new reading. Ships with the [`WanEgressSource`]
/// only; DDNS-EGRESS-3 adds the VPN-tunnel source to the same `Vec`.
pub struct DdnsWorker {
    host: String,
    state_path: PathBuf,
    tick: Duration,
    sources: Vec<Box<dyn EgressIpSource>>,
    /// DDNS-EGRESS-2 — the DNS reconciler driven on a real egress change.
    /// The production reconciler builds the DigitalOcean [`DnsWriter`]
    /// from the persisted `[ddns]` config + the encrypted token; a test
    /// injects a capturing reconciler. `None` only in legacy discovery-
    /// only tests; production always wires one (see [`DdnsWorker::new`]).
    /// An `Arc` so it can be cloned into the `spawn_blocking` hop the
    /// curl-shelling writer must run on.
    reconciler: Option<std::sync::Arc<dyn DnsReconciler>>,
}

/// The seam DDNS-EGRESS-2 plugs the DNS write into. On a real egress
/// change the worker hands the changed source key + the new reading (or
/// "down") to a reconciler, which loads the `[ddns]` config, resolves the
/// records bound to that source, and drives the DigitalOcean
/// [`DnsWriter`](crate::workers::ddns_writer::DnsWriter). Behind a trait
/// so the worker's wiring is unit-tested without a live DO token (the
/// production [`DoDnsReconciler`] needs the secret store + the network).
pub trait DnsReconciler: Send + Sync {
    /// Reconcile every configured record whose `source` matches `key`
    /// (`"wan"` / `"tunnel:<id>"`) against the fresh `reading`. An empty
    /// `reading` (source down) drives each record's `on_down` policy.
    /// Errors are logged inside the impl (one bad record must not wedge
    /// the others), so this returns the count of records reconciled.
    fn reconcile(&self, key: &str, reading: &EgressReading) -> usize;
}

impl DdnsWorker {
    /// Construct the WAN-only worker for `host`, persisting to
    /// [`DEFAULT_STATE_PATH`] at the [`DEFAULT_TICK`] cadence. The VPN
    /// source (DDNS-EGRESS-3) appends to `sources` when it lands. Wires
    /// the production [`DoDnsReconciler`] rooted at the default workgroup
    /// so a detected change actually writes DNS (DDNS-EGRESS-2).
    #[must_use]
    pub fn new(host: impl Into<String>) -> Self {
        let host = host.into();
        let workgroup_root = crate::default_qnm_shared_root();
        let reconciler: std::sync::Arc<dyn DnsReconciler> =
            std::sync::Arc::new(DoDnsReconciler::new(workgroup_root.clone(), host.clone()));
        // DDNS-EGRESS-3 — the WAN source ships always; the per-VPN-tunnel
        // verified-exit-IP sources (one per tunnel VPN-GW-6 has published
        // health for) plug into the SAME `Vec<Box<dyn EgressIpSource>>` — no
        // worker rewrite. Absent any VPN-GW health, `tunnel_sources` is empty
        // and the worker is WAN-only, exactly as before.
        let mut sources: Vec<Box<dyn EgressIpSource>> = vec![Box::new(WanEgressSource::new())];
        sources.extend(tunnel_sources(&workgroup_root));
        Self {
            host,
            state_path: PathBuf::from(DEFAULT_STATE_PATH),
            tick: DEFAULT_TICK,
            sources,
            reconciler: Some(reconciler),
        }
    }

    /// Override the reconciler (DDNS-EGRESS-2 wiring tests inject a
    /// capturing one; discovery-only tests drop it via
    /// [`without_reconciler`](DdnsWorker::without_reconciler)).
    #[must_use]
    pub fn with_reconciler(mut self, reconciler: std::sync::Arc<dyn DnsReconciler>) -> Self {
        self.reconciler = Some(reconciler);
        self
    }

    /// Drop the reconciler — for the legacy discovery-only tests that
    /// assert change-detection without a DO write.
    #[must_use]
    pub fn without_reconciler(mut self) -> Self {
        self.reconciler = None;
        self
    }

    /// Override the persisted-state path (tests use a tempdir).
    #[must_use]
    pub fn with_state_path(mut self, path: PathBuf) -> Self {
        self.state_path = path;
        self
    }

    /// Override the tick cadence (tests dial this down).
    #[must_use]
    pub fn with_tick(mut self, tick: Duration) -> Self {
        self.tick = tick;
        self
    }

    /// Replace the source list — the seam for DDNS-EGRESS-3 to inject a
    /// VPN-tunnel source (and for tests to inject a fake source).
    #[must_use]
    pub fn with_sources(mut self, sources: Vec<Box<dyn EgressIpSource>>) -> Self {
        self.sources = sources;
        self
    }

    /// One discovery pass: poll every source, detect changes against the
    /// on-disk state, publish + persist on a real change. Returns the
    /// number of records that changed (for tests + logging). Loads the
    /// state fresh each tick so an external edit / DDNS-EGRESS-2 write is
    /// respected, and persists only when something actually changed.
    pub async fn tick_once(&self) -> usize {
        let mut state = EgressState::load(&self.state_path);
        let mut changed = 0usize;
        for source in &self.sources {
            let kind = source.kind();
            let key = kind.key();
            let reading = match source.current().await {
                Ok(r) => r,
                Err(e) => {
                    warn!(source = %key, error = %e, "ddns: source probe failed; skipping");
                    continue;
                }
            };
            let outcome = detect_change(&state, &key, &reading);
            match &outcome {
                ChangeOutcome::FirstSeen | ChangeOutcome::Changed => {
                    let first_seen = matches!(outcome, ChangeOutcome::FirstSeen);
                    info!(
                        source = %key,
                        v4 = ?reading.v4,
                        v6 = ?reading.v6,
                        first_seen,
                        "ddns: egress IP changed; publishing event/egress-ip",
                    );
                    publish_change(&self.host, &kind, &reading, first_seen);
                    // DDNS-EGRESS-2 — drive the DigitalOcean DnsWriter for
                    // every record bound to this source so the change
                    // actually lands as a DNS upsert (the writer runs on a
                    // blocking hop — its curl shell-out must not pin the
                    // tokio worker).
                    self.reconcile(&key, &reading).await;
                    state.last.insert(key, reading);
                    changed += 1;
                }
                ChangeOutcome::WentOffline => {
                    // The source has no address now (offline / tunnel down).
                    // We retain the last-seen value in state (no churn) but
                    // DO hand it to the writer so the per-record `on_down`
                    // policy (remove / sentinel / keep) applies — a down
                    // exit must not leave a stale record silently pointing
                    // at a dead/leaking address (design acceptance).
                    debug!(source = %key, "ddns: source offline; applying on_down policy");
                    self.reconcile(&key, &reading).await;
                }
                ChangeOutcome::Unchanged => {
                    debug!(source = %key, "ddns: egress IP unchanged");
                }
            }
        }
        if changed > 0 {
            if let Err(e) = state.store(&self.state_path) {
                warn!(
                    path = %self.state_path.display(),
                    error = %e,
                    "ddns: failed to persist egress state (change will re-fire next tick)",
                );
            }
        }
        changed
    }

    /// Drive the DNS reconciler for one changed source on a blocking hop.
    /// The production reconciler shells to `curl` (DO API) + touches the
    /// filesystem (secret blob, config), so it must not run on the tokio
    /// scheduler thread — `spawn_blocking` keeps the runtime responsive.
    /// A no-op when no reconciler is wired (discovery-only tests).
    async fn reconcile(&self, key: &str, reading: &EgressReading) {
        let Some(reconciler) = self.reconciler.clone() else {
            return;
        };
        // Clone the small owned inputs into the blocking task; the
        // reconciler is `Arc<dyn …>` so it crosses the hop cheaply.
        let key = key.to_owned();
        let reading = reading.clone();
        let n = tokio::task::spawn_blocking(move || reconciler.reconcile(&key, &reading))
            .await
            .unwrap_or(0);
        if n > 0 {
            debug!(records = n, "ddns: reconciled records via DnsWriter");
        }
    }
}

/// DDNS-EGRESS-2 — the production [`DnsReconciler`]: on a changed source,
/// load the persisted `[ddns]` config, find every record bound to that
/// source, and drive the DigitalOcean [`DnsWriter`] (create/update on a
/// present IP, `on_down` policy when down). Holds the workgroup root (to
/// load the config + resolve the encrypted token) + this node's id/host
/// (for record templating + the `ddns/auth` alert).
pub struct DoDnsReconciler {
    workgroup_root: PathBuf,
    host: String,
}

impl DoDnsReconciler {
    /// Build rooted at the shared workgroup (config + secret home) for
    /// `host` (this node's id, used for templating + the alert host).
    #[must_use]
    pub fn new(workgroup_root: PathBuf, host: impl Into<String>) -> Self {
        Self {
            workgroup_root,
            host: host.into(),
        }
    }

    /// Build the DigitalOcean writer from `cfg` with the production seams:
    /// `curl` transport (token off argv), the age-sealed token resolved
    /// from `cfg.token_ref`, and the file alert sink. Split out so the
    /// per-record loop (`reconcile`) is the same whether the writer is
    /// real or (in a test of [`DoDnsReconciler`]) constructed differently.
    fn build_writer(
        &self,
        cfg: &mackes_mesh_types::ddns::DdnsConfig,
    ) -> impl crate::workers::ddns_writer::DnsWriter {
        use crate::workers::ddns_writer::{
            CurlExec, DigitalOceanWriter, FileAlertSink, SealedTokenSource,
        };
        let tokens = SealedTokenSource::new(&self.workgroup_root, &self.host, &cfg.token_ref);
        DigitalOceanWriter::new(
            &cfg.zone,
            self.host.clone(),
            CurlExec::new(),
            tokens,
            FileAlertSink::new(),
        )
    }
}

/// Derive the `{provider}` templating value for a record `source` key:
/// `tunnel:mullvad-1` → `mullvad-1`; `wan` → `wan`. Pure. The `{node}`
/// value is the host; `{n}` is fixed at 1 (multi-instance indexing is a
/// later refinement — single instance per (node, source) for now).
#[must_use]
pub(crate) fn provider_for_source(source: &str) -> &str {
    source.strip_prefix("tunnel:").unwrap_or(source)
}

impl DnsReconciler for DoDnsReconciler {
    fn reconcile(&self, key: &str, reading: &EgressReading) -> usize {
        let cfg = mackes_mesh_types::ddns::load(&self.workgroup_root);
        if !cfg.enabled {
            debug!("ddns: writer disabled in config; skipping reconcile");
            return 0;
        }
        // Only the DigitalOcean adapter exists in v1.
        if cfg.provider != "digitalocean" {
            warn!(provider = %cfg.provider, "ddns: unknown DnsWriter provider; skipping");
            return 0;
        }
        let matching: Vec<_> = cfg.record.iter().filter(|r| r.source == key).collect();
        if matching.is_empty() {
            return 0;
        }
        let writer = self.build_writer(&cfg);
        let provider = provider_for_source(key);
        let mut done = 0usize;
        for rec in matching {
            let fqdn = rec.fqdn(&self.host, provider, 1, &cfg.zone);
            // Reconcile each present family; for a down source the policy
            // is family-agnostic (remove/sentinel/keep handled inside).
            let ips: Vec<Option<&str>> = if reading.is_empty() {
                vec![None]
            } else {
                let mut v = Vec::new();
                if let Some(ip) = reading.v4.as_deref() {
                    v.push(Some(ip));
                }
                if let Some(ip) = reading.v6.as_deref() {
                    v.push(Some(ip));
                }
                v
            };
            for ip in ips {
                if let Err(e) = crate::workers::ddns_writer::reconcile_record(
                    &writer,
                    &fqdn,
                    ip,
                    cfg.ttl,
                    rec.on_down,
                    "", // sentinel address — operator-config seam (unset → keep)
                ) {
                    // One record's failure (incl. an auth alert already
                    // raised inside the writer) must not wedge the others.
                    warn!(record = %rec.name, fqdn = %fqdn, error = %e, "ddns: record reconcile failed");
                } else {
                    done += 1;
                }
            }
        }
        done
    }
}

/// Fire-and-forget Bus publish of an egress-IP change via `mde-bus`. No-op
/// when `mde-bus` isn't on PATH (mirrors the meshfs/compute publishers).
fn publish_change(host: &str, kind: &EgressKind, reading: &EgressReading, first_seen: bool) {
    let topic = egress_topic(host);
    let body = change_payload(host, kind, reading, first_seen);
    let mut cmd = Command::new("mde-bus");
    cmd.args(["publish", &topic, "--body-flag", &body]);
    crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
}

#[async_trait::async_trait]
impl Worker for DdnsWorker {
    fn name(&self) -> &'static str {
        "ddns"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        info!(
            host = %self.host,
            tick_secs = self.tick.as_secs(),
            sources = self.sources.len(),
            state = %self.state_path.display(),
            "ddns: started (WAN egress discovery; VPN-tunnel source deferred to DDNS-EGRESS-3)",
        );
        let mut interval = tokio::time::interval(self.tick);
        loop {
            tokio::select! {
                _ = shutdown.wait() => {
                    info!("ddns: shutdown requested; exiting");
                    return Ok(());
                }
                _ = interval.tick() => {
                    let _ = self.tick_once().await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn reading(v4: Option<&str>, v6: Option<&str>) -> EgressReading {
        EgressReading {
            v4: v4.map(str::to_owned),
            v6: v6.map(str::to_owned),
        }
    }

    fn state_with(key: &str, r: EgressReading) -> EgressState {
        let mut last = BTreeMap::new();
        last.insert(key.to_owned(), r);
        EgressState { last }
    }

    // ── topic + key ────────────────────────────────────────────────

    #[test]
    fn egress_topic_is_per_host() {
        assert_eq!(egress_topic("eagle"), "event/egress-ip/eagle");
    }

    #[test]
    fn egress_kind_key_shapes_match_design_config() {
        assert_eq!(EgressKind::Wan.key(), "wan");
        assert_eq!(
            EgressKind::Tunnel {
                id: "mullvad-1".into()
            }
            .key(),
            "tunnel:mullvad-1"
        );
    }

    // ── IP parse ───────────────────────────────────────────────────

    #[test]
    fn parse_ip_accepts_v4_and_v6_and_trims() {
        assert_eq!(
            parse_ip("  203.0.113.7 ").unwrap().to_string(),
            "203.0.113.7"
        );
        assert_eq!(parse_ip("2001:db8::1").unwrap().to_string(), "2001:db8::1");
    }

    #[test]
    fn parse_ip_rejects_garbage() {
        assert!(parse_ip("not-an-ip").is_none());
        assert!(parse_ip("").is_none());
        assert!(parse_ip("203.0.113.7/24").is_none());
    }

    // ── local routing-table lookup ─────────────────────────────────

    #[test]
    fn local_egress_addr_resolves_a_v4_source_address() {
        // The route lookup binds *some* local v4 source address on any
        // host with an IPv4 stack (loopback at minimum). No packet is
        // sent, so this works offline / in a sandbox.
        let ip = local_egress_addr(ROUTE_PROBE_V4);
        assert!(ip.is_some(), "a v4 host must resolve a source address");
        assert!(ip.unwrap().is_ipv4());
    }

    #[test]
    fn local_egress_addr_rejects_a_malformed_target() {
        assert!(local_egress_addr("not-a-socket-addr").is_none());
    }

    #[test]
    fn local_egress_reading_has_at_least_one_family() {
        // Sandbox CI always has at least an IPv4 loopback route.
        let r = local_egress_reading();
        assert!(
            !r.is_empty(),
            "the host must resolve at least one egress family"
        );
    }

    // ── change detection ───────────────────────────────────────────

    #[test]
    fn first_seen_when_no_prior_state_and_a_value() {
        let st = EgressState::default();
        let fresh = reading(Some("203.0.113.7"), None);
        assert_eq!(detect_change(&st, "wan", &fresh), ChangeOutcome::FirstSeen);
        assert!(detect_change(&st, "wan", &fresh).should_publish());
    }

    #[test]
    fn first_tick_offline_does_not_publish() {
        let st = EgressState::default();
        let fresh = reading(None, None);
        assert_eq!(
            detect_change(&st, "wan", &fresh),
            ChangeOutcome::WentOffline
        );
        assert!(!detect_change(&st, "wan", &fresh).should_publish());
    }

    #[test]
    fn changed_when_address_differs() {
        let st = state_with("wan", reading(Some("203.0.113.7"), None));
        let fresh = reading(Some("198.51.100.9"), None);
        assert_eq!(detect_change(&st, "wan", &fresh), ChangeOutcome::Changed);
        assert!(detect_change(&st, "wan", &fresh).should_publish());
    }

    #[test]
    fn changed_when_a_v6_appears_alongside_v4() {
        let st = state_with("wan", reading(Some("203.0.113.7"), None));
        let fresh = reading(Some("203.0.113.7"), Some("2001:db8::1"));
        assert_eq!(detect_change(&st, "wan", &fresh), ChangeOutcome::Changed);
    }

    #[test]
    fn unchanged_when_identical() {
        let st = state_with("wan", reading(Some("203.0.113.7"), Some("2001:db8::1")));
        let fresh = reading(Some("203.0.113.7"), Some("2001:db8::1"));
        assert_eq!(detect_change(&st, "wan", &fresh), ChangeOutcome::Unchanged);
        assert!(!detect_change(&st, "wan", &fresh).should_publish());
    }

    #[test]
    fn went_offline_retains_a_prior_value_without_churn() {
        let st = state_with("wan", reading(Some("203.0.113.7"), None));
        let fresh = reading(None, None);
        assert_eq!(
            detect_change(&st, "wan", &fresh),
            ChangeOutcome::WentOffline
        );
        assert!(
            !detect_change(&st, "wan", &fresh).should_publish(),
            "an outage must not churn the record to a sentinel"
        );
    }

    // ── persisted state round-trip ─────────────────────────────────

    #[test]
    fn state_json_round_trips() {
        let st = state_with("wan", reading(Some("203.0.113.7"), Some("2001:db8::1")));
        let json = st.to_json().unwrap();
        assert_eq!(EgressState::from_json(&json), st);
    }

    #[test]
    fn state_from_corrupt_json_is_empty_not_a_panic() {
        assert_eq!(EgressState::from_json("{ not json"), EgressState::default());
        assert_eq!(EgressState::from_json(""), EgressState::default());
    }

    #[test]
    fn state_store_then_load_round_trips_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("egress-ip.json");
        let st = state_with("wan", reading(Some("203.0.113.7"), None));
        st.store(&path).unwrap();
        assert!(path.exists(), "store creates parent dirs");
        assert_eq!(EgressState::load(&path), st);
        // No temp sibling left behind.
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn load_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.json");
        assert_eq!(EgressState::load(&path), EgressState::default());
    }

    // ── change payload ─────────────────────────────────────────────

    #[test]
    fn change_payload_carries_what_the_writer_needs() {
        let body = change_payload(
            "eagle",
            &EgressKind::Wan,
            &reading(Some("203.0.113.7"), None),
            true,
        );
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["host"], "eagle");
        assert_eq!(v["source"], "wan");
        assert_eq!(v["reading"]["v4"], "203.0.113.7");
        assert_eq!(v["first_seen"], true);
    }

    #[test]
    fn change_payload_tunnel_source_key_is_forward_compatible() {
        // The DDNS-EGRESS-3 tunnel source will reuse this exact path.
        let body = change_payload(
            "eagle",
            &EgressKind::Tunnel {
                id: "mullvad-1".into(),
            },
            &reading(Some("185.65.x.x".replace('x', "1").as_str()), None),
            false,
        );
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["source"], "tunnel:mullvad-1");
        assert_eq!(v["first_seen"], false);
    }

    // ── worker wiring (with a fake source) ─────────────────────────

    struct FakeSource {
        kind: EgressKind,
        reading: std::sync::Mutex<EgressReading>,
    }

    #[async_trait::async_trait]
    impl EgressIpSource for FakeSource {
        fn kind(&self) -> EgressKind {
            self.kind.clone()
        }
        async fn current(&self) -> anyhow::Result<EgressReading> {
            Ok(self.reading.lock().unwrap().clone())
        }
    }

    #[test]
    fn worker_name_is_stable() {
        let w = DdnsWorker::new("eagle");
        assert_eq!(w.name(), "ddns");
    }

    #[test]
    fn worker_always_ships_the_wan_source_first() {
        // DDNS-EGRESS-3: WAN is always source[0]; the per-tunnel sources (one
        // per VPN-GW-published tunnel) append after it. On a box with no VPN-GW
        // health published, that's WAN-only — but the test only pins the
        // invariant that survives a build host that DOES have a health file:
        // the WAN source is present + first.
        let w = DdnsWorker::new("eagle");
        assert!(!w.sources.is_empty());
        assert_eq!(w.sources[0].kind(), EgressKind::Wan);
    }

    // ── DDNS-EGRESS-3 — TunnelEgressSource (verified exit IP per tunnel) ──────

    fn health_with(
        tunnel: &str,
        exit_ip: Option<&str>,
        is_provider: bool,
        dns_leak: bool,
    ) -> crate::workers::vpn_gateway::HealthState {
        let mut st = crate::workers::vpn_gateway::HealthState::default();
        st.tunnel.insert(
            tunnel.to_owned(),
            mackes_mesh_types::vpn::TunnelHealth::from_probes(
                tunnel,
                exit_ip.is_some(),
                exit_ip.map(str::to_owned),
                is_provider,
                dns_leak,
            ),
        );
        st
    }

    #[test]
    fn tunnel_source_kind_is_the_tunnel_key() {
        let dir = tempfile::tempdir().unwrap();
        let src = TunnelEgressSource::new("mullvad-1", dir.path());
        assert_eq!(
            src.kind(),
            EgressKind::Tunnel {
                id: "mullvad-1".into()
            }
        );
        assert_eq!(src.kind().key(), "tunnel:mullvad-1");
    }

    #[test]
    fn tunnel_source_yields_the_verified_exit_ip_when_healthy() {
        let dir = tempfile::tempdir().unwrap();
        let src = TunnelEgressSource::new("mullvad-1", dir.path());
        // Healthy: live + provider exit IP + no DNS leak → verdict Healthy.
        let st = health_with("mullvad-1", Some("185.65.1.1"), true, false);
        assert_eq!(src.reading_from(&st), reading(Some("185.65.1.1"), None));
        // An IPv6 exit IP routes into the v6 family.
        let st6 = health_with("mullvad-1", Some("2001:db8::9"), true, false);
        assert_eq!(src.reading_from(&st6), reading(None, Some("2001:db8::9")));
    }

    #[test]
    fn tunnel_source_is_empty_when_leaking_or_down_or_absent() {
        let dir = tempfile::tempdir().unwrap();
        let src = TunnelEgressSource::new("mullvad-1", dir.path());
        // Leaking (exit IP == WAN → not provider) → no publishable IP.
        let leaking = health_with("mullvad-1", Some("9.9.9.9"), false, false);
        assert!(
            src.reading_from(&leaking).is_empty(),
            "leaking → no publish"
        );
        // DNS leak even with a provider exit IP → Leaking → no publish.
        let dns_leak = health_with("mullvad-1", Some("185.65.1.1"), true, true);
        assert!(
            src.reading_from(&dns_leak).is_empty(),
            "dns-leak → no publish"
        );
        // Down (no exit IP / not live) → no publish.
        let down = health_with("mullvad-1", None, false, false);
        assert!(src.reading_from(&down).is_empty(), "down → no publish");
        // A different/absent tunnel → empty.
        let other = health_with("proton-2", Some("185.65.1.1"), true, false);
        assert!(src.reading_from(&other).is_empty(), "absent tunnel → empty");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn tunnel_source_reads_the_health_fixture_from_disk() {
        // The full path: a tunnel-health.json fixture → TunnelEgressSource
        // yields the IP through the async `current()` (the worker's call path).
        let dir = tempfile::tempdir().unwrap();
        let health_path = crate::workers::vpn_gateway::default_health_path(dir.path());
        health_with("mullvad-1", Some("185.65.1.1"), true, false)
            .store(&health_path)
            .unwrap();
        let src = TunnelEgressSource::new("mullvad-1", dir.path());
        assert_eq!(
            src.current().await.unwrap(),
            reading(Some("185.65.1.1"), None)
        );
    }

    #[test]
    fn tunnel_sources_builds_one_per_published_tunnel() {
        let dir = tempfile::tempdir().unwrap();
        // No health file → no tunnel sources (graceful, additive).
        assert!(tunnel_sources(dir.path()).is_empty());
        // Publish health for two tunnels → one source each, keyed correctly.
        let mut st = health_with("mullvad-1", Some("185.65.1.1"), true, false);
        st.tunnel.insert(
            "proton-2".into(),
            mackes_mesh_types::vpn::TunnelHealth::from_probes(
                "proton-2",
                true,
                Some("146.70.1.1".into()),
                true,
                false,
            ),
        );
        st.store(&crate::workers::vpn_gateway::default_health_path(
            dir.path(),
        ))
        .unwrap();
        let srcs = tunnel_sources(dir.path());
        assert_eq!(srcs.len(), 2);
        // BTreeMap order: mullvad-1 then proton-2.
        assert_eq!(srcs[0].kind().key(), "tunnel:mullvad-1");
        assert_eq!(srcs[1].kind().key(), "tunnel:proton-2");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn tunnel_exit_ip_flows_through_the_same_change_detect_publish_path() {
        // End-to-end through the worker: a tunnel's verified exit IP is detected
        // as a change + handed to the reconciler under the tunnel:<id> key —
        // the SAME path the WAN IP uses (DDNS-EGRESS-3 acceptance).
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("egress-ip.json");
        let spy = std::sync::Arc::new(SpyReconciler::default());
        let src = TunnelEgressSource::new("mullvad-1", dir.path())
            .with_health_path(dir.path().join("health.json"));
        health_with("mullvad-1", Some("185.65.1.1"), true, false)
            .store(&dir.path().join("health.json"))
            .unwrap();
        let w = DdnsWorker::new("eagle")
            .with_reconciler(spy.clone())
            .with_state_path(state_path)
            .with_sources(vec![Box::new(src)]);
        assert_eq!(w.tick_once().await, 1, "first-seen tunnel exit IP changes");
        let seen = spy.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, "tunnel:mullvad-1");
        assert_eq!(seen[0].1, reading(Some("185.65.1.1"), None));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn tick_first_seen_then_unchanged_then_changed_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("egress-ip.json");
        let fake = std::sync::Arc::new(FakeSource {
            kind: EgressKind::Wan,
            reading: std::sync::Mutex::new(reading(Some("203.0.113.7"), None)),
        });
        // Reconstruct the worker with the same fake each time so it reads
        // the persisted state from disk between ticks (the real flow).
        let mk = || {
            let f = std::sync::Arc::clone(&fake);
            // A thin forwarding source that shares the Arc'd fake.
            struct Fwd(std::sync::Arc<FakeSource>);
            #[async_trait::async_trait]
            impl EgressIpSource for Fwd {
                fn kind(&self) -> EgressKind {
                    self.0.kind()
                }
                async fn current(&self) -> anyhow::Result<EgressReading> {
                    self.0.current().await
                }
            }
            DdnsWorker::new("eagle")
                .without_reconciler() // discovery-only assertions here
                .with_state_path(path.clone())
                .with_sources(vec![Box::new(Fwd(f))])
        };

        // First tick: first-seen → 1 change, persisted.
        assert_eq!(mk().tick_once().await, 1);
        let st = EgressState::load(&path);
        assert_eq!(st.last["wan"], reading(Some("203.0.113.7"), None));

        // Second tick, same reading: unchanged → 0 changes.
        assert_eq!(mk().tick_once().await, 0);

        // Address changes → 1 change, new value persisted.
        *fake.reading.lock().unwrap() = reading(Some("198.51.100.9"), None);
        assert_eq!(mk().tick_once().await, 1);
        assert_eq!(
            EgressState::load(&path).last["wan"],
            reading(Some("198.51.100.9"), None)
        );

        // Goes offline → 0 changes, last value retained (no churn).
        *fake.reading.lock().unwrap() = reading(None, None);
        assert_eq!(mk().tick_once().await, 0);
        assert_eq!(
            EgressState::load(&path).last["wan"],
            reading(Some("198.51.100.9"), None),
            "an outage must not erase the last-seen WAN IP"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn tick_change_survives_a_restart_via_persisted_state() {
        // Simulate: IP X seen + persisted, daemon "restarts", IP is now Y
        // → the next tick (fresh worker, same state file) detects Changed.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("egress-ip.json");
        state_with("wan", reading(Some("203.0.113.7"), None))
            .store(&path)
            .unwrap();
        let fake = FakeSource {
            kind: EgressKind::Wan,
            reading: std::sync::Mutex::new(reading(Some("198.51.100.9"), None)),
        };
        let w = DdnsWorker::new("eagle")
            .without_reconciler()
            .with_state_path(path.clone())
            .with_sources(vec![Box::new(fake)]);
        assert_eq!(
            w.tick_once().await,
            1,
            "a change while down is caught on the next boot"
        );
    }

    // ── DDNS-EGRESS-2 wiring: the worker actually drives the writer ──

    /// A capturing reconciler that records every (key, reading) the
    /// worker hands it — proving the writer is runtime-reachable (§7),
    /// not a dangling trait. Returns a fixed count so `reconcile`'s log
    /// branch is exercised.
    #[derive(Default)]
    struct SpyReconciler {
        seen: std::sync::Mutex<Vec<(String, EgressReading)>>,
    }
    impl DnsReconciler for SpyReconciler {
        fn reconcile(&self, key: &str, reading: &EgressReading) -> usize {
            self.seen
                .lock()
                .unwrap()
                .push((key.to_owned(), reading.clone()));
            1
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn tick_drives_the_reconciler_on_change_and_on_down() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("egress-ip.json");
        let spy = std::sync::Arc::new(SpyReconciler::default());
        let cur = std::sync::Arc::new(std::sync::Mutex::new(reading(Some("203.0.113.7"), None)));

        let mk = || {
            let c = std::sync::Arc::clone(&cur);
            struct Fwd(std::sync::Arc<std::sync::Mutex<EgressReading>>);
            #[async_trait::async_trait]
            impl EgressIpSource for Fwd {
                fn kind(&self) -> EgressKind {
                    EgressKind::Wan
                }
                async fn current(&self) -> anyhow::Result<EgressReading> {
                    Ok(self.0.lock().unwrap().clone())
                }
            }
            DdnsWorker::new("eagle")
                .with_reconciler(spy.clone())
                .with_state_path(path.clone())
                .with_sources(vec![Box::new(Fwd(c))])
        };

        // First-seen → the worker drives the reconciler with the WAN key.
        mk().tick_once().await;
        // Address change → drives it again with the new reading.
        *cur.lock().unwrap() = reading(Some("198.51.100.9"), None);
        mk().tick_once().await;
        // Goes offline → drives the reconciler with an empty reading so
        // the `on_down` policy fires (NOT a silent skip).
        *cur.lock().unwrap() = reading(None, None);
        mk().tick_once().await;

        let seen = spy.seen.lock().unwrap();
        assert_eq!(seen.len(), 3, "first-seen + change + down all reconcile");
        assert_eq!(seen[0].0, "wan");
        assert_eq!(seen[0].1, reading(Some("203.0.113.7"), None));
        assert_eq!(seen[1].1, reading(Some("198.51.100.9"), None));
        assert!(
            seen[2].1.is_empty(),
            "the down reading reaches on_down handling"
        );
    }

    #[test]
    fn provider_for_source_strips_tunnel_prefix() {
        assert_eq!(provider_for_source("tunnel:mullvad-1"), "mullvad-1");
        assert_eq!(provider_for_source("wan"), "wan");
    }
}
