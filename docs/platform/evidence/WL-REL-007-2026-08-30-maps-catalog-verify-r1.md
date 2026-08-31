# WL-REL-007 / WL-REL-006 Maps catalog verifier dest — 2026-08-30

Classification: dest materialization of the in-tree catalog verifier.
Not a preflight pass. Not freeze. Not `production_admitted`. No dest
invented. Surface `bootc_base` stays null.

Tree: `727f0309c`. Isolated crate
`packaging/maps/verifier` is not in the root workspace. Farm
`cargo build --manifest-path packaging/maps/verifier/Cargo.toml --locked`
on `.50` slot `149` finished in 2m17s. Official workspace cargo was
not re-ground.

## Dest

| Dest | Path | Notes |
|---|---|---|
| Verifier | `.50` and BigBoy `/home/mm/mcnf-dest/verify-offline-map-catalog` | mode 0555; debug build of `verify-offline-map-catalog` 0.0.0 |
| Bundle admit | `verify-offline-map-catalog /tmp/mcnf-maps-offline-bundle-f8dce4e0c` | exit 0 |
| Materialize | BigBoy `/home/mm/mcnf-maps-offline-cache-f8dce4e0c` | dest MBTiles `6d01a543…` copied in; verifier admitted the bundle |

Public OSM tiles were not fetched. Dest MBTiles was not replaced.

## Still leftover

Selected bootc dest `3a5e74e6…` is still gone from quay and the farm.
S7 argv was not written. Do not grind `cargo test --workspace`.
