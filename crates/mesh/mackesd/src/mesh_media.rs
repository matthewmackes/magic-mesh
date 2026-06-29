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

use std::path::{Path, PathBuf};

use serde::Deserialize;

// ─────────────────────────────────────────────────────────────────────────────
// MEDIA-LIGHTHOUSE (Navidrome music service on Lighthouse_Media nodes)
//
// The pieces below are the pure doc/state builders the media workers + the
// service registry + the enroll birthright consume. Each is a small,
// dependency-free function with a unit test; the impure wiring (podman /
// systemctl shell-outs, the SecretStore I/O) lives in the feature-gated
// `workers::media_navidrome` worker and `ipc::nebula` / the enroll path, which
// call straight through to these.
// ─────────────────────────────────────────────────────────────────────────────

/// The Navidrome Subsonic API port (lock #1; the port `mde-musicd` speaks).
/// Single-sourced here so the birthright URL, the service registration, and the
/// supervisor all agree.
pub const NAVIDROME_PORT: u16 = 4533;

/// The systemd unit `setup-media-navidrome.sh` installs — the runtime the
/// MEDIA-3 worker ADOPTS + supervises (self-heal/restart). Single-sourced.
pub const NAVIDROME_UNIT: &str = "mcnf-navidrome.service";

/// The container name the unit runs Navidrome under (`podman run --name`).
pub const NAVIDROME_CONTAINER: &str = "navidrome";

/// MEDIA-6 — the single shared service account's username. Matches the
/// auto-created Navidrome admin (`ND_DEFAULTADMINPASSWORD` provisions the
/// `admin` user on first start), so the birthright creds + the server agree.
pub const MEDIA_ACCOUNT_USER: &str = "admin";

/// MEDIA-8 — the birthright client server URL: the active-active `music.mesh`
/// name (MEDIA-5) on the Navidrome port. Every node's `airsonic-creds.json`
/// points here, so a client reaches whichever media lighthouse is live.
pub const MUSIC_BIRTHRIGHT_SERVER_URL: &str = "http://music.mesh:4533";

/// MEDIA-8 — `airsonic-creds.json` location relative to `$HOME` (mirrors
/// `mde_musicd::creds::CREDS_REL_PATH`; kept as a literal so mackesd needn't
/// depend on the musicd crate). The whole workgroup shares one credential
/// (replicated under the mesh data dir), so any node reads the same account.
pub const AIRSONIC_CREDS_REL_PATH: &str = ".local/share/mde/airsonic-creds.json";

// ── MEDIA-3: the Navidrome adopt/supervise decision ──

/// What the MEDIA-3 supervisor should do this tick, derived purely from the
/// observed runtime so the decision is exhaustively unit-testable (the worker is
/// the thin shell-out adapter around this).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavidromeAction {
    /// The unit is active AND the container is running — already adopted and
    /// healthy; do nothing (the unit's own `Restart=always` handles the fast
    /// path; this worker is the higher-level self-heal).
    Healthy,
    /// The unit is down / failed, or its container vanished — heal by
    /// (re)starting the unit so Navidrome comes back (adopt-and-supervise).
    Heal,
}

/// MEDIA-3 — decide the supervisor action from the unit + container state. The
/// worker ADOPTS the existing `setup-media-navidrome.sh` unit (it never
/// re-creates the container — the unit owns the `podman run`), and self-heals by
/// restarting the unit whenever it isn't both active and running.
#[must_use]
pub fn decide_navidrome_action(unit_active: bool, container_running: bool) -> NavidromeAction {
    if unit_active && container_running {
        NavidromeAction::Healthy
    } else {
        NavidromeAction::Heal
    }
}

// ── MEDIA-6: the shared service-account secret ──

/// MEDIA-6 — the leader-managed secret-store key holding the shared Navidrome
/// account password. Namespaced under `media/` alongside the `vpn/` + `xcp/`
/// creds (mirrors `ipc::secret_store::xcp_creds_ref`); this string IS the
/// store/etcd key, so it must stay stable.
#[must_use]
pub fn media_account_secret_ref() -> String {
    "media/navidrome-account".to_string()
}

