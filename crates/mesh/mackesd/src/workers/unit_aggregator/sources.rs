//! EXPLORER-1 — the injectable source seams the aggregator unions.
//!
//! Three seams, each headless-testable with a fake (the cloud worker's two-seam
//! + testkit shape):
//!
//! - [`MeshMirrorSource`] — the mesh half (source (a), lock #2): the replicated
//!   peer directory + the `/mesh/leader` lease + per-peer health the tray/Fleet
//!   already read. Production: [`MeshDirectoryMirror`] over
//!   `crate::substrate::peers::read_directory` + `current_leader_blocking`.
//! - [`CloudMirrorSource`] — the cloud half (source (b), lock #20): the union of
//!   every node's provider-neutral `state/cloud/<node>` Bus mirror, decoded into
//!   host-tagged cloud objects. Production: [`BusCloudMirror`] reads the
//!   persisted Bus tree. It consumes cloud mirrors through the Bus read path.
//! - [`LanScanSource`] — the off-mesh half (EXPLORER-2 producer seam): the
//!   active LAN scan, gated on the surface's scan-active flag (lock #24).
//!   Production here is [`NoScan`] (empty); EXPLORER-2 swaps in the real
//!   nmap-style scan.
//!
//! Forward-compat honesty (§7): today's `state/cloud/<node>` mirror carries
//! *service* supervision, not tenant *objects*, so [`BusCloudMirror`] decodes zero
//! cloud objects on the live fleet — cloud units are simply absent, never faked.
//! When a later slice publishes a resource `objects` array on the mirror,
//! [`CloudMirrorBody`]'s tolerant decode picks it up.

use std::path::PathBuf;

use serde::Deserialize;

use mackes_mesh_types::peers::PeerRecord;

use super::unit::{CloudDetail, UnitKind};

// ─────────────────────────── mesh seam ───────────────────────────

/// One read of the mesh mirror: this node's id, the current leader (if any), and
/// the live peer directory rows.
#[derive(Debug, Clone, Default)]
pub struct MeshSnapshot {
    /// This node's own id — always folded as the first unit (lock #23).
    pub self_host: String,
    /// The hostname holding the `/mesh/leader` lease, when one is elected.
    pub leader: Option<String>,
    /// The replicated peer directory rows (etcd-first, fs-fallback union).
    pub peers: Vec<PeerRecord>,
}

/// The mesh half of the union (source (a), lock #2).
pub trait MeshMirrorSource: Send + Sync {
    /// Read the current mesh mirror snapshot.
    fn read(&self) -> MeshSnapshot;
}

/// Production [`MeshMirrorSource`]: the replicated peer directory + the etcd
/// leader lease — the exact canonical readers the directory responder, the
/// health reconciler, and the Fleet plane use.
pub struct MeshDirectoryMirror {
    workgroup_root: PathBuf,
    self_host: String,
}

impl MeshDirectoryMirror {
    /// Construct over the replicated `workgroup_root` (the peer directory lives
    /// under it) with this node's `self_host` id.
    #[must_use]
    pub const fn new(workgroup_root: PathBuf, self_host: String) -> Self {
        Self {
            workgroup_root,
            self_host,
        }
    }
}

impl MeshMirrorSource for MeshDirectoryMirror {
    fn read(&self) -> MeshSnapshot {
        // The canonical peer directory (etcd substrate first, fs union fallback)
        // — the same `read_directory` the directory RPC + health reconciler use.
        let peers = crate::substrate::peers::read_directory(&self.workgroup_root);
        // The current `/mesh/leader` lease holder, when the coordination plane is
        // provisioned. Best-effort: an absent/unreachable etcd yields no leader
        // (the units still publish — we never block the fold on the leader read).
        let leader = {
            let eps = crate::substrate::etcd::default_endpoints();
            if eps.is_empty() {
                None
            } else {
                crate::substrate::leader::current_leader_blocking(&eps).map(|l| l.node_id)
            }
        };
        MeshSnapshot {
            self_host: self.self_host.clone(),
            leader,
            peers,
        }
    }
}

