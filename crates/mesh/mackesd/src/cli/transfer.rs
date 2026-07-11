//! `Transfer` CLI verb handler.
//!
//! Extracted verbatim from `bin/mackesd.rs` (arch-1). Behaviour is unchanged;
//! only the location moved.
use crate::*;

/// TRANSFERS-1 — `mackesd transfer <sub>`: the CLI half of the typed verb set (§9
/// parity). Mutating verbs (submit/cancel/pause/resume) are handed to the running
/// daemon through the node-local inbox (the daemon is the single ledger writer);
/// `list` reads the persistent ledger directly. Both resolve the same node-local
/// store the daemon uses, so the CLI and the daemon share one queue.
pub fn run(cmd: TransferCmd) -> anyhow::Result<()> {
    use mackesd_core::workers::transfers::{
        default_store_root, discover_destinations, write_verb, Ledger, Method, TransferJob,
        TransferPolicy, TransferVerb,
    };

    let store_root = default_store_root();

    match cmd {
        TransferCmd::Submit {
            source,
            dest,
            method,
            bwlimit,
            verify,
        } => {
            let method: Method = method.parse().map_err(|e: String| anyhow::anyhow!(e))?;
            let policy = TransferPolicy { bwlimit, verify };
            let job = TransferJob::new(source, dest, method, policy);
            let id = job.id.clone();
            write_verb(&store_root, &TransferVerb::Submit(job))
                .with_context(|| format!("writing submit verb under {}", store_root.display()))?;
            println!("transfer submit: queued {id} ({method})");
            println!(
                "  the daemon's transfers worker picks it up; track with `mackesd transfer list`"
            );
        }
        TransferCmd::List { json } => {
            // A pure read: open the ledger directly (never `TransferQueue::open`,
            // which runs the daemon-only Running→Queued crash recovery).
            let ledger = Ledger::open(&store_root).with_context(|| {
                format!("opening the transfers ledger at {}", store_root.display())
            })?;
            let jobs = ledger.load_all();
            if json {
                println!("{}", serde_json::to_string_pretty(&jobs)?);
            } else if jobs.is_empty() {
                println!("no transfers in the ledger");
            } else {
                println!(
                    "{:<26} {:<8} {:<16} SOURCE -> DEST",
                    "ID", "STATE", "METHOD"
                );
                for j in &jobs {
                    let pct = j.progress.map_or_else(String::new, |p| format!(" {p}%"));
                    println!(
                        "{:<26} {:<8} {:<16} {} -> {}{pct}",
                        j.id, j.state, j.method, j.source, j.dest
                    );
                    if let Some(err) = &j.error {
                        println!("    ! {err}");
                    }
                }
            }
        }
        TransferCmd::Destinations { json } => {
            let workgroup_root = mackesd_core::default_qnm_shared_root();
            let self_host = std::env::var("HOSTNAME").ok();
            let destinations = discover_destinations(&workgroup_root, self_host.as_deref());
            if json {
                println!("{}", serde_json::to_string_pretty(&destinations)?);
            } else if destinations.is_empty() {
                println!("no transfer destinations discovered");
            } else {
                println!("{:<18} {:<14} {:<16} DEST", "ID", "KIND", "METHOD");
                for d in &destinations {
                    println!(
                        "{:<18} {:<14} {:<16} {}",
                        d.id,
                        format!("{:?}", d.kind).to_ascii_lowercase(),
                        d.method,
                        d.dest
                    );
                }
            }
        }
        TransferCmd::Cancel { id } => {
            dispatch_transfer_lifecycle(
                &store_root,
                &id,
                TransferVerb::Cancel(id.clone()),
                "cancel",
            )?;
        }
        TransferCmd::Pause { id } => {
            dispatch_transfer_lifecycle(
                &store_root,
                &id,
                TransferVerb::Pause(id.clone()),
                "pause",
            )?;
        }
        TransferCmd::Resume { id } => {
            dispatch_transfer_lifecycle(
                &store_root,
                &id,
                TransferVerb::Resume(id.clone()),
                "resume",
            )?;
        }
    }
    Ok(())
}

/// Hand a lifecycle verb (cancel/pause/resume) to the daemon after an honest early
/// existence check against the ledger (a typo'd id fails fast rather than silently
/// dropping a verb the daemon would refuse).
fn dispatch_transfer_lifecycle(
    store_root: &std::path::Path,
    id: &str,
    verb: mackesd_core::workers::transfers::TransferVerb,
    name: &str,
) -> anyhow::Result<()> {
    use mackesd_core::workers::transfers::{write_verb, Ledger};
    let ledger = Ledger::open(store_root)
        .with_context(|| format!("opening the transfers ledger at {}", store_root.display()))?;
    if ledger.get(id).is_none() {
        anyhow::bail!("no transfer `{id}` in the ledger (see `mackesd transfer list`)");
    }
    write_verb(store_root, &verb)
        .with_context(|| format!("writing {name} verb under {}", store_root.display()))?;
    println!("transfer {name}: requested for {id} (the daemon applies it on its next tick)");
    Ok(())
}