/// MEDIA-6 — generate a fresh shared-account password from the OS CSPRNG: 24
/// URL-safe base64 chars (18 random bytes), strong and free of shell-hostile
/// characters (it lands in `ND_DEFAULTADMINPASSWORD` / a root-only env file).
#[must_use]
pub fn gen_account_password() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 18];
    rand::thread_rng().fill_bytes(&mut bytes);
    url_safe_b64(&bytes)
}

/// MEDIA-6 — idempotent provisioning decision for the shared password: keep an
/// already-distributed secret, else mint a new one. Returns `(password,
/// newly_generated)`. Pure, so the "create-only-once" invariant is unit-testable
/// without touching the store: a node that re-runs after the secret is set reads
/// the SAME password back (`newly_generated == false`) — the account is created
/// on first start only, never reset out from under live sessions.
#[must_use]
pub fn ensure_account_password(existing: Option<String>) -> (String, bool) {
    match existing {
        Some(p) if !p.trim().is_empty() => (p, false),
        _ => (gen_account_password(), true),
    }
}

/// MEDIA-6 — the root-only env file `setup-media-navidrome.sh` reads its
/// `ND_ADMIN_USER` / `ND_ADMIN_PASS` (and the operator's `DO_SPACES_*`) from.
/// The leader-managed account password is merged in here, never passed on argv.
pub const MEDIA_CREDS_ENV_PATH: &str = "/etc/mackesd/media-spaces.env";

/// MEDIA-6 — merge `updates` (`KEY=value`) into an existing `KEY=value` env-file
/// body: replace a key in place, append a new one, and preserve every other line
/// (comments + the operator's `DO_SPACES_*` keys from MEDIA-2). Pure + testable;
/// the worker reads the file, merges the leader's `ND_ADMIN_*`, and rewrites it
/// 0600 — distributing the shared password without clobbering the S3 creds.
#[must_use]
pub fn merge_env_file(existing: &str, updates: &[(&str, &str)]) -> String {
    let mut seen = vec![false; updates.len()];
    let mut out_lines: Vec<String> = Vec::new();
    for line in existing.lines() {
        let trimmed = line.trim_start();
        // Keep comments + blank/non-assignment lines verbatim.
        let key = if trimmed.starts_with('#') {
            None
        } else {
            trimmed.split_once('=').map(|(k, _)| k.trim())
        };
        if let Some(k) = key {
            if let Some(i) = updates.iter().position(|(uk, _)| *uk == k) {
                seen[i] = true;
                out_lines.push(format!("{}={}", updates[i].0, updates[i].1));
                continue;
            }
        }
        out_lines.push(line.to_string());
    }
    for (i, (k, v)) in updates.iter().enumerate() {
        if !seen[i] {
            out_lines.push(format!("{k}={v}"));
        }
    }
    let mut body = out_lines.join("\n");
    body.push('\n');
    body
}

// ── MEDIA-7: published-service registration row ──

/// MEDIA-7 — build this node's `music` row for the published-services registry,
/// or `None` to DE-REGISTER (no stale entry). A node publishes the row only
/// while it is a `Lighthouse_Media` (`serves_media`); on teardown (role changed
/// away / decommissioned) it returns `None`, so the Workbench surface never
/// shows a media instance that isn't one. While registered, `is_publishable`
/// reflects per-instance health (the container actually serving on an overlay
/// IP), so the surface shows WHICH media lighthouses are live.
#[must_use]
pub fn music_service_row(
    serves_media: bool,
    container_healthy: bool,
    overlay_ip: Option<&str>,
) -> Option<serde_json::Value> {
    if !serves_media {
        return None; // de-registered — not a media lighthouse
    }
    let overlay = overlay_ip.filter(|ip| !ip.is_empty());
    Some(serde_json::json!({
        "id": "music",
        "name": "Music (Navidrome)",
        "port": NAVIDROME_PORT,
        "proto": "tcp",
        "overlay_ip": overlay,
        // Healthy AND reachable (an overlay IP exists) ⇒ this instance is serving.
        "is_publishable": container_healthy && overlay.is_some(),
    }))
}