// ─────────────────────────── cloud seam ───────────────────────────

/// The kind of a cloud object (lock #4: the four resource kinds).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloudKind {
    /// A Nova compute instance.
    Instance,
    /// A Cinder volume.
    Volume,
    /// A Glance image.
    Image,
    /// A Neutron network.
    Network,
}

impl CloudKind {
    /// The [`UnitKind`] this cloud object folds into.
    #[must_use]
    pub const fn unit_kind(self) -> UnitKind {
        match self {
            Self::Instance => UnitKind::Instance,
            Self::Volume => UnitKind::Volume,
            Self::Image => UnitKind::Image,
            Self::Network => UnitKind::Network,
        }
    }

    /// The stable unit id for an object of this kind: `cloud:<kind>:<object-id>`
    /// — the mesh-wide dedup key (lock #20: the same object id under two nodes'
    /// mirrors lists once).
    #[must_use]
    pub fn unit_id(self, object_id: &str) -> String {
        format!("cloud:{}:{}", self.unit_kind().as_str(), object_id)
    }
}

/// The foreign-key references a cloud object carries (design E2/E8).
///
/// Feeds EXPLORER-7's edge derivation. Every field is default-absent (§7 honest):
/// today's service-only mirror carries none, so no cloud edges are derived until a
/// forward-compat mirror publishes them. Kept a discrete block so the base
/// [`CloudObjectRecord`] identity fields stay stable.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CloudLinks {
    /// Networks this instance is attached to (Nova → Neutron), by network id.
    pub networks: Vec<String>,
    /// Volumes attached to this instance (Nova → Cinder), by volume id.
    pub volumes: Vec<String>,
    /// The boot image of this instance (Nova → Glance), by image id.
    pub image: Option<String>,
    /// Subnets this network owns (Neutron), by subnet id.
    pub subnets: Vec<String>,
    /// The gateway router of this network (Neutron), by router id.
    pub router: Option<String>,
    /// The instance this volume is attached to (Cinder attachment), by id.
    pub attached_to: Option<String>,
    /// The backing pool/share this volume consumes (Cinder host/pool).
    pub pool: Option<String>,
    /// The volume's size in GiB — the consumed amount, for the storage edge detail.
    pub size_gb: Option<u64>,
}

/// One cloud object folded from a node's `state/cloud/<node>` mirror, tagged
/// with the host node that runs it (lock #20).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudObjectRecord {
    /// The host node id — the object's own `host` when the mirror carries it,
    /// else the publishing node (the `<node>` in the topic).
    pub node: String,
    /// The cloud object id (the dedup key across nodes).
    pub id: String,
    /// The resource kind.
    pub kind: CloudKind,
    /// The object's display name.
    pub name: String,
    /// A fixed/floating address, when the mirror carries one.
    pub address: Option<String>,
    /// The object's foreign keys, feeding EXPLORER-7's edge derivation (E2/E8).
    pub links: CloudLinks,
    /// The deep E4 detail (flavor/state/IPs/keypair/secgroups/…), folded onto the
    /// unit's [`CloudDetail`] block (EXPLORER-9). Empty on today's service-only
    /// mirror (honest §7); populated by a forward-compat object mirror.
    pub detail: CloudDetail,
}

/// The cloud half of the union (source (b), lock #20).
pub trait CloudMirrorSource: Send + Sync {
    /// Read the raw union of cloud objects across every node's mirror. Dedup by
    /// object id is the fold's job (see `super::fold`), so this returns the whole
    /// unioned set (possibly with cross-node duplicates).
    fn read(&self) -> Vec<CloudObjectRecord>;
}

/// The Bus topic prefix every node's provider-neutral cloud mirror publishes
/// under.
pub const CLOUD_TOPIC_PREFIX: &str = "state/cloud/";

