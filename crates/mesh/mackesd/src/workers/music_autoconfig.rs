//! MEDIA-8 — Workstation music auto-config.
//!
//! A fresh Workstation should open `mde-music` and browse the mesh library with
//! ZERO manual connect. This worker is the Workstation half of MEDIA-8 and, for
//! WL-FUNC-014, the first controlled gateway credential materializer: it prefers
//! manually registered LAN AirSonic gateway sources from
//! `<host>/airsonic-gateway-registry.json`, resolves the selected source's sealed
//! `credential_ref`, and idempotently writes the desktop user's
//! `~/.local/share/mde/airsonic-creds.json` with the selected mesh gateway URL.
//! The replicated source record never carries username/password material; this
//! root-owned worker is the controlled boundary that resolves the secret-store
//! reference into the seated user's local client credentials. If a source exists
//! but its credential is absent or malformed, the worker reports that honestly
//! and does not publish or synthesize credentials.
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
use crate::ipc::secret_store::{repo_root, SecretStore};
use crate::mesh_media::{self, AirsonicGatewaySource, MediaServerHealth, MediaServerRecord};

/// 60 s reconcile tick — matches `app_sync` / `remmina_sync`. This picks up a
/// newly enrolled local-network Airsonic source without a tight loop.
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

const MAX_MEDIA_RECORD_BYTES: u64 = 64 * 1024;
const MEDIA_REGISTRY_FILE: &str = "media-registry.json";

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

/// The plaintext body stored under an AirSonic gateway source's sealed
/// `credential_ref`. The registry carries the source URL; the secret carries
/// only the read-only Subsonic auth pair, so a compromised or stale secret cannot
/// override the selected gateway proxy back to a direct LAN URL or `music.mesh`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SealedAirsonicCreds {
    /// Subsonic read-only username.
    username: String,
    /// Subsonic password. Empty is permitted for open Subsonic-compatible
    /// servers, matching `mde-musicd::creds::is_valid`.
    password: String,
}

/// The materialized `mde-musicd::creds::Creds` body selected from one gateway
/// source and one decrypted credential body.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MaterializedGatewayCreds {
    /// Pretty-printed `airsonic-creds.json` body.
    body: String,
    /// Mesh proxy URL that the client will dial; used in write logs.
    source_url: String,
}

/// Build the desktop user's `airsonic-creds.json` from a gateway source and a
/// decrypted sealed credential body. The source owns the server URL; the secret
/// owns only username/password.
fn gateway_creds_json(source: &AirsonicGatewaySource, secret_body: &str) -> Result<String, String> {
    let sealed: SealedAirsonicCreds = serde_json::from_str(secret_body)
        .map_err(|e| format!("parse sealed AirSonic credential: {e}"))?;
    let username = sealed.username.trim();
    if username.is_empty() || username != sealed.username {
        return Err(
            "sealed AirSonic credential username must be non-empty and trimmed".to_string(),
        );
    }
    if source.source_url == mesh_media::music_mesh_server_url() {
        return Err("AirSonic gateway source resolved to legacy music.mesh URL".to_string());
    }
    let v = serde_json::json!({
        "server_url": source.source_url.as_str(),
        "username": username,
        "password": sealed.password,
    });
    serde_json::to_string_pretty(&v).map_err(|e| format!("serialize gateway creds: {e}"))
}

/// Read only the new credential-free Media server record shape. Legacy
/// `MediaRegistration` files remain the compatibility gateway path and are
/// deliberately not treated as versioned operator records here.
fn read_media_server_records_from_plane(workgroup_root: &Path) -> Vec<MediaServerRecord> {
    let Ok(entries) = std::fs::read_dir(workgroup_root) else {
        return Vec::new();
    };
    let mut records = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path().join(MEDIA_REGISTRY_FILE);
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        if !metadata.is_file() || metadata.len() > MAX_MEDIA_RECORD_BYTES {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(path) else {
            continue;
        };
        if let Ok(record) = serde_json::from_str::<MediaServerRecord>(&body) {
            if let Some(record) = record.validated() {
                records.push(record);
            }
        } else if let Ok(roster) = serde_json::from_str::<Vec<MediaServerRecord>>(&body) {
            records.extend(roster.into_iter().filter_map(|record| record.validated()));
        }
    }
    records
}

fn media_health_rank(health: MediaServerHealth) -> u8 {
    match health {
        MediaServerHealth::Healthy => 0,
        MediaServerHealth::Degraded => 1,
        MediaServerHealth::Unavailable => 2,
    }
}

