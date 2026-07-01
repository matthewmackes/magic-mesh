//! App-config sync worker — keeps mesh-media client configs in sync.
//!
//! EPIC-SYNC-APP-CONFIG (Q26) — the native-Rust replacement for
//! `mackes/media_sync_daemon.py` (+ its `mesh_media.py` discovery
//! dep), retiring the last `python3 -m mackes.<daemon>` subprocess
//! the supervisor drove for this feature (advances §11 #6 "every
//! Python daemon ported to Rust"). Discovery lives in
//! [`crate::mesh_media`]; this module owns credential loading, the
//! per-app config writers, the `~/Mackes Media/` launcher view, the
//! GTK bookmark, and the 60 s supervisor tick.
//!
//! **Plugin model** (Q26 "extensible"): each client app implements
//! [`MediaApp`] — its server-kind, config path, byte-faithful config
//! renderer, and `.desktop` launcher. Adding a new app = one more
//! `impl MediaApp` in [`apps()`]. Sublime Music (Airsonic) + Delfin
//! (Jellyfin) ship out of the box.
//!
//! Config JSON is byte-faithful to the retired Python's
//! `json.dumps(..., indent=2)` (serde's `to_string_pretty` matches
//! the 2-space pretty form for the ASCII content these configs hold),
//! so a peer that upgrades sees no spurious config churn.

#![cfg(feature = "async-services")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::{ShutdownToken, Worker};
use crate::mesh_media::{self, MediaServer, KIND_AIRSONIC, KIND_JELLYFIN};

/// Cadence locked at 60 s — matches the retired
/// `mackes-media-sync.timer` `OnUnitActiveSec=60s` so behavior is
/// continuous across the port.
pub const TICK_INTERVAL_S: u64 = 60;

/// Per-host credential fields, keyed by host. Mirrors the Python
/// `{"<host>": {"user": "...", ...}}` schema.
type HostCreds = BTreeMap<String, BTreeMap<String, String>>;

/// QNM-Shared media credentials. Missing file / parse error → empty
/// (clients render their own login prompt), matching the Python.
#[derive(Debug, Default, Deserialize)]
struct Credentials {
    #[serde(default)]
    airsonic: HostCreds,
    #[serde(default)]
    jellyfin: HostCreds,
}

impl Credentials {
    fn for_kind(&self, kind: &str) -> &HostCreds {
        match kind {
            KIND_AIRSONIC => &self.airsonic,
            _ => &self.jellyfin,
        }
    }
}

fn load_credentials(path: &Path) -> Credentials {
    let Ok(bytes) = std::fs::read(path) else {
        return Credentials::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        tracing::warn!(target: "mackesd::app_sync", error = %e, "could not parse media credentials; ignoring");
        Credentials::default()
    })
}

/// Look up `field` for `host`, returning `""` when absent (Python
/// `creds.get(host, {}).get(field, "")`).
fn cred_field<'a>(creds: &'a HostCreds, host: &str, field: &str) -> &'a str {
    creds
        .get(host)
        .and_then(|c| c.get(field))
        .map_or("", String::as_str)
}

// ── Per-app config wire shapes (field order = Python dict order, so
//    serde_json::to_string_pretty is byte-faithful) ───────────────────

#[derive(Serialize)]
struct SublimeProvider {
    name: String,
    server_address: String,
    username: String,
    password: String,
    sync_enabled: bool,
    verify_cert: bool,
}

#[derive(Serialize)]
struct SublimeConfig {
    providers: Vec<SublimeProvider>,
}

#[derive(Serialize)]
struct DelfinServer {
    name: String,
    address: String,
    user: String,
    access_token: String,
}

#[derive(Serialize)]
struct DelfinConfig {
    servers: Vec<DelfinServer>,
}

// ── Plugin model ─────────────────────────────────────────────────────

/// One media client app the worker keeps configured. Implementors are
/// stateless; [`apps`] returns the registry.
trait MediaApp: Send + Sync {
    /// The [`MediaServer`] kind this app consumes.
    fn kind(&self) -> &'static str;
    /// Absolute path of the app's config file (under `paths`).
    fn config_path(&self, paths: &Paths) -> PathBuf;
    /// Render the app's config JSON for its servers. Byte-faithful to
    /// the retired Python writer.
    fn render_config(&self, servers: &[&MediaServer], creds: &HostCreds) -> String;
    /// `(filename, body)` for the server's `~/Mackes Media/` launcher.
    fn desktop_entry(&self, server: &MediaServer) -> (String, String);
}

