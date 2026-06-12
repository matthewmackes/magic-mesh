//! SVC-2 (Q60) — SSH pubkey gossip worker.
//!
//! Makes peer-to-peer SSH passwordless across the mesh with zero
//! operator key juggling: every peer publishes its user's ed25519
//! SSH pubkey into the LizardFS-replicated workgroup root
//! (`<root>/ssh-keys/<hostname>.pub` — replication is the gossip
//! transport, the PEERVER pattern), and merges every peer's
//! published key into `~/.ssh/authorized_keys` inside a managed
//! block. Outside-the-block content is never touched; the merge is
//! idempotent and only rewrites on change.
//!
//! The key is the **user's** standard `~/.ssh/id_ed25519` (generated
//! on first tick when absent) — not the Nebula node identity, which
//! stays single-purpose (§3). `$HOME` decides whose authorized_keys
//! this box offers; on a headless Server that's the service user.
//!
//! No ACL by design (Q62 / W1 — access to the mesh IS the control
//! plane): every enrolled peer's key is honored. Revocation = the
//! peer's `.pub` disappearing from the share (leave/decommission),
//! which the next tick prunes from the managed block.

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{ShutdownToken, Worker};

/// Managed-block open marker. Everything between the markers is
/// owned by this worker; everything outside is the operator's.
pub const BLOCK_BEGIN: &str = "# >>> mde ssh-gossip (managed; do not edit) >>>";
/// Managed-block close marker.
pub const BLOCK_END: &str = "# <<< mde ssh-gossip <<<";

/// Default tick cadence — keys change rarely; a minute keeps a new
/// peer's first SSH wait short without polling storms.
pub const TICK_SECS: u64 = 60;

/// `true` for a line that looks like an OpenSSH ed25519 public key —
/// the only kind this worker publishes or honors (§3 pins ed25519).
#[must_use]
pub fn valid_pubkey_line(line: &str) -> bool {
    let line = line.trim();
    let mut parts = line.split_whitespace();
    matches!(parts.next(), Some("ssh-ed25519"))
        && parts.next().is_some_and(|b64| {
            b64.len() > 40
                && b64
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'+' || c == b'/' || c == b'=')
        })
}

/// Merge `keys` into `existing` authorized_keys content inside the
/// managed block, preserving everything outside it. Returns the new
/// file content. Pure — the worker writes it only when it differs.
#[must_use]
pub fn merge_authorized_keys(existing: &str, keys: &[String]) -> String {
    let mut outside: Vec<&str> = Vec::new();
    let mut in_block = false;
    for line in existing.lines() {
        if line.trim() == BLOCK_BEGIN {
            in_block = true;
            continue;
        }
        if line.trim() == BLOCK_END {
            in_block = false;
            continue;
        }
        if !in_block {
            outside.push(line);
        }
    }
    // Drop trailing blank lines from the preserved content so the
    // block lands after exactly one separator.
    while outside.last().is_some_and(|l| l.trim().is_empty()) {
        outside.pop();
    }
    let mut out = outside.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    if !keys.is_empty() {
        out.push_str(BLOCK_BEGIN);
        out.push('\n');
        for k in keys {
            out.push_str(k.trim());
            out.push('\n');
        }
        out.push_str(BLOCK_END);
        out.push('\n');
    }
    out
}

/// The gossip worker. One tick: ensure the local keypair, publish
/// the pubkey to the share, merge every published key into
/// `authorized_keys`.
pub struct SshPubkeyGossipWorker {
    workgroup_root: PathBuf,
    hostname: String,
    home: PathBuf,
    interval: Duration,
}

impl SshPubkeyGossipWorker {
    /// `workgroup_root` is the LizardFS-replicated QNM root; `hostname`
    /// names this peer's published key file.
    #[must_use]
    pub fn new(workgroup_root: PathBuf, hostname: String) -> Self {
        let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/root"), PathBuf::from);
        Self {
            workgroup_root,
            hostname,
            home,
            interval: Duration::from_secs(TICK_SECS),
        }
    }

    /// Test seam — pin `$HOME` explicitly.
    #[must_use]
    pub fn with_home(mut self, home: PathBuf) -> Self {
        self.home = home;
        self
    }

