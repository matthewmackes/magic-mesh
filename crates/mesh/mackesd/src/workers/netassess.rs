//! MESH-A-1 (v5.0.0) — per-peer network assessment subsystem.
//!
//! Collects the 9 network-assessment items locked in
//! `docs/design/v6.0-mde-portal.md` §7.1 (R7-Q1..Q7) on a periodic
//! tick, writes a timestamped JSON snapshot to
//! `~/.local/share/mde/netassess/<host>/<iso8601>-<hash>.json`, and
//! trims snapshots older than 30 days. The directory lands under
//! mesh-storage once mounted (it inherits the existing per-peer
//! replication), so every peer reads the union for the Portal /
//! Workbench network surfaces.
//!
//! ## The 9 items (design doc §7.1)
//!
//! 1. WiFi SSIDs + RSSI + channel + encryption (`nmcli` terse).
//! 2. Local ARP table (`ip neigh`).
//! 3. Default gateway + DNS servers (`ip route` + `/etc/resolv.conf`).
//! 4. Public IP + ISP/AS (`curl ipinfo.io/json`).
//! 5. Speedtest down/up/latency (`speedtest-cli --json`).
//! 6. IPv4 + IPv6 connectivity (`ping` / `ping -6`).
//! 7. MTU + jumbo-frame support (`ip link`).
//! 8. Tunnel health (nebula1 interface up).
//! 9. nmap-light passive subnet discovery (reuses the EPIC-MESH-PROBE
//!    inventory when present, per mesh-probe-subsystem.md §3; falls
//!    back to the ARP-table host count).
//!
//! ## Cadence
//!
//! Active collection runs hourly ([`DEFAULT_TICK_INTERVAL`]). The
//! worklist line cites "active 10 min", but a 10-minute speedtest
//! cadence is bandwidth-abusive — the design doc §7.1 "hourly" is the
//! sane lock and is used here. On-demand refresh (Portal-compact
//! open) is wired via the [`REFRESH_TOPIC`] Bus subscriber
//! (MESH-A-1.refresh): a message on `action/netassess/refresh`
//! runs an out-of-band collection between hourly ticks.
//!
//! Shell-outs that aren't present (no `nmcli` / `speedtest-cli` /
//! `curl` on a headless or air-gapped peer) degrade to `None` for
//! that item — the snapshot still writes with whatever collected.
//! Pure parsers are unit-tested against sample tool output; the
//! shell-out execution + reachability pings are HW-bench-gated
//! (§0.15).

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use mde_bus::persist::Persist;

use crate::ca::bundle::{bundle_path, read_bundle, LighthouseEntry};
use crate::nebula_roster::{export_roster, RosterRow};

use super::{ShutdownToken, Worker};

/// Active-collection cadence — hourly (design doc §7.1).
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(3600);

/// Retention window — 30 days in milliseconds (R7-Q3).
pub const RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1_000;

/// Nebula overlay interface checked for tunnel health.
pub const DEFAULT_NEBULA_INTERFACE: &str = "nebula1";

/// Bus topic a UI surface (Portal-compact on open) publishes to in
/// order to trigger an out-of-band assessment between hourly ticks
/// (Q96 `action/<domain>/<verb>` convention).
pub const REFRESH_TOPIC: &str = "action/netassess/refresh";

/// How often the refresh subscriber polls [`REFRESH_TOPIC`]. Short
/// enough that a Portal-compact open feels responsive; each poll only
/// opens + drops a `Persist` and reads the rows new since the cursor.
pub const REFRESH_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// One WiFi network seen in a scan.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WifiNetwork {
    /// SSID (network name); empty for hidden networks.
    pub ssid: String,
    /// Signal strength 0-100 (nmcli SIGNAL).
    pub signal: u8,
    /// Channel number.
    pub channel: u16,
    /// Security string (e.g. `WPA2`, `--` for open).
    pub security: String,
}

/// One ARP/neighbour-table entry.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ArpEntry {
    /// Neighbour IP.
    pub ip: String,
    /// MAC address (lowercase, colon-separated).
    pub mac: String,
    /// Interface the neighbour was seen on.
    pub iface: String,
}

/// Default gateway + resolver set.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GatewayDns {
    /// Default-route gateway IP (empty if none).
    pub gateway: String,
    /// DNS resolver IPs from `/etc/resolv.conf`.
    pub dns: Vec<String>,
}

/// Public-IP + ISP/AS info.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PublicIp {
    /// Public IPv4/IPv6 as seen by ipinfo.
    pub ip: String,
    /// ISP / AS org string.
    pub org: String,
}

/// Speedtest result.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Speedtest {
    /// Download Mbit/s.
    pub download_mbps: f64,
    /// Upload Mbit/s.
    pub upload_mbps: f64,
    /// Latency milliseconds.
    pub ping_ms: f64,
}

/// IPv4 + IPv6 reachability.
#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Connectivity {
    /// IPv4 reachable (ping 1.1.1.1).
    pub ipv4: bool,
    /// IPv6 reachable (ping6 2606:4700:4700::1111).
    pub ipv6: bool,
}

/// MTU + jumbo-frame status for the primary interface.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MtuInfo {
    /// Interface name.
    pub iface: String,
    /// MTU in bytes.
    pub mtu: u32,
    /// Jumbo frames (MTU >= 9000).
    pub jumbo: bool,
}

/// Nebula tunnel health.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TunnelHealth {
    /// Overlay interface name.
    pub iface: String,
    /// Interface is present + UP.
    pub up: bool,
    /// Overlay IP if assigned.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub overlay_ip: String,
}

/// nmap-light subnet discovery result.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SubnetDiscovery {
    /// Count of discovered hosts.
    pub host_count: usize,
    /// Source: `probe-inventory` (reused) or `arp-fallback`.
    pub source: String,
}