// ── MEDIA-8: birthright airsonic-creds.json ──

/// MEDIA-8 — the `airsonic-creds.json` body pointing the Music System at the
/// active-active `music.mesh` service + the shared account. Same 3-field shape
/// `mde_musicd::creds::Creds` reads (`server_url` / `username` / `password`),
/// pretty-printed.
#[must_use]
pub fn music_birthright_json(username: &str, password: &str) -> String {
    let creds = serde_json::json!({
        "server_url": MUSIC_BIRTHRIGHT_SERVER_URL,
        "username": username,
        "password": password,
    });
    serde_json::to_string_pretty(&creds).unwrap_or_else(|_| "{}".to_string())
}

/// MEDIA-8 — the per-user `airsonic-creds.json` path under `$HOME` (falls back
/// to `/root` when `$HOME` is unset), mirroring `mde_musicd::creds::default_path`.
#[must_use]
pub fn airsonic_creds_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    Path::new(&home).join(AIRSONIC_CREDS_REL_PATH)
}

/// MEDIA-8 — write the music-client birthright (`airsonic-creds.json`) at
/// `path`, creating the parent dir. Idempotent: re-running with the same account
/// writes byte-identical content, so an already-enrolled node picking it up is a
/// no-op. `mackesd` calls this at enroll (the Auto-Configuration host core).
///
/// # Errors
/// I/O failure creating the parent dir or writing the file.
pub fn write_music_birthright(path: &Path, username: &str, password: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, music_birthright_json(username, password))
}

