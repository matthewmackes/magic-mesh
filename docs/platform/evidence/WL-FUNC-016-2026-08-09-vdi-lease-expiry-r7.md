# WL-FUNC-016 VDI lease-expiry permission boundary (r7)

- Date: 2026-08-09
- Base commit: `e5b545fdfa77c6e614d8a74af41fc7103457e571`
- Scope: `crates/desktop/mde-shell-egui/src/clipboard_permissions.rs`
- Source SHA-256: `4d0902669af19d7495501bd6d4d213114f98fb19f31c621fb6e5d245d1e4129b`

## Correction

VDI permission metadata now expires at the earlier of the rich clipboard offer
and its admitting VDI lease. The direct model path retains the same admitted VDI
metadata as the transport ingress instead of rebuilding a non-VDI request and
discarding lease authority. An approval attempted at lease expiry fails closed,
revokes the one-use materialization ticket, records terminal expiry, and retains
the source sequence through the existing replay cleanup path.

## Farm proof

Host `172.20.0.50`, slot `func016-vdi-lease-expiry-r1-20260809`:

- Exact-file `rustfmt --check`: passed.
- Focused lease-expiry regression: 1 passed, 0 failed.
- Complete `clipboard_permissions::tests` suite: 12 passed, 0 failed.
- Scoped `git diff --check`: passed locally after farm formatting.

The shell build emitted pre-existing warnings outside this correction; no test
failure or warning was introduced as an acceptance substitute. No live VDI guest
was used, so this is deterministic production-path contract evidence, not live
guest release proof.