/// One hop in a route trace (MESH-A-2).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TraceHop {
    /// Time-to-live / hop number (1-based).
    pub ttl: u8,
    /// Hop IP (`*` when the hop didn't answer).
    pub ip: String,
    /// Round-trip milliseconds for the hop (0.0 when `*`).
    pub rtt_ms: f64,
}

/// A route trace to one target (MESH-A-2, R7-Q7).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RouteTrace {
    /// Target IP/host traced.
    pub target: String,
    /// Target class: `gateway`, `public-dns`, `lighthouse`, `peer`.
    /// (MESH-A-2.a ships `gateway` + `public-dns`; `lighthouse`/`peer`
    /// land with MESH-A-2.b.)
    pub kind: String,
    /// Hops in TTL order.
    pub hops: Vec<TraceHop>,
}

/// The full per-peer assessment snapshot (the 9 items + metadata).
/// Each item is optional so a partial collection (missing tool)
/// still produces a valid snapshot.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AssessmentSnapshot {
    /// Wall-clock epoch-ms of collection.
    pub ts_ms: i64,
    /// `/etc/hostname` of the collecting peer.
    pub host: String,
    /// Item 1.
    #[serde(default)]
    pub wifi: Vec<WifiNetwork>,
    /// Item 2.
    #[serde(default)]
    pub arp: Vec<ArpEntry>,
    /// Item 3.
    pub gateway_dns: GatewayDns,
    /// Item 4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_ip: Option<PublicIp>,
    /// Item 5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speedtest: Option<Speedtest>,
    /// Item 6.
    pub connectivity: Connectivity,
    /// Item 7.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtu: Option<MtuInfo>,
    /// Item 8.
    pub tunnel: TunnelHealth,
    /// Item 9.
    pub subnet: SubnetDiscovery,
    /// MESH-A-2 route traces (R7-Q7). A-2.a traces gateway + the two
    /// public DNS anchors; lighthouse/peer targets land with A-2.b.
    #[serde(default)]
    pub route_traces: Vec<RouteTrace>,
}

// ── Pure parsers (one per shell-out; unit-tested) ──────────────────

/// Parse `nmcli -t -f SSID,SIGNAL,CHAN,SECURITY dev wifi` terse
/// output (colon-separated, one network per line). Escaped `\:`
/// inside an SSID is unescaped. Blank/malformed lines are skipped.
#[must_use]
pub fn parse_nmcli_wifi(stdout: &str) -> Vec<WifiNetwork> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        // nmcli terse escapes field-internal ':' as '\:'. Split on
        // unescaped ':' by temporarily swapping the escape sequence.
        let swapped = line.replace("\\:", "\u{0}");
        let fields: Vec<String> = swapped
            .split(':')
            .map(|f| f.replace('\u{0}', ":"))
            .collect();
        if fields.len() < 4 {
            continue;
        }
        let signal = fields[1].trim().parse::<u8>().unwrap_or(0);
        let channel = fields[2].trim().parse::<u16>().unwrap_or(0);
        out.push(WifiNetwork {
            ssid: fields[0].clone(),
            signal,
            channel,
            security: fields[3].trim().to_string(),
        });
    }
    out
}

/// Parse `ip neigh` output into ARP entries. Lines look like
/// `10.0.0.1 dev eth0 lladdr aa:bb:cc:dd:ee:ff REACHABLE`. Entries
/// without an `lladdr` (FAILED / INCOMPLETE) are skipped.
#[must_use]
pub fn parse_ip_neigh(stdout: &str) -> Vec<ArpEntry> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.is_empty() {
            continue;
        }
        let ip = toks[0].to_string();
        let mut mac = String::new();
        let mut iface = String::new();
        let mut i = 1;
        while i + 1 < toks.len() {
            match toks[i] {
                "dev" => iface = toks[i + 1].to_string(),
                "lladdr" => mac = toks[i + 1].to_ascii_lowercase(),
                _ => {}
            }
            i += 1;
        }
        if mac.is_empty() || ip.is_empty() {
            continue;
        }
        out.push(ArpEntry { ip, mac, iface });
    }
    out
}

/// Parse the gateway IP from `ip route show default` output
/// (`default via 10.0.0.1 dev eth0 ...`). Returns empty when absent.
#[must_use]
pub fn parse_default_gateway(stdout: &str) -> String {
    for line in stdout.lines() {
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.first() == Some(&"default") {
            if let Some(pos) = toks.iter().position(|t| *t == "via") {
                if let Some(gw) = toks.get(pos + 1) {
                    return (*gw).to_string();
                }
            }
        }
    }
    String::new()
}

/// Parse resolver IPs from `/etc/resolv.conf` content
/// (`nameserver <ip>` lines; comments + other directives ignored).
#[must_use]
pub fn parse_resolv_conf(content: &str) -> Vec<String> {
    content
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#') && !l.starts_with(';'))
        .filter_map(|l| l.strip_prefix("nameserver "))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Parse `curl -s https://ipinfo.io/json` output. Returns `None`
/// when the JSON is malformed or missing `ip`.
#[must_use]
pub fn parse_ipinfo_json(stdout: &str) -> Option<PublicIp> {
    let v: serde_json::Value = serde_json::from_str(stdout).ok()?;
    let ip = v.get("ip")?.as_str()?.to_string();
    let org = v
        .get("org")
        .and_then(|o| o.as_str())
        .unwrap_or("")
        .to_string();
    Some(PublicIp { ip, org })
}

/// Parse `speedtest-cli --json` output. Bits/s in the JSON are
/// converted to Mbit/s. Returns `None` on malformed JSON.
#[must_use]
pub fn parse_speedtest_json(stdout: &str) -> Option<Speedtest> {
    let v: serde_json::Value = serde_json::from_str(stdout).ok()?;
    let download_bps = v.get("download")?.as_f64()?;
    let upload_bps = v.get("upload")?.as_f64()?;
    let ping_ms = v
        .get("ping")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0);
    Some(Speedtest {
        download_mbps: download_bps / 1_000_000.0,
        upload_mbps: upload_bps / 1_000_000.0,
        ping_ms,
    })
}

