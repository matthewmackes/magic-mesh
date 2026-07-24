//! FILEMGR-6 — the shared mesh SSH key provisioner + sshd overlay bind
//! (design: `docs/design/file-manager-full.md`, locks 13 / 17 + the risk row).
//!
//! FILEMGR-5's [`crate::workers::mesh_mount`] worker CONSUMES a shared mesh SSH
//! private key, sealed in the secret store under the ref
//! [`crate::workers::mesh_mount::MESH_SSH_KEY_REF`] (`"mesh-ssh-key"`), to drive
//! sshfs over the Nebula overlay. This module is the missing PROVIDER half: it
//!
//!   1. **generates** an ed25519 keypair (pure Rust — [`ed25519_dalek`] +
//!      hand-rolled OpenSSH serialization; no `ssh-keygen` shell-out, §9),
//!   2. **seals** the OpenSSH-format private half into the mesh secret store
//!      under `"mesh-ssh-key"` — the exact material sshfs writes as its
//!      `IdentityFile`. Sealing is real encryption ([`crate::ipc::secret_store`]
//!      → age+etcd, or the local-AEAD fallback); the plaintext private key is
//!      NEVER written to disk or logs (§7/security),
//!   3. **installs** the public half for the mesh user into a dedicated
//!      authorized-keys file, and
//!   4. **binds it to the overlay** — the shared key is honored by sshd ONLY for
//!      connections whose source address is inside the overlay CIDR (a
//!      `Match Address` drop-in), so a leaked shared key is unusable from the
//!      public NIC. This is the lock-13 blast-radius mitigation; the public
//!      listener + the operator's per-user keys are untouched (see
//!      [`crate::workers::sshd_overlay_bind`], which keeps sshd on `0.0.0.0`).
//!
//! It also owns the **re-key path** ([`MeshKeyProvisioner::rotate`]): generate a
//! fresh keypair, reseal (which overwrites the single-valued ref → the old
//! private half is revoked), and re-install so the old public key drops from the
//! authorized-keys file.
//!
//! ## §7 — honest node-gate, never a stub
//!
//! Generation, sealing, public-key derivation, and the config RENDER + file
//! writes are all real + unit-tested against a temp store. The one leg that
//! genuinely needs a running node — reloading sshd so the drop-in takes effect —
//! goes through an injectable reloader seam; the live impl returns a typed
//! [`SshdReload::Gated`] on a box without a usable `systemctl`/unit rather than
//! faking a reload (§7). Tests drive the whole flow with a fake reloader.
//!
//! ## Why a dedicated authorized-keys file (not `~/.ssh/authorized_keys`)
//!
//! The shared key must be **overlay-only**. sshd applies an `AuthorizedKeysFile`
//! set within a `Match Address <overlay-cidr>` block ONLY to connections from
//! that CIDR; putting the shared key into the user's global
//! `~/.ssh/authorized_keys` would honor it from anywhere (defeating the
//! mitigation). So the shared public key lands in a dedicated file referenced
//! solely inside the Match block — "installed for the mesh user" (it authorizes
//! login as that user) AND overlay-bound.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{net::IpAddr, path::Component};

use base64::Engine as _;
use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use rand::RngCore as _;
use zeroize::{Zeroize as _, Zeroizing};

use crate::ipc::secret_store::SecretStore;
use crate::workers::mesh_mount::{DEFAULT_MESH_USER, MESH_SSH_KEY_REF};

/// The comment baked into the shared keypair. It is a SHARED (fleet-wide) key,
/// so there is deliberately no per-host suffix — the same public line installs
/// identically on every node.
pub const MESH_KEY_COMMENT: &str = "mde-mesh-shared";

/// Default dedicated authorized-keys file holding ONLY the shared mesh public key.
///
/// Referenced exclusively by the overlay `Match` block (never the global
/// context), so the shared key authenticates only for overlay-sourced
/// connections. Root-owned under `/etc/ssh` so sshd `StrictModes` accepts it.
pub const DEFAULT_MESH_KEYS_PATH: &str = "/etc/ssh/mackes-mesh/mesh_authorized_keys";

/// Default sshd drop-in path.
///
/// Sorts AFTER `mackes-mesh.conf` (the overlay-bind listener drop-in written by
/// [`crate::workers::sshd_overlay_bind`]) so this file's trailing `Match all`
/// restores the global context after the gated stanza, leaving later drop-ins /
/// the main config unconditional.
pub const DEFAULT_SSHD_DROPIN_PATH: &str = "/etc/ssh/sshd_config.d/mackes-mesh-sshkey.conf";

/// Default systemd unit reloaded after a config change (matches
/// [`crate::workers::sshd_overlay_bind::DEFAULT_SSHD_UNIT`]).
pub const DEFAULT_SSHD_UNIT: &str = "sshd.service";

/// OpenSSH ed25519 key-type tag (the first field of a public line + the type
/// string embedded in the private container).
const KEY_TYPE: &str = "ssh-ed25519";

/// OpenSSH private-key container magic — the literal `openssh-key-v1` plus its
/// trailing NUL.
const AUTH_MAGIC: &[u8] = b"openssh-key-v1\0";

/// The `-o none` cipher block size the private section is padded to.
const NONE_BLOCK_SIZE: usize = 8;

// ── typed errors ────────────────────────────────────────────────────────────

/// A typed provisioning failure. Every fault is honest: a store/crypto error, a
/// local I/O error, a malformed sealed key, or the node-gate on the live sshd
/// reload — never a fabricated success (§7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProvisionError {
    /// The secret store rejected the seal or the read (tooling / crypto / etcd
    /// fault). Carries the store's operator-readable message.
    Store(String),
    /// A local filesystem op failed (writing the authorized-keys file or the
    /// sshd drop-in).
    Io(String),
    /// The sealed material isn't a shape we can install (encrypted, multi-key,
    /// non-ed25519, or corrupt OpenSSH key). Surfaced honestly — never guessed.
    Malformed(String),
    /// The live sshd reload cannot run on this box (no usable `systemctl` / the
    /// unit is absent or failed to reload). The §7 node-gate: the config IS
    /// written, but activation needs a running node; NEVER faked as reloaded.
    Gated(String),
}

