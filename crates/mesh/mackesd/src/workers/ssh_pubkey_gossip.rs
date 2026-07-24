//! SVC-2 (Q60) — SSH pubkey gossip worker.
//!
//! Makes peer-to-peer SSH passwordless across the mesh with zero
//! operator key juggling: every peer publishes its user's ed25519
//! SSH pubkey into a signed envelope in the Syncthing-replicated workgroup
//! root (`<root>/ssh-keys/<hostname>.pub` — replication is the gossip
//! transport, the PEERVER pattern), and merges every peer's published key
//! into `~/.ssh/authorized_keys` inside a managed block. Outside-the-block
//! content is never touched; the merge is idempotent and only rewrites on
//! change.
//!
//! The key is the **user's** standard `~/.ssh/id_ed25519` (generated
//! on first tick when absent) — not the Nebula node identity, which
//! stays single-purpose (§3). `$HOME` decides whose authorized_keys
//! this box offers; on a headless Server that's the service user.
//!
//! No membership ACL by design (Q62 / W1 — access to the mesh IS the control
//! plane): every enrolled peer's key is honored. The replicated file is not
//! trusted as arbitrary plaintext, however: the publishing node signs the
//! exact `(node, user lane, SSH public key)` tuple with its persisted node key,
//! and consumers reject unsigned/tampered/re-located envelopes. Revocation =
//! the peer's signed `.pub` disappearing from the share (leave/decommission),
//! which the next tick prunes from the managed block.

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use ed25519_dalek::{Signature, Signer, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

use super::{ShutdownToken, Worker};

/// Managed-block open marker. Everything between the markers is
/// owned by this worker; everything outside is the operator's.
pub const BLOCK_BEGIN: &str = "# >>> mde ssh-gossip (managed; do not edit) >>>";
/// Managed-block close marker.
pub const BLOCK_END: &str = "# <<< mde ssh-gossip <<<";

/// Default tick cadence — keys change rarely; a minute keeps a new
/// peer's first SSH wait short without polling storms.
pub const TICK_SECS: u64 = 60;

/// Versioned signed envelope stored in the replicated SSH-key lane. The
/// embedded public key is provenance, not a new membership authority: the
/// enrolled-mesh auto-trust policy remains intentional, while the signature
/// prevents a writer from changing a peer's already-published tuple silently.
const PUBLISHED_KEY_SCHEMA_VERSION: u64 = 1;
const PUBLISHED_KEY_DOMAIN: &str = "mde-ssh-gossip-pubkey-v1";

#[derive(Debug, Clone, Deserialize, Serialize)]
struct PublishedKeyEnvelope {
    schema_version: u64,
    node_id: String,
    /// Empty for the service-user lane; otherwise the exact Unix username.
    scope: String,
    public_key: String,
    /// The persisted node-signing public key, lower-hex encoded.
    signer_public_key: String,
    /// Detached Ed25519 signature over [`published_key_signing_bytes`].
    signature: String,
}

fn published_key_signing_bytes(
    node_id: &str,
    scope: &str,
    public_key: &str,
    signer_public_key: &str,
) -> Vec<u8> {
    format!(
        "{PUBLISHED_KEY_DOMAIN}\0{PUBLISHED_KEY_SCHEMA_VERSION}\0{node_id}\0{scope}\0{public_key}\0{signer_public_key}"
    )
    .into_bytes()
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn decode_hex<const N: usize>(value: &str) -> Option<[u8; N]> {
    if value.len() != N * 2 {
        return None;
    }
    let mut out = [0_u8; N];
    for (index, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(value.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

fn node_id_for_host(hostname: &str) -> String {
    let host = hostname.strip_prefix("peer:").unwrap_or(hostname);
    format!("peer:{host}")
}

fn signed_published_key(
    signing_key: &ed25519_dalek::SigningKey,
    hostname: &str,
    scope: &str,
    public_key: &str,
) -> String {
    let node_id = node_id_for_host(hostname);
    let signer_public_key = encode_hex(signing_key.verifying_key().as_bytes());
    let signature = signing_key.sign(&published_key_signing_bytes(
        &node_id,
        scope,
        public_key,
        &signer_public_key,
    ));
    serde_json::to_string(&PublishedKeyEnvelope {
        schema_version: PUBLISHED_KEY_SCHEMA_VERSION,
        node_id,
        scope: scope.to_owned(),
        public_key: public_key.to_owned(),
        signer_public_key,
        signature: encode_hex(&signature.to_bytes()),
    })
    .expect("SSH gossip envelope serialization cannot fail")
}

fn verified_published_key(
    raw: &str,
    expected_node_id: &str,
    expected_scope: &str,
) -> Option<String> {
    let envelope: PublishedKeyEnvelope = serde_json::from_str(raw).ok()?;
    if envelope.schema_version != PUBLISHED_KEY_SCHEMA_VERSION
        || envelope.node_id != expected_node_id
        || envelope.scope != expected_scope
        || !valid_pubkey_line(&envelope.public_key)
    {
        return None;
    }
    let signer_bytes = decode_hex::<32>(&envelope.signer_public_key)?;
    let signature = Signature::from_bytes(&decode_hex::<64>(&envelope.signature)?);
    let signer = VerifyingKey::from_bytes(&signer_bytes).ok()?;
    signer
        .verify(
            &published_key_signing_bytes(
                &envelope.node_id,
                &envelope.scope,
                &envelope.public_key,
                &envelope.signer_public_key,
            ),
            &signature,
        )
        .ok()?;
    Some(envelope.public_key)
}

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
    signing_key: Option<ed25519_dalek::SigningKey>,
}

impl SshPubkeyGossipWorker {
    /// `workgroup_root` is the Syncthing-replicated QNM root; `hostname`
    /// names this peer's published key file.
    #[must_use]
    pub fn new(workgroup_root: PathBuf, hostname: String) -> Self {
        let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/root"), PathBuf::from);
        Self {
            workgroup_root,
            hostname: hostname
                .strip_prefix("peer:")
                .unwrap_or(&hostname)
                .to_owned(),
            home,
            interval: Duration::from_secs(TICK_SECS),
            signing_key: None,
        }
    }

    /// Supply the persisted node-signing key. Production wiring refuses to
    /// start this worker without it; tests can keep using a deterministic key
    /// without touching `/var/lib/mackesd`.
    #[must_use]
    pub fn with_signing_key(mut self, signing_key: ed25519_dalek::SigningKey) -> Self {
        self.signing_key = Some(signing_key);
        self
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
    /// `false` only when the workgroup root is the canonical shared dir
    /// (`/mnt/mesh-storage`) and it doesn't exist yet: seeding
    /// `<root>/ssh-keys/<host>.pub` before Syncthing provisions the dir would
    /// land on a bare local dir. Dev/test roots (tempdir, `~/QNM-Shared`) are
    /// always writable, so unaffected. Wraps [`crate::shared_root_writable`].
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
        self.gossip_one(&self.home, &self.share_dir(), "", None)
            .await;
        // Operator accounts — per-user lane (`ssh-keys/<user>/<host>.pub`); the
        // keypair + files are chowned to the user so the SSH client (running as
        // that user) can read its own private key.
        for (user, uid, gid, home) in operator_users() {
            let lane = self.share_dir().join(&user);
            self.gossip_one(&home, &lane, &user, Some((uid, gid))).await;
        }
    }

    /// One gossip pass for a single user's `home` + share `lane`. Every step is
    /// best-effort + logged; a missing share (mesh storage not mounted yet) is a
    /// quiet no-op so the worker degrades gracefully pre-enrollment (§2 posture).
    /// `owner` chowns the generated keypair + `authorized_keys` to that user
    /// (None = leave as the running user, i.e. root for the service lane).
    async fn gossip_one(&self, home: &Path, lane: &Path, scope: &str, owner: Option<(u32, u32)>) {
        let Some(signing_key) = self.signing_key.as_ref() else {
            tracing::warn!(
                "ssh_pubkey_gossip: node signing key unavailable; refusing unsigned lane"
            );
            return;
        };
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

        // LH-JOIN-QNM-1 — never touch the replicated share until it exists.
        // On a node where Syncthing hasn't provisioned the share yet, the
        // `create_dir_all(lane)` below would *succeed* against the bare local
        // dir and seed `<root>/ssh-keys/<host>.pub` — writing into the bare dir
        // before Syncthing provisions it would land on a stale local dir, the
        // live fresh-join wedge. Quiet no-op until the share exists; returning
        // here also preserves the existing `authorized_keys` block rather than
        // pruning it against an empty share.
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
        let published = signed_published_key(signing_key, &self.hostname, scope, &pubkey);
        if current.trim() != published {
            if let Err(e) = tokio::fs::write(&mine, format!("{published}\n")).await {
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
                        if let Some(host) = p.file_stem().and_then(|name| name.to_str()) {
                            let expected_node_id = node_id_for_host(host);
                            if let Some(line) =
                                verified_published_key(&content, &expected_node_id, scope)
                            {
                                keys.push(line);
                            }
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
    fn published_key_requires_signed_exact_node_and_scope() {
        let signer = ed25519_dalek::SigningKey::from_bytes(&[7_u8; 32]);
        let envelope = signed_published_key(&signer, "pine", "", KEY_A);
        assert_eq!(
            verified_published_key(&envelope, "peer:pine", "").as_deref(),
            Some(KEY_A)
        );
        assert!(verified_published_key(&KEY_A.to_string(), "peer:pine", "").is_none());
        assert!(verified_published_key(&envelope, "peer:oak", "").is_none());
        assert!(verified_published_key(&envelope, "peer:pine", "mm").is_none());
        let tampered = envelope.replace("mde-mesh@pine", "mde-mesh@oak");
        assert!(verified_published_key(&tampered, "peer:pine", "").is_none());
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
        let oak_signer = ed25519_dalek::SigningKey::from_bytes(&[8_u8; 32]);
        let oak_envelope = signed_published_key(&oak_signer, "oak", "", KEY_B);
        std::fs::write(share.join("oak.pub"), format!("{oak_envelope}\n")).unwrap();

        let mut w = SshPubkeyGossipWorker::new(root.path().to_path_buf(), "pine".into())
            .with_home(home.path().to_path_buf());
        // Exercise the single-user pass directly (the service/root lane) — never
        // `tick()`, which enumerates real /home users + would touch their ~/.ssh.
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[7_u8; 32]);
        w = w.with_signing_key(signing_key);
        let lane = w.share_dir();
        w.gossip_one(home.path(), &lane, "", None).await;

        // Published our key…
        let published = std::fs::read_to_string(share.join("pine.pub")).unwrap();
        let published: PublishedKeyEnvelope = serde_json::from_str(published.trim()).unwrap();
        assert_eq!(published.public_key, KEY_A);
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
        // stays empty, avoiding writing into a stale local dir before the
        // share is provisioned.
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