struct SublimeMusic;
struct Delfin;

impl MediaApp for SublimeMusic {
    fn kind(&self) -> &'static str {
        KIND_AIRSONIC
    }
    fn config_path(&self, paths: &Paths) -> PathBuf {
        paths.sublime.clone()
    }
    fn render_config(&self, servers: &[&MediaServer], creds: &HostCreds) -> String {
        let providers = servers
            .iter()
            .map(|s| SublimeProvider {
                name: format!("{} (mesh)", s.host),
                server_address: s.url(),
                username: cred_field(creds, &s.host, "user").to_owned(),
                password: cred_field(creds, &s.host, "password").to_owned(),
                sync_enabled: true,
                verify_cert: false,
            })
            .collect();
        serde_json::to_string_pretty(&SublimeConfig { providers })
            .expect("SublimeConfig is plain JSON")
    }
    fn desktop_entry(&self, server: &MediaServer) -> (String, String) {
        desktop_entry(
            &format!("Airsonic — {}", server.host),
            "flatpak run com.sublimemusic.SublimeMusic",
            "audio-x-generic",
            &server.url(),
        )
    }
}

impl MediaApp for Delfin {
    fn kind(&self) -> &'static str {
        KIND_JELLYFIN
    }
    fn config_path(&self, paths: &Paths) -> PathBuf {
        paths.delfin.clone()
    }
    fn render_config(&self, servers: &[&MediaServer], creds: &HostCreds) -> String {
        let out = servers
            .iter()
            .map(|s| DelfinServer {
                name: format!("{} (mesh)", s.host),
                address: s.url(),
                user: cred_field(creds, &s.host, "user").to_owned(),
                access_token: cred_field(creds, &s.host, "access_token").to_owned(),
            })
            .collect();
        serde_json::to_string_pretty(&DelfinConfig { servers: out })
            .expect("DelfinConfig is plain JSON")
    }
    fn desktop_entry(&self, server: &MediaServer) -> (String, String) {
        desktop_entry(
            &format!("Jellyfin — {}", server.host),
            "flatpak run app.drey.Delfin",
            "video-x-generic",
            &server.url(),
        )
    }
}

/// The out-of-box app registry. Extending = add an `impl MediaApp`
/// and one line here.
fn apps() -> Vec<Box<dyn MediaApp>> {
    vec![Box::new(SublimeMusic), Box::new(Delfin)]
}

/// Build a `.desktop` `(filename, body)` from a title + exec + icon.
/// Pure helper (no app-specific branching) — the per-app `exec`/icon
/// come from the [`MediaApp`]. Filename sanitisation matches the
/// Python (`" "→"_"`, `"/"→"_"`, `"—"→"-"`).
fn desktop_entry(title: &str, exec: &str, icon: &str, url: &str) -> (String, String) {
    let filename = format!(
        "{}.desktop",
        title.replace(' ', "_").replace('/', "_").replace('—', "-")
    );
    let body = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={title}\n\
         Comment=Mesh media server at {url}\n\
         Exec={exec}\n\
         Icon={icon}\n\
         Terminal=false\n\
         Categories=AudioVideo;\n"
    );
    (filename, body)
}

// ── Filesystem writers ───────────────────────────────────────────────

/// Atomic write (`tmp` + rename) at `mode`. Returns `false` on any
/// I/O error (logged), matching the Python's failure posture.
fn write_atomic(path: &Path, payload: &str, mode: u32) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Some(parent) = path.parent() else {
        return false;
    };
    if let Err(e) = std::fs::create_dir_all(parent) {
        tracing::warn!(target: "mackesd::app_sync", path = %path.display(), error = %e, "mkdir failed");
        return false;
    }
    let tmp = path.with_extension(format!(
        "{}tmp",
        path.extension()
            .map_or(String::new(), |e| format!("{}.", e.to_string_lossy()))
    ));
    if std::fs::write(&tmp, payload).is_err() {
        return false;
    }
    if std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode)).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        tracing::warn!(target: "mackesd::app_sync", path = %path.display(), error = %e, "rename failed");
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    true
}

