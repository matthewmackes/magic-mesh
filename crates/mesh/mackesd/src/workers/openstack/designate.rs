//! QC-17 (CONSTRUCT-CLOUD) — **Designate replaces naming** (design Q46,
//! `AI_GOVERNANCE.md` §1: "Designate becomes the mesh name service, fed —
//! and re-seedable — by the etcd peer directory").
//!
//! Three legs, all peer-directory-authoritative:
//!
//! 1. **The zone-record feed** ([`derive_zone_records`]) — a pure fold from
//!    the etcd peer directory (+ the Nova instance roster) to the `mesh.`
//!    zone's A-records: every **node** (`<host>.mesh.`), every **instance**
//!    (`<name>.cloud.mesh.`), every **API service** as a multi-A set across
//!    all API nodes (Q22 — APIs on every node), and the **leader-hosted**
//!    names (`mariadb.mesh.`, `ovn-nb.mesh.`, `ovn-sb.mesh.`) pinned to the
//!    current leader so a failover moves the name, not the config (Q15).
//! 2. **The re-seed path** ([`plan_reseed`] → [`render_designate_feed`]) —
//!    the design's own risk note ("Designate as THE name service puts DNS on
//!    the cloud's availability; the peer directory remains the source that
//!    can re-seed it") made executable: an idempotent, drift-correcting
//!    `openstack zone`/`recordset` script the QC-10 bootstrap seed runs.
//!    Against an **empty** Designate it rebuilds the zones from scratch;
//!    against a live one it converges drifted record sets.
//! 3. **The pool topology** ([`render_designate_pools`]) — every node's bind9
//!    is a pool target / nameserver / mdns master (no fixed center, §1),
//!    derived from the same peer directory, applied by the feed via
//!    `designate-manage pool update`.
//!
//! The live legs stay honest (§7): the peer read rides the injectable
//! [`PeerDirectorySource`] seam (production: the etcd-first
//! [`crate::substrate::peers::read_directory`] fold), and the **live DNS
//! resolve check** ([`live_resolve`]) gates with a typed reason when a mesh
//! name doesn't actually resolve on this box — the rendered feed carries the
//! honest gate status in its provenance header, never a claimed-working
//! naming plane.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, ToSocketAddrs as _};
use std::path::{Path, PathBuf};

use super::catalog::ServiceKind;
use super::config_render::{
    write_atomic, RenderError, DB_MESH_NAME, DESIGNATE_FEED_FILE, DESIGNATE_MDNS_PORT,
    DESIGNATE_POOLS_FILE, DESIGNATE_RNDC_KEY_PATH, DNS_PORT, OVN_NB_MESH_NAME, OVN_SB_MESH_NAME,
    RABBIT_MESH_NAME, RNDC_PORT,
};
use super::verbs::CloudInstance;

/// The one mesh zone (absolute) — the flat `<name>.mesh` namespace the whole
/// platform already resolves (W75), now served by Designate (Q46).
pub const MESH_ZONE: &str = "mesh.";

/// The zone's SOA contact — Designate requires one at zone create.
pub const ZONE_EMAIL: &str = "hostmaster@mesh";

/// The sub-label instance records live under (`<name>.cloud.mesh.`), so a
/// tenant instance name can never shadow a node's `<host>.mesh.` record.
pub const INSTANCE_SUBDOMAIN: &str = "cloud";

/// Designate's well-known default-pool id — pinned so a
/// `designate-manage pool update` **replaces** the stock pool with the
/// peer-directory-fed topology instead of adding a second pool.
pub const DEFAULT_POOL_ID: &str = "794ccc2c-d751-44fe-b57f-8894c9f5c842";

/// What a zone record names (provenance — which peer-directory leg fed it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RecordKind {
    /// A mesh node — `<host>.mesh.` → its overlay IP.
    Node,
    /// A cloud instance — `<name>.cloud.mesh.` → its provider-net address.
    Instance,
    /// A service name — an API's multi-A set (Q22) or a leader-hosted name.
    Service,
}

