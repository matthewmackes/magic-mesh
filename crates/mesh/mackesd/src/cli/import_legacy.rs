//! `ImportLegacy` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `import-legacy` subcommand.
#[allow(unreachable_code)]
pub fn run(dry_run: bool, db_path: PathBuf) -> anyhow::Result<()> {
    {
        // Phase 12.13.2 — inventory the legacy caches under the
        // three canonical roots, then either preview the plan
        // (dry-run, default) or write desired-state rows into
        // the store. The importer is conservative: it only
        // creates node rows for mesh-related artifacts whose
        // filename carries an obvious peer identifier; it never
        // overwrites an existing row.
        let roots = mackesd_core::legacy_inventory::default_roots();
        let artifacts = mackesd_core::legacy_inventory::inventory(&roots);
        let mesh_artifacts: Vec<_> = artifacts.iter().filter(|a| a.mesh_data).collect();
        let candidate_node_names = derive_legacy_node_names(&mesh_artifacts);
        if dry_run {
            let report = serde_json::json!({
                "import_legacy_dry_run": true,
                "candidate_paths":       roots
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>(),
                "artifacts_found":       artifacts.len(),
                "mesh_artifacts":        mesh_artifacts.len(),
                "would_import_records":  candidate_node_names.len(),
                "would_insert_nodes":    &candidate_node_names,
            });
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            let mut conn = mackesd_core::store::open(&db_path)
                .with_context(|| format!("opening store at {}", db_path.display()))?;
            let existing: std::collections::BTreeSet<String> =
                mackesd_core::store::list_nodes(&conn)?
                    .into_iter()
                    .map(|n| n.node_id)
                    .collect();
            let mut inserted = Vec::new();
            let mut skipped = Vec::new();
            for name in &candidate_node_names {
                let node_id = format!("peer:{name}");
                if existing.contains(&node_id) {
                    skipped.push(node_id);
                    continue;
                }
                mackesd_core::store::upsert_node(
                    &conn,
                    &node_id,
                    name,
                    // Placeholder key — a subsequent enrollment
                    // will replace this with the real Ed25519
                    // public-key fingerprint.
                    "legacy-import",
                    None,
                )?;
                inserted.push(node_id);
            }
            let payload = serde_json::json!({
                "event":    "import_legacy",
                "inserted": &inserted,
                "skipped":  &skipped,
            })
            .to_string();
            mackesd_core::store::insert_event(
                &mut conn,
                "lifecycle",
                &default_node_id(),
                &payload,
            )?;
            let report = serde_json::json!({
                "import_legacy_dry_run": false,
                "artifacts_found":       artifacts.len(),
                "mesh_artifacts":        mesh_artifacts.len(),
                "inserted_nodes":        inserted,
                "skipped_nodes":         skipped,
            });
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
    }
    Ok(())
}
