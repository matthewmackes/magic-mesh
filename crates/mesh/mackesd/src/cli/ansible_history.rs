//! `AnsibleHistory` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `ansible-history` subcommand.
#[allow(unreachable_code)]
pub fn run(cmd: AnsibleHistoryCmd) -> anyhow::Result<()> {
    {
        // CB-1.5.c follow-up — walks QNM-Shared
        // ansible-runs/<peer>/*.json and emits the union as
        // a sorted JSON array (or human-readable table).
        match cmd {
            AnsibleHistoryCmd::List { json } => {
                let root = ansible_runs_root();
                let rows = collect_ansible_history(&root);
                if json {
                    println!("{}", serde_json::to_string_pretty(&rows)?);
                } else {
                    print_ansible_history_table(&rows);
                }
            }
        }
    }
    Ok(())
}
