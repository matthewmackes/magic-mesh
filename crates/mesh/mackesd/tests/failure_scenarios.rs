//! Failure-injection test suite (Phase 12.11.3).
//!
//! Seven named scenarios from the locked acceptance list:
//!
//!   1. node failure
//!   2. region outage
//!   3. invalid config
//!   4. stale telemetry
//!   5. route conflict
//!   6. policy conflict
//!   7. passcode rotation during apply
//!
//! Each test (a) injects a known-bad state into the relevant
//! `mackesd_core` API surface, (b) invokes the actual function under
//! test, (c) asserts the expected failure mode + recovery row, and
//! (d) re-drives the system back to a good state and asserts the
//! recovery path produces the right answer.
//!
//! Scope is pure Rust — no Docker, no networked daemons. The
//! Docker-based happy-path coverage lives in 12.11.2's integration
//! suite; this file owns the "things break mid-flight" half of the
//! testing matrix.

use std::collections::BTreeSet;

use mackesd_core::passcode;
use mackesd_core::policy::{detect_conflicts, Policy};
use mackesd_core::reconcile::{plan_tick, DriftSeverity};
use mackesd_core::revisions::{diff as revision_diff, next_revision_id, Revision};
use mackesd_core::secrets::Passcode;
use mackesd_core::telemetry::{health_state_from_age, HealthState};
use mackesd_core::topology::{
    calculate, diff as topology_diff, DesiredSnapshot, Edge, EdgeKind, Node, TopologyDiff,
    TopologySnapshot,
};
use mackesd_core::validation::{validate, ValidationError};

// ---------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------

fn node(id: &str, region: &str, healthy: bool, is_host: bool) -> Node {
    Node {
        id: id.to_owned(),
        region: region.to_owned(),
        healthy,
        is_host,
    }
}

fn edge(a: &str, b: &str) -> Edge {
    // Always store edges with lexicographically-ordered endpoints,
    // matching what `topology::calculate` emits.
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    Edge {
        a: lo.to_owned(),
        b: hi.to_owned(),
        kind: EdgeKind::NebulaDirect,
    }
}

/// Build an observed topology snapshot from a flat list of pairs.
/// Caller's responsibility to pass pairs that represent the *actual*
/// adjacencies the peers are reporting.
fn observed_with(pairs: &[(&str, &str)]) -> TopologySnapshot {
    let mut edges = BTreeSet::new();
    for (a, b) in pairs {
        edges.insert(edge(a, b));
    }
    TopologySnapshot {
        edges,
        routes: Default::default(),
    }
}

/// Diff helper — runs `topology::calculate` over the desired snapshot
/// and compares it to the supplied observed snapshot.
fn diff_for(desired: &DesiredSnapshot, observed: &TopologySnapshot) -> TopologyDiff {
    let desired_topo = calculate(desired);
    topology_diff(&desired_topo, observed)
}

// ---------------------------------------------------------------------
// 1. node failure
// ---------------------------------------------------------------------

/// A node that was healthy goes down: observed snapshot loses every
/// edge that touched it. `plan_tick` must surface every dropped edge
/// as auto-repairable drift so the reconciler can re-push the
/// adjacencies. After the node recovers, the next tick must produce
/// an empty plan (no false positives).
#[test]
fn node_failure_emits_auto_repair_drift_then_clears_on_recovery() {
    let desired = DesiredSnapshot {
        nodes: vec![
            node("peer:anvil", "us-east", true, true),
            node("peer:birch", "us-east", true, false),
            node("peer:cedar", "us-east", true, false),
        ],
        allow_east_west: vec![],
        settings_keys: vec![],
        voice_policies: vec![],
    };

    // Observed: only peer:birch ↔ peer:cedar survived; peer:anvil
    // crashed and dropped every adjacency it carried.
    let observed_failed = observed_with(&[("peer:birch", "peer:cedar")]);
    let diff_failed = diff_for(&desired, &observed_failed);

    // Two edges are now missing — anvil↔birch and anvil↔cedar.
    assert_eq!(
        diff_failed.missing.len(),
        2,
        "expected 2 missing edges when peer:anvil fails: {:?}",
        diff_failed.missing
    );
    assert!(diff_failed.extra.is_empty(), "no extra edges expected");

    let plan = plan_tick(&diff_failed, /* auto_repair_enabled = */ true);
    assert_eq!(
        plan.repair_now.len(),
        2,
        "missing edges must be auto-repairable (got: {plan:?})"
    );
    assert!(plan.inbox.is_empty(), "no manual-review rows expected");
    for row in &plan.repair_now {
        assert_eq!(row.severity, DriftSeverity::AutoRepairable);
        assert_eq!(row.detector, "topology");
        assert!(
            row.reason.contains("peer:anvil"),
            "drift row must name the failed peer: {row:?}"
        );
    }

    // Recovery: peer:anvil comes back; observed matches desired again.
    let observed_recovered = observed_with(&[
        ("peer:anvil", "peer:birch"),
        ("peer:anvil", "peer:cedar"),
        ("peer:birch", "peer:cedar"),
    ]);
    let diff_recovered = diff_for(&desired, &observed_recovered);
    let plan_recovered = plan_tick(&diff_recovered, true);
    assert!(
        plan_recovered.repair_now.is_empty() && plan_recovered.inbox.is_empty(),
        "post-recovery plan must be empty: {plan_recovered:?}"
    );
}

