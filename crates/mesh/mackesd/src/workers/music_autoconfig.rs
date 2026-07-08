//! MEDIA-8 — Workstation music auto-config.
//!
//! A fresh Workstation should open `mde-music` and browse the mesh library with
//! ZERO manual connect. This worker is the Workstation half of MEDIA-8: it reads
//! the published mesh media service (the `shared_account` MEDIA-7's
//! `media_registry` worker now puts in `mesh/services/media/<peer>` + the
//! replicated QNM-Shared `<host>/media-registry.json` plane) and idempotently
//! writes the desktop user's `~/.local/share/mde/airsonic-creds.json`, so the
//! player auto-browses instead of showing the first-run connect form.
//!
//! **No mesh age key on Workstations** (binding operator decision): the shared
//! account flows through the SERVICE REGISTRY, not the secret store. A Workstation
//! never reads/holds the `media-spaces` secret — only a Lighthouse_Media node does
//! (the publish side). This worker just reads the already-published account off
//! the replicated plane, exactly as `app_sync` / `apps::fleet_*` read their
//! planes.
//!
//! **Writes to the DESKTOP user's home, not root's.** `mackesd` runs as root, so
//! `$HOME` is `/root` — useless to the seated user's `mde-music`. The worker
//! resolves the uid-1000 desktop user's home from `/etc/passwd` (the same
//! `clipboard_sync` / `ssh_pubkey_gossip` discipline) and writes there, chowned
//! to that user so the player can read it.
//!
//! **Never clobbers a user-set file.** It writes only when the creds file is
//! ABSENT, or when it still matches what THIS worker last auto-wrote (tracked via
//! a sidecar marker). The moment the user edits creds (via the Workbench Music
//! panel or by hand), the live file diverges from the marker and the worker backs
//! off — the user's choice wins.
//!
//! Role-gated to the Workstation tier (rank 1) like the other desktop workers
//! (`remmina-sync`, `clipboard_sync`): a headless Lighthouse/Server has no seated
//! user to configure, so the worker isn't spawned there.

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;

use super::{ShutdownToken, Worker};
use crate::mesh_media::{self, SharedAccount};

/// 60 s reconcile tick — matches `app_sync` / `remmina_sync`. The published
/// account is slow-changing (a shared mesh credential); 60 s picks up a newly
/// serving Lighthouse_Media node well within a fresh-node bring-up without a
/// tight loop.
pub const TICK_INTERVAL_S: u64 = 60;

/// The lowest "real" (non-system) uid — the seated desktop user on a Workstation.
/// Matches `clipboard_sync::session`'s `REGULAR_UID_MIN`.
const DESKTOP_UID: u32 = 1000;

/// Creds file path relative to the desktop user's `$HOME`. Mirrors
/// `mde_musicd::creds::CREDS_REL_PATH` byte-for-byte (pinned by a test) — the
/// path is the contract, but the mesh daemon must NOT depend on the GUI/daemon
/// `mde-musicd` crate (it pulls reqwest + the audio stack), so it's repeated
/// here instead.
const CREDS_REL_PATH: &str = ".local/share/mde/airsonic-creds.json";

/// Sidecar marker recording the exact JSON this worker last auto-wrote, so a
/// later tick can tell "we own this file" (live == marker) from "the user
/// changed it" (live != marker) and back off in the latter case. Lives beside
/// the creds file.
const MARKER_REL_PATH: &str = ".local/share/mde/.airsonic-creds.auto";

/// The uid-1000 desktop user's `(uid, gid, home)` from `/etc/passwd`. `None` on a
/// headless box with no seated user (the worker then no-ops — nothing to
/// configure). Pure over the passwd content via [`parse_desktop_user`]; this is
/// the thin I/O wrapper.
fn desktop_user() -> Option<(u32, u32, PathBuf)> {
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    parse_desktop_user(&passwd, DESKTOP_UID)
}