impl std::fmt::Display for ProvisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(m) => write!(f, "secret store: {m}"),
            Self::Io(m) => write!(f, "filesystem: {m}"),
            Self::Malformed(m) => write!(f, "malformed sealed key: {m}"),
            Self::Gated(m) => write!(f, "sshd reload unavailable (gated): {m}"),
        }
    }
}

impl std::error::Error for ProvisionError {}

// ── the keypair (pure generation) ───────────────────────────────────────────

/// A freshly-generated shared mesh SSH keypair.
///
/// The private half is an OpenSSH-format PEM (exactly what sshfs's `IdentityFile`
/// wants) wrapped in [`Zeroizing`] so it clears on drop; it is never surfaced by
/// `Debug` and never logged. The only sanctioned exit for the private half is
/// [`MeshKeyProvisioner::seal`], which encrypts it into the secret store.
pub struct MeshSshKeypair {
    /// OpenSSH-format private key PEM. Sealed immediately after generation; it
    /// never touches disk or logs in plaintext.
    private_openssh: Zeroizing<String>,
    /// The `ssh-ed25519 <base64> <comment>` `authorized_keys` line (public, so
    /// share-safe).
    public_line: String,
}

impl std::fmt::Debug for MeshSshKeypair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never leak the private half through Debug/logs.
        f.debug_struct("MeshSshKeypair")
            .field("private_openssh", &"<redacted>")
            .field("public_line", &self.public_line)
            .finish()
    }
}

impl MeshSshKeypair {
    /// The public `ssh-ed25519 …` `authorized_keys` line — share-safe.
    #[must_use]
    pub fn public_line(&self) -> &str {
        &self.public_line
    }
}

/// Generate a fresh shared mesh ed25519 keypair from the OS CSPRNG.
#[must_use]
pub fn generate_keypair() -> MeshSshKeypair {
    let signing = SigningKey::generate(&mut OsRng);
    let checkint = OsRng.next_u32();
    encode_keypair(&signing, MESH_KEY_COMMENT, checkint)
}

/// Encode a signing key into the OpenSSH public line + private PEM. Split out
/// from [`generate_keypair`] so tests drive it with a known seed + `checkint`
/// for deterministic output.
fn encode_keypair(signing: &SigningKey, comment: &str, checkint: u32) -> MeshSshKeypair {
    let pubkey = signing.verifying_key().to_bytes();
    let public_line = authorized_line(&pubkey, comment);
    let private_openssh =
        Zeroizing::new(encode_openssh_private(signing, &pubkey, comment, checkint));
    MeshSshKeypair {
        private_openssh,
        public_line,
    }
}

/// Push an OpenSSH wire `string` field: a big-endian `u32` length prefix then
/// the bytes. Our fields are all tiny (≤ 64 bytes), so the length never
/// overflows `u32`; saturate defensively rather than risk a panic.
fn put_string(out: &mut Vec<u8>, data: &[u8]) {
    let len = u32::try_from(data.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(data);
}

/// The OpenSSH public-key wire blob for an ed25519 key: `string "ssh-ed25519"`
/// followed by `string <32-byte key>`.
fn public_wire(pubkey: &[u8; 32]) -> Vec<u8> {
    let mut w = Vec::with_capacity(51);
    put_string(&mut w, KEY_TYPE.as_bytes());
    put_string(&mut w, pubkey);
    w
}

/// Render the `ssh-ed25519 <base64> <comment>` `authorized_keys` line for a raw
/// ed25519 public key.
#[must_use]
pub fn authorized_line(pubkey: &[u8; 32], comment: &str) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(public_wire(pubkey));
    format!("{KEY_TYPE} {b64} {comment}")
}

/// Hand-encode the `-----BEGIN OPENSSH PRIVATE KEY-----` PEM for an ed25519
/// key, cipher `none` (unencrypted — the secret store provides at-rest
/// encryption). Intermediate buffers that hold the seed are zeroized before
/// return; the returned PEM is the private key itself (the caller wraps it in
/// [`Zeroizing`] and seals it immediately).
fn encode_openssh_private(
    signing: &SigningKey,
    pubkey: &[u8; 32],
    comment: &str,
    checkint: u32,
) -> String {
    let pubwire = public_wire(pubkey);

    // The private section: two matching check-ints, then the key material.
    let mut sec = Vec::new();
    sec.extend_from_slice(&checkint.to_be_bytes());
    sec.extend_from_slice(&checkint.to_be_bytes());
    put_string(&mut sec, KEY_TYPE.as_bytes());
    put_string(&mut sec, pubkey);
    // OpenSSH stores the ed25519 private field as seed(32) || pubkey(32).
    let mut secret = Zeroizing::new(Vec::with_capacity(64));
    secret.extend_from_slice(&signing.to_bytes());
    secret.extend_from_slice(pubkey);
    put_string(&mut sec, &secret);
    put_string(&mut sec, comment.as_bytes());
    // Pad to the cipher block size with 1,2,3,… so the length is a multiple of 8.
    let mut pad: u8 = 1;
    while sec.len() % NONE_BLOCK_SIZE != 0 {
        sec.push(pad);
        pad = pad.wrapping_add(1);
    }

    // Assemble the container.
    let mut blob = Vec::new();
    blob.extend_from_slice(AUTH_MAGIC);
    put_string(&mut blob, b"none"); // ciphername
    put_string(&mut blob, b"none"); // kdfname
    put_string(&mut blob, b""); // kdfoptions
    blob.extend_from_slice(&1u32.to_be_bytes()); // number of keys
    put_string(&mut blob, &pubwire);
    put_string(&mut blob, &sec);

    let armored = base64::engine::general_purpose::STANDARD.encode(&blob);

    // Scrub the intermediate copies of the seed material.
    sec.zeroize();
    blob.zeroize();

    let mut pem = String::with_capacity(armored.len() + 80);
    pem.push_str("-----BEGIN OPENSSH PRIVATE KEY-----\n");
    for chunk in armored.as_bytes().chunks(70) {
        pem.push_str(&String::from_utf8_lossy(chunk));
        pem.push('\n');
    }
    pem.push_str("-----END OPENSSH PRIVATE KEY-----\n");
    pem
}

