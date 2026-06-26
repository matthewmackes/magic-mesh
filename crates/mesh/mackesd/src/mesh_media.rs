//! Mesh media-server model + peer overlay-IP enumeration.
//!
//! Originally (EPIC-SYNC-APP-CONFIG) this did mesh-peer TCP-probe
//! discovery for app_sync. **MESH-PROBE-7 (2026-05-28) retired that
//! TCP-probe path**: media discovery now reads the shared probe
//! inventory via `probe_nmap::peers_with_service` (one prober — the
//! probe worker — feeds every consumer), so the bespoke
//! `discover`/`scan_probe`/`probe_port`/`dedupe` TCP-probe is gone.
//!
//! What remains is the small shared model both still need:
//!   * [`MediaServer`] + [`server_from_probe`] — the server type
//!     app_sync's config writers consume (app_sync builds these from
//!     probe-inventory `HostService` rows).
//!   * [`peer_overlay_ips`] — enumerate every peer's Nebula overlay IP
//!     from the GFS-replicated `nebula-bundle.json` files; the probe
//!     worker's [`crate::probe_nmap::mesh_targets`] uses this to know
//!     which peers to scan.
//!
//! ## MEDIA-7 — the mesh service registration
//!
//! A `Lighthouse_Media` node ([`crate::worker_role`]'s `Capability::Media`
//! gate) runs the media-registry worker ([`crate::workers::media_registry`]),
//! which publishes its `navidrome` instance into the SAME mesh service
//! registry the other published services use — the replicated QNM-Shared
//! plane every node already reads (`compute-inventory.json`,
//! `running-apps.json`) plus the per-peer Bus topic — so the media service is
//! discoverable mesh-wide. [`MediaRegistration`] is that registry document,
//! [`probe_navidrome`] is the per-instance health probe behind its `health`
//! field, and [`MEDIA_REGISTRY_FILE`] / [`media_registry_topic`] name the
//! registry locations. The live-stream / bucket acceptance (MEDIA-2) is a
//! separate unit; this is the registry-publish + health half.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream};
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Minimal projection of `nebula-bundle.json` — we only need the
/// overlay IP. Serde ignores the bundle's other fields (cert PEMs,
/// lighthouses, etc.), so this stays decoupled from
/// [`crate::ca::bundle::NebulaBundle`]'s full shape.
#[derive(Deserialize)]
struct BundleOverlayIp {
    overlay_ip: String,
}

/// Airsonic / Subsonic media-server kind tag.
pub const KIND_AIRSONIC: &str = "airsonic";
/// Jellyfin media-server kind tag.
pub const KIND_JELLYFIN: &str = "jellyfin";

/// Default Airsonic/Subsonic port.
pub const AIRSONIC_PORT: u16 = 4040;
/// Default Jellyfin port.
pub const JELLYFIN_PORT: u16 = 8096;

/// One media server reachable on the mesh. Built by app_sync from a
/// probe-inventory `HostService` row; consumed by app_sync's Sublime
/// Music / Delfin config writers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaServer {
    /// [`KIND_AIRSONIC`] or [`KIND_JELLYFIN`].
    pub kind: String,
    /// Hostname (peer node-id) for display.
    pub host: String,
    /// Resolved overlay IP.
    pub ip: String,
    /// Service port.
    pub port: u16,
}

impl MediaServer {
    /// `http://<ip>:<port>` — mesh-internal; Nebula provides the
    /// trust layer, so plain HTTP over the overlay is intentional.
    #[must_use]
    pub fn url(&self) -> String {
        format!("http://{}:{}", self.ip, self.port)
    }
}

/// Build a [`MediaServer`] from its parts. Pure constructor.
#[must_use]
pub fn server_from_probe(kind: &str, host: &str, ip: &str, port: u16) -> MediaServer {
    MediaServer {
        kind: kind.to_owned(),
        host: host.to_owned(),
        ip: ip.to_owned(),
        port,
    }
}

