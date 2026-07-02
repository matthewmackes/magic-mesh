//! FILEMGR-5 — the mackesd **mesh-mount worker** (sshfs lifecycle over the
//! Nebula overlay).
//!
//! Design: `docs/design/file-manager-full.md` (locks 11 / 13 / 15 / 17). The
//! `Surface::Files` file manager gets **automatic sshfs access to every mesh
//! node**; per lock 17 the *lifecycle* is owned here, mesh-side, so the desktop
//! surface only **requests** a mount and browses the returned path — the sealed
//! key, the roster, the mount/unmount/health/reconnect concerns all stay in the
//! mesh tier (§6-clean).
//!
//! ## What this worker owns
//!
//! * Drains `action/mesh-mount/<host>` (the `action/<domain>/+` RPC shape, §9 —
//!   the `<host>` is the topic's verb slot; the body carries a TYPED
//!   [`MeshMountVerb`], never a command string). `mount` mounts a peer at a
//!   stable path (`/run/user/<uid>/mde-mesh/<host>`), **home by default**;
//!   `escalate` re-mounts `/` (full filesystem, lock 14); `unmount` tears it
//!   down.
//! * Publishes the per-host lifecycle to `state/mesh-mount/<host>`
//!   (Mounting / Mounted+path / Unreachable / Reconnecting / Unmounted) so the
//!   Files sidebar renders live state (lock 15's honest states).
//! * **Idle-unmount** after [`Self::idle_timeout`] (lock 11), **reconnect with
//!   backoff** on a drop (lock 15), and **frozen/stale-mount detection +
//!   recovery** via a *bounded* liveness probe that never hangs the worker.
//!
//! ## §9 — typed verbs, no raw shell in the ACTION layer
//!
//! The worker itself never shells out. Every side effect that touches sshfs /
//! fusermount goes through the injectable [`MountBackend`] seam; the shared mesh
//! SSH key is resolved through the [`KeyProvider`] seam (FILEMGR-6 seals it into
//! the secret store — here we only *consume* the key ref). This keeps the pure
//! planning + state-machine folds ([`plan_mount`], [`transition`],
//! [`idle_unmount_due`], [`reconnect_backoff`], [`is_stale`]) unit-testable
//! without a runtime, and lets the live sshfs impl be honestly **integration-
//! gated**: on a headless build/CI box (no `/dev/fuse`, no `sshfs`, no peer, no
//! provisioned key) [`SshfsBackend`] returns a typed [`MountError::Gated`] — it
//! NEVER fakes a successful mount (§7).

#![cfg(feature = "async-services")]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

use super::{ShutdownToken, Worker};

/// The `action/mesh-mount/` RPC domain prefix this worker drains.
///
/// A request topic is `action/mesh-mount/<host>` — the `<host>` is the verb slot
/// (`action/<domain>/+`, `rpc.rs`), and the body carries the typed
/// [`MeshMountVerb`].
pub const ACTION_PREFIX: &str = "action/mesh-mount/";

/// The `state/mesh-mount/` publish prefix. One retained-latest record per host
/// (`state/mesh-mount/<host>`) drives the Files sidebar's live pips.
pub const STATE_PREFIX: &str = "state/mesh-mount/";

/// Secret-store key for the shared, node-sealed mesh SSH **private** key (lock 13).
///
/// FILEMGR-6 seals the keypair under this ref; this worker only consumes it (via
/// [`KeyProvider`]). A bare datacenter-tier name (like `do-token` /
/// `media-spaces`) — this string IS the etcd/on-disk key, so a change orphans the
/// sealed key.
pub const MESH_SSH_KEY_REF: &str = "mesh-ssh-key";

/// The mesh SSH login user for the overlay sshfs session.
///
/// The shared key authenticates as this user (FILEMGR-6 installs it into the
/// user's `authorized_keys`, overlay-bound). `root` matches the fleet's
/// passwordless overlay-SSH baseline (`ssh_pubkey_gossip` / `sshd_overlay_bind`);
/// overridable so FILEMGR-6 can pin a dedicated mesh user.
pub const DEFAULT_MESH_USER: &str = "root";

/// Default poll cadence on the request topics + the health/idle/reconnect tick.
///
/// A mount request is rare + human-driven, so a 2 s tick keeps latency
/// imperceptible while the same tick paces idle/stale/backoff bookkeeping.
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(2);

/// Default idle window before an untouched mount is auto-unmounted (lock 11).
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(600);

/// Default bounded sshfs connect timeout (lock 15 — never a frozen UI). Also the
/// ceiling on a liveness probe.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(8);

/// Default attribute/dir cache lifetime baked into the tuned mount opts
/// (seconds). Lock 18 — cache aggressively; a manual refresh busts it UI-side.
pub const DEFAULT_CACHE_SECS: u64 = 20;

/// How long a mount may go without a successful liveness probe before it's
/// treated as **stale/frozen** and recovered (lock 15).
pub const DEFAULT_STALE_AFTER: Duration = Duration::from_secs(45);

// ── the typed request verb ─────────────────────────────────────────────────

/// The typed body of a `action/mesh-mount/<host>` request. There is
/// deliberately **no command/shell variant** (§9): the only verbs are the three
/// lifecycle intents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "verb", rename_all = "snake_case")]
pub enum MeshMountVerb {
    /// Mount the peer's **home** directory (the least-privilege default, lock 14).
    Mount,
    /// Re-mount the peer's **full filesystem** (`/`) — the explicit escalation
    /// (lock 14). Named `escalate` on the wire.
    Escalate,
    /// Unmount the peer + forget it.
    Unmount,
}

impl MeshMountVerb {
    /// Stable tag for logs.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Mount => "mount",
            Self::Escalate => "escalate",
            Self::Unmount => "unmount",
        }
    }
}

/// Parse a typed mesh-mount request body. An empty body defaults to `mount`
/// (the sidebar's "navigate into a peer" nudge carries no body).
///
/// # Errors
/// A malformed non-empty body surfaces as a human-readable string.
pub fn parse_verb(body: &str) -> Result<MeshMountVerb, String> {
    if body.trim().is_empty() {
        return Ok(MeshMountVerb::Mount);
    }
    serde_json::from_str(body).map_err(|e| format!("malformed mesh-mount request: {e}"))
}

// ── the mount scope + plan (pure) ──────────────────────────────────────────

/// Which slice of the remote node a mount exposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MountScope {
    /// The mesh user's home directory (`~`) — the least-privilege default.
    Home,
    /// The full filesystem (`/`) — the escalated scope.
    Full,
}

impl MountScope {
    /// Stable tag for logs + the published state record.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Full => "full",
        }
    }

    /// The remote path this scope maps to in the sshfs spec. Home is the empty
    /// path (sshfs defaults an empty path to the login user's home); Full is `/`.
    #[must_use]
    pub const fn remote_path(self) -> &'static str {
        match self {
            Self::Home => "",
            Self::Full => "/",
        }
    }
}

/// The tuning knobs folded into a [`MountPlan`]'s options. Pulled into a struct
/// so [`plan_mount`] stays a pure fn of its inputs and the worker's live values
/// (which are configurable) flow through unit tests unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MountTuning {
    /// Bounded connect timeout (lock 15).
    pub connect_timeout: Duration,
    /// Attribute/dir/entry cache lifetime in seconds (lock 18).
    pub cache_secs: u64,
}

