//! `Validate` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `validate` subcommand.
#[allow(unreachable_code)]
pub fn run(cmd: ValidateCmd) -> anyhow::Result<()> {
    {
        // PLANES-19 — the overlay-reachability verdict (W79/W80).
        use magic_fleet::validation;
        let root = mackesd_core::default_qnm_shared_root();
        match cmd {
            ValidateCmd::Run => {
                let vdir = root.join("validation");
                std::fs::create_dir_all(&vdir)?;
                std::fs::write(vdir.join("runnow"), b"mackesd")?;
                println!("requested a fresh overlay-reachability run (the leader mints it)");
                return Ok(());
            }
            ValidateCmd::Status { json } => {
                let latest = validation::list_run_ids(&root).into_iter().next_back();
                let Some(id) = latest else {
                    if json {
                        println!("{}", serde_json::json!({ "run_id": null }));
                    } else {
                        println!("no validation run yet (mded validate run to request one)");
                    }
                    return Ok(());
                };
                let Some(run) = validation::read_run(&root, &id) else {
                    anyhow::bail!("run {id} has no run.json");
                };
                let rows = validation::read_rows(&root, &id);
                let verdict = validation::aggregate(&run, &rows);
                let edge = |e: &validation::Edge| serde_json::json!({ "from": e.from, "to": e.to });
                if json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "run_id": run.run_id,
                            "kind": run.kind,
                            "at": run.at,
                            "passed": verdict.passed(),
                            "reachable": verdict.reachable.iter().map(edge).collect::<Vec<_>>(),
                            "failed": verdict.failed.iter().map(edge).collect::<Vec<_>>(),
                            "missing_reporters": verdict.missing_reporters,
                        })
                    );
                } else {
                    println!(
                        "run {} ({:?}) — {}",
                        run.run_id,
                        run.kind,
                        if verdict.passed() { "PASS" } else { "FAIL" }
                    );
                    println!(
                        "  reachable edges: {}  failed: {}  missing reporters: {}",
                        verdict.reachable.len(),
                        verdict.failed.len(),
                        verdict.missing_reporters.len()
                    );
                    for e in &verdict.failed {
                        println!("  FAIL  {} → {}", e.from, e.to);
                    }
                }
                return Ok(());
            }
        }
    }
    Ok(())
}
