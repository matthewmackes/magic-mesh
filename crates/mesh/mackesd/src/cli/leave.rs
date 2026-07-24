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
        // HA — drop our own etcd cluster membership BEFORE stopping the overlay
        // (the cluster is reached over nebula), so a retired node never leaves a
        // ghost voter dragging quorum. A live etcd failure is fatal: wiping the
        // local identity first would leave an unprunable ghost voter and make a
        // retry impossible from this node.
        {
            use mackesd_core::substrate::{etcd, etcd_membership};
            let eps = etcd::default_endpoints();
            if !eps.is_empty() {
                let sel = match mackesd_core::voip_rtt::own_nebula_ip() {
                    Some(ip) => etcd_membership::MemberSel::Overlay(ip),
                    None => etcd_membership::MemberSel::Hostname(hostname.clone()),
                };
                let result = etcd_membership::remove_member_blocking(&eps, &sel);
                if matches!(&result, Some(Ok(true))) {
                    println!("etcd: removed self from the cluster");
                }
                ensure_member_removal_succeeded(result)?;
            }
        }
        let report = mackesd_core::leave::leave(
            &root,
            &hostname,
            &node_id,
            std::path::Path::new("/etc/nebula"),
            std::path::Path::new("/var/lib/mde/role.toml"),
        );
        let _ = std::process::Command::new("systemctl")
            .args(["stop", "nebula.service"])
            .status();
        println!("left the mesh: {report:#?}");
        println!("re-join later with: mackesd join '<fresh token from a lighthouse>'");
        return Ok(());
    }
    Ok(())
}

/// A configured coordination plane must confirm departure before local state is
/// destroyed. `None` means the blocking bridge could not run; `Err` means etcd
/// rejected the mutation. Both leave the node intact so an operator can retry.
fn ensure_member_removal_succeeded(result: Option<Result<bool, String>>) -> anyhow::Result<()> {
    match result {
        Some(Ok(_)) => Ok(()),
        Some(Err(error)) => {
            anyhow::bail!("etcd member removal failed ({error}); refusing to wipe local mesh state")
        }
        None => {
            anyhow::bail!("etcd member removal could not run; refusing to wipe local mesh state")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ensure_member_removal_succeeded;

    #[test]
    fn departure_gate_accepts_confirmed_or_idempotent_removal() {
        assert!(ensure_member_removal_succeeded(Some(Ok(true))).is_ok());
        assert!(ensure_member_removal_succeeded(Some(Ok(false))).is_ok());
    }

    #[test]
    fn departure_gate_refuses_unavailable_or_failed_etcd() {
        assert!(ensure_member_removal_succeeded(None).is_err());
        assert!(ensure_member_removal_succeeded(Some(Err("offline".into()))).is_err());
    }
}