/// Parse `/etc/passwd` content for `uid`'s `(uid, gid, home)`. Pure — the
/// `name:passwd:uid:gid:gecos:home:shell` colon format, skipping malformed
/// lines. `None` when the uid isn't present.
fn parse_desktop_user(passwd: &str, uid: u32) -> Option<(u32, u32, PathBuf)> {
    for line in passwd.lines() {
        let f: Vec<&str> = line.split(':').collect();
        if f.len() < 7 {
            continue;
        }
        let (Ok(row_uid), Ok(gid)) = (f[2].parse::<u32>(), f[3].parse::<u32>()) else {
            continue;
        };
        if row_uid == uid {
            return Some((row_uid, gid, PathBuf::from(f[5])));
        }
    }
    None
}

/// The on-disk creds JSON for a shared account, in the EXACT shape
/// `mde-musicd::creds::Creds` (de)serializes (`server_url` / `username` /
/// `password`, pretty-printed) — so `mde-music` reads it back as valid creds and
/// auto-browses. Built here (rather than depending on the GUI crate's writer) to
/// keep the mesh worker decoupled from the desktop binary; the field names are
/// the contract, pinned by a test.
fn creds_json(account: &SharedAccount) -> String {
    // serde_json::json! keeps the exact `Creds` field names + a stable order.
    let v = serde_json::json!({
        "server_url": account.server,
        "username": account.username,
        "password": account.password,
    });
    serde_json::to_string_pretty(&v).expect("creds JSON is plain")
}

/// What the worker should do with the creds file this tick — computed PURELY so
/// the no-clobber + idempotency decision is unit-tested apart from any I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
enum WriteDecision {
    /// Write the new creds (file absent, or it still matches our last marker so
    /// we own it and the desired content changed). Carries the JSON to write.
    Write(String),
    /// Leave the file alone — either it already matches the desired creds
    /// (idempotent no-op) or the user set/edited it (live != marker).
    Skip,
}

/// Decide whether to (over)write the creds file, given the desired account and
/// the current on-disk state:
///   * `current`  — the live creds file body (`None` = absent).
///   * `marker`   — what we last auto-wrote (`None` = we never wrote it).
///
/// Rules (no-clobber + idempotent):
///   * absent           → write (first auto-config),
///   * live == desired  → skip (already configured; nothing to do),
///   * live == marker    → write (WE wrote it last + the desired creds changed,
///                         e.g. the shared password rotated — safe to update),
///   * else             → skip (the USER set/changed it — their choice wins).
fn decide(desired: &str, current: Option<&str>, marker: Option<&str>) -> WriteDecision {
    match current {
        // No creds yet → auto-configure.
        None => WriteDecision::Write(desired.to_owned()),
        Some(live) if live == desired => WriteDecision::Skip,
        // We own the file (it's byte-identical to our last write) and the
        // desired creds changed → refresh it.
        Some(live) if marker == Some(live) => WriteDecision::Write(desired.to_owned()),
        // The file diverged from our marker → the user owns it. Back off.
        Some(_) => WriteDecision::Skip,
    }
}

/// One reconcile cycle against `home` (the desktop user's home) + the discovered
/// `account` (`None` = no service published yet → nothing to do). `owner` chowns
/// the written files to the desktop user. Returns whether a write happened
/// (for the worker's log line + the tests). Best-effort: an I/O error is logged,
/// never fatal.
fn reconcile(home: &Path, account: Option<&SharedAccount>, owner: Option<(u32, u32)>) -> bool {
    let Some(account) = account else {
        return false;
    };
    let creds_path = home.join(CREDS_REL_PATH);
    let marker_path = home.join(MARKER_REL_PATH);
    let desired = creds_json(account);
    let current = std::fs::read_to_string(&creds_path).ok();
    let marker = std::fs::read_to_string(&marker_path).ok();

    match decide(&desired, current.as_deref(), marker.as_deref()) {
        WriteDecision::Skip => false,
        WriteDecision::Write(body) => {
            if let Some(parent) = creds_path.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    tracing::warn!(target: "mackesd::music_autoconfig", error = %e, "mkdir failed");
                    return false;
                }
            }
            // Write the creds, then the marker recording exactly what we wrote
            // (so the next tick recognizes the file as ours).
            if let Err(e) = std::fs::write(&creds_path, &body) {
                tracing::warn!(target: "mackesd::music_autoconfig", path = %creds_path.display(), error = %e, "write creds failed");
                return false;
            }
            let _ = std::fs::write(&marker_path, &body);
            chown_owned(&creds_path, owner);
            chown_owned(&marker_path, owner);
            tracing::info!(
                target: "mackesd::music_autoconfig",
                server = %account.server,
                "auto-configured mde-music with the mesh shared account"
            );
            true
        }
    }
}