    fn share_dir(&self) -> PathBuf {
        self.workgroup_root.join("ssh-keys")
    }

    /// One gossip pass. Every step is best-effort + logged; a missing
    /// share (mesh storage not mounted yet) is a quiet no-op so the
    /// worker degrades gracefully pre-enrollment (§2 posture).
    async fn tick(&self) {
        let ssh_dir = self.home.join(".ssh");
        let key_path = ssh_dir.join("id_ed25519");
        let pub_path = ssh_dir.join("id_ed25519.pub");

        // 1. Ensure the user keypair exists.
        if !pub_path.exists() {
            let _ = tokio::fs::create_dir_all(&ssh_dir).await;
            let comment = format!("mde-mesh@{}", self.hostname);
            let mut keygen = tokio::process::Command::new("ssh-keygen");
            keygen
                .args(["-q", "-t", "ed25519", "-N", "", "-C", &comment, "-f"])
                .arg(&key_path);
            // EFF-20 — bound keygen so a stuck entropy/IO wait can't hang the tick.
            match crate::workers::proc::status_with_timeout_async(
                keygen,
                crate::workers::proc::DEFAULT_CMD_TIMEOUT,
            )
            .await
            {
                Ok(st) if st.success() => {
                    tracing::info!("ssh_pubkey_gossip: generated {}", key_path.display());
                }
                Ok(st) => {
                    tracing::warn!("ssh_pubkey_gossip: ssh-keygen exited {st}");
                    return;
                }
                Err(e) => {
                    tracing::warn!("ssh_pubkey_gossip: ssh-keygen unavailable: {e}");
                    return;
                }
            }
        }
        let Ok(pubkey) = tokio::fs::read_to_string(&pub_path).await else {
            return;
        };
        let pubkey = pubkey.trim().to_string();
        if !valid_pubkey_line(&pubkey) {
            tracing::warn!("ssh_pubkey_gossip: local pubkey is not ssh-ed25519; skipping");
            return;
        }

        // 2. Publish into the replicated share (write-on-change).
        let share = self.share_dir();
        if tokio::fs::create_dir_all(&share).await.is_err() {
            // Mesh storage not mounted — quiet no-op this tick.
            return;
        }
        let mine = share.join(format!("{}.pub", self.hostname));
        let current = tokio::fs::read_to_string(&mine).await.unwrap_or_default();
        if current.trim() != pubkey {
            if let Err(e) = tokio::fs::write(&mine, format!("{pubkey}\n")).await {
                tracing::warn!("ssh_pubkey_gossip: publish failed: {e}");
            }
        }

        // 3. Collect every peer's published key (sorted for stability).
        let mut keys: Vec<String> = Vec::new();
        if let Ok(mut rd) = tokio::fs::read_dir(&share).await {
            while let Ok(Some(entry)) = rd.next_entry().await {
                let p = entry.path();
                if p.extension().is_some_and(|e| e == "pub") {
                    if let Ok(content) = tokio::fs::read_to_string(&p).await {
                        let line = content.trim().to_string();
                        if valid_pubkey_line(&line) {
                            keys.push(line);
                        }
                    }
                }
            }
        }
        keys.sort();
        keys.dedup();

        // 4. Merge into authorized_keys (write-on-change, 0600).
        let ak_path = ssh_dir.join("authorized_keys");
        let existing = tokio::fs::read_to_string(&ak_path)
            .await
            .unwrap_or_default();
        let merged = merge_authorized_keys(&existing, &keys);
        if merged != existing {
            if tokio::fs::write(&ak_path, &merged).await.is_ok() {
                set_private_perms(&ak_path).await;
                tracing::info!(
                    keys = keys.len(),
                    "ssh_pubkey_gossip: authorized_keys managed block updated"
                );
            }
        }
    }
}

/// chmod 600 — sshd refuses group/world-readable authorized_keys
/// under StrictModes.
async fn set_private_perms(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = tokio::fs::metadata(path).await {
            let mut perms = meta.permissions();
            perms.set_mode(0o600);
            let _ = tokio::fs::set_permissions(path, perms).await;
        }
    }
}

