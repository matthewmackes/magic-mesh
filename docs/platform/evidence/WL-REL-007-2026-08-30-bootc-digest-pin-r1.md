# WL-REL-007 / WL-REL-006 bootc dest hunt + digest-pin refuse — 2026-08-30

Classification: leftover honesty + producer fail-close. Not a preflight
pass. Not freeze. No dest invented. Surface `bootc_base` stays null.

Tree before this change: `07672a60c`. Farm cargo units were already
fresh; this increment is dest-operator leftover, not a filler workspace
grind.

## Dest hunt (named dests only)

Selected bootc dest
`quay.io/fedora/fedora-bootc@sha256:3a5e74e668761be9e16c6779950ae154d9dcbb0861d1e92140c0751fed1f5357`
is still `manifest unknown` on quay. Live `:44` still resolves
`e8f93cc9…` and was not adopted. `podman images --digests` on
`.50`/`.90`/`.130`/`.170`/`.196` has no `3a5e74e6…` image. No private
OCI copy was found under `/root/mcnf-private` or the farm dest-root
hunt.

No Maps offline-catalog `{regions,tiles}` approval dest exists. Restored
BigBoy source-root is still PBF+TIGER. MBTiles dest `6d01a543…` was not
replaced.

## Producer fail-close

`produce-bootc-digest-receipt.py` now requires the same digest-pinned
reference shape as `release-input-argv.py` (`name@sha256:<64>`). Tag-only
`fedora-bootc:44` refuses before inspect. A pin that does not match the
inspected bytes refuses. Existing dest
`/root/mcnf-private/bootc-all-roles-digest.json` was not replaced (still
tag-only `fedora-bootc:44` bound to `479ec2b8c` / `3a5e74e6…`).

Local: `python3 install-helpers/test-produce-bootc-digest-receipt.py` →
PASS.

S7 argv was not written. Do not grind `cargo test --workspace`.
