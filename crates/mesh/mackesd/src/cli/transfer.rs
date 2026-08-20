//! `Transfer` CLI verb handler.
//!
//! Extracted verbatim from `bin/mackesd.rs` (arch-1). Behaviour is unchanged;
//! only the location moved.
use crate::*;

/// TRANSFERS-1 — `mackesd transfer <sub>`: the CLI half of the typed verb set (§9
/// parity). Mutating verbs (submit/cancel/pause/resume/sync-pair add|remove) are
/// handed to the running daemon through the node-local inbox (the daemon is the
/// single ledger/store writer); `list` reads the persistent ledger or sync-pair
/// store directly. Both resolve the same node-local store the daemon uses, so the
/// CLI and the daemon share one queue.
pub fn run(cmd: TransferCmd) -> anyhow::Result<()> {
    run_with_store(cmd, &mackesd_core::workers::transfers::default_store_root())
}

fn run_with_store(cmd: TransferCmd, store_root: &std::path::Path) -> anyhow::Result<()> {
    use mackesd_core::workers::transfers::{
        discover_destinations, write_verb, Ledger, Method, SyncPair, SyncPairStore, TransferJob,
        TransferPolicy, TransferVerb,
    };

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
            write_verb(store_root, &TransferVerb::Submit(job))
                .with_context(|| format!("writing submit verb under {}", store_root.display()))?;
            println!("transfer submit: queued {id} ({method})");
            println!(
                "  the daemon's transfers worker picks it up; track with `mackesd transfer list`"
            );
        }
        TransferCmd::List { json } => {
            // A pure read: open the ledger directly (never `TransferQueue::open`,
            // which runs the daemon-only Running→Queued crash recovery).
            let ledger = Ledger::open(store_root).with_context(|| {
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
                store_root,
                &id,
                TransferVerb::Cancel(id.clone()),
                "cancel",
            )?;
        }
        TransferCmd::Pause { id } => {
            dispatch_transfer_lifecycle(store_root, &id, TransferVerb::Pause(id.clone()), "pause")?;
        }
        TransferCmd::Resume { id } => {
            dispatch_transfer_lifecycle(
                store_root,
                &id,
                TransferVerb::Resume(id.clone()),
                "resume",
            )?;
        }
        TransferCmd::SyncPair { cmd } => match cmd {
            SyncPairCmd::Add {
                id,
                interval,
                source,
                destination,
                bwlimit,
            } => {
                let every_secs = parse_interval_secs(&interval)?;
                validate_sync_pair_input(id.as_deref(), &source, &destination, bwlimit.as_deref())?;
                let id = match id {
                    Some(id) => id,
                    None => slug_pair_id(&source, &destination),
                };
                let policy = TransferPolicy {
                    bwlimit,
                    verify: false,
                };
                let pair = SyncPair::new(id, source, destination, every_secs, policy);
                let pair_id = pair.id.clone();
                write_verb(store_root, &TransferVerb::SaveSyncPair(pair)).with_context(|| {
                    format!("writing save-sync-pair verb under {}", store_root.display())
                })?;
                println!(
                    "transfer sync-pair add: queued {pair_id} every {every_secs}s (the daemon saves it on its next tick)"
                );
            }
            SyncPairCmd::Remove { id } => {
                let store = SyncPairStore::open(store_root).with_context(|| {
                    format!("opening the sync-pair store at {}", store_root.display())
                })?;
                if store.get(&id).is_none() {
                    anyhow::bail!(
                        "no sync pair `{id}` in the store (see `mackesd transfer sync-pair list`)"
                    );
                }
                write_verb(store_root, &TransferVerb::RemoveSyncPair(id.clone())).with_context(
                    || {
                        format!(
                            "writing remove-sync-pair verb under {}",
                            store_root.display()
                        )
                    },
                )?;
                println!(
                    "transfer sync-pair remove: requested for {id} (the daemon applies it on its next tick)"
                );
            }
            SyncPairCmd::List { json } => {
                let store = SyncPairStore::open(store_root).with_context(|| {
                    format!("opening the sync-pair store at {}", store_root.display())
                })?;
                let pairs = store.load_all();
                if json {
                    println!("{}", serde_json::to_string_pretty(&pairs)?);
                } else if pairs.is_empty() {
                    println!("no sync pairs saved");
                } else {
                    println!("{:<24} {:<10} {:<12} SOURCE -> DEST", "ID", "EVERY", "LAST");
                    for p in &pairs {
                        let last = p
                            .last_fired_ms
                            .map_or_else(|| "never".to_owned(), |ms| ms.to_string());
                        let bw = p
                            .policy
                            .bwlimit
                            .as_deref()
                            .map_or_else(String::new, |b| format!(" bwlimit={b}"));
                        println!(
                            "{:<24} {:<10} {:<12} {} -> {}{bw}",
                            p.id,
                            format!("{}s", p.every_secs),
                            last,
                            p.source,
                            p.dest
                        );
                    }
                }
            }
        },
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

/// Parse a sync-pair interval. Accepts a positive second count or a unit suffix
/// (`s`/`m`/`h`/`d`). Zero, empty, negative, and unknown units refuse.
fn parse_interval_secs(raw: &str) -> anyhow::Result<u64> {
    parse_interval_secs_opt(raw).ok_or_else(|| {
        anyhow::anyhow!(
            "malformed interval `{raw}` (expected a positive duration such as 30s, 5m, 1h, or seconds)"
        )
    })
}

fn parse_interval_secs_opt(raw: &str) -> Option<u64> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if s.bytes().all(|b| b.is_ascii_digit()) {
        let n: u64 = s.parse().ok()?;
        return (n >= 1).then_some(n);
    }
    let split = s.find(|c: char| !c.is_ascii_digit())?;
    if split == 0 {
        return None;
    }
    let (num, unit) = s.split_at(split);
    let n: u64 = num.parse().ok()?;
    if n == 0 {
        return None;
    }
    let mult: u64 = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        _ => return None,
    };
    n.checked_mul(mult).filter(|v| *v >= 1)
}

