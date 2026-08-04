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
use sha2::{Digest as _, Sha256};

/// Minimal projection of `nebula-bundle.json` — we only need the
/// overlay IP. Serde ignores the bundle's other fields (cert PEMs,
/// lighthouses, etc.), so this stays decoupled from
/// [`crate::ca::bundle::NebulaBundle`]'s full shape.
#[derive(Deserialize)]
struct BundleOverlayIp {
    overlay_ip: String,
}

/// Replicated Nebula bundles contain PEM material and a lighthouse roster, so
/// leave room for a useful fleet bundle while keeping peer-controlled JSON
/// bounded before serde materializes it.
const MAX_NEBULA_BUNDLE_BYTES: usize = 4 * 1024 * 1024;

/// Media registrations are compact records. A smaller bound prevents a
/// malformed replicated registration from consuming an unreasonable amount of
/// memory before serde materializes it.
const MAX_MEDIA_REGISTRY_BYTES: usize = 256 * 1024;

/// Read one replicated JSON record through the descriptor that will actually
/// be consumed. Final symlinks and special files are rejected at open time;
/// the bounded read and post-read metadata check reject oversized or changing
/// input before a caller invokes serde.
fn read_bounded_media_json(path: &Path, max_bytes: usize) -> Option<String> {
    #[cfg(unix)]
    let file: std::fs::File = {
        use rustix::fs::{Mode, OFlags};

        rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .ok()?
        .into()
    };
    #[cfg(not(unix))]
    let file = {
        let metadata = std::fs::symlink_metadata(path).ok()?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return None;
        }
        std::fs::File::open(path).ok()?
    };

    let expected_len = file.metadata().ok()?.len();
    read_bounded_media_file(file, expected_len, max_bytes)
}

/// Consume an already-open replicated JSON record and verify that the regular
/// file stayed the same size throughout the bounded read. The explicit
/// `expected_len` seam makes the growth-race regression deterministic without
/// making production behavior depend on thread timing.
fn read_bounded_media_file(
    mut file: std::fs::File,
    expected_len: u64,
    max_bytes: usize,
) -> Option<String> {
    use std::io::Read as _;

    let before = file.metadata().ok()?;
    let max_bytes_u64 = u64::try_from(max_bytes).ok()?;
    if !before.file_type().is_file() || before.len() != expected_len || before.len() > max_bytes_u64
    {
        return None;
    }

    let capacity = usize::try_from(before.len())
        .unwrap_or(max_bytes)
        .min(max_bytes)
        .saturating_add(1);
    let mut bytes = Vec::with_capacity(capacity);
    (&mut file)
        .take(max_bytes_u64.saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;

    let after = file.metadata().ok()?;
    if !after.file_type().is_file()
        || after.len() != expected_len
        || bytes.len() > max_bytes
        || bytes.len() as u64 != after.len()
    {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// Airsonic / Subsonic media-server kind tag.
pub const KIND_AIRSONIC: &str = "airsonic";
/// Jellyfin media-server kind tag.
pub const KIND_JELLYFIN: &str = "jellyfin";

/// Default Airsonic/Subsonic port.
pub const AIRSONIC_PORT: u16 = 4040;
/// Default Jellyfin port.
pub const JELLYFIN_PORT: u16 = 8096;
/// Mesh Jellyfin gateway proxy port.
///
/// Kept separate from [`JELLYFIN_PORT`] so a node that already runs a real local
/// Jellyfin on 8096 does not collide with mackesd's gateway responder or get
/// mis-advertised as a direct Jellyfin instance by the descriptor probe.
pub const JELLYFIN_GATEWAY_PROXY_PORT: u16 = 8097;

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
        let Some(body) = read_bounded_media_json(&bundle_path, MAX_NEBULA_BUNDLE_BYTES) else {
            continue;
        };
        let Ok(bundle) = serde_json::from_str::<BundleOverlayIp>(&body) else {
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

/// File name that holds manually registered LAN AirSonic/Subsonic gateway
/// sources under the gateway node's replicated QNM-Shared directory. Unlike the
/// legacy [`MEDIA_REGISTRY_FILE`], this registry describes LAN upstreams that a
/// node proxies into the mesh; it never carries plaintext credentials.
pub const AIRSONIC_GATEWAY_REGISTRY_FILE: &str = "airsonic-gateway-registry.json";

/// File name that holds manually registered LAN Jellyfin gateway sources under
/// the gateway node's replicated QNM-Shared directory. These records describe a
/// gateway proxy source and a sealed credential/token reference, never plaintext
/// Jellyfin user credentials or API tokens.
pub const JELLYFIN_GATEWAY_REGISTRY_FILE: &str = "jellyfin-gateway-registry.json";

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

/// Gateway health for a LAN AirSonic/Subsonic upstream manually registered on a
/// mesh node. `healthy` means the gateway can currently reach the LAN server;
/// `degraded` keeps the source visible for diagnosis and cached metadata while
/// failover prefers a healthy source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayHealth {
    /// Gateway probe succeeded.
    Healthy,
    /// Gateway/upstream probe failed or timed out.
    Degraded,
}

impl GatewayHealth {
    /// `true` when clients should prefer this gateway for new playback.
    #[must_use]
    pub fn is_healthy(self) -> bool {
        matches!(self, Self::Healthy)
    }
}

/// A gateway node's replicated declaration of one LAN-reachable
/// AirSonic/Subsonic upstream. This is the authoritative source-registration
/// shape for WL-FUNC-014: it stores a canonical LAN URL and a sealed credential
/// reference, never username/password material.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AirsonicGatewayRegistration {
    /// Stable source id derived from gateway node + canonical upstream URL.
    pub id: String,
    /// Mesh node that can reach the LAN AirSonic server.
    pub gateway_node: String,
    /// Canonical LAN URL the gateway proxies to.
    pub upstream_url: String,
    /// Deduplication key. Today it is the canonical upstream URL, so the same
    /// LAN server registered through multiple gateways folds into one source.
    pub upstream_key: String,
    /// Secret-store reference for sealed read-only Subsonic credentials.
    pub credential_ref: String,
    /// Gateway/upstream reachability status.
    pub health: GatewayHealth,
    /// Whether this source is the mesh-wide default when no healthy
    /// user-selected source exists.
    pub mesh_default: bool,
}

impl AirsonicGatewayRegistration {
    /// Build a validated gateway registration. Blank `credential_ref` derives a
    /// stable secret path under `media/airsonic/<source-id>`.
    #[must_use]
    pub fn new(
        gateway_node: &str,
        upstream_url: &str,
        credential_ref: &str,
        health: GatewayHealth,
        mesh_default: bool,
    ) -> Option<Self> {
        let gateway_node = gateway_node.trim();
        if gateway_node.is_empty() || gateway_node.chars().any(char::is_whitespace) {
            return None;
        }

        let upstream_url = canonical_airsonic_upstream_url(upstream_url)?;
        if canonical_url_host(&upstream_url)?.eq_ignore_ascii_case(MUSIC_MESH_HOST) {
            return None;
        }

        let id = airsonic_gateway_source_id(gateway_node, &upstream_url)?;
        let credential_ref = credential_ref.trim();
        if credential_ref.chars().any(char::is_whitespace) {
            return None;
        }
        let credential_ref = if credential_ref.is_empty() {
            airsonic_gateway_secret_ref(&id)
        } else {
            credential_ref.to_owned()
        };

        Some(Self {
            id,
            gateway_node: gateway_node.to_owned(),
            upstream_key: upstream_url.clone(),
            upstream_url,
            credential_ref,
            health,
            mesh_default,
        })
    }

    /// Revalidate a deserialized registration before consumers trust its id,
    /// upstream key, or credential reference.
    #[must_use]
    pub fn validated(&self) -> Option<Self> {
        let expected = Self::new(
            &self.gateway_node,
            &self.upstream_url,
            &self.credential_ref,
            self.health,
            self.mesh_default,
        )?;
        if self.id != expected.id
            || self.gateway_node != expected.gateway_node
            || self.upstream_url != expected.upstream_url
            || self.upstream_key != expected.upstream_key
            || self.credential_ref != expected.credential_ref
        {
            return None;
        }
        Some(expected)
    }
}

/// Client-facing source published from a validated gateway registration. Clients
/// use `source_url` over the mesh and materialize credentials through
/// `credential_ref`; `upstream_url` remains diagnostic/admin-only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AirsonicGatewaySource {
    /// Stable source id.
    pub id: String,
    /// Always [`KIND_AIRSONIC`].
    pub kind: String,
    /// Mesh node proxying the LAN upstream.
    pub gateway_node: String,
    /// Mesh-reachable gateway proxy URL.
    pub source_url: String,
    /// Canonical LAN upstream URL, retained for admin diagnostics.
    pub upstream_url: String,
    /// Upstream dedupe key.
    pub upstream_key: String,
    /// Secret-store reference for sealed read-only Subsonic credentials.
    pub credential_ref: String,
    /// Gateway/upstream reachability status.
    pub health: GatewayHealth,
    /// Mesh-wide default marker.
    pub mesh_default: bool,
}

/// A gateway node's replicated declaration of one LAN-reachable Jellyfin
/// upstream. This is the WL-FUNC-015 sibling to
/// [`AirsonicGatewayRegistration`]: canonical LAN URL, gateway health, default
/// marker, and a sealed credential/token reference only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JellyfinGatewayRegistration {
    /// Stable source id derived from gateway node + canonical upstream URL.
    pub id: String,
    /// Mesh node that can reach the LAN Jellyfin server.
    pub gateway_node: String,
    /// Canonical LAN URL the gateway proxies to.
    pub upstream_url: String,
    /// Deduplication key. Today it is the canonical upstream URL, so the same
    /// LAN server registered through multiple gateways folds into one source.
    pub upstream_key: String,
    /// Secret-store reference for sealed read-only Jellyfin credentials/token.
    pub credential_ref: String,
    /// Gateway/upstream reachability status.
    pub health: GatewayHealth,
    /// Whether this source is the mesh-wide default when no healthy
    /// user-selected source exists.
    pub mesh_default: bool,
}

