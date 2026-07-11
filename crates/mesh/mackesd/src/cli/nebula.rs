//! `Nebula` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `nebula` subcommand.
#[allow(unreachable_code)]
pub fn run(sub: NebulaCmd, db_path: PathBuf) -> anyhow::Result<()> {
    {
        // NF-18.x — mackesd nebula <sub> operator surface.
        let conn = mackesd_core::store::open(&db_path)?;
        match sub {
            NebulaCmd::ExportRoster => {
                // NF-18.2 — JSON array of (node_id, name,
                // overlay_ip, cert_pem, epoch, created_at,
                // expires_at, groups). `groups` is sourced
                // from nodes.role since the Nebula cert
                // groups are encoded in the cert PEM body
                // and we want a flat queryable shape.
                let rows = mackesd_core::nebula_roster::export_roster(&conn)
                    .map_err(|e| anyhow::anyhow!("export-roster: {e}"))?;
                println!("{}", serde_json::to_string_pretty(&rows)?);
            }
        }
    }
    Ok(())
}
