# WL-REL-006 leftover — App VM Containerfile base pin is producer-admissible

This evidence records that `packaging/app-vm/Containerfile` `ARG APP_VM_BASE`
now names the already-admitted `:44` index digest. It is a producer-admissibility
record for that pin, not a built-image, catalog, preflight-closed, Maps, or
seat-admission claim. Maps `production_admitted` is unchanged (`false`). Source
selection is not reopened. This is not Maps admission.

## Source identity

- Source revision: `ace25eff596298371b093983bac17732df9b113c`
- Commit epoch: `1787440569`
- Farm host: `172.20.0.90` (`mcnf-build-kvm-xcp1`)
- Farm slot: `0` (`MCNF_BUILD_SLOT=0` → `~/magic-mesh-farm-0`)
- Receipt producer: `packaging/app-vm/produce-base-image-receipt.py`
- Farm produce dest: `/tmp/rel006-app-vm-base-pin.json` (farm only)
- Receipt SHA-256: `24162e5dffb7628ff12a699122f9432830f9323efe1627277c10ae3317fa18fa`
- Receipt mode/size: `0400` / 616 bytes

## Resolved input

- Reference: `quay.io/fedora/fedora-bootc@sha256:3a5e74e668761be9e16c6779950ae154d9dcbb0861d1e92140c0751fed1f5357`
- Architecture: `amd64`
- App VM target/profile: `mcnf-app-vm/wayland-standard-v1` / `wayland-standard`
- Manifest media type: `application/vnd.oci.image.index.v1+json`
- Resolved digest: `sha256:3a5e74e668761be9e16c6779950ae154d9dcbb0861d1e92140c0751fed1f5357`
- Platform digest: `sha256:68a6e45b472699e311fe59b46734a7579302febd99d43ee27ddcfb47278911ba`

The non-secret receipt JSON stays on the farm under `/tmp`. Control-host
private dest `/root/mcnf-private/app-vm-base-digest.json` was not replaced
(still sha256
`f939be3864024f0e7bbfe53a26272eb796e3f85d9a35231f2a9b7ca6f4fb7891`,
mode `0400`, bound to `aca7573bc` / `quay.io/fedora/fedora:42`). No
`/root/mcnf-private/app-vm-base-pin.json` was written. Bootc, Browser VM,
and Maps dests were not overwritten. Image layers were not pulled
(`skopeo inspect --raw` only). Host `.90` already had `skopeo` 1.22.2.

## Local test

```text
python3 packaging/app-vm/test-produce-base-image-receipt.py
App base-image receipt hostile self-test: PASS

bash packaging/app-vm/verify-contract.sh
App VM contract checks passed
```

No network. `verify-contract.sh` does not hardcode a digest; it only
requires exactly one `APP_VM_BASE` ARG and an immutable
`quay.io/fedora/fedora-bootc@sha256:<64 hex>` value. Hostile uniqueness
fixtures still use `:44`. `FROM ${APP_VM_BASE}` is unchanged.

## Farm admission and Git identity

`MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=0 ./install-helpers/xcp-build.sh sync`
admitted at `102733512` KiB free (required `8388608` KiB). Farm sync omits
`.git`. An immutable depth-1 Git object store of exactly
`ace25eff596298371b093983bac17732df9b113c` was copied to
`/tmp/rel006-app-vm-pin-repo.git` so the producer could
`git show -s --format=%ct` and match epoch `1787440569` without weakening
that policy. The store contains one commit. Host `skopeo` 1.22.2.

## Commands and result

Farm `bash packaging/app-vm/verify-contract.sh` passed. Canonical produce
against the admitted index (live `skopeo inspect --raw`):

```text
python3 packaging/app-vm/produce-base-image-receipt.py \
  --repo /tmp/rel006-app-vm-pin-repo.git produce \
  --image-reference quay.io/fedora/fedora-bootc@sha256:3a5e74e668761be9e16c6779950ae154d9dcbb0861d1e92140c0751fed1f5357 \
  --architecture amd64 \
  --source-revision ace25eff596298371b093983bac17732df9b113c \
  --commit-epoch 1787440569 --output /tmp/rel006-app-vm-base-pin.json
```

Inspect against the same identity passed. Raw index bytes hash to
`sha256:3a5e74e668761be9e16c6779950ae154d9dcbb0861d1e92140c0751fed1f5357`
(`application/vnd.oci.image.index.v1+json`). This matches the Browser VM and
bootc all-roles admitted index. The Containerfile ARG now names that index.
This is not a built App VM image, not a curated catalog, and does not close
`release-input-preflight.sh`.

## Scope and leftover

The Containerfile pin now matches the admitted index. Leftover remains Maps
`production_admitted`, App catalog real refs, RPM signer after freeze, S7
`REPLACE_*`, and live-seat dest (`WL-TEST-002`). Kiron S6 remains admitted.
Do not claim the release-input gate closed.
