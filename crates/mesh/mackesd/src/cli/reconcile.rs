//! `Reconcile` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `reconcile` subcommand.
#[allow(unreachable_code)]
pub fn run(
    once: bool,
    workgroup_root: Option<PathBuf>,
    node_id: Option<String>,
    db_path: PathBuf,
) -> anyhow::Result<()> {
    {
        // Phase 12.5 wiring — the reconcile worker thread.
        let workgroup_root = workgroup_root.unwrap_or_else(mackesd_core::default_qnm_shared_root);
        let node_id = node_id.unwrap_or_else(default_node_id);

        if once {
            // Single-tick dry-run path: useful for CI smoke
            // tests + operator inspection. No background
            // thread, no signal handler.
            let outcome = mackesd_core::worker::tick(&workgroup_root, &node_id, &db_path)
                .with_context(|| format!("one-shot reconcile tick on {}", db_path.display()))?;
            println!("{}", serde_json::to_string_pretty(&outcome)?);
        } else {
            // Long-running path: spawn the worker, install a
            // SIGTERM/SIGINT handler that flips the shutdown
            // flag, then block until the worker exits.
            use std::sync::atomic::{AtomicBool, Ordering};
            use std::sync::Arc;
            let shutdown = Arc::new(AtomicBool::new(false));
            install_signal_handlers(Arc::clone(&shutdown))?;
            let handle = mackesd_core::worker::spawn_reconcile_worker(
                workgroup_root,
                node_id,
                db_path,
                Arc::clone(&shutdown),
            );
            // Wait for either the worker to exit (DB went away,
            // panic — we don't panic by design) or the signal
            // handler to flip shutdown. JoinHandle::join blocks
            // until the thread returns either way.
            if let Err(e) = handle.join() {
                eprintln!("mackesd reconcile: worker thread panicked: {e:?}");
                std::process::exit(1);
            }
            // If we exited because the worker thread itself
            // crashed unexpectedly (e.g. someone moved the db
            // file out from under us), the loop logged the
            // error before returning. Either way: exit 0 on a
            // clean shutdown-flag path.
            if !shutdown.load(Ordering::Relaxed) {
                // Worker exited but no shutdown was requested.
                // Treat as a soft failure.
                eprintln!("mackesd reconcile: worker exited without shutdown request");
                std::process::exit(1);
            }
        }
    }
    Ok(())
}