/// One derived A-record: `fqdn` (absolute, under [`MESH_ZONE`]) → `ip`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ZoneRecord {
    /// The absolute record name (trailing dot), e.g. `eagle.mesh.`.
    pub fqdn: String,
    /// The IPv4 address the record answers.
    pub ip: String,
    /// Which peer-directory leg derived it.
    pub kind: RecordKind,
}

impl ZoneRecord {
    fn new(fqdn: String, ip: &str, kind: RecordKind) -> Self {
        Self {
            fqdn,
            ip: ip.to_string(),
            kind,
        }
    }
}

/// A DNS-safe label from a free-form name: lowercased, `[a-z0-9-]` kept,
/// every other char folded to `-`, trimmed of leading/trailing `-`. `None`
/// when nothing survives (a record is skipped honestly, never half-named).
fn dns_label(name: &str) -> Option<String> {
    let label: String = name
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let label = label.trim_matches('-').to_string();
    if label.is_empty() || label.len() > 63 {
        None
    } else {
        Some(label)
    }
}

/// The pure peer-directory → zone-record fold (Q46: nodes, instances, and
/// services get records, resolvable mesh-wide).
///
/// `peers` is `(hostname, overlay_ip)` off the etcd peer directory (empty IPs
/// are skipped — never a half record, the `mesh_dns` discipline); `leader` is
/// the host currently holding the `/mesh/leader` lease (`None` ⇒ the
/// leader-hosted names are honestly absent, not guessed); `instances` is
/// `(name, ip)` off the Nova roster ([`instance_pairs`]). Output is sorted +
/// deduped, so the derived feed is stable and idempotent.
#[must_use]
pub fn derive_zone_records(
    peers: &[(String, String)],
    leader: Option<&str>,
    instances: &[(String, String)],
) -> Vec<ZoneRecord> {
    let live: Vec<(&str, &str)> = peers
        .iter()
        .filter(|(_, ip)| !ip.is_empty())
        .map(|(h, ip)| (h.as_str(), ip.as_str()))
        .collect();
    let mut out = Vec::new();

    // Node records — <host>.mesh. (skip a hostname that can't label).
    for (host, ip) in &live {
        if let Some(label) = dns_label(host) {
            out.push(ZoneRecord::new(
                format!("{label}.{MESH_ZONE}"),
                ip,
                RecordKind::Node,
            ));
        }
    }

    // API service names — a multi-A set across every node (Q22: APIs on every
    // node, so any live peer answers), plus the every-node RabbitMQ cluster.
    for kind in ServiceKind::ALL {
        if let Some(name) = kind.mesh_dns_name() {
            for (_, ip) in &live {
                out.push(ZoneRecord::new(format!("{name}."), ip, RecordKind::Service));
            }
        }
    }
    for (_, ip) in &live {
        out.push(ZoneRecord::new(
            format!("{RABBIT_MESH_NAME}."),
            ip,
            RecordKind::Service,
        ));
    }

    // Leader-hosted names (Q15) — pinned to the current leader so a failover
    // moves the record, not the rendered configs. Honestly absent when the
    // leader is unknown or not in the directory yet.
    if let Some(leader_ip) =
        leader.and_then(|l| live.iter().find(|(host, _)| *host == l).map(|(_, ip)| *ip))
    {
        for name in [DB_MESH_NAME, OVN_NB_MESH_NAME, OVN_SB_MESH_NAME] {
            out.push(ZoneRecord::new(
                format!("{name}."),
                leader_ip,
                RecordKind::Service,
            ));
        }
    }

    // Instance records — <name>.cloud.mesh. (Q46: instances resolve
    // mesh-wide; namespaced so they can't shadow node records).
    for (name, ip) in instances {
        if ip.is_empty() {
            continue;
        }
        if let Some(label) = dns_label(name) {
            out.push(ZoneRecord::new(
                format!("{label}.{INSTANCE_SUBDOMAIN}.{MESH_ZONE}"),
                ip,
                RecordKind::Instance,
            ));
        }
    }

    out.sort();
    out.dedup();
    out
}

