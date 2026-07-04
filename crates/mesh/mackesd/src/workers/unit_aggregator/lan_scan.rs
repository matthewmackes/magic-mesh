//! EXPLORER-2 — the active LAN scan submodule: the real `LanHost` producer that
//! fills EXPLORER-1's [`super::sources::LanScanSource`] seam (design #3/#24).
//!
//! An **active nmap-style** discovery over the local subnet, running ONLY while
//! the surface-gated `scan_active` flag is set (EXPLORER-1 wires it; the shell
//! flips it while Discovery is visible, lock #24). The pipeline unions three
//! discovery signals over the local /24(s):
//!
//! 1. **mDNS / DNS-SD listen** — advertised services + names (reusing the tested
//!    [`crate::surrounding_hosts::collect_mdns`] avahi parse).
//! 2. **ARP / neighbour-table read** — silent hosts + the stable MAC key
//!    (reusing [`crate::surrounding_hosts::arp_neigh_map`]).
//! 3. **A bounded ping-sweep + light TCP port fingerprint** — the /24 is
//!    ping-swept (`ping -c1 -W1`, fanned out under a thread bound), and each live
//!    candidate is fingerprinted against [`FINGERPRINT_PORTS`]
//!    (22/80/443/3389/5900/5930/5985 → SSH/HTTP/HTTPS/RDP/VNC/Spice/WinRM) via a
//!    short bounded `TcpStream::connect_timeout`. The open-service set feeds the
//!    E5 fingerprint + a coarse type guess.
//!
//! ## Bounding (never hang a tick, design risk note)
//! The whole scan runs on a **detached background thread** ([`LanScan`]): a fold
//! tick calls [`LanScan::scan`], which returns the warm cache *instantly* and
//! kicks a refresh only when none is already in flight. Inside the refresh, the
//! ping-sweep + fingerprint fan out across at most [`MAX_SCAN_THREADS`] scoped
//! threads with per-host timeouts, so a full /24 completes in a few seconds and
//! the async runtime is never blocked. When `scan_active` is false the scan does
//! **no work** and simply serves the warm cache (lock #24).
//!
//! ## Honesty (§7)
//! Every impure probe lives behind the injectable [`ScanEnv`] seam so unit tests
//! drive the whole pipeline with canned command output + a synthetic subnet and
//! **no live network** (mirroring EXPLORER-1's testkit). Unprobed fields stay
//! explicit `None`/empty — a live host with no open port is emitted with an empty
//! service list, never a fabricated service. Self is excluded from its own scan;
//! a LAN host that is also a mesh peer is left for EXPLORER-7's MAC-based dedup
//! (the scan seam has no peer set — see the module note below).
//!
//! ## Dedup vs. mesh peers
//! The [`super::sources::LanScanSource`] contract only passes `scan_active`, not
//! the peer set, so this producer cannot dedup a LAN host against a mesh peer by
//! address here. It excludes *self* (its own interface addresses) and confines
//! discovery to the physical LAN /24(s) — the Nebula overlay iface is skipped, so
//! overlay peer IPs are never swept. A peer that is *also* on the physical LAN
//! surfaces once as a `LanHost` (LAN IP) and once as a `Peer` (overlay IP);
//! collapsing those is EXPLORER-7's edge/dedup job (keyed on MAC / one-ARP-hop).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;

use crate::probe_nmap::DEFAULT_EXCLUDE_IFACE_PREFIXES;
use crate::surrounding_hosts::{
    arp_neigh_map, classify, collect_mdns, reverse_dns, HostSignals, HostType, MdnsService,
};

use super::sources::{LanHostRecord, LanScanSource};

/// The port → service-label fingerprint set (design #3).
///
/// The common remote / management ports, each mapped to the service it implies.
/// Quasar cares about the remote-desktop trio (RDP/VNC/Spice) because a LAN
/// desktop is a broker target — the type guess + the E5 openable-action seam ride
/// these labels.
pub const FINGERPRINT_PORTS: &[(u16, &str)] = &[
    (22, "ssh"),
    (80, "http"),
    (443, "https"),
    (3389, "rdp"),
    (5900, "vnc"),
    (5930, "spice"),
    (5985, "winrm"),
];

