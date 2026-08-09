# WL-FUNC-019 universal Remote Sessions presentation S3 — 2026-08-08

Remote Sessions now ingests the bounded universal resource snapshot into a pure
presentation model. Operators can search, filter by resource kind, and inspect
deterministically grouped cards with capability badges, provenance, freshness,
and explicit unavailable, reconnecting, and identity-conflict states.

The render path performs no Bus, network, filesystem, provider, or backend I/O.
Cards without an admitted typed action remain actionless; the UI does not infer
a command, endpoint, path, or fallback target.

## Verification

- Farm `.90`, slot `func019-resource-browser-ui-s3-r1`:
  `cargo test --locked -p mde-shell-egui --bin mde-shell-egui
  'vdi::resources::tests::remote_sessions_model_' -- --nocapture` passed 4/4,
  with 1,475 unrelated tests filtered.
- Fixtures cover search, type filtering, deterministic grouping, badges,
  provenance/freshness, and unavailable/reconnecting/conflict presentation.
- Scoped formatting and `git diff --check` passed.

## Remaining acceptance gap

Typed action authority, deterministic wide/narrow/largest-text captures, and
live loss/rejoin proof remain. FUNC-019 stays `Remaining`.
