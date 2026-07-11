//! `FleetPushSetting` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `fleet-push-setting` subcommand.
#[allow(unreachable_code)]
pub fn run(
    key: String,
    value: String,
    peers: String,
    author: Option<String>,
    dry_run: bool,
    db_path: PathBuf,
) -> anyhow::Result<()> {
    {
        // v2.0.0 Phase G.4 — fleet push-setting CLI. Writes the
        // matching desired_config row + fleet_settings_apply_log
        // entries, then prints the JSON plan.
        let mut conn = mackesd_core::store::open(&db_path)
            .with_context(|| format!("opening store at {}", db_path.display()))?;
        let author = author.unwrap_or_else(default_node_id);
        let plan = mackesd_core::fleet::plan_push(&key, &value, &peers, &author);
        if !dry_run {
            mackesd_core::fleet::record_push(&mut conn, &plan).context("recording fleet push")?;
        }
        let report = serde_json::json!({
            "fleet_push_setting": {
                "key":          &plan.key,
                "value":        &plan.value,
                "peers":        &plan.peers,
                "author":       &plan.author,
                "revision_id":  &plan.revision_id,
                "dry_run":      dry_run,
            }
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
    }
    Ok(())
}