/// Enumerate every peer's `(node_id, overlay_ip)` from the
/// GFS-replicated nebula bundles under `workgroup_root`. Includes the local
/// peer's own bundle. Missing root or unreadable/malformed bundles are
/// skipped (best-effort). Used by the probe worker
/// ([`crate::probe_nmap::mesh_targets`]) to resolve mesh-peer scan
/// targets.
#[must_use]
pub fn peer_overlay_ips(workgroup_root: &Path) -> Vec<(String, String)> {
    let entries = match std::fs::read_dir(workgroup_root) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut out: Vec<(String, String)> = Vec::new();
    for entry in entries.flatten() {
        let Some(node_id) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let bundle_path = entry.path().join("mackesd").join("nebula-bundle.json");
        let Ok(bytes) = std::fs::read(&bundle_path) else {
            continue;
        };
        let Ok(bundle) = serde_json::from_slice::<BundleOverlayIp>(&bytes) else {
            continue;
        };
        if !bundle.overlay_ip.is_empty() {
            out.push((node_id, bundle.overlay_ip));
        }
    }
    out.sort();
    out
}

// ─────────────────────────────────────────────────────────────────────────
// MEDIA-7 — registering the navidrome/media service into the mesh registry.
// ─────────────────────────────────────────────────────────────────────────

/// Default Navidrome (Subsonic/airsonic-family) port — the pinned localhost
/// port the descriptor probe (`descriptors::MEDIA_PORTS`) and probe-nmap
/// already key media off. The media-registry worker probes + publishes this
/// instance.
pub const NAVIDROME_PORT: u16 = 4533;

/// The registered media service's stable kind tag. A media node always
/// registers `navidrome` (the foundation media service MEDIA-3 spawns);
/// keeping it a constant matches the `WORKER_CAPABILITIES` table's worker
/// name so the gate + the registration speak the same token.
pub const NAVIDROME_KIND: &str = "navidrome";

/// File name a media node mirrors its registration to under its QNM-Shared
/// dir — the SAME replicated registry plane the other published services use
/// (`compute-inventory.json`, `running-apps.json`). Every node reads these to
/// see the fleet's published services.
pub const MEDIA_REGISTRY_FILE: &str = "media-registry.json";

/// Per-instance health budget — localhost answers in microseconds; 200 ms is
/// generous and matches `descriptors::CONNECT_TIMEOUT` so the probe can never
/// stall the worker tick.
const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_millis(200);

/// Health of a registered media instance. `up` when the service answers on
/// its port, `down` when it doesn't — the per-instance health field MEDIA-7
/// requires so a consumer reading the registry knows whether the published
/// navidrome is actually serving, not merely declared.
pub const HEALTH_UP: &str = "up";
/// See [`HEALTH_UP`].
pub const HEALTH_DOWN: &str = "down";

/// The Bus topic a media node publishes its registration to:
/// `mesh/services/media/<peer>`. Mirrors the per-peer topic shape the other
/// published services use (`compute/inventory/<peer>`); `<peer>` is the
/// node-id so registrations don't collide.
#[must_use]
pub fn media_registry_topic(peer: &str) -> String {
    format!("mesh/services/media/{peer}")
}

/// MEDIA-8 — the stable mesh DNS name the published media service is reached
/// at (a CNAME/round-robin to the serving Lighthouse_Media node[s]). The
/// `shared_account.server` published in the registry points here, so a
/// Workstation auto-configures `mde-music` against `music.mesh` rather than a
/// specific peer's overlay IP — the service stays reachable as instances come
/// and go.
pub const MUSIC_MESH_HOST: &str = "music.mesh";

/// MEDIA-8 — the canonical `http://music.mesh:<navidrome-port>` server URL the
/// published [`SharedAccount`] hands to clients. Single-sourced off
/// [`MUSIC_MESH_HOST`] + [`NAVIDROME_PORT`] so the worker write side and the
/// registry agree byte-for-byte.
#[must_use]
pub fn music_mesh_server_url() -> String {
    format!("http://{MUSIC_MESH_HOST}:{NAVIDROME_PORT}")
}

/// MEDIA-8 — the read-only shared music account a Workstation auto-configures
/// its `mde-music` client with. A `Lighthouse_Media` node publishes this into
/// its [`MediaRegistration`] (sourced from the leader-managed `media-spaces`
/// secret's `ND_ADMIN_USER`/`ND_ADMIN_PASS`); a Workstation subscribes and
/// writes `airsonic-creds.json` so the first-run connect form is bypassed.
///
/// **READ-ONLY by intent.** The honest remaining (MEDIA-6) is provisioning a
/// distinct least-privilege Navidrome account; until that lands the only shared
/// account the secret carries is the admin one, which IS published so the
/// auto-config path is real end-to-end. The field name keeps the contract so
/// MEDIA-6 only swaps the *source* of the username/password, not the wire shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedAccount {
    /// Server URL clients connect to — always [`music_mesh_server_url`].
    pub server: String,
    /// Shared account username (from the `media-spaces` secret).
    pub username: String,
    /// Shared account password (from the `media-spaces` secret).
    pub password: String,
}

