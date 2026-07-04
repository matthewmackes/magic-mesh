//! EXPLORER-7 — the edge derivation (design E2/E8): typed relationships between
//! units, computed from the SAME unioned sources the aggregator already folds.
//!
//! No new probes (§7): every edge is derived from the mesh roster
//! ([`MeshSnapshot`]), the `OpenStack` mirror union ([`CloudObjectRecord`]), and
//! the LAN scan set ([`LanHostRecord`]) — the exact three sources `fold::aggregate`
//! unions. When a source is absent (no `OpenStack` objects on today's service-only
//! mirror, a closed LAN scan) its edges are simply absent, never faked.
//!
//! Five edge kinds ([`EdgeKind`], design lock E2):
//! - **`MeshTunnel`** — peer↔peer mesh reachability from the roster, labelled
//!   `direct` or `via <lighthouse>` (a Nebula peer anchors on the lighthouse set).
//!   Logical mesh topology, not a per-flow liveness probe (§7 — no new probe).
//! - **`CloudAttach`** — instance→network / →volume / →boot-image, and
//!   network→subnet / →gateway-router, read from the cloud objects' foreign keys.
//! - **`L2L3Adjacency`** — two LAN hosts sharing an IPv4 `/24` (one broadcast
//!   domain), derived from the scanned addresses.
//! - **`HostPlacement`** — every cloud object → the `Peer` unit of its host node
//!   (the DCIM "runs on" relation, lock #20 / E2(d)).
//! - **`StorageUsage`** — a volume → the instance it's attached to, and a volume →
//!   its backing pool/share (with the consumed size in the detail).
//!
//! Endpoints are the same stable unit ids the fold stamps ([`peer_unit_id`] /
//! [`lan_unit_id`] / [`CloudKind::unit_id`]) so a client can jump an edge straight
//! to a hero unit (EXPLORER-8). Subnets, routers and backing pools are not promoted
//! to hero units, so those endpoints carry their own stable non-unit ids
//! ([`cloud_subnet_endpoint`] / [`cloud_router_endpoint`] / [`storage_pool_endpoint`]);
//! a renderer tells them apart by their id prefix.
//!
//! Symmetric kinds (`MeshTunnel`, `L2L3Adjacency`) are stored with `from <= to`
//! and deduped undirected so `(a,b)` and `(b,a)` collapse to one edge; directed
//! kinds dedup on the exact triple. The output is sorted for a deterministic,
//! churn-free publish (the change gate compares it byte-for-byte).

use std::collections::BTreeSet;
use std::net::Ipv4Addr;

use serde::{Deserialize, Serialize};

use super::sources::{CloudKind, CloudObjectRecord, LanHostRecord, MeshSnapshot};
use super::unit::{lan_unit_id, peer_unit_id};

/// The pinned role token a lighthouse peer carries in the directory row.
const LIGHTHOUSE_ROLE: &str = "lighthouse";

/// The kind of a derived relationship between two units (design E2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// A mesh tunnel between two peers — direct or relayed via a lighthouse.
    MeshTunnel,
    /// A cloud attachment: instance→network/volume/image, network→subnet/router.
    CloudAttach,
    /// L2/L3 adjacency: two LAN hosts sharing a subnet (one broadcast domain).
    L2L3Adjacency,
    /// Host placement: a cloud object runs on a mesh node (the DCIM relation).
    HostPlacement,
    /// Storage usage: a volume attached to an instance / backed by a pool.
    StorageUsage,
}

impl EdgeKind {
    /// Whether the relation is undirected (order-independent): a mesh tunnel and
    /// L2/L3 adjacency read the same both ways, so `(a,b)` and `(b,a)` are one
    /// edge; the cloud/placement/storage relations are directed.
    #[must_use]
    pub const fn is_symmetric(self) -> bool {
        matches!(self, Self::MeshTunnel | Self::L2L3Adjacency)
    }
}