/// Per-port TCP connect budget for the light fingerprint — short so a candidate
/// sweep stays quick; [`MAX_SCAN_THREADS`] bounds the concurrency on top.
const PORT_CONNECT_TIMEOUT: Duration = Duration::from_millis(400);

/// Concurrency ceiling for the ping-sweep + fingerprint fan-out (dependency-free
/// scoped threads — see [`parallel_map`]).
const MAX_SCAN_THREADS: usize = 64;

/// `ping -W` per-host reply budget (seconds). A silent host costs ~1s; the sweep
/// runs those concurrently so the whole /24 stays within a few seconds.
const PING_WAIT_SECS: &str = "1";

/// The service label for a fingerprint `port`, or `None` when it isn't one of
/// [`FINGERPRINT_PORTS`].
#[must_use]
pub fn label_for_port(port: u16) -> Option<&'static str> {
    FINGERPRINT_PORTS
        .iter()
        .find(|(p, _)| *p == port)
        .map(|(_, l)| *l)
}

/// The impure network surface the LAN scan reads — one method per probe.
///
/// Injectable so a unit test can drive the whole pipeline with canned command
/// output + a synthetic subnet and NO live network (§7). [`LiveScanEnv`] is the
/// production implementation (bounded shell-outs + TCP connects); the tests
/// inject an in-memory fake.
pub trait ScanEnv: Send + Sync {
    /// This node's own physical-LAN IPv4 address(es) — the /24(s) to sweep and
    /// the self-addresses to exclude (so we never list ourselves as a LAN host).
    fn local_ipv4s(&self) -> Vec<Ipv4Addr>;
    /// The ARP / neighbour table as an `ip → mac` map (silent-host discovery +
    /// the stable MAC key).
    fn arp_table(&self) -> HashMap<String, String>;
    /// Advertised mDNS / DNS-SD service records (existence + names + service
    /// hints for the type guess).
    fn mdns(&self) -> Vec<MdnsService>;
    /// Whether `ip` answers an ICMP echo within the bounded budget.
    fn ping(&self, ip: Ipv4Addr) -> bool;
    /// Which [`FINGERPRINT_PORTS`] accept a bounded TCP connection on `ip`.
    fn open_ports(&self, ip: Ipv4Addr) -> Vec<u16>;
    /// Reverse-DNS name for `ip`, when the resolver answers.
    fn rdns(&self, ip: Ipv4Addr) -> Option<String>;
}

/// Production [`ScanEnv`]: bounded `ip`/`ping` shell-outs + `TcpStream` connects.
///
/// Reuses the tested pure parsers in [`crate::surrounding_hosts`] /
/// [`crate::probe_nmap`] for ARP / mDNS / interface decoding.
pub struct LiveScanEnv;

impl ScanEnv for LiveScanEnv {
    fn local_ipv4s(&self) -> Vec<Ipv4Addr> {
        match Command::new("ip").args(["-j", "addr"]).output() {
            Ok(o) if o.status.success() => {
                local_ipv4s_from_ip_json(&String::from_utf8_lossy(&o.stdout))
            }
            _ => Vec::new(),
        }
    }

    fn arp_table(&self) -> HashMap<String, String> {
        // Reuse the tested `ip neigh` reader + parser (surrounding_hosts).
        arp_neigh_map()
    }

    fn mdns(&self) -> Vec<MdnsService> {
        // Reuse the tested `avahi-browse -aprt` reader + parser.
        collect_mdns("avahi-browse")
    }

