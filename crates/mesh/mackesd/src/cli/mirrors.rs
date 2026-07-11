//! `Mirrors` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.

/// Handle the `mirrors` subcommand.
#[allow(unreachable_code)]
pub fn run(
    json: bool,
    sync: Option<String>,
    sync_all: bool,
    write_repo: bool,
    repo_dir: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    {
        // PLANES-24 — the package-mirror catalog (core pack + TOML),
        // each with its file:// serving baseurl + last-sync state.
        use mackesd_core::mirrors;
        let root = mackesd_core::default_qnm_shared_root();
        let list = mirrors::load_mirrors(&root);
        // W62 — flip this node to self-serve: write each enabled mirror's
        // dnf .repo (local file:// first, upstream fallback).
        if write_repo {
            let dir =
                repo_dir.unwrap_or_else(|| std::path::PathBuf::from(mirrors::DEFAULT_REPO_DIR));
            let mut failures = 0;
            for m in list.iter().filter(|m| m.enabled) {
                match mirrors::write_dnf_repo(m, &root, &dir) {
                    Ok(p) => println!("wrote {} → {}", m.name, p.display()),
                    Err(e) => {
                        failures += 1;
                        eprintln!("mackesd mirrors: write .repo for {} failed: {e}", m.name);
                    }
                }
            }
            if failures > 0 {
                std::process::exit(1);
            }
            return Ok(());
        }
        // W63 — the one-puller sync path. `--sync <name>` / `--sync-all`
        // reposync the upstream into the mirror dir on the share, createrepo_c
        // the metadata, then stamp `.last-sync`.
        if sync.is_some() || sync_all {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64);
            let targets: Vec<&mirrors::Mirror> = if let Some(name) = &sync {
                match list.iter().find(|m| &m.name == name) {
                    Some(m) => vec![m],
                    None => {
                        eprintln!("mackesd mirrors: no mirror named '{name}'");
                        std::process::exit(1);
                    }
                }
            } else {
                list.iter().filter(|m| m.enabled).collect()
            };
            if targets.is_empty() {
                eprintln!("mackesd mirrors: nothing to sync (no enabled mirrors)");
                return Ok(());
            }
            let mut failures = 0;
            for m in targets {
                match mirrors::sync_mirror(&mirrors::SubprocessSync, m, &root, now_ms) {
                    Ok(r) => println!(
                        "synced {} — {} rpm(s) → {} (@{})",
                        r.name, r.rpm_count, r.served_baseurl, r.synced_at_ms
                    ),
                    Err(e) => {
                        failures += 1;
                        eprintln!("mackesd mirrors: sync {} failed: {e}", m.name);
                    }
                }
            }
            if failures > 0 {
                std::process::exit(1);
            }
            return Ok(());
        }
        if json {
            let rows: Vec<serde_json::Value> = list
                .iter()
                .map(|m| {
                    serde_json::json!({
                        "name": m.name,
                        "description": m.description,
                        "upstream": m.upstream,
                        "enabled": m.enabled,
                        "file_baseurl": m.file_baseurl(&root),
                        "last_sync_ms": m.last_sync_ms(&root),
                    })
                })
                .collect();
            println!("{}", serde_json::to_string(&rows)?);
        } else {
            println!("{:<14} {:<8} {}", "MIRROR", "ENABLED", "UPSTREAM");
            for m in &list {
                let synced = m
                    .last_sync_ms(&root)
                    .map_or_else(|| "never synced".to_string(), |ms| format!("synced @{ms}"));
                println!("{:<14} {:<8} {}", m.name, m.enabled, m.upstream);
                println!(
                    "               serves: {}  ({synced})",
                    m.file_baseurl(&root)
                );
            }
        }
        return Ok(());
    }
    Ok(())
}