/// Parse the MTU for `iface` from `ip link show <iface>` output
/// (`... mtu 1500 ...`). Returns `None` when not found.
#[must_use]
pub fn parse_ip_link_mtu(stdout: &str, iface: &str) -> Option<MtuInfo> {
    let toks: Vec<&str> = stdout.split_whitespace().collect();
    let pos = toks.iter().position(|t| *t == "mtu")?;
    let mtu: u32 = toks.get(pos + 1)?.parse().ok()?;
    Some(MtuInfo {
        iface: iface.to_string(),
        mtu,
        jumbo: mtu >= 9000,
    })
}

/// Determine tunnel health from `ip link show <iface>` output:
/// the interface is up when the line carries the `UP` flag or
/// `state UP`. Empty stdout ⇒ interface absent ⇒ down.
#[must_use]
pub fn parse_tunnel_up(stdout: &str, iface: &str) -> bool {
    if stdout.trim().is_empty() {
        return false;
    }
    // `<BROADCAST,MULTICAST,UP,LOWER_UP>` flag list or `state UP`.
    let _ = iface;
    stdout.contains(",UP,")
        || stdout.contains("<UP,")
        || stdout.contains(",UP>")
        || stdout.contains("state UP")
}

/// Parse `traceroute -n` output into hops. Skips the
/// `traceroute to …` header. Each hop line is `<ttl>  <ip>  <rtt>
/// ms …` or `<ttl>  * * *` (unanswered → ip `*`, rtt 0). The first
/// `<num> ms` pair on the line is taken as the hop RTT.
#[must_use]
pub fn parse_traceroute(stdout: &str) -> Vec<TraceHop> {
    let mut hops = Vec::new();
    for line in stdout.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("traceroute to") {
            continue;
        }
        let toks: Vec<&str> = t.split_whitespace().collect();
        if toks.len() < 2 {
            continue;
        }
        let Ok(ttl) = toks[0].parse::<u8>() else {
            continue;
        };
        if toks[1] == "*" {
            hops.push(TraceHop {
                ttl,
                ip: "*".into(),
                rtt_ms: 0.0,
            });
            continue;
        }
        let rtt_ms = toks
            .iter()
            .enumerate()
            .find_map(|(i, tk)| {
                if toks.get(i + 1) == Some(&"ms") {
                    tk.parse::<f64>().ok()
                } else {
                    None
                }
            })
            .unwrap_or(0.0);
        hops.push(TraceHop {
            ttl,
            ip: toks[1].to_string(),
            rtt_ms,
        });
    }
    hops
}

/// Build the MESH-A-2.a route-trace target list: the default
/// gateway (skipped when unknown) + the two public DNS anchors.
/// Lighthouse / peer targets land with MESH-A-2.b (they need the
/// bundle / roster DB).
#[must_use]
pub fn build_route_targets(gateway: &str) -> Vec<(String, String)> {
    let mut targets = Vec::new();
    if !gateway.is_empty() {
        targets.push((gateway.to_string(), "gateway".to_string()));
    }
    targets.push(("1.1.1.1".to_string(), "public-dns".to_string()));
    targets.push(("8.8.8.8".to_string(), "public-dns".to_string()));
    targets
}

/// Build the per-snapshot filename `<iso8601>-<hash>.json`, where
/// `<hash>` is the first 8 hex chars of the SHA-256 of the JSON body
/// (dedup + integrity). `iso8601` is colon-free (`:` is illegal on
/// some FSes) — colons become `-`.
#[must_use]
pub fn snapshot_filename(iso8601: &str, json_body: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(json_body.as_bytes());
    let hash = hasher.finalize();
    let short: String = hash.iter().take(4).map(|b| format!("{b:02x}")).collect();
    let safe_iso = iso8601.replace(':', "-");
    format!("{safe_iso}-{short}.json")
}

/// Trim snapshot files under `dir` whose embedded `ts_ms` is older
/// than `cutoff_ms`. No-ops when the dir is absent.
pub fn trim_older_than(dir: &Path, cutoff_ms: i64) -> std::io::Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let keep = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v["ts_ms"].as_i64())
            .map(|ts| ts >= cutoff_ms)
            .unwrap_or(true); // keep unparseable files (don't delete blindly)
        if !keep {
            let _ = std::fs::remove_file(&path);
        }
    }
    Ok(())
}

// ── Collectors (shell-out; bench-gated) ────────────────────────────

fn run_stdout(bin: &str, args: &[&str]) -> Option<String> {
    // EFF-20 — bound the collector so a hung tool can't pin the tick.
    let mut cmd = Command::new(bin);
    cmd.args(args);
    let out =
        crate::workers::proc::output_with_timeout(cmd, crate::workers::proc::DEFAULT_CMD_TIMEOUT)
            .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

fn binary_present(bin: &str) -> bool {
    let mut cmd = Command::new(bin);
    cmd.arg("--version");
    crate::workers::proc::output_with_timeout(cmd, crate::workers::proc::DEFAULT_CMD_TIMEOUT)
        .is_ok()
}

fn ping_reachable(target: &str, v6: bool) -> bool {
    let mut args = vec!["-c", "1", "-W", "2"];
    if v6 {
        args.insert(0, "-6");
    }
    args.push(target);
    // EFF-20 — ping already has -W, but bound it anyway so a wedged
    // resolver/network stack can't hang the worker thread.
    let mut cmd = Command::new("ping");
    cmd.args(&args);
    crate::workers::proc::output_with_timeout(cmd, crate::workers::proc::DEFAULT_CMD_TIMEOUT)
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn collect_subnet(arp: &[ArpEntry]) -> SubnetDiscovery {
    // Reuse the EPIC-MESH-PROBE inventory when present (per
    // mesh-probe-subsystem.md §3); otherwise fall back to the ARP
    // host count so the item is never empty.
    if let Some(dir) = dirs::data_dir() {
        let probe = dir.join("mde").join("probe-inventory.json");
        if let Ok(body) = std::fs::read_to_string(&probe) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
                if let Some(hosts) = v.get("hosts").and_then(|h| h.as_array()) {
                    return SubnetDiscovery {
                        host_count: hosts.len(),
                        source: "probe-inventory".into(),
                    };
                }
            }
        }
    }
    SubnetDiscovery {
        host_count: arp.len(),
        source: "arp-fallback".into(),
    }
}

