# WL-REL-001 S2 version matrix — 2026-08-23

Date: 2026-08-23
Classification: S2 version-surface reconfirmation; **not** final freeze,
preflight admission, dest bind, or live enroll
Worklist: `WL-REL-001` S2
Farm host: `172.20.0.50`
Farm slot: `rel001-99237c`
`final_freeze: false`
`production_admitted: false`

This is the named per-package matrix that S2 required and that the 2026-08-16
summary receipts did not emit. It reconfirms version surfaces on the current
checkout after dest-cut `7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac`. It does
not promote this checkout to the freeze.

## Observed identity

| Field | Value |
|---|---|
| Checkout HEAD | `de89cb277f50096e1bf2e18b5c58299d2fba4638` |
| Dest-cut (worklist, not this freeze) | `7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac` / `1787450205` |
| Input-generation candidate | `2872293b1393fdb6d645170cea30fc7d1682569d` / `1787447942` |
| Workspace version | `13.0.0` |
| Workspace members | 43 |
| Members at `13.0.0` | 39 |
| Documented `0.0.0` workspace boundaries | 4 |
| Isolated surfaces | 4 |

## Farm verification

```text
MCNF_BUILD_HOST=172.20.0.50
MCNF_BUILD_SLOT=rel001-99237c
MCNF_BUILD_SHAPE=small
install-helpers/xcp-build.sh cargo metadata --format-version 1
```

```text
==> xcp-build: route: MCNF_BUILD_HOST pinned → 172.20.0.50 (shape routing skipped)
==> xcp-build: remote admission capacity: 65071596 KiB free (required 8388608 KiB; output 0B, scratch 0B)
==> xcp-build: rsync working tree → mm@172.20.0.50:magic-mesh-farm-rel001-99237c (excluding target*/)
```

Result: `PASS` (exit 0). Resolved metadata contained 43 workspace members and
961 packages. No ENOSPC. Admission required 8 GiB; the node had 62.3 GiB free.

## Machine check

`install-helpers/check-release-version-surfaces.sh` encodes the S2 contract:
workspace members must be `13.0.0` except the documented `0.0.0` set; isolated
browser-helper manifests and lockfiles must mirror the workspace version; the
isolated Maps verifier stays `0.0.0`. `--self-test` refuses a drifted shipped
version, a missing `0.0.0` boundary, and a drifted isolated helper.

```text
install-helpers/check-release-version-surfaces.sh --self-test
# check-release-version-surfaces: self-test passed (clean matrix; drifted shipped, missing boundary, and drifted isolated helper fail closed)

install-helpers/check-release-version-surfaces.sh --repo . --metadata-json <farm metadata>
# check-release-version-surfaces: PASS (43 workspace members; workspace 13.0.0; 4 documented 0.0.0 boundaries; 4 isolated surfaces)
```

## Bounded version matrix

| Package | Source | Observed | Expected | Class |
|---|---|---|---|---|
| mcnf-cuttlefish-guest | root workspace member; not a 13.0.0 release role | 13.0.0 | 13.0.0 | deferred-role-source |
| browser-vm-production-control | install-helpers/browser-vm-production-control/Cargo.toml + Cargo.lock | 13.0.0 | 13.0.0 | isolated-browser-helper |
| browser-vm-production-control-guest | install-helpers/browser-vm-production-control/guest-controller/Cargo.toml + Cargo.lock | 13.0.0 | 13.0.0 | isolated-browser-helper |
| serve-browser-vm-performance-rdp | install-helpers/serve-browser-vm-performance-rdp/Cargo.toml + Cargo.lock | 13.0.0 | 13.0.0 | isolated-browser-helper |
| verify-offline-map-catalog | packaging/maps/verifier/Cargo.toml + Cargo.lock | 0.0.0 | 0.0.0 | isolated-maps-verifier |
| mackes-transport | documented 0.0.0 workspace boundary | 0.0.0 | 0.0.0 | non-release-workspace |
| magic-fleet | documented 0.0.0 workspace boundary | 0.0.0 | 0.0.0 | non-release-workspace |
| mde-kdc-host | documented 0.0.0 workspace boundary | 0.0.0 | 0.0.0 | non-release-workspace |
| mde-kdc-proto | documented 0.0.0 workspace boundary | 0.0.0 | 0.0.0 | non-release-workspace |
| mackes-config | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mackes-mesh-types | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mackes-nebula-https-tunnel | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mackesd | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-bookmarks | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-bookmarks-egui | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-bus | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-chat | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-collab-core | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-collab-egui | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-collab-types | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-disclaimer | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-editor-egui | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-egui | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-enroll | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-files | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-files-egui | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-jellyfin | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-maps-location-egui | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-media-core | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-media-egui | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-mesh-view | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-music-egui | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-musicd | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-role | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-role-chooser | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-seal | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-seat | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-shell-egui | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-term-egui | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-theme | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-vdi-core | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-vdi-rdp | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-vdi-spice | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-vdi-vnc | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-voice-hud | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-wayland-workspace | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |
| mde-worker-core | Cargo.toml [workspace.package].version | 13.0.0 | 13.0.0 | shipped-workspace |

No shipped workspace member or isolated browser helper resolved to another
release version. `mcnf-cuttlefish-guest` inherits `13.0.0` because it is a
root workspace member; it is not a `13.0.0` release role.

## What this does not claim

- Final S1/S4 freeze (`WL-REL-001-source-freeze-r1.md` is still due).
- REL-006 preflight pass or `production_admitted`.
- Live FUNC-023 enroll / offboard / reenroll.
- GitHub required-check authority for this SHA.

## Next honest acts

1. Keep using this helper at dest-cut reconfirmation; do not treat a later
   metadata summary as a substitute for the named matrix.
2. Live FUNC-023 enroll, then REL-006 admission against the same candidate
   SHA, then reconfirm that SHA before calling it the freeze.