// ---------------------------------------------------------------------
// 2. region outage
// ---------------------------------------------------------------------

/// Every node in a region goes unreachable at once. The topology
/// engine must (a) exclude those nodes from the calculated edge set
/// because `healthy = false`, and (b) every edge that previously
/// existed against the dead region must surface as drift. When the
/// region comes back, the calculated topology returns to full size
/// and the drift list clears.
#[test]
fn region_outage_excludes_dead_nodes_from_topology_and_flags_drift() {
    let mut desired = DesiredSnapshot {
        nodes: vec![
            node("peer:anvil", "us-east", true, true),
            node("peer:birch", "us-east", true, false),
            // us-west region: two nodes that just went dark.
            node("peer:pine", "us-west", false, false),
            node("peer:redwood", "us-west", false, false),
        ],
        allow_east_west: vec![],
        settings_keys: vec![],
        voice_policies: vec![],
    };

    let topo = calculate(&desired);
    // Only one edge survives — peer:anvil ↔ peer:birch in us-east.
    assert_eq!(
        topo.edges.len(),
        1,
        "us-west outage must drop edges touching dead nodes: {:?}",
        topo.edges
    );
    let surviving = topo.edges.iter().next().unwrap();
    assert!(surviving.a.contains("peer:anvil") || surviving.b.contains("peer:anvil"));
    assert!(surviving.a.contains("peer:birch") || surviving.b.contains("peer:birch"));

    // Simulate that the observed topology still reports the dead
    // region's edges (stale Tailscale ACL view, say). The reconciler
    // must flag every one of those as drift requiring manual review.
    let observed = observed_with(&[
        ("peer:anvil", "peer:birch"),
        ("peer:anvil", "peer:pine"),
        ("peer:birch", "peer:redwood"),
    ]);
    let diff = topology_diff(&topo, &observed);
    assert_eq!(diff.extra.len(), 2, "two stale edges must be detected");
    let plan = plan_tick(&diff, true);
    assert!(
        plan.repair_now.is_empty(),
        "stale extras must NOT auto-repair: {plan:?}"
    );
    assert_eq!(plan.inbox.len(), 2);
    for row in &plan.inbox {
        assert_eq!(row.severity, DriftSeverity::ManualReview);
    }

    // Recovery: us-west comes back. Healthy flag flips to true on
    // every dead row; the calculated topology returns to full mesh.
    for n in &mut desired.nodes {
        if n.region == "us-west" {
            n.healthy = true;
        }
    }
    let topo_recovered = calculate(&desired);
    assert_eq!(
        topo_recovered.edges.len(),
        6,
        "4 healthy nodes -> 6 edges full mesh"
    );
}

// ---------------------------------------------------------------------
// 3. invalid config
// ---------------------------------------------------------------------

/// Feed `validate()` a snapshot that breaks multiple invariants in
/// one shot: empty id, empty region, duplicate id, unknown region in
/// `allow_east_west`. Assert each variant fires AND that fixing them
/// returns an empty error list (no spurious leftovers).
#[test]
fn invalid_config_returns_specific_errors_then_accepts_fixed_payload() {
    let bad = DesiredSnapshot {
        nodes: vec![
            node("peer:dup", "us-east", true, false),
            node("peer:dup", "us-west", true, false), // duplicate id
            node("", "us-east", true, false),         // empty id
            node("peer:ok", "", true, false),         // empty region
        ],
        allow_east_west: vec![
            ("us-east".into(), "typo-region".into()), // unknown region
        ],
        settings_keys: vec![],
        voice_policies: vec![],
    };
    let errors = validate(&bad);

    let any = |pred: fn(&ValidationError) -> bool| errors.iter().any(pred);
    assert!(
        any(|e| matches!(e, ValidationError::DuplicateNodeId { id } if id == "peer:dup")),
        "expected DuplicateNodeId(peer:dup): {errors:?}"
    );
    assert!(
        any(|e| matches!(e, ValidationError::EmptyRequiredField { path } if path.ends_with(".id"))),
        "expected EmptyRequiredField for an id"
    );
    assert!(
        any(
            |e| matches!(e, ValidationError::EmptyRequiredField { path } if path.ends_with(".region"))
        ),
        "expected EmptyRequiredField for a region"
    );
    assert!(
        any(|e| matches!(e, ValidationError::UnknownRegion { region } if region == "typo-region")),
        "expected UnknownRegion(typo-region)"
    );

    // Recovery: fix every field. Now validate returns no errors.
    let good = DesiredSnapshot {
        nodes: vec![
            node("peer:anvil", "us-east", true, true),
            node("peer:birch", "us-east", true, false),
        ],
        allow_east_west: vec![("us-east".into(), "us-east".into())],
        settings_keys: vec![],
        voice_policies: vec![],
    };
    assert!(
        validate(&good).is_empty(),
        "fixed snapshot must validate cleanly"
    );
}

