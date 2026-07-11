//! `InventoryLegacy` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `inventory-legacy` subcommand.
#[allow(unreachable_code)]
pub fn run(mesh_only: bool, json: bool) -> anyhow::Result<()> {
    {
        // Phase 12.13.1 — read-only walk of the three legacy
        // roots. Operator runs this before `import-legacy` to
        // see what's on disk.
        let roots = mackesd_core::legacy_inventory::default_roots();
        let mut artifacts = mackesd_core::legacy_inventory::inventory(&roots);
        if mesh_only {
            artifacts.retain(|a| a.mesh_data);
        }
        if json {
            println!("{}", serde_json::to_string_pretty(&artifacts)?);
        } else {
            print_inventory_table(&artifacts);
        }
    }
    #[cfg(feature = "async-services")]
    Ok(())
}
