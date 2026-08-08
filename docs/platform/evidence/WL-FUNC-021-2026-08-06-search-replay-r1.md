# WL-FUNC-021 — Search generation replay guard (2026-08-06)

Music UI search updates now require a strictly newer request generation, so a
duplicate or delayed response cannot replace the accepted result. Source
SHA-256:

```text
9c1ca583e1ee995e43c8071e8141fd374243da8edf0fb68ade140f24122b6dfd  crates/desktop/mde-music-egui/src/model.rs
```

Farm command:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=music-search-generation-20260806-r1 install-helpers/xcp-build.sh cargo test --locked -p mde-music-egui model::tests::replayed_search_generation_cannot_replace_the_accepted_result -- --exact --nocapture
```

Result: **1/1 passed**. This is projection-order proof only; live provider
outage and rendered acceptance remain open.