impl JellyfinGatewayRegistration {
    /// Build a validated gateway registration. Blank `credential_ref` derives a
    /// stable secret path under `media/jellyfin/<source-id>`.
    #[must_use]
    pub fn new(
        gateway_node: &str,
        upstream_url: &str,
        credential_ref: &str,
        health: GatewayHealth,
        mesh_default: bool,
    ) -> Option<Self> {
        let gateway_node = gateway_node.trim();
        if gateway_node.is_empty() || gateway_node.chars().any(char::is_whitespace) {
            return None;
        }

        let upstream_url = canonical_jellyfin_upstream_url(upstream_url)?;
        if canonical_url_host(&upstream_url)?.eq_ignore_ascii_case(MUSIC_MESH_HOST) {
            return None;
        }

        let id = jellyfin_gateway_source_id(gateway_node, &upstream_url)?;
        let credential_ref = credential_ref.trim();
        if credential_ref.chars().any(char::is_whitespace) {
            return None;
        }
        let credential_ref = if credential_ref.is_empty() {
            jellyfin_gateway_secret_ref(&id)
        } else {
            credential_ref.to_owned()
        };

        Some(Self {
            id,
            gateway_node: gateway_node.to_owned(),
            upstream_key: upstream_url.clone(),
            upstream_url,
            credential_ref,
            health,
            mesh_default,
        })
    }

    /// Revalidate a deserialized registration before consumers trust its id,
    /// upstream key, or credential reference.
    #[must_use]
    pub fn validated(&self) -> Option<Self> {
        let expected = Self::new(
            &self.gateway_node,
            &self.upstream_url,
            &self.credential_ref,
            self.health,
            self.mesh_default,
        )?;
        if self.id != expected.id
            || self.gateway_node != expected.gateway_node
            || self.upstream_url != expected.upstream_url
            || self.upstream_key != expected.upstream_key
            || self.credential_ref != expected.credential_ref
        {
            return None;
        }
        Some(expected)
    }
}

/// Client-facing Jellyfin source published from a validated gateway
/// registration. Clients use `source_url` over the mesh and materialize the
/// shared read-only credential/token through `credential_ref`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JellyfinGatewaySource {
    /// Stable source id.
    pub id: String,
    /// Always [`KIND_JELLYFIN`].
    pub kind: String,
    /// Mesh node proxying the LAN upstream.
    pub gateway_node: String,
    /// Mesh-reachable gateway proxy URL.
    pub source_url: String,
    /// Canonical LAN upstream URL, retained for admin diagnostics.
    pub upstream_url: String,
    /// Upstream dedupe key.
    pub upstream_key: String,
    /// Secret-store reference for sealed read-only Jellyfin credentials/token.
    pub credential_ref: String,
    /// Gateway/upstream reachability status.
    pub health: GatewayHealth,
    /// Mesh-wide default marker.
    pub mesh_default: bool,
}