impl SharedAccount {
    /// Build the shared account a Workstation auto-configures against:
    /// `http://music.mesh:4533` + the supplied username/password. The server is
    /// pinned to [`music_mesh_server_url`] so every published account agrees.
    #[must_use]
    pub fn new(username: &str, password: &str) -> Self {
        Self {
            server: music_mesh_server_url(),
            username: username.to_owned(),
            password: password.to_owned(),
        }
    }

    /// MEDIA-8 — derive the shared account from the `media-spaces` leader
    /// secret body. The secret is the `.env`-style file
    /// `install-helpers/setup-media-navidrome.sh` consumes (`KEY=VAL` lines), so
    /// we read `ND_ADMIN_USER` + `ND_ADMIN_PASS` out of it. `None` when either is
    /// missing/empty — a node holding a malformed/partial secret publishes NO
    /// account rather than a half-built one. Today this is the admin account
    /// (the only one the secret carries); MEDIA-6 swaps the source for a
    /// least-privilege read-only account without changing this shape.
    #[must_use]
    pub fn from_media_spaces_env(env_body: &str) -> Option<Self> {
        let user = env_var_value(env_body, "ND_ADMIN_USER")?;
        let pass = env_var_value(env_body, "ND_ADMIN_PASS")?;
        if user.is_empty() || pass.is_empty() {
            return None;
        }
        Some(Self::new(&user, &pass))
    }
}

/// Pull `KEY`'s value out of a `.env`-style body (`KEY=VAL` per line). Handles
/// `export KEY=VAL`, surrounding whitespace, and single/double quotes around the
/// value; ignores `#` comments and blank lines. Returns the FIRST match's value.
/// `None` when the key is absent. Pure — unit-tested apart from the secret store.
fn env_var_value(body: &str, key: &str) -> Option<String> {
    for raw in body.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").map_or(line, str::trim_start);
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        if k.trim() != key {
            continue;
        }
        let v = v.trim();
        // Strip a single matched pair of surrounding quotes.
        let v = v
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .or_else(|| v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
            .unwrap_or(v);
        return Some(v.to_owned());
    }
    None
}

/// One media service registered into the mesh service registry by a
/// `Lighthouse_Media` node — the document MEDIA-7 publishes. Carries the
/// per-instance `health` field ([`HEALTH_UP`] / [`HEALTH_DOWN`]) so a
/// consumer knows whether the published instance is actually serving.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaRegistration {
    /// Registering node-id (the registry key / topic suffix).
    pub node_id: String,
    /// Service kind — always [`NAVIDROME_KIND`] today.
    pub kind: String,
    /// Port the instance is bound to.
    pub port: u16,
    /// Per-instance health: [`HEALTH_UP`] when the service answers on its
    /// port, else [`HEALTH_DOWN`].
    pub health: String,
    /// MEDIA-8 — the read-only shared music account a Workstation
    /// auto-configures its client with (server + username + password). `None`
    /// when the publishing node couldn't resolve the `media-spaces` secret (so
    /// a node that hasn't been handed the shared creds publishes a registration
    /// without an account rather than a fake one). `skip_serializing_if` keeps
    /// the wire shape backward-compatible: a registration without an account
    /// omits the field entirely, and an older reader ignores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_account: Option<SharedAccount>,
}

impl MediaRegistration {
    /// `true` when the registered instance answered its health probe.
    #[must_use]
    pub fn is_up(&self) -> bool {
        self.health == HEALTH_UP
    }
}

