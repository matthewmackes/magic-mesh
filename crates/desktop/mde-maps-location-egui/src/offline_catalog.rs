//! Verified, bounded catalog contracts for approved offline map regions.
//!
//! Catalog acquisition belongs to a worker. This module only admits already
//! fetched bytes, binds them to a caller-supplied SHA-256, and exposes stable
//! region/tile identities to the disk cache and future navigation authority.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};

pub const CATALOG_SCHEMA: u16 = 1;
pub const APPROVED_PROVIDER: &str = "openstreetmap-derived";
pub const MAX_CATALOG_BYTES: usize = 256 * 1024;
pub const MAX_REGIONS: usize = 256;
pub const MAX_REGION_ID_BYTES: usize = 64;
pub const MAX_REVISION_BYTES: usize = 96;
pub const MAX_TILE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_ZOOM: u8 = 22;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogError {
    Oversized,
    Digest,
    Json(String),
    Policy(&'static str),
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Oversized => write!(f, "catalog exceeds its byte bound"),
            Self::Digest => write!(f, "catalog digest is malformed or does not match"),
            Self::Json(error) => write!(f, "catalog JSON is invalid: {error}"),
            Self::Policy(error) => f.write_str(error),
        }
    }
}

impl std::error::Error for CatalogError {}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RegionId(String);

