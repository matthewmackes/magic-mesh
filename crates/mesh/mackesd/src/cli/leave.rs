//! `Leave` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.

/// Handle the `leave` subcommand.
#[allow(unreachable_code)]
pub fn run(yes: bool) -> anyhow::Result<()> {
    {
        if !yes {
            anyhow::bail!(
                "leave wipes this box's mesh state (cert, keys, role). \
                     Re-run with --yes to confirm."
            );
        }
        let root = mackesd_core::default_qnm_shared_root();
        let hostname = std::process::Command::new("hostname")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".to_string());
        let node_id = format!("peer:{hostname}");
        let report = mackesd_core::leave::leave(
            &root,
            &hostname,
            &node_id,
            std::path::Path::new("/etc/nebula"),
            std::path::Path::new("/var/lib/mde/role.toml"),
        );
        // HA — drop our own etcd cluster membership BEFORE stopping the overlay
        // (the cluster is reached over nebula), so a retired node never leaves a
        // ghost voter dragging quorum. Best-effort; a non-member is a no-op.
        {
            use mackesd_core::substrate::{etcd, etcd_membership};
            let eps = etcd::default_endpoints();
            if !eps.is_empty() {
                let sel = match mackesd_core::voip_rtt::own_nebula_ip() {
                    Some(ip) => etcd_membership::MemberSel::Overlay(ip),
                    None => etcd_membership::MemberSel::Hostname(hostname.clone()),
                };
                match etcd_membership::remove_member_blocking(&eps, &sel) {
                    Some(Ok(true)) => println!("etcd: removed self from the cluster"),
                    Some(Ok(false)) | None => {}
                    Some(Err(e)) => eprintln!(
                        "etcd: could not remove self ({e}) — prune the stale member \
                             with `etcdctl member remove`"
                    ),
                }
            }
        }
        let _ = std::process::Command::new("systemctl")
            .args(["stop", "nebula.service"])
            .status();
        println!("left the mesh: {report:#?}");
        println!("re-join later with: mackesd join '<fresh token from a lighthouse>'");
        return Ok(());
    }
    Ok(())
}