/// The tolerant decode of one `state/cloud/<node>` body.
///
/// Only the fields the aggregator cares about are declared; serde ignores the
/// rest (the mirror's doctrine/runtime/services). `objects` is absent on today's
/// service-only mirror (⇒ empty, honest §7) and populated once a publisher
/// carries tenant resources on the cloud topic.
#[derive(Debug, Clone, Deserialize)]
pub struct CloudMirrorBody {
    /// The publishing node id (the mirror `host` stamp), if present.
    #[serde(default)]
    pub host: Option<String>,
    /// The tenant resource objects, when the mirror carries them.
    #[serde(default)]
    pub objects: Vec<CloudObjectWire>,
}

/// One cloud object as it rides the mirror wire.
#[derive(Debug, Clone, Deserialize)]
pub struct CloudObjectWire {
    /// The cloud object id.
    pub id: String,
    /// The resource kind.
    pub kind: CloudKind,
    /// The display name.
    pub name: String,
    /// A fixed/floating address, when known.
    #[serde(default)]
    pub address: Option<String>,
    /// The object's host node, when the mirror names it (overrides the
    /// publishing node as the host tag).
    #[serde(default)]
    pub host: Option<String>,
    /// Instance → network ids (EXPLORER-7 edge FK). Absent on today's mirror.
    #[serde(default)]
    pub networks: Vec<String>,
    /// Instance → volume ids (EXPLORER-7 edge FK).
    #[serde(default)]
    pub volumes: Vec<String>,
    /// Instance → boot image id (EXPLORER-7 edge FK).
    #[serde(default)]
    pub image: Option<String>,
    /// Network → subnet ids (EXPLORER-7 edge FK).
    #[serde(default)]
    pub subnets: Vec<String>,
    /// Network → gateway router id (EXPLORER-7 edge FK).
    #[serde(default)]
    pub router: Option<String>,
    /// Volume → attached instance id (EXPLORER-7 edge FK).
    #[serde(default)]
    pub attached_to: Option<String>,
    /// Volume → backing pool/share (EXPLORER-7 edge FK).
    #[serde(default)]
    pub pool: Option<String>,
    /// Volume size in GiB — the storage edge's consumed-amount detail (also the
    /// [`CloudDetail::size_gb`] of a Volume unit).
    #[serde(default)]
    pub size_gb: Option<u64>,
    // ── E4 detail (EXPLORER-9) — absent on today's service-only mirror (§7). ──
    /// Nova flavor name.
    #[serde(default)]
    pub flavor: Option<String>,
    /// vCPU count (Nova flavor).
    #[serde(default)]
    pub vcpus: Option<u32>,
    /// RAM in MiB (Nova flavor).
    #[serde(default)]
    pub ram_mb: Option<u64>,
    /// Root disk in GiB (Nova flavor).
    #[serde(default)]
    pub disk_gb: Option<u64>,
    /// Nova power state (`running`/`shutdown`/…).
    #[serde(default)]
    pub power_state: Option<String>,
    /// Nova task state while transitioning (`spawning`/`deleting`/…).
    #[serde(default)]
    pub task_state: Option<String>,
    /// Cinder/Glance/Neutron object status (`available`/`in-use`/`active`/…).
    #[serde(default)]
    pub status: Option<String>,
    /// All fixed IPs (Nova/Neutron).
    #[serde(default)]
    pub fixed_ips: Vec<String>,
    /// All floating IPs (Nova/Neutron).
    #[serde(default)]
    pub floating_ips: Vec<String>,
    /// Neutron port ids attached to this instance.
    #[serde(default)]
    pub ports: Vec<String>,
    /// Nova keypair name.
    #[serde(default)]
    pub keypair: Option<String>,
    /// Security-group names (Nova/Neutron).
    #[serde(default)]
    pub security_groups: Vec<String>,
    /// Creation timestamp as the mirror carries it (ISO-8601 string).
    #[serde(default)]
    pub created: Option<String>,
    /// Uptime in seconds, when the mirror reports it.
    #[serde(default)]
    pub uptime_s: Option<u64>,
}