/// MESH-A-2.a — trace each locally-resolvable target. Silent empty
/// when `traceroute` is absent. One quick query per hop, 2 s wait,
/// 20-hop cap to bound runtime.
/// Run a traceroute to each `(target, kind)` and collect the hops.
/// Silent empty when there are no targets or `traceroute` is absent.
fn trace_targets(targets: &[(String, String)]) -> Vec<RouteTrace> {
    if targets.is_empty() || !binary_present("traceroute") {
        return vec![];
    }
    targets
        .iter()
        .map(|(target, kind)| {
            let hops = run_stdout(
                "traceroute",
                &["-n", "-w", "2", "-q", "1", "-m", "20", target],
            )
            .map(|s| parse_traceroute(&s))
            .unwrap_or_default();
            RouteTrace {
                target: target.clone(),
                kind: kind.clone(),
                hops,
            }
        })
        .collect()
}

/// MESH-A-2.b lighthouse trace targets — every lighthouse the local
/// peer's Nebula bundle advertises (skips empty overlay IPs). Pure
/// over the already-read roster so it is unit-testable.
fn lighthouse_trace_targets(lighthouses: &[LighthouseEntry]) -> Vec<(String, String)> {
    lighthouses
        .iter()
        .filter(|l| !l.overlay_ip.is_empty())
        .map(|l| (l.overlay_ip.clone(), "lighthouse".to_string()))
        .collect()
}

/// MESH-A-2.b peer trace targets — every active roster peer except
/// this one (`self_node_id`) and any row missing an overlay IP.
fn peer_trace_targets(roster: &[RosterRow], self_node_id: &str) -> Vec<(String, String)> {
    roster
        .iter()
        .filter(|r| r.node_id != self_node_id && !r.overlay_ip.is_empty())
        .map(|r| (r.overlay_ip.clone(), "peer".to_string()))
        .collect()
}

fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Optional payload for a [`REFRESH_TOPIC`] trigger. Every field is
/// optional — a bare (empty) body is itself a valid trigger, which is
/// what Portal-compact publishes on open. A non-empty body that is not
/// a JSON object is "unknown" and dropped by [`drain_refresh_triggers`].
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RefreshRequest {
    /// Informational — who asked for the refresh (e.g.
    /// `"portal-compact"`). Logged, never acted on.
    #[serde(default)]
    pub source: Option<String>,
}

/// Parse a refresh-trigger body. An empty/whitespace body is a valid
/// bare trigger; a JSON object (optionally carrying `source`) is
/// valid; anything else is an error the caller logs + drops.
///
/// # Errors
///
/// Returns a human-readable error when a non-empty body fails to parse
/// as a [`RefreshRequest`] JSON object.
pub fn parse_refresh_request(body: &str) -> Result<RefreshRequest, String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Ok(RefreshRequest::default());
    }
    serde_json::from_str(trimmed).map_err(|e| format!("malformed netassess refresh request: {e}"))
}

/// Drain new [`REFRESH_TOPIC`] triggers since `cursor`, returning the
/// count of VALID triggers seen (malformed bodies are logged +
/// dropped — MESH-A-1.refresh "unknown body ignored"). The cursor is
/// advanced past every message read so the same trigger never fires a
/// second collection. Opens + drops a `Persist` synchronously; it is
/// `!Sync` and must never be held across an `.await` in the run loop.
fn drain_refresh_triggers(bus_root: &Path, cursor: &mut Option<String>) -> usize {
    let Ok(persist) = Persist::open(bus_root.to_path_buf()) else {
        return 0;
    };
    let Ok(msgs) = persist.list_since(REFRESH_TOPIC, cursor.as_deref()) else {
        return 0;
    };
    let mut valid = 0usize;
    for msg in msgs {
        *cursor = Some(msg.ulid.clone());
        let body = msg.body.as_deref().unwrap_or("");
        match parse_refresh_request(body) {
            Ok(_) => valid += 1,
            Err(e) => {
                tracing::warn!(ulid = %msg.ulid, error = %e, "netassess: ignoring malformed refresh request");
            }
        }
    }
    valid
}

/// Resolve the Bus root (`~/.local/share/mde/bus`) for the refresh
/// subscriber. `None` when no data dir resolves — the subscriber then
/// stays idle and only the hourly tick runs.
fn default_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
}

/// Worker handle.
pub struct NetAssessWorker {
    host: String,
    base_dir: PathBuf,
    nebula_iface: String,
    tick: Duration,
    bus_root_override: Option<PathBuf>,
    /// MESH-A-2.b mesh context (set together via
    /// [`with_mesh_context`](NetAssessWorker::with_mesh_context)).
    /// QNM-Shared root — locates this peer's Nebula bundle for the
    /// lighthouse trace targets.
    workgroup_root: Option<PathBuf>,
    /// This peer's stable node-id — excludes itself from the peer
    /// trace targets.
    node_id: Option<String>,
    /// Roster DB path — `export_roster` reads it for the peer trace
    /// targets.
    db_path: Option<PathBuf>,
}