    fn ping(&self, ip: Ipv4Addr) -> bool {
        // Bounded-proc path: `.output()` waits on a child that self-terminates
        // via ping's own `-W` reply timeout (same idiom as the surrounding_hosts
        // `curl --max-time` collectors). `-n` keeps it numeric (no rDNS stall).
        Command::new("ping")
            .args(["-c", "1", "-W", PING_WAIT_SECS, "-n", &ip.to_string()])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn open_ports(&self, ip: Ipv4Addr) -> Vec<u16> {
        FINGERPRINT_PORTS
            .iter()
            .filter(|(port, _)| {
                let addr = SocketAddr::new(IpAddr::V4(ip), *port);
                TcpStream::connect_timeout(&addr, PORT_CONNECT_TIMEOUT).is_ok()
            })
            .map(|(port, _)| *port)
            .collect()
    }

    fn rdns(&self, ip: Ipv4Addr) -> Option<String> {
        reverse_dns(&ip.to_string())
    }
}

/// Parse `ip -j addr` JSON into this node's physical-LAN IPv4 addresses.
///
/// Skips loopback / overlay / virtual interfaces (the same prefix set the probe
/// target resolver excludes). Tolerant: malformed JSON → empty (§7). Pure.
#[must_use]
pub fn local_ipv4s_from_ip_json(json: &str) -> Vec<Ipv4Addr> {
    #[derive(Deserialize)]
    struct Iface {
        ifname: String,
        #[serde(default)]
        addr_info: Vec<AddrInfo>,
    }
    #[derive(Deserialize)]
    struct AddrInfo {
        family: String,
        local: String,
    }
    let Ok(ifaces) = serde_json::from_str::<Vec<Iface>>(json) else {
        return Vec::new();
    };
    let mut out: Vec<Ipv4Addr> = Vec::new();
    for iface in ifaces {
        if DEFAULT_EXCLUDE_IFACE_PREFIXES
            .iter()
            .any(|p| iface.ifname.starts_with(p))
        {
            continue;
        }
        for a in iface.addr_info {
            if a.family != "inet" {
                continue;
            }
            if let Ok(ip) = a.local.parse::<Ipv4Addr>() {
                if !out.contains(&ip) {
                    out.push(ip);
                }
            }
        }
    }
    out
}

/// The `/24` network base (first three octets) an address sits in.
const fn slash24_base(ip: Ipv4Addr) -> [u8; 3] {
    let o = ip.octets();
    [o[0], o[1], o[2]]
}

/// Every host address (`.1`–`.254`) in the local `/24`(s), excluding self and the
/// network/broadcast edges — the bounded ping-sweep target list.
fn sweep_hosts(locals: &[Ipv4Addr]) -> Vec<Ipv4Addr> {
    let self_set: HashSet<Ipv4Addr> = locals.iter().copied().collect();
    let bases: BTreeSet<[u8; 3]> = locals.iter().map(|ip| slash24_base(*ip)).collect();
    let mut hosts = Vec::new();
    for b in bases {
        for h in 1..=254u8 {
            let ip = Ipv4Addr::new(b[0], b[1], b[2], h);
            if !self_set.contains(&ip) {
                hosts.push(ip);
            }
        }
    }
    hosts
}

/// A coarse device-type guess from the discovery signals (E5).
///
/// Reuses the platform surrounding-host [`classify`] cascade for the mDNS /
/// hostname / well-known-port signals it already knows, then falls back to the
/// remote-management ports that taxonomy's port map doesn't cover. Best-choice
/// (no design lock): RDP/VNC/Spice/WinRM ⇒ a desktop `computer` (a Quasar broker
/// target); SSH-only ⇒ a headless `server`; HTTP-only is too weak to type ⇒
/// honest unknown (§7).
#[must_use]
pub fn guess_type(
    labels: &[String],
    open_ports: &[u16],
    mdns_services: &[String],
    hostname: &str,
) -> Option<String> {
    let sig = HostSignals {
        mdns_services: mdns_services.to_vec(),
        open_ports: open_ports.to_vec(),
        hostname: hostname.to_string(),
        ..Default::default()
    };
    let classified = classify(&sig);
    if classified != HostType::Unknown {
        return Some(classified.wire_name().to_string());
    }
    if labels
        .iter()
        .any(|l| matches!(l.as_str(), "rdp" | "vnc" | "spice" | "winrm"))
    {
        return Some(HostType::Computer.wire_name().to_string());
    }
    if labels.iter().any(|l| l == "ssh") {
        return Some(HostType::Server.wire_name().to_string());
    }
    None
}

/// Fingerprint one live candidate into a [`LanHostRecord`]: the open-port service
/// labels, the MAC key (ARP) / rDNS+mDNS name, and the coarse type guess. Pure
/// over the injected `env` + the already-read `arp`/`mdns` tables.
fn probe_one(
    env: &dyn ScanEnv,
    ip: Ipv4Addr,
    arp: &HashMap<String, String>,
    mdns: &[MdnsService],
) -> LanHostRecord {
    let ip_s = ip.to_string();
    let open_ports = env.open_ports(ip);
    let services: Vec<String> = open_ports
        .iter()
        .filter_map(|p| label_for_port(*p))
        .map(str::to_string)
        .collect();

    let mac = arp.get(&ip_s).cloned();
    let mdns_services: Vec<String> = mdns
        .iter()
        .filter(|m| m.ip == ip_s)
        .map(|m| m.service_type.clone())
        .collect();
    let mdns_name = mdns
        .iter()
        .find(|m| m.ip == ip_s && !m.hostname.is_empty())
        .map(|m| m.hostname.clone());
    // rDNS first (system resolver), else the mDNS-advertised name, else the IP.
    let rdns = env.rdns(ip).or(mdns_name);
    let name = rdns.clone().unwrap_or_else(|| ip_s.clone());
    // The MAC is the stable key across IP changes / roaming; fall back to the IP.
    let key = mac.unwrap_or_else(|| ip_s.clone());
    let type_guess = guess_type(&services, &open_ports, &mdns_services, &name);

    LanHostRecord {
        key,
        name,
        address: Some(ip_s),
        services,
        open_ports,
        type_guess,
        rdns,
    }
}

/// Run one full scan pass over the injected environment.
///
/// Unions the ping-sweep + ARP + mDNS candidates on the local /24(s),
/// fingerprints each, and returns the discovered [`LanHostRecord`]s
/// (self-excluded, deterministically ordered). The pure heart of the scan — I/O
/// only through `env`, so tests drive it directly.
#[must_use]
pub fn build_records(env: &dyn ScanEnv) -> Vec<LanHostRecord> {
    let locals = env.local_ipv4s();
    if locals.is_empty() {
        // No physical-LAN interface resolved → nothing to sweep. Honest empty
        // (§7), never a fabricated host.
        return Vec::new();
    }
    let self_set: HashSet<Ipv4Addr> = locals.iter().copied().collect();
    let bases: HashSet<[u8; 3]> = locals.iter().map(|ip| slash24_base(*ip)).collect();

    // 1. Bounded ping-sweep of the local /24(s) for silent (non-advertising)
    //    hosts, fanned out under the thread bound — straight into the candidate
    //    set (no intermediate Vec).
    let hosts = sweep_hosts(&locals);
    let mut candidates: BTreeSet<Ipv4Addr> =
        parallel_map(&hosts, MAX_SCAN_THREADS, &|ip: &Ipv4Addr| {
            if env.ping(*ip) {
                Some(*ip)
            } else {
                None
            }
        })
        .into_iter()
        .flatten()
        .collect();

    // 2. Union the ARP/neighbour table + mDNS advertisers (in-subnet, non-self) —
    //    hosts we've talked to / that announce, even if they didn't answer ping.
    let arp = env.arp_table();
    let mdns = env.mdns();
    let in_scope = |ip: Ipv4Addr| bases.contains(&slash24_base(ip)) && !self_set.contains(&ip);
    for ip_s in arp.keys() {
        if let Ok(ip) = ip_s.parse::<Ipv4Addr>() {
            if in_scope(ip) {
                candidates.insert(ip);
            }
        }
    }
    for m in &mdns {
        if let Ok(ip) = m.ip.parse::<Ipv4Addr>() {
            if in_scope(ip) {
                candidates.insert(ip);
            }
        }
    }

    // 3. Light port fingerprint + name/type enrichment per live candidate.
    let candidates: Vec<Ipv4Addr> = candidates.into_iter().collect();
    let mut records = parallel_map(&candidates, MAX_SCAN_THREADS, &|ip: &Ipv4Addr| {
        probe_one(env, *ip, &arp, &mdns)
    });
    // Deterministic, display-friendly order (name then key). The fold re-sorts,
    // but a stable producer keeps publish-on-change quiet across ticks.
    records.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.key.cmp(&b.key)));
    records
}

