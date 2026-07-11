//! `AdoptXcp` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `adopt-xcp` subcommand.
#[allow(unreachable_code)]
pub fn run(
    pool_address: String,
    overlay_ip: String,
    credential_ref: String,
    dry_run: bool,
) -> anyhow::Result<()> {
    {
        // Plan the adoption: gather this node's facts (mesh-id, CA holder,
        // whether the host credential resolves), fold into a plan. The live
        // member-enroll + xe/tofu apply is integration-gated behind the Adopter
        // seam; --dry-run stops at the plan + ordered steps.
        use mackesd_core::adopt_xcp as ax;
        let node_id = default_node_id();
        let root = mackesd_core::default_qnm_shared_root();
        let target = ax::AdoptTarget {
            pool_address,
            overlay_ip,
            credential_ref,
        };
        let facts = ax::gather(&root, &node_id, &target);
        let plan = ax::plan_adopt(&target, &facts);
        println!("adopt-xcp: {}", plan.human());
        if dry_run {
            for (i, step) in plan.steps().iter().enumerate() {
                println!("  {}. {}", i + 1, step.describe());
            }
            return Ok(());
        }
        // Live path: drive the integration-gated Adopter seam (enroll static
        // member → drive toolstack).
        match ax::execute(&plan, &ax::LiveAdopter) {
            Ok(ax::AdoptOutcome::Adopted { host }) => {
                println!(
                    "  adopted {} as a static member (overlay {})",
                    host.pool_address, host.overlay_ip
                );
            }
            Ok(ax::AdoptOutcome::Blocked { reason }) => {
                println!("  no-op — blocked ({reason}); retry available");
            }
            Err(e) => {
                eprintln!("  adopt-xcp failed (live enroll + xe/tofu is integration-gated): {e}");
                std::process::exit(1);
            }
        }
    }
    Ok(())
}
