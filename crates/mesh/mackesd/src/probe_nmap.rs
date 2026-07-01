//! EPIC-MESH-PROBE (MESH-PROBE-2) — the nmap probe engine.
//!
//! Per Q3/Q4, nmap is the sole probe engine. This module owns:
//!   * [`fast_argv`] / [`deep_argv`] — pure-fn `nmap` argv builders
//!     for the two-tier cadence (Q6): a fast liveness/known-port pass
//!     and a deep `-sV`/NSE identification pass. Both emit `-oX -`
//!     (XML to stdout) and are `-T`-rate-limited (never `-T5`).
//!   * [`parse_nmap_xml`] — roxmltree parse of `-oX` output into
//!     [`crate::card`] Host cards with Service children (Q7).
//!   * [`scan`] — shell `nmap`, parse the result; nmap-absent ⇒ empty
//!     + warn (no panic). The `Requires: nmap` RPM dep (MESH-PROBE-3)
//!     guarantees the binary in production; this graceful-degrade is
//!     for dev hosts / pre-install peers.
//!
//! The scheduled two-tier worker + GFS write + Bus `probe/changed`
//! event are MESH-PROBE-4; the operator-facing `mackesd probe scan`
//! CLI in `bin/mackesd.rs` is the runtime entry point that makes this
//! engine reachable end-to-end today (§0.12).

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::card::probe::{
    host_card, host_facts, service_card, service_facts, HostFacts, HostSource, ServiceFacts,
};
use crate::card::Card;

/// Curated port set both profiles scan. Union of the media ports the
/// EPIC-SYNC-APP-CONFIG discovery needs (Airsonic 4040, Jellyfin
/// 8096, Navidrome 4533) + the MESH-A-7 well-known connect-action
/// ports (SSH/HTTP/HTTPS/SMB/RDP/VNC/FTP/CUPS/psql/mysql/redis/
/// HTTP-alt/mongo). Kept small so the fast pass stays ~sub-second
/// per host.
pub const CURATED_PORTS: &[u16] = &[
    21, 22, 80, 443, 445, 631, 3306, 3389, 4040, 4533, 5432, 5900, 6379, 8080, 8096, 27017,
];

/// nmap timing template. `-T3` ("normal", the nmap default) is the
/// polite choice — fast enough for an 8-peer mesh + a LAN segment
/// without the IDS-tripping aggression of `-T4`/`-T5`. The design
/// §7 risk note mandates "not `-T5`".
const TIMING: &str = "-T3";

/// nmap binary name (overridable in `scan` for tests).
pub const DEFAULT_NMAP_BINARY: &str = "nmap";

/// Which probe profile to run (Q6 two-tier cadence).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// Fast liveness + curated-port open/closed pass (no `-sV`).
    Fast,
    /// Deep `-sV --version-all` + bundled-NSE identification pass.
    Deep,
}

/// Comma-joined curated port list for `-p`.
fn port_spec() -> String {
    CURATED_PORTS
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// Build the `nmap` argv (sans binary) for the fast profile over
/// `targets`. Pure — exposed for unit tests.
#[must_use]
pub fn fast_argv(targets: &[String]) -> Vec<String> {
    // SUBAUDIT-C2 — no `--open`: Discovered Hosts must list every *up*
    // mesh peer, even one with no open curated port (the parser already
    // cards an up host with empty services). `--open` suppressed those,
    // leaving the panel at "0 hosts" on a healthy mesh.
    let mut argv = vec![
        TIMING.to_owned(),
        // SUBAUDIT-C2 — bound per-host time so a slow/LAN target can't
        // stall the inventory write (the fast cycle must stay fast).
        "--host-timeout".to_owned(),
        "20s".to_owned(),
        "-p".to_owned(),
        port_spec(),
        "-oX".to_owned(),
        "-".to_owned(),
    ];
    argv.extend(targets.iter().cloned());
    argv
}

/// Build the `nmap` argv (sans binary) for the deep profile over
/// `targets`. `nse_dir` is the bundled-NSE script path
/// (MESH-PROBE-3); when empty the `--script` flag is omitted so the
/// argv still runs against stock nmap. Pure — exposed for tests.
#[must_use]
pub fn deep_argv(targets: &[String], nse_dir: &str) -> Vec<String> {
    let mut argv = vec![
        TIMING.to_owned(),
        "--host-timeout".to_owned(),
        "60s".to_owned(),
        "-sV".to_owned(),
        "--version-all".to_owned(),
        "-p".to_owned(),
        port_spec(),
        // SUBAUDIT-C2 — no `--open`; list up hosts even with no open port.
    ];
    if !nse_dir.is_empty() {
        argv.push("--script".to_owned());
        argv.push(nse_dir.to_owned());
    }
    argv.push("-oX".to_owned());
    argv.push("-".to_owned());
    argv.extend(targets.iter().cloned());
    argv
}

/// Parse nmap `-oX` XML into a Host card per up-host, each with a
/// Service child card per open port. `source` + `now_ts` are the
/// scan context the XML doesn't carry (the caller knows whether it
/// scanned a mesh peer / LAN / arbitrary target, and the wall clock).
/// Malformed XML ⇒ empty vec (logged by the caller). Hosts that are
/// not `up`, and ports that are not `open`, are skipped.
#[must_use]
pub fn parse_nmap_xml(xml: &str, source: HostSource, now_ts: u64) -> Vec<Card> {
    // Real nmap `-oX` output opens with `<!DOCTYPE nmaprun>`;
    // roxmltree rejects a DOCTYPE unless `allow_dtd` is set.
    let opts = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..roxmltree::ParsingOptions::default()
    };
    let doc = match roxmltree::Document::parse_with_options(xml, opts) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for host in doc.descendants().filter(|n| n.has_tag_name("host")) {
        // Only `up` hosts.
        let up = host
            .children()
            .find(|n| n.has_tag_name("status"))
            .and_then(|s| s.attribute("state"))
            .is_some_and(|st| st == "up");
        if !up {
            continue;
        }
        // First IPv4 address.
        let Some(ip) = host
            .children()
            .filter(|n| n.has_tag_name("address"))
            .find(|n| n.attribute("addrtype") == Some("ipv4"))
            .and_then(|n| n.attribute("addr"))
        else {
            continue;
        };
        // Optional first hostname.
        let hostname = host
            .descendants()
            .find(|n| n.has_tag_name("hostname"))
            .and_then(|n| n.attribute("name"))
            .unwrap_or("")
            .to_owned();

        let mut services = Vec::new();
        for port in host.descendants().filter(|n| n.has_tag_name("port")) {
            let open = port
                .children()
                .find(|n| n.has_tag_name("state"))
                .and_then(|s| s.attribute("state"))
                .is_some_and(|st| st == "open");
            if !open {
                continue;
            }
            let Some(portid) = port.attribute("portid").and_then(|p| p.parse::<u16>().ok()) else {
                continue;
            };
            let svc = port.children().find(|n| n.has_tag_name("service"));
            let service_kind = svc
                .and_then(|s| s.attribute("name"))
                .unwrap_or("")
                .to_owned();
            let product = svc
                .and_then(|s| s.attribute("product"))
                .unwrap_or("")
                .to_owned();
            let version = svc
                .and_then(|s| s.attribute("version"))
                .unwrap_or("")
                .to_owned();
            services.push(service_card(
                &ServiceFacts {
                    port: portid,
                    service_kind,
                    product,
                    version,
                    fingerprint: String::new(),
                },
                now_ts,
            ));
        }

        out.push(host_card(
            &HostFacts {
                ip: ip.to_owned(),
                hostname,
                source,
                trust_state: String::new(),
                last_seen: now_ts,
            },
            services,
            now_ts,
        ));
    }
    out
}