/// Probe a localhost port and map the result to the per-instance health
/// string. A successful TCP connect → [`HEALTH_UP`], else [`HEALTH_DOWN`].
/// Pure-ish (only a localhost connect, bounded by [`HEALTH_PROBE_TIMEOUT`]);
/// the same localhost-connect liveness check `descriptors::listening` uses,
/// so a port the descriptor scan reports as a media service is exactly the
/// one this registers as `up`.
#[must_use]
pub fn probe_navidrome(port: u16) -> String {
    let addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port));
    if TcpStream::connect_timeout(&addr, HEALTH_PROBE_TIMEOUT).is_ok() {
        HEALTH_UP.to_owned()
    } else {
        HEALTH_DOWN.to_owned()
    }
}

/// Build this node's media registration from a probed health string. Pure
/// constructor so the registration shape is unit-tested without a socket. No
/// shared account is attached — see [`registration_with_account`] for the
/// MEDIA-8 auto-config path.
#[must_use]
pub fn registration(node_id: &str, port: u16, health: &str) -> MediaRegistration {
    MediaRegistration {
        node_id: node_id.to_owned(),
        kind: NAVIDROME_KIND.to_owned(),
        port,
        health: health.to_owned(),
        shared_account: None,
    }
}

/// MEDIA-8 — like [`registration`] but attaches the read-only shared account a
/// Workstation auto-configures `mde-music` with. `account` is `None` when the
/// publishing node couldn't resolve the `media-spaces` secret (an honest "no
/// account to publish" rather than a fabricated one).
#[must_use]
pub fn registration_with_account(
    node_id: &str,
    port: u16,
    health: &str,
    account: Option<SharedAccount>,
) -> MediaRegistration {
    MediaRegistration {
        shared_account: account,
        ..registration(node_id, port, health)
    }
}