impl Default for MountTuning {
    fn default() -> Self {
        Self {
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            cache_secs: DEFAULT_CACHE_SECS,
        }
    }
}

/// A fully-resolved, executable mount plan — the pure output of [`plan_mount`].
/// The [`MountBackend`] seam turns this into an sshfs invocation; nothing here
/// touches the filesystem or shells out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountPlan {
    /// Short peer hostname (the roster key + topic verb slot).
    pub host: String,
    /// The sshfs remote spec, `user@<host>.mesh:<remote-path>`.
    pub remote_spec: String,
    /// The stable local mountpoint, `<runtime>/mde-mesh/<host>`.
    pub mountpoint: PathBuf,
    /// Home vs full-filesystem.
    pub scope: MountScope,
    /// The tuned `-o` option list (lock 18).
    pub options: Vec<String>,
    /// The shared mesh SSH identity file the mount authenticates with.
    pub identity_key: PathBuf,
}

/// The stable mountpoint for a peer under a runtime base: `<base>/mde-mesh/<host>`.
#[must_use]
pub fn mountpoint_for(runtime_base: &Path, host: &str) -> PathBuf {
    runtime_base.join("mde-mesh").join(host)
}

/// Build the (pure) mount plan for `host` at `scope`.
///
/// `runtime_base` is the desktop user's `XDG_RUNTIME_DIR` (`/run/user/<uid>`);
/// `mesh_user` + `identity_key` come from lock 13's shared sealed key.
///
/// The option list is the lock-18 tuned set: aggressive attr/dir/kernel cache,
/// `big_writes` + a large `max_read`, `Compression=yes` for WAN, a bounded
/// `ConnectTimeout` + `ServerAlive*` keepalives, and sshfs's own `reconnect`.
#[must_use]
pub fn plan_mount(
    host: &str,
    runtime_base: &Path,
    mesh_user: &str,
    identity_key: &Path,
    scope: MountScope,
    tuning: MountTuning,
) -> MountPlan {
    let fqdn = format!("{host}.{}", super::mesh_dns::MESH_SUFFIX);
    let remote_spec = format!("{mesh_user}@{fqdn}:{}", scope.remote_path());
    let cache = tuning.cache_secs;
    let connect = tuning.connect_timeout.as_secs().max(1);
    let options = vec![
        format!("IdentityFile={}", identity_key.display()),
        // The overlay peer's host key rotates on re-enrollment; accept-new keeps
        // first-use pinning without a stale-key hard-fail, and we never persist
        // it to the operator's known_hosts.
        "StrictHostKeyChecking=accept-new".to_string(),
        "UserKnownHostsFile=/dev/null".to_string(),
        "BatchMode=yes".to_string(),
        format!("ConnectTimeout={connect}"),
        "ServerAliveInterval=15".to_string(),
        "ServerAliveCountMax=3".to_string(),
        // sshfs's own transparent reconnect on a dropped transport.
        "reconnect".to_string(),
        // lock 18 — cache attributes/dir entries/kernel pages; a UI refresh busts it.
        "cache=yes".to_string(),
        format!("cache_timeout={cache}"),
        format!("attr_timeout={cache}"),
        format!("entry_timeout={cache}"),
        "dir_cache=yes".to_string(),
        "kernel_cache".to_string(),
        // lock 18 — big writes + large reads + WAN compression.
        "big_writes".to_string(),
        "max_read=65536".to_string(),
        "Compression=yes".to_string(),
        "follow_symlinks".to_string(),
    ];
    MountPlan {
        host: host.to_string(),
        remote_spec,
        mountpoint: mountpoint_for(runtime_base, host),
        scope,
        options,
        identity_key: identity_key.to_path_buf(),
    }
}

// ── the pure decision folds ────────────────────────────────────────────────

/// Idle-unmount decision (lock 11): a mount untouched for at least
/// `idle_timeout` is due to be released.
#[must_use]
pub fn idle_unmount_due(idle_elapsed: Duration, idle_timeout: Duration) -> bool {
    idle_elapsed >= idle_timeout
}

/// Reconnect backoff (lock 15): exponential from the shared supervisor floor,
/// doubling per attempt, capped at the shared ceiling. `attempt` is the 0-based
/// retry count, so attempt 0 waits the floor.
#[must_use]
pub fn reconnect_backoff(attempt: u32) -> Duration {
    let base_ms = u64::try_from(super::INITIAL_BACKOFF.as_millis()).unwrap_or(u64::MAX);
    // `1 << attempt` saturates to u64::MAX for large attempts, which the `.min`
    // clamps to the cap anyway — so a long outage never overflows.
    let shifted = base_ms.checked_shl(attempt).unwrap_or(u64::MAX);
    Duration::from_millis(shifted).min(super::BACKOFF_CAP)
}

/// Stale/frozen-mount detection (lock 15): a mount whose last successful
/// liveness probe is older than `stale_after` is treated as frozen and recovered.
#[must_use]
pub fn is_stale(since_last_ok_probe: Duration, stale_after: Duration) -> bool {
    since_last_ok_probe >= stale_after
}

// ── the state machine (pure) ───────────────────────────────────────────────

/// The lifecycle phase of one host's mount. Published (via [`state_tag`]) to
/// `state/mesh-mount/<host>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// No mount + not tracked as failing.
    Unmounted,
    /// A mount attempt is in flight.
    Mounting,
    /// Mounted + live.
    Mounted,
    /// Was mounted (or a mount failed transiently) — retrying with backoff.
    Reconnecting,
    /// Peer is offline / the mount can't be established (honest dead-end).
    Unreachable,
}

impl Phase {
    /// The wire tag for the published state record.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Unmounted => "unmounted",
            Self::Mounting => "mounting",
            Self::Mounted => "mounted",
            Self::Reconnecting => "reconnecting",
            Self::Unreachable => "unreachable",
        }
    }
}

/// The lifecycle events the state machine reacts to. Requests arrive on the Bus;
/// the rest are produced by the worker's own health tick + backend outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountEvent {
    /// A `mount`/`escalate` request (the scope is tracked out-of-band).
    Request,
    /// The backend reported the mount succeeded.
    MountOk,
    /// The backend reported a transient mount failure (retry with backoff).
    MountFailed,
    /// The peer is offline / unreachable (an honest dead-end, no fast retry).
    Unreachable,
    /// A previously-live mount dropped its transport.
    Dropped,
    /// The bounded liveness probe succeeded.
    ProbeOk,
    /// The bounded liveness probe timed out / failed — the mount is frozen.
    ProbeStale,
    /// The idle window elapsed.
    IdleTimeout,
    /// The backoff elapsed — time to retry the mount.
    RetryTick,
    /// An explicit `unmount` request (or a completed idle-unmount).
    Unmount,
}

/// The side effect the worker must perform after a [`transition`]. The state
/// table is pure; the worker executes the action through the [`MountBackend`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountAction {
    /// Nothing to do.
    None,
    /// Attempt a fresh mount.
    Mount,
    /// Tear down a broken/frozen mount, then attempt a fresh mount.
    Remount,
    /// Tear down the mount.
    Unmount,
}

