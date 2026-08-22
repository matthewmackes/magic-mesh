# WL-REL-006 S7 leftover — private bootc receipt materialization

Date: 2026-08-22 UTC  
Classification: private dest copy of the `.170` `all-roles` bootc receipt;
**not** release-input preflight admission  
Source revision bound in the receipt: `479ec2b8c0bbf6290b68938bb36b37af9901c3f2`  
Source epoch: `1787438953`  
`production_admitted: false`

This unit copied the farm-produced bootc receipt onto the control host
private dest and wrote a bootc-bound private argv object. It does not
claim `release-input-preflight.sh` passed.

## Private dests (not in Git)

| path | mode | bytes | sha256 |
|---|---|---|---|
| `/root/mcnf-private/bootc-all-roles-digest.json` | 0400 | 411 | `2e1a183fc48de8124624881d7ec5f99770d954d81a61dcc4cf4d07919f2326ae` |
| `/root/mcnf-private/release-preflight.bootc-bound.json` | 0400 | 1044 | (private argv; not hashed into Git) |

Receipt fields: role `all-roles`, arch `amd64`, reference
`quay.io/fedora/fedora-bootc:44`, digest
`sha256:3a5e74e668761be9e16c6779950ae154d9dcbb0861d1e92140c0751fed1f5357`,
kind `mcnf-bootc-image-digest`. No registry credentials.

The operator template
`/root/mcnf-private/release-preflight.template.json` was **not**
overwritten (still `source_revision` `5de12c56b…`, bootc receipt
`REPLACE_BOOTC_BASE_DIGEST_RECEIPT`).

The bootc-bound object fills only bootc fields and binds
`source_revision`/`source_epoch` to the receipt identity. These remain
`REPLACE_*`: App VM receipts/refs, Maps approval/MBTiles/source-root,
RPM signing receipt. `maps_quota_bytes` stays `0`.

## Non-claims

- `release-input-preflight.sh` was not run as a pass.
- Maps `production_admitted` is still false.
- Dell / Seat 15 / Surface dest is not claimed.
- Later freeze will reconfirm a single candidate; this receipt is bound
  to `479ec2b8c`, not to later evidence-only commits.

## Leftover

S7 still needs operator-filled `REPLACE_*` inputs, Maps production
admission, and a preflight pass against one frozen revision. App VM S3
and Kiron S6 remain admitted.