/// Run an nmap `profile` against `targets` via `binary`, returning the
/// parsed inventory cards. Best-effort: a missing nmap binary, a
/// non-zero exit with no usable XML, or unparseable output all yield
/// an empty vec (logged at warn) rather than an error — the probe
/// must never crash the daemon. `nse_dir` is passed to the deep
/// profile only, and **only when it exists**: nmap aborts (exit 1,
/// zero hosts) if `--script <dir>` points at a missing path, so on a
/// peer where the bundled scripts aren't installed yet the deep pass
/// degrades to plain `-sV` rather than failing outright.
#[must_use]
pub fn scan(
    binary: &str,
    profile: Profile,
    targets: &[String],
    excludes: &[String],
    nse_dir: &str,
    source: HostSource,
    now_ts: u64,
) -> Vec<Card> {
    if targets.is_empty() {
        return Vec::new();
    }
    let mut argv = match profile {
        Profile::Fast => fast_argv(targets),
        Profile::Deep => {
            // Guard the NSE dir existence here (I/O), keeping
            // `deep_argv` a pure fn: an empty dir string makes
            // `deep_argv` omit `--script` entirely.
            let effective_nse = if !nse_dir.is_empty() && Path::new(nse_dir).is_dir() {
                nse_dir
            } else {
                if !nse_dir.is_empty() {
                    tracing::debug!(
                        target: "mackesd::probe_nmap",
                        nse_dir,
                        "NSE script dir absent; deep pass falls back to plain -sV",
                    );
                }
                ""
            };
            deep_argv(targets, effective_nse)
        }
    };
    // Q9 do-not-scan escape hatch — nmap's own `--exclude` keeps the
    // excluded hosts/CIDRs out of the scan. Appended after the target
    // spec (nmap accepts options in any position).
    if !excludes.is_empty() {
        argv.push("--exclude".to_owned());
        argv.push(excludes.join(","));
    }
    let output = match Command::new(binary).args(&argv).output() {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!(
                target: "mackesd::probe_nmap",
                binary = %binary,
                error = %e,
                "could not spawn nmap (graceful-degrade; empty inventory). \
                 Install nmap (Requires: nmap in the RPM) to enable probing.",
            );
            return Vec::new();
        }
    };
    // nmap exits non-zero in some partial-scan cases but still emits
    // usable XML on stdout; parse whatever we got. Empty/garbage ⇒
    // parse returns empty.
    let xml = String::from_utf8_lossy(&output.stdout);
    let cards = parse_nmap_xml(&xml, source, now_ts);
    if cards.is_empty() && !output.status.success() {
        tracing::warn!(
            target: "mackesd::probe_nmap",
            code = ?output.status.code(),
            "nmap produced no parseable hosts (non-zero exit)",
        );
    }
    cards
}

// ── Probe cycle orchestration (MESH-PROBE-4) ─────────────────────────
//
// Sync (no tokio), so the `mackesd probe scan/refresh` CLI reaches it
// without the `async-services` feature. The scheduled
// `workers::probe::ProbeWorker` (gated) wraps `run_probe_cycle` with
// the two-tier cadence timer.

/// Inventory filename under `<workgroup_root>/<self>/mackesd/`.
pub const INVENTORY_FILENAME: &str = "probe-inventory.json";
/// Bus topic published when the inventory materially changes (Q2).
pub const CHANGED_TOPIC: &str = "probe/changed";

/// Path this peer writes its probe inventory to. Mirrors the per-peer
/// GFS layout of `ban_list_path` / `pending_enroll_path` so mesh-home
/// replication fans it out to every peer.
#[must_use]
pub fn inventory_path(workgroup_root: &Path, self_node_id: &str) -> PathBuf {
    workgroup_root
        .join(self_node_id)
        .join("mackesd")
        .join(INVENTORY_FILENAME)
}

/// Resolve the mesh-peer scan targets: every peer's Nebula overlay IP
/// from the GFS-replicated bundles (the same source app_sync uses).
#[must_use]
pub fn mesh_targets(workgroup_root: &Path) -> Vec<String> {
    crate::mesh_media::peer_overlay_ips(workgroup_root)
        .into_iter()
        .map(|(_node_id, ip)| ip)
        .collect()
}

/// Filename the `compute_registry` worker mirrors each host's VM/container
/// inventory to under its QNM-Shared dir (`<root>/<host>/<file>`). Mirror of
/// [`crate::workers::compute_registry::SHARED_INVENTORY_FILE`], named here so
/// this resolver doesn't depend on the `async-services`-gated worker module.
const COMPUTE_INVENTORY_FILE: &str = "compute-inventory.json";

/// SVC-VIEW-2 — harvest the overlay IPs of enrolled VMs so the probe scans a
/// VM's services (e.g. whatever runs *inside* MDE-KVM-1), not just full mesh
/// peers + the LAN.
///
/// A VM's Nebula overlay IP lives only in the per-host
/// `<root>/<host>/compute-inventory.json` files (the WORKLOAD-FLEET plane,
/// written by every node's `compute_registry`); it is **not** in the per-peer
/// `nebula-bundle.json` that [`mesh_targets`] reads, so an enrolled VM's
/// overlay IP is otherwise never a probe target and its services never reach
/// `probe-inventory.json` → the Services view. This reads those inventory
/// files and returns each VM's non-empty `nebula_ip`. Fail-open per file: a
/// missing / malformed / unreadable inventory is skipped (debug-logged), and a
/// missing `workgroup_root` → empty. Pure (only reads the replicated plane);
/// `merge_targets` dedupes the result against the mesh + LAN targets.
#[must_use]
pub fn vm_overlay_targets(workgroup_root: &Path) -> Vec<String> {
    // Deserialize just the field we need — decoupled from the full
    // `compute_registry::Inventory` so a future schema change there can't break
    // the probe, and so this stays usable without the `async-services` feature.
    #[derive(serde::Deserialize)]
    struct InvVms {
        #[serde(default)]
        vms: Vec<InvVm>,
    }
    #[derive(serde::Deserialize)]
    struct InvVm {
        #[serde(default)]
        nebula_ip: String,
    }
    let Ok(entries) = std::fs::read_dir(workgroup_root) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path().join(COMPUTE_INVENTORY_FILE);
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        match serde_json::from_str::<InvVms>(&body) {
            Ok(inv) => {
                for vm in inv.vms {
                    if !vm.nebula_ip.is_empty() && !out.contains(&vm.nebula_ip) {
                        out.push(vm.nebula_ip);
                    }
                }
            }
            Err(e) => tracing::debug!(
                target: "mackesd::probe_nmap",
                path = %path.display(),
                error = %e,
                "skipping malformed compute inventory for VM scan targets (fail-open)",
            ),
        }
    }
    out
}

