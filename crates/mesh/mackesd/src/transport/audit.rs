//! KDC2-1.12 — PathSwitch audit-chain event payload.
//!
//! Every time `mesh_router::tick_once` flips a peer's primary
//! transport, it emits one `PathSwitchEvent` into the
//! `mackesd::audit` chain. Operators reading the audit log can
//! see every transport switch's `(peer_id, from, to, reason)`
//! tuple — zero silent failovers, the v2.1 KDC2 lock's
//! Definition of Done requirement.
//!
//! The SLO histogram (`kdc2_router_decision_us` Prometheus-style
//! buckets) lands in KDC2-1.12.a once `mackesd::metrics` grows
//! the histogram primitive.

use mackes_transport::peer_path::SwitchReason;
use mackes_transport::TransportKind;
use serde::{Deserialize, Serialize};

/// Audit-chain payload emitted on every mesh-router transport
/// switch. Serializes to JSON bytes for the
/// `mackesd::audit::AuditRow.payload` field.
///
/// Stable serde shape — once written into a peer's audit chain,
/// the schema can't change without a chain-replay migration.
/// Adding a new field is fine (the serde defaults handle older
/// rows); removing one is a breaking change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum PathSwitchEvent {
    /// Primary transport for `peer_id` changed.
    PathSwitch {
        /// Peer the switch happened for.
        peer_id: String,
        /// Previous primary transport (None at peer first-
        /// sighting with `SwitchReason::Initial`).
        from: Option<TransportKind>,
        /// New primary transport.
        to: TransportKind,
        /// Why the switch happened. The audit-token form
        /// (`SwitchReason::audit_token`) is the human-readable
        /// rendering operators grep on.
        reason: SwitchReason,
        /// Unix epoch milliseconds. Independent of the
        /// `AuditRow.timestamp_ms` so the event self-contained
        /// rows can be parsed without the row envelope.
        at_ms: i64,
    },
}

impl PathSwitchEvent {
    /// Construct a `PathSwitch` event from the constituent
    /// pieces. Pure helper used by the mesh-router worker.
    #[must_use]
    pub fn switch(
        peer_id: String,
        from: Option<TransportKind>,
        to: TransportKind,
        reason: SwitchReason,
        at_ms: i64,
    ) -> Self {
        Self::PathSwitch {
            peer_id,
            from,
            to,
            reason,
            at_ms,
        }
    }

    /// Serialize to JSON bytes for the audit-chain row payload.
    /// Panics only if `serde_json::to_vec` fails, which is
    /// impossible for the variants in this enum.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("PathSwitchEvent serializes")
    }

    /// Human-readable one-line summary the operator grep'ing
    /// the audit log uses. Format:
    ///
    /// ```text
    /// path_switch peer=<id> from=<transport|none> to=<transport> reason=<token>
    /// ```
    #[must_use]
    pub fn summary(&self) -> String {
        match self {
            PathSwitchEvent::PathSwitch {
                peer_id,
                from,
                to,
                reason,
                ..
            } => {
                let from_token = from
                    .map(|t| t.as_str().to_string())
                    .unwrap_or_else(|| "none".to_string());
                format!(
                    "path_switch peer={peer_id} from={from_token} to={to} reason={}",
                    reason.audit_token(),
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switch_constructor_records_every_field() {
        let e = PathSwitchEvent::switch(
            "peer-A".into(),
            Some(TransportKind::NebulaDirect),
            TransportKind::KdcTls,
            SwitchReason::HealthDegraded(TransportKind::NebulaDirect),
            1_700_000_000_000,
        );
        match e {
            PathSwitchEvent::PathSwitch {
                peer_id,
                from,
                to,
                reason,
                at_ms,
            } => {
                assert_eq!(peer_id, "peer-A");
                assert_eq!(from, Some(TransportKind::NebulaDirect));
                assert_eq!(to, TransportKind::KdcTls);
                assert_eq!(
                    reason,
                    SwitchReason::HealthDegraded(TransportKind::NebulaDirect),
                );
                assert_eq!(at_ms, 1_700_000_000_000);
            }
        }
    }

    #[test]
    fn to_bytes_round_trips_through_serde_json() {
        let e = PathSwitchEvent::switch(
            "peer-A".into(),
            None,
            TransportKind::NebulaDirect,
            SwitchReason::Initial,
            1,
        );
        let raw = e.to_bytes();
        let back: PathSwitchEvent = serde_json::from_slice(&raw).unwrap();
        assert_eq!(back, e);
    }

    #[test]
    fn serialization_uses_path_switch_tag() {
        // Wire schema lock: the JSON has `"event":"path_switch"`
        // as the tag so audit-log readers can dispatch on the
        // string without parsing the rest.
        let e = PathSwitchEvent::switch(
            "p".into(),
            None,
            TransportKind::NebulaDirect,
            SwitchReason::Initial,
            0,
        );
        let raw = String::from_utf8(e.to_bytes()).unwrap();
        assert!(raw.contains(r#""event":"path_switch""#));
    }

    #[test]
    fn summary_renders_human_readable_one_liner_with_initial_from_none() {
        let e = PathSwitchEvent::switch(
            "peer-A".into(),
            None,
            TransportKind::NebulaDirect,
            SwitchReason::Initial,
            0,
        );
        let s = e.summary();
        assert!(s.contains("path_switch"));
        assert!(s.contains("peer=peer-A"));
        assert!(s.contains("from=none"));
        assert!(s.contains("to=nebula_direct"));
        assert!(s.contains("reason=initial"));
    }

    #[test]
    fn summary_includes_transport_in_health_degraded_token() {
        let e = PathSwitchEvent::switch(
            "peer-B".into(),
            Some(TransportKind::NebulaDirect),
            TransportKind::KdcTls,
            SwitchReason::HealthDegraded(TransportKind::NebulaDirect),
            0,
        );
        let s = e.summary();
        assert!(s.contains("from=nebula_direct"));
        assert!(s.contains("to=kdc_tls"));
        // SwitchReason::HealthDegraded carries the bumped transport
        // suffix in its audit_token.
        assert!(s.contains("reason=health_degraded_nebula_direct"));
    }

    #[test]
    fn summary_uses_https443_token_for_https_transport() {
        // Token alignment lock: the Display + as_str + serde
        // forms must all agree. Locked by mackes-transport's
        // own tests; this case verifies the audit summary
        // picks up the right token.
        let e = PathSwitchEvent::switch(
            "p".into(),
            Some(TransportKind::NebulaDirect),
            TransportKind::NebulaHttps443,
            SwitchReason::Policy,
            0,
        );
        let s = e.summary();
        assert!(s.contains("to=nebula_https443"));
        assert!(s.contains("reason=policy"));
    }
}