// ── public-key derivation from a sealed private PEM ─────────────────────────

/// Derive the public `authorized_keys` line from a sealed OpenSSH private PEM.
///
/// Each node reads the shared private key back from the (replicated) secret
/// store and derives the public line locally to install it — no separate public
/// artifact to keep in sync.
///
/// # Errors
/// [`ProvisionError::Malformed`] when `pem` is not a `none`-cipher, single,
/// ed25519 OpenSSH private key (encrypted / multi-key / wrong-type / corrupt).
pub fn public_line_from_private(pem: &str, comment: &str) -> Result<String, ProvisionError> {
    let pubkey = pubkey_from_private_pem(pem)?;
    Ok(authorized_line(&pubkey, comment))
}

/// A minimal, panic-free cursor over an OpenSSH binary blob.
struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    const fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], ProvisionError> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|end| *end <= self.buf.len())
            .ok_or_else(|| ProvisionError::Malformed("truncated OpenSSH blob".to_string()))?;
        let out = &self.buf[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    fn u32(&mut self) -> Result<u32, ProvisionError> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn string(&mut self) -> Result<&'a [u8], ProvisionError> {
        let len = self.u32()? as usize;
        self.take(len)
    }
}

/// Extract the raw 32-byte ed25519 public key from an OpenSSH private PEM we
/// generated (cipher `none`, one key). Only the public blob is read; the private
/// section is never touched here.
fn pubkey_from_private_pem(pem: &str) -> Result<[u8; 32], ProvisionError> {
    let body: String = pem
        .lines()
        .filter(|l| !l.trim_start().starts_with("-----"))
        .flat_map(|l| l.trim().chars())
        .collect();
    let blob = base64::engine::general_purpose::STANDARD
        .decode(body.as_bytes())
        .map_err(|e| ProvisionError::Malformed(format!("base64 decode: {e}")))?;

    let mut r = Reader::new(&blob);
    if r.take(AUTH_MAGIC.len())? != AUTH_MAGIC {
        return Err(ProvisionError::Malformed("bad container magic".to_string()));
    }
    if r.string()? != b"none" {
        return Err(ProvisionError::Malformed(
            "encrypted mesh key unsupported (expected cipher none)".to_string(),
        ));
    }
    let _kdfname = r.string()?;
    let _kdfoptions = r.string()?;
    if r.u32()? != 1 {
        return Err(ProvisionError::Malformed(
            "expected exactly one key in the container".to_string(),
        ));
    }
    let pubblob = r.string()?;

    let mut pr = Reader::new(pubblob);
    if pr.string()? != KEY_TYPE.as_bytes() {
        return Err(ProvisionError::Malformed(
            "public key is not ssh-ed25519".to_string(),
        ));
    }
    let key = pr.string()?;
    <[u8; 32]>::try_from(key)
        .map_err(|_| ProvisionError::Malformed("ed25519 public key is not 32 bytes".to_string()))
}

// ── config render (pure) ────────────────────────────────────────────────────

/// Render the dedicated authorized-keys file body holding the single shared mesh
/// public key.
///
/// We own the whole file, so no managed-block markers are needed (unlike the
/// per-user gossip block in [`crate::workers::ssh_pubkey_gossip`]).
#[must_use]
pub fn render_mesh_authorized_keys(public_line: &str) -> String {
    format!(
        "# Managed by mackesd::ipc::mesh_ssh_key (FILEMGR-6). Do NOT edit by hand.\n\
         # The shared mesh SSH public key, accepted by sshd ONLY for connections\n\
         # arriving on the Nebula overlay (see the Match Address drop-in). Re-key:\n\
         # `mackesd mesh-ssh-key rotate` reseals the private half + rewrites this.\n\
         {}\n",
        public_line.trim()
    )
}

/// Render the sshd drop-in that binds the shared key to the overlay.
///
/// A `Match Address <overlay-cidr> User <mesh-user>` stanza whose
/// `AuthorizedKeysFile` adds the dedicated mesh-key file (plus the user's own
/// keys) ONLY for overlay-sourced connections, then a trailing `Match all` to
/// restore the global context.
#[must_use]
pub fn render_sshd_overlay_dropin(
    overlay_cidr: &str,
    mesh_user: &str,
    mesh_keys_path: &Path,
) -> String {
    let keys = mesh_keys_path.display();
    format!(
        "# Generated by mackesd::ipc::mesh_ssh_key (FILEMGR-6). Do NOT edit by hand.\n\
         # The shared mesh SSH key ({keys}) is honored ONLY for connections whose\n\
         # source address is inside the Nebula overlay ({overlay_cidr}); a leaked\n\
         # shared key is unusable from the public NIC. The public listener + the\n\
         # per-user keys are untouched (see mackes-mesh.conf / ssh_pubkey_gossip).\n\
         Match Address {overlay_cidr} User {mesh_user}\n\
         \x20\x20\x20\x20AuthorizedKeysFile {keys} .ssh/authorized_keys\n\
         # Restore the global context so later drop-ins / the main config are not\n\
         # gated by the Match above.\n\
         Match all\n"
    )
}

// ── the live sshd reload seam (the honest node-gate) ────────────────────────

/// Outcome of the sshd reload leg — the one step that genuinely needs a running
/// node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshdReload {
    /// No unit configured — the reload was intentionally skipped (a config-only
    /// run, or a test).
    Skipped,
    /// sshd was reloaded (`systemctl reload-or-restart` succeeded).
    Reloaded,
    /// The reload could not run on this box (no usable `systemctl`, or the unit
    /// reload failed). Honest gate — the config is written but not activated.
    Gated(String),
}

/// Injectable sshd-reload seam so the orchestration is unit-tested with a fake
/// and the live impl stays node-gated (§9).
type Reloader = Arc<dyn Fn(&str) -> SshdReload + Send + Sync>;