/// Canonicalize an AirSonic/Subsonic LAN upstream URL for stable ids and
/// deduplication. Scheme and authority are lowercase, a missing scheme defaults
/// to `http`, and trailing slashes are removed. Userinfo, query strings, and
/// fragments are rejected so credentials and non-canonical selectors cannot
/// leak into replicated source ids.
#[must_use]
pub fn canonical_airsonic_upstream_url(raw: &str) -> Option<String> {
    canonical_media_upstream_url(raw)
}

/// Canonicalize a Jellyfin LAN upstream URL for stable ids and deduplication.
/// This intentionally matches the AirSonic gateway URL contract so the two media
/// gateway types reject userinfo/query/fragment leakage consistently.
#[must_use]
pub fn canonical_jellyfin_upstream_url(raw: &str) -> Option<String> {
    canonical_media_upstream_url(raw)
}

fn canonical_media_upstream_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.chars().any(char::is_whitespace) {
        return None;
    }
    let with_scheme = if raw.contains("://") {
        raw.to_owned()
    } else {
        format!("http://{raw}")
    };
    if with_scheme.contains('@') || with_scheme.contains('?') || with_scheme.contains('#') {
        return None;
    }
    let (scheme, rest) = with_scheme.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https") {
        return None;
    }
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let authority = rest[..authority_end].to_ascii_lowercase();
    if authority.is_empty() || authority.starts_with(':') {
        return None;
    }
    let path = rest[authority_end..].trim_end_matches('/');
    let mut out = format!("{scheme}://{authority}");
    if !path.is_empty() {
        out.push_str(path);
    }
    Some(out)
}

/// Stable AirSonic gateway source id derived from gateway node + canonical
/// upstream. The source id does not include plaintext credentials.
#[must_use]
pub fn airsonic_gateway_source_id(gateway_node: &str, upstream_url: &str) -> Option<String> {
    let gateway_node = gateway_node.trim();
    if gateway_node.is_empty() || gateway_node.chars().any(char::is_whitespace) {
        return None;
    }
    let upstream_url = canonical_airsonic_upstream_url(upstream_url)?;
    let digest = short_hash_hex(&[gateway_node, &upstream_url]);
    Some(format!(
        "airsonic-{}-{digest}",
        safe_media_token(gateway_node)
    ))
}

/// Default sealed credential location for a gateway AirSonic source.
#[must_use]
pub fn airsonic_gateway_secret_ref(source_id: &str) -> String {
    format!("media/airsonic/{}", safe_media_token(source_id))
}

/// Stable Jellyfin gateway source id derived from gateway node + canonical
/// upstream. The source id does not include plaintext credentials or tokens.
#[must_use]
pub fn jellyfin_gateway_source_id(gateway_node: &str, upstream_url: &str) -> Option<String> {
    let gateway_node = gateway_node.trim();
    if gateway_node.is_empty() || gateway_node.chars().any(char::is_whitespace) {
        return None;
    }
    let upstream_url = canonical_jellyfin_upstream_url(upstream_url)?;
    let digest = short_hash_hex(&[gateway_node, &upstream_url]);
    Some(format!(
        "jellyfin-{}-{digest}",
        safe_media_token(gateway_node)
    ))
}

/// Default sealed credential/token location for a gateway Jellyfin source.
#[must_use]
pub fn jellyfin_gateway_secret_ref(source_id: &str) -> String {
    format!("media/jellyfin/{}", safe_media_token(source_id))
}

/// Build a client-facing mesh source from a validated gateway registration.
#[must_use]
pub fn source_from_airsonic_gateway(
    registration: &AirsonicGatewayRegistration,
) -> Option<AirsonicGatewaySource> {
    let registration = registration.validated()?;
    let gateway_host = format!("{}.mesh", safe_media_token(&registration.gateway_node));
    Some(AirsonicGatewaySource {
        id: registration.id.clone(),
        kind: KIND_AIRSONIC.to_owned(),
        gateway_node: registration.gateway_node.clone(),
        source_url: format!(
            "http://{gateway_host}:{AIRSONIC_PORT}/mde/airsonic/{}",
            registration.id
        ),
        upstream_url: registration.upstream_url.clone(),
        upstream_key: registration.upstream_key.clone(),
        credential_ref: registration.credential_ref.clone(),
        health: registration.health,
        mesh_default: registration.mesh_default,
    })
}

/// Build a client-facing mesh source from a validated Jellyfin gateway
/// registration.
#[must_use]
pub fn source_from_jellyfin_gateway(
    registration: &JellyfinGatewayRegistration,
) -> Option<JellyfinGatewaySource> {
    let registration = registration.validated()?;
    let gateway_host = format!("{}.mesh", safe_media_token(&registration.gateway_node));
    Some(JellyfinGatewaySource {
        id: registration.id.clone(),
        kind: KIND_JELLYFIN.to_owned(),
        gateway_node: registration.gateway_node.clone(),
        source_url: format!(
            "http://{gateway_host}:{JELLYFIN_GATEWAY_PROXY_PORT}/mde/jellyfin/{}",
            registration.id
        ),
        upstream_url: registration.upstream_url.clone(),
        upstream_key: registration.upstream_key.clone(),
        credential_ref: registration.credential_ref.clone(),
        health: registration.health,
        mesh_default: registration.mesh_default,
    })
}