/// Extract `(name, ipv4)` pairs from a Nova roster — the instance leg of the
/// zone feed.
///
/// The `Networks` column renders like `mesh=10.42.100.7` (possibly several
/// nets / v6 addresses); the first parseable IPv4 wins. Instances with no
/// address yet are skipped (never a half record).
#[must_use]
pub fn instance_pairs(instances: &[CloudInstance]) -> Vec<(String, String)> {
    instances
        .iter()
        .filter_map(|i| {
            let nets = i.networks.as_deref()?;
            let ip = nets
                .split(|c: char| c == '=' || c == ',' || c == ';' || c.is_whitespace())
                .map(|tok| tok.trim_matches(|c: char| !c.is_ascii_hexdigit() && c != '.'))
                .find(|tok| tok.parse::<Ipv4Addr>().is_ok())?;
            Some((i.name.clone(), ip.to_string()))
        })
        .collect()
}

/// The re-seed plan: the ordered `openstack` argv sequence that rebuilds the
/// mesh zone **from scratch** off the derived records.
///
/// Zone create first, then one `recordset create` per name carrying its whole
/// A-set (a service name's multi-A answers in one recordset). Pure + pinned
/// by tests so the command surface can't drift (the argv-builder discipline
/// `verbs` uses).
#[must_use]
pub fn plan_reseed(records: &[ZoneRecord]) -> Vec<Vec<String>> {
    let mut plan = vec![vec![
        "zone".to_string(),
        "create".to_string(),
        "--email".to_string(),
        ZONE_EMAIL.to_string(),
        MESH_ZONE.to_string(),
    ]];
    // Group each fqdn's ips (records arrive sorted, so the grouping — and the
    // emitted plan — is deterministic).
    let mut sets: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for r in records {
        let ips = sets.entry(r.fqdn.as_str()).or_default();
        if !ips.contains(&r.ip.as_str()) {
            ips.push(r.ip.as_str());
        }
    }
    for (fqdn, ips) in sets {
        let mut argv = vec![
            "recordset".to_string(),
            "create".to_string(),
            MESH_ZONE.to_string(),
            fqdn.to_string(),
            "--type".to_string(),
            "A".to_string(),
        ];
        for ip in ips {
            argv.push("--record".to_string());
            argv.push(ip.to_string());
        }
        plan.push(argv);
    }
    plan
}