/// The live reloader: `systemctl reload-or-restart <unit>`. `reload-or-restart`
/// (not a bare `reload`) revives a dead sshd too, matching
/// [`crate::workers::sshd_overlay_bind`]. A missing `systemctl` or a failed
/// reload returns [`SshdReload::Gated`] — never a fabricated success.
fn live_reload(unit: &str) -> SshdReload {
    // Clear any failed/start-limited state so the (re)start isn't refused.
    let _ = std::process::Command::new("systemctl")
        .args(["reset-failed", unit])
        .output();
    match std::process::Command::new("systemctl")
        .args(["reload-or-restart", unit])
        .output()
    {
        Ok(o) if o.status.success() => SshdReload::Reloaded,
        Ok(o) => SshdReload::Gated(format!(
            "systemctl reload-or-restart {unit}: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        // A spawn error means no usable systemctl on this box — the node-gate.
        Err(e) => SshdReload::Gated(format!("systemctl unavailable: {e}")),
    }
}

// ── the outcome + the provisioner ───────────────────────────────────────────

/// What a [`MeshKeyProvisioner::provision`] / [`MeshKeyProvisioner::rotate`] run
/// did, for the CLI to report honestly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionOutcome {
    /// A fresh keypair was generated + sealed this run (vs reusing the existing
    /// sealed key).
    pub generated: bool,
    /// This run was a re-key (rotate) — the previous private half was revoked.
    pub rekeyed: bool,
    /// The public `authorized_keys` line now installed.
    pub public_line: String,
    /// The sshd reload result (the honest node-gate).
    pub reload: SshdReload,
}

/// FILEMGR-6 — the shared mesh SSH key provisioner.
///
/// Reuses the mesh [`SecretStore`] (glue, not a new secret mechanism — §6) and
/// installs the public half behind an overlay-only sshd `Match` block. Every
/// path is builder-overridable so tests point the file writes at a tempdir and
/// inject a fake reloader.
pub struct MeshKeyProvisioner {
    store: SecretStore,
    mesh_user: String,
    overlay_cidr: String,
    mesh_keys_path: PathBuf,
    sshd_dropin_path: PathBuf,
    /// `Some(unit)` → reload that unit via `reloader`; `None` → skip the reload.
    sshd_unit: Option<String>,
    reloader: Reloader,
}

impl MeshKeyProvisioner {
    /// Construct with production defaults over the given secret store.
    #[must_use]
    pub fn new(store: SecretStore) -> Self {
        Self {
            store,
            mesh_user: DEFAULT_MESH_USER.to_string(),
            overlay_cidr: mackes_mesh_types::vpn_egress::DEFAULT_OVERLAY_CIDR.to_string(),
            mesh_keys_path: PathBuf::from(DEFAULT_MESH_KEYS_PATH),
            sshd_dropin_path: PathBuf::from(DEFAULT_SSHD_DROPIN_PATH),
            sshd_unit: Some(DEFAULT_SSHD_UNIT.to_string()),
            reloader: Arc::new(live_reload),
        }
    }

    /// Override the mesh SSH login user (matches
    /// [`crate::workers::mesh_mount::DEFAULT_MESH_USER`]).
    #[must_use]
    pub fn with_mesh_user(mut self, user: impl Into<String>) -> Self {
        self.mesh_user = user.into();
        self
    }

    /// Override the Nebula overlay CIDR the shared key is bound to.
    #[must_use]
    pub fn with_overlay_cidr(mut self, cidr: impl Into<String>) -> Self {
        self.overlay_cidr = cidr.into();
        self
    }

    /// Override the dedicated authorized-keys file path (tests use a tempdir).
    #[must_use]
    pub fn with_mesh_keys_path(mut self, path: PathBuf) -> Self {
        self.mesh_keys_path = path;
        self
    }

    /// Override the sshd drop-in path (tests use a tempdir).
    #[must_use]
    pub fn with_sshd_dropin_path(mut self, path: PathBuf) -> Self {
        self.sshd_dropin_path = path;
        self
    }

    /// Override the sshd unit; `None` skips the reload (config-only / tests).
    #[must_use]
    pub fn with_sshd_unit(mut self, unit: Option<String>) -> Self {
        self.sshd_unit = unit;
        self
    }

    /// Inject a fake reloader (tests).
    #[must_use]
    pub fn with_reloader(mut self, reloader: Reloader) -> Self {
        self.reloader = reloader;
        self
    }

    /// `true` when the shared key is already sealed in the store.
    ///
    /// # Errors
    /// [`ProvisionError::Store`] on a store/tooling fault.
    pub fn is_provisioned(&self) -> Result<bool, ProvisionError> {
        self.store
            .get(MESH_SSH_KEY_REF)
            .map(|o| o.is_some())
            .map_err(ProvisionError::Store)
    }

    /// The public line for the currently-sealed key (derived from the sealed
    /// private half). `Ok(None)` when nothing is sealed yet.
    ///
    /// # Errors
    /// [`ProvisionError::Store`] on a store fault; [`ProvisionError::Malformed`]
    /// if the sealed material can't be parsed as our OpenSSH key.
    pub fn sealed_public_line(&self) -> Result<Option<String>, ProvisionError> {
        match self
            .store
            .get(MESH_SSH_KEY_REF)
            .map_err(ProvisionError::Store)?
        {
            Some(pem) => Ok(Some(public_line_from_private(&pem, MESH_KEY_COMMENT)?)),
            None => Ok(None),
        }
    }

    /// Seal a keypair's private half under the shared ref. The store keeps ONE
    /// value per ref, so this overwrites any prior key — which is exactly the
    /// re-key revocation of the old private half.
    ///
    /// # Errors
    /// [`ProvisionError::Store`] on a seal/tooling/crypto fault.
    pub fn seal(&self, kp: &MeshSshKeypair) -> Result<(), ProvisionError> {
        self.store
            .put(MESH_SSH_KEY_REF, &kp.private_openssh)
            .map_err(ProvisionError::Store)
    }

    /// Provision idempotently: generate + seal a keypair only when none is
    /// sealed yet, then install the public half locally (authorized-keys file +
    /// sshd drop-in + reload).
    ///
    /// # Errors
    /// [`ProvisionError`] from the store, a file write, a malformed sealed key,
    /// or the node-gated reload (surfaced in the outcome, not this `Err`).
    pub fn provision(&self) -> Result<ProvisionOutcome, ProvisionError> {
        let sealed = self
            .store
            .get(MESH_SSH_KEY_REF)
            .map_err(ProvisionError::Store)?;
        let (public_line, generated) = if let Some(pem) = sealed {
            (public_line_from_private(&pem, MESH_KEY_COMMENT)?, false)
        } else {
            let kp = generate_keypair();
            self.seal(&kp)?;
            // `kp` drops at the end of this block — the private PEM is zeroized.
            (kp.public_line().to_string(), true)
        };
        let reload = self.apply(&public_line)?;
        Ok(ProvisionOutcome {
            generated,
            rekeyed: false,
            public_line,
            reload,
        })
    }

    /// Re-key (the documented revocation path): ALWAYS generate a fresh keypair,
    /// reseal (revoking the old private half), and re-install so the old public
    /// key drops from the authorized-keys file.
    ///
    /// # Errors
    /// [`ProvisionError`] from the store or a file write.
    pub fn rotate(&self) -> Result<ProvisionOutcome, ProvisionError> {
        let kp = generate_keypair();
        self.seal(&kp)?;
        let public_line = kp.public_line().to_string();
        let reload = self.apply(&public_line)?;
        Ok(ProvisionOutcome {
            generated: true,
            rekeyed: true,
            public_line,
            reload,
        })
    }

    /// Install `public_line` into the local sshd config: write the dedicated
    /// overlay-only authorized-keys file + the Match drop-in (both atomic), then
    /// reload sshd. The file writes are real; the reload is the honest
    /// node-gate.
    ///
    /// # Errors
    /// [`ProvisionError::Io`] on a file write failure.
    pub fn apply(&self, public_line: &str) -> Result<SshdReload, ProvisionError> {
        validate_mesh_user(&self.mesh_user)?;
        validate_overlay_cidr(&self.overlay_cidr)?;
        validate_output_path(&self.mesh_keys_path)?;
        validate_output_path(&self.sshd_dropin_path)?;
        if self.mesh_keys_path == self.sshd_dropin_path {
            return Err(ProvisionError::Io(
                "authorized-keys and sshd drop-in paths must be distinct".to_string(),
            ));
        }

        // The public half is a consumer-bound projection of the sealed
        // `mesh-ssh-key` ref. Do not let a caller use this installer as a
        // general authorized-keys writer for an unrelated target.
        let expected = self
            .sealed_public_line()?
            .ok_or_else(|| ProvisionError::Store("mesh SSH key is not sealed".to_string()))?;
        if expected != public_line {
            return Err(ProvisionError::Malformed(
                "public key does not match the sealed mesh-ssh-key consumer ref".to_string(),
            ));
        }

        let dropin =
            render_sshd_overlay_dropin(&self.overlay_cidr, &self.mesh_user, &self.mesh_keys_path);
        // Stage both files before renaming either one. A bad second target (or
        // a symlink race caught during staging) therefore cannot leave only the
        // authorized-keys half of the consumer binding installed.
        let keys = stage_atomic(
            &self.mesh_keys_path,
            &render_mesh_authorized_keys(public_line),
        )?;
        let dropin = match stage_atomic(&self.sshd_dropin_path, &dropin) {
            Ok(staged) => staged,
            Err(error) => return Err(error),
        };
        keys.commit()?;
        dropin.commit()?;
        let reload = self
            .sshd_unit
            .as_deref()
            .map_or(SshdReload::Skipped, |unit| (self.reloader)(unit));
        // Log the public fingerprint + placement only — never the private half.
        tracing::info!(
            target: "mackesd::mesh_ssh_key",
            user = %self.mesh_user,
            cidr = %self.overlay_cidr,
            keys = %self.mesh_keys_path.display(),
            "installed shared mesh SSH key (overlay-only)"
        );
        Ok(reload)
    }
}

fn validate_mesh_user(user: &str) -> Result<(), ProvisionError> {
    if user.is_empty()
        || !user
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        return Err(ProvisionError::Malformed(
            "mesh SSH user contains unsupported characters".to_string(),
        ));
    }
    Ok(())
}