#[async_trait::async_trait]
impl Worker for SshPubkeyGossipWorker {
    fn name(&self) -> &'static str {
        "ssh_pubkey_gossip"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            self.tick().await;
            tokio::select! {
                _ = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(self.interval) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_A: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPlaceholderPlaceholderPlaceholderPlac mde-mesh@pine";
    const KEY_B: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIQlaceholderPlaceholderPlaceholderPlac mde-mesh@oak";

    #[test]
    fn valid_pubkey_accepts_ed25519_and_rejects_others() {
        assert!(valid_pubkey_line(KEY_A));
        assert!(!valid_pubkey_line("ssh-rsa AAAAB3NzaC1yc2E mde@x"));
        assert!(!valid_pubkey_line("ssh-ed25519"));
        assert!(!valid_pubkey_line("# comment"));
        assert!(!valid_pubkey_line(""));
    }

    #[test]
    fn merge_preserves_operator_content_outside_the_block() {
        let existing = "ssh-rsa OPERATORKEY operator@laptop\n";
        let merged = merge_authorized_keys(existing, &[KEY_A.to_string()]);
        assert!(merged.starts_with("ssh-rsa OPERATORKEY operator@laptop\n"));
        assert!(merged.contains(BLOCK_BEGIN));
        assert!(merged.contains(KEY_A));
        assert!(merged.trim_end().ends_with(BLOCK_END));
    }

    #[test]
    fn merge_is_idempotent() {
        let once = merge_authorized_keys("", &[KEY_A.to_string(), KEY_B.to_string()]);
        let twice = merge_authorized_keys(&once, &[KEY_A.to_string(), KEY_B.to_string()]);
        assert_eq!(once, twice);
    }

    #[test]
    fn merge_prunes_keys_no_longer_published() {
        let with_both = merge_authorized_keys("", &[KEY_A.to_string(), KEY_B.to_string()]);
        let pruned = merge_authorized_keys(&with_both, &[KEY_A.to_string()]);
        assert!(pruned.contains(KEY_A));
        assert!(!pruned.contains(KEY_B), "departed peer's key must drop");
    }

    #[test]
    fn merge_with_no_keys_removes_the_block_entirely() {
        let with_key = merge_authorized_keys("ssh-rsa OP op@x\n", &[KEY_A.to_string()]);
        let emptied = merge_authorized_keys(&with_key, &[]);
        assert_eq!(emptied, "ssh-rsa OP op@x\n");
    }

    #[tokio::test]
    async fn tick_publishes_and_merges_round_trip() {
        let root = tempfile::tempdir().expect("root");
        let home = tempfile::tempdir().expect("home");
        // Seed a fake local keypair so the tick skips ssh-keygen.
        let ssh_dir = home.path().join(".ssh");
        std::fs::create_dir_all(&ssh_dir).unwrap();
        std::fs::write(ssh_dir.join("id_ed25519.pub"), format!("{KEY_A}\n")).unwrap();
        // Seed a second peer's published key.
        let share = root.path().join("ssh-keys");
        std::fs::create_dir_all(&share).unwrap();
        std::fs::write(share.join("oak.pub"), format!("{KEY_B}\n")).unwrap();

        let w = SshPubkeyGossipWorker::new(root.path().to_path_buf(), "pine".into())
            .with_home(home.path().to_path_buf());
        w.tick().await;

        // Published our key…
        let published = std::fs::read_to_string(share.join("pine.pub")).unwrap();
        assert_eq!(published.trim(), KEY_A);
        // …and merged both into authorized_keys.
        let ak = std::fs::read_to_string(ssh_dir.join("authorized_keys")).unwrap();
        assert!(ak.contains(KEY_A) && ak.contains(KEY_B));
        assert!(ak.contains(BLOCK_BEGIN));
    }

    #[tokio::test]
    async fn worker_name_is_locked() {
        let w = SshPubkeyGossipWorker::new(PathBuf::from("/tmp/x"), "pine".into());
        assert_eq!(w.name(), "ssh_pubkey_gossip");
    }
}