/// The `<node>` leaf of a `state/cloud/<node>` topic, or `None` if `topic` isn't
/// under the cloud prefix (or names no node).
#[must_use]
pub fn cloud_topic_node(topic: &str) -> Option<&str> {
    topic
        .strip_prefix(CLOUD_TOPIC_PREFIX)
        .filter(valid_topic_leaf)
}

fn valid_topic_leaf(node: &&str) -> bool {
    !node.is_empty() && !node.contains('/')
}

/// Fold one mirror body (published by `topic_node`) into cloud object records.
///
/// The host tag prefers the object's own `host`, falling back to the publishing
/// node. Pure — the wire→record mapping the Bus reader + the tests share.
#[must_use]
pub fn records_from_body(topic_node: &str, body: &CloudMirrorBody) -> Vec<CloudObjectRecord> {
    let publisher = body.host.as_deref().unwrap_or(topic_node);
    body.objects
        .iter()
        .map(|o| CloudObjectRecord {
            node: o.host.as_deref().unwrap_or(publisher).to_string(),
            id: o.id.clone(),
            kind: o.kind,
            name: o.name.clone(),
            address: o.address.clone(),
            links: CloudLinks {
                networks: o.networks.clone(),
                volumes: o.volumes.clone(),
                image: o.image.clone(),
                subnets: o.subnets.clone(),
                router: o.router.clone(),
                attached_to: o.attached_to.clone(),
                pool: o.pool.clone(),
                size_gb: o.size_gb,
            },
            detail: CloudDetail {
                flavor: o.flavor.clone(),
                vcpus: o.vcpus,
                ram_mb: o.ram_mb,
                disk_gb: o.disk_gb,
                size_gb: o.size_gb,
                power_state: o.power_state.clone(),
                task_state: o.task_state.clone(),
                status: o.status.clone(),
                fixed_ips: o.fixed_ips.clone(),
                floating_ips: o.floating_ips.clone(),
                ports: o.ports.clone(),
                keypair: o.keypair.clone(),
                security_groups: o.security_groups.clone(),
                created: o.created.clone(),
                uptime_s: o.uptime_s,
            },
        })
        .collect()
}

/// Production [`CloudMirrorSource`] — reads the persisted Bus tree.
///
/// Takes the latest body on each provider-neutral `state/cloud/<node>` topic and
/// folds its objects.
pub struct BusCloudMirror {
    bus_root: PathBuf,
}

impl BusCloudMirror {
    /// Construct over the Bus root (the persisted message tree).
    #[must_use]
    pub const fn new(bus_root: PathBuf) -> Self {
        Self { bus_root }
    }
}

impl CloudMirrorSource for BusCloudMirror {
    fn read(&self) -> Vec<CloudObjectRecord> {
        let persist = match mde_bus::persist::Persist::open(self.bus_root.clone()) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(target: "mackesd::units", error = %e, "cloud mirror: persist open failed");
                return Vec::new();
            }
        };
        let topics = match persist.list_topics() {
            Ok(t) => t,
            Err(e) => {
                tracing::debug!(target: "mackesd::units", error = %e, "cloud mirror: list_topics failed");
                return Vec::new();
            }
        };
        let mut out = Vec::new();
        for topic in topics {
            let Some(node) = cloud_topic_node(&topic) else {
                continue;
            };
            let Ok(msgs) = persist.list_since(&topic, None) else {
                continue;
            };
            // The latest body on the topic is the live mirror row.
            let Some(body_str) = msgs.into_iter().next_back().and_then(|m| m.body) else {
                continue;
            };
            match serde_json::from_str::<CloudMirrorBody>(&body_str) {
                Ok(body) => out.extend(records_from_body(node, &body)),
                Err(e) => {
                    tracing::debug!(target: "mackesd::units", topic = %topic, error = %e, "cloud mirror: body decode failed");
                }
            }
        }
        out
    }
}