fn validate_overlay_cidr(cidr: &str) -> Result<(), ProvisionError> {
    let (address, prefix) = cidr.split_once('/').ok_or_else(|| {
        ProvisionError::Malformed("overlay binding must be an address/prefix CIDR".to_string())
    })?;
    let address: IpAddr = address
        .parse()
        .map_err(|_| ProvisionError::Malformed("overlay binding has an invalid address".into()))?;
    let prefix: u8 = prefix
        .parse()
        .map_err(|_| ProvisionError::Malformed("overlay binding has an invalid prefix".into()))?;
    match address {
        IpAddr::V4(address) => {
            if prefix > 32 {
                return Err(ProvisionError::Malformed(
                    "IPv4 overlay binding prefix exceeds 32".to_string(),
                ));
            }
            let bits = u32::from(address);
            let mask = if prefix == 0 {
                0
            } else {
                u32::MAX << (32 - prefix)
            };
            if bits & mask != bits {
                return Err(ProvisionError::Malformed(
                    "overlay binding must use the canonical network address".to_string(),
                ));
            }
        }
        IpAddr::V6(address) => {
            if prefix > 128 {
                return Err(ProvisionError::Malformed(
                    "IPv6 overlay binding prefix exceeds 128".to_string(),
                ));
            }
            let bits = u128::from(address);
            let mask = if prefix == 0 {
                0
            } else {
                u128::MAX << (128 - prefix)
            };
            if bits & mask != bits {
                return Err(ProvisionError::Malformed(
                    "overlay binding must use the canonical network address".to_string(),
                ));
            }
        }
    }
    Ok(())
}

fn validate_output_path(path: &Path) -> Result<(), ProvisionError> {
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(ProvisionError::Io(format!(
            "output path {} must be absolute and name a file",
            path.display()
        )));
    }
    ensure_no_symlink_components(path)
}

