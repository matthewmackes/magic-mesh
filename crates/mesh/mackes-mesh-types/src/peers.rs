//! Peer-data convergence records (PEERVER-1).
//!
//! The substrate locked in `docs/design/v2.7-peer-data-convergence.md`:
//! each peer writes its own `<mesh-home>/peers/<hostname>.json` (own-row
//! authority — sole writer per file); Syncthing replicates the dir to
//! every peer; any tool [`read_peers`] unions the dir. No broker / D-Bus
//! / mackesd dependency for reads.
//!
//! This module is the shared home so both `mackesd` (writer, on the
//! heartbeat tick — PEERVER-2) and `mde-installer` (reader — PEERVER-3)
//! use one code path without `mde-installer` linking the heavy
//! `mackesd_core`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Version of the collision-detectable overlay claimant contract.
pub const OVERLAY_IDENTITY_CLAIM_SCHEMA_VERSION: u16 = 1;
/// Maximum encoded size of one overlay identity claim.
pub const MAX_OVERLAY_IDENTITY_CLAIM_BYTES: usize = 1_024;
/// Maximum suffix length after the required `peer:` node-id prefix.
pub const MAX_OVERLAY_NODE_SUFFIX_BYTES: usize = 128;
/// Maximum Nebula certificate-name length.
pub const MAX_NEBULA_NAME_BYTES: usize = 253;

/// A strict, credential-free assertion that one physical machine and boot are
/// actively using one public Nebula identity.
///
/// The claimant fields are already-derived, domain-separated SHA-256 digests.
/// Raw machine-id, boot-id, certificate bytes, paths, credentials, and secrets
/// are deliberately absent from this wire contract. Callers must obtain every
/// field from validated local public identity facts; this type never invents a
/// certificate fingerprint or treats a hostname as identity authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayIdentityClaim {
    /// Closed wire-schema discriminator.
    pub schema_version: u16,
    /// Stable enrolled platform node id (`peer:<safe-name>`).
    pub nebula_node_id: String,
    /// Exact public name printed from the active Nebula certificate.
    pub nebula_name: String,
    /// Canonical IPv4 address printed from the active Nebula certificate.
    pub nebula_address: String,
    /// Lowercase, bare SHA-256 fingerprint of the public Nebula certificate.
    pub certificate_fingerprint: String,
    /// Lowercase, bare certificate-scoped, domain-separated SHA-256
    /// physical-machine claimant.
    pub machine_claimant_digest: String,
    /// Lowercase, bare certificate-scoped, domain-separated SHA-256
    /// current-boot claimant.
    pub boot_claimant_digest: String,
}

/// Validation failure at the overlay identity claimant boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayIdentityClaimError {
    /// The encoded claim exceeds its allocation bound.
    PayloadTooLarge,
    /// The body is not the exact closed JSON shape.
    MalformedWire,
    /// The schema version is not supported.
    UnsupportedSchema(u16),
    /// A named identity field is malformed or inconsistent.
    InvalidField(&'static str),
    /// A named digest is not exactly 64 lowercase hexadecimal characters.
    InvalidDigest(&'static str),
    /// Machine and boot dimensions reused the same digest.
    DuplicateClaimantDigest,
}

impl std::fmt::Display for OverlayIdentityClaimError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PayloadTooLarge => formatter.write_str("overlay identity claim is too large"),
            Self::MalformedWire => formatter.write_str("malformed overlay identity claim"),
            Self::UnsupportedSchema(version) => {
                write!(
                    formatter,
                    "unsupported overlay identity claim schema {version}"
                )
            }
            Self::InvalidField(field) => {
                write!(formatter, "invalid overlay identity claim field {field}")
            }
            Self::InvalidDigest(field) => {
                write!(formatter, "invalid overlay identity claim digest {field}")
            }
            Self::DuplicateClaimantDigest => {
                formatter.write_str("machine and boot claimant digests must be domain-distinct")
            }
        }
    }
}

impl std::error::Error for OverlayIdentityClaimError {}