/// Render the QC-17 **zone feed / re-seed script** to
/// `<config_root>/bootstrap/designate-feed.sh` (the QC-10 seed runs it when
/// present).
///
/// Idempotent + drift-correcting: the zone create is show-guarded; each
/// record set is `create || set`, so a fresh Designate is seeded from
/// scratch (the Q46 re-seed path) and a drifted live one converges to the
/// peer directory. The pool topology is applied first when rendered
/// ([`render_designate_pools`]). `resolve_note` is the honest
/// [`live_resolve`] gate status stamped into the provenance header.
///
/// # Errors
/// A [`RenderError::Io`] if the script (or its parent dir) can't be written.
pub fn render_designate_feed(
    config_root: &Path,
    release: &str,
    records: &[ZoneRecord],
    resolve_note: &str,
) -> Result<PathBuf, RenderError> {
    use std::fmt::Write as _;

    let mut body = String::new();
    body.push_str("#!/bin/sh\n");
    let _ = writeln!(
        body,
        "# rendered by mackesd openstack worker (QC-17) — kolla release {release}"
    );
    body.push_str(
        "# Designate zone feed + re-seed (design Q46): the etcd peer directory is\n\
         # the source of truth — run against an EMPTY Designate this rebuilds the\n\
         # mesh zone from scratch; run against a live one it converges drifted\n\
         # record sets. Idempotent (guarded create / drift-correcting set).\n",
    );
    let _ = writeln!(body, "# live-resolve gate: {resolve_note}");
    body.push_str("set -eu\n\n");

    // Pool first — the peer-directory-fed topology (every node's bind9 a
    // target; no fixed center). Applied inside the central container, which
    // owns designate-manage; skipped honestly when the pool file isn't there.
    let _ = writeln!(
        body,
        "# Peer-directory-fed pool topology (every node's bind9; no fixed center).\n\
         POOLS=\"$(dirname \"$0\")/{DESIGNATE_POOLS_FILE}\"\n\
         if [ -f \"$POOLS\" ]; then\n  \
         podman cp \"$POOLS\" designate_central:/tmp/{DESIGNATE_POOLS_FILE}\n  \
         podman exec designate_central designate-manage pool update \\\n    \
         --file /tmp/{DESIGNATE_POOLS_FILE}\n\
         fi\n"
    );

    body.push_str(
        "ensure_zone() {\n  \
         openstack zone show \"$1\" >/dev/null 2>&1 && return 0\n  \
         openstack zone create --email \"$2\" \"$1\"\n}\n\
         ensure_rrset() {  # <fqdn> <ip>...\n  \
         fqdn=\"$1\"; shift\n  \
         args=\"\"\n  \
         for ip in \"$@\"; do args=\"$args --record $ip\"; done\n  \
         # shellcheck disable=SC2086 — the args are our own --record pairs\n  \
         openstack recordset create ",
    );
    let _ = write!(
        body,
        "{MESH_ZONE} \"$fqdn\" --type A $args >/dev/null 2>&1 \\\n    \
         || openstack recordset set {MESH_ZONE} \"$fqdn\" $args\n}}\n\n"
    );
    let _ = writeln!(body, "ensure_zone {MESH_ZONE} {ZONE_EMAIL}");

    // One ensure per fqdn, carrying its whole (deterministic) A-set.
    let mut sets: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for r in records {
        let ips = sets.entry(r.fqdn.as_str()).or_default();
        if !ips.contains(&r.ip.as_str()) {
            ips.push(r.ip.as_str());
        }
    }
    for (fqdn, ips) in sets {
        let _ = writeln!(body, "ensure_rrset {fqdn} {}", ips.join(" "));
    }

    let path = config_root.join("bootstrap").join(DESIGNATE_FEED_FILE);
    write_atomic(&path, &body).map_err(|source| RenderError::Io {
        service: "designate-feed".to_string(),
        path: path.display().to_string(),
        source,
    })?;
    Ok(path)
}