// ── Read API (MESH-PROBE-6) ──────────────────────────────────────────
//
// The consumer-facing side: merge every peer's GFS-replicated
// `probe-inventory.json` into one `Vec<Card>`, and a query for "which
// hosts run service X". The design (Q8) names this `mackesd::probe::`;
// it lives here in `probe_nmap` co-located with the inventory format +
// path (`inventory_path`) + writer it mirrors, rather than a separate
// module that would have to re-export those. In-process consumers
// (app_sync, MESH-A workers) poll `inventory()` on their tick (the
// established mackesd worker pattern) + use `inventory_fingerprint()`
// to skip a re-parse when nothing changed; the push side
// (`probe/changed` Bus event from MESH-PROBE-4) drives the Portal's
// subscription (MESH-PROBE-9) via the standard mde-bus subs.

/// One host + one of its services — the unit `peers_with_service`
/// returns so a consumer can build, e.g., a media-server URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostService {
    /// The host the service runs on.
    pub host: HostFacts,
    /// The matching service.
    pub service: ServiceFacts,
}

/// Merge every peer's `<workgroup_root>/*/mackesd/probe-inventory.json` into
/// one `Vec<Card>` (the union of all host cards across the mesh).
/// Fail-open per file: a missing/malformed/unreadable inventory is
/// skipped (logged at debug) so one corrupt peer can't blind the
/// reader to the others. Missing `workgroup_root` → empty.
#[must_use]
pub fn inventory(workgroup_root: &Path) -> Vec<Card> {
    let entries = match std::fs::read_dir(workgroup_root) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut out: Vec<Card> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path().join("mackesd").join(INVENTORY_FILENAME);
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        match serde_json::from_str::<Vec<Card>>(&body) {
            Ok(cards) => out.extend(cards),
            Err(e) => tracing::debug!(
                target: "mackesd::probe_nmap",
                path = %path.display(),
                error = %e,
                "skipping malformed probe inventory (fail-open)",
            ),
        }
    }
    out
}

/// Find every `(host, service)` in the merged inventory whose service
/// kind matches `kind` (case-insensitive). The query app_sync uses to
/// locate media peers (`peers_with_service("airsonic")`).
#[must_use]
pub fn peers_with_service(workgroup_root: &Path, kind: &str) -> Vec<HostService> {
    let want = kind.to_ascii_lowercase();
    let mut out = Vec::new();
    for host_card in inventory(workgroup_root) {
        let Some(host) = host_facts(&host_card) else {
            continue;
        };
        for child in &host_card.children {
            if let Some(service) = service_facts(child) {
                if service.service_kind.to_ascii_lowercase() == want {
                    out.push(HostService {
                        host: host.clone(),
                        service,
                    });
                }
            }
        }
    }
    out
}

/// Cheap change-detection token over the inventory files: a hash of
/// each peer dir's `(filename, len, mtime)`. A polling consumer keeps
/// the last value + only re-parses [`inventory`] when it changes —
/// avoiding a full JSON parse every tick. Order-independent (sums per
/// file) so directory iteration order doesn't matter.
#[must_use]
pub fn inventory_fingerprint(workgroup_root: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let Ok(entries) = std::fs::read_dir(workgroup_root) else {
        return 0;
    };
    let mut acc: u64 = 0;
    for entry in entries.flatten() {
        let path = entry.path().join("mackesd").join(INVENTORY_FILENAME);
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let mut h = std::collections::hash_map::DefaultHasher::new();
        path.hash(&mut h);
        meta.len().hash(&mut h);
        if let Ok(mtime) = meta.modified() {
            if let Ok(d) = mtime.duration_since(std::time::UNIX_EPOCH) {
                d.as_nanos().hash(&mut h);
            }
        }
        // XOR-accumulate so peer order doesn't affect the result.
        acc ^= h.finish();
    }
    acc
}

// ── Target resolver (MESH-PROBE-5) ───────────────────────────────────
//
// Q5 scope = mesh peers ∪ local LAN ∪ operator-arbitrary; Q9 default =
// scan everything, with a do-not-scan exclusion list as the escape
// hatch (passed to nmap `--exclude`). The pure pieces
// (`ipv4_network_cidr`, `lan_cidrs_from_ip_json`, `merge_targets`,
// `read_toml_string_list`) are unit-tested; the env-touching wrappers
// (`detect_lan_cidrs`, `resolve_targets`) compose them.

/// Interface name-prefixes excluded from LAN detection: loopback, the
/// Nebula overlay (mesh peers are already covered by `mesh_targets`),
/// and the usual virtual bridges (container / VM / legacy-mesh nets).
pub const DEFAULT_EXCLUDE_IFACE_PREFIXES: &[&str] = &[
    "lo", "nebula", "docker", "podman", "virbr", "cni", "veth", "br-",
];

/// SUBAUDIT-C3 — smallest LAN prefix (largest range) the probe will
/// auto-scan. A `/16` home LAN (65 536 hosts — real on .13's wlan) makes
/// the full scan run effectively forever, hanging the probe cycle so the
/// inventory never updates (Discovered Hosts stuck at 0). Cap at `/22`
/// (≤1024 hosts); larger LANs are skipped from auto-scan — the mesh peers
/// still come from `mesh_targets`, and a specific subnet can be added via
/// `probe-targets.toml`.
pub const MIN_LAN_SCAN_PREFIX: u8 = 22;

