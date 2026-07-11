//! `Playbooks` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `playbooks` subcommand.
#[allow(unreachable_code)]
pub fn run(cmd: PlaybooksCmd) -> anyhow::Result<()> {
    {
        // CB-1.5.b follow-up — curated playbook surface.
        match cmd {
            PlaybooksCmd::List { json } => {
                let root = playbooks_root();
                let mut entries = enumerate_playbook_roles(&root);
                entries.sort();
                let rows: Vec<serde_json::Value> = entries
                    .into_iter()
                    .map(|name| {
                        let description = playbook_description(&name);
                        serde_json::json!({
                            "name":        name,
                            "description": description,
                        })
                    })
                    .collect();
                if json {
                    println!("{}", serde_json::to_string_pretty(&rows)?);
                } else if rows.is_empty() {
                    println!("(no curated playbooks under {})", root.display());
                } else {
                    for r in &rows {
                        let name = r.get("name").and_then(|v| v.as_str()).unwrap_or("");
                        let desc = r.get("description").and_then(|v| v.as_str()).unwrap_or("");
                        println!("{name:<28} {desc}");
                    }
                }
            }
            PlaybooksCmd::Run { name } => {
                // Spawn ansible-pull directly so the user sees
                // its progress streaming. Exit with whatever
                // ansible-pull exited with.
                let status = std::process::Command::new("ansible-pull")
                    .args(["--tags", &name, "site.yml"])
                    .status();
                match status {
                    Ok(s) => std::process::exit(s.code().unwrap_or(1)),
                    Err(e) => {
                        eprintln!("mded: ansible-pull spawn failed: {e}");
                        std::process::exit(2);
                    }
                }
            }
        }
    }
    Ok(())
}

/// `$QNM_SHARED_ROOT/.qnm-sync/playbooks/roles/` — same
/// resolution the Iced playbooks panel uses.
fn playbooks_root() -> PathBuf {
    let base = std::env::var("QNM_SHARED_ROOT").map(PathBuf::from).ok();
    let base = base.unwrap_or_else(|| {
        std::env::var("HOME")
            .map(|h| PathBuf::from(h).join("QNM-Shared"))
            .unwrap_or_else(|_| PathBuf::from("/var/empty"))
    });
    base.join(".qnm-sync").join("playbooks").join("roles")
}

/// Walk roles/ for subdirectories. Returns role names (bare
/// basenames); empty on any I/O error so the panel + CLI can
/// surface the empty-state message.
fn enumerate_playbook_roles(root: &std::path::Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for entry in rd.flatten() {
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            if let Some(name) = entry.file_name().to_str() {
                names.push(name.to_string());
            }
        }
    }
    names
}

/// Curated descriptions per the Phase 1.3.0 lock. Mirrors the
/// `playbook_from_name` helper in the Iced playbooks panel so
/// the CLI and the GUI agree.
fn playbook_description(name: &str) -> &'static str {
    match name {
        "system-update" => "Apply pending dnf upgrades (gated, never runs on default tag)",
        "mesh-state-snapshot" => "Snapshot QNM-Shared state for offline review",
        "selinux-permissive-toggle" => "Flip SELinux to permissive (op-tagged, never default)",
        "container-runtime-setup" => "Install + configure podman / docker runtime",
        "xfconf-baseline" => "Apply baseline xfconf keys (default-tagged)",
        "bloat-removal" => "Remove the curated bloat package list",
        "apps-install" => "Install the curated MDE app list",
        _ => "Custom role",
    }
}