/// Run `f` over `items` across at most `max_threads` scoped threads, preserving
/// input order. Dependency-free bounded concurrency (no rayon): the ping-sweep +
/// per-host fingerprint fan out here so a /24 finishes in a few seconds instead
/// of minutes, while never exceeding the thread bound.
fn parallel_map<T, R, F>(items: &[T], max_threads: usize, f: &F) -> Vec<R>
where
    T: Sync,
    R: Send,
    F: Fn(&T) -> R + Sync,
{
    let n = items.len();
    if n == 0 {
        return Vec::new();
    }
    let threads = max_threads.clamp(1, n);
    let chunk = n.div_ceil(threads);
    std::thread::scope(|scope| {
        let handles: Vec<_> = items
            .chunks(chunk)
            .map(|c| scope.spawn(move || c.iter().map(f).collect::<Vec<R>>()))
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("lan-scan worker thread panicked"))
            .collect()
    })
}

/// The EXPLORER-2 [`LanScanSource`]: a warm-cached, surface-gated active LAN scan.
///
/// [`Self::scan`] never blocks — it returns the warm cache and, when the surface
/// is active, kicks a detached background refresh (unless one is already running)
/// that swaps the cache when it finishes (design #24). A closed surface does no
/// work and simply serves the last result.
pub struct LanScan {
    env: Arc<dyn ScanEnv>,
    /// The last scan result — served instantly on (re)open, refreshed behind it.
    cache: Arc<Mutex<Vec<LanHostRecord>>>,
    /// Set while a background refresh is in flight (coalesces overlapping ticks).
    refreshing: Arc<AtomicBool>,
}