/// SVC-VIEW-2 — known LAN service hosts the probe always scans as
/// single IPs, regardless of the [`MIN_LAN_SCAN_PREFIX`] auto-scan cap.
///
/// The auto-scan of a detected LAN is skipped when the prefix is wider
/// than `/22` (e.g. the `172.20.0.0/16` lab LAN — enumerating it would
/// hang the probe cycle), so service hosts on that LAN never get
/// scanned and their open ports (e.g. the Airsonic web UI on
/// `172.20.0.2:4040`) never reach `probe-inventory.json`. Listing those
/// hosts as individual targets gets them scanned cheaply without
/// re-enabling the oversized full-range scan. Operators can add more via
/// `probe-targets.toml`; these are the built-in defaults so the feature
/// works out of the box.
pub const KNOWN_LAN_SERVICE_HOSTS: &[&str] = &[
    // Airsonic / Subsonic media server (SVC-VIEW-2).
    "172.20.0.2",
];

/// Config file (under `~/.config/mde/`) of operator-arbitrary scan
/// targets — TOML `targets = ["host", "cidr", ...]`.
pub const ARBITRARY_TARGETS_FILE: &str = "probe-targets.toml";
/// Config file of do-not-scan exclusions — TOML `exclude = [...]`.
pub const DO_NOT_SCAN_FILE: &str = "probe-do-not-scan.toml";

/// The resolved scan scope: targets to scan + hosts/CIDRs to exclude.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TargetSet {
    /// Hosts / CIDRs nmap will scan.
    pub targets: Vec<String>,
    /// Hosts / CIDRs nmap will `--exclude` (Q9 escape hatch).
    pub excludes: Vec<String>,
}

/// Compute the network CIDR for an IPv4 `local` address + `prefixlen`
/// (e.g. `172.20.146.48` /16 → `172.20.0.0/16`). `None` for malformed
/// input or a prefix > 32. Pure.
#[must_use]
pub fn ipv4_network_cidr(local: &str, prefixlen: u8) -> Option<String> {
    if prefixlen > 32 {
        return None;
    }
    let octets: Vec<u8> = local.split('.').filter_map(|o| o.parse().ok()).collect();
    if octets.len() != 4 {
        return None;
    }
    let ip = u32::from_be_bytes([octets[0], octets[1], octets[2], octets[3]]);
    let mask = if prefixlen == 0 {
        0
    } else {
        u32::MAX << (32 - prefixlen)
    };
    let b = (ip & mask).to_be_bytes();
    Some(format!("{}.{}.{}.{}/{prefixlen}", b[0], b[1], b[2], b[3]))
}

/// Parse `ip -j addr` JSON into the deduped IPv4 network CIDRs of every
/// interface whose name doesn't start with one of `exclude_prefixes`.
/// Pure — the live wrapper [`detect_lan_cidrs`] feeds it real output.
#[must_use]
pub fn lan_cidrs_from_ip_json(json: &str, exclude_prefixes: &[&str]) -> Vec<String> {
    #[derive(serde::Deserialize)]
    struct Iface {
        ifname: String,
        #[serde(default)]
        addr_info: Vec<AddrInfo>,
    }
    #[derive(serde::Deserialize)]
    struct AddrInfo {
        family: String,
        local: String,
        #[serde(default)]
        prefixlen: u8,
    }
    let Ok(ifaces) = serde_json::from_str::<Vec<Iface>>(json) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for iface in ifaces {
        if exclude_prefixes.iter().any(|p| iface.ifname.starts_with(p)) {
            continue;
        }
        for a in iface.addr_info {
            if a.family != "inet" {
                continue;
            }
            // SUBAUDIT-C3 — skip oversized LANs (e.g. a /16) that would
            // hang the scan; a /22-or-smaller range is safe to enumerate.
            if a.prefixlen < MIN_LAN_SCAN_PREFIX {
                tracing::debug!(
                    target: "mackesd::probe_nmap",
                    iface = %iface.ifname,
                    cidr = %format!("{}/{}", a.local, a.prefixlen),
                    "LAN too large to auto-scan; skipped (mesh_targets still covered)",
                );
                continue;
            }
            if let Some(cidr) = ipv4_network_cidr(&a.local, a.prefixlen) {
                if !out.contains(&cidr) {
                    out.push(cidr);
                }
            }
        }
    }
    out
}

/// Detect the local LAN network CIDRs by shelling `ip -j addr`. Best-
/// effort: returns empty when `ip` is missing or errors.
#[must_use]
pub fn detect_lan_cidrs() -> Vec<String> {
    match Command::new("ip").args(["-j", "addr"]).output() {
        Ok(o) if o.status.success() => lan_cidrs_from_ip_json(
            &String::from_utf8_lossy(&o.stdout),
            DEFAULT_EXCLUDE_IFACE_PREFIXES,
        ),
        _ => Vec::new(),
    }
}