/// The fallback [`CloudMirrorSource`] when no Bus root resolves (a headless
/// dev box with no `dirs::data_dir()`): no cloud objects, honestly absent (§7).
pub struct NoCloud;

impl CloudMirrorSource for NoCloud {
    fn read(&self) -> Vec<CloudObjectRecord> {
        Vec::new()
    }
}

// ─────────────────────────── LAN scan seam ───────────────────────────

/// One LAN host the active scan discovered (EXPLORER-2 fills these).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LanHostRecord {
    /// The stable id key — the host's MAC (preferred) or IP (fallback).
    pub key: String,
    /// The display name (rDNS/mDNS name, or the address until enriched).
    pub name: String,
    /// The host's LAN address, when known.
    pub address: Option<String>,
    /// Open service labels from the light port fingerprint (E5, EXPLORER-2):
    /// e.g. `["ssh", "rdp", "vnc"]`. Empty when nothing answered / unprobed (§7).
    pub services: Vec<String>,
    /// The raw open fingerprint ports behind [`Self::services`] — the seam the
    /// openable per-service actions (E5) ride. Empty when none.
    pub open_ports: Vec<u16>,
    /// Coarse device-type guess from the fingerprint + mDNS (E5). `None` when
    /// nothing is confident enough — honest unknown, never guessed (§7).
    pub type_guess: Option<String>,
    /// Reverse-DNS / mDNS name (E5), when resolved. `None` ⇒ unresolved.
    pub rdns: Option<String>,
}

/// The off-mesh half (EXPLORER-2 producer seam). The active scan runs ONLY while
/// `scan_active` is set by the open surface (lock #24); a closed surface scans
/// nothing.
pub trait LanScanSource: Send + Sync {
    /// Return the discovered LAN hosts. `scan_active` is the surface-gated flag
    /// (lock #24): an implementation MUST NOT probe the network when it is
    /// `false` (it may still return a warm cache).
    fn scan(&self, scan_active: bool) -> Vec<LanHostRecord>;
}

/// The EXPLORER-1 default: no scan. Always empty regardless of the flag —
/// EXPLORER-2 swaps in the real mDNS/ARP/ping-sweep scan behind this seam.
pub struct NoScan;

impl LanScanSource for NoScan {
    fn scan(&self, _scan_active: bool) -> Vec<LanHostRecord> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_bus::hooks::config::Priority;

    use crate::workers::unit_aggregator::fold::{aggregate, SeenTracker};
    use crate::workers::unit_aggregator::unit::UnitKind;

    #[test]
    fn topic_node_accepts_the_cloud_prefix() {
        assert_eq!(cloud_topic_node("state/cloud/node-a"), Some("node-a"));
        // Not a cloud mirror topic → None (the retired legacy openstack prefix is
        // gone — a `state/openstack/*` topic no longer resolves to a node).
        assert_eq!(cloud_topic_node("state/storage/node-a"), None);
        assert_eq!(cloud_topic_node("state/openstack/node-a"), None);
        // Empty or nested leaves are invalid.
        assert_eq!(cloud_topic_node("state/cloud/"), None);
        assert_eq!(cloud_topic_node("state/cloud/node-a/extra"), None);
    }

    #[test]
    fn cloud_ids_are_stable_and_kind_namespaced() {
        assert_eq!(
            CloudKind::Instance.unit_id("uuid-1"),
            "cloud:instance:uuid-1"
        );
        assert_eq!(CloudKind::Volume.unit_id("v9"), "cloud:volume:v9");
        // Same object id under different kinds never collides.
        assert_ne!(
            CloudKind::Image.unit_id("x"),
            CloudKind::Network.unit_id("x")
        );
        assert_eq!(CloudKind::Instance.unit_kind(), UnitKind::Instance);
    }