impl LanScan {
    /// Construct the production scan over [`LiveScanEnv`] with a cold cache. Cheap
    /// — no probing happens until the surface flips `scan_active` on.
    #[must_use]
    pub fn live() -> Self {
        Self::with_env(Arc::new(LiveScanEnv))
    }

    /// Construct over an injected [`ScanEnv`] (tests inject a fake).
    fn with_env(env: Arc<dyn ScanEnv>) -> Self {
        Self {
            env,
            cache: Arc::new(Mutex::new(Vec::new())),
            refreshing: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Kick a background refresh unless one is already in flight. Non-blocking:
    /// the scan runs on a detached thread and swaps the warm cache when done, so
    /// a fold tick never waits on the network (design #24 / the scan risk note).
    fn spawn_refresh(&self) {
        if self
            .refreshing
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return; // a refresh is already running — don't stack scans.
        }
        let env = Arc::clone(&self.env);
        let cache = Arc::clone(&self.cache);
        let flag = Arc::clone(&self.refreshing);
        std::thread::spawn(move || {
            let recs = build_records(&*env);
            if let Ok(mut c) = cache.lock() {
                *c = recs;
            }
            flag.store(false, Ordering::Release);
        });
    }

    /// Run one scan synchronously into the warm cache — the deterministic path the
    /// tests drive; production uses the detached [`Self::spawn_refresh`].
    #[cfg(test)]
    fn refresh_blocking(&self) {
        let recs = build_records(&*self.env);
        *self.cache.lock().unwrap() = recs;
    }
}

impl LanScanSource for LanScan {
    fn scan(&self, scan_active: bool) -> Vec<LanHostRecord> {
        if scan_active {
            // Surface visible → refresh in the background (lock #24). The warm
            // cache below is served instantly meanwhile.
            self.spawn_refresh();
        }
        // Closed surface: no work, just serve the warm cache (lock #24).
        self.cache.lock().map(|c| c.clone()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// An in-memory [`ScanEnv`] driving the pipeline with canned data + no live
    /// network (§7). Counts ping/port probes so a test can prove "no work while
    /// the surface is closed".
    #[derive(Default)]
    struct FakeScanEnv {
        locals: Vec<Ipv4Addr>,
        arp: HashMap<String, String>,
        mdns: Vec<MdnsService>,
        alive: HashSet<Ipv4Addr>,
        ports: HashMap<Ipv4Addr, Vec<u16>>,
        rdns: HashMap<Ipv4Addr, String>,
        ping_calls: AtomicUsize,
        port_calls: AtomicUsize,
    }

    impl ScanEnv for FakeScanEnv {
        fn local_ipv4s(&self) -> Vec<Ipv4Addr> {
            self.locals.clone()
        }
        fn arp_table(&self) -> HashMap<String, String> {
            self.arp.clone()
        }
        fn mdns(&self) -> Vec<MdnsService> {
            self.mdns.clone()
        }
        fn ping(&self, ip: Ipv4Addr) -> bool {
            self.ping_calls.fetch_add(1, Ordering::Relaxed);
            self.alive.contains(&ip)
        }
        fn open_ports(&self, ip: Ipv4Addr) -> Vec<u16> {
            self.port_calls.fetch_add(1, Ordering::Relaxed);
            self.ports.get(&ip).cloned().unwrap_or_default()
        }
        fn rdns(&self, ip: Ipv4Addr) -> Option<String> {
            self.rdns.get(&ip).cloned()
        }
    }

    fn ip(s: &str) -> Ipv4Addr {
        s.parse().unwrap()
    }

    fn find<'a>(recs: &'a [LanHostRecord], addr: &str) -> &'a LanHostRecord {
        recs.iter()
            .find(|r| r.address.as_deref() == Some(addr))
            .expect("record present for address")
    }

