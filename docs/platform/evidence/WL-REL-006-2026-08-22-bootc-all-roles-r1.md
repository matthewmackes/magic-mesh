# WL-REL-006 S5 leftover — current-revision bootc `all-roles` receipt

This evidence records the farm-produced bootc input for HEAD
`479ec2b8c0bbf6290b68938bb36b37af9901c3f2`. It is an input receipt, not a
release, preflight-closed, or seat-admission claim. Maps
`production_admitted` is unchanged (`false`).

## Source identity

- Source revision: `479ec2b8c0bbf6290b68938bb36b37af9901c3f2`
- Commit epoch: `1787438953`
- Farm host: `172.20.0.170` (`mcnf-build-xen-194`)
- Farm slot: `0` (`MCNF_BUILD_SLOT=0` → `~/magic-mesh-farm-0`)
- Receipt producer: `install-helpers/produce-bootc-digest-receipt.py`
- Receipt SHA-256: `2e1a183fc48de8124624881d7ec5f99770d954d81a61dcc4cf4d07919f2326ae`
- Receipt mode/size: `0400` / 411 bytes

## Resolved input

- Reference: `quay.io/fedora/fedora-bootc:44`
- Architecture: `amd64`
- Release role: `all-roles`
- Manifest media type: `application/vnd.oci.image.index.v1+json`
- Resolved digest: `sha256:3a5e74e668761be9e16c6779950ae154d9dcbb0861d1e92140c0751fed1f5357`

The non-secret receipt JSON stays outside Git (same as
`WL-REL-006-2026-08-16-bootc-receipt-r1.md`). No registry credentials were
written to the receipt, the farm transcript, or this file.

## Local test

```text
python3 install-helpers/test-produce-bootc-digest-receipt.py
bootc digest receipt hostile self-test: PASS
```

No network. Suite covers `all-roles` produce/inspect and refuses legacy
`base` / `unified-seat-server` before any registry read.

## Farm admission and Git identity

`MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=0 ./install-helpers/xcp-build.sh sync`
admitted at `13472020` KiB free (required `8388608` KiB). Farm sync omits
`.git`. An immutable depth-1 Git object store of exactly
`479ec2b8c0bbf6290b68938bb36b37af9901c3f2` was copied to
`/tmp/rel006-bootc-repo.git` so the producer could `rev-parse` and match
epoch `1787438953` without weakening that policy. A cloneable
`git bundle` of this one commit is not complete (parent `ab4a9d55…` is
intentionally omitted).

## Commands and result

Legacy `base` refused (exit 2); no file written:

```text
python3 install-helpers/produce-bootc-digest-receipt.py \
  --repo /tmp/rel006-bootc-repo.git produce \
  --image-reference quay.io/fedora/fedora-bootc:44 --architecture amd64 \
  --source-revision 479ec2b8c0bbf6290b68938bb36b37af9901c3f2 \
  --commit-epoch 1787438953 --release-role base \
  --output /tmp/rel006-bootc-base-must-not-exist.json
bootc-digest-receipt: REFUSED: bootc receipt refuses legacy base role identity
```

Canonical produce (live `skopeo inspect --raw`; host `skopeo` 1.22.2):

```text
python3 install-helpers/produce-bootc-digest-receipt.py \
  --repo /tmp/rel006-bootc-repo.git produce \
  --image-reference quay.io/fedora/fedora-bootc:44 --architecture amd64 \
  --source-revision 479ec2b8c0bbf6290b68938bb36b37af9901c3f2 \
  --commit-epoch 1787438953 --release-role all-roles \
  --output /tmp/rel006-bootc-all-roles.json
```

Inspect against the same identity passed. Historical
`WL-REL-006-2026-08-16-bootc-receipt-r1.md` bound `52fd0793…` with
`--release-role base` and digest
`sha256:295dd6ecda23780e9babf6a889914762ae118c621819d777c879992884d2b681`;
that role identity is now `LEGACY_ROLES` and is refused.

## Scope and leftover

This replaces the stale `base` receipt for the current revision. It does
not close `release-input-preflight.sh`, does not materialize the S7
private mode-0400 argv, and does not admit Maps production. Leftover
remains Maps `production_admitted` plus live-seat dest (`WL-TEST-002`)
and S7 private preflight. App VM S3 and Kiron S6 remain admitted.
