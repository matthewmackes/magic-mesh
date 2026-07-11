//! `Tags` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `tags` subcommand.
#[allow(unreachable_code)]
pub fn run(json: bool, db_path: PathBuf) -> anyhow::Result<()> {
    {
        // PLANES-3/W82 — the fleet tag census: for each v1 tag, the
        // roster nodes that carry it (read from the cap-tags store).
        use mackes_mesh_types::cap_tags::{read_tags, CapabilityTag};
        let root = mackesd_core::default_qnm_shared_root();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let svc = mackesd_core::ipc::directory::DirectoryService::new(&root, Some(db_path.clone()));
        let dir = svc.build_directory(now);
        let hosts: Vec<String> = dir["peers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| p["hostname"].as_str().map(str::to_string))
            .collect();
        let all_tags = [
            CapabilityTag::Hop,
            CapabilityTag::Execution,
            CapabilityTag::Headless,
        ];
        let rows: Vec<serde_json::Value> = all_tags
            .iter()
            .map(|tag| {
                let carriers: Vec<&str> = hosts
                    .iter()
                    .filter(|h| read_tags(&root, h).has(*tag))
                    .map(String::as_str)
                    .collect();
                serde_json::json!({ "tag": tag.as_str(), "nodes": carriers })
            })
            .collect();
        if json {
            println!("{}", serde_json::to_string(&rows)?);
        } else {
            println!("{:<12} {}", "TAG", "NODES");
            for r in &rows {
                let nodes = r["nodes"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                println!(
                    "{:<12} {}",
                    r["tag"].as_str().unwrap_or("-"),
                    if nodes.is_empty() { "(none)" } else { &nodes }
                );
            }
        }
        return Ok(());
    }
    Ok(())
}
