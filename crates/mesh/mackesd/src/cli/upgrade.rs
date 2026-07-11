//! `Upgrade` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.

/// Handle the `upgrade` subcommand.
#[allow(unreachable_code)]
pub fn run(coordinate: bool, version: Option<String>) -> anyhow::Result<()> {
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
        match mackesd_core::workers::upgrade_intent_watcher::write_intent(&root, &label, now_ms) {
            Ok(p) => println!(
                "coordinated upgrade '{label}' — intent published at {} \
                     (each peer upgrades behind the quorum + grace barrier)",
                p.display()
            ),
            Err(e) => {
                eprintln!("mackesd upgrade --coordinate: {e}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }
    Ok(())
}