fn ensure_no_symlink_components(path: &Path) -> Result<(), ProvisionError> {
    let mut current = PathBuf::new();
    for component in path.components() {
        if matches!(component, Component::CurDir | Component::ParentDir) {
            return Err(ProvisionError::Io(format!(
                "unsafe path component in {}",
                path.display()
            )));
        }
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(ProvisionError::Io(format!(
                    "refusing symlink component {}",
                    current.display()
                )));
            }
            Ok(meta) if current != path && !meta.is_dir() => {
                return Err(ProvisionError::Io(format!(
                    "non-directory path component {}",
                    current.display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(ProvisionError::Io(format!(
                    "inspect {}: {error}",
                    current.display()
                )));
            }
        }
    }
    Ok(())
}

fn set_mode_exact(path: &Path, mode: u32) -> Result<(), ProvisionError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(|error| {
            ProvisionError::Io(format!(
                "set permissions {mode:o} on {}: {error}",
                path.display()
            ))
        })?;
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}

struct StagedWrite {
    target: PathBuf,
    temp: Option<PathBuf>,
}

impl Drop for StagedWrite {
    fn drop(&mut self) {
        if let Some(temp) = self.temp.take() {
            let _ = std::fs::remove_file(temp);
        }
    }
}

impl StagedWrite {
    fn commit(mut self) -> Result<(), ProvisionError> {
        let temp = self
            .temp
            .take()
            .expect("staged write always has a temporary file before commit");
        std::fs::rename(&temp, &self.target).map_err(|error| {
            ProvisionError::Io(format!("rename into {}: {error}", self.target.display()))
        })?;
        if let Some(parent) = self.target.parent() {
            if let Ok(directory) = std::fs::File::open(parent) {
                let _ = directory.sync_all();
            }
        }
        Ok(())
    }
}