/// Best-effort chown to the desktop user so the seated user's `mde-music` can
/// read the file `mackesd` (root) wrote. A no-op when `owner` is unknown (test
/// paths / a box where the uid didn't resolve).
fn chown_owned(path: &Path, owner: Option<(u32, u32)>) {
    #[cfg(unix)]
    if let Some((uid, gid)) = owner {
        use std::os::unix::fs::chown;
        let _ = chown(path, Some(uid), Some(gid));
    }
    #[cfg(not(unix))]
    let _ = (path, owner);
}

/// Full cycle: resolve the desktop user + the published shared account, then
/// reconcile the creds file. No-op (logs nothing) when there's no desktop user
/// or no service is published yet.
fn run_once(workgroup_root: &Path) -> bool {
    let Some((uid, gid, home)) = desktop_user() else {
        return false;
    };
    let account = mesh_media::read_shared_account_from_plane(workgroup_root);
    reconcile(&home, account.as_ref(), Some((uid, gid)))
}

/// Workstation music auto-config worker. 60 s tick; each tick reads the published
/// shared account off the replicated registry plane and idempotently writes the
/// desktop user's creds.
pub struct MusicAutoconfigWorker {
    workgroup_root: PathBuf,
    tick: Duration,
}

impl MusicAutoconfigWorker {
    /// Construct with production defaults (the replicated QNM-Shared root).
    #[must_use]
    pub fn new() -> Self {
        Self {
            workgroup_root: crate::default_qnm_shared_root(),
            tick: Duration::from_secs(TICK_INTERVAL_S),
        }
    }

    /// Override the registry-plane root (honors `--workgroup-root` at the spawn
    /// site so the worker reads where the registry writers write).
    #[must_use]
    pub fn with_workgroup_root(mut self, p: PathBuf) -> Self {
        self.workgroup_root = p;
        self
    }
}

impl Default for MusicAutoconfigWorker {
    fn default() -> Self {
        Self::new()
    }
}

/// Construct the default-configured worker for the supervisor.
#[must_use]
pub fn build() -> MusicAutoconfigWorker {
    MusicAutoconfigWorker::new()
}