// ---------------------------------------------------------------------
// 4. stale telemetry
// ---------------------------------------------------------------------

/// A telemetry row whose `at_ms` is older than the unreachable
/// threshold (30 s per 12.3.3) must classify as `Unreachable`. A
/// row in the degraded window (10-30 s) classifies as `Degraded`. A
/// fresh row clears back to `Healthy`. Drives the live/stale filter
/// every health-summary path uses.
#[test]
fn stale_telemetry_classifies_unreachable_then_clears_on_fresh_sample() {
    // The age helper takes a millisecond age value — we don't need
    // to spin the wall clock to exercise it.
    let fresh = health_state_from_age(0);
    let one_cycle = health_state_from_age(15_000);
    let three_cycles = health_state_from_age(35_000);
    assert_eq!(fresh, HealthState::Healthy);
    assert_eq!(one_cycle, HealthState::Degraded);
    assert_eq!(three_cycles, HealthState::Unreachable);

    // Boundary lock from 12.3.3: < 10s = healthy, [10s, 30s) = degraded,
    // ≥ 30s = unreachable. Boundary inclusivity goes UP — exactly at
    // 10_000ms the row is degraded, exactly at 30_000ms it's
    // unreachable.
    assert_eq!(health_state_from_age(9_999), HealthState::Healthy);
    assert_eq!(health_state_from_age(10_000), HealthState::Degraded);
    assert_eq!(health_state_from_age(29_999), HealthState::Degraded);
    assert_eq!(health_state_from_age(30_000), HealthState::Unreachable);

    // Recovery: the peer's mackesd writes a fresh heartbeat. Age
    // drops back to 0, classifier returns Healthy. The "live set"
    // filter that the panel uses includes anything that isn't
    // Unreachable, so a re-classify to Healthy unambiguously
    // re-includes the peer.
    let recovered = health_state_from_age(500);
    assert_eq!(recovered, HealthState::Healthy);
    assert_ne!(recovered, three_cycles);
}

// ---------------------------------------------------------------------
// 5. route conflict
// ---------------------------------------------------------------------

/// Two revisions are committed in quick succession; both claim the
/// same destination peer in their route table but disagree on the
/// next hop. The locked rule is "later revision wins" — verify
/// `next_revision_id` increments monotonically AND `revisions::diff`
/// flags the route field as changed (so the GUI surfaces the
/// override cleanly).
#[test]
fn route_conflict_resolves_by_later_revision_via_diff_and_id_monotonicity() {
    // Earlier revision: peer:b reachable via peer:host.
    let earlier = Revision {
        id: "r-2026-05-19-0001".into(),
        author: "alice".into(),
        summary: "initial route plan".into(),
        created_at: 1_700_000_000_000,
        payload_json: r#"{"route_peer_b":"peer:host"}"#.to_owned(),
    };
    // Later revision: same destination, different next hop.
    let later = Revision {
        id: next_revision_id("2026-05-19", Some(&earlier.id)),
        author: "alice".into(),
        summary: "override route after host outage".into(),
        created_at: 1_700_000_001_000,
        payload_json: r#"{"route_peer_b":"peer:fallback"}"#.to_owned(),
    };

    // Lock: ids increment within the same day.
    assert_eq!(later.id, "r-2026-05-19-0002");

    let d = revision_diff(&earlier, &later).expect("diff must succeed");
    assert!(
        d.changed.contains_key("route_peer_b"),
        "diff must flag the conflicting key: {d:?}"
    );
    let (from_value, to_value) = &d.changed["route_peer_b"];
    assert!(from_value.contains("peer:host"));
    assert!(to_value.contains("peer:fallback"));
    assert!(d.added.is_empty(), "no new keys expected");
    assert!(d.removed.is_empty(), "no dropped keys expected");

    // Recovery semantics: the LATER revision's value is the live
    // route. Verify by diffing the other way — what was "new" before
    // becomes "old", proving the value swap is single-direction-
    // detectable.
    let reverse = revision_diff(&later, &earlier).expect("reverse diff");
    let (rev_from, rev_to) = &reverse.changed["route_peer_b"];
    assert!(rev_from.contains("peer:fallback"));
    assert!(rev_to.contains("peer:host"));
}

