# WL-FUNC-018 unavailable runtime admission r2 — 2026-08-09

App-VM runtime evidence in the explicit `unavailable` state is no longer usable
for a resume or desired-state refresh. The typed observation remains visible for
diagnosis, but production admission now refuses it before Workloads desired
state can be changed.

Hostile regressions prove that a fresh, identity-matching `unavailable`
observation cannot become launch readiness and that a resume carrying a newer
catalog revision leaves the existing desired-state document byte-for-byte
unchanged.

Farm verification used `172.20.0.90`, slot
`func018-appvm-unavailable-r1-20260809`:

- `cargo test -p mackesd --lib --features async-services workers::cloud::verbs::app --locked -- --nocapture`: 25 passed, 0 failed.
- Exact-file `rustfmt --edition 2021 --check`: passed.
- Changed-path `git diff --check`: passed.

Source hashes:

```text
b7be6f2ace944d990f4d68f94b6e9e51e4c24153885721ef74b27a4d1a32c9f0  crates/mesh/mackesd/src/workers/cloud/verbs/app.rs
9d19867a9e199c91f297cc4ed62b46a1849593c20eec625ffd446a17eb17b474  crates/mesh/mackesd/src/workers/cloud/verbs/app_image.rs
```

`docs/platform/WORKLIST.md` was not edited by this slice. Live guest boot and
five-seat App-VM acceptance remain outside this bounded checkpoint.
