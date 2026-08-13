use std::fs;
use std::path::{Path, PathBuf};

use mde_maps_location_egui::offline_cache::{CachePolicy, OfflineTile, OfflineTileCache};
use mde_maps_location_egui::offline_catalog::{RegionId, TileId, VerifiedCatalog};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u16,
    kind: String,
    provider: String,
    attribution: String,
    license: String,
    source_revision: String,
    source_epoch: u64,
    quota_bytes: u64,
    payload_bytes: u64,
    catalog_sha256: String,
    cache_index_sha256: String,
    regions: Vec<Region>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Region {
    region_id: String,
    revision: String,
    bounds: Bounds,
    min_zoom: u8,
    max_zoom: u8,
    expires_at_ms: u64,
    tiles: Vec<Tile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Bounds {
    west: f64,
    south: f64,
    east: f64,
    north: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Tile {
    z: u8,
    x: u32,
    y: u32,
    sha256: String,
    size_bytes: u64,
    path: String,
}

fn sha256(bytes: &[u8]) -> String {
    mde_maps_location_egui::offline_catalog::sha256_hex(bytes)
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        let target = destination.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), target).map_err(|error| error.to_string())?;
        } else {
            return Err("bundle contains a non-file payload entry".into());
        }
    }
    Ok(())
}

fn verify(bundle: PathBuf) -> Result<(), String> {
    let manifest_bytes =
        fs::read(bundle.join("manifest.json")).map_err(|error| error.to_string())?;
    let manifest: Manifest =
        serde_json::from_slice(&manifest_bytes).map_err(|error| error.to_string())?;
    if manifest.schema != 1
        || manifest.kind != "mcnf-offline-map-catalog"
        || manifest.provider != "openstreetmap-derived"
        || manifest.attribution.trim().is_empty()
        || manifest.license.trim().is_empty()
        || manifest.source_revision.len() != 40
        || manifest.regions.is_empty()
    {
        return Err("release manifest policy is invalid".into());
    }
    let catalog_bytes = fs::read(bundle.join("catalog.json")).map_err(|error| error.to_string())?;
    if sha256(&catalog_bytes) != manifest.catalog_sha256 {
        return Err("runtime catalog digest is not bound to release manifest".into());
    }
    let catalog = VerifiedCatalog::admit_json(&catalog_bytes, &manifest.catalog_sha256)
        .map_err(|error| error.to_string())?;
    let index_bytes =
        fs::read(bundle.join("payload/index.json")).map_err(|error| error.to_string())?;
    if sha256(&index_bytes) != manifest.cache_index_sha256 {
        return Err("cache index digest is not bound to release manifest".into());
    }
    let materialized = tempfile::tempdir().map_err(|error| error.to_string())?;
    copy_tree(&bundle.join("payload"), materialized.path())?;
    let mut cache = OfflineTileCache::open(
        materialized.path(),
        CachePolicy::bounded(manifest.quota_bytes, u64::MAX).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    if cache.used_bytes() != manifest.payload_bytes {
        return Err("cache payload usage differs from release manifest".into());
    }
    let now_ms = manifest
        .source_epoch
        .checked_mul(1000)
        .ok_or("source epoch overflow")?;
    for region in manifest.regions {
        if !(region.bounds.west < region.bounds.east && region.bounds.south < region.bounds.north)
            || region.revision.is_empty()
            || region.min_zoom > region.max_zoom
            || region.expires_at_ms <= now_ms
        {
            return Err("region policy is invalid".into());
        }
        let region_id = RegionId::parse(region.region_id).map_err(|error| error.to_string())?;
        for tile in region.tiles {
            let identity = TileId::new(region_id.clone(), tile.z, tile.x, tile.y)
                .map_err(|error| error.to_string())?;
            let expected_suffix = format!(
                "/{}/{}/{}/{}-{}.tile",
                identity.region.as_str(),
                tile.z,
                tile.x,
                tile.y,
                tile.sha256
            );
            if !tile.path.ends_with(&expected_suffix) || tile.size_bytes == 0 {
                return Err("tile path/size is not bound to its identity".into());
            }
            match cache.lookup(&catalog, &identity, now_ms) {
                OfflineTile::Verified {
                    bytes,
                    sha256: actual,
                } if actual == tile.sha256 && bytes.len() as u64 == tile.size_bytes => {}
                _ => return Err("Maps cache rejected a produced tile".into()),
            }
        }
    }
    Ok(())
}

fn main() {
    let bundle = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            eprintln!("usage: verify-offline-map-catalog <bundle>");
            std::process::exit(2);
        });
    if let Err(error) = verify(bundle) {
        eprintln!("verify-offline-map-catalog: refusal: {error}");
        std::process::exit(2);
    }
}
