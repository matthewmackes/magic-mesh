//! `Recovery` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `recovery` subcommand.
#[allow(unreachable_code)]
pub fn run(
    node_id: Option<String>,
    token: Option<String>,
    dry_run: bool,
    evict: bool,
    db_path: PathBuf,
) -> anyhow::Result<()> {
    {
        // OW-13 — plan a reinstalled box's FRESH re-enroll and report the OLD
        // identity's passive-revocation status (short-TTL certs self-lapse — no
        // CRL, no key-backup) + the auto-renewal decision for the current cert.
        // The live re-enroll is integration-gated behind the RecoveryApply seam.
        use mackesd_core::recovery as rec;
        let node_id = node_id.unwrap_or_else(default_node_id);
        let root = mackesd_core::default_qnm_shared_root();
        // Reuse the persisted roster (nebula_peer_certs.expires_at + cert_pem) to
        // find the old cert's expiry (drives passive revocation) and, for
        // --evict, its PEM to fingerprint.
        let roster = mackesd_core::store::open(&db_path)
            .ok()
            .and_then(|conn| mackesd_core::nebula_roster::export_roster(&conn).ok())
            .unwrap_or_default();
        let facts = rec::gather(&root, &node_id, &roster, token.is_some());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
            .unwrap_or(0);
        let plan = rec::plan_recovery(&node_id, &facts);
        println!("recovery: {}", plan.human());
        // Passive revocation of the OLD identity + the renewal decision for it.
        if let Some(expiry) = facts.old_cert_expiry {
            match rec::passive_revocation_status(expiry, now) {
                rec::RevocationStatus::Expired => {
                    println!("  old identity: already expired — passively revoked, no CRL needed");
                }
                rec::RevocationStatus::StillValid { expires_in } => {
                    println!(
                        "  old identity: still valid, self-expires in {expires_in}s \
                             (short-TTL passive revocation; --evict for an immediate blocklist)"
                    );
                }
            }
            match rec::plan_renewal(expiry, now, &rec::TtlPolicy::short_ttl()) {
                rec::RenewDecision::Renew { remaining_secs } => {
                    println!("  renewal: due now ({remaining_secs}s left, within lead time)");
                }
                rec::RenewDecision::Ok { remaining_secs } => {
                    println!("  renewal: not yet ({remaining_secs}s left)");
                }
                rec::RenewDecision::Expired { overdue_secs } => {
                    println!("  renewal: overdue by {overdue_secs}s — re-enroll");
                }
            }
        } else {
            println!("  old identity: no active roster row (already reaped or never present)");
        }
        if dry_run {
            for (i, step) in plan.steps().iter().enumerate() {
                println!("  {}. {}", i + 1, step.describe());
            }
            return Ok(());
        }
        let generation = u64::try_from(now.max(1)).unwrap_or(1);
        let lifecycle_plan = mackes_mesh_types::lifecycle::LifecyclePlanV1 {
            schema_version: 1,
            request_id: format!("recovery-{node_id}-{generation}"),
            target_id: node_id.clone(),
            intent: mackes_mesh_types::lifecycle::LifecycleIntentKind::VerifyAndCorrect,
            generation,
            steps: vec!["identity".into()],
        };
        let mut authority =
            mackesd_core::lifecycle_authority::LifecycleAuthority::begin(&root, lifecycle_plan)
                .map_err(|error| anyhow::anyhow!("cannot acquire recovery authority: {error:?}"))?;
        let result = authority.run_next(|_| {
            run_live_recovery(node_id, roster, root, plan, evict).map_err(|error| error.to_string())
        });
        if let Err(error) = result {
            let _ = authority.finish();
            return Err(anyhow::anyhow!("recovery lifecycle failure: {error:?}"));
        }
        authority
            .finish()
            .map_err(|error| anyhow::anyhow!("cannot release recovery authority: {error:?}"))?;
    }
    Ok(())
}

fn run_live_recovery(
    node_id: String,
    roster: Vec<mackesd_core::nebula_roster::RosterRow>,
    root: PathBuf,
    plan: mackesd_core::recovery::RecoveryPlan,
    evict: bool,
) -> anyhow::Result<()> {
    use mackesd_core::recovery::{self as rec, RecoveryApply as _};
    if evict {
        if let Some(row) = roster.iter().find(|r| r.node_id == node_id) {
            let fingerprints = rec::fingerprint_old_cert(&row.cert_pem).ok_or_else(|| {
                anyhow::anyhow!("--evict needs nebula-cert to fingerprint the old cert")
            })?;
            let req = rec::EvictRequest {
                workgroup_root: root,
                node_id: node_id.clone(),
                fingerprints,
                node_key_path: PathBuf::from(mackesd_core::node_key::DEFAULT_KEY_PATH),
            };
            let receipt = rec::LiveRecovery
                .blocklist_old_identity(&req)
                .map_err(|error| anyhow::anyhow!("immediate eviction failed: {error}"))?;
            println!(
                "  evicted old identity into the blocklist at {} (signed={})",
                receipt.blocklist_path.display(),
                receipt.signed
            );
        } else {
            println!("  --evict: no old roster row for {node_id} — nothing to blocklist");
        }
    }
    match rec::execute(&plan, &rec::LiveRecovery) {
        Ok(rec::RecoveryOutcome::Reenrolled { receipt }) => println!(
            "  re-enrolled {} into {} (overlay {})",
            receipt.node_id, receipt.mesh_id, receipt.overlay_ip
        ),
        Ok(rec::RecoveryOutcome::Blocked { reason }) => {
            println!("  no-op — blocked ({reason}); retry available")
        }
        Err(error) => return Err(anyhow::anyhow!("recovery re-enroll failed: {error}")),
    }
    Ok(())
}