fn media_server_creds_json(
    record: &MediaServerRecord,
    secret_body: &str,
) -> Result<String, String> {
    let sealed: SealedAirsonicCreds = serde_json::from_str(secret_body)
        .map_err(|e| format!("parse sealed Media credential: {e}"))?;
    let username = sealed.username.trim();
    if username.is_empty() || username != sealed.username {
        return Err("sealed Media credential username must be non-empty and trimmed".to_string());
    }
    let value = serde_json::json!({
        "server_url": record.endpoint,
        "username": username,
        "password": sealed.password,
    });
    serde_json::to_string_pretty(&value).map_err(|e| format!("serialize Media creds: {e}"))
}

fn materialized_media_server_creds(
    records: &[MediaServerRecord],
    mut read_secret: impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<Option<MaterializedGatewayCreds>, String> {
    let Some(record) = records
        .iter()
        .filter(|record| !record.credential_ref.is_empty())
        .min_by_key(|record| {
            (
                media_health_rank(record.health),
                record.priority,
                record.latency.unwrap_or(u32::MAX),
                record.endpoint.as_str(),
            )
        })
    else {
        return Ok(None);
    };
    let secret_body = read_secret(&record.credential_ref)
        .map_err(|e| {
            format!(
                "read sealed Media credential {}: {e}",
                record.credential_ref
            )
        })?
        .ok_or_else(|| {
            format!(
                "sealed Media credential {} is absent",
                record.credential_ref
            )
        })?;
    let body = media_server_creds_json(record, &secret_body)?;
    Ok(Some(MaterializedGatewayCreds {
        body,
        source_url: record.endpoint.clone(),
    }))
}

/// Select the best gateway source for this tick and resolve its sealed
/// credential. `read_secret` is injected so tests can prove the failover and
/// no-plaintext contract without depending on a live secret backend.
fn materialized_gateway_creds(
    sources: &[AirsonicGatewaySource],
    last_selected: Option<&str>,
    mut read_secret: impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<Option<MaterializedGatewayCreds>, String> {
    let Some(source) = mesh_media::select_airsonic_gateway_source(sources, last_selected) else {
        return Ok(None);
    };
    let secret_body = read_secret(&source.credential_ref)
        .map_err(|e| {
            format!(
                "read sealed AirSonic credential {}: {e}",
                source.credential_ref
            )
        })?
        .ok_or_else(|| {
            format!(
                "sealed AirSonic credential {} is absent for source {}",
                source.credential_ref, source.id
            )
        })?;
    let body = gateway_creds_json(source, &secret_body)?;
    Ok(Some(MaterializedGatewayCreds {
        body,
        source_url: source.source_url.clone(),
    }))
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

/// Reconcile an already-built `mde-musicd::creds::Creds` JSON body into the
/// seated user's home while preserving the existing no-clobber marker discipline.
fn reconcile_desired(
    home: &Path,
    desired: Option<&str>,
    owner: Option<(u32, u32)>,
    server: &str,
) -> bool {
    let Some(desired) = desired else {
        return false;
    };
    let creds_path = home.join(CREDS_REL_PATH);
    let marker_path = home.join(MARKER_REL_PATH);
    let current = std::fs::read_to_string(&creds_path).ok();
    let marker = std::fs::read_to_string(&marker_path).ok();

    match decide(desired, current.as_deref(), marker.as_deref()) {
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
                server = %server,
                "auto-configured mde-music credentials"
            );
            true
        }
    }
}

/// Reconcile AirSonic gateway sources into the desktop user's client creds.
/// Presence of any gateway source means WL-FUNC-014 owns this tick: missing or
/// malformed sealed credentials are surfaced and do not synthesize a fallback
/// account.
fn reconcile_gateway_sources(
    home: &Path,
    sources: &[AirsonicGatewaySource],
    owner: Option<(u32, u32)>,
    read_secret: impl FnMut(&str) -> Result<Option<String>, String>,
) -> bool {
    match materialized_gateway_creds(sources, None, read_secret) {
        Ok(Some(materialized)) => reconcile_desired(
            home,
            Some(&materialized.body),
            owner,
            &materialized.source_url,
        ),
        Ok(None) => false,
        Err(e) => {
            tracing::warn!(
                target: "mackesd::music_autoconfig",
                error = %e,
                "AirSonic gateway credential materialization pending"
            );
            false
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

/// Full cycle: resolve the desktop user + the selected replicated Airsonic
/// source, resolve its secret-store reference, and reconcile the local creds
/// file. No-op when there's no desktop user or no source is published yet.
fn run_once(workgroup_root: &Path) -> bool {
    let Some((uid, gid, home)) = desktop_user() else {
        return false;
    };
    let store = SecretStore::resolve(&repo_root(), workgroup_root);
    let records = read_media_server_records_from_plane(workgroup_root);
    if !records.is_empty() {
        return match materialized_media_server_creds(&records, |cred_ref| store.get(cred_ref)) {
            Ok(Some(materialized)) => reconcile_desired(
                &home,
                Some(&materialized.body),
                Some((uid, gid)),
                &materialized.source_url,
            ),
            Ok(None) => false,
            Err(e) => {
                tracing::warn!(
                    target: "mackesd::music_autoconfig",
                    error = %e,
                    "Media server credential materialization pending"
                );
                false
            }
        };
    }
    let sources = mesh_media::read_airsonic_gateway_sources_from_plane(workgroup_root);
    reconcile_gateway_sources(&home, &sources, Some((uid, gid)), |cred_ref| {
        store.get(cred_ref)
    })
}

/// Workstation music auto-config worker. Each tick reads Airsonic server records
/// off the replicated registry plane, resolves their secret references, and
/// idempotently writes the desktop user's local creds.
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

    fn gateway_source(
        node: &str,
        upstream: &str,
        credential_ref: &str,
        health: mesh_media::GatewayHealth,
        mesh_default: bool,
    ) -> AirsonicGatewaySource {
        let reg = mesh_media::AirsonicGatewayRegistration::new(
            node,
            upstream,
            credential_ref,
            health,
            mesh_default,
        )
        .unwrap();
        mesh_media::source_from_airsonic_gateway(&reg).unwrap()
    }

    fn sealed_airsonic_creds(user: &str, pass: &str) -> String {
        serde_json::json!({
            "username": user,
            "password": pass,
        })
        .to_string()
    }

    fn media_record(
        endpoint: &str,
        priority: u16,
        health: MediaServerHealth,
        reference: &str,
    ) -> MediaServerRecord {
        MediaServerRecord::new(
            endpoint,
            mesh_media::MediaServerKind::Airsonic,
            priority,
            health,
            Some(20),
            reference,
        )
        .unwrap()
    }

    #[test]
    fn media_records_are_read_from_the_shared_plane_and_ranked() {
        let root = tmp_home("media-records");
        std::fs::create_dir_all(root.join("media-a")).unwrap();
        std::fs::write(
            root.join("media-a").join(MEDIA_REGISTRY_FILE),
            serde_json::to_string(&media_record(
                "http://media-a.lan:4040",
                20,
                MediaServerHealth::Healthy,
                "media/airsonic/a",
            ))
            .unwrap(),
        )
        .unwrap();
        std::fs::create_dir_all(root.join("media-b")).unwrap();
        std::fs::write(
            root.join("media-b").join(MEDIA_REGISTRY_FILE),
            serde_json::to_string(&media_record(
                "http://media-b.lan:4040",
                1,
                MediaServerHealth::Degraded,
                "media/airsonic/b",
            ))
            .unwrap(),
        )
        .unwrap();

        let records = read_media_server_records_from_plane(&root);
        let materialized = materialized_media_server_creds(&records, |reference| {
            assert_eq!(reference, "media/airsonic/a");
            Ok(Some(sealed_airsonic_creds("mesh-readonly", "hunter2")))
        })
        .unwrap()
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&materialized.body).unwrap();
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(value["server_url"], "http://media-a.lan:4040");
        assert_eq!(value["username"], "mesh-readonly");
        assert!(!materialized.body.contains("credential_ref"));
    }

    #[test]
    fn media_record_without_secret_does_not_materialize_or_fallback() {
        let record = media_record(
            "http://media-a.lan:4040",
            1,
            MediaServerHealth::Healthy,
            "media/airsonic/a",
        );
        let result = materialized_media_server_creds(&[record], |_reference| Ok(None));
        assert!(result.is_err());
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
    fn gateway_creds_json_uses_proxy_url_and_sealed_pair_only() {
        let source = gateway_source(
            "gateway-a",
            "http://nas.lan:4040",
            "media/airsonic/shared-readonly",
            mesh_media::GatewayHealth::Healthy,
            true,
        );

        let json = gateway_creds_json(&source, &sealed_airsonic_creds("mesh-readonly", "hunter2"))
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(v["server_url"], source.source_url);
        assert_eq!(v["username"], "mesh-readonly");
        assert_eq!(v["password"], "hunter2");
        assert_eq!(v.as_object().unwrap().len(), 3);
        assert!(
            !json.contains("nas.lan"),
            "client creds must not expose the LAN upstream URL"
        );
        assert!(
            !json.contains("credential_ref"),
            "client creds must not embed the sealed reference"
        );
        assert_ne!(v["server_url"], mesh_media::music_mesh_server_url());
    }

    #[test]
    fn gateway_creds_json_rejects_secret_url_override_and_blank_user() {
        let source = gateway_source(
            "gateway-a",
            "http://nas.lan:4040",
            "media/airsonic/shared-readonly",
            mesh_media::GatewayHealth::Healthy,
            true,
        );

        let override_err = gateway_creds_json(
            &source,
            r#"{"username":"mesh-readonly","password":"hunter2","server_url":"http://music.mesh:4533"}"#,
        )
        .unwrap_err();
        assert!(
            override_err.contains("unknown field"),
            "secret body must not be allowed to override source_url: {override_err}"
        );

        let blank_err =
            gateway_creds_json(&source, &sealed_airsonic_creds(" mesh ", "hunter2")).unwrap_err();
        assert!(blank_err.contains("username"));
    }

    #[test]
    fn materialized_gateway_creds_fails_over_to_healthy_source_before_secret_read() {
        let degraded_default = gateway_source(
            "gateway-degraded",
            "http://default.lan:4040",
            "media/airsonic/degraded-default",
            mesh_media::GatewayHealth::Degraded,
            true,
        );
        let healthy = gateway_source(
            "gateway-healthy",
            "http://healthy.lan:4040",
            "media/airsonic/healthy",
            mesh_media::GatewayHealth::Healthy,
            false,
        );

        let materialized = materialized_gateway_creds(
            &[degraded_default, healthy.clone()],
            None,
            |credential_ref| {
                assert_eq!(
                    credential_ref, healthy.credential_ref,
                    "a healthy source should win over a degraded default"
                );
                Ok(Some(sealed_airsonic_creds("mesh-readonly", "hunter2")))
            },
        )
        .unwrap()
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&materialized.body).unwrap();

        assert_eq!(v["server_url"], healthy.source_url);
        assert_eq!(materialized.source_url, healthy.source_url);
    }

    #[test]
    fn reconcile_gateway_sources_writes_proxy_creds_and_marker() {
        let home = tmp_home("gateway-write");
        let source = gateway_source(
            "gateway-a",
            "http://nas.lan:4040",
            "media/airsonic/shared-readonly",
            mesh_media::GatewayHealth::Healthy,
            true,
        );

        let wrote = reconcile_gateway_sources(&home, &[source.clone()], None, |credential_ref| {
            assert_eq!(credential_ref, source.credential_ref);
            Ok(Some(sealed_airsonic_creds("mesh-readonly", "hunter2")))
        });
        let creds = std::fs::read_to_string(home.join(CREDS_REL_PATH)).unwrap();
        let marker = std::fs::read_to_string(home.join(MARKER_REL_PATH)).unwrap();
        let _ = std::fs::remove_dir_all(&home);
        let v: serde_json::Value = serde_json::from_str(&creds).unwrap();

        assert!(wrote, "gateway source materialization should write creds");
        assert_eq!(v["server_url"], source.source_url);
        assert_eq!(v["username"], "mesh-readonly");
        assert_eq!(v["password"], "hunter2");
        assert_eq!(marker, creds, "marker records the worker-owned write");
        assert_ne!(v["server_url"], mesh_media::music_mesh_server_url());
    }

    #[test]
    fn reconcile_gateway_sources_waits_when_secret_is_absent() {
        let home = tmp_home("gateway-pending");
        let source = gateway_source(
            "gateway-a",
            "http://nas.lan:4040",
            "media/airsonic/shared-readonly",
            mesh_media::GatewayHealth::Healthy,
            true,
        );

        let wrote = reconcile_gateway_sources(&home, &[source], None, |_credential_ref| Ok(None));
        let exists = home.join(CREDS_REL_PATH).exists();
        let _ = std::fs::remove_dir_all(&home);

        assert!(!wrote, "pending sealed credential must not fake a write");
        assert!(
            !exists,
            "legacy music.mesh fallback must not be materialized"
        );
    }

    // ── the pure write decision (no-clobber + idempotent) ──

    #[test]
    fn decide_writes_when_absent() {
        assert_eq!(
            decide("desired", None, None),
            WriteDecision::Write("desired".to_string())
        );
    }

    #[test]
    fn decide_skips_when_already_configured() {
        assert_eq!(
            decide("desired", Some("desired"), None),
            WriteDecision::Skip
        );
    }

    #[test]
    fn decide_does_not_clobber_a_user_set_file() {
        assert_eq!(
            decide("desired", Some("user-set"), None),
            WriteDecision::Skip
        );
        assert_eq!(
            decide("desired", Some("user-set"), Some("stale")),
            WriteDecision::Skip
        );
    }

    #[test]
    fn decide_refreshes_a_file_we_own_when_creds_rotate() {
        assert_eq!(
            decide("new", Some("old"), Some("old")),
            WriteDecision::Write("new".to_string())
        );
    }
}