/// A typed relationship between two units (design E8 — derived, not probed).
///
/// `from`/`to` are unit ids (the fold's stable keys) — except the not-yet-modelled
/// subnet/router/pool endpoints, which carry their own prefixed ids so a renderer
/// can distinguish them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    /// The relation kind.
    pub kind: EdgeKind,
    /// The source unit id.
    pub from: String,
    /// The target unit id (or a non-unit subnet/router/pool endpoint id).
    pub to: String,
    /// A short human-readable qualifier (`direct` / `via anvil` / `boot image` /
    /// `runs on node-a` / `backing pool (40 GiB)` …). `None` when nothing to add.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl Edge {
    /// A directed edge `from → to`.
    const fn directed(kind: EdgeKind, from: String, to: String, detail: Option<String>) -> Self {
        Self {
            kind,
            from,
            to,
            detail,
        }
    }

    /// An undirected edge, stored with the endpoints normalized to `from <= to` so
    /// the two orderings dedup to one.
    fn symmetric(kind: EdgeKind, a: String, b: String, detail: Option<String>) -> Self {
        let (from, to) = if a <= b { (a, b) } else { (b, a) };
        Self {
            kind,
            from,
            to,
            detail,
        }
    }
}

/// The stable non-unit endpoint id for a Neutron subnet.
#[must_use]
pub fn cloud_subnet_endpoint(subnet_id: &str) -> String {
    format!("cloud:subnet:{subnet_id}")
}

/// The stable non-unit endpoint id for a Neutron router.
#[must_use]
pub fn cloud_router_endpoint(router_id: &str) -> String {
    format!("cloud:router:{router_id}")
}

/// The stable non-unit endpoint id for a storage backing pool/share.
#[must_use]
pub fn storage_pool_endpoint(pool: &str) -> String {
    format!("pool:{pool}")
}

/// Derive the whole typed edge set from the three unioned sources.
///
/// Pure + I/O-free — the deterministic decision the worker's fold and the tests
/// share (mirrors [`super::fold::aggregate`]). Cloud objects are deduped by unit id
/// first (the same first-sorting-node tie-break the unit fold uses) so a
/// cross-node duplicate emits one edge set, not two.
#[must_use]
pub fn derive_edges(
    mesh: &MeshSnapshot,
    cloud: &[CloudObjectRecord],
    lan: &[LanHostRecord],
) -> Vec<Edge> {
    let mut edges = Vec::new();
    mesh_tunnels(mesh, &mut edges);
    let cloud = super::fold::dedup_cloud_records(cloud);
    cloud_edges(&cloud, &mut edges);
    lan_adjacency(lan, &mut edges);
    dedup(edges)
}

/// `MeshTunnel` edges: every pair of in-mesh peers (self + the roster), labelled
/// `direct` when either endpoint is a lighthouse (peers anchor directly on the
/// lighthouse set) else `via <lighthouse>` when a lighthouse exists to relay them,
/// else `direct` (a flat lighthouse-less mesh).
fn mesh_tunnels(mesh: &MeshSnapshot, out: &mut Vec<Edge>) {
    // Self is always a peer unit (lock #23); union it with the roster, deduped +
    // sorted so the endpoint order is deterministic.
    let mut hosts: BTreeSet<&str> = mesh.peers.iter().map(|p| p.hostname.as_str()).collect();
    hosts.insert(mesh.self_host.as_str());
    let lighthouses: BTreeSet<&str> = mesh
        .peers
        .iter()
        .filter(|p| p.role.as_deref() == Some(LIGHTHOUSE_ROLE))
        .map(|p| p.hostname.as_str())
        .collect();
    let relay = lighthouses.iter().next().copied();
    let hosts: Vec<&str> = hosts.into_iter().collect();
    for (idx, &host) in hosts.iter().enumerate() {
        for &other in &hosts[idx + 1..] {
            let detail = if lighthouses.contains(host) || lighthouses.contains(other) {
                "direct".to_string()
            } else if let Some(lh) = relay {
                format!("via {lh}")
            } else {
                "direct".to_string()
            };
            out.push(Edge::symmetric(
                EdgeKind::MeshTunnel,
                peer_unit_id(host),
                peer_unit_id(other),
                Some(detail),
            ));
        }
    }
}

