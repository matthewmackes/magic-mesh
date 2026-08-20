//! `Upgrade` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.

/// Handle the `upgrade` subcommand.
#[allow(unreachable_code)]
pub fn run(
    coordinate: bool,
    version: Option<String>,
    artifact_digest: Option<String>,
    artifact_selection_json: Option<String>,
) -> anyhow::Result<()> {
    {
        // PLANES-7 (W28) — publish a coordinated-upgrade intent the
        // fleet's watchers process (quorum + grace barrier).
        if !coordinate {
            eprintln!("mackesd upgrade: pass --coordinate to publish an upgrade intent");
            std::process::exit(1);
        }
        let root = mackesd_core::default_qnm_shared_root();
        let label = version.unwrap_or_else(|| "latest".to_string());
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
        use mackes_mesh_types::lifecycle::{LifecycleIntentKind, LifecyclePlanV1};
        let target_id = "upgrade-coordinator".to_string();
        let generation = now_ms.max(1);
        let lifecycle_plan = LifecyclePlanV1 {
            schema_version: 1,
            request_id: format!("upgrade-coordinate-{target_id}-{generation}"),
            target_id,
            intent: LifecycleIntentKind::Upgrade,
            generation,
            steps: vec!["configuration".into()],
        };
        let mut authority =
            mackesd_core::lifecycle_authority::LifecycleAuthority::begin(&root, lifecycle_plan)
                .map_err(|error| {
                    anyhow::anyhow!(
                        "cannot acquire lifecycle authority for upgrade coordination: {error:?}"
                    )
                })?;
        let result = authority.run_next(|_| {
            match artifact_selection_json {
                Some(selection) => mackesd_core::workers::upgrade_intent_watcher::write_intent_with_selection_json(
                    &root, &label, now_ms, &selection,
                ),
                None => mackesd_core::workers::upgrade_intent_watcher::write_intent_with_artifact_digest(
                    &root, &label, now_ms, artifact_digest.as_deref(),
                ),
            }
                .map(|path| println!("coordinated upgrade '{label}' — intent published at {} (each peer upgrades behind the quorum + grace barrier)", path.display()))
                .map_err(|error| error.to_string())
        });
        if let Err(error) = result {
            let _ = authority.finish();
            return Err(anyhow::anyhow!(
                "mackesd upgrade --coordinate lifecycle failure: {error:?}"
            ));
        }
        authority
            .finish()
            .map_err(|error| anyhow::anyhow!("cannot release lifecycle authority: {error:?}"))?;
        return Ok(());
    }
    Ok(())
}