/// MEDIA-8 — fold the replicated QNM-Shared registry plane
/// (`<root>/<host>/media-registry.json`, written by each Lighthouse_Media
/// node's [`crate::workers::media_registry`]) and return the first published
/// [`SharedAccount`] a Workstation can auto-configure against. The same
/// `read_dir`-over-the-share discipline `app_sync` / `apps::fleet_*` use.
///
/// Prefers an account from a registration whose instance is [`is_up`], so a
/// Workstation auto-configures against a *serving* node when one is published;
/// falls back to any published account otherwise (the account is the same shared
/// creds regardless of which node published it — they all point at `music.mesh`).
/// `None` when the share isn't mounted or no registration carries an account.
///
/// [`is_up`]: MediaRegistration::is_up
#[must_use]
pub fn read_shared_account_from_plane(workgroup_root: &Path) -> Option<SharedAccount> {
    let entries = std::fs::read_dir(workgroup_root).ok()?;
    let mut fallback: Option<SharedAccount> = None;
    for ent in entries.flatten() {
        let path = ent.path().join(MEDIA_REGISTRY_FILE);
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(reg) = serde_json::from_str::<MediaRegistration>(&body) else {
            continue;
        };
        // Read `is_up` BEFORE moving the account out of the registration.
        let up = reg.is_up();
        let Some(acct) = reg.shared_account else {
            continue;
        };
        // A serving (up) instance wins immediately; otherwise remember the
        // first account as a fallback and keep scanning for an up one.
        if up {
            return Some(acct);
        }
        fallback.get_or_insert(acct);
    }
    fallback
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_is_http_over_overlay() {
        let s = server_from_probe(KIND_AIRSONIC, "peer-a", "10.42.0.5", AIRSONIC_PORT);
        assert_eq!(s.url(), "http://10.42.0.5:4040");
    }

    #[test]
    fn server_from_probe_sets_fields() {
        let s = server_from_probe(KIND_JELLYFIN, "peer-b", "10.42.0.6", JELLYFIN_PORT);
        assert_eq!(s.kind, KIND_JELLYFIN);
        assert_eq!(s.host, "peer-b");
        assert_eq!(s.ip, "10.42.0.6");
        assert_eq!(s.port, 8096);
    }

    #[test]
    fn peer_overlay_ips_empty_for_missing_root() {
        let out = peer_overlay_ips(Path::new("/nonexistent/qnm/root/xyz"));
        assert!(out.is_empty());
    }

    #[test]
    fn peer_overlay_ips_reads_bundles() {
        let tmp = std::env::temp_dir().join(format!("mde-mediatest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        for (peer, ip) in [("peer-a", "10.42.0.5"), ("peer-b", "10.42.0.6")] {
            let dir = tmp.join(peer).join("mackesd");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("nebula-bundle.json"),
                format!(r#"{{"overlay_ip":"{ip}","node_id":"{peer}"}}"#),
            )
            .unwrap();
        }
        let out = peer_overlay_ips(&tmp);
        let _ = std::fs::remove_dir_all(&tmp);
        assert_eq!(
            out,
            vec![
                ("peer-a".to_string(), "10.42.0.5".to_string()),
                ("peer-b".to_string(), "10.42.0.6".to_string()),
            ]
        );
    }

    // ── MEDIA-7: the mesh service registration ──

    #[test]
    fn registry_topic_is_per_peer() {
        assert_eq!(
            media_registry_topic("peer:eagle"),
            "mesh/services/media/peer:eagle"
        );
    }

    #[test]
    fn registration_pins_navidrome_kind_and_round_trips() {
        let reg = registration("peer:eagle", NAVIDROME_PORT, HEALTH_UP);
        assert_eq!(reg.kind, NAVIDROME_KIND);
        assert_eq!(reg.port, 4533);
        assert!(reg.is_up());
        let json = serde_json::to_string(&reg).unwrap();
        // The per-instance health field MEDIA-7 requires is on the wire.
        assert!(json.contains("\"health\":\"up\""));
        assert!(json.contains("\"kind\":\"navidrome\""));
        let back: MediaRegistration = serde_json::from_str(&json).unwrap();
        assert_eq!(back, reg);
    }

    #[test]
    fn health_down_when_port_closed() {
        // Port 1 is privileged + unbound in CI → connect fails → down.
        // (No service is started; the probe must degrade to `down`, never
        // hang — the timeout bounds it.)
        assert_eq!(probe_navidrome(1), HEALTH_DOWN);
        let reg = registration("peer:host", NAVIDROME_PORT, &probe_navidrome(1));
        assert!(!reg.is_up());
    }

    // ── MEDIA-8: the published shared account ──

    #[test]
    fn music_mesh_url_pins_host_and_navidrome_port() {
        // The auto-config server URL the registry hands out is `music.mesh`
        // (the stable mesh name) on the navidrome port, NOT a peer overlay IP.
        assert_eq!(music_mesh_server_url(), "http://music.mesh:4533");
    }

    #[test]
    fn shared_account_pins_the_music_mesh_server() {
        // A SharedAccount always points at music.mesh; only the creds vary.
        let acct = SharedAccount::new("mesh-music", "s3cret");
        assert_eq!(acct.server, "http://music.mesh:4533");
        assert_eq!(acct.username, "mesh-music");
        assert_eq!(acct.password, "s3cret");
    }

    #[test]
    fn registration_with_account_round_trips_on_the_wire() {
        // The shared_account rides the same registry document MEDIA-7 publishes;
        // it must (de)serialize so a Workstation reader reconstructs it exactly.
        let acct = SharedAccount::new("mesh-music", "s3cret");
        let reg =
            registration_with_account("peer:eagle", NAVIDROME_PORT, HEALTH_UP, Some(acct.clone()));
        let json = serde_json::to_string(&reg).unwrap();
        // The account is on the wire under `shared_account`.
        assert!(json.contains("\"shared_account\""));
        assert!(json.contains("\"server\":\"http://music.mesh:4533\""));
        assert!(json.contains("\"username\":\"mesh-music\""));
        let back: MediaRegistration = serde_json::from_str(&json).unwrap();
        assert_eq!(back, reg);
        assert_eq!(back.shared_account, Some(acct));
    }

    #[test]
    fn shared_account_from_media_spaces_env_reads_nd_admin_creds() {
        // The media-spaces secret is the .env body setup-media-navidrome.sh
        // consumes; the shared account reads ND_ADMIN_USER/PASS out of it.
        let body = "\
DO_SPACES_KEY=AKIAEXAMPLE\n\
DO_SPACES_SECRET=secret\n\
DO_SPACES_BUCKET=mcnf-mesh-media\n\
ND_ADMIN_USER=mesh-music\n\
ND_ADMIN_PASS=hunter2\n";
        let acct = SharedAccount::from_media_spaces_env(body).expect("creds present");
        assert_eq!(acct.username, "mesh-music");
        assert_eq!(acct.password, "hunter2");
        assert_eq!(acct.server, "http://music.mesh:4533");
    }

    #[test]
    fn shared_account_env_handles_export_quotes_and_comments() {
        let body = "\
# media-spaces secret\n\
export ND_ADMIN_USER=\"mesh music\"\n\
ND_ADMIN_PASS='p@ss=word'\n";
        let acct = SharedAccount::from_media_spaces_env(body).unwrap();
        assert_eq!(acct.username, "mesh music");
        // The value's own '=' is preserved (only the first '=' splits).
        assert_eq!(acct.password, "p@ss=word");
    }

    #[test]
    fn shared_account_env_none_when_creds_missing_or_empty() {
        // No ND_ADMIN_* at all → None (the node publishes no account).
        assert_eq!(
            SharedAccount::from_media_spaces_env("DO_SPACES_KEY=x\n"),
            None
        );
        // Present but empty → None (a half-built account is worse than none).
        assert_eq!(
            SharedAccount::from_media_spaces_env("ND_ADMIN_USER=u\nND_ADMIN_PASS=\n"),
            None
        );
    }

    #[test]
    fn registration_without_account_omits_the_field_and_back_compat_deserializes() {
        // A node that couldn't resolve the secret publishes NO account — the
        // field is omitted entirely (skip_serializing_if), so an older reader's
        // document (no `shared_account` key) still deserializes to `None`.
        let reg = registration("peer:eagle", NAVIDROME_PORT, HEALTH_UP);
        assert_eq!(reg.shared_account, None);
        let json = serde_json::to_string(&reg).unwrap();
        assert!(
            !json.contains("shared_account"),
            "absent account omits the field, not null"
        );
        // A legacy MEDIA-7 document (pre-MEDIA-8) deserializes with no account.
        let legacy = r#"{"node_id":"peer:x","kind":"navidrome","port":4533,"health":"up"}"#;
        let back: MediaRegistration = serde_json::from_str(legacy).unwrap();
        assert_eq!(back.shared_account, None);
        assert_eq!(back.kind, NAVIDROME_KIND);
    }

    fn seed_plane_doc(root: &Path, host: &str, reg: &MediaRegistration) {
        let dir = root.join(host);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(MEDIA_REGISTRY_FILE),
            serde_json::to_string(reg).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn read_shared_account_from_plane_empty_when_no_share() {
        assert_eq!(
            read_shared_account_from_plane(Path::new("/nonexistent/qnm/xyz")),
            None
        );
    }

    #[test]
    fn read_shared_account_from_plane_prefers_an_up_instance() {
        let tmp = std::env::temp_dir().join(format!("mde-mediaplane-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        // A DOWN instance publishes account "old"; an UP one publishes "live".
        // The reader must prefer the serving (up) node's account.
        seed_plane_doc(
            &tmp,
            "downhost",
            &registration_with_account(
                "peer:down",
                NAVIDROME_PORT,
                HEALTH_DOWN,
                Some(SharedAccount::new("old", "p1")),
            ),
        );
        seed_plane_doc(
            &tmp,
            "uphost",
            &registration_with_account(
                "peer:up",
                NAVIDROME_PORT,
                HEALTH_UP,
                Some(SharedAccount::new("live", "p2")),
            ),
        );
        let acct = read_shared_account_from_plane(&tmp).expect("an account is published");
        let _ = std::fs::remove_dir_all(&tmp);
        assert_eq!(acct.username, "live");
    }

    #[test]
    fn read_shared_account_from_plane_falls_back_to_a_down_account() {
        let tmp = std::env::temp_dir().join(format!("mde-mediaplane-dn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        // Only a down instance is published, but it still carries the shared
        // account — better to auto-config than show the first-run form.
        seed_plane_doc(
            &tmp,
            "downhost",
            &registration_with_account(
                "peer:down",
                NAVIDROME_PORT,
                HEALTH_DOWN,
                Some(SharedAccount::new("mesh-music", "p1")),
            ),
        );
        // A doc WITHOUT an account is skipped (not all media nodes have creds).
        seed_plane_doc(
            &tmp,
            "noacct",
            &registration("peer:x", NAVIDROME_PORT, HEALTH_UP),
        );
        let acct = read_shared_account_from_plane(&tmp).expect("the down account is the fallback");
        let _ = std::fs::remove_dir_all(&tmp);
        assert_eq!(acct.username, "mesh-music");
    }
}