/// The pure state-transition table. `(phase, event) → (next_phase, action)`.
/// Fully unit-testable without a runtime, the FUSE layer, or a peer — this is
/// the load-bearing lifecycle logic the acceptance criteria pin.
///
/// Written one-transition-per-arm on purpose: the table reads as the lifecycle
/// diagram, so we keep semantically-distinct arms that happen to share a result
/// separate rather than collapse them into `|`-patterns.
#[must_use]
#[allow(clippy::match_same_arms)]
pub const fn transition(phase: Phase, event: MountEvent) -> (Phase, MountAction) {
    use MountAction as A;
    use MountEvent as E;
    use Phase as P;
    match (phase, event) {
        // A fresh request always drives toward Mounting. From a live mount
        // (an escalate) we must tear down first, hence Remount.
        (P::Mounted, E::Request) => (P::Mounting, A::Remount),
        (_, E::Request) => (P::Mounting, A::Mount),

        // Mount attempt outcomes.
        (P::Mounting, E::MountOk) => (P::Mounted, A::None),
        (P::Mounting, E::MountFailed) => (P::Reconnecting, A::None),
        (P::Mounting, E::Unreachable) => (P::Unreachable, A::None),

        // A live mount degrading — recover (unmount the frozen handle, remount).
        (P::Mounted, E::Dropped | E::ProbeStale) => (P::Reconnecting, A::Remount),
        (P::Mounted, E::ProbeOk) => (P::Mounted, A::None),
        (P::Mounted, E::IdleTimeout) => (P::Unmounted, A::Unmount),

        // Reconnect loop.
        (P::Reconnecting, E::RetryTick) => (P::Mounting, A::Remount),
        (P::Reconnecting, E::MountOk) => (P::Mounted, A::None),
        (P::Reconnecting, E::Unreachable) => (P::Unreachable, A::None),

        // A stubborn dead-end that later comes back is retried on the next tick.
        (P::Unreachable, E::RetryTick) => (P::Mounting, A::Mount),

        // Explicit unmount from any phase (idempotent teardown).
        (_, E::Unmount) => (P::Unmounted, A::Unmount),

        // Everything else is a no-op self-loop (e.g. a ProbeOk while Unmounted,
        // an IdleTimeout while Reconnecting) — never a panic.
        (p, _) => (p, A::None),
    }
}

// ── the typed backend errors + injectable seams ────────────────────────────

/// A typed mount/unmount/probe failure. In-flight ops on a dropped mount fail
/// with one of these (lock 15) — never a fabricated success.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MountError {
    /// The peer is offline / the transport can't be established.
    Unreachable(String),
    /// A bounded operation hit its deadline (a frozen mount / a slow peer).
    Timeout,
    /// The mount handle is stale/frozen.
    Stale,
    /// The backend prerequisites aren't available on this box (no `/dev/fuse`,
    /// no `sshfs`, no provisioned key). The **honest headless gate** — the live
    /// mount is integration-only; it is NEVER faked as success (§7).
    Gated(String),
    /// Any other backend fault, with an operator-readable message.
    Backend(String),
}

impl std::fmt::Display for MountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(m) => write!(f, "unreachable: {m}"),
            Self::Timeout => write!(f, "operation timed out"),
            Self::Stale => write!(f, "mount is stale/frozen"),
            Self::Gated(m) => write!(f, "mount unavailable (gated): {m}"),
            Self::Backend(m) => write!(f, "backend error: {m}"),
        }
    }
}

impl std::error::Error for MountError {}

impl MountError {
    /// Map a failed mount attempt onto the state event it should drive: a hard
    /// unreachable/gated dead-end vs a transient failure worth a backoff retry.
    #[must_use]
    pub const fn as_mount_event(&self) -> MountEvent {
        match self {
            Self::Unreachable(_) | Self::Gated(_) => MountEvent::Unreachable,
            Self::Timeout | Self::Stale | Self::Backend(_) => MountEvent::MountFailed,
        }
    }
}

/// The sshfs/fusermount seam (§9 — the only place that touches FUSE). Injectable
/// so the worker's orchestration is tested with a fake and the live impl stays
/// integration-gated.
pub trait MountBackend: Send + Sync {
    /// Mount per `plan`. Returns a typed error; NEVER fakes success.
    ///
    /// # Errors
    /// Any [`MountError`]; on a headless box it is [`MountError::Gated`].
    fn mount(&self, plan: &MountPlan) -> Result<(), MountError>;

    /// Unmount `mountpoint` (best-effort, idempotent).
    ///
    /// # Errors
    /// A [`MountError`] if the unmount tool fails.
    fn unmount(&self, mountpoint: &Path) -> Result<(), MountError>;

    /// Bounded liveness probe: `Ok(true)` live, `Ok(false)` frozen/stale, `Err`
    /// on a probe fault. MUST return within `timeout` — a frozen FUSE handle can
    /// wedge a bare `stat`, so the impl runs the probe under a hard deadline.
    ///
    /// # Errors
    /// A [`MountError`] if the probe itself fails to run.
    fn probe(&self, mountpoint: &Path, timeout: Duration) -> Result<bool, MountError>;
}

/// The shared-mesh-key seam (lock 13). Resolves the node-sealed private key to a
/// concrete identity file. Injectable so the worker is tested without the secret
/// store.
pub trait KeyProvider: Send + Sync {
    /// Materialize the shared mesh SSH private key to a mode-600 file + return
    /// its path.
    ///
    /// # Errors
    /// [`MountError::Gated`] when the key hasn't been provisioned yet
    /// (FILEMGR-6) — an honest "not provisioned", never a fabricated key.
    fn identity_key(&self) -> Result<PathBuf, MountError>;
}

// ── the live, integration-gated backend ────────────────────────────────────

/// The live sshfs/fusermount backend.
///
/// Integration-only: it honestly refuses on a headless box (no `/dev/fuse`, no
/// `sshfs`) with [`MountError::Gated`] and bounds every operation, so it can
/// never hang the worker and never fakes a mount (§7).
#[derive(Debug, Clone, Default)]
pub struct SshfsBackend;

impl SshfsBackend {
    /// Construct the live backend.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Preflight the host's FUSE prerequisites + the provisioned key. Returns the
    /// gate reason (as a [`MountError::Gated`]) when the live mount can't run
    /// here — this is what keeps the farm/CI path honest.
    fn preflight(plan: &MountPlan) -> Result<(), MountError> {
        if !Path::new("/dev/fuse").exists() {
            return Err(MountError::Gated(
                "/dev/fuse absent — FUSE not available (headless build/CI)".to_string(),
            ));
        }
        if !binary_on_path("sshfs") {
            return Err(MountError::Gated("sshfs binary not found".to_string()));
        }
        if !plan.identity_key.is_file() {
            return Err(MountError::Gated(format!(
                "mesh SSH key not provisioned at {} (FILEMGR-6)",
                plan.identity_key.display()
            )));
        }
        Ok(())
    }
}