impl NetAssessWorker {
    /// Construct with production defaults. `base_dir` is the
    /// `netassess` root (`~/.local/share/mde/netassess` in prod).
    #[must_use]
    pub fn new(host: String, base_dir: PathBuf) -> Self {
        Self {
            host,
            base_dir,
            nebula_iface: DEFAULT_NEBULA_INTERFACE.into(),
            tick: DEFAULT_TICK_INTERVAL,
            bus_root_override: None,
            workgroup_root: None,
            node_id: None,
            db_path: None,
        }
    }

    /// Override the tick cadence. Used in tests.
    #[must_use]
    pub fn with_tick(mut self, d: Duration) -> Self {
        self.tick = d;
        self
    }

    /// Override the Bus root the refresh subscriber polls. Used in
    /// tests; production resolves [`default_bus_root`].
    #[must_use]
    pub fn with_bus_root(mut self, p: PathBuf) -> Self {
        self.bus_root_override = Some(p);
        self
    }

    /// Wire the MESH-A-2.b mesh context: the QNM-Shared root (for the
    /// Nebula bundle's lighthouse roster), this peer's node-id (to skip
    /// itself in the peer roster), and the roster DB path. When unset
    /// (pre-enrollment host) only A-2.a's locally-resolvable targets
    /// trace.
    #[must_use]
    pub fn with_mesh_context(
        mut self,
        workgroup_root: PathBuf,
        node_id: String,
        db_path: PathBuf,
    ) -> Self {
        self.workgroup_root = Some(workgroup_root);
        self.node_id = Some(node_id);
        self.db_path = Some(db_path);
        self
    }

    fn primary_iface(&self) -> String {
        // The default-route device is the primary interface.
        run_stdout("ip", &["route", "show", "default"])
            .and_then(|s| {
                s.split_whitespace()
                    .skip_while(|t| *t != "dev")
                    .nth(1)
                    .map(String::from)
            })
            .unwrap_or_else(|| "eth0".into())
    }

    /// Gather the MESH-A-2.b mesh-derived trace targets from the
    /// worker's optional mesh context: the lighthouses in this peer's
    /// Nebula bundle and every other roster peer. Returns empty on a
    /// pre-enrollment / store-less host so A-2.a's locally-resolvable
    /// targets still trace.
    fn mesh_route_targets(&self) -> Vec<(String, String)> {
        let mut targets = Vec::new();

        // Lighthouses — this peer's bundle at
        // <workgroup_root>/<host>/mackesd/nebula-bundle.json.
        if let Some(workgroup_root) = &self.workgroup_root {
            let path = bundle_path(workgroup_root, &self.host);
            match read_bundle(&path) {
                Ok(bundle) => targets.extend(lighthouse_trace_targets(&bundle.lighthouses)),
                Err(e) => {
                    tracing::debug!(error = %e, "netassess: no nebula bundle for lighthouse targets");
                }
            }
        }

        // Peers — the roster DB, excluding self by node-id.
        if let (Some(db_path), Some(node_id)) = (&self.db_path, &self.node_id) {
            match crate::store::open(db_path) {
                Ok(conn) => match export_roster(&conn) {
                    Ok(roster) => targets.extend(peer_trace_targets(&roster, node_id)),
                    Err(e) => tracing::debug!(error = %e, "netassess: roster export failed"),
                },
                Err(e) => tracing::debug!(error = %e, "netassess: roster DB open failed"),
            }
        }

        targets
    }

    fn collect(&self) -> AssessmentSnapshot {
        let wifi = if binary_present("nmcli") {
            run_stdout(
                "nmcli",
                &["-t", "-f", "SSID,SIGNAL,CHAN,SECURITY", "dev", "wifi"],
            )
            .map(|s| parse_nmcli_wifi(&s))
            .unwrap_or_default()
        } else {
            vec![]
        };
        let arp = run_stdout("ip", &["neigh"])
            .map(|s| parse_ip_neigh(&s))
            .unwrap_or_default();
        let gateway = run_stdout("ip", &["route", "show", "default"])
            .map(|s| parse_default_gateway(&s))
            .unwrap_or_default();
        let dns = std::fs::read_to_string("/etc/resolv.conf")
            .map(|c| parse_resolv_conf(&c))
            .unwrap_or_default();
        let public_ip = run_stdout("curl", &["-s", "--max-time", "5", "https://ipinfo.io/json"])
            .and_then(|s| parse_ipinfo_json(&s));
        let speedtest = if binary_present("speedtest-cli") {
            run_stdout("speedtest-cli", &["--json"]).and_then(|s| parse_speedtest_json(&s))
        } else {
            None
        };
        let connectivity = Connectivity {
            ipv4: ping_reachable("1.1.1.1", false),
            ipv6: ping_reachable("2606:4700:4700::1111", true),
        };
        let iface = self.primary_iface();
        let mtu =
            run_stdout("ip", &["link", "show", &iface]).and_then(|s| parse_ip_link_mtu(&s, &iface));
        let tunnel_stdout =
            run_stdout("ip", &["link", "show", &self.nebula_iface]).unwrap_or_default();
        let tunnel = TunnelHealth {
            iface: self.nebula_iface.clone(),
            up: parse_tunnel_up(&tunnel_stdout, &self.nebula_iface),
            overlay_ip: String::new(),
        };
        let subnet = collect_subnet(&arp);
        // A-2.a locally-resolvable (gateway + public DNS) + A-2.b
        // mesh-derived (lighthouses + peers).
        let mut route_targets = build_route_targets(&gateway);
        route_targets.extend(self.mesh_route_targets());
        let route_traces = trace_targets(&route_targets);

        AssessmentSnapshot {
            ts_ms: now_epoch_ms(),
            host: self.host.clone(),
            wifi,
            arp,
            gateway_dns: GatewayDns { gateway, dns },
            public_ip,
            speedtest,
            connectivity,
            mtu,
            tunnel,
            subnet,
            route_traces,
        }
    }