/// Fold all gateway registrations into client sources. The same upstream server
/// dedupes by canonical URL; a healthy gateway wins over degraded, mesh default
/// wins within the same health tier, and ids provide deterministic tie-breaks.
#[must_use]
pub fn merge_airsonic_gateway_sources(
    registrations: &[AirsonicGatewayRegistration],
) -> Vec<AirsonicGatewaySource> {
    let mut by_upstream: std::collections::BTreeMap<String, AirsonicGatewaySource> =
        std::collections::BTreeMap::new();
    for registration in registrations {
        let Some(candidate) = source_from_airsonic_gateway(registration) else {
            continue;
        };
        by_upstream
            .entry(candidate.upstream_key.clone())
            .and_modify(|current| {
                if airsonic_gateway_source_wins(&candidate, current) {
                    *current = candidate.clone();
                }
            })
            .or_insert(candidate);
    }

    let mut out: Vec<_> = by_upstream.into_values().collect();
    out.sort_by(|a, b| {
        b.mesh_default
            .cmp(&a.mesh_default)
            .then_with(|| b.health.is_healthy().cmp(&a.health.is_healthy()))
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

/// Fold all Jellyfin gateway registrations into client sources. The same
/// upstream server dedupes by canonical URL; a healthy gateway wins over
/// degraded, mesh default wins within the same health tier, and ids provide
/// deterministic tie-breaks.
#[must_use]
pub fn merge_jellyfin_gateway_sources(
    registrations: &[JellyfinGatewayRegistration],
) -> Vec<JellyfinGatewaySource> {
    let mut by_upstream: std::collections::BTreeMap<String, JellyfinGatewaySource> =
        std::collections::BTreeMap::new();
    for registration in registrations {
        let Some(candidate) = source_from_jellyfin_gateway(registration) else {
            continue;
        };
        by_upstream
            .entry(candidate.upstream_key.clone())
            .and_modify(|current| {
                if jellyfin_gateway_source_wins(&candidate, current) {
                    *current = candidate.clone();
                }
            })
            .or_insert(candidate);
    }

    let mut out: Vec<_> = by_upstream.into_values().collect();
    out.sort_by(|a, b| {
        b.mesh_default
            .cmp(&a.mesh_default)
            .then_with(|| b.health.is_healthy().cmp(&a.health.is_healthy()))
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

/// Select the AirSonic source a Music client should use now: keep the user's
/// last selected source only while it is healthy, otherwise fall back to the
/// healthy mesh default, then any healthy source, then visible degraded default.
#[must_use]
pub fn select_airsonic_gateway_source<'a>(
    sources: &'a [AirsonicGatewaySource],
    last_selected: Option<&str>,
) -> Option<&'a AirsonicGatewaySource> {
    if let Some(last_selected) = last_selected.map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(source) = sources
            .iter()
            .find(|source| source.id == last_selected && source.health.is_healthy())
        {
            return Some(source);
        }
    }

    sources
        .iter()
        .find(|source| source.mesh_default && source.health.is_healthy())
        .or_else(|| sources.iter().find(|source| source.health.is_healthy()))
        .or_else(|| sources.iter().find(|source| source.mesh_default))
        .or_else(|| sources.first())
}

/// Select the Jellyfin source a Media Workspace client should use now: keep the
/// user's last selected source only while it is healthy, otherwise fall back to
/// the healthy mesh default, then any healthy source, then visible degraded
/// default.
#[must_use]
pub fn select_jellyfin_gateway_source<'a>(
    sources: &'a [JellyfinGatewaySource],
    last_selected: Option<&str>,
) -> Option<&'a JellyfinGatewaySource> {
    if let Some(last_selected) = last_selected.map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(source) = sources
            .iter()
            .find(|source| source.id == last_selected && source.health.is_healthy())
        {
            return Some(source);
        }
    }

    sources
        .iter()
        .find(|source| source.mesh_default && source.health.is_healthy())
        .or_else(|| sources.iter().find(|source| source.health.is_healthy()))
        .or_else(|| sources.iter().find(|source| source.mesh_default))
        .or_else(|| sources.first())
}

/// Read all replicated AirSonic gateway declarations from the QNM-Shared plane
/// and return the deduped, client-facing source list.
#[must_use]
pub fn read_airsonic_gateway_sources_from_plane(
    workgroup_root: &Path,
) -> Vec<AirsonicGatewaySource> {
    let entries = match std::fs::read_dir(workgroup_root) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let mut registrations = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path().join(AIRSONIC_GATEWAY_REGISTRY_FILE);
        let Some(body) = read_bounded_media_json(&path, MAX_MEDIA_REGISTRY_BYTES) else {
            continue;
        };
        registrations.extend(parse_airsonic_gateway_registrations(&body));
    }
    merge_airsonic_gateway_sources(&registrations)
}

/// Read all replicated Jellyfin gateway declarations from the QNM-Shared plane
/// and return the deduped, client-facing source list.
#[must_use]
pub fn read_jellyfin_gateway_sources_from_plane(
    workgroup_root: &Path,
) -> Vec<JellyfinGatewaySource> {
    let entries = match std::fs::read_dir(workgroup_root) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let mut registrations = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path().join(JELLYFIN_GATEWAY_REGISTRY_FILE);
        let Some(body) = read_bounded_media_json(&path, MAX_MEDIA_REGISTRY_BYTES) else {
            continue;
        };
        registrations.extend(parse_jellyfin_gateway_registrations(&body));
    }
    merge_jellyfin_gateway_sources(&registrations)
}

fn parse_airsonic_gateway_registrations(body: &str) -> Vec<AirsonicGatewayRegistration> {
    if let Ok(registrations) = serde_json::from_str::<Vec<AirsonicGatewayRegistration>>(body) {
        return registrations
            .into_iter()
            .filter_map(|registration| registration.validated())
            .collect();
    }
    serde_json::from_str::<AirsonicGatewayRegistration>(body)
        .ok()
        .and_then(|registration| registration.validated())
        .into_iter()
        .collect()
}

fn parse_jellyfin_gateway_registrations(body: &str) -> Vec<JellyfinGatewayRegistration> {
    if let Ok(registrations) = serde_json::from_str::<Vec<JellyfinGatewayRegistration>>(body) {
        return registrations
            .into_iter()
            .filter_map(|registration| registration.validated())
            .collect();
    }
    serde_json::from_str::<JellyfinGatewayRegistration>(body)
        .ok()
        .and_then(|registration| registration.validated())
        .into_iter()
        .collect()
}

fn airsonic_gateway_source_wins(
    candidate: &AirsonicGatewaySource,
    current: &AirsonicGatewaySource,
) -> bool {
    candidate
        .health
        .is_healthy()
        .cmp(&current.health.is_healthy())
        .then_with(|| candidate.mesh_default.cmp(&current.mesh_default))
        .then_with(|| current.id.cmp(&candidate.id))
        .is_gt()
}

fn jellyfin_gateway_source_wins(
    candidate: &JellyfinGatewaySource,
    current: &JellyfinGatewaySource,
) -> bool {
    candidate
        .health
        .is_healthy()
        .cmp(&current.health.is_healthy())
        .then_with(|| candidate.mesh_default.cmp(&current.mesh_default))
        .then_with(|| current.id.cmp(&candidate.id))
        .is_gt()
}

