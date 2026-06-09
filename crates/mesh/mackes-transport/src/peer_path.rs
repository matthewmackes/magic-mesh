//! KDC2-1.3 — `PeerPath` per-peer router state.
//!
//! The mesh router (KDC2-1.8) holds one `PeerPath` per known
//! peer. It tracks which transport is currently primary for that
//! peer, which is fallback, when the last switch happened, and
//! why. Per-message-class overrides let an operator pin (for
//! example) a `FileBulk` transport to KdcTls even when the
//! router's health-based default would pick NebulaDirect.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{MessageClass, TransportKind};

/// Phase 12.18 — per-peer HTTPS-tunnel activation state.
/// The mesh-router observes probe outcomes per peer + feeds them
/// into a per-peer `FailureWindow` (re-exported from
/// `mackesd::https_fallback`). When the failure window meets the
/// 3-cycle threshold, this state advances from `Inactive` to
/// `Activating`; once the NebulaHttps443 transport reports a successful
/// TLS handshake, to `Active`. A fresh direct-UDP-or-DERP success
/// snaps it back to `Inactive`.
///
/// Mirrors `mackesd::https_fallback::HttpsFallbackState` 1:1 so
/// callers don't have to thread the mackesd type through the
/// `mackes-transport` crate (which sits one level lower in the
/// dep graph). The transition rules live in
/// `mackesd::https_fallback::transition`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HttpsFallbackState {
    /// Default — direct-UDP / DERP-UDP paths are healthy.
    #[default]
    Inactive,
    /// Failure threshold met; TLS handshake to the configured
    /// fallback host in flight.
    Activating,
    /// Tunnel up + carrying traffic.
    Active,
    /// Active tunnel's TLS or TCP layer broke; revert to
    /// failure-window evaluation.
    Failing,
}

impl HttpsFallbackState {
    /// `true` when the routing layer should treat the HTTPS
    /// tunnel as the active path.
    #[must_use]
    pub fn is_active(self) -> bool {
        matches!(self, Self::Active)
    }

    /// `true` when the UI should surface the "connecting via
    /// HTTPS…" toast.
    #[must_use]
    pub fn is_activating(self) -> bool {
        matches!(self, Self::Activating)
    }
}

/// One ICE-style candidate sourced from STUN (Phase 12.17) for a
/// peer. Tailscale's WireGuard endpoint set is seeded with the
/// `reflexive` address ahead of its own NAT-traversal probe so
/// symmetric-NAT edges find a hole-punch path inside the 1.5 s
/// candidate-gather budget locked by Q8 in
/// `docs/design/v12-connectivity-scope.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StunCandidate {
    /// The peer-side address as reported by the STUN server's
    /// `XOR-MAPPED-ADDRESS` attribute. This is the address other
    /// peers should try first when reaching this peer.
    pub reflexive: SocketAddr,
    /// The STUN server that produced this candidate. Useful for
    /// audit + diagnostic logs when multiple servers respond.
    pub server: SocketAddr,
    /// SystemTime when the candidate was last refreshed. Stale
    /// candidates (older than the Q13 backoff-cap, 5 min) get
    /// pruned by the gather worker.
    pub observed_at: SystemTime,
}

/// Reasons the router switched from one transport to another.
/// Every `PathSwitch` audit-log entry carries one of these.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwitchReason {
    /// First-ever path selection for this peer.
    Initial,
    /// The previously-primary transport health degraded; carries
    /// the transport that got bumped.
    HealthDegraded(TransportKind),
    /// Operator policy in `/etc/mde/connect/policy.toml`
    /// explicitly preferred this transport.
    Policy,
    /// Operator manually pinned the path (e.g. via
    /// `mde-kdc pin NebulaDirect peer-id`).
    ManualOverride,
    /// Router observed flapping (rapid back-and-forth) on the
    /// previous transport and applied a 5-minute cooldown
    /// before allowing it back as primary.
    FlapPenalty,
    /// KDC2-4.5 — phone went off-LAN; mesh-shunt activated,
    /// router now reaches the phone via a neighbor MDE peer's
    /// KDC channel. Distinct from `HealthDegraded(NebulaDirect)`
    /// so the audit log + operator UI can show "via peer-A"
    /// instead of "direct UDP lost."
    MeshShuntActivated,
    /// KDC2-4.5 — phone re-appeared on the local LAN; router
    /// is back on the direct path. Pairs with
    /// `MeshShuntActivated` so the audit log captures the
    /// complete roaming cycle.
    DirectLanRecovered,
}