impl MountBackend for SshfsBackend {
    fn mount(&self, plan: &MountPlan) -> Result<(), MountError> {
        // Honest gate FIRST: on a box without FUSE/sshfs/key we refuse cleanly
        // rather than shell out into a failure (or, worse, a hang).
        Self::preflight(plan)?;
        if let Some(parent) = plan.mountpoint.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| MountError::Backend(format!("mkdir mountpoint parent: {e}")))?;
        }
        std::fs::create_dir_all(&plan.mountpoint)
            .map_err(|e| MountError::Backend(format!("mkdir mountpoint: {e}")))?;
        // sshfs backgrounds itself on success; the -o ConnectTimeout in the plan
        // bounds the connect so this never hangs indefinitely.
        let opts = plan.options.join(",");
        let out = Command::new("sshfs")
            .arg(&plan.remote_spec)
            .arg(&plan.mountpoint)
            .arg("-o")
            .arg(&opts)
            .output()
            .map_err(|e| MountError::Backend(format!("spawn sshfs: {e}")))?;
        if out.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            Err(MountError::Unreachable(if stderr.is_empty() {
                format!("sshfs exit {:?}", out.status.code())
            } else {
                stderr
            }))
        }
    }

    fn unmount(&self, mountpoint: &Path) -> Result<(), MountError> {
        // fusermount3 is the fuse3 name; fall back to fusermount. `-u -z` lazily
        // detaches even a busy/frozen handle so recovery never blocks.
        for bin in ["fusermount3", "fusermount"] {
            if binary_on_path(bin) {
                let out = Command::new(bin)
                    .arg("-u")
                    .arg("-z")
                    .arg(mountpoint)
                    .output()
                    .map_err(|e| MountError::Backend(format!("spawn {bin}: {e}")))?;
                return if out.status.success() {
                    Ok(())
                } else {
                    Err(MountError::Backend(
                        String::from_utf8_lossy(&out.stderr).trim().to_string(),
                    ))
                };
            }
        }
        Err(MountError::Gated(
            "no fusermount binary available".to_string(),
        ))
    }

    fn probe(&self, mountpoint: &Path, timeout: Duration) -> Result<bool, MountError> {
        // A frozen FUSE handle can wedge a bare `stat` in uninterruptible sleep,
        // so run the probe under coreutils `timeout`, which SIGKILLs the child on
        // deadline (exit 124) — the bounded stale-detection guard (lock 15).
        if !binary_on_path("timeout") {
            return Err(MountError::Gated("no timeout binary available".to_string()));
        }
        let secs = timeout.as_secs().max(1);
        let out = Command::new("timeout")
            .arg(secs.to_string())
            .arg("stat")
            .arg("--")
            .arg(mountpoint)
            .output()
            .map_err(|e| MountError::Backend(format!("spawn probe: {e}")))?;
        // Exit 0 = the stat returned ⇒ live. Anything else is stale/frozen:
        // 124 = `timeout` SIGKILLed a wedged stat, and any other non-zero
        // (ENOTCONN, a vanished mountpoint, …) is likewise a dead mount.
        Ok(out.status.code() == Some(0))
    }
}

/// The live [`KeyProvider`]: reads the sealed key from the mesh secret store +
/// materializes it mode-600 under the runtime dir.
pub struct SecretStoreKeyProvider {
    /// Where to write the materialized key file.
    key_path: PathBuf,
    /// The repo root the secret-store helper resolves from.
    repo_dir: PathBuf,
    /// The workgroup root for the local-AEAD fallback store.
    workgroup_root: PathBuf,
}

impl SecretStoreKeyProvider {
    /// Construct with the materialized-key path + secret-store roots.
    #[must_use]
    pub const fn new(key_path: PathBuf, repo_dir: PathBuf, workgroup_root: PathBuf) -> Self {
        Self {
            key_path,
            repo_dir,
            workgroup_root,
        }
    }
}

impl KeyProvider for SecretStoreKeyProvider {
    fn identity_key(&self) -> Result<PathBuf, MountError> {
        let store =
            crate::ipc::secret_store::SecretStore::resolve(&self.repo_dir, &self.workgroup_root);
        let material = store
            .get(MESH_SSH_KEY_REF)
            .map_err(MountError::Backend)?
            .ok_or_else(|| {
                MountError::Gated(format!(
                    "shared mesh SSH key `{MESH_SSH_KEY_REF}` not sealed yet (FILEMGR-6)"
                ))
            })?;
        if let Some(parent) = self.key_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| MountError::Backend(format!("mkdir key dir: {e}")))?;
        }
        std::fs::write(&self.key_path, material.as_bytes())
            .map_err(|e| MountError::Backend(format!("write key: {e}")))?;
        set_mode_600(&self.key_path).map_err(|e| MountError::Backend(format!("chmod key: {e}")))?;
        Ok(self.key_path.clone())
    }
}

