# WL-REL-006 leftover — current-revision Browser VM base-image receipt

This evidence records the farm-produced Browser VM base-image input for HEAD
`b30954e31edb25264313c0e2a3d217b58e6e2d0b`. It is a registry input receipt,
not a built-image, catalog, preflight-closed, or seat-admission claim. Maps
`production_admitted` is unchanged (`false`).

## Source identity

- Source revision: `b30954e31edb25264313c0e2a3d217b58e6e2d0b`
- Commit epoch: `1787440151`
- Farm host: `172.20.0.90` (`mcnf-build-kvm-xcp1`)
- Farm slot: `1` (`MCNF_BUILD_SLOT=1` → `~/magic-mesh-farm-1`)
- Receipt producer: `packaging/browser-vm/produce-base-image-receipt.py`
- Receipt SHA-256: `ac9755db790445048eb621542b69ec24220b58ecec3e056a9e570309b7c100a9`
- Receipt mode/size: `0400` / 563 bytes

## Resolved input

- Reference: `quay.io/fedora/fedora-bootc:44` (after digest-pin refusal below)
- Architecture: `amd64`
- Browser VM target/profile: `mcnf-browser-vm/browser-vm-chromium-v1` /
  `browser-vm-chromium`
- Manifest media type: `application/vnd.oci.image.index.v1+json`
- Resolved digest: `sha256:3a5e74e668761be9e16c6779950ae154d9dcbb0861d1e92140c0751fed1f5357`
- Platform digest: `sha256:68a6e45b472699e311fe59b46734a7579302febd99d43ee27ddcfb47278911ba`

The non-secret receipt JSON stays outside Git. No registry credentials were
written to the receipt, the farm transcript, or this file. Control-host
private dest `/root/mcnf-private/browser-vm-base-digest.json` was created
mode `0400` (no-replace). Bootc and App VM dests were not overwritten. Maps
`REPLACE_*` fields were not filled. Image layers were not pulled
(`skopeo inspect --raw` only).

## Local test

```text
python3 packaging/browser-vm/test-produce-base-image-receipt.py
Browser base-image receipt hostile self-test: PASS
```

No network. Suite covers produce/inspect, no-replace, architecture
mismatch, changed manifest, tampered revision, symlink receipt, and
duplicate-platform index refusal.

## Farm admission and Git identity

`MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=1 ./install-helpers/xcp-build.sh sync`
admitted at `102830204` KiB free (required `8388608` KiB). Slot 0 on `.90`
still held the App VM leftover (`/tmp/rel006-app-vm-base.json`); this unit
used slot 1. Farm sync omits `.git`. An immutable depth-1 Git object store
of exactly `b30954e31edb25264313c0e2a3d217b58e6e2d0b` was copied to
`/tmp/rel006-browser-vm-repo.git` so the producer could
`git show -s --format=%ct` and match epoch `1787440151` without weakening
that policy. A cloneable `git bundle` of this one commit is not complete
(parent is intentionally omitted). Host `skopeo` 1.22.2.

## Digest-pin refusal (recorded before `:44`)

Containerfile pin (selection not reopened):
`quay.io/fedora/fedora-bootc@sha256:3b80fff7ae609cc4c0ea6a1c728e32003a72719d1e0441637894a46ce840b0fe`

```text
python3 packaging/browser-vm/produce-base-image-receipt.py \
  --repo /tmp/rel006-browser-vm-repo.git produce \
  --image-reference quay.io/fedora/fedora-bootc@sha256:3b80fff7ae609cc4c0ea6a1c728e32003a72719d1e0441637894a46ce840b0fe \
  --architecture amd64 \
  --source-revision b30954e31edb25264313c0e2a3d217b58e6e2d0b \
  --commit-epoch 1787440151 --output /tmp/rel006-browser-vm-base.json
browser-base-image-receipt: REFUSED: registry media type is absent or unsupported
```

Exit 2. No receipt file was written. `skopeo inspect --raw` of that digest
returned schemaVersion 2 with `annotations`/`config`/`layers` and no
`mediaType` (producer requires an OCI/Docker manifest or index media type).

## Commands and result

Canonical produce after the recorded refusal (live `skopeo inspect --raw`):

```text
python3 packaging/browser-vm/produce-base-image-receipt.py \
  --repo /tmp/rel006-browser-vm-repo.git produce \
  --image-reference quay.io/fedora/fedora-bootc:44 --architecture amd64 \
  --source-revision b30954e31edb25264313c0e2a3d217b58e6e2d0b \
  --commit-epoch 1787440151 --output /tmp/rel006-browser-vm-base.json
```

Inspect against the same identity passed. The `:44` index digest matches the
current bootc `all-roles` registry identity; this receipt is the Browser VM
kind/target/profile binding, not a bootc role admission.

## Scope and leftover

This binds a current-revision Browser VM receipt for `b30954e31`. It does
not close `release-input-preflight.sh`, does not materialize the S7 private
mode-0400 argv, and does not admit Maps production. Leftover remains Maps
`production_admitted`, App catalog real refs, RPM signer after freeze, S7
`REPLACE_*`, and live-seat dest (`WL-TEST-002`). Kiron S6 remains admitted.
