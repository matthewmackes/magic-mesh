//! `Dns` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `dns` subcommand.
#[allow(unreachable_code)]
pub fn run(cmd: DnsCmd, db_path: PathBuf) -> anyhow::Result<()> {
    {
        // PLANES-18 — the flat <host>.mesh record set, built from
        // the live roster (the same records mesh_dns feeds resolved).
        use mackesd_core::workers::mesh_dns;
        let DnsCmd::List { json } = cmd;
        let root = mackesd_core::default_qnm_shared_root();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let svc = mackesd_core::ipc::directory::DirectoryService::new(&root, Some(db_path.clone()));
        let dir = svc.build_directory(now);
        // The flat <host>.mesh join + the MEDIA-5 active-active music.mesh
        // set — the SAME two record lists the mesh_dns worker serves, so
        // the CLI dump matches what actually resolves.
        let mut records = mesh_dns::build_records(&mesh_dns::directory_records(&dir));
        records.extend(mesh_dns::build_music_records(&mesh_dns::media_overlay_ips(
            &dir,
        )));
        if json {
            let rows: Vec<serde_json::Value> = records
                .iter()
                .map(|r| serde_json::json!({ "fqdn": r.fqdn, "overlay_ip": r.overlay_ip }))
                .collect();
            println!("{}", serde_json::to_string(&rows)?);
        } else if records.is_empty() {
            println!("no mesh DNS records (no roster peers with overlay IPs yet)");
        } else {
            println!("{:<28} {:<16}", "NAME", "OVERLAY IP");
            for r in &records {
                println!("{:<28} {:<16}", r.fqdn, r.overlay_ip);
            }
        }
        return Ok(());
    }
    Ok(())
}
