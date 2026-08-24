# WL-FUNC-023 — enroll --token-stdin uses fingerprint-pinned join — r1

Date: 2026-08-23  
Classification: source glue; **not** live re-enroll and **not**
`production_admitted`  
`production_admitted: false`

## Authority

Operator 2026-08-23: execute leftover demand now. Do not park an
open-source / already-chosen path.

## Change

Leftover (3) on Seat 15 needed current-tree `join` because installed
`enroll --token-stdin` called retired `enroll_with_token` (CSR publish).

`enroll --token-stdin` now routes a `mesh:…#…?fp=` token to `join::run`.
A token without a TLS pin, or a non-token, fails closed with the retired
CSR message.

`token_uses_join_path` is tested next to the existing `?fp=` parser.

## Farm

`MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=enroll-join-023`

`install-helpers/xcp-build.sh cargo test -p mackesd --lib nebula_enroll -- --nocapture`

`test result: ok. 102 passed; 0 failed`

## Still leftover

Installed `magic-mesh-13.0.0-35` still has the CSR stub until a
current-revision package is on a seat. Live leftover (3) identity on
Seat 15 is already rematerialized. Do not flip `production_admitted`.
