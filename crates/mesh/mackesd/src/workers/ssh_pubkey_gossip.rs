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

    /// LH-JOIN-QNM-1 — is it safe to write into the replicated share this tick?
    /// `false` only when the workgroup root is the canonical mountpoint
    /// (`/mnt/mesh-storage`) and it isn't actually a FUSE mount: seeding
    /// `<root>/ssh-keys/<host>.pub` into the bare mountpoint fills it so
    /// LizardFS can never `mfsmount` over it ("mountpoint is not empty") — the
    /// exact stray-write race that wedged a fresh lighthouse join. Dev/test
    /// roots (tempdir, `~/QNM-Shared`) are always writable, so unaffected.
    /// Mirrors `meshfs_worker`'s `shared_root_writable` guard.
    #[must_use]
    pub fn share_writable(&self) -> bool {
        crate::shared_root_writable(&self.workgroup_root)
    }

    /// One gossip pass across every relevant user. SSH-MESH-NOCREDS-1: gossip
    /// for the service user (`self.home`, typically root — the flat back-compat
    /// lane) AND every regular desktop user (uid 1000–60000 under `/home`, e.g.
    /// the operator `mm`), each in its own share lane, so `ssh <operator>@<peer>`
    /// is key-only too — not just `ssh root@<peer>` (the live gap: keys only
    /// reached root's `authorized_keys`). Best-effort + logged.
    async fn tick(&self) {
        // Service user — flat lane (`ssh-keys/<host>.pub`), back-compat.
        self.gossip_one(&self.home, &self.share_dir(), None).await;
        // Operator accounts — per-user lane (`ssh-keys/<user>/<host>.pub`); the
        // keypair + files are chowned to the user so the SSH client (running as
        // that user) can read its own private key.
        for (user, uid, gid, home) in operator_users() {
            let lane = self.share_dir().join(&user);
            self.gossip_one(&home, &lane, Some((uid, gid))).await;
        }
    }

    /// One gossip pass for a single user's `home` + share `lane`. Every step is
    /// best-effort + logged; a missing share (mesh storage not mounted yet) is a
    /// quiet no-op so the worker degrades gracefully pre-enrollment (§2 posture).
    /// `owner` chowns the generated keypair + `authorized_keys` to that user
    /// (None = leave as the running user, i.e. root for the service lane).
    async fn gossip_one(&self, home: &Path, lane: &Path, owner: Option<(u32, u32)>) {
        let ssh_dir = home.join(".ssh");
        let key_path = ssh_dir.join("id_ed25519");
        let pub_path = ssh_dir.join("id_ed25519.pub");

        // 1. Ensure the user keypair exists.
        if !pub_path.exists() {
            let _ = tokio::fs::create_dir_all(&ssh_dir).await;
            chown_to(&ssh_dir, owner);
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
                    // The private key must be readable by the user whose SSH
                    // client offers it — chown it (+ the pub) to them.
                    chown_to(&key_path, owner);
                    chown_to(&pub_path, owner);
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

        // LH-JOIN-QNM-1 — never touch the replicated share until it's a real
        // mount. On the canonical mountpoint that isn't mounted, the
        // `create_dir_all(lane)` below would *succeed* against the bare local
        // dir and seed `<root>/ssh-keys/<host>.pub`, poisoning the mountpoint so
        // LizardFS can never `mfsmount` over it ("not empty") — the live
        // fresh-join wedge. Quiet no-op until mounted; returning here also
        // preserves the existing `authorized_keys` block rather than pruning it
        // against an empty share.
        if !self.share_writable() {
            return;
        }

        // 2. Publish into the replicated share lane (write-on-change).
        if tokio::fs::create_dir_all(lane).await.is_err() {
            // Mesh storage not mounted — quiet no-op this tick.
            return;
        }
        let mine = lane.join(format!("{}.pub", self.hostname));
        let current = tokio::fs::read_to_string(&mine).await.unwrap_or_default();
        if current.trim() != pubkey {
            if let Err(e) = tokio::fs::write(&mine, format!("{pubkey}\n")).await {
                tracing::warn!("ssh_pubkey_gossip: publish failed: {e}");
            }
        }

        // 3. Collect every peer's published key in this lane (sorted, stable).
        let mut keys: Vec<String> = Vec::new();
        if let Ok(mut rd) = tokio::fs::read_dir(lane).await {
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
                chown_to(&ak_path, owner);
                tracing::info!(
                    keys = keys.len(),
                    home = %ssh_dir.display(),
                    "ssh_pubkey_gossip: authorized_keys managed block updated"
                );
            }
        }
    }
}

