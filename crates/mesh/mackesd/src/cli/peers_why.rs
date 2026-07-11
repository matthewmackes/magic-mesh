//! `PeersWhy` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `peers-why` subcommand.
#[allow(unreachable_code)]
pub fn run(node_id: String, db_path: PathBuf) -> anyhow::Result<()> {
    {
        // Phase 12.4.4 — explanation surface. Loads the node
        // roster from the store, runs `topology::calculate`,
        // and walks the resulting edge set + route table to
        // emit a per-edge reason chain for the named peer.
        let conn = mackesd_core::store::open(&db_path)
            .with_context(|| format!("opening store at {}", db_path.display()))?;
        let nodes = mackesd_core::store::list_nodes(&conn).context("listing nodes from store")?;
        let report = explain_peer(&node_id, &nodes);
        println!("{}", serde_json::to_string_pretty(&report)?);
    }
    Ok(())
}
