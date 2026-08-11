# WL-FUNC-019 manual-source metadata replacement — 2026-08-11

- Scope: authenticated `action/desktops/add-source` updates now atomically
  replace changed display metadata for an existing stable endpoint identity.
  Exact replays remain idempotent, durable state retains one row, and the
  universal resource projection publishes the current metadata.
- Farm: BigBoy `172.20.0.130` after `.50` refused sync at its free-space guard.
- Focused gate: `install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::desktop_sources::tests::add_source_verb_replaces_metadata_for_the_same_stable_identity -- --exact --nocapture`.
- Result: PASS, 1 passed, 0 failed.