impl OverlayIdentityClaim {
    /// Construct and validate a claim exclusively from caller-supplied public
    /// identity facts and privacy-bounded claimant digests.
    ///
    /// # Errors
    /// Returns [`OverlayIdentityClaimError`] when any field is malformed,
    /// inconsistent, or outside the closed v1 bounds.
    pub fn new(
        nebula_node_id: impl Into<String>,
        nebula_name: impl Into<String>,
        nebula_address: impl Into<String>,
        certificate_fingerprint: impl Into<String>,
        machine_claimant_digest: impl Into<String>,
        boot_claimant_digest: impl Into<String>,
    ) -> Result<Self, OverlayIdentityClaimError> {
        let claim = Self {
            schema_version: OVERLAY_IDENTITY_CLAIM_SCHEMA_VERSION,
            nebula_node_id: nebula_node_id.into(),
            nebula_name: nebula_name.into(),
            nebula_address: nebula_address.into(),
            certificate_fingerprint: certificate_fingerprint.into(),
            machine_claimant_digest: machine_claimant_digest.into(),
            boot_claimant_digest: boot_claimant_digest.into(),
        };
        claim.validate()?;
        Ok(claim)
    }

    /// Validate the exact v1 identity grammar and claimant dimensions.
    ///
    /// # Errors
    /// Returns [`OverlayIdentityClaimError`] for unsupported versions,
    /// malformed public identity facts, placeholder digests, or reused
    /// machine/boot claimant dimensions.
    pub fn validate(&self) -> Result<(), OverlayIdentityClaimError> {
        if self.schema_version != OVERLAY_IDENTITY_CLAIM_SCHEMA_VERSION {
            return Err(OverlayIdentityClaimError::UnsupportedSchema(
                self.schema_version,
            ));
        }
        validate_overlay_node_id(&self.nebula_node_id)?;
        validate_nebula_name(&self.nebula_name)?;
        if self.nebula_node_id != self.nebula_name {
            return Err(OverlayIdentityClaimError::InvalidField("nebula_name"));
        }
        validate_nebula_address(&self.nebula_address)?;
        validate_claim_digest("certificate_fingerprint", &self.certificate_fingerprint)?;
        validate_claim_digest("machine_claimant_digest", &self.machine_claimant_digest)?;
        validate_claim_digest("boot_claimant_digest", &self.boot_claimant_digest)?;
        if self.machine_claimant_digest == self.boot_claimant_digest {
            return Err(OverlayIdentityClaimError::DuplicateClaimantDigest);
        }
        if self.to_json_unchecked()?.len() > MAX_OVERLAY_IDENTITY_CLAIM_BYTES {
            return Err(OverlayIdentityClaimError::PayloadTooLarge);
        }
        Ok(())
    }

    /// Decode and validate one bounded, closed-shape JSON claim.
    ///
    /// Serde rejects duplicate and unknown fields before semantic admission.
    ///
    /// # Errors
    /// Returns [`OverlayIdentityClaimError`] when the body is oversized,
    /// malformed, duplicated, unknown-field-bearing, or semantically invalid.
    pub fn from_json(bytes: &[u8]) -> Result<Self, OverlayIdentityClaimError> {
        if bytes.len() > MAX_OVERLAY_IDENTITY_CLAIM_BYTES {
            return Err(OverlayIdentityClaimError::PayloadTooLarge);
        }
        let claim: Self =
            serde_json::from_slice(bytes).map_err(|_| OverlayIdentityClaimError::MalformedWire)?;
        claim.validate()?;
        Ok(claim)
    }

    /// Encode the validated claim into its deterministic bounded JSON value.
    ///
    /// # Errors
    /// Returns [`OverlayIdentityClaimError`] when the claim is invalid or its
    /// encoded representation exceeds the wire bound.
    pub fn to_json(&self) -> Result<String, OverlayIdentityClaimError> {
        self.validate()?;
        self.to_json_unchecked()
    }

    fn to_json_unchecked(&self) -> Result<String, OverlayIdentityClaimError> {
        serde_json::to_string(self).map_err(|_| OverlayIdentityClaimError::MalformedWire)
    }
}

fn validate_overlay_node_id(value: &str) -> Result<(), OverlayIdentityClaimError> {
    let Some(suffix) = value.strip_prefix("peer:") else {
        return Err(OverlayIdentityClaimError::InvalidField("nebula_node_id"));
    };
    if suffix.is_empty()
        || suffix.len() > MAX_OVERLAY_NODE_SUFFIX_BYTES
        || !suffix.bytes().enumerate().all(|(index, byte)| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => true,
            b'.' | b'_' | b'-' => index > 0,
            _ => false,
        })
    {
        return Err(OverlayIdentityClaimError::InvalidField("nebula_node_id"));
    }
    Ok(())
}