/// Render the peer-directory-fed **pool topology** to
/// `<config_root>/bootstrap/designate-pools.yaml`.
///
/// Every live node's bind9 is a pool target / nameserver, every node's
/// mini-DNS a master (no fixed center, §1). Pinned to Designate's default
/// pool id so `pool update` replaces the stock pool. The feed script applies
/// it.
///
/// # Errors
/// A [`RenderError::Io`] if the file (or its parent dir) can't be written.
pub fn render_designate_pools(
    config_root: &Path,
    release: &str,
    peers: &[(String, String)],
) -> Result<PathBuf, RenderError> {
    use std::fmt::Write as _;

    let live: Vec<(&str, &str)> = peers
        .iter()
        .filter(|(_, ip)| !ip.is_empty())
        .map(|(h, ip)| (h.as_str(), ip.as_str()))
        .collect();

    let mut body = String::new();
    let _ = writeln!(
        body,
        "# rendered by mackesd openstack worker (QC-17) — kolla release {release}"
    );
    body.push_str(
        "# Peer-directory-fed Designate pool (design Q46, §1 no-fixed-center):\n\
         # every live node's bind9 serves the mesh zone; re-rendered as the\n\
         # directory changes; applied by designate-feed.sh (pool update).\n",
    );
    body.push_str("- name: default\n");
    let _ = writeln!(body, "  id: {DEFAULT_POOL_ID}");
    body.push_str("  description: MCNF mesh pool (QC-17, peer-directory-fed)\n");
    body.push_str("  attributes: {}\n");
    body.push_str("  ns_records:\n");
    for (i, (host, _)) in live.iter().enumerate() {
        let label = dns_label(host).unwrap_or_else(|| format!("node-{i}"));
        let _ = writeln!(body, "    - hostname: {label}.{MESH_ZONE}");
        let _ = writeln!(body, "      priority: {}", i + 1);
    }
    body.push_str("  nameservers:\n");
    for (_, ip) in &live {
        let _ = writeln!(body, "    - host: {ip}\n      port: {DNS_PORT}");
    }
    body.push_str("  targets:\n");
    for (host, ip) in &live {
        let _ = writeln!(body, "    - type: bind9");
        let _ = writeln!(body, "      description: bind9 on {host}");
        body.push_str("      masters:\n");
        for (_, master_ip) in &live {
            let _ = writeln!(
                body,
                "        - host: {master_ip}\n          port: {DESIGNATE_MDNS_PORT}"
            );
        }
        body.push_str("      options:\n");
        let _ = writeln!(body, "        host: {ip}");
        let _ = writeln!(body, "        port: {DNS_PORT}");
        let _ = writeln!(body, "        rndc_host: {ip}");
        let _ = writeln!(body, "        rndc_port: {RNDC_PORT}");
        let _ = writeln!(body, "        rndc_key_file: {DESIGNATE_RNDC_KEY_PATH}");
    }

    let path = config_root.join("bootstrap").join(DESIGNATE_POOLS_FILE);
    write_atomic(&path, &body).map_err(|source| RenderError::Io {
        service: "designate-pools".to_string(),
        path: path.display().to_string(),
        source,
    })?;
    Ok(path)
}

/// The honest live-resolve status (§7 — never a claimed-working naming plane).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveOutcome {
    /// The name resolved on this box — the answering addresses.
    Resolved {
        /// The resolved addresses.
        ips: Vec<String>,
    },
    /// It didn't — the typed gate reason (the naming plane isn't serving this
    /// name here yet, or the resolver path isn't wired to the mesh zone).
    Gated {
        /// Why the live resolve check failed.
        reason: String,
    },
}

/// Live-check that `fqdn` actually resolves on this box — the QC-17 honest
/// gate.
///
/// A real `getaddrinfo` through the system resolver path — the same path
/// every mesh client takes — never a fabricated "DNS works". A trailing zone
/// dot is accepted and trimmed.
#[must_use]
pub fn live_resolve(fqdn: &str) -> ResolveOutcome {
    let name = fqdn.trim_end_matches('.');
    match (name, DNS_PORT).to_socket_addrs() {
        Ok(addrs) => {
            let ips: Vec<String> = addrs.map(|a| a.ip().to_string()).collect();
            if ips.is_empty() {
                ResolveOutcome::Gated {
                    reason: format!("{name}: the resolver answered an empty address set"),
                }
            } else {
                ResolveOutcome::Resolved { ips }
            }
        }
        Err(e) => ResolveOutcome::Gated {
            reason: format!(
                "{name} does not resolve on this box yet ({e}) — the Designate plane \
                 (or the resolver path to it) isn't serving the mesh zone here"
            ),
        },
    }
}

/// The one-line provenance note [`render_designate_feed`] stamps from a
/// [`live_resolve`] outcome.
#[must_use]
pub fn resolve_note(outcome: &ResolveOutcome) -> String {
    match outcome {
        ResolveOutcome::Resolved { ips } => format!("resolved → {}", ips.join(", ")),
        ResolveOutcome::Gated { reason } => format!("GATED — {reason}"),
    }
}

// ─────────────────────── the peer-directory seam ───────────────────────

/// The injectable peer-directory read the zone feed derives from. Production
/// wires [`MeshPeerDirectory`]; tests wire a fixture directory so the whole
/// derivation runs without etcd.
pub trait PeerDirectorySource {
    /// The live `(hostname, overlay_ip)` pairs. Peers with no published
    /// overlay IP yet are omitted by the caller's derivation (never a half
    /// record).
    fn pairs(&self) -> Vec<(String, String)>;
}