impl SwitchReason {
    /// Stable audit-token suffix. The full audit chain entry
    /// reads `path_switch reason=<token> ...`. The token for
    /// `HealthDegraded(TransportKind)` includes the transport
    /// suffix so the reader can grep for `reason=health_degraded_kdc_tls`.
    #[must_use]
    pub fn audit_token(&self) -> String {
        match self {
            SwitchReason::Initial => "initial".to_string(),
            SwitchReason::HealthDegraded(t) => format!("health_degraded_{}", t.as_str()),
            SwitchReason::Policy => "policy".to_string(),
            SwitchReason::ManualOverride => "manual_override".to_string(),
            SwitchReason::FlapPenalty => "flap_penalty".to_string(),
            SwitchReason::MeshShuntActivated => "mesh_shunt_activated".to_string(),
            SwitchReason::DirectLanRecovered => "direct_lan_recovered".to_string(),
        }
    }
}

/// One peer's current routing state.
///
/// Note: `health_score` is `f32` so this struct cannot derive
/// `Eq` (f32's NaN breaks reflexive equality). `PartialEq` is
/// sufficient for the router's internal usage; tests that need
/// strict equality compare individual fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeerPath {
    /// Peer identifier (mesh peer-id / KDC device-id depending
    /// on context).
    pub peer_id: String,
    /// Currently-primary transport — the router sends here first.
    pub primary: TransportKind,
    /// Best-known fallback. `None` when no probed alternative
    /// exists yet.
    pub fallback: Option<TransportKind>,
    /// When the most recent switch happened (None for paths
    /// that have never switched since `Initial`).
    pub last_switch_at: Option<SystemTime>,
    /// Why the most recent switch happened.
    pub last_switch_reason: SwitchReason,
    /// Composite health score 0.0..=1.0 of the current `primary`
    /// transport. Used by the mesh-router tie-break.
    pub health_score: f32,
    /// Per-message-class overrides. When a class is in this map,
    /// the router uses the mapped TransportKind for that class
    /// regardless of `primary` / health.
    pub message_class_overrides: BTreeMap<MessageClass, TransportKind>,
    /// Phase 12.17 — STUN candidates the router can hand to
    /// Tailscale's endpoint set so symmetric-NAT edges
    /// hole-punch before falling through to DERP. The
    /// `kdc_stun_gather` worker (`mackesd::workers::stun_gather`)
    /// populates this list; the mesh-router consults it on each
    /// tick when picking the primary.
    ///
    /// Default empty so existing tests + non-symmetric peers
    /// see no behavior change.
    #[serde(default)]
    pub candidates: Vec<StunCandidate>,
    /// Phase 12.18 — per-peer consecutive UDP-failure count.
    /// The mesh-router increments this on each tick when both
    /// direct-UDP and DERP-UDP probes fail; resets to 0 when
    /// either succeeds. Crossing the 3-cycle threshold flips
    /// `https_state` to `Activating`. Plain u32 (instead of the
    /// richer `FailureWindow`) so the field is serde-friendly +
    /// the transition logic remains in the mackesd crate.
    #[serde(default)]
    pub consecutive_udp_failures: u32,
    /// Phase 12.18 — current HTTPS-tunnel activation state.
    /// Advanced by the mesh-router via the
    /// `mackesd::https_fallback::transition` table.
    #[serde(default)]
    pub https_state: HttpsFallbackState,
}

impl PeerPath {
    /// Construct a fresh `PeerPath` with `Initial` switch reason.
    /// Used by the router when it first sees a peer.
    #[must_use]
    pub fn initial(peer_id: String, primary: TransportKind) -> Self {
        Self {
            peer_id,
            primary,
            fallback: None,
            last_switch_at: None,
            last_switch_reason: SwitchReason::Initial,
            health_score: 1.0,
            message_class_overrides: BTreeMap::new(),
            candidates: Vec::new(),
            consecutive_udp_failures: 0,
            https_state: HttpsFallbackState::Inactive,
        }
    }

    /// Phase 12.17 — replace this peer's STUN candidate list with
    /// freshly-gathered values from the `stun_gather` worker.
    /// Sorted-by-reflexive so the audit log + router's tie-
    /// breaker stays deterministic.
    pub fn set_candidates(&mut self, mut candidates: Vec<StunCandidate>) {
        candidates.sort_by_key(|c| c.reflexive);
        self.candidates = candidates;
    }

    /// Resolve the transport for a specific message class.
    /// Honors `message_class_overrides` first; falls back to
    /// `primary` otherwise.
    #[must_use]
    pub fn transport_for(&self, class: MessageClass) -> TransportKind {
        self.message_class_overrides
            .get(&class)
            .copied()
            .unwrap_or(self.primary)
    }
}

// MessageClass needs Ord for use as a BTreeMap key.
impl Ord for MessageClass {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (*self as u8).cmp(&(*other as u8))
    }
}