fn validate_nebula_name(value: &str) -> Result<(), OverlayIdentityClaimError> {
    if value.is_empty()
        || value.len() > MAX_NEBULA_NAME_BYTES
        || value.bytes().any(|byte| byte.is_ascii_control())
        || value.contains(['/', '\\'])
    {
        return Err(OverlayIdentityClaimError::InvalidField("nebula_name"));
    }
    Ok(())
}

fn validate_nebula_address(value: &str) -> Result<(), OverlayIdentityClaimError> {
    let address = value
        .parse::<std::net::Ipv4Addr>()
        .map_err(|_| OverlayIdentityClaimError::InvalidField("nebula_address"))?;
    let octets = address.octets();
    if address.to_string() != value
        || octets[0] != 10
        || octets[1] != 42
        || octets[2] > 127
        || address.is_unspecified()
        || address.is_loopback()
        || address.is_multicast()
        || address == std::net::Ipv4Addr::new(10, 42, 0, 0)
        || address == std::net::Ipv4Addr::new(10, 42, 127, 255)
    {
        return Err(OverlayIdentityClaimError::InvalidField("nebula_address"));
    }
    Ok(())
}

fn validate_claim_digest(
    field: &'static str,
    value: &str,
) -> Result<(), OverlayIdentityClaimError> {
    if value.len() != 64
        || value.bytes().all(|byte| byte == b'0')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(OverlayIdentityClaimError::InvalidDigest(field));
    }
    Ok(())
}

/// One peer's self-reported state. The file `<hostname>.json` IS the
/// row; the peer that owns `hostname` is its sole writer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerRecord {
    /// System hostname — the file stem + the row key.
    pub hostname: String,
    /// Installed `mde-core` RPM version (`None` if not yet detected).
    #[serde(default)]
    pub mde_version: Option<String>,
    /// Wall-clock epoch milliseconds of the last write (liveness).
    pub last_seen_ms: u64,
    /// `healthy` | `degraded` | `critical` | `unreachable` | `unknown`
    /// — Netdata-alarm-derived since PD-2 (L15 3-tier mapping).
    #[serde(default = "default_health")]
    pub health: String,
    /// PD-2 — what this peer offers outside Workload authority (remote access,
    /// media services, mesh storage) plus its Netdata alarm summary. VM and
    /// container state lives only in `state/workloads/<node>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub descriptors: Option<ServiceDescriptors>,
    /// This peer's own Nebula overlay IP (`nebula1`), recorded by the
    /// node itself each heartbeat so the directory carries it mesh-wide.
    /// Previously the overlay IP lived only in the signer's local sqlite
    /// nebula roster, so peers (whose sqlite is empty) rendered Mesh DNS /
    /// Service Publishing / Routing with no overlay addresses. `None` from
    /// pre-overlay-IP writers / pre-enrollment; readers tolerate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlay_ip: Option<String>,
    /// This peer's pinned deployment role (`lighthouse` | `server` |
    /// `workstation`), recorded by the node itself each heartbeat so the
    /// replicated directory carries it mesh-wide. Lets any node identify the
    /// lighthouse set (LIGHTHOUSE-1/Q1) without a separate probe. `None` from
    /// pre-role writers / unpinned boxes; readers tolerate (treated as a
    /// non-lighthouse peer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// LIGHTHOUSE-10 — a lighthouse's PUBLIC-internet `ip:port` (the Nebula
    /// underlay address peers dial), recorded by the lighthouse itself each
    /// heartbeat so the replicated directory carries the FULL lighthouse set
    /// with reachable addresses. This is what makes up-to-5 lighthouses
    /// *redundant*: the enroll roster reads every `role=="lighthouse"` record's
    /// `external_addr` so a joining node learns ALL lighthouses, not just the
    /// one it enrolled through. `None` on non-lighthouses / pre-LIGHTHOUSE-10
    /// writers (such a lighthouse is skipped from a built roster — never
    /// advertised without a dialable address).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_addr: Option<String>,
    /// MEDIA-1 — the `media` **capability tag**: `true` when this lighthouse is
    /// the `Lighthouse_Media` subclass (hosts the Navidrome / `music.mesh`
    /// service). Stamped by the node itself each heartbeat from its pinned
    /// `role.toml` capability (`mde_role::Capability::Media`), so any node can
    /// discover the media-lighthouse set from the replicated directory without a
    /// probe (mirrors how `role`/`external_addr` ride). Orthogonal to `role`
    /// (§9: capability tags are not roles) — only meaningful when
    /// `role == "lighthouse"`. `false`/absent on every other node and on
    /// pre-MEDIA-1 writers; readers tolerate (treated as not media-capable).
    #[serde(default, skip_serializing_if = "is_false")]
    pub media: bool,
}