fn canonical_url_host(canonical_url: &str) -> Option<&str> {
    let (_, rest) = canonical_url.split_once("://")?;
    let authority = rest.split('/').next().unwrap_or(rest);
    if let Some(after_bracket) = authority.strip_prefix('[') {
        return after_bracket.split_once(']').map(|(host, _)| host);
    }
    authority
        .rsplit_once(':')
        .filter(|(_, port)| port.chars().all(|c| c.is_ascii_digit()))
        .map(|(host, _)| host)
        .or(Some(authority))
}

fn safe_media_token(raw: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in raw.trim().chars() {
        let mapped = if ch.is_ascii_alphanumeric() {
            Some(ch.to_ascii_lowercase())
        } else if matches!(ch, '.' | '-' | '_') {
            Some(if ch == '_' { '-' } else { ch })
        } else {
            None
        };
        match mapped {
            Some(ch) => {
                out.push(ch);
                last_dash = false;
            }
            None if !last_dash && !out.is_empty() => {
                out.push('-');
                last_dash = true;
            }
            None => {}
        }
    }
    while out.starts_with(['-', '.']) {
        out.remove(0);
    }
    while out.ends_with(['-', '.']) {
        out.pop();
    }
    if out.is_empty() {
        "node".to_owned()
    } else {
        out
    }
}

fn short_hash_hex(parts: &[&str]) -> String {
    use std::fmt::Write as _;

    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    let digest = hasher.finalize();
    let mut out = String::with_capacity(16);
    for byte in digest.iter().take(8) {
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
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

/// Operator-configured media protocol accepted by the native Media surface.
/// The endpoint is Airsonic/Subsonic-compatible; credentials are resolved from
/// `credential_ref` and are never part of this replicated record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaServerKind {
    /// Airsonic, Navidrome, and other Subsonic-compatible servers.
    Airsonic,
    /// Navidrome's native name for the same Subsonic-compatible API.
    Navidrome,
    /// Generic Subsonic-compatible implementation.
    Subsonic,
}

/// Health state for an operator-configured media endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaServerHealth {
    /// Endpoint answered its bounded probe.
    Healthy,
    /// Endpoint was reachable but did not complete a healthy probe.
    Degraded,
    /// Endpoint is configured but unavailable.
    Unavailable,
}

/// Bounded operator-configured media endpoint record.
///
/// This is deliberately separate from [`MediaRegistration`]. Existing
/// `MediaRegistration` JSON remains readable, while new records have no
/// username/password fields and carry only a secret-store reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaServerRecord {
    /// Canonical HTTP(S) Airsonic-compatible endpoint without userinfo/query.
    pub endpoint: String,
    /// Protocol kind.
    pub kind: MediaServerKind,
    /// Lower values are preferred by consumers.
    pub priority: u16,
    /// Current endpoint health.
    pub health: MediaServerHealth,
    /// Last bounded probe latency in milliseconds, when measured.
    pub latency: Option<u32>,
    /// Secret-store reference; never a username, password, or token.
    pub credential_ref: String,
}

impl MediaServerRecord {
    /// Construct a validated operator record. Empty `credential_ref` is valid
    /// for an endpoint that does not require authentication; a non-empty value
    /// must identify a secret-store entry rather than contain secret material.
    #[must_use]
    pub fn new(
        endpoint: &str,
        kind: MediaServerKind,
        priority: u16,
        health: MediaServerHealth,
        latency: Option<u32>,
        credential_ref: &str,
    ) -> Option<Self> {
        let endpoint = canonical_airsonic_upstream_url(endpoint)?;
        if latency.is_some_and(|value| value > 60_000) {
            return None;
        }
        let credential_ref = credential_ref.trim();
        if !valid_media_credential_ref(credential_ref) {
            return None;
        }
        Some(Self {
            endpoint,
            kind,
            priority,
            health,
            latency,
            credential_ref: credential_ref.to_owned(),
        })
    }

    /// Revalidate a deserialized record before publishing or consuming it.
    #[must_use]
    pub fn validated(&self) -> Option<Self> {
        let expected = Self::new(
            &self.endpoint,
            self.kind,
            self.priority,
            self.health,
            self.latency,
            &self.credential_ref,
        )?;
        (self == &expected).then_some(expected)
    }
}

/// Environment variable carrying an operator-supplied JSON object or array of
/// [`MediaServerRecord`] values. It is bounded and parsed as data, never shell
/// evaluated.
pub const MEDIA_SERVER_RECORDS_ENV: &str = "MDE_MEDIA_SERVER_RECORDS";
const MAX_MEDIA_SERVER_RECORDS_BYTES: usize = 64 * 1024;

fn valid_media_credential_ref(value: &str) -> bool {
    value.is_empty()
        || (value.starts_with("media/") || value.starts_with("secret:"))
            && !value
                .chars()
                .any(|c| c.is_whitespace() || c == '=' || c == '@')
}

/// Parse operator-configured media records. Accepts either one object or an
/// array, filters invalid records, and never returns plaintext credential data.
#[must_use]
pub fn parse_media_server_records(body: &str) -> Vec<MediaServerRecord> {
    if body.len() > MAX_MEDIA_SERVER_RECORDS_BYTES {
        return Vec::new();
    }
    if let Ok(records) = serde_json::from_str::<Vec<MediaServerRecord>>(body) {
        return records
            .into_iter()
            .filter_map(|record| record.validated())
            .collect();
    }
    if let Some(record) = serde_json::from_str::<MediaServerRecord>(body)
        .ok()
        .and_then(|record| record.validated())
    {
        return vec![record];
    }
    let legacy = if let Ok(records) = serde_json::from_str::<Vec<MediaRegistration>>(body) {
        records
    } else if let Ok(record) = serde_json::from_str::<MediaRegistration>(body) {
        vec![record]
    } else {
        return Vec::new();
    };
    legacy
        .into_iter()
        .filter_map(|record| {
            let endpoint = format!(
                "http://{}:{}",
                safe_media_token(&record.node_id),
                record.port
            );
            let health = match record.health.as_str() {
                HEALTH_UP => MediaServerHealth::Healthy,
                HEALTH_DOWN => MediaServerHealth::Unavailable,
                _ => MediaServerHealth::Degraded,
            };
            // Never migrate the legacy SharedAccount username/password. The
            // new record points to the sealed location derived for this peer.
            MediaServerRecord::new(
                &endpoint,
                MediaServerKind::Airsonic,
                0,
                health,
                None,
                &format!("media/airsonic/{}", safe_media_token(&record.node_id)),
            )
        })
        .collect()
}

