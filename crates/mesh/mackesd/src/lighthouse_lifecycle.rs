//! #13 — turn-key lighthouse lifecycle.
//!
//! The pure HA decision logic for `mackesd lighthouse retire`. The IO
//! orchestration — count the live directory, mint a role-scoped token + shell the
//! join provisioner (`add`), and drain → remove-peer → droplet-delete (`retire`) —
//! lives at the CLI boundary in `bin/mackesd.rs` (it composes the existing
//! verbs/helpers: `add-peer`'s token mint, `etcd_membership`, `remove-peer`,
//! `doctl`). This module stays pure so the gate is unit-tested.

/// HA drain gate — refuse to retire a lighthouse when doing so would drop the live
/// lighthouse count below [`mackes_mesh_types::lighthouse::HA_MIN_LIGHTHOUSES`],
/// unless `force`. `current` is the live lighthouse count INCLUDING the one being
/// retired, so the post-retirement count is `current - 1`.
///
/// # Errors
/// A human-facing message when retiring would breach the HA floor without `--force`.
pub fn drain_gate(current: usize, force: bool) -> Result<(), String> {
    let after = current.saturating_sub(1);
    let floor = mackes_mesh_types::lighthouse::HA_MIN_LIGHTHOUSES;
    if !force && after < floor {
        return Err(format!(
            "retiring this lighthouse would leave {after} reachable (< HA_MIN_LIGHTHOUSES={floor}); \
             stand up a replacement first, or pass --force"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_gate_holds_the_ha_floor() {
        // HA_MIN_LIGHTHOUSES == 2: retiring from 2 → 1 is below the floor.
        assert!(drain_gate(2, false).is_err(), "2→1 breaches the HA floor");
        assert!(drain_gate(3, false).is_ok(), "3→2 == floor is allowed");
        assert!(drain_gate(4, false).is_ok());
        // --force overrides the gate (operator's explicit call).
        assert!(drain_gate(2, true).is_ok(), "--force overrides");
        assert!(drain_gate(1, true).is_ok());
    }
}