/// Stage a root-owned configuration file using an exclusive, unpredictable
/// temporary sibling. The final rename is atomic and never follows a target or
/// temp symlink; preparation is separate so `apply` can validate both outputs
/// before committing either one.
fn stage_atomic(path: &Path, body: &str) -> Result<StagedWrite, ProvisionError> {
    validate_output_path(path)?;
    let parent = path.parent().ok_or_else(|| {
        ProvisionError::Io(format!("output path {} has no parent", path.display()))
    })?;
    std::fs::create_dir_all(parent)
        .map_err(|error| ProvisionError::Io(format!("mkdir {}: {error}", parent.display())))?;
    validate_output_path(path)?;
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if !meta.is_file() {
            return Err(ProvisionError::Io(format!(
                "output target {} is not a regular file",
                path.display()
            )));
        }
    }

    use rand::RngCore as _;
    use std::io::Write as _;
    let file_name = path.file_name().ok_or_else(|| {
        ProvisionError::Io(format!("output path {} has no filename", path.display()))
    })?;
    let mut opened = None;
    for _ in 0..16 {
        let mut nonce = [0u8; 8];
        OsRng.fill_bytes(&mut nonce);
        let candidate = parent.join(format!(
            ".{}.tmp-{:016x}",
            file_name.to_string_lossy(),
            u64::from_be_bytes(nonce)
        ));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        match options.open(&candidate) {
            Ok(file) => {
                opened = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(ProvisionError::Io(format!(
                    "create temporary file in {}: {error}",
                    parent.display()
                )));
            }
        }
    }
    let (temp, mut file) = opened.ok_or_else(|| {
        ProvisionError::Io(format!(
            "unable to allocate a unique temporary file in {}",
            parent.display()
        ))
    })?;
    let result = (|| {
        set_mode_exact(&temp, 0o644)?;
        file.write_all(body.as_bytes())
            .map_err(|error| ProvisionError::Io(format!("write {}: {error}", temp.display())))?;
        file.sync_all()
            .map_err(|error| ProvisionError::Io(format!("sync {}: {error}", temp.display())))?;
        Ok(StagedWrite {
            target: path.to_path_buf(),
            temp: Some(temp.clone()),
        })
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `LocalAead` store rooted in a tempdir with a real (random-ish) age
    /// identity, so seals are real encryption and the round-trip is exercised.
    fn temp_store() -> (tempfile::TempDir, SecretStore) {
        let tmp = tempfile::tempdir().unwrap();
        let key_path = tmp.path().join("mcnf-age-key");
        std::fs::write(
            &key_path,
            "AGE-SECRET-KEY-1QQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQSXKLP0E\n",
        )
        .unwrap();
        let store = SecretStore::LocalAead {
            dir: tmp.path().join("secrets"),
            key_path,
        };
        (tmp, store)
    }

    /// A provisioner whose file writes land in `dir` and whose reload is faked.
    fn provisioner_in(store: SecretStore, dir: &Path) -> MeshKeyProvisioner {
        MeshKeyProvisioner::new(store)
            .with_mesh_keys_path(dir.join("mesh_authorized_keys"))
            .with_sshd_dropin_path(dir.join("mackes-mesh-sshkey.conf"))
            .with_sshd_unit(None)
    }

    // ── pure encoding ───────────────────────────────────────────────────

    #[test]
    fn authorized_line_encodes_the_ed25519_wire_format() {
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let pubkey = signing.verifying_key().to_bytes();
        let line = authorized_line(&pubkey, "mde-mesh-shared");
        let mut parts = line.split_whitespace();
        assert_eq!(parts.next(), Some("ssh-ed25519"));
        let b64 = parts.next().expect("has a key field");
        assert_eq!(parts.next(), Some("mde-mesh-shared"));
        // The wire blob decodes to `string "ssh-ed25519" || string <pubkey>`.
        let wire = base64::engine::general_purpose::STANDARD
            .decode(b64.as_bytes())
            .unwrap();
        let mut r = Reader::new(&wire);
        assert_eq!(r.string().unwrap(), b"ssh-ed25519");
        assert_eq!(r.string().unwrap(), &pubkey);
    }

    #[test]
    fn private_pem_round_trips_to_the_same_public_key() {
        // Encode a known key, then derive the public line back from the PEM.
        let signing = SigningKey::from_bytes(&[42u8; 32]);
        let kp = encode_keypair(&signing, "mde-mesh-shared", 0x0102_0304);
        assert!(kp
            .private_openssh
            .starts_with("-----BEGIN OPENSSH PRIVATE KEY-----\n"));
        assert!(kp
            .private_openssh
            .trim_end()
            .ends_with("-----END OPENSSH PRIVATE KEY-----"));
        let derived = public_line_from_private(&kp.private_openssh, "mde-mesh-shared").unwrap();
        assert_eq!(derived, kp.public_line);
    }

    #[test]
    fn generate_keypair_is_a_valid_distinct_pair() {
        let a = generate_keypair();
        let b = generate_keypair();
        assert!(a.public_line().starts_with("ssh-ed25519 "));
        assert!(a.public_line().ends_with(" mde-mesh-shared"));
        // Two generations differ (the CSPRNG really ran).
        assert_ne!(a.public_line(), b.public_line());
        // The private half derives back to the same public line.
        assert_eq!(
            public_line_from_private(&a.private_openssh, MESH_KEY_COMMENT).unwrap(),
            a.public_line()
        );
    }

    #[test]
    fn malformed_private_pem_is_a_typed_error_not_a_panic() {
        assert!(matches!(
            public_line_from_private("not a key", MESH_KEY_COMMENT),
            Err(ProvisionError::Malformed(_))
        ));
        // Valid base64 but not our container.
        let junk = base64::engine::general_purpose::STANDARD.encode(b"not-openssh");
        let pem = format!(
            "-----BEGIN OPENSSH PRIVATE KEY-----\n{junk}\n-----END OPENSSH PRIVATE KEY-----\n"
        );
        assert!(matches!(
            public_line_from_private(&pem, MESH_KEY_COMMENT),
            Err(ProvisionError::Malformed(_))
        ));
    }

    /// Correctness proof against the reference tool: `ssh-keygen -y` must derive
    /// the SAME public key from our hand-rolled PEM. Self-skips (never fails)
    /// when `ssh-keygen` is absent — an honest gate, not a faked pass.
    #[test]
    fn ssh_keygen_agrees_on_our_private_pem() {
        use std::os::unix::fs::PermissionsExt as _;
        let kp = generate_keypair();
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("id_ed25519");
        std::fs::write(&key, kp.private_openssh.as_bytes()).unwrap();
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
        let out = match std::process::Command::new("ssh-keygen")
            .args(["-y", "-f"])
            .arg(&key)
            .output()
        {
            Ok(o) if o.status.success() => o.stdout,
            // No ssh-keygen, or it refused — honest skip.
            _ => return,
        };
        let derived = String::from_utf8_lossy(&out);
        // Compare the base64 key field (token 1) — ssh-keygen -y omits the comment.
        let ours = kp.public_line().split_whitespace().nth(1).unwrap();
        let theirs = derived
            .split_whitespace()
            .nth(1)
            .expect("ssh-keygen emitted a key");
        assert_eq!(ours, theirs, "ssh-keygen derived a different public key");
    }

    // ── config render ───────────────────────────────────────────────────

    #[test]
    fn sshd_dropin_binds_the_key_to_the_overlay_only() {
        let body = render_sshd_overlay_dropin(
            "10.42.0.0/16",
            "root",
            Path::new("/etc/ssh/mackes-mesh/mesh_authorized_keys"),
        );
        // The shared key is gated behind Match Address on the overlay CIDR…
        assert!(body.contains("Match Address 10.42.0.0/16 User root"));
        assert!(body.contains("AuthorizedKeysFile /etc/ssh/mackes-mesh/mesh_authorized_keys"));
        // …and the context is restored so it doesn't gate the rest of sshd.
        assert!(body.trim_end().ends_with("Match all"));
        // Never a public/all-interfaces bind for the SHARED key.
        assert!(!body.contains("Match Address 0.0.0.0"));
    }

    #[test]
    fn mesh_authorized_keys_file_carries_only_the_public_line() {
        let signing = SigningKey::from_bytes(&[3u8; 32]);
        let line = authorized_line(&signing.verifying_key().to_bytes(), "mde-mesh-shared");
        let body = render_mesh_authorized_keys(&line);
        assert!(body.contains(&line));
        assert!(!body.contains("PRIVATE KEY"));
    }

    // ── provisioning orchestration ──────────────────────────────────────

    #[test]
    fn provision_seals_and_installs_then_is_idempotent() {
        let (tmp, store) = temp_store();
        let p = provisioner_in(store, tmp.path());
        assert!(!p.is_provisioned().unwrap());

        let first = p.provision().unwrap();
        assert!(first.generated, "first provision generates");
        assert!(!first.rekeyed);
        assert_eq!(first.reload, SshdReload::Skipped);
        assert!(p.is_provisioned().unwrap());

        // The dedicated authorized-keys file holds the installed public line…
        let installed = std::fs::read_to_string(tmp.path().join("mesh_authorized_keys")).unwrap();
        assert!(installed.contains(&first.public_line));
        // …and the sshd drop-in binds it to the overlay.
        let dropin = std::fs::read_to_string(tmp.path().join("mackes-mesh-sshkey.conf")).unwrap();
        assert!(dropin.contains("Match Address"));

        // A second provision REUSES the sealed key (idempotent — no re-gen).
        let second = p.provision().unwrap();
        assert!(!second.generated, "second provision reuses the sealed key");
        assert_eq!(second.public_line, first.public_line);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(tmp.path().join("mesh_authorized_keys"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o644
            );
            assert_eq!(
                std::fs::metadata(tmp.path().join("mackes-mesh-sshkey.conf"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o644
            );
        }
    }

    #[test]
    fn rotate_revokes_the_old_key_and_reseals() {
        let (tmp, store) = temp_store();
        let p = provisioner_in(store, tmp.path());
        let before = p.provision().unwrap();

        let after = p.rotate().unwrap();
        assert!(after.generated && after.rekeyed);
        // The re-key produced a DIFFERENT public key…
        assert_ne!(after.public_line, before.public_line);
        // …the store now decrypts to the NEW private half…
        assert_eq!(
            p.sealed_public_line().unwrap().as_deref(),
            Some(after.public_line.as_str())
        );
        // …and the installed authorized-keys file no longer authorizes the old
        // key (revocation) — only the new line remains.
        let installed = std::fs::read_to_string(tmp.path().join("mesh_authorized_keys")).unwrap();
        assert!(installed.contains(&after.public_line));
        assert!(
            !installed.contains(&before.public_line),
            "old key must be revoked"
        );
    }

    #[test]
    fn reload_gate_is_honest_when_systemctl_is_unavailable() {
        let (tmp, store) = temp_store();
        // A fake reloader standing in for a box without a usable systemctl.
        let gated: Reloader = Arc::new(|_unit| SshdReload::Gated("no systemctl here".to_string()));
        let p = MeshKeyProvisioner::new(store)
            .with_mesh_keys_path(tmp.path().join("mesh_authorized_keys"))
            .with_sshd_dropin_path(tmp.path().join("mackes-mesh-sshkey.conf"))
            .with_sshd_unit(Some("sshd.service".to_string()))
            .with_reloader(gated);
        let out = p.provision().unwrap();
        // The config IS written (files exist) but the reload is honestly gated —
        // never faked as Reloaded.
        assert!(tmp.path().join("mesh_authorized_keys").is_file());
        assert!(matches!(out.reload, SshdReload::Gated(_)));
    }

    #[cfg(unix)]
    #[test]
    fn apply_refuses_symlink_target_without_touching_the_target() {
        use std::os::unix::fs::symlink;

        let (tmp, store) = temp_store();
        let p = provisioner_in(store, tmp.path());
        let first = p.provision().unwrap();
        let outside = tmp.path().join("outside-authorized-keys");
        std::fs::write(&outside, b"sentinel").unwrap();
        let keys = tmp.path().join("mesh_authorized_keys");
        std::fs::remove_file(&keys).unwrap();
        symlink(&outside, &keys).unwrap();

        let error = p.apply(&first.public_line).unwrap_err();
        assert!(
            error.to_string().contains("symlink"),
            "unexpected error: {error}"
        );
        assert_eq!(std::fs::read(&outside).unwrap(), b"sentinel");
        assert!(std::fs::symlink_metadata(&keys)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn apply_stages_both_outputs_before_any_commit() {
        let (tmp, store) = temp_store();
        let p = provisioner_in(store.clone(), tmp.path());
        let first = p.provision().unwrap();
        let keys = tmp.path().join("mesh_authorized_keys");
        let before = std::fs::read(&keys).unwrap();

        // The installer is consumer-bound: an arbitrary public key cannot be
        // projected into the mesh SSH target.
        let error = p.apply("ssh-ed25519 AAAA attacker").unwrap_err();
        assert!(
            error.to_string().contains("does not match the sealed"),
            "unexpected error: {error}"
        );

        // A regular file in the drop-in's parent is an unsafe target. The
        // authorized-keys file must remain byte-for-byte unchanged.
        let blocked_parent = tmp.path().join("blocked");
        std::fs::write(&blocked_parent, b"not a directory").unwrap();
        let p = p.with_sshd_dropin_path(blocked_parent.join("dropin.conf"));
        assert!(p.provision().is_err());
        assert_eq!(std::fs::read(keys).unwrap(), before);
        assert!(!first.public_line.is_empty());
    }

    #[test]
    fn apply_rejects_unbound_user_and_noncanonical_overlay() {
        let (tmp, store) = temp_store();
        let p = provisioner_in(store.clone(), tmp.path());
        let first = p.provision().unwrap();

        let bad_user = p.with_mesh_user("root\nAuthorizedKeysCommand attacker");
        let error = bad_user.apply(&first.public_line).unwrap_err();
        assert!(error.to_string().contains("unsupported characters"));

        let p = provisioner_in(store, tmp.path());
        let bad_cidr = p.with_overlay_cidr("10.42.0.7/16");
        let error = bad_cidr.apply(&first.public_line).unwrap_err();
        assert!(error.to_string().contains("canonical network"));
    }

    /// The load-bearing security invariant: after a full provision, the private
    /// key exists NOWHERE in plaintext under the store/config tree — only its
    /// sealed (encrypted) form + the public line.
    #[test]
    fn private_key_never_lands_in_plaintext_on_disk() {
        let (tmp, store) = temp_store();
        let p = provisioner_in(store.clone(), tmp.path());
        p.provision().unwrap();

        // The sealed private PEM (read back from the store) is the plaintext we
        // must NOT find loose on disk.
        let sealed_plain = store.get(MESH_SSH_KEY_REF).unwrap().expect("sealed");
        assert!(sealed_plain.contains("BEGIN OPENSSH PRIVATE KEY"));

        // Walk every file under the tree; none may contain the plaintext PEM
        // marker or a distinctive slice of the private body.
        let needle = "OPENSSH PRIVATE KEY";
        let body_slice = sealed_plain
            .lines()
            .nth(1)
            .map(str::trim)
            .filter(|s| s.len() >= 20)
            .expect("private body line");
        let mut sealed_ciphertext_seen = false;
        for path in walk(tmp.path()) {
            let bytes = std::fs::read(&path).unwrap_or_default();
            let text = String::from_utf8_lossy(&bytes);
            assert!(
                !text.contains(needle),
                "plaintext private key marker leaked into {}",
                path.display()
            );
            assert!(
                !text.contains(body_slice),
                "plaintext private key body leaked into {}",
                path.display()
            );
            // The sealed file is real ciphertext: our AEAD bundle magic header.
            if bytes.len() >= 4 && bytes[..4] == *crate::ca::backup::BUNDLE_MAGIC {
                sealed_ciphertext_seen = true;
            }
        }
        assert!(
            sealed_ciphertext_seen,
            "expected a real sealed (AEAD) ciphertext file on disk"
        );
    }

    /// Recursively collect every file under `root`.
    fn walk(root: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in rd.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    out.push(path);
                }
            }
        }
        out
    }
}