/// `true` when `bin` resolves on `$PATH` (via `which`), used by the live backend
/// gate. Cheap + bounded.
fn binary_on_path(bin: &str) -> bool {
    Command::new("which")
        .arg(bin)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// chmod 600 a freshly-written private key (Unix).
fn set_mode_600(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

// ── the published state record ─────────────────────────────────────────────

/// One host's live mount state, published to `state/mesh-mount/<host>`. The
/// Files sidebar reads this for the per-peer pip + the mounted path.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MeshMountState {
    /// The peer hostname.
    pub host: String,
    /// The phase tag (`mounted` / `mounting` / …).
    pub state: String,
    /// The mount scope tag, when relevant (`home` / `full`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// The live mountpoint, present once `mounted`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// A human-readable reason on a degrade path (unreachable/reconnecting).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Wall-clock epoch millis of this transition.
    pub since_ms: u64,
}

// ── the worker ─────────────────────────────────────────────────────────────

/// Per-host live bookkeeping (worker-internal; the pure decisions are folded out
/// into the free fns above).
struct HostEntry {
    phase: Phase,
    scope: MountScope,
    mountpoint: PathBuf,
    /// Last time a request or a successful probe touched this mount (idle clock).
    last_activity: Instant,
    /// Last successful liveness probe (stale clock).
    last_ok_probe: Instant,
    /// Current reconnect attempt (drives the backoff).
    reconnect_attempt: u32,
    /// When the next reconnect retry is due (while Reconnecting).
    next_retry_at: Option<Instant>,
    /// The last degrade reason, surfaced in the published state.
    reason: Option<String>,
}

impl HostEntry {
    fn new(scope: MountScope, mountpoint: PathBuf) -> Self {
        let now = Instant::now();
        Self {
            phase: Phase::Unmounted,
            scope,
            mountpoint,
            last_activity: now,
            last_ok_probe: now,
            reconnect_attempt: 0,
            next_retry_at: None,
            reason: None,
        }
    }
}

/// FILEMGR-5 — the mesh-mount lifecycle worker.
pub struct MeshMountWorker {
    /// Desktop runtime base (`/run/user/<uid>`) — resolved once at startup.
    runtime_base: PathBuf,
    /// The mesh SSH login user.
    mesh_user: String,
    /// The sshfs backend seam.
    backend: std::sync::Arc<dyn MountBackend>,
    /// The shared-key seam.
    keys: std::sync::Arc<dyn KeyProvider>,
    /// Mount tuning (connect timeout + cache).
    tuning: MountTuning,
    /// Idle-unmount window.
    idle_timeout: Duration,
    /// Stale-mount detection window.
    stale_after: Duration,
    /// Poll/health tick cadence.
    tick: Duration,
    /// Bus spool root override (tests point this at a tempdir).
    bus_root_override: Option<PathBuf>,
    /// Per-host live state.
    entries: HashMap<String, HostEntry>,
    /// Per-topic request cursors (`action/mesh-mount/<host>` → last ULID).
    cursors: HashMap<String, String>,
}

impl MeshMountWorker {
    /// Construct with production seams + defaults. `runtime_base` is the desktop
    /// user's `/run/user/<uid>`; `repo_dir`/`workgroup_root` locate the secret
    /// store the shared key is sealed in.
    #[must_use]
    pub fn new(runtime_base: PathBuf, repo_dir: PathBuf, workgroup_root: PathBuf) -> Self {
        let key_path = runtime_base.join("mde-mesh").join(".mesh-ssh-key");
        let keys = std::sync::Arc::new(SecretStoreKeyProvider::new(
            key_path,
            repo_dir,
            workgroup_root,
        ));
        Self {
            runtime_base,
            mesh_user: DEFAULT_MESH_USER.to_string(),
            backend: std::sync::Arc::new(SshfsBackend::new()),
            keys,
            tuning: MountTuning::default(),
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            stale_after: DEFAULT_STALE_AFTER,
            tick: DEFAULT_TICK_INTERVAL,
            bus_root_override: None,
            entries: HashMap::new(),
            cursors: HashMap::new(),
        }
    }

    /// Inject the backend seam (tests use a fake).
    #[must_use]
    pub fn with_backend(mut self, backend: std::sync::Arc<dyn MountBackend>) -> Self {
        self.backend = backend;
        self
    }

    /// Inject the key-provider seam (tests use a fake).
    #[must_use]
    pub fn with_key_provider(mut self, keys: std::sync::Arc<dyn KeyProvider>) -> Self {
        self.keys = keys;
        self
    }

    /// Override the Bus spool root (tests).
    #[must_use]
    pub fn with_bus_root(mut self, root: PathBuf) -> Self {
        self.bus_root_override = Some(root);
        self
    }

    /// Override the idle-unmount window (tests use a short value).
    #[must_use]
    pub const fn with_idle_timeout(mut self, d: Duration) -> Self {
        self.idle_timeout = d;
        self
    }

    /// Override the poll/health cadence (tests use a short value).
    #[must_use]
    pub const fn with_tick(mut self, d: Duration) -> Self {
        self.tick = d;
        self
    }

    /// Override the mesh SSH login user.
    #[must_use]
    pub fn with_mesh_user(mut self, user: impl Into<String>) -> Self {
        self.mesh_user = user.into();
        self
    }

    /// Publish the current state record for `host`.
    fn publish_state(&self, persist: &Persist, host: &str) {
        let Some(entry) = self.entries.get(host) else {
            return;
        };
        let scope = matches!(entry.phase, Phase::Mounted | Phase::Mounting)
            .then(|| entry.scope.tag().to_string());
        let path =
            matches!(entry.phase, Phase::Mounted).then(|| entry.mountpoint.display().to_string());
        let rec = MeshMountState {
            host: host.to_string(),
            state: entry.phase.tag().to_string(),
            scope,
            path,
            reason: entry.reason.clone(),
            since_ms: now_ms(),
        };
        let body = serde_json::to_string(&rec).unwrap_or_default();
        let topic = format!("{STATE_PREFIX}{host}");
        if let Err(e) = persist.write(&topic, Priority::Default, None, Some(&body)) {
            tracing::warn!(target: "mackesd::mesh_mount", host, error = %e, "state publish failed");
        }
    }

    /// Drive one event through the pure [`transition`] table + execute the
    /// resulting [`MountAction`] against the backend, chaining the outcome event
    /// (`MountOk` / a typed failure). Then publish the new state.
    fn apply(&mut self, persist: &Persist, host: &str, event: MountEvent) {
        let Some(entry) = self.entries.get_mut(host) else {
            return;
        };
        let (next, action) = transition(entry.phase, event);
        entry.phase = next;
        match action {
            MountAction::None => {}
            MountAction::Mount => self.do_mount(host),
            MountAction::Remount => {
                self.do_unmount(host);
                self.do_mount(host);
            }
            MountAction::Unmount => self.do_unmount(host),
        }
        self.publish_state(persist, host);
    }

    /// Execute a mount attempt for `host`: resolve the shared key, build the pure
    /// plan, call the backend, and fold the typed outcome back into the phase.
    fn do_mount(&mut self, host: &str) {
        let scope = self.entries.get(host).map_or(MountScope::Home, |e| e.scope);
        let key = match self.keys.identity_key() {
            Ok(k) => k,
            Err(e) => {
                self.record_mount_failure(host, &e);
                return;
            }
        };
        let plan = plan_mount(
            host,
            &self.runtime_base,
            &self.mesh_user,
            &key,
            scope,
            self.tuning,
        );
        match self.backend.mount(&plan) {
            Ok(()) => {
                if let Some(entry) = self.entries.get_mut(host) {
                    entry.phase = Phase::Mounted;
                    entry.mountpoint = plan.mountpoint;
                    entry.reason = None;
                    entry.reconnect_attempt = 0;
                    entry.next_retry_at = None;
                    let now = Instant::now();
                    entry.last_activity = now;
                    entry.last_ok_probe = now;
                }
                tracing::info!(target: "mackesd::mesh_mount", host, scope = scope.tag(), "mounted");
            }
            Err(e) => self.record_mount_failure(host, &e),
        }
    }

    /// Fold a typed mount failure into the phase: a hard unreachable/gated
    /// dead-end vs a transient failure that schedules a backoff retry.
    fn record_mount_failure(&mut self, host: &str, err: &MountError) {
        let event = err.as_mount_event();
        let Some(entry) = self.entries.get_mut(host) else {
            return;
        };
        entry.reason = Some(err.to_string());
        let (next, _) = transition(entry.phase, event);
        entry.phase = next;
        if matches!(next, Phase::Reconnecting) {
            let backoff = reconnect_backoff(entry.reconnect_attempt);
            entry.next_retry_at = Some(Instant::now() + backoff);
            entry.reconnect_attempt = entry.reconnect_attempt.saturating_add(1);
        }
        tracing::warn!(
            target: "mackesd::mesh_mount",
            host,
            phase = next.tag(),
            error = %err,
            "mount attempt failed",
        );
    }

    /// Best-effort teardown (idempotent). Backend errors are logged, not fatal.
    fn do_unmount(&self, host: &str) {
        let Some(entry) = self.entries.get(host) else {
            return;
        };
        let mountpoint = entry.mountpoint.clone();
        if let Err(e) = self.backend.unmount(&mountpoint) {
            tracing::debug!(target: "mackesd::mesh_mount", host, error = %e, "unmount (best-effort)");
        }
    }

    /// Drain net-new requests across every `action/mesh-mount/<host>` topic +
    /// run the health/idle/reconnect tick. Fully synchronous so the `&Persist`
    /// borrow is held across the whole sweep without breaking `Send`.
    fn sweep(&mut self, persist: &Persist) {
        self.drain_requests(persist);
        self.health_tick(persist);
    }

    /// Poll each request topic since its cursor, mapping the typed verb onto a
    /// state event.
    fn drain_requests(&mut self, persist: &Persist) {
        let topics = match persist.list_topics() {
            Ok(t) => t,
            Err(e) => {
                tracing::debug!(target: "mackesd::mesh_mount", error = %e, "list_topics failed");
                return;
            }
        };
        for topic in topics
            .into_iter()
            .filter(|t| t.starts_with(ACTION_PREFIX) && t.len() > ACTION_PREFIX.len())
        {
            let host = topic[ACTION_PREFIX.len()..].to_string();
            let cursor = self.cursors.get(&topic).cloned();
            let msgs = match persist.list_since(&topic, cursor.as_deref()) {
                Ok(m) => m,
                Err(e) => {
                    tracing::debug!(target: "mackesd::mesh_mount", topic, error = %e, "list_since failed");
                    continue;
                }
            };
            for msg in msgs {
                self.cursors.insert(topic.clone(), msg.ulid.clone());
                let verb = match parse_verb(msg.body.as_deref().unwrap_or_default()) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(target: "mackesd::mesh_mount", host = %host, error = %e, "bad request");
                        continue;
                    }
                };
                self.handle_verb(persist, &host, verb);
            }
        }
    }

    /// Apply one typed verb to a host, tracking the desired scope.
    fn handle_verb(&mut self, persist: &Persist, host: &str, verb: MeshMountVerb) {
        let mountpoint = mountpoint_for(&self.runtime_base, host);
        match verb {
            MeshMountVerb::Mount | MeshMountVerb::Escalate => {
                let scope = if matches!(verb, MeshMountVerb::Escalate) {
                    MountScope::Full
                } else {
                    // A plain re-request keeps an already-escalated scope; a fresh
                    // host defaults to Home.
                    self.entries.get(host).map_or(MountScope::Home, |e| e.scope)
                };
                let entry = self
                    .entries
                    .entry(host.to_string())
                    .or_insert_with(|| HostEntry::new(scope, mountpoint));
                entry.scope = scope;
                entry.last_activity = Instant::now();
                self.apply(persist, host, MountEvent::Request);
            }
            MeshMountVerb::Unmount => {
                if self.entries.contains_key(host) {
                    self.apply(persist, host, MountEvent::Unmount);
                    self.entries.remove(host);
                    // A terminal unmounted record so the sidebar clears the pip.
                    let rec = MeshMountState {
                        host: host.to_string(),
                        state: Phase::Unmounted.tag().to_string(),
                        scope: None,
                        path: None,
                        reason: None,
                        since_ms: now_ms(),
                    };
                    let body = serde_json::to_string(&rec).unwrap_or_default();
                    let _ = persist.write(
                        &format!("{STATE_PREFIX}{host}"),
                        Priority::Default,
                        None,
                        Some(&body),
                    );
                }
            }
        }
    }

    /// The health tick: idle-unmount live mounts, probe for stale/frozen mounts
    /// + recover, and fire due reconnect retries.
    fn health_tick(&mut self, persist: &Persist) {
        let now = Instant::now();
        let hosts: Vec<String> = self.entries.keys().cloned().collect();
        for host in hosts {
            let Some(entry) = self.entries.get(&host) else {
                continue;
            };
            match entry.phase {
                Phase::Mounted => {
                    // Idle-unmount takes precedence over a probe.
                    if idle_unmount_due(now.duration_since(entry.last_activity), self.idle_timeout)
                    {
                        tracing::info!(target: "mackesd::mesh_mount", host = %host, "idle — unmounting");
                        self.apply(persist, &host, MountEvent::IdleTimeout);
                        continue;
                    }
                    // Bounded liveness probe → stale detection/recovery.
                    let mountpoint = entry.mountpoint.clone();
                    match self.backend.probe(&mountpoint, self.tuning.connect_timeout) {
                        Ok(true) => {
                            if let Some(e) = self.entries.get_mut(&host) {
                                e.last_ok_probe = now;
                            }
                            self.apply(persist, &host, MountEvent::ProbeOk);
                        }
                        Ok(false) | Err(_) => {
                            if is_stale(now.duration_since(entry.last_ok_probe), self.stale_after) {
                                if let Some(e) = self.entries.get_mut(&host) {
                                    e.reason = Some("connection lost — reconnecting".to_string());
                                }
                                self.apply(persist, &host, MountEvent::ProbeStale);
                            }
                        }
                    }
                }
                Phase::Reconnecting => {
                    if entry.next_retry_at.is_some_and(|due| now >= due) {
                        self.apply(persist, &host, MountEvent::RetryTick);
                    }
                }
                _ => {}
            }
        }
    }
}

