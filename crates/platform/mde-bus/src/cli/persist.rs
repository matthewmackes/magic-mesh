//! `mde-bus persist` — diagnostics on the per-peer SQLite index +
//! per-topic file tree (BUS-1.4 persistence layer).
//!
//! Read-only — never deletes / never rewrites. Operators looking
//! for corruption or out-of-sync index state run `verify`; CI
//! gates can gate on the nonzero exit when divergence is detected.

use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use clap::Subcommand;

use crate::persist::Persist;

/// CLI sub-verbs for `mde-bus persist`.
#[derive(Subcommand, Debug)]
pub enum PersistOp {
    /// Walk the persistence layer + flag divergence between the
    /// SQLite index and the file tree. Prints two lists:
    /// `files_without_rows` (the file tree has it but the index
    /// doesn't — likely external write or index corruption) +
    /// `rows_without_files` (the index has it but the file tree
    /// doesn't — likely external delete or retention bug). Exits
    /// nonzero when any divergence is found so CI can gate on it.
    Verify {
        /// Override the bus-root directory.
        #[arg(long)]
        bus_root: Option<PathBuf>,
        /// Emit a JSON `{files_without_rows, rows_without_files}`
        /// object instead of the human-readable summary. Useful
        /// for piping to `jq` from CI scripts.
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Print the total number of indexed messages in the SQLite
    /// store. Read-only; symmetric with `audit count`. Useful for
    /// monitoring + dashboards ("how many bus messages persist
    /// right now?") + capacity planning ("at the current pace,
    /// when will we hit the retention quota?").
    Count {
        /// Override the bus-root directory.
        #[arg(long)]
        bus_root: Option<PathBuf>,
        /// Emit `{"count":N}` instead of the bare integer for
        /// jq-pipe symmetry with `audit count --json`.
        #[arg(long, default_value_t = false)]
        json: bool,
    },
}

fn resolve_bus_root(arg: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = arg {
        return Ok(p);
    }
    crate::default_data_dir().ok_or_else(|| anyhow!("no $HOME / $XDG_DATA_HOME — pass --bus-root"))
}

/// Execute the `persist` verb. Read-only.
pub fn run(op: PersistOp) -> Result<()> {
    match op {
        PersistOp::Verify { bus_root, json } => {
            let root = resolve_bus_root(bus_root)?;
            let p = Persist::open(root.clone())
                .with_context(|| format!("open persist at {}", root.display()))?;
            let report = p
                .detect_divergence()
                .with_context(|| format!("scan {}", root.display()))?;
            let clean = report.is_clean();
            if json {
                let val = serde_json::json!({
                    "files_without_rows": report.files_without_rows,
                    "rows_without_files": report.rows_without_files,
                });
                println!("{val}");
            } else if clean {
                println!("OK — persist clean (0 divergent rows, 0 orphan files)");
            } else {
                println!(
                    "DIVERGENT — {} file(s) without rows, {} row(s) without files",
                    report.files_without_rows.len(),
                    report.rows_without_files.len(),
                );
                for f in &report.files_without_rows {
                    println!("  file-without-row: {f}");
                }
                for r in &report.rows_without_files {
                    println!("  row-without-file: {r}");
                }
            }
            if !clean {
                return Err(anyhow!(
                    "persist verify: {} file(s) without rows, {} row(s) without files",
                    report.files_without_rows.len(),
                    report.rows_without_files.len(),
                ));
            }
        }
        PersistOp::Count { bus_root, json } => {
            let root = resolve_bus_root(bus_root)?;
            let p = Persist::open(root.clone())
                .with_context(|| format!("open persist at {}", root.display()))?;
            let n = p
                .count()
                .with_context(|| format!("count messages at {}", root.display()))?;
            if json {
                println!("{{\"count\":{n}}}");
            } else {
                println!("{n}");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_empty_bus_root_returns_clean() {
        let tmp = tempfile::tempdir().unwrap();
        // Open Persist to seed the SQLite index file; then no
        // publishes happen → 0 rows + 0 files → clean.
        let _p = Persist::open(tmp.path().to_path_buf()).unwrap();
        drop(_p);
        let r = run(PersistOp::Verify {
            bus_root: Some(tmp.path().to_path_buf()),
            json: false,
        });
        assert!(r.is_ok(), "empty persist should be clean: {r:?}");
    }

    #[test]
    fn verify_with_orphan_file_returns_err() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persist::open(tmp.path().to_path_buf()).unwrap();
        drop(p);
        // Drop a JSON file with no matching SQLite row.
        let orphan_dir = tmp.path().join("orphan");
        std::fs::create_dir_all(&orphan_dir).unwrap();
        std::fs::write(orphan_dir.join("01XYZ.json"), "{}").unwrap();
        let r = run(PersistOp::Verify {
            bus_root: Some(tmp.path().to_path_buf()),
            json: false,
        });
        // Divergence detected → run returns Err.
        assert!(r.is_err());
    }

    #[test]
    fn verify_json_path_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let _p = Persist::open(tmp.path().to_path_buf()).unwrap();
        drop(_p);
        let r = run(PersistOp::Verify {
            bus_root: Some(tmp.path().to_path_buf()),
            json: true,
        });
        assert!(r.is_ok());
    }

    #[test]
    fn count_on_empty_persist_returns_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let _p = Persist::open(tmp.path().to_path_buf()).unwrap();
        drop(_p);
        // Both default + JSON paths exercise the verb. Output goes
        // to stdout; we just confirm dispatch + no error.
        let r = run(PersistOp::Count {
            bus_root: Some(tmp.path().to_path_buf()),
            json: false,
        });
        assert!(r.is_ok());
        let r = run(PersistOp::Count {
            bus_root: Some(tmp.path().to_path_buf()),
            json: true,
        });
        assert!(r.is_ok());
    }

    #[test]
    fn count_returns_actual_message_count() {
        use crate::hooks::config::Priority;
        let tmp = tempfile::tempdir().unwrap();
        let p = Persist::open(tmp.path().to_path_buf()).unwrap();
        for i in 0..3 {
            p.write("t/x", Priority::Default, None, Some(&i.to_string()))
                .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        // 6 = 3 t/x messages + 3 audit/<peer> records (one auto-emitted
        // per publish, EPIC-BUS-EXT-AUDIT-BUS).
        assert_eq!(p.count().unwrap(), 6);
        drop(p);
        // Dispatch the verb — count() is exercised, output to stdout.
        let r = run(PersistOp::Count {
            bus_root: Some(tmp.path().to_path_buf()),
            json: false,
        });
        assert!(r.is_ok());
    }
}