impl RegionId {
    pub fn parse(value: impl Into<String>) -> Result<Self, CatalogError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_REGION_ID_BYTES
            || !value.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
            })
            || value.starts_with(['-', '_'])
            || value.ends_with(['-', '_'])
        {
            return Err(CatalogError::Policy("region id is not a bounded safe slug"));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TileId {
    pub region: RegionId,
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

impl TileId {
    pub fn new(region: RegionId, z: u8, x: u32, y: u32) -> Result<Self, CatalogError> {
        let side = 1_u32
            .checked_shl(u32::from(z))
            .filter(|_| z <= MAX_ZOOM)
            .ok_or(CatalogError::Policy("tile zoom is unsupported"))?;
        if x >= side || y >= side {
            return Err(CatalogError::Policy("tile coordinate is outside its zoom"));
        }
        Ok(Self { region, z, x, y })
    }

    #[must_use]
    pub fn stable_identity(&self) -> String {
        format!("{}/{}/{}/{}", self.region.as_str(), self.z, self.x, self.y)
    }

    pub(crate) fn validate(&self) -> Result<(), CatalogError> {
        let region = RegionId::parse(self.region.0.clone())?;
        Self::new(region, self.z, self.x, self.y).map(|_| ())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogRegion {
    pub region_id: RegionId,
    pub revision: String,
    pub min_zoom: u8,
    pub max_zoom: u8,
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogDocument {
    schema: u16,
    provider: String,
    regions: Vec<CatalogRegion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedCatalog {
    digest: String,
    regions: Vec<CatalogRegion>,
}

impl VerifiedCatalog {
    pub fn admit_json(bytes: &[u8], expected_sha256: &str) -> Result<Self, CatalogError> {
        if bytes.is_empty() || bytes.len() > MAX_CATALOG_BYTES {
            return Err(CatalogError::Oversized);
        }
        if !is_sha256_hex(expected_sha256) || sha256_hex(bytes) != expected_sha256 {
            return Err(CatalogError::Digest);
        }
        let document: CatalogDocument =
            serde_json::from_slice(bytes).map_err(|error| CatalogError::Json(error.to_string()))?;
        if document.schema != CATALOG_SCHEMA {
            return Err(CatalogError::Policy("unsupported catalog schema"));
        }
        if document.provider != APPROVED_PROVIDER {
            return Err(CatalogError::Policy("catalog provider is not approved"));
        }
        if document.regions.is_empty() || document.regions.len() > MAX_REGIONS {
            return Err(CatalogError::Policy(
                "catalog region count is outside bounds",
            ));
        }
        let mut identities = BTreeSet::new();
        for region in &document.regions {
            RegionId::parse(region.region_id.0.clone())?;
            if region.revision.is_empty()
                || region.revision.len() > MAX_REVISION_BYTES
                || region.revision.chars().any(char::is_control)
            {
                return Err(CatalogError::Policy("region revision is invalid"));
            }
            if region.min_zoom > region.max_zoom || region.max_zoom > MAX_ZOOM {
                return Err(CatalogError::Policy("region zoom range is invalid"));
            }
            if region.expires_at_ms == 0 {
                return Err(CatalogError::Policy("region expiry is invalid"));
            }
            if !identities.insert(region.region_id.clone()) {
                return Err(CatalogError::Policy("catalog contains a duplicate region"));
            }
        }
        Ok(Self {
            digest: expected_sha256.to_string(),
            regions: document.regions,
        })
    }

    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    #[must_use]
    pub fn permits(&self, tile: &TileId, now_ms: u64) -> bool {
        tile.validate().is_ok()
            && self.regions.iter().any(|region| {
                region.region_id == tile.region
                    && tile.z >= region.min_zoom
                    && tile.z <= region.max_zoom
                    && now_ms <= region.expires_at_ms
            })
    }
}

pub(crate) fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

/// Dependency-light SHA-256 used to bind bounded local catalog and tile bytes.
pub fn sha256_hex(bytes: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut state = [
        0x6a09e667_u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut padded = bytes.to_vec();
    let bit_len = (padded.len() as u64).wrapping_mul(8);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in padded.chunks_exact(64) {
        let mut words = [0_u32; 64];
        for (index, word) in words[..16].iter_mut().enumerate() {
            let mut bytes = [0_u8; 4];
            bytes.copy_from_slice(&chunk[index * 4..index * 4 + 4]);
            *word = u32::from_be_bytes(bytes);
        }
        for index in 16..64 {
            let a = words[index - 15].rotate_right(7)
                ^ words[index - 15].rotate_right(18)
                ^ (words[index - 15] >> 3);
            let b = words[index - 2].rotate_right(17)
                ^ words[index - 2].rotate_right(19)
                ^ (words[index - 2] >> 10);
            words[index] = words[index - 16]
                .wrapping_add(a)
                .wrapping_add(words[index - 7])
                .wrapping_add(b);
        }
        let mut work = state;
        for index in 0..64 {
            let sigma1 =
                work[4].rotate_right(6) ^ work[4].rotate_right(11) ^ work[4].rotate_right(25);
            let choose = (work[4] & work[5]) ^ (!work[4] & work[6]);
            let first = work[7]
                .wrapping_add(sigma1)
                .wrapping_add(choose)
                .wrapping_add(K[index])
                .wrapping_add(words[index]);
            let sigma0 =
                work[0].rotate_right(2) ^ work[0].rotate_right(13) ^ work[0].rotate_right(22);
            let majority = (work[0] & work[1]) ^ (work[0] & work[2]) ^ (work[1] & work[2]);
            let second = sigma0.wrapping_add(majority);
            work = [
                first.wrapping_add(second),
                work[0],
                work[1],
                work[2],
                work[3].wrapping_add(first),
                work[4],
                work[5],
                work[6],
            ];
        }
        for (slot, value) in state.iter_mut().zip(work) {
            *slot = slot.wrapping_add(value);
        }
    }
    state.iter().map(|word| format!("{word:08x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog_json() -> Vec<u8> {
        br#"{"schema":1,"provider":"openstreetmap-derived","regions":[{"region_id":"east-texas","revision":"2026-08","min_zoom":0,"max_zoom":18,"expires_at_ms":2000}]}"#.to_vec()
    }

    #[test]
    fn deterministic_identity_and_catalog_digest_admission() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let bytes = catalog_json();
        let catalog = VerifiedCatalog::admit_json(&bytes, &sha256_hex(&bytes)).unwrap();
        let tile = TileId::new(RegionId::parse("east-texas").unwrap(), 12, 957, 1661).unwrap();
        assert_eq!(tile.stable_identity(), "east-texas/12/957/1661");
        assert!(catalog.permits(&tile, 2_000));
        assert!(!catalog.permits(&tile, 2_001));
    }

    #[test]
    fn traversal_duplicate_fields_and_wrong_digest_fail_closed() {
        for hostile in ["../east", "east/texas", ".", "East-Texas", "east texas"] {
            assert!(RegionId::parse(hostile).is_err(), "accepted {hostile}");
        }
        let duplicate =
            br#"{"schema":1,"schema":1,"provider":"openstreetmap-derived","regions":[]}"#;
        assert!(VerifiedCatalog::admit_json(duplicate, &sha256_hex(duplicate)).is_err());
        let bytes = catalog_json();
        assert!(VerifiedCatalog::admit_json(&bytes, &"0".repeat(64)).is_err());
    }

    #[test]
    fn tile_coordinate_property_is_bounded_by_zoom() {
        let region = RegionId::parse("r").unwrap();
        for z in 0..=MAX_ZOOM {
            let side = 1_u32 << z;
            assert!(TileId::new(region.clone(), z, side - 1, side - 1).is_ok());
            assert!(TileId::new(region.clone(), z, side, 0).is_err());
        }
        assert!(TileId::new(region, MAX_ZOOM + 1, 0, 0).is_err());
    }
}