/// Skip-serializer for a defaulted `false` bool — keeps a non-media record's
/// JSON byte-for-byte identical to a pre-MEDIA-1 writer's.
#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_false(b: &bool) -> bool {
    !*b
}

/// PD-2 — a peer's locally-probed non-Workload service inventory,
/// published on the heartbeat (one cycle, one write — L13). Every
/// probe is localhost-only; nothing leaves the publishing host (Q19).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ServiceDescriptors {
    /// Remote-access listeners on this box.
    pub remote_access: RemoteAccess,
    /// Media services answering on the pinned localhost port list (L12).
    pub media: Vec<MediaService>,
    /// Netdata alarm summary (L15 3-tier).
    pub alarms: AlarmSummary,
    /// This peer's physical LAN MAC addresses (PD-12 — the Wake-on-LAN
    /// targets; own-row authority beats ARP-cache guessing).
    pub lan_macs: Vec<String>,
    /// MESHFS-2 — this peer's Mesh-Sync (`/mnt/mesh-storage`) `df` usage,
    /// published on the heartbeat so any node can aggregate the whole share.
    pub mesh_fs: MeshFsUsage,
}

/// MESHFS-2 — a peer's Mesh-Sync mount `df` usage.
///
/// `present` is false on a pre-MESHFS-2 writer (serde-defaulted) or a missing
/// mount, so an aggregator
/// skips it rather than reporting a phantom 0-byte share.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MeshFsUsage {
    /// True when the mount was probed and `df` succeeded.
    pub present: bool,
    /// Bytes used on the Mesh-Sync mount.
    pub used_bytes: u64,
    /// Bytes available on the Mesh-Sync mount.
    pub avail_bytes: u64,
}

/// Which remote-access services listen locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteAccess {
    /// SSH daemon is listening on this host.
    pub ssh: bool,
    /// RDP (xrdp) is listening on this host.
    pub rdp: bool,
    /// VNC server is listening on this host.
    pub vnc: bool,
}

/// One media service answering on a pinned localhost port (L12).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaService {
    /// Human-readable service name (e.g. `jellyfin`, `plex`).
    pub name: String,
    /// Pinned localhost port the service is bound to.
    pub port: u16,
}

/// Netdata alarm summary (L15): `healthy` (no active alarms) ·
/// `degraded` (any WARNING) · `critical` (any CRITICAL), with the
/// worst alarm named.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AlarmSummary {
    /// Worst-case tier: `healthy` | `degraded` | `critical`.
    pub tier: String,
    /// Name of the worst active Netdata alarm (`None` when healthy).
    pub worst: Option<String>,
}

fn default_health() -> String {
    "unknown".to_string()
}

impl PeerRecord {
    /// Build a record stamped with the current time.
    #[must_use]
    pub fn now(
        hostname: impl Into<String>,
        mde_version: Option<String>,
        health: impl Into<String>,
    ) -> Self {
        Self {
            hostname: hostname.into(),
            mde_version,
            last_seen_ms: now_ms(),
            health: health.into(),
            descriptors: None,
            overlay_ip: None,
            role: None,
            external_addr: None,
            media: false,
        }
    }

    /// Age in milliseconds against the current wall clock (saturating).
    #[must_use]
    pub fn age_ms(&self) -> u64 {
        now_ms().saturating_sub(self.last_seen_ms)
    }

    /// Whether this record is older than `threshold_ms` (stale/offline).
    #[must_use]
    pub fn is_stale(&self, threshold_ms: u64) -> bool {
        self.age_ms() > threshold_ms
    }
}

/// The `peers/` directory under a mesh-home mount.
#[must_use]
pub fn peers_dir(mesh_home: &Path) -> PathBuf {
    mesh_home.join("peers")
}

/// Resolve the mesh-home mount: `$MDE_MESH_HOME` if set, else
/// `~/.mde-mesh` (the coordination mount per `AI_GOVERNANCE` §3.1).
#[must_use]
pub fn default_mesh_home() -> PathBuf {
    if let Ok(p) = std::env::var("MDE_MESH_HOME") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    PathBuf::from(home).join(".mde-mesh")
}

