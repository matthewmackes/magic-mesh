# WL-CRIT-006 — first full release RPM gate (r1)

- Date: 2026-08-12
- Source revision: `1db80d0532e04122498f688e90a9091280a3c0ad`
- Farm host / slot: `172.20.0.90` / `first-full-release-20260812`
- Builder target: Fedora 42; `MCNF_RPM_TARGET_FEDORA=42`
- Command: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=first-full-release-20260812 MCNF_BUILD_SHAPE=big MCNF_RPM_TARGET_FEDORA=42 install-helpers/xcp-build.sh rpm`

## Gates

- `cargo build --workspace --release --locked`: passed in 25m17s.
- `cargo build --release --locked -p mde-shell-egui --features drm,live-vdi,media-mpv`: passed in 11m29s.
- `cargo generate-rpm -p crates/mesh/mackesd`: passed.
- `cargo generate-rpm -p crates/mesh/mackesd --variant lighthouse`: passed.
- `install-helpers/verify-rpm-payload.sh size` for both RPMs: passed.

## Artifacts

| Artifact | Size | SHA-256 |
|---|---:|---|
| `magic-mesh-12.1.6-33.x86_64.rpm` | 86.9 MiB | `2d548a06e05aaa9fcc03c99ee59efb0ef77ef14ed36cfd87d705910727edfabb` |
| `magic-mesh-lighthouse-12.1.6-11.x86_64.rpm` | 14.1 MiB | `bfb753ac63c4b0a0ffcbf0b10d025cd07e9420cc77cb3afe358c9280d804c4f4` |

Artifacts were pulled to `/root/mcnf-release-artifacts`. This is build/package/artifact-integrity evidence for the first full release; post-release baseline live-seat acceptance and recovery remain outstanding by policy.
