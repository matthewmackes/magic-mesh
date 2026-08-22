# WL-REL-006 leftover — bootc Containerfile ARG default is producer-admissible

This evidence records that `packaging/bootc/Containerfile` `ARG
BOOTC_BASE` now names the already-admitted all-roles index digest. It is
ARG-default alignment so a raw `podman build` without `--build-arg`
cannot default to a zero digest. It is not a built-image, catalog,
preflight-closed, Maps, Surface, or seat-admission claim, and it is not
a new role admission. Source selection is not reopened. Maps
`production_admitted` is unchanged (`false`). Surface `bootc_base`
remains null.

## Source identity

- Worktree HEAD (this unit): `ace25eff596298371b093983bac17732df9b113c`
- Commit epoch (this unit): `1787440569`
- Farm host: `172.20.0.50` (`mcnf-build-home-services`)
- Farm slot: `0` (`MCNF_BUILD_SLOT=0` → `~/magic-mesh-farm-0`)
- Bound receipt identity (unchanged): `479ec2b8c0bbf6290b68938bb36b37af9901c3f2`
- Bound receipt epoch: `1787438953`
- Receipt SHA-256 (unchanged private dest):
  `2e1a183fc48de8124624881d7ec5f99770d954d81a61dcc4cf4d07919f2326ae`

No new receipt was produced. `install-helpers/produce-bootc-digest-receipt.py`
was not run. Control-host private dest
`/root/mcnf-private/bootc-all-roles-digest.json` was not replaced
(still bound to `479ec2b8c`). Treadmill re-bind is forbidden.

## Resolved input

- Previous ARG default (replaced; selection not reopened):
  `quay.io/fedora/fedora-bootc:44@sha256:0000000000000000000000000000000000000000000000000000000000000000`
- New ARG default:
  `quay.io/fedora/fedora-bootc@sha256:3a5e74e668761be9e16c6779950ae154d9dcbb0861d1e92140c0751fed1f5357`
- `FROM ${BOOTC_BASE}` is unchanged.
- Manifest media type: `application/vnd.oci.image.index.v1+json`
- Raw index bytes hash:
  `sha256:3a5e74e668761be9e16c6779950ae154d9dcbb0861d1e92140c0751fed1f5357`

This matches the already-admitted all-roles index in
`WL-REL-006-2026-08-22-bootc-all-roles-r1.md` (receipt `479ec2b8c`).
Image layers were not pulled (`skopeo inspect --raw` only). Host `.50`
already had `skopeo` 1.22.2; no package install.

`packaging/bootc/build-image.sh` was not edited (it already overrides
`BOOTC_BASE` from the generated surface lock).
`packaging/surface/surface-stack.f44.json` was not edited
(`bootc_base` is still null / Surface blocked).

## Local test

```text
rg -n '^ARG BOOTC_BASE=' packaging/bootc/Containerfile
53:ARG BOOTC_BASE=quay.io/fedora/fedora-bootc@sha256:3a5e74e668761be9e16c6779950ae154d9dcbb0861d1e92140c0751fed1f5357

rg -n '0000000000000000000000000000000000000000000000000000000000000000' packaging/bootc/Containerfile
(no matches)
```

No network. No local cargo. No new receipt bind.

## Farm admission and Git identity

`MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=0 ./install-helpers/xcp-build.sh sync`
admitted at `71287708` KiB free (required `8388608` KiB). Farm sync omits
`.git`. This unit did not copy a Git object store and did not produce a
HEAD-bound receipt. Host `skopeo` 1.22.2.

## Commands and result

Canonical inspect against the admitted index (live `skopeo inspect --raw`):

```text
skopeo inspect --raw \
  docker://quay.io/fedora/fedora-bootc@sha256:3a5e74e668761be9e16c6779950ae154d9dcbb0861d1e92140c0751fed1f5357
```

Inspect against that identity passed. `mediaType` is
`application/vnd.oci.image.index.v1+json`. Raw index bytes hash to
`sha256:3a5e74e668761be9e16c6779950ae154d9dcbb0861d1e92140c0751fed1f5357`.
Farm-slot Containerfile ARG names that index; the 64-zero digest is
absent. This is not a bootc role admission and does not close
`release-input-preflight.sh`.

## Scope and leftover

The Containerfile ARG default now matches the admitted index
`3a5e74e6…` / bootc receipt `479ec2b8c`. Leftover remains Maps
`production_admitted`, App catalog real refs, RPM signer after freeze,
S7 `REPLACE_*`, live-seat dest (`WL-TEST-002`), and Surface `bootc_base`
still null. Kiron S6 remains admitted. Do not claim the release-input
gate closed. Do not claim Surface stack or preflight closed.