/// Resolve the workgroup-root directory — the single source of truth.
///
/// It is shared by `mackesd` (directory/healthz) and the GUI shell (every
/// panel that reads off mesh-storage). Under SUBSTRATE-V2 this is the
/// plain Syncthing-replicated dir at `/mnt/mesh-storage`.
///
/// Precedence: `$MDE_WORKGROUP_ROOT` (canonical) > `$QNM_SHARED_ROOT`
/// (back-compat) > `~/QNM-Shared` > the system fallback
/// `/var/lib/mackesd/qnm-shared`.
///
/// Historically the workbench panels fell back to a phantom
/// `/mnt/mesh-storage` while `mackesd`'s directory read the real
/// `~/QNM-Shared`, reporting "not mounted" against a healthy mesh.
/// Routing every caller through this one function removes that split-brain.
#[must_use]
pub fn default_workgroup_root() -> PathBuf {
    if let Ok(root) = std::env::var("MDE_WORKGROUP_ROOT") {
        return PathBuf::from(root);
    }
    if let Ok(root) = std::env::var("QNM_SHARED_ROOT") {
        return PathBuf::from(root);
    }
    // FOUND-NEBULA-5 (2026-06-23): the env-less fallback is the CANONICAL mount, NOT
    // ~/QNM-Shared. The daemon runs with MDE_WORKGROUP_ROOT=/mnt/mesh-storage (its
    // systemd unit + environment.d), but `sudo mackesd <cmd>` strips that env — so a
    // CLI `add-peer` wrote the issued-bearer ledger to /root/QNM-Shared/ca/issued-bearers
    // while the serve process's /enroll listener validated against
    // /mnt/mesh-storage/ca/issued-bearers → every fresh `join` 401'd "bearer not
    // issued-and-unredeemed". Resolving the env-less default to the canonical mount keeps
    // the CLI and daemon byte-for-byte consistent whether or not the volume is mounted.
    // Confirmed live: B joined the moment the roots agreed.
    PathBuf::from("/mnt/mesh-storage")
}

/// Write `rec` to `<dir>/<hostname>.json` atomically (temp + rename),
/// creating `dir` if needed.
///
/// # Errors
/// IO or serialization failures.
pub fn write_peer_record(dir: &Path, rec: &PeerRecord) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let final_path = dir.join(format!("{}.json", rec.hostname));
    let tmp_path = dir.join(format!(".{}.json.tmp", rec.hostname));
    let json = serde_json::to_string_pretty(rec)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(&tmp_path, json)?;
    fs::rename(&tmp_path, &final_path)?;
    Ok(final_path)
}

