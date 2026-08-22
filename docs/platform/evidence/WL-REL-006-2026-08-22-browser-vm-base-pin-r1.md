# WL-REL-006 leftover — Browser VM Containerfile base pin is producer-admissible

This evidence records that `packaging/browser-vm/Containerfile` `ARG
BROWSER_VM_BASE` now names the already-admitted `:44` index digest. It is a
producer-admissibility record for that pin, not a built-image, catalog,
preflight-closed, Maps, or seat-admission claim. Maps `production_admitted`
is unchanged (`false`). Source selection is not reopened.

## Source identity

- Source revision: `834aab9cd2d33e7e547f7c89519ca5dc8971e652`
- Commit epoch: `1787440328`
- Farm host: `172.20.0.50` (`mcnf-build-home-services`)
- Farm slot: `0` (`MCNF_BUILD_SLOT=0` → `~/magic-mesh-farm-0`)
- Receipt producer: `packaging/browser-vm/produce-base-image-receipt.py`
- Farm produce dest: `/tmp/rel006-browser-vm-base-pin.json` (farm only)
- Receipt SHA-256: `805600158e9ea8355b7113c74cdba22a9344ec8a02366922d211a5894c507754`
- Receipt mode/size: `0400` / 632 bytes

## Resolved input

- Reference: `quay.io/fedora/fedora-bootc@sha256:3a5e74e668761be9e16c6779950ae154d9dcbb0861d1e92140c0751fed1f5357`
- Architecture: `amd64`
- Browser VM target/profile: `mcnf-browser-vm/browser-vm-chromium-v1` /
  `browser-vm-chromium`
- Manifest media type: `application/vnd.oci.image.index.v1+json`
- Resolved digest: `sha256:3a5e74e668761be9e16c6779950ae154d9dcbb0861d1e92140c0751fed1f5357`
- Platform digest: `sha256:68a6e45b472699e311fe59b46734a7579302febd99d43ee27ddcfb47278911ba`

The non-secret receipt JSON stays on the farm under `/tmp`. Control-host
private dest `/root/mcnf-private/browser-vm-base-digest.json` was not
replaced (still sha256
`ac9755db790445048eb621542b69ec24220b58ecec3e056a9e570309b7c100a9`,
mode `0400`, bound to `b30954e31` / `:44`). No
`/root/mcnf-private/browser-vm-base-pin.json` was written. Bootc, App VM,
and Maps dests were not overwritten. Image layers were not pulled
(`skopeo inspect --raw` only). Host `.50` lacked `skopeo`; `dnf install`
placed the same `1.22.2` already present on `.90`.

## Local test

```text
python3 packaging/browser-vm/test-produce-base-image-receipt.py
Browser base-image receipt hostile self-test: PASS

bash packaging/browser-vm/verify-contract.sh --base-receipt-self-test
Browser base-image receipt contract self-test passed
```

No network. `verify-contract.sh` does not hardcode a digest; it only
requires an immutable `quay.io/fedora/fedora-bootc@sha256:<64 hex>` ARG.
`FROM ${BROWSER_VM_BASE}` is unchanged.

## Farm admission and Git identity

`MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=0 ./install-helpers/xcp-build.sh sync`
admitted at `71298952` KiB free (required `8388608` KiB). Farm sync omits
`.git`. An immutable depth-1 Git object store of exactly
`834aab9cd2d33e7e547f7c89519ca5dc8971e652` was copied to
`/tmp/rel006-browser-vm-pin-repo.git` so the producer could
`git show -s --format=%ct` and match epoch `1787440328` without weakening
that policy. The store contains one commit. Host `skopeo` 1.22.2.

## Digest-pin refusal (old Containerfile ARG)

Previous ARG (now replaced; selection not reopened):
`quay.io/fedora/fedora-bootc@sha256:3b80fff7ae609cc4c0ea6a1c728e32003a72719d1e0441637894a46ce840b0fe`

```text
python3 packaging/browser-vm/produce-base-image-receipt.py \
  --repo /tmp/rel006-browser-vm-pin-repo.git produce \
  --image-reference quay.io/fedora/fedora-bootc@sha256:3b80fff7ae609cc4c0ea6a1c728e32003a72719d1e0441637894a46ce840b0fe \
  --architecture amd64 \
  --source-revision 834aab9cd2d33e7e547f7c89519ca5dc8971e652 \
  --commit-epoch 1787440328 --output /tmp/rel006-browser-vm-old-pin-must-not-exist.json
browser-base-image-receipt: REFUSED: registry media type is absent or unsupported
```

Exit 2. No receipt file was written. `skopeo inspect --raw` of that digest
returned schemaVersion 2 with `annotations`/`config`/`layers` and no
`mediaType` (producer requires an OCI/Docker manifest or index media type).

## Commands and result

Canonical produce against the admitted index (live `skopeo inspect --raw`):

```text
python3 packaging/browser-vm/produce-base-image-receipt.py \
  --repo /tmp/rel006-browser-vm-pin-repo.git produce \
  --image-reference quay.io/fedora/fedora-bootc@sha256:3a5e74e668761be9e16c6779950ae154d9dcbb0861d1e92140c0751fed1f5357 \
  --architecture amd64 \
  --source-revision 834aab9cd2d33e7e547f7c89519ca5dc8971e652 \
  --commit-epoch 1787440328 --output /tmp/rel006-browser-vm-base-pin.json
```

Inspect against the same identity passed. Raw index bytes hash to
`sha256:3a5e74e668761be9e16c6779950ae154d9dcbb0861d1e92140c0751fed1f5357`
(`application/vnd.oci.image.index.v1+json`). This matches the current-revision
`:44` receipt in `WL-REL-006-2026-08-22-browser-vm-receipt-r1.md`. The
Containerfile ARG now names that index. This is not a bootc role admission
and does not close `release-input-preflight.sh`.

## Scope and leftover

The Containerfile pin now matches the admitted index. Leftover remains Maps
`production_admitted`, App catalog real refs, RPM signer after freeze, S7
`REPLACE_*`, and live-seat dest (`WL-TEST-002`). Kiron S6 remains admitted.
Do not claim the release-input gate closed.