impl PartialOrd for MessageClass {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_path_initial_construction_sets_expected_defaults() {
        let p = PeerPath::initial("peer-A".into(), TransportKind::NebulaDirect);
        assert_eq!(p.peer_id, "peer-A");
        assert_eq!(p.primary, TransportKind::NebulaDirect);
        assert_eq!(p.fallback, None);
        assert_eq!(p.last_switch_at, None);
        assert_eq!(p.last_switch_reason, SwitchReason::Initial);
        assert!((p.health_score - 1.0).abs() < 1e-6);
        assert!(p.message_class_overrides.is_empty());
    }

    #[test]
    fn transport_for_returns_primary_when_no_override() {
        let p = PeerPath::initial("p".into(), TransportKind::NebulaDirect);
        for class in [
            MessageClass::Control,
            MessageClass::Clipboard,
            MessageClass::FileBulk,
            MessageClass::Notification,
        ] {
            assert_eq!(p.transport_for(class), TransportKind::NebulaDirect);
        }
    }

    #[test]
    fn transport_for_honors_override() {
        let mut p = PeerPath::initial("p".into(), TransportKind::NebulaDirect);
        p.message_class_overrides
            .insert(MessageClass::FileBulk, TransportKind::KdcTls);
        assert_eq!(
            p.transport_for(MessageClass::FileBulk),
            TransportKind::KdcTls,
        );
        // Other classes still fall through to primary.
        assert_eq!(
            p.transport_for(MessageClass::Clipboard),
            TransportKind::NebulaDirect,
        );
    }

    #[test]
    fn switch_reason_audit_token_health_degraded_includes_transport() {
        let r = SwitchReason::HealthDegraded(TransportKind::KdcTls);
        assert_eq!(r.audit_token(), "health_degraded_kdc_tls");
        let r = SwitchReason::HealthDegraded(TransportKind::NebulaHttps443);
        assert_eq!(r.audit_token(), "health_degraded_nebula_https443");
    }

    #[test]
    fn switch_reason_audit_tokens_are_stable() {
        // Audit-log readers grep on these strings. Any change
        // requires a coordinated reader update.
        assert_eq!(SwitchReason::Initial.audit_token(), "initial");
        assert_eq!(SwitchReason::Policy.audit_token(), "policy");
        assert_eq!(
            SwitchReason::ManualOverride.audit_token(),
            "manual_override"
        );
        assert_eq!(SwitchReason::FlapPenalty.audit_token(), "flap_penalty");
        // KDC2-4.5 — mesh-shunt + direct-LAN-recovery tokens.
        assert_eq!(
            SwitchReason::MeshShuntActivated.audit_token(),
            "mesh_shunt_activated",
        );
        assert_eq!(
            SwitchReason::DirectLanRecovered.audit_token(),
            "direct_lan_recovered",
        );
    }

    #[test]
    fn mesh_shunt_variants_round_trip_through_json() {
        for r in [
            SwitchReason::MeshShuntActivated,
            SwitchReason::DirectLanRecovered,
        ] {
            let raw = serde_json::to_string(&r).unwrap();
            let back: SwitchReason = serde_json::from_str(&raw).unwrap();
            assert_eq!(back, r);
        }
    }

    #[test]
    fn mesh_shunt_variants_serialize_as_snake_case() {
        // Wire-compat lock: the JSON form matches the audit-
        // token form (same string).
        assert_eq!(
            serde_json::to_string(&SwitchReason::MeshShuntActivated).unwrap(),
            r#""mesh_shunt_activated""#,
        );
        assert_eq!(
            serde_json::to_string(&SwitchReason::DirectLanRecovered).unwrap(),
            r#""direct_lan_recovered""#,
        );
    }

    #[test]
    fn peer_path_round_trips_through_json() {
        let mut p = PeerPath::initial("peer-X".into(), TransportKind::KdcTls);
        p.fallback = Some(TransportKind::NebulaLighthouseRelay);
        p.last_switch_reason = SwitchReason::HealthDegraded(TransportKind::NebulaDirect);
        p.health_score = 0.85;
        p.message_class_overrides
            .insert(MessageClass::FileBulk, TransportKind::NebulaHttps443);
        // SystemTime serializes (we don't use it in the round-
        // trip test to keep the JSON deterministic across runs).
        let s = serde_json::to_string(&p).unwrap();
        let back: PeerPath = serde_json::from_str(&s).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn switch_reason_round_trips_through_json() {
        for r in [
            SwitchReason::Initial,
            SwitchReason::Policy,
            SwitchReason::ManualOverride,
            SwitchReason::FlapPenalty,
            SwitchReason::HealthDegraded(TransportKind::KdcTls),
        ] {
            let s = serde_json::to_string(&r).unwrap();
            let back: SwitchReason = serde_json::from_str(&s).unwrap();
            assert_eq!(back, r);
        }
    }
}
