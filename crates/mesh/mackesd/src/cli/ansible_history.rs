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

/// `~/QNM-Shared/.qnm-sync/ansible-runs/` (or its
/// `$QNM_SHARED_ROOT` override). Same resolution the retired
/// Workbench's run-history panel used — the on-disk layout is
/// the load-bearing contract.
fn ansible_runs_root() -> PathBuf {
    let base = std::env::var("QNM_SHARED_ROOT").map(PathBuf::from).ok();
    let base = base.unwrap_or_else(|| {
        std::env::var("HOME")
            .map(|h| PathBuf::from(h).join("QNM-Shared"))
            .unwrap_or_else(|_| PathBuf::from("/var/empty"))
    });
    base.join(".qnm-sync").join("ansible-runs")
}

/// Walk every peer subdir + parse each `*.json` as a record.
/// Returns the union sorted by timestamp descending. Errors
/// are swallowed silently (no peer dir / unreadable file
/// just drops that row) — matches the panel's
/// non-aborting walk.
fn collect_ansible_history(root: &std::path::Path) -> Vec<serde_json::Value> {
    let Ok(peers) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    for peer_entry in peers.flatten() {
        let Ok(ft) = peer_entry.file_type() else {
            continue;
        };
        if !ft.is_dir() {
            continue;
        }
        let peer_name = peer_entry
            .file_name()
            .to_str()
            .map(str::to_string)
            .unwrap_or_default();
        if peer_name.is_empty() {
            continue;
        }
        let peer_dir = peer_entry.path();
        let Ok(files) = std::fs::read_dir(&peer_dir) else {
            continue;
        };
        for file_entry in files.flatten() {
            let path = file_entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&raw) else {
                continue;
            };
            // Inject the peer name + source path so the JSON
            // row is self-describing (the panel does the same
            // mapping).
            if let Some(obj) = v.as_object_mut() {
                obj.insert("peer".into(), serde_json::Value::String(peer_name.clone()));
                obj.insert(
                    "_source_path".into(),
                    serde_json::Value::String(path.to_string_lossy().into_owned()),
                );
            }
            rows.push(v);
        }
    }
    rows.sort_by(|a, b| {
        let ts_a = a.get("timestamp").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let ts_b = b.get("timestamp").and_then(|v| v.as_f64()).unwrap_or(0.0);
        ts_b.partial_cmp(&ts_a).unwrap_or(std::cmp::Ordering::Equal)
    });
    rows
}

fn print_ansible_history_table(rows: &[serde_json::Value]) {
    if rows.is_empty() {
        println!("(no ansible-pull runs recorded)");
        return;
    }
    println!(
        "{:<16} {:<24} {:<6} {:<8} {:<8} {:<10}",
        "peer", "playbook", "exit", "changed", "ok", "trigger"
    );
    for r in rows {
        let peer = r
            .get("peer")
            .and_then(|v| v.as_str())
            .unwrap_or("-")
            .chars()
            .take(16)
            .collect::<String>();
        let playbook = r
            .get("playbook")
            .and_then(|v| v.as_str())
            .unwrap_or("-")
            .chars()
            .take(24)
            .collect::<String>();
        let exit = r.get("exit_code").and_then(|v| v.as_i64()).unwrap_or(0);
        let changed = r.get("changed").and_then(|v| v.as_u64()).unwrap_or(0);
        let ok = r.get("ok").and_then(|v| v.as_u64()).unwrap_or(0);
        let trigger = r
            .get("triggered_by")
            .and_then(|v| v.as_str())
            .unwrap_or("pull");
        println!("{peer:<16} {playbook:<24} {exit:<6} {changed:<8} {ok:<8} {trigger:<10}");
    }
}
