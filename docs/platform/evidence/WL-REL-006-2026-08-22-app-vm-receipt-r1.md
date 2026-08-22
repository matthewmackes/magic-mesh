# WL-REL-006 S3 leftover — current-revision App VM base-image receipt

This evidence records the farm-produced App VM base-image input for HEAD
`aca7573bc721370cefcb7d0f6628a7d39e2c2b81`. It is a registry input receipt,
not a built-image, catalog, preflight-closed, or seat-admission claim. Maps
`production_admitted` is unchanged (`false`).

## Source identity

- Source revision: `aca7573bc721370cefcb7d0f6628a7d39e2c2b81`
- Commit epoch: `1787439334`
- Farm host: `172.20.0.90` (`mcnf-build-kvm-xcp1`)
- Farm slot: `0` (`MCNF_BUILD_SLOT=0` → `~/magic-mesh-farm-0`)
- Receipt producer: `packaging/app-vm/produce-base-image-receipt.py`
- Receipt SHA-256: `f939be3864024f0e7bbfe53a26272eb796e3f85d9a35231f2a9b7ca6f4fb7891`
- Receipt mode/size: `0400` / 541 bytes

## Resolved input

- Reference: `quay.io/fedora/fedora:42`
- Architecture: `amd64`
- App VM target/profile: `mcnf-app-vm/wayland-standard-v1` / `wayland-standard`
- Manifest media type: `application/vnd.oci.image.index.v1+json`
- Resolved digest: `sha256:e78cd1a688cd079c23864f289a89a49a3f4ad66d817864e325e1d058310ee95c`
- Platform digest: `sha256:63773f454664cd77e239f8e0b13ae7f18effe9e3d6612a325b5646eb3bda11f1`

The non-secret receipt JSON stays outside Git (same as
`WL-REL-006-2026-08-16-app-vm-receipt-r1.md`). No registry credentials were
written to the receipt, the farm transcript, or this file. Control-host
private dest `/root/mcnf-private/app-vm-base-digest.json` was created
mode `0400` (no-replace). Bootc dests were not overwritten. Maps
`REPLACE_*` fields were not filled.

## Local test

```text
python3 packaging/app-vm/test-produce-base-image-receipt.py
App base-image receipt hostile self-test: PASS
```

No network. Suite covers produce/inspect, no-replace, architecture
mismatch, changed manifest, tampered revision, symlink receipt, and
duplicate-platform index refusal.

## Farm admission and Git identity

`MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=0 ./install-helpers/xcp-build.sh sync`
admitted at `102926568` KiB free (required `8388608` KiB). Farm sync omits
`.git`. An immutable depth-1 Git object store of exactly
`aca7573bc721370cefcb7d0f6628a7d39e2c2b81` was copied to
`/tmp/rel006-app-vm-repo.git` so the producer could `git show -s --format=%ct`
and match epoch `1787439334` without weakening that policy. A cloneable
`git bundle` of this one commit is not complete (parent is intentionally
omitted). Host `skopeo` 1.22.2.

## Commands and result

Canonical produce (live `skopeo inspect --raw`):

```text
python3 packaging/app-vm/produce-base-image-receipt.py \
  --repo /tmp/rel006-app-vm-repo.git produce \
  --image-reference quay.io/fedora/fedora:42 --architecture amd64 \
  --source-revision aca7573bc721370cefcb7d0f6628a7d39e2c2b81 \
  --commit-epoch 1787439334 --output /tmp/rel006-app-vm-base.json
```

Inspect against the same identity passed. Historical
`WL-REL-006-2026-08-16-app-vm-receipt-r1.md` bound `0e0cd1b3…` at epoch
`1786921850` with the same `quay.io/fedora/fedora:42` registry identity;
that revision/epoch pair is stale vs HEAD. Image selection was not
reopened (`fedora:42` per the 2026-08-16 identity; worklist S3
`fedora-bootc:44` remains the role-family input).

## Scope and leftover

This replaces the stale `0e0cd1b3` App VM receipt for the current
revision. It does not close `release-input-preflight.sh`, does not
materialize the S7 private mode-0400 argv, and does not admit Maps
production. Leftover remains Maps `production_admitted` plus live-seat
dest (`WL-TEST-002`) and S7 `REPLACE_*` (Maps/RPM/App catalog). Kiron S6
remains admitted (`verify-package.sh --source` passed against this HEAD).