/// Recreate `media_dir` to hold exactly the `expected` launchers
/// (filename → body), removing stale `*.desktop` files. Mirrors the
/// Python `_rebuild_thunar_view`.
fn rebuild_media_view(media_dir: &Path, expected: &BTreeMap<String, String>) {
    if let Err(e) = std::fs::create_dir_all(media_dir) {
        tracing::warn!(target: "mackesd::app_sync", dir = %media_dir.display(), error = %e, "could not create media dir");
        return;
    }
    for (name, body) in expected {
        let path = media_dir.join(name);
        if std::fs::read_to_string(&path).is_ok_and(|cur| cur == *body) {
            continue;
        }
        let _ = write_atomic(&path, body, 0o644);
    }
    if let Ok(entries) = std::fs::read_dir(media_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.ends_with(".desktop") && !expected.contains_key(name) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// Append the `file://<media_dir> Mackes Media` line to the GTK-3
/// bookmarks file if not already present. Mirrors the Python
/// `_ensure_bookmark`.
fn ensure_bookmark(bookmarks: &Path, media_dir: &Path) {
    let line = format!("file://{} Mackes Media", media_dir.display());
    let Some(parent) = bookmarks.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let existing = std::fs::read_to_string(bookmarks).unwrap_or_default();
    if existing.lines().any(|l| l == line) {
        return;
    }
    let mut out = existing.clone();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&line);
    out.push('\n');
    let _ = std::fs::write(bookmarks, out);
}

// ── Path set ─────────────────────────────────────────────────────────

/// Resolved output paths + the discovery root. Defaults are
/// `$HOME`-based (overridable in tests via [`Paths::under`]).
#[derive(Debug, Clone)]
struct Paths {
    workgroup_root: PathBuf,
    media_dir: PathBuf,
    sublime: PathBuf,
    delfin: PathBuf,
    bookmarks: PathBuf,
    creds: PathBuf,
}

impl Paths {
    /// Production defaults: `$HOME`-rooted client paths +
    /// `default_qnm_shared_root()` for discovery.
    fn defaults() -> Self {
        let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/root"), PathBuf::from);
        Self::under(&home, crate::default_qnm_shared_root())
    }

    /// All client paths rooted under `home`; discovery under
    /// `workgroup_root`. The seam tests use.
    fn under(home: &Path, workgroup_root: PathBuf) -> Self {
        Self {
            workgroup_root,
            media_dir: home.join("Mackes Media"),
            sublime: home.join(".config/sublime-music/config.json"),
            delfin: home.join(".local/share/Delfin/servers.json"),
            bookmarks: home.join(".config/gtk-3.0/bookmarks"),
            creds: home.join(".local/share/mackes/qnm-shared/mackes/media-credentials.json"),
        }
    }
}

// ── One sync cycle ───────────────────────────────────────────────────

/// One sync cycle against the given paths + discovered servers. Split
/// from discovery so tests drive it with a fixed server list (no
/// sockets). Returns the number of servers configured.
fn sync_servers(paths: &Paths, servers: &[MediaServer]) -> usize {
    let creds = load_credentials(&paths.creds);
    let registry = apps();

    for app in &registry {
        let app_servers: Vec<&MediaServer> =
            servers.iter().filter(|s| s.kind == app.kind()).collect();
        let payload = app.render_config(&app_servers, creds.for_kind(app.kind()));
        write_atomic(&app.config_path(paths), &payload, 0o600);
    }

    let mut expected: BTreeMap<String, String> = BTreeMap::new();
    for server in servers {
        if let Some(app) = registry.iter().find(|a| a.kind() == server.kind) {
            let (name, body) = app.desktop_entry(server);
            expected.insert(name, body);
        }
    }
    rebuild_media_view(&paths.media_dir, &expected);
    ensure_bookmark(&paths.bookmarks, &paths.media_dir);

    servers.len()
}

/// Discover mesh media servers from the shared probe inventory
/// (MESH-PROBE-7). Reads `probe_nmap::peers_with_service` for each
/// media kind instead of TCP-probing peers directly — one prober
/// (the probe worker) now feeds every consumer. Subsonic-family
/// servers (`airsonic` + `navidrome`, both spoken by Sublime Music)
/// map to `KIND_AIRSONIC`; `jellyfin` maps to `KIND_JELLYFIN`. The
/// precise `service_kind` comes from the bundled NSE detector
/// (MESH-PROBE-3) on the worker's deep pass.
fn discover_from_inventory(workgroup_root: &std::path::Path) -> Vec<MediaServer> {
    let mut servers = Vec::new();
    for (probe_kind, media_kind) in [
        ("airsonic", KIND_AIRSONIC),
        ("navidrome", KIND_AIRSONIC),
        ("jellyfin", KIND_JELLYFIN),
    ] {
        for hs in crate::probe_nmap::peers_with_service(workgroup_root, probe_kind) {
            let host = if hs.host.hostname.is_empty() {
                hs.host.ip.clone()
            } else {
                hs.host.hostname.clone()
            };
            servers.push(mesh_media::server_from_probe(
                media_kind,
                &host,
                &hs.host.ip,
                hs.service.port,
            ));
        }
    }
    servers
}

/// Full cycle: read discovered media servers from the probe inventory,
/// then sync configs.
fn run_once(paths: &Paths) -> usize {
    let servers = discover_from_inventory(&paths.workgroup_root);
    let n = sync_servers(paths, &servers);
    if n > 0 {
        let airsonic = servers.iter().filter(|s| s.kind == KIND_AIRSONIC).count();
        let jellyfin = servers.iter().filter(|s| s.kind == KIND_JELLYFIN).count();
        tracing::info!(target: "mackesd::app_sync", airsonic, jellyfin, "synced mesh media servers");
    }
    n
}

// ── Worker ───────────────────────────────────────────────────────────

/// Supervisor-ready app-config sync worker. 60 s tick; each tick
/// discovers + writes configs. Best-effort throughout (a missing
/// `workgroup_root` or unreachable peer just yields fewer servers).
pub struct AppSyncWorker {
    paths: Paths,
    tick: Duration,
}

impl AppSyncWorker {
    fn new(paths: Paths) -> Self {
        Self {
            paths,
            tick: Duration::from_secs(TICK_INTERVAL_S),
        }
    }
}

/// Construct the default-configured worker for the supervisor.
#[must_use]
pub fn build() -> AppSyncWorker {
    AppSyncWorker::new(Paths::defaults())
}

#[async_trait]
impl Worker for AppSyncWorker {
    fn name(&self) -> &'static str {
        "app-sync"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        run_once(&self.paths);
        loop {
            tokio::select! {
                _ = shutdown.wait() => return Ok(()),
                _ = tokio::time::sleep(self.tick) => {
                    run_once(&self.paths);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh_media::{AIRSONIC_PORT, JELLYFIN_PORT};

    fn srv(kind: &str, host: &str, ip: &str, port: u16) -> MediaServer {
        mesh_media::server_from_probe(kind, host, ip, port)
    }

    fn tmp_home(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("mde-appsync-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn worker_name_is_app_sync() {
        assert_eq!(build().name(), "app-sync");
    }

    #[test]
    fn discover_from_inventory_maps_probe_services_to_media_servers() {
        // Seed a probe inventory (MESH-PROBE-7 source): peer-a runs
        // jellyfin, peer-b runs navidrome (a Subsonic-family server →
        // KIND_AIRSONIC), peer-c runs ssh (ignored — not a media kind).
        use crate::card::probe::{host_card, service_card, HostFacts, HostSource, ServiceFacts};
        let root = tmp_home("probe-discover");
        let seed = |peer: &str, ip: &str, kind: &str, port: u16| {
            let dir = root.join(peer).join("mackesd");
            std::fs::create_dir_all(&dir).unwrap();
            let svc = service_card(
                &ServiceFacts {
                    port,
                    service_kind: kind.to_owned(),
                    product: String::new(),
                    version: String::new(),
                    fingerprint: String::new(),
                },
                1,
            );
            let host = host_card(
                &HostFacts {
                    ip: ip.to_owned(),
                    hostname: peer.to_owned(),
                    source: HostSource::Mesh,
                    trust_state: String::new(),
                    last_seen: 1,
                },
                vec![svc],
                1,
            );
            std::fs::write(
                dir.join(crate::probe_nmap::INVENTORY_FILENAME),
                crate::probe_nmap::serialize_inventory(&[host]),
            )
            .unwrap();
        };
        seed("peer-a", "10.42.0.5", "jellyfin", 8096);
        seed("peer-b", "10.42.0.6", "navidrome", 4533);
        seed("peer-c", "10.42.0.7", "ssh", 22);

        let mut servers = discover_from_inventory(&root);
        servers.sort_by(|a, b| a.ip.cmp(&b.ip));
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(servers.len(), 2, "ssh ignored; jellyfin + navidrome kept");
        // peer-a jellyfin → KIND_JELLYFIN.
        assert_eq!(servers[0].ip, "10.42.0.5");
        assert_eq!(servers[0].kind, KIND_JELLYFIN);
        assert_eq!(servers[0].port, 8096);
        // peer-b navidrome → KIND_AIRSONIC (Subsonic-family).
        assert_eq!(servers[1].ip, "10.42.0.6");
        assert_eq!(servers[1].kind, KIND_AIRSONIC);
        assert_eq!(servers[1].port, 4533);
    }

    #[test]
    fn sublime_config_is_byte_faithful_to_python() {
        let home = tmp_home("sublime");
        let paths = Paths::under(&home, home.join("qnm"));
        let servers = vec![srv(KIND_AIRSONIC, "peer-a", "10.42.0.5", AIRSONIC_PORT)];
        sync_servers(&paths, &servers);
        let got = std::fs::read_to_string(&paths.sublime).unwrap();
        let _ = std::fs::remove_dir_all(&home);
        let expected = "{\n  \"providers\": [\n    {\n      \"name\": \"peer-a (mesh)\",\n      \"server_address\": \"http://10.42.0.5:4040\",\n      \"username\": \"\",\n      \"password\": \"\",\n      \"sync_enabled\": true,\n      \"verify_cert\": false\n    }\n  ]\n}";
        assert_eq!(got, expected);
    }

    #[test]
    fn delfin_config_is_byte_faithful_to_python() {
        let home = tmp_home("delfin");
        let paths = Paths::under(&home, home.join("qnm"));
        let servers = vec![srv(KIND_JELLYFIN, "peer-b", "10.42.0.6", JELLYFIN_PORT)];
        sync_servers(&paths, &servers);
        let got = std::fs::read_to_string(&paths.delfin).unwrap();
        let _ = std::fs::remove_dir_all(&home);
        let expected = "{\n  \"servers\": [\n    {\n      \"name\": \"peer-b (mesh)\",\n      \"address\": \"http://10.42.0.6:8096\",\n      \"user\": \"\",\n      \"access_token\": \"\"\n    }\n  ]\n}";
        assert_eq!(got, expected);
    }

    #[test]
    fn empty_server_list_writes_empty_providers() {
        let home = tmp_home("empty");
        let paths = Paths::under(&home, home.join("qnm"));
        sync_servers(&paths, &[]);
        let got = std::fs::read_to_string(&paths.sublime).unwrap();
        let _ = std::fs::remove_dir_all(&home);
        assert_eq!(got, "{\n  \"providers\": []\n}");
    }

    #[test]
    fn credentials_flow_into_sublime_config() {
        let home = tmp_home("creds");
        let paths = Paths::under(&home, home.join("qnm"));
        std::fs::create_dir_all(paths.creds.parent().unwrap()).unwrap();
        std::fs::write(
            &paths.creds,
            r#"{"airsonic":{"peer-a":{"user":"mm","password":"hunter2"}}}"#,
        )
        .unwrap();
        sync_servers(
            &paths,
            &[srv(KIND_AIRSONIC, "peer-a", "10.42.0.5", AIRSONIC_PORT)],
        );
        let got = std::fs::read_to_string(&paths.sublime).unwrap();
        let _ = std::fs::remove_dir_all(&home);
        assert!(got.contains("\"username\": \"mm\""));
        assert!(got.contains("\"password\": \"hunter2\""));
    }

    #[test]
    fn media_view_writes_launcher_and_prunes_stale() {
        let home = tmp_home("view");
        let paths = Paths::under(&home, home.join("qnm"));
        // Pre-seed a stale launcher that should be pruned.
        std::fs::create_dir_all(&paths.media_dir).unwrap();
        std::fs::write(paths.media_dir.join("Stale_Server.desktop"), "old").unwrap();
        sync_servers(
            &paths,
            &[srv(KIND_AIRSONIC, "peer-a", "10.42.0.5", AIRSONIC_PORT)],
        );
        let live = paths.media_dir.join("Airsonic_-_peer-a.desktop");
        let stale = paths.media_dir.join("Stale_Server.desktop");
        let live_exists = live.is_file();
        let stale_exists = stale.is_file();
        let body = std::fs::read_to_string(&live).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&home);
        assert!(live_exists, "live launcher written");
        assert!(!stale_exists, "stale launcher pruned");
        assert!(body.contains("Exec=flatpak run com.sublimemusic.SublimeMusic"));
        assert!(body.contains("Comment=Mesh media server at http://10.42.0.5:4040"));
    }

    #[test]
    fn bookmark_is_idempotent() {
        let home = tmp_home("bookmark");
        let paths = Paths::under(&home, home.join("qnm"));
        sync_servers(&paths, &[]);
        sync_servers(&paths, &[]);
        let got = std::fs::read_to_string(&paths.bookmarks).unwrap();
        let _ = std::fs::remove_dir_all(&home);
        let line = format!("file://{}/Mackes Media Mackes Media", home.display());
        assert_eq!(
            got.matches(&line).count(),
            1,
            "bookmark line appears exactly once"
        );
    }
}