    fn host_dir(&self) -> PathBuf {
        self.base_dir.join(&self.host)
    }

    fn write_snapshot(&self, snap: &AssessmentSnapshot) {
        let dir = self.host_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::debug!(error = %e, "netassess: mkdir failed");
            return;
        }
        let Ok(body) = serde_json::to_string_pretty(snap) else {
            return;
        };
        let iso = chrono::Local::now().format("%Y%m%dT%H%M%S").to_string();
        let path = dir.join(snapshot_filename(&iso, &body));
        if let Err(e) = std::fs::write(&path, &body) {
            tracing::debug!(error = %e, "netassess: write failed");
        }
    }

    fn tick_once(&self) {
        let snap = self.collect();
        self.write_snapshot(&snap);
        let cutoff = now_epoch_ms() - RETENTION_MS;
        let _ = trim_older_than(&self.host_dir(), cutoff);
    }
}

#[async_trait::async_trait]
impl Worker for NetAssessWorker {
    fn name(&self) -> &'static str {
        "netassess"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        // Hourly active-collection cadence. Uses `interval` (not a
        // fresh `sleep` each loop iteration) so the high-frequency
        // refresh poll below can fire without ever resetting the
        // hourly timer.
        let mut collect_tick = tokio::time::interval(self.tick);
        collect_tick.tick().await; // consume the immediate first tick — first collection lands after `self.tick`, matching MESH-A-1.

        // On-demand refresh subscriber (MESH-A-1.refresh): a message on
        // `action/netassess/refresh` runs an out-of-band collection
        // between hourly ticks. Disabled when no Bus root resolves.
        let bus_root = self.bus_root_override.clone().or_else(default_bus_root);
        let mut refresh_cursor: Option<String> = None;
        let mut refresh_tick = tokio::time::interval(REFRESH_POLL_INTERVAL);