    #[test]
    fn ping_and_arp_yield_lan_records_excluding_self() {
        let mut env = FakeScanEnv {
            locals: vec![ip("192.168.1.10")],
            ..Default::default()
        };
        // A silent host known only from the ARP table (MAC key) …
        env.arp
            .insert("192.168.1.20".into(), "aa:bb:cc:dd:ee:01".into());
        // … and a ping-only responder (IP key).
        env.alive.insert(ip("192.168.1.30"));
        let recs = build_records(&env);
        assert_eq!(recs.len(), 2, "two hosts discovered");
        // Self (….10) is never listed as a LAN host.
        assert!(recs
            .iter()
            .all(|r| r.address.as_deref() != Some("192.168.1.10")));
        // ARP host → keyed on its MAC; ping-only host → keyed on its IP.
        assert_eq!(find(&recs, "192.168.1.20").key, "aa:bb:cc:dd:ee:01");
        assert_eq!(find(&recs, "192.168.1.30").key, "192.168.1.30");
        // Reachability is OnLan once the fold builds the unit; the record carries
        // the address + honest-empty service list for the un-fingerprinted host.
        assert!(find(&recs, "192.168.1.30").services.is_empty());
    }

    #[test]
    fn port_fingerprint_maps_service_labels_and_type_guess() {
        let mut env = FakeScanEnv {
            locals: vec![ip("192.168.1.10")],
            ..Default::default()
        };
        // A desktop broker target: RDP + VNC open.
        env.alive.insert(ip("192.168.1.40"));
        env.ports.insert(ip("192.168.1.40"), vec![3389, 5900]);
        // A headless server: SSH only.
        env.alive.insert(ip("192.168.1.41"));
        env.ports.insert(ip("192.168.1.41"), vec![22]);
        let recs = build_records(&env);

        let desktop = find(&recs, "192.168.1.40");
        assert_eq!(desktop.services, vec!["rdp".to_string(), "vnc".to_string()]);
        assert_eq!(desktop.open_ports, vec![3389, 5900]);
        assert_eq!(desktop.type_guess.as_deref(), Some("computer"));

        let server = find(&recs, "192.168.1.41");
        assert_eq!(server.services, vec!["ssh".to_string()]);
        assert_eq!(server.type_guess.as_deref(), Some("server"));
    }

