//! `MeshInit` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `mesh-init` subcommand.
#[allow(unreachable_code)]
pub fn run(
    mesh_id: String,
    external_addr: String,
    role: String,
    db_path: PathBuf,
) -> anyhow::Result<()> {
    {
        let parsed: mde_role::Role = role.parse().map_err(|_| {
            anyhow::anyhow!("unknown role `{role}` — expected lighthouse|workstation")
        })?;
        let conn = mackesd_core::store::open(&db_path)
            .with_context(|| format!("opening store at {}", db_path.display()))?;
        mackesd_core::store::migrate(&conn).context("migrating store")?;
        let root = mackesd_core::default_qnm_shared_root();
        // Bed fix #10: use the SAME node-id resolution `serve` uses
        // (MACKESD_NODE_ID → HOSTNAME → `hostname` → peer:unknown). The
        // old code here shelled ONLY `hostname` (falling back to
        // "founder") and ignored MACKESD_NODE_ID + the HOSTNAME env — so
        // on a box where those disagree (a container with no `hostname`
        // binary, or an operator-set MACKESD_NODE_ID), mesh-init wrote the
        // founding bundle under one id while the next `serve`'s
        // nebula-supervisor looked under a DIFFERENT id, never found it,
        // and the founding lighthouse's overlay never came up. Caught by
        // the OBS-1 container E2E.
        let node_id = default_node_id();
        let report = mackesd_core::mesh_init::mesh_init(
            &mackesd_core::ca::SubprocessBackend,
            &conn,
            &root,
            &node_id,
            &mesh_id,
            &external_addr,
            std::path::Path::new("/var/lib/mackesd/nebula-ca/ca.crt"),
            std::path::Path::new("/var/lib/mackesd/nebula-ca/ca.key"),
            std::path::Path::new("/var/lib/mackesd/nebula-ca/scratch"),
            std::path::Path::new("/etc/nebula"),
            parsed,
        )?;
        // Best-effort unit starts — the supervisor (next serve)
        // also materializes + starts; containerized test envs
        // without systemd still get a complete on-disk state.
        let _ = std::process::Command::new("systemctl")
            .args(["start", "nebula.service"])
            .status();
        println!(
            "mesh `{}` initialized — lighthouse {} ({})",
            report.mesh_id, node_id, report.overlay_ip
        );
        if let Some(r) = &report.pinned_role {
            println!("role pinned: {r}");
        }
        println!("bundle: {}", report.bundle_path.display());
        println!(
            "\nfirst peer joins with:\n  mackesd join '{}'",
            report.join_token
        );
        return Ok(());
    }
    Ok(())
}