/// Union every `*.json` in `dir` into a `PeerRecord` list.
///
/// Malformed or unreadable files are skipped (not fatal) — a
/// half-written file from a concurrent writer must not break a reader.
/// A missing dir yields an empty list. Sorted by hostname.
#[must_use]
pub fn read_peers(dir: &Path) -> Vec<PeerRecord> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        // Skip the atomic-write temp files (".<host>.json.tmp" — though
        // those don't end in .json, belt-and-suspenders on dotfiles).
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.'))
        {
            continue;
        }
        if let Ok(data) = fs::read_to_string(&path) {
            if let Ok(rec) = serde_json::from_str::<PeerRecord>(&data) {
                out.push(rec);
            }
        }
    }
    out.sort_by(|a, b| a.hostname.cmp(&b.hostname));
    out
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const CERT: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const MACHINE: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const BOOT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn overlay_claim() -> OverlayIdentityClaim {
        OverlayIdentityClaim::new(
            "peer:SURFACE",
            "peer:SURFACE",
            "10.42.0.7",
            CERT,
            MACHINE,
            BOOT,
        )
        .expect("valid overlay claim")
    }

    #[test]
    fn overlay_claim_roundtrips_exact_bounded_public_shape() {
        let claim = overlay_claim();
        let json = claim.to_json().expect("encode");

        assert!(json.len() <= MAX_OVERLAY_IDENTITY_CLAIM_BYTES);
        assert!(!json.contains("machine-id"));
        assert!(!json.contains("boot-id"));
        assert!(!json.contains('/'));
        assert_eq!(
            OverlayIdentityClaim::from_json(json.as_bytes()).expect("decode"),
            claim
        );
    }

    #[test]
    fn overlay_claim_rejects_duplicate_and_unknown_wire_fields() {
        let claim = overlay_claim();
        let json = claim.to_json().expect("encode");
        let duplicate = json.replacen("{", "{\"schema_version\":1,", 1);
        assert_eq!(
            OverlayIdentityClaim::from_json(duplicate.as_bytes()),
            Err(OverlayIdentityClaimError::MalformedWire)
        );

        let unknown = json.replacen("{", "{\"raw_machine_id\":\"forbidden\",", 1);
        assert_eq!(
            OverlayIdentityClaim::from_json(unknown.as_bytes()),
            Err(OverlayIdentityClaimError::MalformedWire)
        );
    }

    #[test]
    fn overlay_claim_rejects_malformed_identity_and_reused_digest() {
        assert_eq!(
            OverlayIdentityClaim::new(
                "peer:SURFACE",
                "peer:OTHER",
                "10.42.0.7",
                CERT,
                MACHINE,
                BOOT,
            ),
            Err(OverlayIdentityClaimError::InvalidField("nebula_name"))
        );
        assert_eq!(
            OverlayIdentityClaim::new(
                "peer:SURFACE",
                "peer:SURFACE",
                "10.42.128.7",
                CERT,
                MACHINE,
                BOOT,
            ),
            Err(OverlayIdentityClaimError::InvalidField("nebula_address"))
        );
        assert_eq!(
            OverlayIdentityClaim::new(
                "peer:SURFACE",
                "peer:SURFACE",
                "10.42.0.7",
                CERT,
                MACHINE,
                MACHINE,
            ),
            Err(OverlayIdentityClaimError::DuplicateClaimantDigest)
        );
    }

    #[test]
    fn write_then_read_roundtrips() {
        let dir = tempdir().unwrap();
        let rec = PeerRecord::now("anvil", Some("5.0.0".into()), "healthy");
        let p = write_peer_record(dir.path(), &rec).unwrap();
        assert!(p.ends_with("anvil.json"));
        let back = read_peers(dir.path());
        assert_eq!(back, vec![rec]);
    }

    #[test]
    fn read_unions_multiple_files_sorted() {
        let dir = tempdir().unwrap();
        write_peer_record(
            dir.path(),
            &PeerRecord::now("forge", Some("5.0.0".into()), "healthy"),
        )
        .unwrap();
        write_peer_record(
            dir.path(),
            &PeerRecord::now("anvil", Some("5.0.0".into()), "healthy"),
        )
        .unwrap();
        let peers = read_peers(dir.path());
        assert_eq!(peers.len(), 2);
        assert_eq!(peers[0].hostname, "anvil");
        assert_eq!(peers[1].hostname, "forge");
    }

    #[test]
    fn malformed_file_is_skipped_not_fatal() {
        let dir = tempdir().unwrap();
        write_peer_record(dir.path(), &PeerRecord::now("anvil", None, "healthy")).unwrap();
        fs::write(dir.path().join("broken.json"), "{ not json").unwrap();
        let peers = read_peers(dir.path());
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].hostname, "anvil");
    }

    #[test]
    fn retired_runtime_descriptor_fields_are_ignored_and_not_republished() {
        let mut value =
            serde_json::to_value(PeerRecord::now("anvil", Some("13.0.0".into()), "healthy"))
                .unwrap();
        value["descriptors"] = serde_json::json!({
            "containers": [{"name": "old", "state": "running"}],
            "vms": [{"name": "old-vm", "state": "running"}],
            "remote_access": {"ssh": true, "rdp": false, "vnc": false}
        });
        let record: PeerRecord = serde_json::from_value(value).unwrap();
        let encoded = serde_json::to_value(record).unwrap();
        let descriptors = encoded["descriptors"].as_object().unwrap();
        assert!(!descriptors.contains_key("containers"));
        assert!(!descriptors.contains_key("vms"));
        assert_eq!(descriptors["remote_access"]["ssh"], true);
    }

    #[test]
    fn missing_dir_is_empty() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("nope");
        assert!(read_peers(&missing).is_empty());
    }

    #[test]
    fn stale_classification() {
        let mut rec = PeerRecord::now("anvil", None, "healthy");
        rec.last_seen_ms = 1; // ancient
        assert!(rec.is_stale(60_000));
        let fresh = PeerRecord::now("forge", None, "healthy");
        assert!(!fresh.is_stale(60_000));
    }

    #[test]
    fn peers_dir_under_mesh_home() {
        assert_eq!(
            peers_dir(Path::new("/home/u/.mde-mesh")),
            Path::new("/home/u/.mde-mesh/peers")
        );
    }
}
