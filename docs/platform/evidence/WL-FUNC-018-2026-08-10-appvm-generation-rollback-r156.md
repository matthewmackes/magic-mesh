# WL-FUNC-018 — App VM generation rollback refusal (r156)

Date: 2026-08-10

App VM runtime evidence is consumed in oldest-first Bus order and refuses a
later matching observation that regresses to a lower guest generation. This
prevents a delayed older VM incarnation from authorizing resume.

## Farm proof

Build VM `.90` (`172.20.0.90`), slot `func018-runtime-generation-r156`:

```text
cargo test -p mackesd --lib workers::cloud::verbs::app_image::tests::runtime_evidence_rejects_a_late_lower_generation_row -- --nocapture
1 passed; 0 failed; 0 ignored; 0 measured; 4696 filtered out
```

Live App VM boot/VDI proof remains open.
