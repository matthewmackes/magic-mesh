//! `Converge` CLI verb handler.
//!
//! Extracted verbatim from `bin/mackesd.rs` (arch-1). Behaviour is unchanged;
//! only the location moved.
use crate::*;

/// SETUP-7 — re-apply `/etc/mackesd/site.yml` locally via `ansible-playbook`.
pub fn run(site: Option<PathBuf>) -> anyhow::Result<()> {
    let site = site.unwrap_or_else(|| PathBuf::from(mackesd_core::site_yml::DEFAULT_SITE_YML));
    if !site.exists() {
        anyhow::bail!(
            "no convergence playbook at {} — found/join generates it; run one first",
            site.display()
        );
    }
    if which_on_path("ansible-playbook").is_none() {
        println!(
            "ansible-playbook not installed — skipping converge ({} is ready for when it is)",
            site.display()
        );
        return Ok(());
    }
    println!("converging from {} …", site.display());
    let status = std::process::Command::new("ansible-playbook")
        .arg("-c")
        .arg("local")
        .arg("-i")
        .arg("localhost,")
        .arg(&site)
        .env("ANSIBLE_ROLES_PATH", "/usr/share/mackes/ansible/roles")
        .status()
        .context("running ansible-playbook")?;
    if status.success() {
        println!("converge complete");
        Ok(())
    } else {
        anyhow::bail!("ansible-playbook exited {:?}", status.code())
    }
}

/// Best-effort `which`: returns the resolved path of `bin` on `$PATH`, or None.
fn which_on_path(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(bin))
        .find(|p| p.is_file())
}