// ---------------------------------------------------------------------
// 6. policy conflict
// ---------------------------------------------------------------------

/// AllowEastWest + DenyEastWest over the same region pair must
/// surface as a conflict naming BOTH rule ids. The orthogonal
/// BandwidthCap rule must NOT spuriously join the conflict report.
/// Recovery: removing one of the contradictory rules clears the list.
#[test]
fn policy_conflict_surfaces_both_rule_ids_then_clears_when_one_is_dropped() {
    let bandwidth = Policy::BandwidthCap {
        id: "r-bw".into(),
        from_region: "us-east".into(),
        to_region: "us-west".into(),
        mbps: 100,
    };
    let rules = vec![
        Policy::AllowEastWest {
            id: "r-allow".into(),
            from_region: "us-east".into(),
            to_region: "us-west".into(),
        },
        Policy::DenyEastWest {
            id: "r-deny".into(),
            from_region: "us-east".into(),
            to_region: "us-west".into(),
        },
        bandwidth.clone(),
    ];
    let conflicts = detect_conflicts(&rules);
    assert_eq!(conflicts.len(), 1, "exactly one conflict expected");
    let c = &conflicts[0];
    let pair = [c.rule_a.as_str(), c.rule_b.as_str()];
    assert!(pair.contains(&"r-allow"));
    assert!(pair.contains(&"r-deny"));
    assert!(
        c.reason.contains("us-east") && c.reason.contains("us-west"),
        "conflict reason must name the regions: {}",
        c.reason
    );
    // Bandwidth rule is orthogonal — must NOT appear in the report.
    assert!(!pair.contains(&"r-bw"));

    // Recovery: drop the deny rule, only allow + bandwidth left.
    let fixed = vec![
        Policy::AllowEastWest {
            id: "r-allow".into(),
            from_region: "us-east".into(),
            to_region: "us-west".into(),
        },
        bandwidth,
    ];
    assert!(
        detect_conflicts(&fixed).is_empty(),
        "removing the deny rule must clear the conflict list"
    );
}

// ---------------------------------------------------------------------
// 7. passcode rotation during apply
// ---------------------------------------------------------------------

/// Simulates the in-flight apply scenario from 12.10.1: an operator
/// kicks off `apply --dry-run` with passcode A, then the host
/// rotates the passcode to B before the actual `reconcile --once`
/// runs. Asserts:
///   * The old passcode no longer matches via constant-time check.
///   * The new passcode is well-formed AND distinct.
///   * The reconcile path that was holding the OLD passcode rejects
///     it (ct_eq returns false) — the apply is dropped.
///   * A fresh apply built from the NEW passcode is accepted.
#[test]
fn passcode_rotation_rejects_inflight_apply_then_accepts_fresh_one() {
    // 1. Apply starts: snapshot the active passcode as the in-flight
    //    creds.
    let original_text = passcode::generate();
    assert!(passcode::looks_valid(&original_text));
    let inflight = Passcode::new(&original_text).expect("original must parse");

    // 2. Host rotates: a new passcode is generated and stored in the
    //    libsecret keyring. The reconciler reads the current value
    //    fresh on every tick.
    let rotated_text = passcode::generate();
    assert!(passcode::looks_valid(&rotated_text));
    assert_ne!(
        original_text, rotated_text,
        "rotation must produce a distinct value"
    );
    let current = Passcode::new(&rotated_text).expect("rotated must parse");

    // 3. The reconciler compares the in-flight apply's credential
    //    (held by the operator's session) against the CURRENT
    //    passcode — they MUST not match, so the in-flight apply
    //    fails closed.
    assert!(
        !inflight.ct_eq(&current),
        "in-flight apply must be rejected after rotation"
    );

    // 4. Recovery: the operator re-fetches the rotated passcode and
    //    drives a fresh apply. That credential matches the current
    //    one and the apply proceeds.
    let refreshed = Passcode::new(&rotated_text).expect("rotated parses again");
    assert!(
        refreshed.ct_eq(&current),
        "fresh apply with rotated passcode must be accepted"
    );

    // 5. Belt-and-braces: the OLD secret can never silently grant
    //    access after rotation, even if reconstructed.
    let resurrected = Passcode::new(&original_text).expect("old text still parses");
    assert!(
        !resurrected.ct_eq(&current),
        "resurrecting the rotated-out passcode must NOT match current"
    );

    // 6. Length-shape guard: a passcode that's the wrong length
    //    can't even be constructed, so an apply built from a
    //    truncated copy fails before reaching ct_eq.
    let truncated = &rotated_text[..rotated_text.len() - 1];
    assert!(
        Passcode::new(truncated).is_none(),
        "truncated passcode must be rejected by the constructor"
    );
}
