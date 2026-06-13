//! Topology engine — pure function that computes expected peer
//! adjacencies + route tables given the current desired-state
//! snapshot (Phase 12.4).
//!
//! Lives outside the rest of the daemon's I/O paths so it can be
//! tested deterministically with golden fixtures (12.4.1). The
//! reconciler calls into this module on every tick; the panel's
//! topology renderer reads its output for the live drawing
//! (12.9.1).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use crate::policy::Policy;

/// One node in the mesh as the topology engine sees it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    /// Stable node id (e.g. `peer:anvil`).
    pub id: String,
    /// Region tag — drives east-west policy decisions.
    pub region: String,
    /// `true` when the node has been registered + heartbeated.
    pub healthy: bool,
    /// `true` when the node is `Host`-role (eligible for
    /// leader election + relay duties).
    pub is_host: bool,
}

/// One edge in the calculated topology.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Edge {
    /// Lexicographically lower node id.
    pub a: String,
    /// Lexicographically higher node id.
    pub b: String,
    /// Transport flavor: direct UDP, DERP relay, or HTTPS 443
    /// tunnel (per 12.14 / 12.16 / 12.18).
    pub kind: EdgeKind,
}

/// Distinct transports the topology engine knows about. Two edges
/// between the same pair via different transports are tracked
/// independently — the operator's diff overlay (12.9.3) renders
/// them as separate lines.
///
/// Stays in lock-step with `mackes_transport::TransportKind`: the
/// `From<TransportKind> for EdgeKind` conversion below is the
/// single bridge between the two enums, so adding a new transport
/// (KDC2-7 deferred BLE / LoRa / Matrix variants, future
/// post-quantum transport, etc.) means edits in exactly two
/// places — the `TransportKind` enum and this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// Best-case: direct UDP between two peers' Nebula sockets
    /// (hole-punched via the lighthouse).
    NebulaDirect,
    /// Relayed through a Nebula lighthouse/relay node (used when
    /// direct UDP fails NAT traversal).
    NebulaLighthouseRelay,
    /// HTTPS-tunneled fallback over TCP/443 (per 12.18).
    NebulaHttps443,
    /// KDE Connect wire over TLS (KDC2-1, v2.1 lock 2026-05-22).
    /// Used for phone↔peer + peer↔peer-via-KDC paths once
    /// `mde-kdc` (KDC2-3) lands. The reconciler renders KDC edges
    /// the same way it renders DERP edges in the diff overlay.
    KdcTls,
}

impl From<mackes_transport::TransportKind> for EdgeKind {
    /// Bridge `mackes_transport`'s enum into the topology engine's
    /// enum. Every variant maps 1:1 — if this match becomes
    /// non-exhaustive a `TransportKind` was added without the
    /// corresponding `EdgeKind`.
    fn from(t: mackes_transport::TransportKind) -> Self {
        match t {
            mackes_transport::TransportKind::NebulaDirect => EdgeKind::NebulaDirect,
            mackes_transport::TransportKind::NebulaLighthouseRelay => {
                EdgeKind::NebulaLighthouseRelay
            }
            mackes_transport::TransportKind::NebulaHttps443 => EdgeKind::NebulaHttps443,
            mackes_transport::TransportKind::KdcTls => EdgeKind::KdcTls,
        }
    }
}