/// Refuse malformed producer input before it becomes an inbox record. The
/// daemon store repeats these checks, but rejecting here prevents a CLI request
/// from looking queued while the worker later drops it.
fn validate_sync_pair_input(
    id: Option<&str>,
    source: &str,
    destination: &str,
    bwlimit: Option<&str>,
) -> anyhow::Result<()> {
    if source.trim().is_empty() || destination.trim().is_empty() {
        anyhow::bail!("sync pair requires non-empty source and destination");
    }
    if source.as_bytes().contains(&0) || destination.as_bytes().contains(&0) {
        anyhow::bail!("sync pair source and destination must not contain NUL bytes");
    }
    if let Some(limit) = bwlimit {
        if !valid_sync_pair_bwlimit(limit) {
            anyhow::bail!("invalid sync pair bwlimit `{limit}`");
        }
    }
    if let Some(id) = id {
        let id = id.trim();
        let valid = !id.is_empty()
            && id != "."
            && id != ".."
            && id.len() <= 120
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
        if !valid {
            anyhow::bail!("invalid sync pair id `{id}`");
        }
    }
    Ok(())
}

fn valid_sync_pair_bwlimit(raw: &str) -> bool {
    !raw.is_empty()
        && raw.len() <= 32
        && raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn slug_pair_id(source: &str, dest: &str) -> String {
    let raw = format!("{source}-{dest}");
    let slug: String = raw
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(80)
        .collect();
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "sync-pair".to_owned()
    } else {
        slug.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackesd_core::workers::transfers::{
        take_verbs, SyncPair, SyncPairStore, TransferPolicy, TransferVerb,
    };

    #[test]
    fn malformed_intervals_refuse() {
        for raw in ["", "abc", "0", "0s", "-5m", "10y", "1.5h", "h"] {
            assert!(
                parse_interval_secs(raw).is_err(),
                "interval `{raw}` must refuse"
            );
        }
    }

    #[test]
    fn well_formed_intervals_parse() {
        assert_eq!(parse_interval_secs("30s").unwrap(), 30);
        assert_eq!(parse_interval_secs("5m").unwrap(), 300);
        assert_eq!(parse_interval_secs("1h").unwrap(), 3600);
        assert_eq!(parse_interval_secs("2d").unwrap(), 172_800);
        assert_eq!(parse_interval_secs("90").unwrap(), 90);
    }

    #[test]
    fn sync_pair_add_posts_save_verb() {
        let tmp = tempfile::tempdir().unwrap();
        run_with_store(
            TransferCmd::SyncPair {
                cmd: SyncPairCmd::Add {
                    id: Some("docs".into()),
                    interval: "15m".into(),
                    source: "/src".into(),
                    destination: "/dst".into(),
                    bwlimit: Some("2m".into()),
                },
            },
            tmp.path(),
        )
        .unwrap();
        let verbs = take_verbs(tmp.path());
        assert_eq!(verbs.len(), 1);
        match &verbs[0] {
            TransferVerb::SaveSyncPair(pair) => {
                assert_eq!(pair.id, "docs");
                assert_eq!(pair.source, "/src");
                assert_eq!(pair.dest, "/dst");
                assert_eq!(pair.every_secs, 900);
                assert_eq!(pair.policy.bwlimit.as_deref(), Some("2m"));
            }
            other => panic!("expected SaveSyncPair, got {other:?}"),
        }
    }

    #[test]
    fn sync_pair_add_refuses_malformed_interval_without_writing() {
        let tmp = tempfile::tempdir().unwrap();
        let err = run_with_store(
            TransferCmd::SyncPair {
                cmd: SyncPairCmd::Add {
                    id: Some("docs".into()),
                    interval: "nope".into(),
                    source: "/src".into(),
                    destination: "/dst".into(),
                    bwlimit: None,
                },
            },
            tmp.path(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("malformed interval"), "got {err}");
        assert!(take_verbs(tmp.path()).is_empty());
    }

    #[test]
    fn sync_pair_add_refuses_invalid_store_inputs_without_writing() {
        for (id, source, destination, expected, bwlimit) in [
            (
                Some("../escape"),
                "/src",
                "/dst",
                "invalid sync pair id",
                None,
            ),
            (Some("docs"), "", "/dst", "non-empty source", None),
            (Some("docs"), "/src\0", "/dst", "NUL bytes", None),
            (Some("docs"), "/src", "/dst", "bwlimit", Some("1m;rm")),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let err = run_with_store(
                TransferCmd::SyncPair {
                    cmd: SyncPairCmd::Add {
                        id: id.map(str::to_owned),
                        interval: "15m".into(),
                        source: source.into(),
                        destination: destination.into(),
                        bwlimit: bwlimit.map(str::to_owned),
                    },
                },
                tmp.path(),
            )
            .unwrap_err();
            assert!(err.to_string().contains(expected), "got {err}");
            assert!(take_verbs(tmp.path()).is_empty());
        }
    }

    #[test]
    fn sync_pair_remove_unknown_id_refuses() {
        let tmp = tempfile::tempdir().unwrap();
        let err = run_with_store(
            TransferCmd::SyncPair {
                cmd: SyncPairCmd::Remove {
                    id: "missing".into(),
                },
            },
            tmp.path(),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("no sync pair `missing`"),
            "got {err}"
        );
        assert!(take_verbs(tmp.path()).is_empty());
    }

    #[test]
    fn sync_pair_remove_known_id_posts_remove_verb() {
        let tmp = tempfile::tempdir().unwrap();
        let store = SyncPairStore::open(tmp.path()).unwrap();
        store
            .upsert(&SyncPair::new(
                "docs",
                "/src",
                "/dst",
                60,
                TransferPolicy::default(),
            ))
            .unwrap();
        run_with_store(
            TransferCmd::SyncPair {
                cmd: SyncPairCmd::Remove { id: "docs".into() },
            },
            tmp.path(),
        )
        .unwrap();
        let verbs = take_verbs(tmp.path());
        assert_eq!(verbs, vec![TransferVerb::RemoveSyncPair("docs".into())]);
    }

    #[test]
    fn sync_pair_list_reads_the_store() {
        let tmp = tempfile::tempdir().unwrap();
        let store = SyncPairStore::open(tmp.path()).unwrap();
        store
            .upsert(&SyncPair::new(
                "docs",
                "/src",
                "/dst",
                60,
                TransferPolicy::default(),
            ))
            .unwrap();
        run_with_store(
            TransferCmd::SyncPair {
                cmd: SyncPairCmd::List { json: true },
            },
            tmp.path(),
        )
        .unwrap();
    }
}