#[async_trait::async_trait]
impl Worker for MeshMountWorker {
    fn name(&self) -> &'static str {
        "mesh_mount"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(bus_root) = self
            .bus_root_override
            .clone()
            .or_else(mde_bus::default_data_dir)
        else {
            tracing::debug!(target: "mackesd::mesh_mount", "no bus root; worker idle");
            return Ok(());
        };
        let persist = match Persist::open(bus_root) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(target: "mackesd::mesh_mount", error = %e, "persist open failed; worker idle");
                return Ok(());
            }
        };
        // Seed each existing request topic's cursor at its tail so a restart
        // doesn't replay + re-mount stale requests.
        if let Ok(topics) = persist.list_topics() {
            for topic in topics
                .into_iter()
                .filter(|t| t.starts_with(ACTION_PREFIX) && t.len() > ACTION_PREFIX.len())
            {
                if let Ok(Some(ulid)) = persist.latest_ulid(&topic) {
                    self.cursors.insert(topic, ulid);
                }
            }
        }
        let mut tick = tokio::time::interval(self.tick);
        tick.tick().await; // burn the immediate first tick
        loop {
            tokio::select! {
                _ = tick.tick() => self.sweep(&persist),
                () = shutdown.wait() => break,
            }
        }
        // Clean shutdown: unmount everything we hold so a restart comes back to a
        // clean slate (no orphaned frozen handles).
        let hosts: Vec<String> = self.entries.keys().cloned().collect();
        for host in hosts {
            self.do_unmount(&host);
        }
        Ok(())
    }
}

/// Resolve the desktop user's `/run/user/<uid>` runtime base for the mount
/// tree, via the seated graphical session (reused from `clipboard_sync`);
/// falls back to `$XDG_RUNTIME_DIR`, then `/run/user/1000`.
#[must_use]
pub fn resolve_runtime_base() -> PathBuf {
    if let Ok(session) = super::clipboard_sync::session::discover() {
        return session.runtime_dir;
    }
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir);
    }
    PathBuf::from("/run/user/1000")
}