#[async_trait]
impl Worker for MusicAutoconfigWorker {
    fn name(&self) -> &'static str {
        "music_autoconfig"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        run_once(&self.workgroup_root);
        loop {
            tokio::select! {
                _ = shutdown.wait() => return Ok(()),
                _ = tokio::time::sleep(self.tick) => {
                    run_once(&self.workgroup_root);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_home(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("mde-musicac-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn acct(user: &str, pass: &str) -> SharedAccount {
        SharedAccount::new(user, pass)
    }

    #[test]
    fn worker_name_is_music_autoconfig() {
        assert_eq!(build().name(), "music_autoconfig");
    }

    #[test]
    fn parse_desktop_user_finds_uid_1000() {
        let passwd = "root:x:0:0:root:/root:/bin/bash\n\
                      mm:x:1000:1000:Matthew:/home/mm:/bin/bash\n";
        let (uid, gid, home) = parse_desktop_user(passwd, 1000).unwrap();
        assert_eq!(uid, 1000);
        assert_eq!(gid, 1000);
        assert_eq!(home, PathBuf::from("/home/mm"));
        // A box with no uid-1000 entry → None (headless; nothing to configure).
        assert_eq!(
            parse_desktop_user("root:x:0:0::/root:/bin/sh\n", 1000),
            None
        );
    }

    #[test]
    fn creds_json_is_the_mde_musicd_creds_shape() {
        // The written JSON must carry EXACTLY the `mde_musicd::creds::Creds`
        // field names (`server_url`/`username`/`password`) or mde-music wouldn't
        // read it as valid creds (it'd show the first-run form). We pin the wire
        // shape here rather than depend on the GUI crate (it pulls reqwest).
        let json = creds_json(&acct("mesh-music", "hunter2"));
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["server_url"], "http://music.mesh:4533");
        assert_eq!(v["username"], "mesh-music");
        assert_eq!(v["password"], "hunter2");
        // No extra/renamed fields that a strict Creds deserialize would reject.
        assert_eq!(v.as_object().unwrap().len(), 3);
    }

    // ── the pure write decision (no-clobber + idempotent) ──

    #[test]
    fn decide_writes_when_absent() {
        let desired = creds_json(&acct("u", "p"));
        assert_eq!(
            decide(&desired, None, None),
            WriteDecision::Write(desired.clone())
        );
    }

    #[test]
    fn decide_skips_when_already_configured() {
        let desired = creds_json(&acct("u", "p"));
        // Live file already equals the desired creds → idempotent no-op.
        assert_eq!(decide(&desired, Some(&desired), None), WriteDecision::Skip);
    }

    #[test]
    fn decide_does_not_clobber_a_user_set_file() {
        let desired = creds_json(&acct("mesh-music", "p"));
        let user_set = creds_json(&acct("my-own-server-user", "myp"));
        // The user set their own creds (no marker, or a stale marker) → back off.
        assert_eq!(decide(&desired, Some(&user_set), None), WriteDecision::Skip);
        // Even with a marker, if the live file diverged from it the user owns it.
        let stale_marker = creds_json(&acct("mesh-music", "old"));
        assert_eq!(
            decide(&desired, Some(&user_set), Some(&stale_marker)),
            WriteDecision::Skip
        );
    }

    #[test]
    fn decide_refreshes_a_file_we_own_when_creds_rotate() {
        // We last wrote `old`; the shared password rotated to `new`. The live
        // file still matches our marker (we own it) → safe to update.
        let old = creds_json(&acct("mesh-music", "old"));
        let new = creds_json(&acct("mesh-music", "new"));
        assert_eq!(
            decide(&new, Some(&old), Some(&old)),
            WriteDecision::Write(new.clone())
        );
    }

    // ── reconcile end-to-end against a temp home ──

    #[test]
    fn reconcile_writes_creds_and_marker_when_account_appears() {
        let home = tmp_home("write");
        let wrote = reconcile(&home, Some(&acct("mesh-music", "p")), None);
        let creds = std::fs::read_to_string(home.join(CREDS_REL_PATH)).unwrap();
        let marker_exists = home.join(MARKER_REL_PATH).is_file();
        let _ = std::fs::remove_dir_all(&home);
        assert!(wrote, "first appearance auto-configures");
        assert!(marker_exists, "marker recorded so we know we own the file");
        let v: serde_json::Value = serde_json::from_str(&creds).unwrap();
        assert_eq!(v["server_url"], "http://music.mesh:4533");
        assert_eq!(v["username"], "mesh-music");
    }

    #[test]
    fn reconcile_is_idempotent_across_ticks() {
        let home = tmp_home("idem");
        let a = acct("mesh-music", "p");
        assert!(reconcile(&home, Some(&a), None), "first tick writes");
        // Second tick: nothing changed → no write.
        assert!(
            !reconcile(&home, Some(&a), None),
            "second tick is a no-op (already configured)"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn reconcile_does_not_overwrite_user_set_creds() {
        let home = tmp_home("userset");
        // Simulate the user having set their own creds via the Workbench panel:
        // a creds file with NO marker (the worker never wrote it).
        let creds_path = home.join(CREDS_REL_PATH);
        std::fs::create_dir_all(creds_path.parent().unwrap()).unwrap();
        let user_body = creds_json(&acct("my-server", "mine"));
        std::fs::write(&creds_path, &user_body).unwrap();
        let wrote = reconcile(&home, Some(&acct("mesh-music", "p")), None);
        let after = std::fs::read_to_string(&creds_path).unwrap();
        let _ = std::fs::remove_dir_all(&home);
        assert!(!wrote, "must not clobber the user's file");
        assert_eq!(after, user_body, "user's creds left intact");
    }

    #[test]
    fn reconcile_noop_when_no_account_published() {
        let home = tmp_home("noacct");
        let wrote = reconcile(&home, None, None);
        let exists = home.join(CREDS_REL_PATH).exists();
        let _ = std::fs::remove_dir_all(&home);
        assert!(!wrote);
        assert!(!exists, "no service published → no creds written");
    }
}
