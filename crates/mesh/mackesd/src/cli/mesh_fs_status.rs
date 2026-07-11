//! `MeshFsStatus` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `mesh-fs-status` subcommand.
#[allow(unreachable_code)]
pub fn run() -> anyhow::Result<()> {
    {
        // MESHFS-2 — aggregate every peer's share usage from the replicated
        // directory; both GUI consumers parse this JSON.
        let report = mesh_fs_report(&mackesd_core::default_qnm_shared_root());
        println!("{}", serde_json::to_string(&report)?);
    }
    Ok(())
}
