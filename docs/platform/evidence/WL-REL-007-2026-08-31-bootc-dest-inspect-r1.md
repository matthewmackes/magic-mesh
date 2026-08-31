# WL-REL-007 / WL-REL-006 admit selected bootc dest against a digest pin — 2026-08-31

Classification: leftover honesty + inspect glue. Not a preflight pass.
Not freeze. No dest invented. Surface `bootc_base` stays null.

Tree: `02560c5da`. Farm cargo units were already fresh; this increment
is dest-operator leftover, not a filler workspace grind.

## Dest hunt

Selected index `sha256:3a5e74e668761be9e16c6779950ae154d9dcbb0861d1e92140c0751fed1f5357`
is still `manifest unknown` as
`quay.io/fedora/fedora-bootc@sha256:3a5e74e6…`. Live tags `44` /
`latest` still resolve `e8f93cc9…`. `44-x86_64` resolves
`53dff583…`. Unpublished dest-cuts hold RPMs only. Spaces
`saved-keys/` has `release-signing/` only. No OCI copy was found.

## Inspect glue

`produce-bootc-digest-receipt.py inspect` now admits an already-selected
tag-only dest receipt when the expected reference is digest-pinned to
that receipt's `resolved_digest` and names the same repository.
Produce still refuses tag-only and mismatched pins.

Local dest probe against
`/root/mcnf-private/bootc-all-roles-digest.json` with
`quay.io/fedora/fedora-bootc@sha256:3a5e74e6…` at receipt revision
`479ec2b8c` / `1787438953` → PASS.

`python3 install-helpers/test-produce-bootc-digest-receipt.py` → PASS.

S7 argv was not written: Maps catalog dest is bound to `f8dce4e0c` and
RPM/App receipts to `54ee58acf`. Do not grind `cargo test --workspace`.