/// Production [`PeerDirectorySource`]: the canonical etcd-first peer
/// directory.
///
/// [`crate::substrate::peers::read_directory`] (liveness = the keepalive
/// lease) with the replicated-fs union fallback under `workgroup_root` — the
/// same precedence every other directory reader uses.
#[derive(Debug, Clone)]
pub struct MeshPeerDirectory {
    /// Shared-storage root — the fs-union fallback when etcd is absent.
    workgroup_root: PathBuf,
}

impl MeshPeerDirectory {
    /// Construct over the mesh workgroup root.
    #[must_use]
    pub const fn new(workgroup_root: PathBuf) -> Self {
        Self { workgroup_root }
    }
}

impl PeerDirectorySource for MeshPeerDirectory {
    fn pairs(&self) -> Vec<(String, String)> {
        crate::substrate::peers::read_directory(&self.workgroup_root)
            .into_iter()
            .filter_map(|r| {
                let ip = r.overlay_ip.unwrap_or_default();
                if ip.is_empty() {
                    None
                } else {
                    Some((r.hostname, ip))
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A three-node fixture peer directory (eagle is the leader in most
    /// tests; `pending` has no overlay IP yet — never a record).
    fn fixture_peers() -> Vec<(String, String)> {
        vec![
            ("eagle".to_string(), "10.42.0.9".to_string()),
            ("nyc3".to_string(), "10.42.0.4".to_string()),
            ("pending".to_string(), String::new()),
        ]
    }

    fn records_for<'a>(fqdn: &str, records: &'a [ZoneRecord]) -> Vec<&'a ZoneRecord> {
        records.iter().filter(|r| r.fqdn == fqdn).collect()
    }

    #[test]
    fn node_records_derive_from_the_fixture_peer_directory() {
        // Q46 — every live peer gets <host>.mesh.; a peer with no overlay IP
        // yet gets nothing (never a half record).
        let records = derive_zone_records(&fixture_peers(), None, &[]);
        let eagle = records_for("eagle.mesh.", &records);
        assert_eq!(eagle.len(), 1);
        assert_eq!(eagle[0].ip, "10.42.0.9");
        assert_eq!(eagle[0].kind, RecordKind::Node);
        assert_eq!(records_for("nyc3.mesh.", &records).len(), 1);
        assert!(records_for("pending.mesh.", &records).is_empty());
    }

    #[test]
    fn api_service_names_are_multi_a_sets_across_every_node() {
        // Q22 — APIs on every node, so each API name answers with EVERY live
        // peer's address (the whole set, like the music.mesh precedent).
        let records = derive_zone_records(&fixture_peers(), None, &[]);
        for name in ["keystone.mesh.", "nova.mesh.", "designate.mesh."] {
            let set = records_for(name, &records);
            let ips: Vec<&str> = set.iter().map(|r| r.ip.as_str()).collect();
            assert_eq!(ips, vec!["10.42.0.4", "10.42.0.9"], "{name}");
            assert!(set.iter().all(|r| r.kind == RecordKind::Service));
        }
        // The every-node RabbitMQ cluster name answers the whole set too.
        assert_eq!(records_for("rabbitmq.mesh.", &records).len(), 2);
    }

    #[test]
    fn leader_hosted_names_pin_the_leader_and_are_absent_without_one() {
        // Q15 — mariadb/ovn-nb/ovn-sb resolve to the CURRENT leader only; a
        // failover moves the record. No leader known ⇒ honestly absent.
        let records = derive_zone_records(&fixture_peers(), Some("eagle"), &[]);
        for name in ["mariadb.mesh.", "ovn-nb.mesh.", "ovn-sb.mesh."] {
            let set = records_for(name, &records);
            assert_eq!(set.len(), 1, "{name}");
            assert_eq!(set[0].ip, "10.42.0.9", "{name} pins the leader");
        }
        let leaderless = derive_zone_records(&fixture_peers(), None, &[]);
        assert!(records_for("mariadb.mesh.", &leaderless).is_empty());
        // A leader that isn't in the directory (departed mid-tick) is absent
        // too — never a guessed address.
        let ghost = derive_zone_records(&fixture_peers(), Some("ghost"), &[]);
        assert!(records_for("mariadb.mesh.", &ghost).is_empty());
    }

    #[test]
    fn instance_records_are_namespaced_and_sanitized() {
        // Q46 — instances resolve mesh-wide under .cloud.mesh. (never
        // shadowing a node); names label-fold; addressless instances skip.
        let instances = vec![
            ("web-1".to_string(), "10.42.100.7".to_string()),
            ("My App!!".to_string(), "10.42.100.8".to_string()),
            ("no-addr".to_string(), String::new()),
        ];
        let records = derive_zone_records(&fixture_peers(), None, &instances);
        let web = records_for("web-1.cloud.mesh.", &records);
        assert_eq!(web.len(), 1);
        assert_eq!(web[0].ip, "10.42.100.7");
        assert_eq!(web[0].kind, RecordKind::Instance);
        assert_eq!(records_for("my-app.cloud.mesh.", &records).len(), 1);
        assert!(!records.iter().any(|r| r.fqdn.contains("no-addr")));
    }

    #[test]
    fn instance_pairs_extract_the_first_ipv4_from_the_networks_column() {
        let list = vec![
            CloudInstance {
                id: "u1".into(),
                name: "web-1".into(),
                status: "ACTIVE".into(),
                flavor: None,
                image: None,
                networks: Some("mesh=10.42.100.7".into()),
            },
            CloudInstance {
                id: "u2".into(),
                name: "db-1".into(),
                status: "ACTIVE".into(),
                flavor: None,
                image: None,
                networks: Some("mesh=fe80::1, 10.42.100.9".into()),
            },
            CloudInstance {
                id: "u3".into(),
                name: "booting".into(),
                status: "BUILD".into(),
                flavor: None,
                image: None,
                networks: None,
            },
        ];
        assert_eq!(
            instance_pairs(&list),
            vec![
                ("web-1".to_string(), "10.42.100.7".to_string()),
                ("db-1".to_string(), "10.42.100.9".to_string()),
            ]
        );
    }

    #[test]
    fn reseed_plan_creates_the_zone_first_then_grouped_recordsets() {
        // The Q46 re-seed path: zone create leads, then one recordset per
        // name carrying its whole A-set — enough to rebuild from scratch.
        let records = derive_zone_records(&fixture_peers(), Some("eagle"), &[]);
        let plan = plan_reseed(&records);
        assert_eq!(
            plan[0],
            vec!["zone", "create", "--email", ZONE_EMAIL, MESH_ZONE]
        );
        // Every derived fqdn appears exactly once past the zone create.
        let mut fqdns: Vec<String> = records.iter().map(|r| r.fqdn.clone()).collect();
        fqdns.sort();
        fqdns.dedup();
        assert_eq!(plan.len(), 1 + fqdns.len());
        // A multi-A service name carries every address in ONE recordset.
        let keystone = plan
            .iter()
            .find(|argv| argv.contains(&"keystone.mesh.".to_string()))
            .expect("keystone recordset");
        assert_eq!(
            keystone.iter().filter(|a| *a == "--record").count(),
            2,
            "{keystone:?}"
        );
        assert_eq!(keystone[0], "recordset");
        assert_eq!(keystone[2], MESH_ZONE);
    }

    #[test]
    fn feed_script_is_idempotent_and_carries_the_records_and_gate_note() {
        let dir = tempfile::tempdir().unwrap();
        let records = derive_zone_records(&fixture_peers(), Some("eagle"), &[]);
        let path = render_designate_feed(
            dir.path(),
            "2024.1",
            &records,
            "GATED — fixture: not resolving yet",
        )
        .unwrap();
        assert!(path.ends_with("bootstrap/designate-feed.sh"));
        let body = std::fs::read_to_string(&path).unwrap();
        // Idempotent guards: show-guarded zone create + drift-correcting set.
        assert!(body.contains("openstack zone show"), "{body}");
        assert!(body.contains("|| openstack recordset set"), "{body}");
        // The zone is ensured before any recordset.
        let zone_at = body.find("ensure_zone mesh.").expect("zone ensure");
        let first_rr = body.find("ensure_rrset ").expect("recordsets");
        assert!(zone_at < first_rr, "zone before records");
        // The derived records are all fed (node + multi-A service + leader).
        assert!(
            body.contains("ensure_rrset eagle.mesh. 10.42.0.9"),
            "{body}"
        );
        assert!(
            body.contains("ensure_rrset keystone.mesh. 10.42.0.4 10.42.0.9"),
            "{body}"
        );
        assert!(
            body.contains("ensure_rrset mariadb.mesh. 10.42.0.9"),
            "{body}"
        );
        // The pool is applied first, guarded on its rendered presence.
        assert!(body.contains("designate-manage pool update"), "{body}");
        // The honest live-resolve gate rides the provenance header (§7).
        assert!(
            body.contains("# live-resolve gate: GATED — fixture: not resolving yet"),
            "{body}"
        );
        // Re-render is byte-stable (sorted + deduped derivation).
        render_designate_feed(
            dir.path(),
            "2024.1",
            &records,
            "GATED — fixture: not resolving yet",
        )
        .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), body);
    }

    #[test]
    fn pool_topology_lists_every_node_as_target_nameserver_and_master() {
        // §1 no-fixed-center: every live node's bind9 is a target + a
        // nameserver, and every node's mini-DNS a master for each target.
        let dir = tempfile::tempdir().unwrap();
        let path = render_designate_pools(dir.path(), "2024.1", &fixture_peers()).unwrap();
        let body = std::fs::read_to_string(path).unwrap();
        assert!(body.contains(&format!("id: {DEFAULT_POOL_ID}")), "{body}");
        assert!(body.contains("hostname: eagle.mesh."), "{body}");
        assert!(body.contains("hostname: nyc3.mesh."), "{body}");
        assert_eq!(body.matches("- type: bind9").count(), 2, "{body}");
        // Each of the 2 targets lists both mdns masters (2×2) …
        assert_eq!(body.matches("port: 5354").count(), 4, "{body}");
        // … and drives its bind9 over the sealed-key rndc channel.
        assert_eq!(body.matches("rndc_port: 953").count(), 2, "{body}");
        assert!(
            body.contains("rndc_key_file: /etc/designate/rndc.key"),
            "{body}"
        );
        // The no-overlay-IP peer is nowhere in the topology.
        assert!(!body.contains("pending"), "{body}");
    }

    #[test]
    fn live_resolve_gates_honestly_on_an_unresolvable_name() {
        // §7 — the live DNS check never fabricates a working naming plane:
        // .invalid is RFC-2606-reserved, guaranteed NXDOMAIN everywhere.
        let outcome = live_resolve("no-such-host.invalid.");
        let ResolveOutcome::Gated { reason } = &outcome else {
            unreachable!("a reserved-TLD name must gate, got {outcome:?}");
        };
        assert!(reason.contains("no-such-host.invalid"), "{reason}");
        assert!(reason.contains("does not resolve"), "{reason}");
        let note = resolve_note(&outcome);
        assert!(note.starts_with("GATED — "), "{note}");
    }

    #[test]
    fn dns_labels_fold_and_reject_honestly() {
        assert_eq!(dns_label("Eagle"), Some("eagle".to_string()));
        assert_eq!(dns_label("My App!!"), Some("my-app".to_string()));
        assert_eq!(dns_label("--"), None, "nothing survives");
        assert_eq!(dns_label(""), None);
        assert_eq!(dns_label(&"x".repeat(64)), None, "over the label bound");
    }
}
