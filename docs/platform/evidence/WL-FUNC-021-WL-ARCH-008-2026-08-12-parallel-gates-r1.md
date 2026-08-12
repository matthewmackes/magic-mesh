# WL-FUNC-021 / WL-ARCH-008 — parallel farm gates (r1)

- Date: 2026-08-12
- Source revision tested: `bdde8b16` workspace with the preceding `9b9af886` music queue admission slice
- Lanes: `.90` / `func021-music-20260812`; `.50` / `arch008-browser-control-20260812`

## WL-FUNC-021

Command: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func021-music-20260812 MCNF_BUILD_SHAPE=small install-helpers/xcp-build.sh cargo test -p mde-musicd --locked`

- Initial result: 260 passed, 8 failed, 0 ignored, 268 total. The failures exposed that cross-peer stale roster snapshots were incorrectly rejected by the global authority check.
- Implemented the bounded fix in `crates/services/mde-musicd/src/state.rs`: same-peer stale/equivocal revisions remain refused; a stale snapshot for another peer updates only that peer's roster file and cannot replace newer global authority.
- Focused farm regressions passed: `read_all_peer_states_collects_and_sorts_snapshots`, `typed_workspace_queue_actions_use_the_shared_queue_authority`, and `workspace_targets_project_fresh_idle_and_refused_peer_heartbeats` (1/1 each).
- Serial full farm gate (`-- --test-threads=1`): 263 passed, 5 failed, 268 total. The remaining five are provider-admission mutation assertions; the same bookmark path passes in isolation, so they remain a separate provider-test blocker.
- Additional isolated farm checks passed: `typed_star_actions_use_admitted_provider_and_refuse_other_sources` and `typed_source_curation_uses_the_selected_admitted_provider` (1/1 each on independent `.90` slots). The remaining provider failures therefore do not reproduce in isolated execution and are retained as test-fixture interference, not accepted as a production green suite.
- Farm clippy: `cargo clippy -p mde-musicd --locked --lib` passed with warnings only (253).

## WL-ARCH-008

Command: `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=arch008-browser-control-20260812 MCNF_BUILD_SHAPE=small install-helpers/xcp-build.sh cargo test --manifest-path install-helpers/browser-vm-production-control/Cargo.toml --locked`

- Result: helper cargo test could not start cleanly because Cargo attempted to update the standalone helper lockfile while `--locked` forbade it; the workspace compile then proceeded but the command is not accepted as a green locked gate.
- Independent local boundary self-test: `install-helpers/lint-browser-vm-boundary.sh --self-test` passed.
- Status: locked standalone helper gate needs a lockfile synchronization decision before it can be accepted.
