//! `RoleGate` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.

/// Handle the `role-gate` subcommand.
#[allow(unreachable_code)]
pub fn run(min_rank: u8) -> anyhow::Result<()> {
    {
        let rank = mackesd_core::worker_role::resolve_rank();
        if rank < min_rank {
            let role = mde_role::load()
                .map(|r| r.to_string())
                .unwrap_or_else(|_| "unpinned".to_string());
            eprintln!(
                    "mackesd role-gate: role conflict — this {role} box (rank {rank}) does not \
                     satisfy the unit's required min-rank {min_rank}; refusing to start the service"
                );
            std::process::exit(1);
        }
        // rank >= min_rank: the gate is satisfied; the unit may start (exit 0).
    }
    Ok(())
}