/// Snapshot value passed into `calculate()`. Pure-function input —
/// the engine doesn't read from the SQL store directly so tests can
/// build fixtures by hand.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DesiredSnapshot {
    /// Every known node, ordered by `id` for determinism.
    pub nodes: Vec<Node>,
    /// Allow-list of region pairs that may peer east-west. Empty
    /// means "fully connected" (every healthy node peers with every
    /// other healthy node).
    pub allow_east_west: Vec<(String, String)>,
    /// v2.0.0 Phase G.1 — fleet-managed settings. Each (key,
    /// value_json) pair is applied on every targeted peer by the
    /// reconcile loop via `settings::apply()`. JSON-serialized
    /// values so the snapshot stays self-contained without coupling
    /// to the SettingValue enum. Empty when the snapshot doesn't
    /// touch settings.
    #[serde(default)]
    pub settings_keys: Vec<(String, String)>,

    /// VV-2.a (v4.0) — approved `Policy::VoiceMesh` +
    /// `Policy::VoicePublic` revisions. The voice materializer
    /// hook in the reconciler tick consumes these to rebuild
    /// `/var/lib/mackesd/voice-desired.json` whenever the set
    /// changes; `voice_config` then sees the mtime advance and
    /// reloads `kamailio-mde` + `rtpengine-mde`. Default-empty
    /// for backward compat with the v3.x `DesiredSnapshot`
    /// shape that pre-dated voice routing.
    #[serde(default)]
    pub voice_policies: Vec<Policy>,
}

/// Output of `calculate()` — what the reconciler tries to make
/// reality match.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TopologySnapshot {
    /// All expected edges, ordered.
    pub edges: BTreeSet<Edge>,
    /// Per-node route table: `node_id → (peer_id → next_hop_id)`.
    /// Empty `next_hop` means direct.
    pub routes: BTreeMap<String, BTreeMap<String, String>>,
}

/// Pure function: given a desired snapshot, emit the expected
/// adjacency + route layout. No I/O, no shared state — fully
/// deterministic for the same input.
#[must_use]
pub fn calculate(snapshot: &DesiredSnapshot) -> TopologySnapshot {
    let mut edges = BTreeSet::new();
    let mut routes: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();

    let healthy: Vec<&Node> = snapshot.nodes.iter().filter(|n| n.healthy).collect();

    // Edge generation: every healthy pair gets an edge if the
    // east-west policy allows it (or if no allow list is set).
    for (i, a) in healthy.iter().enumerate() {
        for b in &healthy[i + 1..] {
            if !east_west_allowed(snapshot, &a.region, &b.region) {
                continue;
            }
            let (lo, hi) = order_pair(&a.id, &b.id);
            edges.insert(Edge {
                a: lo.to_owned(),
                b: hi.to_owned(),
                kind: EdgeKind::NebulaDirect,
            });
        }
    }

    // Route table: each node sees every other healthy node as either
    // a direct neighbor (edge exists between them) or a relay hop
    // through the first Host node in lexicographic order.
    let host = healthy.iter().find(|n| n.is_host).map(|n| n.id.clone());
    for n in &healthy {
        let mut table = BTreeMap::new();
        for other in &healthy {
            if other.id == n.id {
                continue;
            }
            let (lo, hi) = order_pair(&n.id, &other.id);
            let direct = edges.contains(&Edge {
                a: lo.to_owned(),
                b: hi.to_owned(),
                kind: EdgeKind::NebulaDirect,
            });
            let next_hop = if direct {
                String::new()
            } else {
                host.clone().unwrap_or_default()
            };
            table.insert(other.id.clone(), next_hop);
        }
        routes.insert(n.id.clone(), table);
    }

    TopologySnapshot { edges, routes }
}

/// Diff two topology snapshots — used by the GUI's "Diff" overlay
/// mode (12.9.3) and the reconciler's drift detector (12.5.1).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TopologyDiff {
    /// Edges in `desired` but not in `actual` — these should exist
    /// but don't. Renders red in the panel.
    pub missing: BTreeSet<Edge>,
    /// Edges in `actual` but not in `desired` — exist but
    /// shouldn't. Renders amber.
    pub extra: BTreeSet<Edge>,
    /// Edges present in both. Renders normally.
    pub healthy: BTreeSet<Edge>,
}

/// Compute the set-difference + intersection of two topology
/// snapshots. Pure function over the inputs.
#[must_use]
pub fn diff(desired: &TopologySnapshot, actual: &TopologySnapshot) -> TopologyDiff {
    let missing: BTreeSet<Edge> = desired.edges.difference(&actual.edges).cloned().collect();
    let extra: BTreeSet<Edge> = actual.edges.difference(&desired.edges).cloned().collect();
    let healthy: BTreeSet<Edge> = desired.edges.intersection(&actual.edges).cloned().collect();
    TopologyDiff {
        missing,
        extra,
        healthy,
    }
}

