# WL-FUNC-018 runtime VM identity binding r3 — 2026-08-09

App-VM resume admission now binds guest runtime evidence to the exact Workloads
VM identity in addition to the session and Flatpak app identities. A fresh
observation for the right session/app but a different VM fails closed before
the existing desired-state document can change.

Farm verification used BigBoy `172.20.0.130`, slot
`func018-runtime-vm-bind-r1-20260809`:

- Focused hostile regression: 1 passed, 0 failed.
- Complete `workers::cloud::verbs::app` library slice: 26 passed, 0 failed.
- Exact-file `rustfmt --edition 2021 --check`: passed.
- Changed-path `git diff --check`: passed.
- The broader non-library target remains blocked by the pre-existing cloud-gate
  export/visibility errors; no production file outside this slice was changed.

Source hashes:

```text
d185d911af2d6ff851acae79a8a70ad73750ddc7e115c5d24f9a66a6d1e1e9a9  crates/mesh/mackesd/src/workers/cloud/verbs/app.rs
9bbb758de81ef88d077c106ff7504887aef4aa9afa20a82c8cfa16845c8cb387  crates/mesh/mackesd/src/workers/cloud/verbs/app_image.rs
```

`docs/platform/WORKLIST.md` was not edited by this slice. Live guest boot,
session-close cleanup, and five-seat acceptance remain outside this checkpoint.