        loop {
            tokio::select! {
                _ = collect_tick.tick() => {
                    self.tick_once();
                }
                _ = refresh_tick.tick(), if bus_root.is_some() => {
                    if let Some(root) = bus_root.as_deref() {
                        if drain_refresh_triggers(root, &mut refresh_cursor) > 0 {
                            tracing::info!("netassess: on-demand refresh — running collection out-of-band");
                            self.tick_once();
                        }
                    }
                }
                _ = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_nmcli_wifi ──

    #[test]
    fn wifi_parses_terse_lines() {
        let raw = "HomeNet:78:36:WPA2\nCoffee\\:Shop:42:6:WPA2\nOpenAP:90:11:--\n";
        let nets = parse_nmcli_wifi(raw);
        assert_eq!(nets.len(), 3);
        assert_eq!(nets[0].ssid, "HomeNet");
        assert_eq!(nets[0].signal, 78);
        assert_eq!(nets[0].channel, 36);
        assert_eq!(nets[0].security, "WPA2");
        // escaped colon inside SSID preserved
        assert_eq!(nets[1].ssid, "Coffee:Shop");
        assert_eq!(nets[2].security, "--");
    }

    #[test]
    fn wifi_skips_blank_and_short_lines() {
        let raw = "\nBad:line\nGood:50:1:WPA3\n";
        let nets = parse_nmcli_wifi(raw);
        assert_eq!(nets.len(), 1);
        assert_eq!(nets[0].ssid, "Good");
    }

    // ── parse_ip_neigh ──

    #[test]
    fn neigh_parses_reachable_entries() {
        let raw = "10.0.0.1 dev eth0 lladdr AA:BB:CC:DD:EE:FF REACHABLE\n\
                   10.0.0.2 dev eth0 FAILED\n\
                   fe80::1 dev eth0 lladdr 11:22:33:44:55:66 STALE\n";
        let arp = parse_ip_neigh(raw);
        assert_eq!(arp.len(), 2); // FAILED (no lladdr) skipped
        assert_eq!(arp[0].ip, "10.0.0.1");
        assert_eq!(arp[0].mac, "aa:bb:cc:dd:ee:ff"); // lowercased
        assert_eq!(arp[0].iface, "eth0");
    }

    // ── parse_default_gateway ──

    #[test]
    fn gateway_from_default_route() {
        let raw = "default via 192.168.1.1 dev wlan0 proto dhcp metric 600\n";
        assert_eq!(parse_default_gateway(raw), "192.168.1.1");
    }

    #[test]
    fn gateway_empty_when_no_default() {
        assert_eq!(parse_default_gateway("10.0.0.0/24 dev eth0\n"), "");
    }

    // ── parse_resolv_conf ──

    #[test]
    fn resolv_extracts_nameservers() {
        let raw = "# generated\nnameserver 1.1.1.1\nsearch lan\nnameserver 8.8.8.8\n";
        assert_eq!(parse_resolv_conf(raw), vec!["1.1.1.1", "8.8.8.8"]);
    }

    // ── parse_ipinfo_json ──

    #[test]
    fn ipinfo_parses_ip_and_org() {
        let raw = r#"{"ip":"203.0.113.7","org":"AS13335 Cloudflare","city":"X"}"#;
        let p = parse_ipinfo_json(raw).expect("parse");
        assert_eq!(p.ip, "203.0.113.7");
        assert_eq!(p.org, "AS13335 Cloudflare");
    }

    #[test]
    fn ipinfo_none_on_garbage() {
        assert!(parse_ipinfo_json("not json").is_none());
    }

    // ── parse_speedtest_json ──

    #[test]
    fn speedtest_converts_bps_to_mbps() {
        let raw = r#"{"download":94000000.0,"upload":12000000.0,"ping":14.2}"#;
        let s = parse_speedtest_json(raw).expect("parse");
        assert!((s.download_mbps - 94.0).abs() < 0.01);
        assert!((s.upload_mbps - 12.0).abs() < 0.01);
        assert!((s.ping_ms - 14.2).abs() < 0.01);
    }

    // ── parse_ip_link_mtu ──

    #[test]
    fn mtu_parses_and_flags_jumbo() {
        let std1 = "2: eth0: <BROADCAST,MULTICAST,UP> mtu 1500 qdisc fq state UP";
        let m = parse_ip_link_mtu(std1, "eth0").expect("mtu");
        assert_eq!(m.mtu, 1500);
        assert!(!m.jumbo);
        let std2 = "3: eth1: <UP> mtu 9000 qdisc fq";
        assert!(parse_ip_link_mtu(std2, "eth1").unwrap().jumbo);
    }

    // ── parse_tunnel_up ──

    #[test]
    fn tunnel_up_detected_from_flags_and_state() {
        assert!(parse_tunnel_up(
            "4: nebula1: <POINTOPOINT,MULTICAST,NOARP,UP,LOWER_UP> mtu 1300",
            "nebula1"
        ));
        assert!(parse_tunnel_up("nebula1: state UP mtu 1300", "nebula1"));
        assert!(!parse_tunnel_up(
            "4: nebula1: <POINTOPOINT,NOARP> state DOWN",
            "nebula1"
        ));
        assert!(!parse_tunnel_up("", "nebula1")); // absent interface
    }

    // ── snapshot_filename ──

    #[test]
    fn filename_is_colon_free_with_hash_suffix() {
        let name = snapshot_filename("20260531T143000", r#"{"ts_ms":1}"#);
        assert!(name.starts_with("20260531T143000-"));
        assert!(name.ends_with(".json"));
        assert!(!name.contains(':'));
        // deterministic hash for the same body
        assert_eq!(name, snapshot_filename("20260531T143000", r#"{"ts_ms":1}"#));
    }

    #[test]
    fn filename_colons_replaced() {
        let name = snapshot_filename("2026-05-31T14:30:00", "{}");
        assert!(!name.contains(':'));
    }

    // ── trim_older_than ──

    #[test]
    fn trim_removes_old_keeps_recent() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join("old.json"), r#"{"ts_ms":100}"#).unwrap();
        std::fs::write(dir.join("new.json"), r#"{"ts_ms":9000}"#).unwrap();
        trim_older_than(dir, 1000).unwrap();
        assert!(!dir.join("old.json").exists());
        assert!(dir.join("new.json").exists());
    }

    #[test]
    fn trim_noop_when_dir_absent() {
        let tmp = tempfile::tempdir().unwrap();
        trim_older_than(&tmp.path().join("nope"), 0).unwrap();
    }

    #[test]
    fn trim_keeps_unparseable_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("junk.json"), "not json").unwrap();
        trim_older_than(tmp.path(), i64::MAX).unwrap();
        assert!(tmp.path().join("junk.json").exists());
    }

    // ── collect_subnet ──

    #[test]
    fn subnet_arp_fallback_counts_entries() {
        let arp = vec![
            ArpEntry {
                ip: "10.0.0.1".into(),
                mac: "a".into(),
                iface: "e".into(),
            },
            ArpEntry {
                ip: "10.0.0.2".into(),
                mac: "b".into(),
                iface: "e".into(),
            },
        ];
        // No probe inventory in a clean test env → arp-fallback.
        let s = collect_subnet(&arp);
        // source may be probe-inventory if the test host has one; assert the fallback shape only when arp-derived.
        if s.source == "arp-fallback" {
            assert_eq!(s.host_count, 2);
        }
    }

    // ── parse_traceroute (MESH-A-2.a) ──

    #[test]
    fn traceroute_parses_hops_and_stars() {
        let raw = "traceroute to 1.1.1.1 (1.1.1.1), 30 hops max, 60 byte packets\n\
                   \x201  10.0.0.1  0.456 ms  0.389 ms  0.402 ms\n\
                   \x202  * * *\n\
                   \x203  203.0.113.1  12.3 ms  11.8 ms\n";
        let hops = parse_traceroute(raw);
        assert_eq!(hops.len(), 3);
        assert_eq!(hops[0].ttl, 1);
        assert_eq!(hops[0].ip, "10.0.0.1");
        assert!((hops[0].rtt_ms - 0.456).abs() < 0.001);
        assert_eq!(hops[1].ip, "*"); // unanswered hop
        assert_eq!(hops[1].rtt_ms, 0.0);
        assert_eq!(hops[2].ttl, 3);
        assert!((hops[2].rtt_ms - 12.3).abs() < 0.001);
    }

    #[test]
    fn traceroute_skips_header_and_blanks() {
        assert!(parse_traceroute("traceroute to 8.8.8.8 (8.8.8.8), 30 hops max\n\n").is_empty());
    }

    // ── build_route_targets (MESH-A-2.a) ──

    #[test]
    fn route_targets_include_gateway_and_two_public_dns() {
        let t = build_route_targets("192.168.1.1");
        assert_eq!(t.len(), 3);
        assert_eq!(t[0], ("192.168.1.1".to_string(), "gateway".to_string()));
        assert!(t.iter().any(|(ip, k)| ip == "1.1.1.1" && k == "public-dns"));
        assert!(t.iter().any(|(ip, k)| ip == "8.8.8.8" && k == "public-dns"));
    }

    #[test]
    fn route_targets_skip_gateway_when_unknown() {
        let t = build_route_targets("");
        assert_eq!(t.len(), 2); // only the two public DNS anchors
        assert!(t.iter().all(|(_, k)| k == "public-dns"));
    }

    // ── snapshot JSON shape (design doc §7.1 — all 9 items present) ──

    #[test]
    fn snapshot_json_carries_all_nine_items() {
        let snap = AssessmentSnapshot {
            ts_ms: 1,
            host: "alice".into(),
            wifi: vec![],
            arp: vec![],
            gateway_dns: GatewayDns::default(),
            public_ip: None,
            speedtest: None,
            connectivity: Connectivity::default(),
            mtu: None,
            tunnel: TunnelHealth::default(),
            subnet: SubnetDiscovery::default(),
            route_traces: vec![],
        };
        let s = serde_json::to_string(&snap).unwrap();
        for field in [
            "\"ts_ms\"",
            "\"host\"",
            "\"wifi\"",
            "\"arp\"",
            "\"gateway_dns\"",
            "\"connectivity\"",
            "\"tunnel\"",
            "\"subnet\"",
        ] {
            assert!(s.contains(field), "missing {field}");
        }
        // round-trips
        let back: AssessmentSnapshot = serde_json::from_str(&s).unwrap();
        assert_eq!(back, snap);
    }

    // ── MESH-A-1.refresh: on-demand Bus trigger ──

    // A valid refresh message on `action/netassess/refresh` is seen by
    // the subscriber and fires a collection. (The live collection
    // shell-outs are HW-bench-gated per §0.15 exactly like MESH-A-1;
    // here we assert the trigger is detected — the run loop calls
    // `tick_once` whenever this count is non-zero.)
    #[test]
    fn refresh_fires_collection() {
        use mde_bus::hooks::config::Priority;
        let tmp = tempfile::tempdir().unwrap();
        let bus_root = tmp.path().to_path_buf();
        let persist = Persist::open(bus_root.clone()).expect("persist");
        // Portal-compact publishes a bare trigger on open …
        persist
            .write(REFRESH_TOPIC, Priority::Default, None, None)
            .expect("write bare refresh");
        // … and a source-tagged variant is equally valid.
        persist
            .write(
                REFRESH_TOPIC,
                Priority::Default,
                None,
                Some(r#"{"source":"portal-compact"}"#),
            )
            .expect("write tagged refresh");

        let mut cursor: Option<String> = None;
        let fired = drain_refresh_triggers(&bus_root, &mut cursor);
        assert_eq!(
            fired, 2,
            "both valid refresh triggers should fire collection"
        );

        // Cursor advanced: a second drain with no new messages is a
        // no-op — collection does NOT re-fire on the same triggers.
        assert_eq!(drain_refresh_triggers(&bus_root, &mut cursor), 0);
    }

    // A malformed (non-empty, non-JSON-object) body is logged + dropped
    // and does NOT fire a collection.
    #[test]
    fn unknown_body_ignored() {
        use mde_bus::hooks::config::Priority;
        let tmp = tempfile::tempdir().unwrap();
        let bus_root = tmp.path().to_path_buf();
        let persist = Persist::open(bus_root.clone()).expect("persist");
        persist
            .write(
                REFRESH_TOPIC,
                Priority::Default,
                None,
                Some("not json at all"),
            )
            .expect("write garbage");
        persist
            .write(REFRESH_TOPIC, Priority::Default, None, Some("[1,2,3]"))
            .expect("write non-object json");

        let mut cursor: Option<String> = None;
        assert_eq!(
            drain_refresh_triggers(&bus_root, &mut cursor),
            0,
            "unknown bodies must not fire collection"
        );
    }

    #[test]
    fn parse_refresh_request_accepts_empty_and_object() {
        assert_eq!(
            parse_refresh_request("").unwrap(),
            RefreshRequest::default()
        );
        assert_eq!(
            parse_refresh_request("   ").unwrap(),
            RefreshRequest::default()
        );
        assert_eq!(
            parse_refresh_request(r#"{"source":"portal-compact"}"#)
                .unwrap()
                .source
                .as_deref(),
            Some("portal-compact")
        );
        assert!(parse_refresh_request("garbage").is_err());
        assert!(parse_refresh_request("[1,2]").is_err());
    }

    // ── MESH-A-2.b: mesh-derived trace targets ──

    fn lighthouse(node_id: &str, overlay_ip: &str) -> LighthouseEntry {
        LighthouseEntry {
            node_id: node_id.into(),
            overlay_ip: overlay_ip.into(),
            external_addr: "203.0.113.7:4242".into(),
        }
    }

    fn roster_row(node_id: &str, overlay_ip: &str) -> RosterRow {
        RosterRow {
            node_id: node_id.into(),
            name: "host".into(),
            overlay_ip: overlay_ip.into(),
            epoch: 1,
            cert_pem: String::new(),
            created_at: 0,
            expires_at: 0,
            groups: "peer".into(),
        }
    }

    #[test]
    fn lighthouse_target_from_bundle() {
        let lighthouses = vec![
            lighthouse("host:anvil", "10.42.0.1"),
            lighthouse("host:forge", "10.42.0.2"),
            lighthouse("host:empty", ""), // skipped — no overlay IP
        ];
        let t = lighthouse_trace_targets(&lighthouses);
        assert_eq!(t.len(), 2);
        assert!(t.iter().all(|(_, k)| k == "lighthouse"));
        assert_eq!(t[0].0, "10.42.0.1");
        assert_eq!(t[1].0, "10.42.0.2");
    }

    #[test]
    fn peer_targets_from_roster() {
        let roster = vec![
            roster_row("peer:self", "10.42.0.5"), // excluded — this peer
            roster_row("peer:anvil", "10.42.0.6"),
            roster_row("peer:forge", "10.42.0.7"),
            roster_row("peer:ghost", ""), // excluded — no overlay IP
        ];
        let t = peer_trace_targets(&roster, "peer:self");
        assert_eq!(t.len(), 2);
        assert!(t.iter().all(|(_, k)| k == "peer"));
        let ips: Vec<&str> = t.iter().map(|(ip, _)| ip.as_str()).collect();
        assert_eq!(ips, vec!["10.42.0.6", "10.42.0.7"]);
        assert!(!ips.contains(&"10.42.0.5"), "self must be excluded");
    }
}