/// Tie-break two candidate paths by health then latency (Phase
/// 12.4.3). Returns `Ordering::Less` when `a` is the better path.
/// Pure function so the reconciler's route selection is testable.
#[must_use]
pub fn rank_paths(
    a_healthy: bool,
    a_rtt_ms: Option<u32>,
    b_healthy: bool,
    b_rtt_ms: Option<u32>,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    // Healthy paths always beat unhealthy.
    if a_healthy != b_healthy {
        return if a_healthy {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    // Both healthy (or both unhealthy) — prefer lower latency. A
    // measured latency beats an unmeasured one (better signal).
    match (a_rtt_ms, b_rtt_ms) {
        (Some(a), Some(b)) => a.cmp(&b),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn east_west_allowed(snapshot: &DesiredSnapshot, a_region: &str, b_region: &str) -> bool {
    if snapshot.allow_east_west.is_empty() {
        return true;
    }
    snapshot.allow_east_west.iter().any(|(from, to)| {
        (from == a_region && to == b_region) || (from == b_region && to == a_region)
    })
}

fn order_pair<'a>(a: &'a str, b: &'a str) -> (&'a str, &'a str) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, region: &str, healthy: bool, is_host: bool) -> Node {
        Node {
            id: id.to_owned(),
            region: region.to_owned(),
            healthy,
            is_host,
        }
    }

    #[test]
    fn empty_snapshot_yields_no_edges() {
        let snap = DesiredSnapshot::default();
        let topo = calculate(&snap);
        assert!(topo.edges.is_empty());
        assert!(topo.routes.is_empty());
    }

    #[test]
    fn full_mesh_of_three_healthy_nodes() {
        let snap = DesiredSnapshot {
            nodes: vec![
                node("peer:a", "us-east", true, true),
                node("peer:b", "us-east", true, false),
                node("peer:c", "us-east", true, false),
            ],
            allow_east_west: vec![],
            settings_keys: vec![],
            voice_policies: vec![],
        };
        let topo = calculate(&snap);
        assert_eq!(topo.edges.len(), 3);
        assert_eq!(topo.routes.len(), 3);
        for (_, table) in &topo.routes {
            assert_eq!(table.len(), 2);
        }
    }

    #[test]
    fn unhealthy_node_is_excluded() {
        let snap = DesiredSnapshot {
            nodes: vec![
                node("peer:a", "r", true, true),
                node("peer:b", "r", false, false),
                node("peer:c", "r", true, false),
            ],
            allow_east_west: vec![],
            settings_keys: vec![],
            voice_policies: vec![],
        };
        let topo = calculate(&snap);
        assert_eq!(topo.edges.len(), 1);
    }

    #[test]
    fn east_west_policy_blocks_disallowed_pairs() {
        let snap = DesiredSnapshot {
            nodes: vec![
                node("peer:a", "us-east", true, true),
                node("peer:b", "us-west", true, false),
                node("peer:c", "us-east", true, false),
            ],
            allow_east_west: vec![("us-east".into(), "us-east".into())],
            settings_keys: vec![],
            voice_policies: vec![],
        };
        let topo = calculate(&snap);
        assert_eq!(topo.edges.len(), 1);
        let only_edge = topo.edges.iter().next().unwrap();
        assert!(only_edge.a.contains("peer:a") || only_edge.a.contains("peer:c"));
    }

    #[test]
    fn diff_missing_extra_healthy_sets() {
        let mk = |pairs: &[(&str, &str)]| TopologySnapshot {
            edges: pairs
                .iter()
                .map(|(a, b)| Edge {
                    a: (*a).to_owned(),
                    b: (*b).to_owned(),
                    kind: EdgeKind::NebulaDirect,
                })
                .collect(),
            routes: BTreeMap::new(),
        };
        let desired = mk(&[("a", "b"), ("b", "c"), ("c", "d")]);
        let actual = mk(&[("b", "c"), ("c", "d"), ("d", "e")]);
        let d = diff(&desired, &actual);
        assert_eq!(d.missing.len(), 1);
        assert_eq!(d.extra.len(), 1);
        assert_eq!(d.healthy.len(), 2);
    }

    #[test]
    fn order_pair_lexicographic() {
        assert_eq!(order_pair("b", "a"), ("a", "b"));
        assert_eq!(order_pair("a", "b"), ("a", "b"));
        assert_eq!(order_pair("a", "a"), ("a", "a"));
    }

    #[test]
    fn rank_paths_healthy_beats_unhealthy() {
        use std::cmp::Ordering;
        assert_eq!(rank_paths(true, Some(50), false, Some(10)), Ordering::Less);
        assert_eq!(
            rank_paths(false, Some(10), true, Some(50)),
            Ordering::Greater
        );
    }

    #[test]
    fn rank_paths_lower_latency_wins_when_both_healthy() {
        use std::cmp::Ordering;
        assert_eq!(rank_paths(true, Some(10), true, Some(50)), Ordering::Less);
        assert_eq!(
            rank_paths(true, Some(50), true, Some(10)),
            Ordering::Greater
        );
        assert_eq!(rank_paths(true, Some(20), true, Some(20)), Ordering::Equal);
    }

    #[test]
    fn rank_paths_measured_beats_unmeasured() {
        use std::cmp::Ordering;
        assert_eq!(rank_paths(true, Some(50), true, None), Ordering::Less);
        assert_eq!(rank_paths(true, None, true, Some(50)), Ordering::Greater);
    }

    #[test]
    fn rank_paths_both_unmeasured_is_equal() {
        use std::cmp::Ordering;
        assert_eq!(rank_paths(true, None, true, None), Ordering::Equal);
        assert_eq!(rank_paths(false, None, false, None), Ordering::Equal);
    }

    #[test]
    fn rank_paths_both_unhealthy_falls_to_latency() {
        use std::cmp::Ordering;
        assert_eq!(rank_paths(false, Some(10), false, Some(50)), Ordering::Less);
    }

    #[test]
    fn east_west_allow_list_is_symmetric() {
        // Allow `us-east → us-west` must also accept `us-west → us-east`
        // pair directions when classifying nodes.
        let snap = DesiredSnapshot {
            nodes: vec![
                node("peer:a", "us-east", true, true),
                node("peer:b", "us-west", true, false),
            ],
            allow_east_west: vec![("us-east".into(), "us-west".into())],
            settings_keys: vec![],
            voice_policies: vec![],
        };
        let topo = calculate(&snap);
        assert_eq!(topo.edges.len(), 1);
    }

    #[test]
    fn route_table_falls_back_to_host_when_no_direct_edge() {
        // Two nodes in different regions with no east-west allow list
        // would have no direct edge. The host fallback should populate
        // the route table with the host's id as next_hop.
        let snap = DesiredSnapshot {
            nodes: vec![
                node("peer:host", "us-east", true, true),
                node("peer:a", "us-east", true, false),
                node("peer:b", "us-west", true, false),
            ],
            // Allow only us-east <-> us-east — peer:b can't peer directly.
            allow_east_west: vec![("us-east".into(), "us-east".into())],
            settings_keys: vec![],
            voice_policies: vec![],
        };
        let topo = calculate(&snap);
        // Route from peer:a to peer:b goes via the Host.
        let a_table = &topo.routes["peer:a"];
        let nh = &a_table["peer:b"];
        assert_eq!(nh, "peer:host");
        // Direct route from peer:a to peer:host has empty next_hop.
        let direct = &a_table["peer:host"];
        assert!(direct.is_empty());
    }

    #[test]
    fn route_table_empty_when_no_host_available() {
        // No is_host=true node exists — next_hop falls back to empty.
        let snap = DesiredSnapshot {
            nodes: vec![
                node("peer:a", "us-east", true, false),
                node("peer:b", "us-west", true, false),
            ],
            allow_east_west: vec![("us-east".into(), "us-east".into())],
            settings_keys: vec![],
            voice_policies: vec![],
        };
        let topo = calculate(&snap);
        let table = &topo.routes["peer:a"];
        // peer:b is not reachable directly; next_hop must be "" since
        // no host is available.
        assert_eq!(table.get("peer:b"), Some(&String::new()));
    }

    #[test]
    fn diff_identical_snapshots_has_only_healthy_edges() {
        let mk = |pairs: &[(&str, &str)]| TopologySnapshot {
            edges: pairs
                .iter()
                .map(|(a, b)| Edge {
                    a: (*a).to_owned(),
                    b: (*b).to_owned(),
                    kind: EdgeKind::NebulaDirect,
                })
                .collect(),
            routes: BTreeMap::new(),
        };
        let s = mk(&[("a", "b"), ("c", "d")]);
        let d = diff(&s, &s);
        assert!(d.missing.is_empty());
        assert!(d.extra.is_empty());
        assert_eq!(d.healthy.len(), 2);
    }

    #[test]
    fn edge_kinds_distinguish_separate_transports() {
        // Two edges over the same pair but different kinds are kept
        // separate via Ord on (a, b, kind).
        let e1 = Edge {
            a: "a".into(),
            b: "b".into(),
            kind: EdgeKind::NebulaDirect,
        };
        let e2 = Edge {
            a: "a".into(),
            b: "b".into(),
            kind: EdgeKind::NebulaLighthouseRelay,
        };
        let e3 = Edge {
            a: "a".into(),
            b: "b".into(),
            kind: EdgeKind::NebulaHttps443,
        };
        let mut set = BTreeSet::new();
        set.insert(e1);
        set.insert(e2);
        set.insert(e3);
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn edge_kind_serializes_snake_case() {
        // Wire compatibility across the panel/daemon split — these
        // snake_case strings are the ABI.
        let k1 = serde_json::to_string(&EdgeKind::NebulaDirect).unwrap();
        let k2 = serde_json::to_string(&EdgeKind::NebulaLighthouseRelay).unwrap();
        let k3 = serde_json::to_string(&EdgeKind::NebulaHttps443).unwrap();
        let k4 = serde_json::to_string(&EdgeKind::KdcTls).unwrap();
        assert_eq!(k1, "\"nebula_direct\"");
        assert_eq!(k2, "\"nebula_lighthouse_relay\"");
        assert_eq!(k3, "\"nebula_https443\"");
        assert_eq!(k4, "\"kdc_tls\"");
    }

    #[test]
    fn transport_kind_into_edge_kind_is_total_and_token_aligned() {
        // KDC2-1 lock: every TransportKind has a 1:1 EdgeKind. The
        // serde tokens MUST match between the two enums so audit
        // chain readers + the operator's topology diff overlay
        // (12.9.3) report the same string regardless of which
        // enum they encounter the variant through.
        for t in mackes_transport::TransportKind::all() {
            let e: EdgeKind = t.into();
            let t_tok = serde_json::to_string(&t).unwrap();
            let e_tok = serde_json::to_string(&e).unwrap();
            assert_eq!(
                t_tok, e_tok,
                "serde token drift between TransportKind::{t:?} and EdgeKind::{e:?}",
            );
        }
    }

    #[test]
    fn order_pair_equal_inputs() {
        assert_eq!(order_pair("z", "z"), ("z", "z"));
    }

    #[test]
    fn east_west_disallowed_returns_no_edge() {
        let snap = DesiredSnapshot {
            nodes: vec![
                node("peer:a", "us-east", true, true),
                node("peer:b", "us-west", true, false),
            ],
            // Allow list set but empty for this pair → block.
            allow_east_west: vec![("us-east".into(), "us-east".into())],
            settings_keys: vec![],
            voice_policies: vec![],
        };
        let topo = calculate(&snap);
        assert!(topo.edges.is_empty());
    }
}