/// Cloud edges over the deduped object set: `HostPlacement` for every object, plus
/// `CloudAttach`/`StorageUsage` from each object's foreign keys.
fn cloud_edges(cloud: &[&CloudObjectRecord], out: &mut Vec<Edge>) {
    for rec in cloud {
        let from = rec.kind.unit_id(&rec.id);
        // HostPlacement (E2(d)): the object runs on its host node's Peer unit.
        out.push(Edge::directed(
            EdgeKind::HostPlacement,
            from.clone(),
            peer_unit_id(&rec.node),
            Some(format!("runs on {}", rec.node)),
        ));
        let links = &rec.links;
        match rec.kind {
            CloudKind::Instance => {
                for net in &links.networks {
                    out.push(Edge::directed(
                        EdgeKind::CloudAttach,
                        from.clone(),
                        CloudKind::Network.unit_id(net),
                        Some("network".to_string()),
                    ));
                }
                for vol in &links.volumes {
                    out.push(Edge::directed(
                        EdgeKind::CloudAttach,
                        from.clone(),
                        CloudKind::Volume.unit_id(vol),
                        Some("volume".to_string()),
                    ));
                }
                if let Some(image) = &links.image {
                    out.push(Edge::directed(
                        EdgeKind::CloudAttach,
                        from.clone(),
                        CloudKind::Image.unit_id(image),
                        Some("boot image".to_string()),
                    ));
                }
            }
            CloudKind::Network => {
                for subnet in &links.subnets {
                    out.push(Edge::directed(
                        EdgeKind::CloudAttach,
                        from.clone(),
                        cloud_subnet_endpoint(subnet),
                        Some("subnet".to_string()),
                    ));
                }
                if let Some(router) = &links.router {
                    out.push(Edge::directed(
                        EdgeKind::CloudAttach,
                        from.clone(),
                        cloud_router_endpoint(router),
                        Some("gateway router".to_string()),
                    ));
                }
            }
            CloudKind::Volume => {
                if let Some(instance) = &links.attached_to {
                    out.push(Edge::directed(
                        EdgeKind::StorageUsage,
                        from.clone(),
                        CloudKind::Instance.unit_id(instance),
                        Some("attached".to_string()),
                    ));
                }
                if let Some(pool) = &links.pool {
                    let detail = links.size_gb.map_or_else(
                        || "backing pool".to_string(),
                        |gb| format!("backing pool ({gb} GiB)"),
                    );
                    out.push(Edge::directed(
                        EdgeKind::StorageUsage,
                        from.clone(),
                        storage_pool_endpoint(pool),
                        Some(detail),
                    ));
                }
            }
            CloudKind::Image => {}
        }
    }
}

/// `L2L3Adjacency` edges: two LAN hosts whose IPv4 addresses share a `/24` (one
/// broadcast domain). A shared `/24` never over-claims a larger real subnet (same
/// `/24` ⇒ same `/16`), so this is a safe under-approximation — hosts on a larger
/// subnet split across `/24`s simply aren't linked (honest false-negative, §7).
/// A non-IPv4 / unparseable address yields no adjacency.
fn lan_adjacency(lan: &[LanHostRecord], out: &mut Vec<Edge>) {
    let hosts: Vec<(String, [u8; 4])> = lan
        .iter()
        .filter_map(|h| {
            let addr = h.address.as_deref()?;
            let ip: Ipv4Addr = addr.trim().parse().ok()?;
            Some((lan_unit_id(&h.key), ip.octets()))
        })
        .collect();
    for (idx, (id, octets)) in hosts.iter().enumerate() {
        for (other_id, other_octets) in &hosts[idx + 1..] {
            if octets[..3] == other_octets[..3] {
                let net = format!("{}.{}.{}.0/24", octets[0], octets[1], octets[2]);
                out.push(Edge::symmetric(
                    EdgeKind::L2L3Adjacency,
                    id.clone(),
                    other_id.clone(),
                    Some(format!("same subnet {net}")),
                ));
            }
        }
    }
}

