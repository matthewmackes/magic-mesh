//! `Leave` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.

/// Handle the `leave` subcommand.
pub fn run(yes: bool, confirmation_json: String, verifying_key_hex: String) -> anyhow::Result<()> {
    if !yes {
        anyhow::bail!(
            "leave wipes this box's mesh state (cert, keys, relay authority trust, role). \
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
    let generation = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs().max(1))
        .unwrap_or(1);
    let plan = mackes_mesh_types::lifecycle::LifecyclePlanV1 {
        schema_version: 1,
        request_id: format!("offboard-{node_id}-{generation}"),
        target_id: node_id,
        intent: mackes_mesh_types::lifecycle::LifecycleIntentKind::Offboard,
        generation,
        steps: vec!["offboard".into()],
    };
    let mut authority = mackesd_core::lifecycle_authority::LifecycleAuthority::begin(&root, plan)
        .map_err(|error| anyhow::anyhow!("cannot acquire offboard authority: {error:?}"))?;
    let confirmation: mackes_mesh_types::lifecycle::LifecycleConfirmationV1 =
        serde_json::from_str(&confirmation_json)
            .map_err(|error| anyhow::anyhow!("invalid offboard confirmation: {error}"))?;
    let key_bytes = parse_hex_32(&verifying_key_hex)
        .map_err(|error| anyhow::anyhow!("invalid verifying key hex: {error}"))?;
    let key_bytes: [u8; 32] = key_bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("verifying key must contain exactly 32 bytes"))?;
    let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(&key_bytes)
        .map_err(|error| anyhow::anyhow!("invalid verifying key: {error}"))?;
    authority
        .accept_confirmation(confirmation, &verifying_key)
        .map_err(|error| anyhow::anyhow!("offboard confirmation rejected: {error:?}"))?;
    let result = authority.run_next(|_| {
        run_inner(yes, root, hostname).map_err(|error| error.to_string())
    });
    if let Err(error) = result {
        let _ = authority.finish();
        return Err(anyhow::anyhow!("offboard lifecycle failure: {error:?}"));
    }
    authority
        .finish()
        .map_err(|error| anyhow::anyhow!("cannot release offboard authority: {error:?}"))
}

fn parse_hex_32(value: &str) -> Result<Vec<u8>, &'static str> {
    let bytes = value.as_bytes();
    if bytes.len() != 64 {
        return Err("expected exactly 64 hexadecimal characters");
    }
    let mut output = Vec::with_capacity(32);
    for pair in bytes.chunks_exact(2) {
        let high = (pair[0] as char).to_digit(16).ok_or("non-hex character")?;
        let low = (pair[1] as char).to_digit(16).ok_or("non-hex character")?;
        output.push(((high << 4) | low) as u8);
    }
    Ok(output)
}

fn run_inner(yes: bool, root: std::path::PathBuf, hostname: String) -> anyhow::Result<()> {
    debug_assert!(yes);
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
        let trust_teardown = ensure_required_trust_teardown_succeeded(&report);
        let _ = std::process::Command::new("systemctl")
            .args(["stop", "nebula.service"])
            .status();
        trust_teardown?;
        println!("left the mesh: {report:#?}");
        println!("re-join later with: mackesd join '<fresh token from a lighthouse>'");
        Ok(())
}

/// An absent pin/key is an idempotent success (including a Workstation that
/// never held the Lighthouse-only private key). A filesystem or path-safety
/// failure is not: the caller must report an incomplete leave and return
/// non-zero without exposing key contents or other secret material.
fn ensure_required_trust_teardown_succeeded(
    report: &mackesd_core::leave::LeaveReport,
) -> anyhow::Result<()> {
    let mut failed = Vec::new();
    if report.relay_trust_authority_key_removal_failed {
        failed.push("relay authority private key");
    }
    if report.relay_trust_authority_pin_removal_failed {
        failed.push("relay authority public pin");
    }
    if failed.is_empty() {
        return Ok(());
    }

    anyhow::bail!(
        "leave incomplete: required local trust teardown failed for {}; other leave steps may \
         already have completed; correct local filesystem access and rerun with --yes",
        failed.join(", ")
    )
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
    use super::{ensure_member_removal_succeeded, ensure_required_trust_teardown_succeeded};
    use mackesd_core::leave::LeaveReport;

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

    #[test]
    fn trust_teardown_gate_accepts_removed_or_idempotently_absent_material() {
        assert!(ensure_required_trust_teardown_succeeded(&LeaveReport::default()).is_ok());

        let removed = LeaveReport {
            relay_trust_authority_pin_removed: true,
            relay_trust_authority_key_removed: true,
            ..LeaveReport::default()
        };
        assert!(ensure_required_trust_teardown_succeeded(&removed).is_ok());
    }

    #[test]
    fn trust_teardown_gate_rejects_each_required_removal_failure_safely() {
        let report = LeaveReport {
            relay_trust_authority_pin_removal_failed: true,
            relay_trust_authority_key_removal_failed: true,
            ..LeaveReport::default()
        };
        let error = ensure_required_trust_teardown_succeeded(&report)
            .expect_err("trust teardown failures must make leave fail")
            .to_string();

        assert!(error.contains("leave incomplete"));
        assert!(error.contains("relay authority private key"));
        assert!(error.contains("relay authority public pin"));
        assert!(error.contains("other leave steps may already have completed"));
        assert!(!error.contains("ed25519"));
    }
}
