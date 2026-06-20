//! ROUTE-TRACE-1 — the typed **PathGraph** result of `action/route/trace`
//! (design: `docs/design/route-trace.md`).
//!
//! A PathGraph is the logical path between two endpoints — a topology of typed
//! **nodes** (endpoints + waypoints: host/VM/container, overlay peers, the VPN
//! gateway + exit, the lighthouse ingress, the internet cloud, the service) and
//! **edges** (links carrying the per-segment layer + transport + RTT/loss + an
//! optional firewall/control verdict). The mackesd responder assembles it from
//! real state (compute/service inventory, routing/netstate, VPN-GW, the CONNECT
//! ingress mapping) and the GUI renders it; this crate is the **shared model +
//! the pure assembly/derivation logic** both sides agree on.
//!
//! Pure + serde — no I/O. The state-gathering lives in `mackesd`; here we keep
//! the types, the builder, and the derivations (`blocked_at`, edge validation)
//! that are identical regardless of how the segments were measured.

use serde::{Deserialize, Serialize};

/// Which way the path is traced (lock #2 — a selectable perspective).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Direction {
    /// A mesh node → an external destination, through gateway + tunnel.
    #[default]
    Egress,
    /// An external client → a published service, through the lighthouse ingress.
    Ingress,
}

/// What a path node represents.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NodeKind {
    /// A host-level endpoint (a mesh node).
    #[default]
    Host,
    /// A libvirt/KVM virtual machine.
    Vm,
    /// A Podman container.
    Container,
    /// Another node on the Nebula overlay (a waypoint).
    OverlayPeer,
    /// The VPN egress gateway node.
    Gateway,
    /// The VPN provider's exit (the public egress IP).
    VpnExit,
    /// The lighthouse reverse-proxy ingress.
    Ingress,
    /// The public internet cloud (an opaque waypoint).
    Internet,
    /// A published service (the destination of an ingress trace / a mesh service).
    Service,
}

/// The network layer an edge crosses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Layer {
    /// Local to a host (loopback / host↔its-VM/container).
    #[default]
    Host,
    /// The Nebula overlay (peer↔peer).
    Mesh,
    /// A VPN tunnel (gateway↔exit).
    Vpn,
    /// The public internet.
    Public,
}

/// How an edge's traffic is carried.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Transport {
    /// A direct Nebula overlay tunnel (hole-punched).
    #[default]
    DirectOverlay,
    /// A relayed Nebula overlay path (via a lighthouse).
    RelayOverlay,
    /// A WireGuard/OpenVPN tunnel.
    VpnTunnel,
    /// Plain public-internet routing.
    Public,
    /// On-host loopback (host ↔ its own VM/container/service).
    Loopback,
}

/// A firewall/control verdict on an edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    /// The control point permits this segment.
    #[default]
    Allow,
    /// The control point denies this segment (the path is blocked here).
    Block,
}

/// A control point an edge crosses (a firewall / kill-switch) + its verdict.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlPoint {
    /// Which firewall/control evaluated this segment (e.g. `firewalld:public`).
    pub firewall: String,
    /// Allow or block.
    pub verdict: Verdict,
    /// The matching rule, cited (e.g. `--add-port 443/tcp` / `default deny`).
    pub rule: String,
}

/// One node (endpoint or waypoint) in the path.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathNode {
    /// Stable id, referenced by edges' `from`/`to`.
    pub id: String,
    /// What this node is.
    pub kind: NodeKind,
    /// Human label (hostname / service name / "Internet" / the exit region).
    pub label: String,
    /// LAN/host IP, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_ip: Option<String>,
    /// Nebula overlay IP, if on the overlay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlay_ip: Option<String>,
    /// Public IP, if applicable (the VPN exit, the ingress lighthouse).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_ip: Option<String>,
    /// Public DNS name, if published (DDNS / the ingress hostname).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns_name: Option<String>,
    /// The node hosting this endpoint (for a VM/container/service).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hosting_node: Option<String>,
}

