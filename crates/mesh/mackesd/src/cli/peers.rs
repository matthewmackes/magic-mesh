//! `Peers` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `peers` subcommand.
#[allow(unreachable_code)]
pub fn run(json: bool, db_path: PathBuf) -> anyhow::Result<()> {
    {
        // PD-1 — the joined directory, CLI face.
        let root = mackesd_core::default_qnm_shared_root();
        let svc = mackesd_core::ipc::directory::DirectoryService::new(&root, Some(db_path.clone()));
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let dir = svc.build_directory(now);
        if json {
            println!("{dir}");
        } else {
            let head = dir["head"]
                .as_u64()
                .map_or("-".to_string(), |v| v.to_string());
            println!("fleet head: {head}");
            println!(
                "{:<16} {:<8} {:<10} {:<12} {:<15} {:<8}",
                "PEER", "PRESENCE", "HEALTH", "VERSION", "OVERLAY IP", "REVISION"
            );
            for p in dir["peers"].as_array().into_iter().flatten() {
                println!(
                    "{:<16} {:<8} {:<10} {:<12} {:<15} {:<8}",
                    p["hostname"].as_str().unwrap_or("-"),
                    p["presence"].as_str().unwrap_or("-"),
                    p["health"].as_str().unwrap_or("-"),
                    p["mde_version"].as_str().unwrap_or("-"),
                    p["overlay_ip"].as_str().unwrap_or("-"),
                    p["revision"]["currency"].as_str().unwrap_or("-"),
                );
            }
        }
        return Ok(());
    }
    Ok(())
}
