//! Library contract tests (Phase 12.11.5).
//!
//! Snapshot the public-API value shapes via `insta`. Any breaking
//! change to the consumed surface fails CI loudly — the panel +
//! the mackes-mesh-types crate + future Workbench panels all
//! depend on the JSON shapes captured here.

use insta::assert_json_snapshot;
use mackesd_core::health::HealthReport;
use mackesd_core::policy::Policy;
use mackesd_core::reconcile::LifecycleState;
use mackesd_core::telemetry::{HealthState, Heartbeat};
use mackesd_core::topology::{DesiredSnapshot, Node};

#[test]
fn snapshot_health_report_shape() {
    let report = HealthReport::empty();
    // Pin everything that's not a string-version dependent field;
    // version comes from CARGO_PKG_VERSION which changes per
    // release.
    assert_json_snapshot!("health_report_empty", report, {
        ".version" => "[version]",
    });
}

#[test]
fn snapshot_policy_kinds_serialize_snake_case() {
    let kinds = vec![
        Policy::AllowEastWest {
            id: "r1".into(),
            from_region: "us-east".into(),
            to_region: "us-west".into(),
        },
        Policy::DenyEastWest {
            id: "r2".into(),
            from_region: "us-east".into(),
            to_region: "eu-west".into(),
        },
        Policy::BandwidthCap {
            id: "r3".into(),
            from_region: "us-east".into(),
            to_region: "us-west".into(),
            mbps: 100,
        },
    ];
    assert_json_snapshot!("policy_kinds", kinds);
}

#[test]
fn snapshot_telemetry_heartbeat_shape() {
    let hb = Heartbeat {
        node_id: "peer:anvil".into(),
        at_ms: 1_700_000_000,
        agent_version: "1.1.0".into(),
        applied_revision: Some("r-2026-05-19-0042".into()),
        health: HealthState::Healthy,
    };
    assert_json_snapshot!("heartbeat_healthy", hb);
}

#[test]
fn snapshot_lifecycle_state_round_trip() {
    let states = vec![
        LifecycleState::Draft,
        LifecycleState::Validated,
        LifecycleState::Approved,
        LifecycleState::Deploying,
        LifecycleState::Applied,
        LifecycleState::Verified,
        LifecycleState::FailedValidation,
        LifecycleState::RolledBack,
    ];
    assert_json_snapshot!("lifecycle_states", states);
}

#[test]
fn snapshot_topology_node_shape() {
    let node = Node {
        id: "peer:anvil".into(),
        region: "us-east".into(),
        healthy: true,
        is_host: true,
    };
    assert_json_snapshot!("topology_node", node);
}

#[test]
fn snapshot_desired_snapshot_is_empty_by_default() {
    let snap = DesiredSnapshot::default();
    assert_json_snapshot!("desired_snapshot_default", snap);
}