/// Read the operator record environment without treating it as a secret body.
#[must_use]
pub fn operator_media_server_records() -> Vec<MediaServerRecord> {
    std::env::var(MEDIA_SERVER_RECORDS_ENV)
        .ok()
        .map_or_else(Vec::new, |body| parse_media_server_records(&body))
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
        let Some(body) = read_bounded_media_json(&path, MAX_MEDIA_REGISTRY_BYTES) else {
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

    #[test]
    fn peer_overlay_ips_skips_hostile_bundle_inputs() {
        let tmp = tempfile::tempdir().unwrap();
        let valid = tmp
            .path()
            .join("valid-peer")
            .join("mackesd")
            .join("nebula-bundle.json");
        std::fs::create_dir_all(valid.parent().unwrap()).unwrap();
        std::fs::write(&valid, br#"{"overlay_ip":"10.42.0.5"}"#).unwrap();

        let invalid_utf8 = tmp
            .path()
            .join("invalid-utf8")
            .join("mackesd")
            .join("nebula-bundle.json");
        std::fs::create_dir_all(invalid_utf8.parent().unwrap()).unwrap();
        std::fs::write(&invalid_utf8, [0xff, 0xfe, 0xfd]).unwrap();

        let oversized = tmp
            .path()
            .join("oversized")
            .join("mackesd")
            .join("nebula-bundle.json");
        std::fs::create_dir_all(oversized.parent().unwrap()).unwrap();
        std::fs::write(&oversized, vec![b'x'; MAX_NEBULA_BUNDLE_BYTES + 1]).unwrap();

        let directory = tmp
            .path()
            .join("directory")
            .join("mackesd")
            .join("nebula-bundle.json");
        std::fs::create_dir_all(directory.parent().unwrap()).unwrap();
        std::fs::create_dir(&directory).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            use std::process::Command;

            let linked = tmp
                .path()
                .join("symlink")
                .join("mackesd")
                .join("nebula-bundle.json");
            std::fs::create_dir_all(linked.parent().unwrap()).unwrap();
            symlink(&valid, &linked).unwrap();

            let fifo = tmp
                .path()
                .join("fifo")
                .join("mackesd")
                .join("nebula-bundle.json");
            std::fs::create_dir_all(fifo.parent().unwrap()).unwrap();
            assert!(Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success());
        }

        assert_eq!(
            peer_overlay_ips(tmp.path()),
            vec![("valid-peer".to_string(), "10.42.0.5".to_string())]
        );
    }

    #[test]
    fn bounded_media_json_reader_rejects_growth() {
        use std::io::Write as _;

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("growth.json");
        let body = br#"{"overlay_ip":"10.42.0.5"}"#;
        std::fs::write(&path, body).unwrap();
        let file = std::fs::File::open(&path).unwrap();
        let expected_len = file.metadata().unwrap().len();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b" ")
            .unwrap();

        assert!(read_bounded_media_file(file, expected_len, MAX_NEBULA_BUNDLE_BYTES).is_none());
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

    // ── WL-FUNC-014: LAN AirSonic gateways ──

    #[test]
    fn gateway_health_wire_shape_round_trips() {
        assert_eq!(
            serde_json::to_string(&GatewayHealth::Healthy).unwrap(),
            "\"healthy\""
        );
        assert_eq!(
            serde_json::from_str::<GatewayHealth>("\"degraded\"").unwrap(),
            GatewayHealth::Degraded
        );
    }

    #[test]
    fn canonical_airsonic_upstream_url_normalizes_scheme_host_and_slash() {
        assert_eq!(
            canonical_airsonic_upstream_url("NAS.LAN:4040/"),
            Some("http://nas.lan:4040".to_owned())
        );
        assert_eq!(
            canonical_airsonic_upstream_url("HTTPS://NAS.LAN:4040/music///"),
            Some("https://nas.lan:4040/music".to_owned())
        );
        assert_eq!(canonical_airsonic_upstream_url("ftp://nas.lan:4040"), None);
        assert_eq!(
            canonical_airsonic_upstream_url("http://user:pass@nas.lan:4040"),
            None
        );
        assert_eq!(
            canonical_airsonic_upstream_url("http://nas.lan:4040?token=x"),
            None
        );
    }

    #[test]
    fn airsonic_gateway_source_uses_gateway_proxy_not_music_mesh() {
        let reg = AirsonicGatewayRegistration::new(
            "Seat-15",
            "HTTP://NAS.LAN:4040/",
            "",
            GatewayHealth::Healthy,
            true,
        )
        .unwrap();
        let source = source_from_airsonic_gateway(&reg).unwrap();

        assert_eq!(source.kind, KIND_AIRSONIC);
        assert_eq!(source.gateway_node, "Seat-15");
        assert_eq!(source.upstream_url, "http://nas.lan:4040");
        assert_eq!(
            source.source_url,
            format!("http://seat-15.mesh:4040/mde/airsonic/{}", source.id)
        );
        assert!(!source.source_url.contains(MUSIC_MESH_HOST));
        assert_ne!(source.source_url, music_mesh_server_url());
        assert_eq!(
            source.credential_ref,
            airsonic_gateway_secret_ref(&source.id)
        );
        assert!(source.mesh_default);
        assert!(source.health.is_healthy());
    }

    #[test]
    fn airsonic_gateway_registration_serializes_no_plaintext_secret() {
        let reg = AirsonicGatewayRegistration::new(
            "gateway-a",
            "http://nas.lan:4040",
            "media/airsonic/shared-readonly",
            GatewayHealth::Healthy,
            false,
        )
        .unwrap();
        let json = serde_json::to_string(&reg).unwrap();

        assert!(json.contains("\"credential_ref\":\"media/airsonic/shared-readonly\""));
        assert!(!json.contains("hunter2"));
        assert!(!json.contains("password"));
        assert!(!json.contains("username"));
    }

    #[test]
    fn legacy_music_mesh_url_is_not_a_gateway_registration() {
        assert!(AirsonicGatewayRegistration::new(
            "gateway-a",
            &music_mesh_server_url(),
            "",
            GatewayHealth::Healthy,
            true,
        )
        .is_none());
    }

    #[test]
    fn airsonic_gateway_merge_dedupes_same_upstream_and_prefers_healthy_gateway() {
        let degraded_default = AirsonicGatewayRegistration::new(
            "gateway-b",
            "HTTP://NAS.LAN:4040/",
            "",
            GatewayHealth::Degraded,
            true,
        )
        .unwrap();
        let healthy_non_default = AirsonicGatewayRegistration::new(
            "gateway-a",
            "http://nas.lan:4040",
            "",
            GatewayHealth::Healthy,
            false,
        )
        .unwrap();

        let sources = merge_airsonic_gateway_sources(&[degraded_default, healthy_non_default]);

        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].gateway_node, "gateway-a");
        assert!(sources[0].health.is_healthy());
        assert_eq!(sources[0].upstream_key, "http://nas.lan:4040");
    }

    #[test]
    fn airsonic_gateway_select_keeps_healthy_last_then_defaults() {
        let mesh_default = AirsonicGatewayRegistration::new(
            "gateway-default",
            "http://default.lan:4040",
            "",
            GatewayHealth::Healthy,
            true,
        )
        .unwrap();
        let preferred_last = AirsonicGatewayRegistration::new(
            "gateway-last",
            "http://last.lan:4040",
            "",
            GatewayHealth::Healthy,
            false,
        )
        .unwrap();
        let degraded_last = AirsonicGatewayRegistration::new(
            "gateway-degraded",
            "http://degraded.lan:4040",
            "",
            GatewayHealth::Degraded,
            false,
        )
        .unwrap();
        let default_id = mesh_default.id.clone();
        let preferred_id = preferred_last.id.clone();
        let degraded_id = degraded_last.id.clone();
        let sources =
            merge_airsonic_gateway_sources(&[mesh_default, preferred_last, degraded_last]);

        assert_eq!(
            select_airsonic_gateway_source(&sources, Some(&preferred_id))
                .unwrap()
                .id,
            preferred_id
        );
        assert_eq!(
            select_airsonic_gateway_source(&sources, Some(&degraded_id))
                .unwrap()
                .id,
            default_id
        );
        assert_eq!(
            select_airsonic_gateway_source(&sources, None).unwrap().id,
            default_id
        );
    }

    #[test]
    fn read_airsonic_gateway_sources_from_plane_reads_single_and_list_docs() {
        let tmp = tempfile::tempdir().unwrap();
        let single = AirsonicGatewayRegistration::new(
            "gateway-a",
            "http://nas-a.lan:4040",
            "",
            GatewayHealth::Healthy,
            true,
        )
        .unwrap();
        seed_airsonic_gateway_doc(
            tmp.path(),
            "gateway-a",
            &serde_json::to_string(&single).unwrap(),
        );

        let list = vec![
            AirsonicGatewayRegistration::new(
                "gateway-b",
                "http://nas-a.lan:4040/",
                "",
                GatewayHealth::Degraded,
                false,
            )
            .unwrap(),
            AirsonicGatewayRegistration::new(
                "gateway-c",
                "http://nas-c.lan:4040",
                "",
                GatewayHealth::Healthy,
                false,
            )
            .unwrap(),
        ];
        seed_airsonic_gateway_doc(
            tmp.path(),
            "gateway-b",
            &serde_json::to_string(&list).unwrap(),
        );

        let mut tampered = single.clone();
        tampered.id = "forged".to_owned();
        seed_airsonic_gateway_doc(
            tmp.path(),
            "tampered",
            &serde_json::to_string(&tampered).unwrap(),
        );

        let sources = read_airsonic_gateway_sources_from_plane(tmp.path());

        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].id, single.id);
        assert_eq!(sources[0].gateway_node, "gateway-a");
        assert!(sources
            .iter()
            .any(|source| source.gateway_node == "gateway-c"));
        assert!(!sources.iter().any(|source| source.id == "forged"));
    }

    // ── WL-FUNC-015: LAN Jellyfin gateways ──

    #[test]
    fn canonical_jellyfin_upstream_url_normalizes_like_other_gateway_media() {
        assert_eq!(
            canonical_jellyfin_upstream_url("JELLYFIN.LAN:8096/"),
            Some("http://jellyfin.lan:8096".to_owned())
        );
        assert_eq!(
            canonical_jellyfin_upstream_url("HTTPS://JELLYFIN.LAN:8920/base///"),
            Some("https://jellyfin.lan:8920/base".to_owned())
        );
        assert_eq!(canonical_jellyfin_upstream_url("ftp://jellyfin.lan"), None);
        assert_eq!(
            canonical_jellyfin_upstream_url("http://token@jellyfin.lan:8096"),
            None
        );
    }

    #[test]
    fn jellyfin_gateway_source_uses_gateway_proxy_not_direct_lan() {
        let reg = JellyfinGatewayRegistration::new(
            "Seat-15",
            "HTTP://JELLYFIN.LAN:8096/",
            "",
            GatewayHealth::Healthy,
            true,
        )
        .unwrap();
        let source = source_from_jellyfin_gateway(&reg).unwrap();

        assert_eq!(source.kind, KIND_JELLYFIN);
        assert_eq!(source.gateway_node, "Seat-15");
        assert_eq!(source.upstream_url, "http://jellyfin.lan:8096");
        assert_eq!(
            source.source_url,
            format!(
                "http://seat-15.mesh:{JELLYFIN_GATEWAY_PROXY_PORT}/mde/jellyfin/{}",
                source.id
            )
        );
        assert!(!source.source_url.contains("jellyfin.lan"));
        assert_eq!(
            source.credential_ref,
            jellyfin_gateway_secret_ref(&source.id)
        );
        assert!(source.mesh_default);
        assert!(source.health.is_healthy());
    }

    #[test]
    fn jellyfin_gateway_registration_serializes_no_plaintext_token() {
        let reg = JellyfinGatewayRegistration::new(
            "gateway-a",
            "http://jellyfin.lan:8096",
            "media/jellyfin/shared-readonly",
            GatewayHealth::Healthy,
            false,
        )
        .unwrap();
        let json = serde_json::to_string(&reg).unwrap();

        assert!(json.contains("\"credential_ref\":\"media/jellyfin/shared-readonly\""));
        assert!(!json.contains("hunter2"));
        assert!(!json.contains("password"));
        assert!(!json.contains("api_key"));
        assert!(!json.contains("token"));
    }

    #[test]
    fn legacy_music_mesh_url_is_not_a_jellyfin_gateway_registration() {
        assert!(JellyfinGatewayRegistration::new(
            "gateway-a",
            &music_mesh_server_url(),
            "",
            GatewayHealth::Healthy,
            true,
        )
        .is_none());
    }

    #[test]
    fn jellyfin_gateway_merge_dedupes_same_upstream_and_prefers_healthy_gateway() {
        let degraded_default = JellyfinGatewayRegistration::new(
            "gateway-b",
            "HTTP://JELLYFIN.LAN:8096/",
            "",
            GatewayHealth::Degraded,
            true,
        )
        .unwrap();
        let healthy_non_default = JellyfinGatewayRegistration::new(
            "gateway-a",
            "http://jellyfin.lan:8096",
            "",
            GatewayHealth::Healthy,
            false,
        )
        .unwrap();

        let sources = merge_jellyfin_gateway_sources(&[degraded_default, healthy_non_default]);

        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].gateway_node, "gateway-a");
        assert!(sources[0].health.is_healthy());
        assert_eq!(sources[0].upstream_key, "http://jellyfin.lan:8096");
    }

    #[test]
    fn jellyfin_gateway_select_keeps_healthy_last_then_defaults() {
        let mesh_default = JellyfinGatewayRegistration::new(
            "gateway-default",
            "http://default-jellyfin.lan:8096",
            "",
            GatewayHealth::Healthy,
            true,
        )
        .unwrap();
        let preferred_last = JellyfinGatewayRegistration::new(
            "gateway-last",
            "http://last-jellyfin.lan:8096",
            "",
            GatewayHealth::Healthy,
            false,
        )
        .unwrap();
        let degraded_last = JellyfinGatewayRegistration::new(
            "gateway-degraded",
            "http://degraded-jellyfin.lan:8096",
            "",
            GatewayHealth::Degraded,
            false,
        )
        .unwrap();
        let default_id = mesh_default.id.clone();
        let preferred_id = preferred_last.id.clone();
        let degraded_id = degraded_last.id.clone();
        let sources =
            merge_jellyfin_gateway_sources(&[mesh_default, preferred_last, degraded_last]);

        assert_eq!(
            select_jellyfin_gateway_source(&sources, Some(&preferred_id))
                .unwrap()
                .id,
            preferred_id
        );
        assert_eq!(
            select_jellyfin_gateway_source(&sources, Some(&degraded_id))
                .unwrap()
                .id,
            default_id
        );
        assert_eq!(
            select_jellyfin_gateway_source(&sources, None).unwrap().id,
            default_id
        );
    }

    #[test]
    fn read_jellyfin_gateway_sources_from_plane_reads_single_and_list_docs() {
        let tmp = tempfile::tempdir().unwrap();
        let single = JellyfinGatewayRegistration::new(
            "gateway-a",
            "http://jellyfin-a.lan:8096",
            "",
            GatewayHealth::Healthy,
            true,
        )
        .unwrap();
        seed_jellyfin_gateway_doc(
            tmp.path(),
            "gateway-a",
            &serde_json::to_string(&single).unwrap(),
        );

        let list = vec![
            JellyfinGatewayRegistration::new(
                "gateway-b",
                "http://jellyfin-a.lan:8096/",
                "",
                GatewayHealth::Degraded,
                false,
            )
            .unwrap(),
            JellyfinGatewayRegistration::new(
                "gateway-c",
                "http://jellyfin-c.lan:8096",
                "",
                GatewayHealth::Healthy,
                false,
            )
            .unwrap(),
        ];
        seed_jellyfin_gateway_doc(
            tmp.path(),
            "gateway-b",
            &serde_json::to_string(&list).unwrap(),
        );

        let mut tampered = single.clone();
        tampered.id = "forged".to_owned();
        seed_jellyfin_gateway_doc(
            tmp.path(),
            "tampered",
            &serde_json::to_string(&tampered).unwrap(),
        );

        let sources = read_jellyfin_gateway_sources_from_plane(tmp.path());

        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].id, single.id);
        assert_eq!(sources[0].gateway_node, "gateway-a");
        assert!(sources
            .iter()
            .any(|source| source.gateway_node == "gateway-c"));
        assert!(!sources.iter().any(|source| source.id == "forged"));
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

    fn seed_airsonic_gateway_doc(root: &Path, host: &str, body: &str) {
        let dir = root.join(host);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(AIRSONIC_GATEWAY_REGISTRY_FILE), body).unwrap();
    }

    fn seed_jellyfin_gateway_doc(root: &Path, host: &str, body: &str) {
        let dir = root.join(host);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(JELLYFIN_GATEWAY_REGISTRY_FILE), body).unwrap();
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

    #[test]
    fn read_shared_account_from_plane_skips_hostile_registry_inputs() {
        let tmp = tempfile::tempdir().unwrap();
        seed_plane_doc(
            tmp.path(),
            "valid",
            &registration_with_account(
                "peer:valid",
                NAVIDROME_PORT,
                HEALTH_UP,
                Some(SharedAccount::new("mesh-music", "p1")),
            ),
        );

        let invalid_utf8 = tmp.path().join("invalid-utf8").join(MEDIA_REGISTRY_FILE);
        std::fs::create_dir_all(invalid_utf8.parent().unwrap()).unwrap();
        std::fs::write(&invalid_utf8, [0xff, 0xfe, 0xfd]).unwrap();

        let oversized = tmp.path().join("oversized").join(MEDIA_REGISTRY_FILE);
        std::fs::create_dir_all(oversized.parent().unwrap()).unwrap();
        std::fs::write(&oversized, vec![b'x'; MAX_MEDIA_REGISTRY_BYTES + 1]).unwrap();

        let directory = tmp.path().join("directory").join(MEDIA_REGISTRY_FILE);
        std::fs::create_dir_all(directory.parent().unwrap()).unwrap();
        std::fs::create_dir(&directory).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            use std::process::Command;

            let outside = tmp.path().join("outside-registry.json");
            std::fs::write(
                &outside,
                serde_json::to_string(&registration_with_account(
                    "peer:outside",
                    NAVIDROME_PORT,
                    HEALTH_UP,
                    Some(SharedAccount::new("outside", "p2")),
                ))
                .unwrap(),
            )
            .unwrap();
            let linked = tmp.path().join("symlink").join(MEDIA_REGISTRY_FILE);
            std::fs::create_dir_all(linked.parent().unwrap()).unwrap();
            symlink(&outside, &linked).unwrap();

            let fifo = tmp.path().join("fifo").join(MEDIA_REGISTRY_FILE);
            std::fs::create_dir_all(fifo.parent().unwrap()).unwrap();
            assert!(Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success());
        }

        let account = read_shared_account_from_plane(tmp.path()).unwrap();
        assert_eq!(account.username, "mesh-music");
        assert_eq!(account.password, "p1");
    }
}
