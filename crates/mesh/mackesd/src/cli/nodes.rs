//! `Nodes` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `nodes` subcommand.
#[allow(unreachable_code)]
pub fn run(cmd: NodesCmd, db_path: PathBuf) -> anyhow::Result<()> {
    {
        // CB-1.5.a — fleet node roster surface. The Iced
        // inventory panel consumes the JSON shape directly.
        match cmd {
            NodesCmd::List { json } => {
                // The roster is the replicated directory, not the
                // local sqlite `nodes` table (empty mesh-wide). See
                // directory_to_node_rows for the why.
                let root = mackesd_core::default_qnm_shared_root();
                let svc = mackesd_core::ipc::directory::DirectoryService::new(
                    &root,
                    Some(db_path.clone()),
                );
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as u64);
                let dir = svc.build_directory(now);
                let nodes = directory_to_node_rows(&dir);
                if json {
                    println!("{}", serde_json::to_string_pretty(&nodes_to_json(&nodes))?);
                } else {
                    print_nodes_table(&nodes);
                }
            }
        }
    }
    Ok(())
}