    #[test]
    fn mdns_advertiser_named_and_out_of_subnet_arp_excluded() {
        let mut env = FakeScanEnv {
            locals: vec![ip("192.168.1.10")],
            ..Default::default()
        };
        // A printer that only announces mDNS (no ping / open fingerprint port).
        env.mdns.push(MdnsService {
            ip: "192.168.1.60".into(),
            hostname: "printer.local".into(),
            service_type: "_ipp._tcp".into(),
        });
        // An ARP entry on a DIFFERENT subnet (e.g. the Nebula overlay) must not
        // be listed — the scan is confined to the local /24.
        env.arp
            .insert("10.42.0.2".into(), "aa:bb:cc:dd:ee:99".into());
        let recs = build_records(&env);
        assert_eq!(recs.len(), 1, "only the in-subnet mDNS host");
        let printer = &recs[0];
        assert_eq!(printer.address.as_deref(), Some("192.168.1.60"));
        assert_eq!(printer.name, "printer.local");
        assert_eq!(printer.rdns.as_deref(), Some("printer.local"));
        assert_eq!(printer.type_guess.as_deref(), Some("printer"));
        assert!(recs
            .iter()
            .all(|r| r.address.as_deref() != Some("10.42.0.2")));
    }

    #[test]
    fn scan_gate_off_serves_cache_without_probing() {
        let env = Arc::new(FakeScanEnv {
            locals: vec![ip("192.168.1.10")],
            ..Default::default()
        });
        let scan = LanScan::with_env(env.clone());
        // Warm the cache as a prior active scan would have.
        let warm = vec![LanHostRecord {
            key: "cached".into(),
            name: "cached-host".into(),
            address: Some("192.168.1.99".into()),
            ..Default::default()
        }];
        *scan.cache.lock().unwrap() = warm.clone();
        // Flag off → the prior set is served AND nothing is probed (no work).
        let served = scan.scan(false);
        assert_eq!(served, warm, "closed surface serves the warm cache");
        assert_eq!(
            env.ping_calls.load(Ordering::Relaxed),
            0,
            "no probing while the surface is closed"
        );
        assert_eq!(env.port_calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn refresh_fills_the_cache_and_it_persists_while_closed() {
        let mut fake = FakeScanEnv {
            locals: vec![ip("192.168.1.10")],
            ..Default::default()
        };
        fake.alive.insert(ip("192.168.1.50"));
        fake.ports.insert(ip("192.168.1.50"), vec![22]);
        let env = Arc::new(fake);
        let scan = LanScan::with_env(env.clone());
        // A synchronous refresh (the deterministic analogue of an active tick)
        // populates the warm cache from the live probe.
        scan.refresh_blocking();
        let filled = scan.scan(false);
        assert_eq!(filled.len(), 1);
        assert_eq!(filled[0].address.as_deref(), Some("192.168.1.50"));
        let probed = env.ping_calls.load(Ordering::Relaxed);
        assert!(probed > 0, "the refresh probed the subnet");
        // Surface stays closed: the warm set is re-served with no further probing.
        let again = scan.scan(false);
        assert_eq!(again, filled, "warm cache re-served on a closed surface");
        assert_eq!(
            env.ping_calls.load(Ordering::Relaxed),
            probed,
            "no re-probe while closed"
        );
    }

    #[test]
    fn active_scan_spawns_a_refresh_that_fills_the_cache() {
        let mut fake = FakeScanEnv {
            locals: vec![ip("192.168.1.10")],
            ..Default::default()
        };
        fake.alive.insert(ip("192.168.1.70"));
        let env = Arc::new(fake);
        let scan = LanScan::with_env(env);
        // Cold cache served instantly (empty) while the background refresh runs.
        let immediate = scan.scan(true);
        assert!(
            immediate.is_empty(),
            "cold cache served instantly while the scan runs"
        );
        // The detached refresh fills the cache shortly (bounded poll — the fake
        // completes in milliseconds).
        let mut filled = Vec::new();
        for _ in 0..300 {
            filled = scan.scan(false); // serve cache; false ⇒ no re-spawn.
            if !filled.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(filled.len(), 1, "background refresh populated the cache");
        assert_eq!(filled[0].address.as_deref(), Some("192.168.1.70"));
    }

    #[test]
    fn malformed_ip_addr_json_is_tolerated() {
        assert!(local_ipv4s_from_ip_json("not json at all").is_empty());
        assert!(local_ipv4s_from_ip_json("").is_empty());
        // A faithful `ip -j addr` sample: lo + nebula excluded, the LAN iface kept.
        let json = r#"[
            {"ifname":"lo","addr_info":[{"family":"inet","local":"127.0.0.1","prefixlen":8}]},
            {"ifname":"nebula1","addr_info":[{"family":"inet","local":"10.42.0.9","prefixlen":16}]},
            {"ifname":"eth0","addr_info":[
                {"family":"inet","local":"192.168.1.10","prefixlen":24},
                {"family":"inet6","local":"fe80::1","prefixlen":64}
            ]}
        ]"#;
        assert_eq!(local_ipv4s_from_ip_json(json), vec![ip("192.168.1.10")]);
        // A no-LAN environment scans nothing rather than panicking (§7).
        let env = FakeScanEnv::default();
        assert!(build_records(&env).is_empty());
    }

    #[test]
    fn guess_type_prefers_classifier_then_falls_back_to_ports() {
        // mDNS printer service → the platform classifier's "printer".
        assert_eq!(
            guess_type(&[], &[], &["_ipp._tcp".to_string()], "").as_deref(),
            Some("printer")
        );
        // Spice-only (a VM console) → computer via the remote-desktop fallback.
        assert_eq!(
            guess_type(&["spice".to_string()], &[5930], &[], "").as_deref(),
            Some("computer")
        );
        // SSH-only → server.
        assert_eq!(
            guess_type(&["ssh".to_string()], &[22], &[], "").as_deref(),
            Some("server")
        );
        // HTTP-only is too weak → honest unknown.
        assert!(guess_type(&["http".to_string()], &[80], &[], "").is_none());
        assert!(guess_type(&[], &[], &[], "").is_none());
    }

    #[test]
    fn sweep_enumerates_the_slash24_excluding_self_and_edges() {
        let hosts = sweep_hosts(&[ip("192.168.1.10")]);
        assert_eq!(hosts.len(), 253, "1..=254 minus self");
        assert!(hosts.contains(&ip("192.168.1.1")));
        assert!(hosts.contains(&ip("192.168.1.254")));
        assert!(!hosts.contains(&ip("192.168.1.10")), "self excluded");
        assert!(
            !hosts.contains(&ip("192.168.1.0")),
            "network address skipped"
        );
        assert!(!hosts.contains(&ip("192.168.1.255")), "broadcast skipped");
    }

    #[test]
    fn fingerprint_ports_cover_the_remote_desktop_trio() {
        assert_eq!(label_for_port(22), Some("ssh"));
        assert_eq!(label_for_port(3389), Some("rdp"));
        assert_eq!(label_for_port(5900), Some("vnc"));
        assert_eq!(label_for_port(5930), Some("spice"));
        assert_eq!(label_for_port(5985), Some("winrm"));
        assert_eq!(label_for_port(65000), None);
    }
}