/// Dedup + sort into the deterministic published order. Symmetric edges dedup
/// undirected (the endpoints are already normalized `from <= to`); directed edges
/// dedup on the exact `(kind, from, to)` triple. Two opposite-direction edges of
/// *different* kinds (e.g. instance→volume `CloudAttach` and volume→instance
/// `StorageUsage`) are both real perspectives and both kept.
fn dedup(edges: Vec<Edge>) -> Vec<Edge> {
    let mut seen: BTreeSet<(EdgeKind, String, String)> = BTreeSet::new();
    let mut out: Vec<Edge> = Vec::with_capacity(edges.len());
    for edge in edges {
        // Symmetric endpoints are normalized at construction; normalize the key
        // too (belt-and-suspenders) so an undirected duplicate always collapses.
        let (lo, hi) = if edge.kind.is_symmetric() && edge.to < edge.from {
            (edge.to.clone(), edge.from.clone())
        } else {
            (edge.from.clone(), edge.to.clone())
        };
        if seen.insert((edge.kind, lo, hi)) {
            out.push(edge);
        }
    }
    out.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then_with(|| a.from.cmp(&b.from))
            .then_with(|| a.to.cmp(&b.to))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::unit_aggregator::sources::CloudLinks;
    use mackes_mesh_types::peers::PeerRecord;

    fn peer(host: &str, role: Option<&str>) -> PeerRecord {
        let mut r = PeerRecord::now(host, None, "healthy");
        r.role = role.map(ToString::to_string);
        r
    }

    fn mesh(self_host: &str, peers: Vec<PeerRecord>) -> MeshSnapshot {
        MeshSnapshot {
            self_host: self_host.to_string(),
            leader: None,
            peers,
        }
    }

    fn cloud(
        node: &str,
        id: &str,
        kind: CloudKind,
        name: &str,
        links: CloudLinks,
    ) -> CloudObjectRecord {
        CloudObjectRecord {
            node: node.to_string(),
            id: id.to_string(),
            kind,
            name: name.to_string(),
            address: None,
            links,
            detail: crate::workers::unit_aggregator::unit::CloudDetail::default(),
        }
    }

    fn lan(key: &str, addr: &str) -> LanHostRecord {
        LanHostRecord {
            key: key.to_string(),
            name: addr.to_string(),
            address: Some(addr.to_string()),
            ..Default::default()
        }
    }

    fn of_kind(edges: &[Edge], kind: EdgeKind) -> Vec<&Edge> {
        edges.iter().filter(|e| e.kind == kind).collect()
    }

    #[test]
    fn absent_sources_derive_no_edges() {
        // A lone node (no peers, no cloud, no LAN) has no relationships — honest
        // empty, never a faked self-loop (§7).
        let edges = derive_edges(&mesh("me", vec![]), &[], &[]);
        assert!(edges.is_empty());
    }

    #[test]
    fn mesh_tunnels_link_every_peer_pair_and_label_the_relay() {
        // self + two peers, one a lighthouse. 3 hosts → 3 undirected pairs.
        let m = mesh(
            "me",
            vec![
                peer("me", None),
                peer("anvil", Some("lighthouse")),
                peer("zed", None),
            ],
        );
        let edges = derive_edges(&m, &[], &[]);
        let tunnels = of_kind(&edges, EdgeKind::MeshTunnel);
        assert_eq!(tunnels.len(), 3, "3 peers → 3 undirected tunnels");
        // Endpoints are normalized (from <= to), so anvil<me<zed sorts cleanly.
        let find = |a: &str, b: &str| -> &Edge {
            tunnels
                .iter()
                .copied()
                .find(|e| e.from == peer_unit_id(a) && e.to == peer_unit_id(b))
                .expect("mesh tunnel edge present")
        };
        // A pair touching the lighthouse is direct.
        assert_eq!(find("anvil", "me").detail.as_deref(), Some("direct"));
        assert_eq!(find("anvil", "zed").detail.as_deref(), Some("direct"));
        // Two non-lighthouse peers relay via the lighthouse.
        assert_eq!(find("me", "zed").detail.as_deref(), Some("via anvil"));
    }

    #[test]
    fn flat_mesh_without_a_lighthouse_is_direct() {
        let m = mesh("me", vec![peer("me", None), peer("box", None)]);
        let tunnels = of_kind(&derive_edges(&m, &[], &[]), EdgeKind::MeshTunnel)
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(tunnels.len(), 1);
        assert_eq!(tunnels[0].detail.as_deref(), Some("direct"));
    }

    #[test]
    fn cloud_attach_reads_the_instance_foreign_keys() {
        let inst = cloud(
            "node-a",
            "i1",
            CloudKind::Instance,
            "web",
            CloudLinks {
                networks: vec!["n1".into()],
                volumes: vec!["v1".into(), "v2".into()],
                image: Some("img1".into()),
                ..Default::default()
            },
        );
        let edges = derive_edges(&mesh("me", vec![]), &[inst], &[]);
        let attach = of_kind(&edges, EdgeKind::CloudAttach);
        let targets: BTreeSet<&str> = attach.iter().map(|e| e.to.as_str()).collect();
        assert!(targets.contains("cloud:network:n1"));
        assert!(targets.contains("cloud:volume:v1"));
        assert!(targets.contains("cloud:volume:v2"));
        assert!(targets.contains("cloud:image:img1"));
        assert!(attach.iter().all(|e| e.from == "cloud:instance:i1"));
    }

    #[test]
    fn cloud_attach_reads_the_network_subnet_router_chain() {
        let net = cloud(
            "node-a",
            "n1",
            CloudKind::Network,
            "tenant-net",
            CloudLinks {
                subnets: vec!["s1".into(), "s2".into()],
                router: Some("r1".into()),
                ..Default::default()
            },
        );
        let edges = derive_edges(&mesh("me", vec![]), &[net], &[]);
        let attach = of_kind(&edges, EdgeKind::CloudAttach);
        let targets: BTreeSet<&str> = attach.iter().map(|e| e.to.as_str()).collect();
        assert!(targets.contains(cloud_subnet_endpoint("s1").as_str()));
        assert!(targets.contains(cloud_subnet_endpoint("s2").as_str()));
        assert!(targets.contains(cloud_router_endpoint("r1").as_str()));
    }

    #[test]
    fn host_placement_points_each_object_at_its_node_peer() {
        let inst = cloud(
            "node-a",
            "i1",
            CloudKind::Instance,
            "web",
            CloudLinks::default(),
        );
        let edges = derive_edges(&mesh("me", vec![]), &[inst], &[]);
        let placement = of_kind(&edges, EdgeKind::HostPlacement);
        assert_eq!(placement.len(), 1);
        assert_eq!(placement[0].from, "cloud:instance:i1");
        assert_eq!(placement[0].to, peer_unit_id("node-a"));
        assert_eq!(placement[0].detail.as_deref(), Some("runs on node-a"));
    }

    #[test]
    fn storage_usage_links_the_volume_to_instance_and_pool() {
        let vol = cloud(
            "node-a",
            "v1",
            CloudKind::Volume,
            "data",
            CloudLinks {
                attached_to: Some("i1".into()),
                pool: Some("ceph-ssd".into()),
                size_gb: Some(40),
                ..Default::default()
            },
        );
        let edges = derive_edges(&mesh("me", vec![]), &[vol], &[]);
        let storage = of_kind(&edges, EdgeKind::StorageUsage);
        let by_to = |to: &str| storage.iter().find(|e| e.to == to);
        // volume → instance attachment.
        let attach = by_to("cloud:instance:i1").expect("attachment edge");
        assert_eq!(attach.from, "cloud:volume:v1");
        assert_eq!(attach.detail.as_deref(), Some("attached"));
        // volume → backing pool, with the consumed size in the detail.
        let pool = by_to(&storage_pool_endpoint("ceph-ssd")).expect("pool edge");
        assert_eq!(pool.detail.as_deref(), Some("backing pool (40 GiB)"));
    }

    #[test]
    fn lan_adjacency_links_same_slash24_only() {
        let hosts = vec![
            lan("a", "172.20.0.5"),
            lan("b", "172.20.0.9"),
            lan("c", "172.20.5.9"), // different /24
            lan("d", "not-an-ip"),  // unparseable → no edge
        ];
        let edges = derive_edges(&mesh("me", vec![]), &[], &hosts);
        let adj = of_kind(&edges, EdgeKind::L2L3Adjacency);
        // Only a<->b share a /24.
        assert_eq!(adj.len(), 1);
        assert_eq!(adj[0].from, lan_unit_id("a"));
        assert_eq!(adj[0].to, lan_unit_id("b"));
        assert_eq!(adj[0].detail.as_deref(), Some("same subnet 172.20.0.0/24"));
    }

    #[test]
    fn cloud_dedup_across_nodes_yields_one_edge_set() {
        // The same instance mirrored by two nodes must not double its edges.
        let a = cloud(
            "node-a",
            "i1",
            CloudKind::Instance,
            "web",
            CloudLinks {
                networks: vec!["n1".into()],
                ..Default::default()
            },
        );
        let b = cloud(
            "node-b",
            "i1",
            CloudKind::Instance,
            "web",
            CloudLinks {
                networks: vec!["n1".into()],
                ..Default::default()
            },
        );
        let edges = derive_edges(&mesh("me", vec![]), &[a, b], &[]);
        // One HostPlacement (to node-a, the first-sorting node) + one CloudAttach.
        assert_eq!(of_kind(&edges, EdgeKind::HostPlacement).len(), 1);
        let placement = of_kind(&edges, EdgeKind::HostPlacement)[0];
        assert_eq!(placement.to, peer_unit_id("node-a"));
        assert_eq!(of_kind(&edges, EdgeKind::CloudAttach).len(), 1);
    }

    #[test]
    fn symmetric_edges_dedup_undirected() {
        // Two LAN hosts on one /24 must produce exactly one undirected edge,
        // regardless of scan order.
        let forward = derive_edges(
            &mesh("me", vec![]),
            &[],
            &[lan("a", "10.0.0.1"), lan("b", "10.0.0.2")],
        );
        let reverse = derive_edges(
            &mesh("me", vec![]),
            &[],
            &[lan("b", "10.0.0.2"), lan("a", "10.0.0.1")],
        );
        assert_eq!(of_kind(&forward, EdgeKind::L2L3Adjacency).len(), 1);
        // Order-independent: the normalized edge is identical both ways.
        assert_eq!(forward, reverse);
    }

    #[test]
    fn output_is_sorted_and_round_trips_json() {
        let m = mesh("me", vec![peer("me", None), peer("box", None)]);
        let vol = cloud(
            "node-a",
            "v1",
            CloudKind::Volume,
            "data",
            CloudLinks {
                attached_to: Some("i1".into()),
                ..Default::default()
            },
        );
        let edges = derive_edges(&m, &[vol], &[lan("a", "10.0.0.1"), lan("b", "10.0.0.2")]);
        // Sorted by (kind, from, to): MeshTunnel < CloudAttach? No — declaration
        // order defines Ord, so MeshTunnel(0) first. Assert non-decreasing kinds.
        let kinds: Vec<EdgeKind> = edges.iter().map(|e| e.kind).collect();
        let mut sorted = kinds.clone();
        sorted.sort_unstable();
        assert_eq!(
            kinds, sorted,
            "edges are kind-sorted for a churn-free publish"
        );
        let json = serde_json::to_string(&edges).expect("serialize");
        let back: Vec<Edge> = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, edges);
    }
}
