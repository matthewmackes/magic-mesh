//! `Validate` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Write the leader nudge without following a hostile final-component
/// symlink. `validation/` is intentionally sticky-world-writable so the
/// desktop user can request a run, which makes the marker itself an
/// attacker-controlled filesystem boundary for this root CLI.
fn write_run_now(root: &std::path::Path) -> std::io::Result<()> {
    use rustix::fs::{Mode, OFlags};
    use std::io::Write;

    let vdir = root.join("validation");
    std::fs::create_dir_all(&vdir)?;
    let fd = rustix::fs::open(
        vdir.join("runnow"),
        OFlags::WRONLY | OFlags::CREATE | OFlags::TRUNC | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR | Mode::RGRP | Mode::WGRP | Mode::ROTH | Mode::WOTH,
    )?;
    let mut file: std::fs::File = fd.into();
    file.write_all(b"mackesd")?;
    file.sync_all()
}

/// Handle the `validate` subcommand.
#[allow(unreachable_code)]
pub fn run(cmd: ValidateCmd) -> anyhow::Result<()> {
    {
        // PLANES-19 — the overlay-reachability verdict (W79/W80).
        use magic_fleet::validation;
        let root = mackesd_core::default_qnm_shared_root();
        match cmd {
            ValidateCmd::Run => {
                write_run_now(&root)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn run_now_refuses_a_final_symlink_without_touching_its_target() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("mesh");
        let vdir = root.join("validation");
        std::fs::create_dir_all(&vdir).expect("validation dir");
        let outside = tmp.path().join("outside");
        std::fs::write(&outside, b"keep me").expect("sentinel");
        symlink(&outside, vdir.join("runnow")).expect("symlink");

        let error = write_run_now(&root).expect_err("symlink must be refused");
        assert!(
            error.raw_os_error().is_some(),
            "symlink refusal should preserve the underlying OS error"
        );
        assert_eq!(std::fs::read(&outside).expect("sentinel read"), b"keep me");

        std::fs::remove_file(vdir.join("runnow")).expect("remove symlink");
        write_run_now(&root).expect("regular marker write");
        assert_eq!(
            std::fs::read(vdir.join("runnow")).expect("marker read"),
            b"mackesd"
        );
    }
}