/// URL-safe, unpadded base64 of `bytes` (the alphabet `mde-musicd`/Subsonic
/// tolerate in a credential; no `+`/`/`/`=`). Kept local so this stays
/// dependency-free.
fn url_safe_b64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        let take = chunk.len() + 1; // 3 bytes → 4 chars, 2 → 3, 1 → 2
        for i in 0..take {
            let idx = (n >> (18 - 6 * i)) & 0x3f;
            out.push(ALPHABET[idx as usize] as char);
        }
    }
    out
}

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

    // ── MEDIA-3 ──

    #[test]
    fn navidrome_action_heals_only_when_not_fully_up() {
        // Adopted + running ⇒ leave it alone; anything else ⇒ self-heal.
        assert_eq!(
            decide_navidrome_action(true, true),
            NavidromeAction::Healthy
        );
        assert_eq!(decide_navidrome_action(false, true), NavidromeAction::Heal);
        assert_eq!(decide_navidrome_action(true, false), NavidromeAction::Heal);
        assert_eq!(decide_navidrome_action(false, false), NavidromeAction::Heal);
    }

    // ── MEDIA-6 ──

    #[test]
    fn account_secret_ref_is_stable_and_namespaced() {
        assert_eq!(media_account_secret_ref(), "media/navidrome-account");
        assert!(media_account_secret_ref().starts_with("media/"));
    }

    #[test]
    fn ensure_account_password_is_idempotent_keeps_existing() {
        // First start (no secret) mints one; the SAME password reads back
        // unchanged thereafter — the account is created once, never reset.
        let (first, made) = ensure_account_password(None);
        assert!(made, "first call generates");
        assert!(first.len() >= 16, "strong password");
        let (again, made2) = ensure_account_password(Some(first.clone()));
        assert!(!made2, "an existing secret is kept");
        assert_eq!(again, first, "idempotent — never re-minted");
        // A blank/whitespace stored secret is treated as absent (re-minted).
        let (fresh, made3) = ensure_account_password(Some("   ".into()));
        assert!(made3);
        assert_ne!(fresh, first);
    }

    #[test]
    fn merge_env_file_replaces_keys_and_preserves_operator_lines() {
        // The operator's MEDIA-2 S3 keys + a comment survive; the leader's
        // ND_ADMIN_* are merged (one replaced in place, one appended).
        let existing = "# operator creds (MEDIA-2)\n\
                        DO_SPACES_KEY=AKIA123\n\
                        DO_SPACES_SECRET=shh\n\
                        ND_ADMIN_USER=stale\n";
        let merged = merge_env_file(
            &existing,
            &[("ND_ADMIN_USER", "admin"), ("ND_ADMIN_PASS", "pw-xyz")],
        );
        assert!(merged.contains("# operator creds (MEDIA-2)"));
        assert!(merged.contains("DO_SPACES_KEY=AKIA123"));
        assert!(merged.contains("DO_SPACES_SECRET=shh"));
        assert!(merged.contains("ND_ADMIN_USER=admin"), "replaced in place");
        assert!(!merged.contains("ND_ADMIN_USER=stale"));
        assert!(merged.contains("ND_ADMIN_PASS=pw-xyz"), "appended");
        // Idempotent — merging the same updates again is a fixed point.
        assert_eq!(
            merge_env_file(
                &merged,
                &[("ND_ADMIN_USER", "admin"), ("ND_ADMIN_PASS", "pw-xyz")]
            ),
            merged
        );
    }

    #[test]
    fn generated_passwords_are_url_safe_and_distinct() {
        let a = gen_account_password();
        let b = gen_account_password();
        assert_ne!(a, b, "CSPRNG output differs");
        assert!(
            a.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "no shell-hostile chars in {a}"
        );
    }

    // ── MEDIA-7 ──

    #[test]
    fn music_service_registers_only_on_a_serving_media_node() {
        // Registered + healthy on a live media LH.
        let row = music_service_row(true, true, Some("10.42.0.7")).expect("registered");
        assert_eq!(row["id"], "music");
        assert_eq!(row["port"], NAVIDROME_PORT);
        assert_eq!(row["overlay_ip"], "10.42.0.7");
        assert_eq!(row["is_publishable"], true, "serving ⇒ publishable");
        // Registered but container down ⇒ shown unhealthy (per-instance health).
        let down = music_service_row(true, false, Some("10.42.0.7")).expect("registered");
        assert_eq!(down["is_publishable"], false);
        // Registered but no overlay yet ⇒ not publishable.
        let pre = music_service_row(true, true, None).expect("registered");
        assert_eq!(pre["is_publishable"], false);
        assert_eq!(pre["overlay_ip"], serde_json::Value::Null);
        // De-registered on a non-media node ⇒ no stale entry.
        assert!(music_service_row(false, true, Some("10.42.0.7")).is_none());
        assert!(music_service_row(false, false, None).is_none());
    }

    // ── MEDIA-8 ──

    #[test]
    fn birthright_json_points_at_music_mesh_and_the_shared_account() {
        let body = music_birthright_json(MEDIA_ACCOUNT_USER, "s3cr3t-pw");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["server_url"], "http://music.mesh:4533");
        assert_eq!(v["username"], "admin");
        assert_eq!(v["password"], "s3cr3t-pw");
    }

    #[test]
    fn write_music_birthright_is_idempotent() {
        let tmp = std::env::temp_dir().join(format!("mde-birthright-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let path = tmp.join("sub").join("airsonic-creds.json"); // parent created
        write_music_birthright(&path, MEDIA_ACCOUNT_USER, "pw1").unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        // Re-running with the same account writes byte-identical content.
        write_music_birthright(&path, MEDIA_ACCOUNT_USER, "pw1").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first);
        // The written file is exactly what mde-musicd's creds loader expects.
        assert!(first.contains("\"server_url\""));
        assert!(first.contains("http://music.mesh:4533"));
        let _ = std::fs::remove_dir_all(&tmp);
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
}