/// SSH-MESH-NOCREDS-1 — chown a path to `(uid, gid)` when an owner is given
/// (the operator-user lanes; the service lane passes `None` to leave it root).
/// Best-effort: mackesd runs as root, so this succeeds for real users + is a
/// harmless no-op otherwise.
fn chown_to(path: &Path, owner: Option<(u32, u32)>) {
    #[cfg(unix)]
    if let Some((uid, gid)) = owner {
        let _ = std::os::unix::fs::chown(path, Some(uid), Some(gid));
    }
    #[cfg(not(unix))]
    let _ = (path, owner);
}

/// SSH-MESH-NOCREDS-1 — the regular desktop/operator accounts (uid 1000–60000
/// with a home under `/home`) that need passwordless peer→peer SSH, parsed from
/// `/etc/passwd` (no extra dep). Returns `(user, uid, gid, home)`.
fn operator_users() -> Vec<(String, u32, u32, std::path::PathBuf)> {
    let Ok(contents) = std::fs::read_to_string("/etc/passwd") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in contents.lines() {
        let f: Vec<&str> = line.split(':').collect();
        if f.len() < 7 {
            continue;
        }
        let (Ok(uid), Ok(gid)) = (f[2].parse::<u32>(), f[3].parse::<u32>()) else {
            continue;
        };
        if !(1000..60000).contains(&uid) {
            continue;
        }
        let home = std::path::PathBuf::from(f[5]);
        if home.starts_with("/home") && home.is_dir() {
            out.push((f[0].to_string(), uid, gid, home));
        }
    }
    out
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
        // Exercise the single-user pass directly (the service/root lane) — never
        // `tick()`, which enumerates real /home users + would touch their ~/.ssh.
        w.gossip_one(home.path(), &w.share_dir(), None).await;

        // Published our key…
        let published = std::fs::read_to_string(share.join("pine.pub")).unwrap();
        assert_eq!(published.trim(), KEY_A);
        // …and merged both into authorized_keys.
        let ak = std::fs::read_to_string(ssh_dir.join("authorized_keys")).unwrap();
        assert!(ak.contains(KEY_A) && ak.contains(KEY_B));
        assert!(ak.contains(BLOCK_BEGIN));
    }

    #[test]
    fn share_writable_gates_on_real_mount_for_canonical_root() {
        // LH-JOIN-QNM-1 regression: on the canonical mountpoint the share is
        // writable exactly when /proc/mounts lists it as mounted — so on an
        // unmounted node the gossip publish is a no-op and the bare mountpoint
        // stays empty (LizardFS can then mfsmount over it on the first try,
        // instead of looping forever on "mountpoint is not empty").
        let canonical =
            SshPubkeyGossipWorker::new(PathBuf::from(crate::CANONICAL_QNM_MOUNT), "pine".into());
        let mounted = std::fs::read_to_string("/proc/mounts")
            .map(|c| {
                c.lines()
                    .any(|l| l.split_whitespace().nth(1) == Some(crate::CANONICAL_QNM_MOUNT))
            })
            .unwrap_or(false);
        assert_eq!(canonical.share_writable(), mounted);
        // A dev/non-canonical root is always writable — tests + dev unaffected.
        let dev = SshPubkeyGossipWorker::new(PathBuf::from("/tmp/qnm-dev"), "pine".into());
        assert!(dev.share_writable());
    }

    #[tokio::test]
    async fn worker_name_is_locked() {
        let w = SshPubkeyGossipWorker::new(PathBuf::from("/tmp/x"), "pine".into());
        assert_eq!(w.name(), "ssh_pubkey_gossip");
    }
}
