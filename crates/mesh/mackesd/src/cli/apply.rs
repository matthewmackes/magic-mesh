//! `Apply` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.

/// Handle the `apply` subcommand.
#[allow(unreachable_code)]
pub fn run(dry_run: bool) -> anyhow::Result<()> {
    {
        if dry_run {
            // Phase 12.7.4 — run validation against an empty
            // snapshot today; once the store wires the
            // serialized desired-config row in, the dry-run
            // path returns the real diff + event-log preview.
            let snapshot = mackesd_core::topology::DesiredSnapshot::default();
            let errors = mackesd_core::validation::validate(&snapshot);
            let report = serde_json::json!({
                "dry_run": true,
                "validation_errors": errors.len(),
                "would_apply_revisions": 0,
            });
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            eprintln!(
                "mackesd: non-dry-run apply requires the reconcile loop \
                     (Phase 12.5) — use `mackesd apply --dry-run` for the \
                     validation + plan preview."
            );
            std::process::exit(2);
        }
    }
    Ok(())
}