/// One edge (segment) linking two nodes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PathEdge {
    /// Source node id.
    pub from: String,
    /// Destination node id.
    pub to: String,
    /// The layer this segment crosses.
    pub layer: Layer,
    /// How the segment is carried.
    pub transport: Transport,
    /// Measured round-trip time (ms), when a probe ran for this segment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtt_ms: Option<f64>,
    /// Measured loss fraction 0.0..=1.0, when probed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loss: Option<f64>,
    /// The firewall/control verdict on this segment, if it crosses one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control: Option<ControlPoint>,
}

impl PathEdge {
    /// True when a control point on this edge denies the segment.
    #[must_use]
    pub fn is_blocked(&self) -> bool {
        self.control
            .as_ref()
            .is_some_and(|c| c.verdict == Verdict::Block)
    }

    /// A stable edge id (`<from>-><to>`) for `blocked_at` references.
    #[must_use]
    pub fn id(&self) -> String {
        format!("{}->{}", self.from, self.to)
    }
}

/// The typed path between two endpoints.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PathGraph {
    /// Which perspective this trace was built from.
    pub direction: Direction,
    /// Endpoints + waypoints.
    pub nodes: Vec<PathNode>,
    /// Segments linking them, source→dest order.
    pub edges: Vec<PathEdge>,
    /// The id (`<from>-><to>`) of the first denying control point, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_at: Option<String>,
}

impl PathGraph {
    /// Start an empty graph for `direction`.
    #[must_use]
    pub fn new(direction: Direction) -> Self {
        Self {
            direction,
            ..Default::default()
        }
    }

    /// Append a node (builder style).
    #[must_use]
    pub fn with_node(mut self, node: PathNode) -> Self {
        self.nodes.push(node);
        self
    }

    /// Append an edge (builder style).
    #[must_use]
    pub fn with_edge(mut self, edge: PathEdge) -> Self {
        self.edges.push(edge);
        self
    }

    /// Re-derive [`Self::blocked_at`] from the edges — the id of the FIRST edge
    /// (source→dest order) whose control point blocks. Call after assembling all
    /// edges. Returns the blocked edge id (also stored).
    pub fn recompute_blocked_at(&mut self) -> Option<String> {
        self.blocked_at = self.edges.iter().find(|e| e.is_blocked()).map(PathEdge::id);
        self.blocked_at.clone()
    }

    /// True when the path reaches the destination unblocked.
    #[must_use]
    pub fn is_reachable(&self) -> bool {
        self.blocked_at.is_none()
    }

    /// Validate that every edge references nodes that exist — a self-consistency
    /// check the assembler must satisfy before returning a graph (a dangling
    /// edge would render as a floating segment). Returns the first offending
    /// `(edge-id, missing-node-id)`.
    ///
    /// # Errors
    /// `(edge_id, missing_node_id)` for the first edge with an unknown endpoint.
    pub fn validate(&self) -> Result<(), (String, String)> {
        let ids: std::collections::HashSet<&str> =
            self.nodes.iter().map(|n| n.id.as_str()).collect();
        for e in &self.edges {
            if !ids.contains(e.from.as_str()) {
                return Err((e.id(), e.from.clone()));
            }
            if !ids.contains(e.to.as_str()) {
                return Err((e.id(), e.to.clone()));
            }
        }
        Ok(())
    }

