# WL-FUNC-031 space-scoped Documents share sessions

- Date: 2026-08-20
- Scope: Documents share-session lifecycle seam
- Behavior: deterministic session topics now include both `SpaceId` and
  `DocumentId`, so one document linked into multiple spaces cannot cross-talk
  through a single `collab/session/<id>` topic.
- Regression: `same_document_gets_independent_session_topics_per_space`
  asserts two Start intents for the same document produce distinct session
  identifiers.
- Farm: XEN-194 `172.20.0.170`, slot `func031-session-isolation`
- Command: `cargo test -p mde-collab-egui --lib same_document_gets_independent_session_topics_per_space -- --nocapture`
- Result: 1 passed, 0 failed, 162 filtered out.
- Full crate follow-up: `cargo test -p mde-collab-egui --lib -- --nocapture`
  passed with 163 tests, 0 failed, 0 ignored on XEN-194 slot
  `func031-full-lifecycle`.
- Note: the initial BigBoy admission was refused because `/home` had only
  3.1 GiB free against the 8 GiB sync headroom; the same admitted job passed
  on XEN-194 with 15 GiB free.
