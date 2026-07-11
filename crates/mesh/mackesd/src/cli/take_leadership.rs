//! `TakeLeadership` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `take-leadership` subcommand.
#[allow(unreachable_code)]
pub fn run(as_node: String) -> anyhow::Result<()> {
    {
        // Phase 12.1.1b — operator-forced leadership bump.
        let lock_path = mackesd_core::default_qnm_shared_root().join(".mackesd-leader.lock");
        let lease = mackesd_core::leader::force_take(&lock_path, &as_node)
            .with_context(|| format!("rewriting {}", lock_path.display()))?;
        println!(
            "leader: {} (epoch {}) — lease renewed at {}",
            lease.node_id, lease.epoch, lease.renewed_at_s
        );
    }
    Ok(())
}
