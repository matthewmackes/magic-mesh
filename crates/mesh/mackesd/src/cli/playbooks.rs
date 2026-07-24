//! `Playbooks` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;
use std::path::{Component, Path};

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
                let root = playbooks_root();
                validate_playbook_name(&root, &name)?;
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

/// Enforce the same role boundary advertised by `playbooks list` before a
/// privileged `ansible-pull` is started. Without this check, `run` accepted
/// any arbitrary tag from `site.yml`, including tags that were never present in
/// the curated role tree.
fn validate_playbook_name(root: &Path, name: &str) -> anyhow::Result<()> {
    let mut components = Path::new(name).components();
    let is_single_name = matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(_)), None)
    );
    if !is_single_name || name.chars().any(|c| c == '\\' || c.is_control()) {
        anyhow::bail!("playbooks run: `{name}` is not a valid role name")
    }

    let role_path = root.join(name);
    let metadata = std::fs::symlink_metadata(&role_path).with_context(|| {
        format!(
            "playbooks run: `{name}` is not a curated role under {}",
            root.display()
        )
    })?;
    if !metadata.file_type().is_dir() {
        anyhow::bail!(
            "playbooks run: `{name}` is not a curated role under {}",
            root.display()
        )
    }

    // A role directory must not escape the curated root through a symlink in
    // an ancestor. The final component is already rejected as a symlink by the
    // metadata check above.
    let canonical_root = std::fs::canonicalize(root).with_context(|| {
        format!(
            "playbooks run: curated role root {} is unavailable",
            root.display()
        )
    })?;
    let canonical_role = std::fs::canonicalize(&role_path)
        .with_context(|| format!("playbooks run: cannot resolve curated role `{name}`"))?;
    if !canonical_role.starts_with(&canonical_root) {
        anyhow::bail!("playbooks run: curated role `{name}` resolves outside its root")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_accepts_only_a_real_role_directory_under_the_curated_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("roles");
        std::fs::create_dir_all(root.join("system-update")).unwrap();

        assert!(validate_playbook_name(&root, "system-update").is_ok());
        assert!(validate_playbook_name(&root, "not-listed").is_err());
        assert!(validate_playbook_name(&root, "../site").is_err());
        assert!(validate_playbook_name(&root, "system-update/child").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn run_rejects_a_role_symlink_that_escapes_the_curated_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("roles");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("escaped")).unwrap();

        assert!(validate_playbook_name(&root, "escaped").is_err());
    }
}