    /// Serialize to a JSON string (the `action/route/trace` reply body).
    ///
    /// # Errors
    /// A serde JSON error.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, kind: NodeKind) -> PathNode {
        PathNode {
            id: id.into(),
            kind,
            label: id.into(),
            ..Default::default()
        }
    }

    fn edge(from: &str, to: &str, layer: Layer, transport: Transport) -> PathEdge {
        PathEdge {
            from: from.into(),
            to: to.into(),
            layer,
            transport,
            ..Default::default()
        }
    }

    #[test]
    fn egress_path_round_trips_through_json() {
        let g = PathGraph::new(Direction::Egress)
            .with_node(node("eagle", NodeKind::Host))
            .with_node(node("gw", NodeKind::Gateway))
            .with_node(node("exit", NodeKind::VpnExit))
            .with_node(node("net", NodeKind::Internet))
            .with_edge(edge("eagle", "gw", Layer::Mesh, Transport::DirectOverlay))
            .with_edge(edge("gw", "exit", Layer::Vpn, Transport::VpnTunnel))
            .with_edge(edge("exit", "net", Layer::Public, Transport::Public));
        let json = g.to_json().unwrap();
        let back: PathGraph = serde_json::from_str(&json).unwrap();
        assert_eq!(back, g);
        assert_eq!(back.direction, Direction::Egress);
        assert_eq!(back.nodes.len(), 4);
        assert_eq!(back.edges.len(), 3);
        // kebab-case on the wire.
        assert!(json.contains("\"direct-overlay\""));
        assert!(json.contains("\"vpn-tunnel\""));
    }

    #[test]
    fn blocked_at_is_the_first_denying_edge() {
        let mut g = PathGraph::new(Direction::Ingress)
            .with_node(node("net", NodeKind::Internet))
            .with_node(node("lh", NodeKind::Ingress))
            .with_node(node("svc", NodeKind::Service))
            .with_edge(edge("net", "lh", Layer::Public, Transport::Public))
            .with_edge(edge("lh", "svc", Layer::Mesh, Transport::DirectOverlay));
        // No control points yet → reachable.
        assert_eq!(g.recompute_blocked_at(), None);
        assert!(g.is_reachable());
        // Deny the ingress edge.
        g.edges[0].control = Some(ControlPoint {
            firewall: "firewalld:public".into(),
            verdict: Verdict::Block,
            rule: "default deny (no exposure policy)".into(),
        });
        assert_eq!(g.recompute_blocked_at().as_deref(), Some("net->lh"));
        assert!(!g.is_reachable());
    }

    #[test]
    fn blocked_at_picks_the_earliest_of_multiple_blocks() {
        let mut g = PathGraph::new(Direction::Egress)
            .with_node(node("a", NodeKind::Host))
            .with_node(node("b", NodeKind::Gateway))
            .with_node(node("c", NodeKind::VpnExit))
            .with_edge(edge("a", "b", Layer::Mesh, Transport::DirectOverlay))
            .with_edge(edge("b", "c", Layer::Vpn, Transport::VpnTunnel));
        let block = |rule: &str| {
            Some(ControlPoint {
                firewall: "fw".into(),
                verdict: Verdict::Block,
                rule: rule.into(),
            })
        };
        g.edges[0].control = block("first");
        g.edges[1].control = block("second");
        assert_eq!(g.recompute_blocked_at().as_deref(), Some("a->b"));
    }

    #[test]
    fn validate_catches_a_dangling_edge() {
        let g = PathGraph::new(Direction::Egress)
            .with_node(node("a", NodeKind::Host))
            .with_edge(edge("a", "ghost", Layer::Mesh, Transport::DirectOverlay));
        let err = g.validate().unwrap_err();
        assert_eq!(err, ("a->ghost".to_string(), "ghost".to_string()));
        // A consistent graph validates.
        let ok = PathGraph::new(Direction::Egress)
            .with_node(node("a", NodeKind::Host))
            .with_node(node("b", NodeKind::Host))
            .with_edge(edge("a", "b", Layer::Mesh, Transport::DirectOverlay));
        assert!(ok.validate().is_ok());
    }

    #[test]
    fn edge_id_and_is_blocked() {
        let mut e = edge("x", "y", Layer::Public, Transport::Public);
        assert_eq!(e.id(), "x->y");
        assert!(!e.is_blocked());
        e.control = Some(ControlPoint {
            firewall: "f".into(),
            verdict: Verdict::Allow,
            rule: "ok".into(),
        });
        assert!(!e.is_blocked());
        e.control.as_mut().unwrap().verdict = Verdict::Block;
        assert!(e.is_blocked());
    }
}