/// Read a TOML string-array under `key` from `path`. Best-effort:
/// missing file / parse error / wrong type → empty.
#[must_use]
pub fn read_toml_string_list(path: &Path, key: &str) -> Vec<String> {
    let Ok(body) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(val) = toml::from_str::<toml::Value>(&body) else {
        return Vec::new();
    };
    val.get(key)
        .and_then(toml::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Union `mesh` ∪ `lan` ∪ `arbitrary`, deduped, first-seen order
/// (mesh, then lan, then arbitrary). Pure.
#[must_use]
pub fn merge_targets(mesh: &[String], lan: &[String], arbitrary: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in mesh.iter().chain(lan).chain(arbitrary) {
        if !t.is_empty() && !out.contains(t) {
            out.push(t.clone());
        }
    }
    out
}

/// Resolve the full scan scope (Q5): mesh peers (from `workgroup_root`) ∪
/// enrolled-VM overlay IPs (SVC-VIEW-2, from the replicated compute-inventory
/// plane) ∪ detected LAN CIDRs ∪ operator-arbitrary targets (from
/// `~/.config/mde/probe-targets.toml`), minus the do-not-scan list
/// (`~/.config/mde/probe-do-not-scan.toml`, passed to nmap
/// `--exclude`). `home` locates the config files (injected for tests).
#[must_use]
pub fn resolve_targets(workgroup_root: &Path, home: &Path) -> TargetSet {
    // Mesh peers (per-peer nebula bundles) ∪ enrolled-VM overlay IPs (from the
    // replicated compute-inventory files). Both are overlay-IP targets, so they
    // share the first-class "mesh" ordering ahead of LAN + arbitrary; the VM
    // IPs are appended so a VM that also happens to be a full peer dedupes to
    // the peer entry (SVC-VIEW-2).
    let mut mesh = mesh_targets(workgroup_root);
    for vm_ip in vm_overlay_targets(workgroup_root) {
        if !mesh.contains(&vm_ip) {
            mesh.push(vm_ip);
        }
    }
    let lan = detect_lan_cidrs();
    let cfg = home.join(".config").join("mde");
    // SVC-VIEW-2 — known single-IP LAN service hosts (e.g. Airsonic on
    // 172.20.0.2) are always scanned, ahead of operator-arbitrary
    // targets; `merge_targets` dedupes, so an overlap is harmless.
    let mut arbitrary: Vec<String> = KNOWN_LAN_SERVICE_HOSTS
        .iter()
        .map(|h| (*h).to_owned())
        .collect();
    arbitrary.extend(read_toml_string_list(
        &cfg.join(ARBITRARY_TARGETS_FILE),
        "targets",
    ));
    let excludes = read_toml_string_list(&cfg.join(DO_NOT_SCAN_FILE), "exclude");
    TargetSet {
        targets: merge_targets(&mesh, &lan, &arbitrary),
        excludes,
    }
}

/// Serialize the inventory to the canonical JSON-array form (one Host
/// card per up host, each with Service children). Pretty-printed so a
/// human + a diff can read it.
#[must_use]
pub fn serialize_inventory(cards: &[Card]) -> String {
    serde_json::to_string_pretty(cards).unwrap_or_else(|_| "[]".to_owned())
}

/// Atomic-write `payload` to `path` (temp + rename), creating parent
/// dirs. Returns `true` when the content changed vs. what was on disk
/// (or the file was absent) — the signal the caller uses to decide
/// whether to publish `probe/changed`. A write error logs at warn +
/// returns `false`.
fn write_inventory_if_changed(path: &Path, payload: &str) -> bool {
    if std::fs::read_to_string(path).is_ok_and(|existing| existing == payload) {
        return false;
    }
    let Some(parent) = path.parent() else {
        return false;
    };
    if let Err(e) = std::fs::create_dir_all(parent) {
        tracing::warn!(target: "mackesd::probe_nmap", path = %path.display(), error = %e, "mkdir failed");
        return false;
    }
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, payload) {
        tracing::warn!(target: "mackesd::probe_nmap", error = %e, "inventory temp write failed");
        return false;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        tracing::warn!(target: "mackesd::probe_nmap", error = %e, "inventory rename failed");
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    true
}

/// Publish `probe/changed` to the Bus (best-effort, graceful-degrade —
/// same shell-out pattern as the gluster conflict + urgency_router
/// publishers).
fn publish_changed(host_count: usize) {
    let body = format!("probe inventory updated: {host_count} host(s)");
    match Command::new("mde-bus")
        .arg("publish")
        .arg(CHANGED_TOPIC)
        .arg("--priority")
        .arg("min")
        .arg("--body-flag")
        .arg(&body)
        .status()
    {
        Ok(s) if s.success() => {}
        Ok(s) => {
            tracing::warn!(target: "mackesd::probe_nmap", exit = ?s.code(), "mde-bus publish probe/changed non-zero")
        }
        Err(e) => {
            tracing::warn!(target: "mackesd::probe_nmap", error = %e, "could not spawn mde-bus (graceful-degrade)")
        }
    }
}

/// Core of one probe cycle against an already-resolved [`TargetSet`]:
/// scan, write the inventory, announce on the Bus if it changed.
/// Returns the number of host cards written. Taking the `TargetSet`
/// as input keeps this deterministic + testable (no env / LAN probe);
/// [`run_probe_cycle`] is the resolving wrapper the worker + CLI use.
pub fn run_probe_cycle_with(
    workgroup_root: &Path,
    self_node_id: &str,
    binary: &str,
    targets: &TargetSet,
    nse_dir: &str,
    deep: bool,
) -> usize {
    if targets.targets.is_empty() {
        return 0;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let profile = if deep { Profile::Deep } else { Profile::Fast };
    let cards = scan(
        binary,
        profile,
        &targets.targets,
        &targets.excludes,
        nse_dir,
        HostSource::Mesh,
        now,
    );
    let payload = serialize_inventory(&cards);
    let path = inventory_path(workgroup_root, self_node_id);
    if write_inventory_if_changed(&path, &payload) {
        publish_changed(cards.len());
        tracing::info!(
            target: "mackesd::probe_nmap",
            hosts = cards.len(),
            deep,
            path = %path.display(),
            "probe inventory updated + announced on probe/changed",
        );
    }
    cards.len()
}

/// Resolve the full scan scope (mesh ∪ LAN ∪ arbitrary, minus
/// do-not-scan) from `workgroup_root` + `home`, then run one cycle. Shared
/// by the scheduled worker + the `mackesd probe refresh` CLI.
pub fn run_probe_cycle(
    workgroup_root: &Path,
    self_node_id: &str,
    home: &Path,
    binary: &str,
    nse_dir: &str,
    deep: bool,
) -> usize {
    let targets = resolve_targets(workgroup_root, home);
    run_probe_cycle_with(
        workgroup_root,
        self_node_id,
        binary,
        &targets,
        nse_dir,
        deep,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::CardKind;

    // A faithful `nmap -sV -oX -` sample: one up host (10.42.0.5,
    // peer-a.mesh.mde) with two open service ports (Airsonic 4040,
    // Jellyfin 8096) + one closed port that must be skipped, plus a
    // second host that is `down` and must be skipped entirely.
    const NMAP_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE nmaprun>
<nmaprun scanner="nmap" args="nmap -sV -oX - 10.42.0.5" start="1700000000" version="7.94" xmloutputversion="1.05">
<scaninfo type="syn" protocol="tcp" numservices="16"/>
<host starttime="1700000001" endtime="1700000010">
<status state="up" reason="echo-reply" reason_ttl="64"/>
<address addr="10.42.0.5" addrtype="ipv4"/>
<hostnames>
<hostname name="peer-a.mesh.mde" type="PTR"/>
</hostnames>
<ports>
<port protocol="tcp" portid="4040">
<state state="open" reason="syn-ack" reason_ttl="64"/>
<service name="http" product="Airsonic" version="11.1" method="probed" conf="10"/>
</port>
<port protocol="tcp" portid="8096">
<state state="open" reason="syn-ack" reason_ttl="64"/>
<service name="http" product="Jellyfin" version="10.9" method="probed" conf="10"/>
</port>
<port protocol="tcp" portid="22">
<state state="closed" reason="conn-refused" reason_ttl="64"/>
<service name="ssh" method="table" conf="3"/>
</port>
</ports>
</host>
<host starttime="1700000001" endtime="1700000010">
<status state="down" reason="no-response"/>
<address addr="10.42.0.99" addrtype="ipv4"/>
</host>
<runstats>
<finished time="1700000010" elapsed="9.0" exit="success"/>
<hosts up="1" down="1" total="2"/>
</runstats>
</nmaprun>"#;

    #[test]
    fn fast_argv_is_rate_limited_and_xml_stdout() {
        let argv = fast_argv(&["10.42.0.5".to_owned()]);
        assert!(argv.contains(&"-T3".to_owned()), "polite timing present");
        assert!(!argv.contains(&"-T5".to_owned()), "never -T5");
        assert!(!argv.contains(&"-T4".to_owned()), "not aggressive -T4");
        // -oX - => XML to stdout.
        let ox = argv.iter().position(|a| a == "-oX").expect("-oX present");
        assert_eq!(argv[ox + 1], "-");
        // No -sV in the fast pass.
        assert!(!argv.contains(&"-sV".to_owned()));
        // Target is last.
        assert_eq!(argv.last().unwrap(), "10.42.0.5");
    }

    #[test]
    fn deep_argv_has_version_detection_and_nse_when_dir_given() {
        let argv = deep_argv(&["10.42.0.5".to_owned()], "/usr/share/mde/nmap");
        assert!(argv.contains(&"-sV".to_owned()));
        assert!(argv.contains(&"--version-all".to_owned()));
        assert!(argv.contains(&"-T3".to_owned()));
        assert!(!argv.contains(&"-T5".to_owned()));
        let s = argv.iter().position(|a| a == "--script").expect("--script");
        assert_eq!(argv[s + 1], "/usr/share/mde/nmap");
    }

    #[test]
    fn deep_argv_omits_script_when_nse_dir_empty() {
        let argv = deep_argv(&["10.42.0.5".to_owned()], "");
        assert!(!argv.contains(&"--script".to_owned()));
        assert!(argv.contains(&"-sV".to_owned()));
    }

    #[test]
    fn port_spec_lists_curated_ports() {
        let spec = port_spec();
        assert!(spec.contains("4040")); // Airsonic
        assert!(spec.contains("8096")); // Jellyfin
        assert!(spec.contains("22")); // SSH
        assert!(spec.starts_with("21,"));
    }

    #[test]
    fn parse_extracts_up_host_with_open_services() {
        let cards = parse_nmap_xml(NMAP_XML, HostSource::Mesh, 1700);
        // Only the up host (down host skipped).
        assert_eq!(cards.len(), 1);
        let host = &cards[0];
        assert_eq!(host.kind, CardKind::Host);
        let hf = host_facts(host).expect("host facts");
        assert_eq!(hf.ip, "10.42.0.5");
        assert_eq!(hf.hostname, "peer-a.mesh.mde");
        assert_eq!(hf.source, HostSource::Mesh);
        assert_eq!(hf.last_seen, 1700);
    }

    #[test]
    fn parse_skips_closed_ports_keeps_open() {
        let cards = parse_nmap_xml(NMAP_XML, HostSource::Mesh, 1);
        let host = &cards[0];
        // 4040 + 8096 open; 22 closed → 2 services.
        assert_eq!(host.children.len(), 2);
        let ports: Vec<u16> = host
            .children
            .iter()
            .filter_map(|c| service_facts(c).map(|f| f.port))
            .collect();
        assert!(ports.contains(&4040));
        assert!(ports.contains(&8096));
        assert!(!ports.contains(&22), "closed port skipped");
    }

    #[test]
    fn parse_captures_service_product_and_version() {
        let cards = parse_nmap_xml(NMAP_XML, HostSource::Lan, 1);
        let svc = cards[0]
            .children
            .iter()
            .find_map(|c| service_facts(c).filter(|f| f.port == 8096))
            .expect("jellyfin service");
        assert_eq!(svc.service_kind, "http");
        assert_eq!(svc.product, "Jellyfin");
        assert_eq!(svc.version, "10.9");
    }

    #[test]
    fn parse_returns_empty_for_garbage() {
        assert!(parse_nmap_xml("not xml at all", HostSource::Mesh, 0).is_empty());
        assert!(parse_nmap_xml("", HostSource::Mesh, 0).is_empty());
    }

    #[test]
    fn scan_with_missing_binary_returns_empty() {
        // The graceful-degrade path: nmap not installed.
        let cards = scan(
            "/nonexistent/nmap-xyz",
            Profile::Fast,
            &["10.42.0.5".to_owned()],
            &[],
            "",
            HostSource::Mesh,
            0,
        );
        assert!(cards.is_empty());
    }

    #[test]
    fn scan_with_empty_targets_is_noop() {
        let cards = scan(
            DEFAULT_NMAP_BINARY,
            Profile::Deep,
            &[],
            &[],
            "",
            HostSource::Mesh,
            0,
        );
        assert!(cards.is_empty());
    }

    // ── Cycle orchestration (MESH-PROBE-4) ──────────────────────────

    fn tmp_root(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("mde-probecyc-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn inventory_path_mirrors_peer_layout() {
        assert_eq!(
            inventory_path(Path::new("/qnm"), "peer-a"),
            Path::new("/qnm/peer-a/mackesd/probe-inventory.json")
        );
    }

    #[test]
    fn mesh_targets_reads_peer_overlay_ips() {
        let root = tmp_root("targets");
        for (peer, ip) in [("peer-a", "10.42.0.5"), ("peer-b", "10.42.0.6")] {
            let dir = root.join(peer).join("mackesd");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("nebula-bundle.json"),
                format!(r#"{{"overlay_ip":"{ip}"}}"#),
            )
            .unwrap();
        }
        let mut t = mesh_targets(&root);
        t.sort();
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(t, vec!["10.42.0.5".to_string(), "10.42.0.6".to_string()]);
    }

    // SVC-VIEW-2 — seed `<root>/<host>/compute-inventory.json` with the given
    // VMs (name, overlay_ip) for the VM-target-resolution tests. Mirrors the
    // doc `compute_registry::write_shared_inventory` writes.
    fn seed_compute_inventory(root: &Path, host: &str, vms: &[(&str, &str)]) {
        let dir = root.join(host);
        std::fs::create_dir_all(&dir).unwrap();
        let vm_json: Vec<String> = vms
            .iter()
            .map(|(name, ip)| {
                format!(
                    r#"{{"id":"u-{name}","name":"{name}","state":"running","cpu_pct":0.0,"ram_mb":2048,"disk_path":"","nebula_ip":"{ip}","meshfs_available":false}}"#
                )
            })
            .collect();
        let body = format!(
            r#"{{"peer":"10.42.0.3","hostname":"{host}","vms":[{}],"containers":[]}}"#,
            vm_json.join(",")
        );
        std::fs::write(dir.join("compute-inventory.json"), body).unwrap();
    }

    #[test]
    fn vm_overlay_targets_harvests_enrolled_vm_ips() {
        // The VM-internal-services path: an enrolled VM's overlay IP lives only
        // in the per-host compute inventory, not the nebula bundles, so it must
        // come from `vm_overlay_targets` to ever be scanned.
        let root = tmp_root("vm-targets");
        seed_compute_inventory(
            &root,
            "fedora",
            &[("MDE-KVM-1", "10.42.1.20"), ("MDE-VM-2", "10.42.1.21")],
        );
        // A VM with no overlay IP yet (no sidecar) must be skipped.
        seed_compute_inventory(&root, "host-b", &[("unenrolled", "")]);
        // A malformed inventory must be skipped (fail-open), not abort.
        let bad = root.join("host-c");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("compute-inventory.json"), "{ not json").unwrap();
        let mut got = vm_overlay_targets(&root);
        got.sort();
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(
            got,
            vec!["10.42.1.20".to_string(), "10.42.1.21".to_string()],
            "both enrolled VM overlay IPs harvested; empty + malformed skipped"
        );
    }

    #[test]
    fn vm_overlay_targets_empty_for_missing_root() {
        assert!(vm_overlay_targets(Path::new("/nonexistent/qnm/xyz")).is_empty());
    }

    #[test]
    fn vm_overlay_targets_dedupes_same_ip_across_hosts() {
        let root = tmp_root("vm-dedup");
        seed_compute_inventory(&root, "host-a", &[("vmX", "10.42.1.30")]);
        seed_compute_inventory(&root, "host-b", &[("vmX-dup", "10.42.1.30")]);
        let got = vm_overlay_targets(&root);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(
            got,
            vec!["10.42.1.30".to_string()],
            "duplicate IP collapsed"
        );
    }

    #[test]
    fn write_inventory_detects_change_then_noop() {
        let root = tmp_root("write");
        let path = inventory_path(&root, "self");
        assert!(write_inventory_if_changed(&path, "[]"), "absent → changed");
        assert!(!write_inventory_if_changed(&path, "[]"), "same → no change");
        assert!(
            write_inventory_if_changed(&path, "[{\"x\":1}]"),
            "diff → changed"
        );
        let leftover = path.with_extension("json.tmp").exists();
        let body = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_dir_all(&root);
        assert!(!leftover, "no temp file left behind");
        assert_eq!(body, "[{\"x\":1}]");
    }

    #[test]
    fn serialize_inventory_empty_is_json_array() {
        assert_eq!(serialize_inventory(&[]), "[]");
    }

    #[test]
    fn run_cycle_with_no_targets_writes_nothing() {
        let root = tmp_root("notargets");
        // Empty TargetSet → no scan, no inventory (deterministic; the
        // resolving wrapper would probe the real LAN, so the core
        // takes the resolved set as input).
        let n = run_probe_cycle_with(
            &root,
            "self",
            "/nonexistent/nmap",
            &TargetSet::default(),
            "",
            true,
        );
        let existed = inventory_path(&root, "self").exists();
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(n, 0);
        assert!(!existed, "no inventory when there are no targets");
    }

    // ── Target resolver (MESH-PROBE-5) ──────────────────────────────

    #[test]
    fn ipv4_network_cidr_masks_to_network() {
        assert_eq!(
            ipv4_network_cidr("172.20.146.48", 16).as_deref(),
            Some("172.20.0.0/16")
        );
        assert_eq!(
            ipv4_network_cidr("192.168.1.42", 24).as_deref(),
            Some("192.168.1.0/24")
        );
        assert_eq!(
            ipv4_network_cidr("10.0.0.5", 8).as_deref(),
            Some("10.0.0.0/8")
        );
        assert_eq!(
            ipv4_network_cidr("1.2.3.4", 33),
            None,
            "prefix > 32 rejected"
        );
        assert_eq!(ipv4_network_cidr("not.an.ip", 24), None);
    }

    #[test]
    fn lan_cidrs_skips_loopback_and_overlay_keeps_lan() {
        // Faithful `ip -j addr` shape: lo + nebula1 excluded, the
        // physical iface's /16 kept as a network CIDR.
        let json = r#"[
            {"ifname":"lo","addr_info":[{"family":"inet","local":"127.0.0.1","prefixlen":8}]},
            {"ifname":"nebula1","addr_info":[{"family":"inet","local":"10.42.0.5","prefixlen":16}]},
            {"ifname":"wlp2s0","addr_info":[
                {"family":"inet","local":"192.168.1.50","prefixlen":24},
                {"family":"inet6","local":"fe80::1","prefixlen":64}
            ]}
        ]"#;
        let cidrs = lan_cidrs_from_ip_json(json, DEFAULT_EXCLUDE_IFACE_PREFIXES);
        assert_eq!(cidrs, vec!["192.168.1.0/24".to_string()]);
    }

    #[test]
    fn lan_cidrs_empty_for_garbage() {
        assert!(lan_cidrs_from_ip_json("not json", DEFAULT_EXCLUDE_IFACE_PREFIXES).is_empty());
    }

    #[test]
    fn merge_targets_unions_and_dedupes_in_order() {
        let mesh = vec!["10.42.0.5".to_string(), "10.42.0.6".to_string()];
        let lan = vec!["192.168.1.0/24".to_string(), "10.42.0.5".to_string()];
        let arb = vec!["8.8.8.8".to_string(), "192.168.1.0/24".to_string()];
        assert_eq!(
            merge_targets(&mesh, &lan, &arb),
            vec![
                "10.42.0.5".to_string(),
                "10.42.0.6".to_string(),
                "192.168.1.0/24".to_string(),
                "8.8.8.8".to_string(),
            ]
        );
    }

    #[test]
    fn read_toml_string_list_extracts_array() {
        let root = tmp_root("toml");
        let path = root.join("probe-targets.toml");
        std::fs::write(&path, "targets = [\"10.0.0.1\", \"192.168.0.0/24\"]\n").unwrap();
        let got = read_toml_string_list(&path, "targets");
        let missing = read_toml_string_list(&path, "exclude");
        let absent = read_toml_string_list(&root.join("nope.toml"), "targets");
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(
            got,
            vec!["10.0.0.1".to_string(), "192.168.0.0/24".to_string()]
        );
        assert!(missing.is_empty(), "absent key → empty");
        assert!(absent.is_empty(), "absent file → empty");
    }

    // ── Read API (MESH-PROBE-6) ─────────────────────────────────────

    // Seed `<root>/<peer>/mackesd/probe-inventory.json` with a host
    // card carrying the given services, for the read-API tests.
    fn seed_inventory(root: &Path, peer: &str, ip: &str, services: &[(&str, u16)]) {
        let dir = root.join(peer).join("mackesd");
        std::fs::create_dir_all(&dir).unwrap();
        let svc_cards: Vec<Card> = services
            .iter()
            .map(|(kind, port)| {
                service_card(
                    &ServiceFacts {
                        port: *port,
                        service_kind: (*kind).to_owned(),
                        product: String::new(),
                        version: String::new(),
                        fingerprint: String::new(),
                    },
                    1,
                )
            })
            .collect();
        let host = host_card(
            &HostFacts {
                ip: ip.to_owned(),
                hostname: peer.to_owned(),
                source: HostSource::Mesh,
                trust_state: String::new(),
                last_seen: 1,
            },
            svc_cards,
            1,
        );
        std::fs::write(dir.join(INVENTORY_FILENAME), serialize_inventory(&[host])).unwrap();
    }

    #[test]
    fn inventory_unions_all_peer_files() {
        let root = tmp_root("inv-union");
        seed_inventory(&root, "peer-a", "10.42.0.5", &[("ssh", 22)]);
        seed_inventory(&root, "peer-b", "10.42.0.6", &[("jellyfin", 8096)]);
        // A malformed file must be skipped (fail-open), not abort.
        let bad = root.join("peer-c").join("mackesd");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join(INVENTORY_FILENAME), "{ not an array").unwrap();
        let inv = inventory(&root);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(inv.len(), 2, "two valid host cards, malformed skipped");
    }

    #[test]
    fn inventory_empty_for_missing_root() {
        assert!(inventory(Path::new("/nonexistent/xyz")).is_empty());
    }

    #[test]
    fn peers_with_service_filters_by_kind() {
        let root = tmp_root("inv-svc");
        seed_inventory(
            &root,
            "peer-a",
            "10.42.0.5",
            &[("ssh", 22), ("jellyfin", 8096)],
        );
        seed_inventory(&root, "peer-b", "10.42.0.6", &[("airsonic", 4040)]);
        let jelly = peers_with_service(&root, "jellyfin");
        let airs = peers_with_service(&root, "AIRSONIC"); // case-insensitive
        let none = peers_with_service(&root, "redis");
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(jelly.len(), 1);
        assert_eq!(jelly[0].host.ip, "10.42.0.5");
        assert_eq!(jelly[0].service.port, 8096);
        assert_eq!(airs.len(), 1);
        assert_eq!(airs[0].host.ip, "10.42.0.6");
        assert!(none.is_empty());
    }

    #[test]
    fn inventory_fingerprint_changes_on_write() {
        let root = tmp_root("inv-fp");
        seed_inventory(&root, "peer-a", "10.42.0.5", &[("ssh", 22)]);
        let fp1 = inventory_fingerprint(&root);
        // Adding a peer changes the fingerprint.
        seed_inventory(&root, "peer-b", "10.42.0.6", &[("ssh", 22)]);
        let fp2 = inventory_fingerprint(&root);
        let _ = std::fs::remove_dir_all(&root);
        assert_ne!(
            fp1, fp2,
            "fingerprint changes when a peer's inventory appears"
        );
        assert_ne!(fp1, 0);
    }

    #[test]
    fn resolve_targets_merges_arbitrary_and_reads_excludes() {
        // workgroup_root with one peer + a home with arbitrary + exclude
        // config. LAN detection is environment-dependent so we only
        // assert the mesh + arbitrary + exclude pieces are present.
        let root = tmp_root("resolve");
        let qnm = root.join("qnm");
        std::fs::create_dir_all(qnm.join("peerX").join("mackesd")).unwrap();
        std::fs::write(
            qnm.join("peerX").join("mackesd").join("nebula-bundle.json"),
            r#"{"overlay_ip":"10.42.0.9"}"#,
        )
        .unwrap();
        let home = root.join("home");
        let cfg = home.join(".config").join("mde");
        std::fs::create_dir_all(&cfg).unwrap();
        std::fs::write(
            cfg.join(ARBITRARY_TARGETS_FILE),
            "targets = [\"8.8.8.8\"]\n",
        )
        .unwrap();
        std::fs::write(cfg.join(DO_NOT_SCAN_FILE), "exclude = [\"10.42.0.99\"]\n").unwrap();
        let ts = resolve_targets(&qnm, &home);
        let _ = std::fs::remove_dir_all(&root);
        assert!(
            ts.targets.contains(&"10.42.0.9".to_string()),
            "mesh peer included"
        );
        assert!(
            ts.targets.contains(&"8.8.8.8".to_string()),
            "arbitrary included"
        );
        assert_eq!(
            ts.excludes,
            vec!["10.42.0.99".to_string()],
            "do-not-scan loaded"
        );
        // SVC-VIEW-2 — built-in known LAN service host(s) are always in
        // scope, even with no operator probe-targets.toml entry.
        for host in KNOWN_LAN_SERVICE_HOSTS {
            assert!(
                ts.targets.contains(&(*host).to_string()),
                "known LAN service host {host} included by default"
            );
        }
    }

    #[test]
    fn resolve_targets_includes_known_hosts_without_arbitrary_config() {
        // SVC-VIEW-2 — with no probe-targets.toml at all, the Airsonic
        // host (172.20.0.2) is still scanned so its open ports surface.
        // Hermetic: empty mesh + no operator config; `detect_lan_cidrs`
        // emits network-CIDR strings (e.g. "172.20.0.0/22"), never a
        // bare host IP, so a `172.20.0.2` target can ONLY come from the
        // built-in `KNOWN_LAN_SERVICE_HOSTS` constant.
        let root = tmp_root("resolve-known");
        let qnm = root.join("qnm");
        std::fs::create_dir_all(&qnm).unwrap();
        let home = root.join("home"); // no .config/mde — nothing seeded
        let ts = resolve_targets(&qnm, &home);
        let _ = std::fs::remove_dir_all(&root);
        assert!(
            ts.targets.contains(&"172.20.0.2".to_string()),
            "Airsonic host scanned even with no operator config"
        );
        assert!(
            KNOWN_LAN_SERVICE_HOSTS.contains(&"172.20.0.2"),
            "Airsonic host is a built-in known LAN service host"
        );
    }

    #[test]
    fn resolve_targets_includes_enrolled_vm_overlay_ip() {
        // SVC-VIEW-2 — an enrolled VM's overlay IP (from the replicated
        // compute-inventory plane) must reach the resolved scan scope so the
        // probe scans the VM's services. Hermetic: the VM IP `10.42.1.20` is in
        // the 10.x overlay range and seeded ONLY via the compute inventory —
        // `detect_lan_cidrs`/mesh bundles can't produce it — so its presence
        // proves the `vm_overlay_targets` wiring.
        let root = tmp_root("resolve-vm");
        let qnm = root.join("qnm");
        std::fs::create_dir_all(&qnm).unwrap();
        seed_compute_inventory(&qnm, "fedora", &[("MDE-KVM-1", "10.42.1.20")]);
        let home = root.join("home"); // no operator config
        let ts = resolve_targets(&qnm, &home);
        let _ = std::fs::remove_dir_all(&root);
        assert!(
            ts.targets.contains(&"10.42.1.20".to_string()),
            "enrolled VM overlay IP scanned for its internal services"
        );
    }
}
