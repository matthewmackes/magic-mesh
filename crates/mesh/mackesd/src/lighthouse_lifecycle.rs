//! #13 — turn-key lighthouse lifecycle.
//!
//! The pure HA decision logic for `mackesd lighthouse retire`. The IO
//! orchestration — probe the authoritative live etcd members, mint a role-scoped
//! token + shell the join provisioner (`add`), and drain → remove-peer →
//! droplet-delete (`retire`) — lives at the CLI boundary in `bin/mackesd.rs` (it
//! composes the existing verbs/helpers: `add-peer`'s token mint,
//! `etcd_membership`, `remove-peer`, `doctl`). This module stays pure so the gate
//! is unit-tested.

/// HA drain gate over authoritative etcd membership and direct member probes.
/// Only reachable, started voters count toward the survivor floor. An absent
/// target is accepted as an idempotent retry only when the already-surviving
/// live voters still satisfy the floor.
pub fn drain_gate_members(
    members: &[crate::substrate::etcd_membership::MemberSnapshot],
    target: &str,
    force: bool,
) -> Result<(), String> {
    let reachable_voters = members
        .iter()
        .filter(|member| !member.learner && member.reachable && !member.name.is_empty())
        .count();
    let target_is_reachable_voter = members
        .iter()
        .any(|member| member.name == target && !member.learner && member.reachable);
    let after = reachable_voters.saturating_sub(if target_is_reachable_voter { 1 } else { 0 });
    if !force && after < mackes_mesh_types::lighthouse::HA_MIN_LIGHTHOUSES {
        return Err(format!(
            "retiring '{target}' would leave {after} directly reachable etcd voters (< HA_MIN_LIGHTHOUSES={}); stand up and converge a replacement first, or pass --force",
            mackes_mesh_types::lighthouse::HA_MIN_LIGHTHOUSES
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(
        name: &str,
        learner: bool,
        reachable: bool,
    ) -> crate::substrate::etcd_membership::MemberSnapshot {
        crate::substrate::etcd_membership::MemberSnapshot {
            name: name.into(),
            learner,
            reachable,
        }
    }

    #[test]
    fn authoritative_gate_never_counts_stale_or_unstarted_survivors() {
        let members = [
            member("lh-1", false, true),
            member("lh-2", false, true),
            member("lh-3", false, false),
            member("", false, true),
            member("joining", true, true),
        ];
        assert!(drain_gate_members(&members, "lh-1", false).is_err());
        assert!(drain_gate_members(&members, "lh-3", false).is_ok());
    }

    #[test]
    fn authoritative_gate_allows_safe_four_to_three_and_idempotent_retry() {
        let members = [
            member("lh-1", false, true),
            member("lh-2", false, true),
            member("lh-3", false, true),
            member("lh-4", false, true),
        ];
        assert!(drain_gate_members(&members, "lh-2", false).is_ok());
        assert!(drain_gate_members(&members, "already-retired", false).is_ok());

        let sole_survivor = [member("lh-1", false, true)];
        assert!(drain_gate_members(&sole_survivor, "lh-1", false).is_err());
        assert!(drain_gate_members(&sole_survivor, "lh-1", true).is_ok());
    }
}