    #[test]
    fn service_only_body_folds_zero_objects() {
        // Today's QC-2 mirror body (doctrine/runtime/services, no `objects`)
        // decodes to zero cloud objects — honest absence, not a fake (§7).
        let body_str = r#"{
            "host":"node-a",
            "doctrine":{"status":"disabled"},
            "runtime":{"status":"available"},
            "services":[{"service":"keystone","status":{"state":"running"}}],
            "extras":[],
            "published_at_ms":1
        }"#;
        let body: CloudMirrorBody = serde_json::from_str(body_str).expect("decode");
        assert_eq!(body.host.as_deref(), Some("node-a"));
        assert!(records_from_body("node-a", &body).is_empty());
    }

    #[test]
    fn objects_body_folds_host_tagged_records() {
        // A forward-compat mirror that DOES carry tenant objects folds them,
        // host-tagged (the object's own host overrides the publishing node).
        let body_str = r#"{
            "host":"node-a",
            "objects":[
                {"id":"i1","kind":"instance","name":"web","address":"10.0.0.5"},
                {"id":"n1","kind":"network","name":"tenant-net","host":"node-b"}
            ]
        }"#;
        let body: CloudMirrorBody = serde_json::from_str(body_str).expect("decode");
        let recs = records_from_body("node-a", &body);
        assert_eq!(recs.len(), 2);
        // Object without its own host → tagged with the publishing node.
        assert_eq!(recs[0].node, "node-a");
        assert_eq!(recs[0].kind, CloudKind::Instance);
        assert_eq!(recs[0].address.as_deref(), Some("10.0.0.5"));
        // Object with its own host → tagged there (the host-node tag, lock #20).
        assert_eq!(recs[1].node, "node-b");
        assert_eq!(recs[1].kind, CloudKind::Network);
    }

    #[test]
    fn bus_cloud_source_folds_cloud_topics_into_units() {
        let bus = tempfile::tempdir().expect("temp bus");
        let persist =
            mde_bus::persist::Persist::open(bus.path().to_path_buf()).expect("open temp bus");
        persist
            .write(
                "state/cloud/node-a",
                Priority::Default,
                None,
                Some(
                    r#"{
                    "host":"node-a",
                    "objects":[
                        {"id":"i1","kind":"instance","name":"web","address":"10.0.0.5"}
                    ]
                }"#,
                ),
            )
            .expect("publish cloud mirror");
        // A second node's provider-neutral cloud mirror — the union folds both.
        persist
            .write(
                "state/cloud/node-b",
                Priority::Default,
                None,
                Some(
                    r#"{
                    "host":"node-b",
                    "objects":[
                        {"id":"v1","kind":"volume","name":"data","size_gb":40}
                    ]
                }"#,
                ),
            )
            .expect("publish cloud mirror");

        let reader = BusCloudMirror::new(bus.path().to_path_buf());
        let records = CloudMirrorSource::read(&reader);
        assert_eq!(records.len(), 2);
        assert!(records.iter().any(|r| r.id == "i1" && r.node == "node-a"));
        assert!(records.iter().any(|r| r.id == "v1" && r.node == "node-b"));

        let mesh = MeshSnapshot {
            self_host: "self-node".to_string(),
            ..MeshSnapshot::default()
        };
        let mut seen = SeenTracker::new();
        let units = aggregate(&mesh, &records, &[], &mut seen, 1);
        assert!(units
            .iter()
            .any(|u| u.id == "cloud:instance:i1" && u.kind == UnitKind::Instance));
        assert!(units
            .iter()
            .any(|u| u.id == "cloud:volume:v1" && u.kind == UnitKind::Volume));
    }

    #[test]
    fn object_foreign_keys_decode_into_links_for_edge_derivation() {
        // A forward-compat mirror carrying EXPLORER-7 foreign keys folds them into
        // the record's links block (feeding the edge derivation); a plain object
        // leaves every link default-absent (§7).
        let body_str = r#"{
            "host":"node-a",
            "objects":[
                {"id":"i1","kind":"instance","name":"web",
                 "networks":["n1"],"volumes":["v1"],"image":"img1"},
                {"id":"v1","kind":"volume","name":"data",
                 "attached_to":"i1","pool":"ceph","size_gb":40},
                {"id":"img1","kind":"image","name":"cirros"}
            ]
        }"#;
        let body: CloudMirrorBody = serde_json::from_str(body_str).expect("decode");
        let recs = records_from_body("node-a", &body);
        assert_eq!(recs.len(), 3);
        assert_eq!(recs[0].links.networks, vec!["n1".to_string()]);
        assert_eq!(recs[0].links.volumes, vec!["v1".to_string()]);
        assert_eq!(recs[0].links.image.as_deref(), Some("img1"));
        assert_eq!(recs[1].links.attached_to.as_deref(), Some("i1"));
        assert_eq!(recs[1].links.pool.as_deref(), Some("ceph"));
        assert_eq!(recs[1].links.size_gb, Some(40));
        // A plain object → default-absent links.
        assert_eq!(recs[2].links, CloudLinks::default());
    }

    #[test]
    fn instance_object_body_decodes_full_e4_detail() {
        // A forward-compat mirror carrying the E4 instance detail sheet folds every
        // field into the record's CloudDetail; a plain object leaves it empty (§7).
        let body_str = r#"{
            "host":"node-a",
            "objects":[
                {"id":"i1","kind":"instance","name":"web","address":"10.0.0.5",
                 "flavor":"m1.small","vcpus":2,"ram_mb":2048,"disk_gb":20,
                 "power_state":"running","task_state":null,"status":"ACTIVE",
                 "fixed_ips":["10.0.0.5"],"floating_ips":["172.24.4.7"],
                 "ports":["p1","p2"],"keypair":"mesh-key",
                 "security_groups":["default","web"],
                 "created":"2026-07-04T12:00:00Z","uptime_s":3600},
                {"id":"v1","kind":"volume","name":"data","size_gb":40,"status":"in-use"},
                {"id":"img1","kind":"image","name":"cirros"}
            ]
        }"#;
        let body: CloudMirrorBody = serde_json::from_str(body_str).expect("decode");
        let recs = records_from_body("node-a", &body);
        assert_eq!(recs.len(), 3);
        let inst = &recs[0].detail;
        assert_eq!(inst.flavor.as_deref(), Some("m1.small"));
        assert_eq!(inst.vcpus, Some(2));
        assert_eq!(inst.ram_mb, Some(2048));
        assert_eq!(inst.disk_gb, Some(20));
        assert_eq!(inst.power_state.as_deref(), Some("running"));
        assert_eq!(inst.task_state, None);
        assert_eq!(inst.status.as_deref(), Some("ACTIVE"));
        assert_eq!(inst.fixed_ips, vec!["10.0.0.5".to_string()]);
        assert_eq!(inst.floating_ips, vec!["172.24.4.7".to_string()]);
        assert_eq!(inst.ports, vec!["p1".to_string(), "p2".to_string()]);
        assert_eq!(inst.keypair.as_deref(), Some("mesh-key"));
        assert_eq!(
            inst.security_groups,
            vec!["default".to_string(), "web".to_string()]
        );
        assert_eq!(inst.created.as_deref(), Some("2026-07-04T12:00:00Z"));
        assert_eq!(inst.uptime_s, Some(3600));
        // The volume's own capacity + status ride the detail too.
        assert_eq!(recs[1].detail.size_gb, Some(40));
        assert_eq!(recs[1].detail.status.as_deref(), Some("in-use"));
        // A plain object → empty detail (honest unknown, §7).
        assert!(recs[2].detail.is_empty());
    }

    #[test]
    fn no_scan_returns_empty_regardless_of_the_flag() {
        assert!(NoScan.scan(true).is_empty());
        assert!(NoScan.scan(false).is_empty());
    }
}