/// Wall-clock epoch millis for the published state record.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    // ── pure plan folds ─────────────────────────────────────────────────

    #[test]
    fn plan_home_mounts_the_empty_remote_path() {
        let plan = plan_mount(
            "oak",
            Path::new("/run/user/1000"),
            "root",
            Path::new("/run/user/1000/mde-mesh/.mesh-ssh-key"),
            MountScope::Home,
            MountTuning::default(),
        );
        assert_eq!(plan.remote_spec, "root@oak.mesh:");
        assert_eq!(
            plan.mountpoint,
            PathBuf::from("/run/user/1000/mde-mesh/oak")
        );
        assert_eq!(plan.scope, MountScope::Home);
    }

    #[test]
    fn plan_escalated_mounts_the_full_filesystem() {
        let plan = plan_mount(
            "oak",
            Path::new("/run/user/1000"),
            "mesh",
            Path::new("/k"),
            MountScope::Full,
            MountTuning::default(),
        );
        assert_eq!(plan.remote_spec, "mesh@oak.mesh:/");
        assert_eq!(plan.scope, MountScope::Full);
    }

    #[test]
    fn plan_carries_the_tuned_options_and_identity() {
        let plan = plan_mount(
            "oak",
            Path::new("/run/user/1000"),
            "root",
            Path::new("/keys/id"),
            MountScope::Home,
            MountTuning {
                connect_timeout: Duration::from_secs(8),
                cache_secs: 20,
            },
        );
        let opts = plan.options.join(",");
        // lock 18 tuning present.
        assert!(opts.contains("IdentityFile=/keys/id"));
        assert!(opts.contains("big_writes"));
        assert!(opts.contains("kernel_cache"));
        assert!(opts.contains("Compression=yes"));
        assert!(opts.contains("reconnect"));
        assert!(opts.contains("cache_timeout=20"));
        assert!(opts.contains("ConnectTimeout=8"));
        assert!(opts.contains("ServerAliveInterval=15"));
    }

    // ── pure decision folds ─────────────────────────────────────────────

    #[test]
    fn idle_decision_fires_at_the_window() {
        assert!(!idle_unmount_due(
            Duration::from_secs(59),
            Duration::from_secs(60)
        ));
        assert!(idle_unmount_due(
            Duration::from_secs(60),
            Duration::from_secs(60)
        ));
        assert!(idle_unmount_due(
            Duration::from_secs(600),
            Duration::from_secs(60)
        ));
    }

    #[test]
    fn reconnect_backoff_doubles_and_caps() {
        assert_eq!(reconnect_backoff(0), super::super::INITIAL_BACKOFF);
        assert_eq!(reconnect_backoff(1), super::super::INITIAL_BACKOFF * 2);
        assert_eq!(reconnect_backoff(2), super::super::INITIAL_BACKOFF * 4);
        // Large attempts never overflow + clamp to the ceiling.
        assert_eq!(reconnect_backoff(40), super::super::BACKOFF_CAP);
        assert_eq!(reconnect_backoff(1000), super::super::BACKOFF_CAP);
    }

    #[test]
    fn stale_detection_at_the_bound() {
        assert!(!is_stale(Duration::from_secs(44), Duration::from_secs(45)));
        assert!(is_stale(Duration::from_secs(45), Duration::from_secs(45)));
    }

    // ── the state-transition table ──────────────────────────────────────

    #[test]
    fn transitions_cover_the_happy_path() {
        // request → mounting(mount) → mounted
        assert_eq!(
            transition(Phase::Unmounted, MountEvent::Request),
            (Phase::Mounting, MountAction::Mount)
        );
        assert_eq!(
            transition(Phase::Mounting, MountEvent::MountOk),
            (Phase::Mounted, MountAction::None)
        );
        // idle → unmounted(unmount)
        assert_eq!(
            transition(Phase::Mounted, MountEvent::IdleTimeout),
            (Phase::Unmounted, MountAction::Unmount)
        );
    }

    #[test]
    fn transitions_cover_drop_and_recovery() {
        // a live mount that drops or goes stale → reconnecting(remount)
        assert_eq!(
            transition(Phase::Mounted, MountEvent::Dropped),
            (Phase::Reconnecting, MountAction::Remount)
        );
        assert_eq!(
            transition(Phase::Mounted, MountEvent::ProbeStale),
            (Phase::Reconnecting, MountAction::Remount)
        );
        // the backoff elapses → retry
        assert_eq!(
            transition(Phase::Reconnecting, MountEvent::RetryTick),
            (Phase::Mounting, MountAction::Remount)
        );
    }

    #[test]
    fn transitions_cover_unreachable_and_escalate() {
        // a mount attempt against an offline peer → unreachable
        assert_eq!(
            transition(Phase::Mounting, MountEvent::Unreachable),
            (Phase::Unreachable, MountAction::None)
        );
        // an unreachable peer that returns is retried
        assert_eq!(
            transition(Phase::Unreachable, MountEvent::RetryTick),
            (Phase::Mounting, MountAction::Mount)
        );
        // an escalate on a live mount tears down + remounts
        assert_eq!(
            transition(Phase::Mounted, MountEvent::Request),
            (Phase::Mounting, MountAction::Remount)
        );
        // explicit unmount from anywhere
        assert_eq!(
            transition(Phase::Reconnecting, MountEvent::Unmount),
            (Phase::Unmounted, MountAction::Unmount)
        );
    }

    #[test]
    fn unknown_transitions_are_noop_self_loops() {
        assert_eq!(
            transition(Phase::Unmounted, MountEvent::ProbeOk),
            (Phase::Unmounted, MountAction::None)
        );
        assert_eq!(
            transition(Phase::Reconnecting, MountEvent::IdleTimeout),
            (Phase::Reconnecting, MountAction::None)
        );
    }

    #[test]
    fn verb_parse_defaults_empty_to_mount() {
        assert_eq!(parse_verb("").unwrap(), MeshMountVerb::Mount);
        assert_eq!(
            parse_verb(r#"{"verb":"escalate"}"#).unwrap(),
            MeshMountVerb::Escalate
        );
        assert_eq!(
            parse_verb(r#"{"verb":"unmount"}"#).unwrap(),
            MeshMountVerb::Unmount
        );
        assert!(parse_verb(r#"{"verb":"rm-rf"}"#).is_err());
    }

    #[test]
    fn mount_error_maps_to_the_right_event() {
        assert_eq!(
            MountError::Unreachable("x".into()).as_mount_event(),
            MountEvent::Unreachable
        );
        assert_eq!(
            MountError::Gated("x".into()).as_mount_event(),
            MountEvent::Unreachable
        );
        assert_eq!(
            MountError::Timeout.as_mount_event(),
            MountEvent::MountFailed
        );
        assert_eq!(MountError::Stale.as_mount_event(), MountEvent::MountFailed);
    }

    // ── the live backend is honestly gated (never fakes success) ─────────

    #[test]
    fn live_backend_never_fakes_a_mount_when_key_absent() {
        // With a plan pointing at a nonexistent identity key, the live backend
        // MUST refuse with a typed Gated error (never Ok) — the §7 honest gate.
        // (Deterministic: the key-file preflight fails fast, no network, no hang.)
        let plan = plan_mount(
            "nohost",
            Path::new("/run/user/1000"),
            "root",
            Path::new("/nonexistent/definitely/not/a/key"),
            MountScope::Home,
            MountTuning::default(),
        );
        let backend = SshfsBackend::new();
        let res = backend.mount(&plan);
        assert!(res.is_err(), "headless live mount must never succeed");
        // On a box without /dev/fuse or sshfs the gate reason is Gated; on a box
        // that has them the missing key is still a Gated refusal. Either way it's
        // never Ok and never a different (unexpected) success.
        assert!(matches!(res, Err(MountError::Gated(_))));
    }

    // ── orchestration over fake seams (no FUSE, no network) ──────────────

    /// A scripted fake backend: `mount()` consults a queue of results so a test
    /// can drive Ok/typed-error sequences; `probe()` returns a settable liveness.
    struct FakeBackend {
        mount_results: Mutex<Vec<Result<(), MountError>>>,
        mounts: AtomicUsize,
        unmounts: AtomicUsize,
        probe_live: Mutex<bool>,
    }

    impl FakeBackend {
        fn new(results: Vec<Result<(), MountError>>) -> Arc<Self> {
            Arc::new(Self {
                mount_results: Mutex::new(results),
                mounts: AtomicUsize::new(0),
                unmounts: AtomicUsize::new(0),
                probe_live: Mutex::new(true),
            })
        }
    }

    impl MountBackend for FakeBackend {
        fn mount(&self, _plan: &MountPlan) -> Result<(), MountError> {
            self.mounts.fetch_add(1, Ordering::SeqCst);
            let mut q = self.mount_results.lock().unwrap();
            if q.is_empty() {
                Ok(())
            } else {
                q.remove(0)
            }
        }
        fn unmount(&self, _mountpoint: &Path) -> Result<(), MountError> {
            self.unmounts.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn probe(&self, _m: &Path, _t: Duration) -> Result<bool, MountError> {
            Ok(*self.probe_live.lock().unwrap())
        }
    }

    struct FakeKeys;
    impl KeyProvider for FakeKeys {
        fn identity_key(&self) -> Result<PathBuf, MountError> {
            Ok(PathBuf::from("/tmp/fake-mesh-key"))
        }
    }

    fn worker_with(backend: Arc<dyn MountBackend>) -> MeshMountWorker {
        MeshMountWorker::new(
            PathBuf::from("/run/user/1000"),
            PathBuf::from("/nonexistent-repo"),
            PathBuf::from("/nonexistent-wg"),
        )
        .with_backend(backend)
        .with_key_provider(Arc::new(FakeKeys))
        .with_idle_timeout(Duration::from_millis(0))
    }

    fn temp_persist() -> (tempfile::TempDir, Persist) {
        let dir = tempfile::tempdir().unwrap();
        let persist = Persist::open(dir.path().to_path_buf()).unwrap();
        (dir, persist)
    }

    #[test]
    fn request_mount_reaches_mounted_over_the_fake_backend() {
        let (_d, persist) = temp_persist();
        let backend = FakeBackend::new(vec![Ok(())]);
        let mut w = worker_with(backend.clone());
        w.handle_verb(&persist, "oak", MeshMountVerb::Mount);
        assert_eq!(w.entries["oak"].phase, Phase::Mounted);
        assert_eq!(w.entries["oak"].scope, MountScope::Home);
        assert_eq!(backend.mounts.load(Ordering::SeqCst), 1);
        // state was published.
        let latest = persist.latest_ulid("state/mesh-mount/oak").unwrap();
        assert!(latest.is_some());
    }

    #[test]
    fn escalate_remounts_at_full_scope() {
        let (_d, persist) = temp_persist();
        let backend = FakeBackend::new(vec![Ok(()), Ok(())]);
        let mut w = worker_with(backend.clone());
        w.handle_verb(&persist, "oak", MeshMountVerb::Mount);
        w.handle_verb(&persist, "oak", MeshMountVerb::Escalate);
        assert_eq!(w.entries["oak"].scope, MountScope::Full);
        assert_eq!(w.entries["oak"].phase, Phase::Mounted);
        // the escalate tore down (Remount = unmount+mount) then mounted again.
        assert!(backend.unmounts.load(Ordering::SeqCst) >= 1);
        assert_eq!(backend.mounts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn unreachable_peer_lands_in_unreachable_not_mounted() {
        let (_d, persist) = temp_persist();
        let backend = FakeBackend::new(vec![Err(MountError::Unreachable("offline".into()))]);
        let mut w = worker_with(backend);
        w.handle_verb(&persist, "gone", MeshMountVerb::Mount);
        assert_eq!(w.entries["gone"].phase, Phase::Unreachable);
        assert!(w.entries["gone"].reason.is_some());
    }

    #[test]
    fn transient_failure_schedules_a_backoff_retry() {
        let (_d, persist) = temp_persist();
        let backend = FakeBackend::new(vec![Err(MountError::Timeout)]);
        let mut w = worker_with(backend);
        w.handle_verb(&persist, "slow", MeshMountVerb::Mount);
        let e = &w.entries["slow"];
        assert_eq!(e.phase, Phase::Reconnecting);
        assert!(e.next_retry_at.is_some());
        assert_eq!(e.reconnect_attempt, 1);
    }

    #[test]
    fn stale_probe_recovers_a_frozen_mount() {
        let (_d, persist) = temp_persist();
        // mount ok, then the remount after recovery ok.
        let backend = FakeBackend::new(vec![Ok(()), Ok(())]);
        let mut w = worker_with(backend.clone()).with_idle_timeout(Duration::from_secs(3600)); // don't idle-unmount
        w.stale_after = Duration::from_millis(0); // any missed probe is stale
        w.handle_verb(&persist, "frozen", MeshMountVerb::Mount);
        assert_eq!(w.entries["frozen"].phase, Phase::Mounted);
        // freeze the mount + run a health tick → stale detected → recovery remount.
        *backend.probe_live.lock().unwrap() = false;
        w.health_tick(&persist);
        // Remount fires (unmount + a fresh mount) and, since the fake mount is Ok,
        // it comes back Mounted.
        assert!(backend.unmounts.load(Ordering::SeqCst) >= 1);
        assert_eq!(backend.mounts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn idle_mount_auto_unmounts() {
        let (_d, persist) = temp_persist();
        let backend = FakeBackend::new(vec![Ok(())]);
        let mut w = worker_with(backend.clone()); // idle_timeout = 0
        w.handle_verb(&persist, "oak", MeshMountVerb::Mount);
        assert_eq!(w.entries["oak"].phase, Phase::Mounted);
        w.health_tick(&persist);
        assert_eq!(w.entries["oak"].phase, Phase::Unmounted);
        assert!(backend.unmounts.load(Ordering::SeqCst) >= 1);
    }

    #[test]
    fn explicit_unmount_forgets_the_host() {
        let (_d, persist) = temp_persist();
        let backend = FakeBackend::new(vec![Ok(())]);
        let mut w = worker_with(backend);
        w.handle_verb(&persist, "oak", MeshMountVerb::Mount);
        w.handle_verb(&persist, "oak", MeshMountVerb::Unmount);
        assert!(!w.entries.contains_key("oak"));
    }
}
