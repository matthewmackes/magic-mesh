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
}

impl DdnsWorker {
    /// Construct the WAN-only worker for `host`, persisting to
    /// [`DEFAULT_STATE_PATH`] at the [`DEFAULT_TICK`] cadence. The VPN
    /// source (DDNS-EGRESS-3) appends to `sources` when it lands.
    #[must_use]
    pub fn new(host: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            state_path: PathBuf::from(DEFAULT_STATE_PATH),
            tick: DEFAULT_TICK,
            sources: vec![Box::new(WanEgressSource::new())],
        }
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
                    state.last.insert(key, reading);
                    changed += 1;
                }
                ChangeOutcome::WentOffline => {
                    debug!(source = %key, "ddns: source offline; retaining last-seen IP");
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
    fn worker_ships_wan_source_only_vpn_deferred() {
        let w = DdnsWorker::new("eagle");
        assert_eq!(w.sources.len(), 1, "WAN only; VPN source is DDNS-EGRESS-3");
        assert_eq!(w.sources[0].kind(), EgressKind::Wan);
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
            .with_state_path(path.clone())
            .with_sources(vec![Box::new(fake)]);
        assert_eq!(
            w.tick_once().await,
            1,
            "a change while down is caught on the next boot"
        );
    }
}
