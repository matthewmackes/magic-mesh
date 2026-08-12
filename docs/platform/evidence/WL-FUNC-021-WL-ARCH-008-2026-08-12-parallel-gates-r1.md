# WL-FUNC-021 / WL-ARCH-008 — parallel farm gates (r1)

- Date: 2026-08-12
- Source revision tested: `bdde8b16` workspace with the preceding `9b9af886` music queue admission slice
- Lanes: `.90` / `func021-music-20260812`; `.50` / `arch008-browser-control-20260812`

## WL-FUNC-021

Command: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func021-music-20260812 MCNF_BUILD_SHAPE=small install-helpers/xcp-build.sh cargo test -p mde-musicd --locked`

- Result: 260 passed, 8 failed, 0 ignored, 268 total.
- Failures are durable revision/order regressions in provider mutations, workspace queue actions, and peer-state reads; the exact error is `music state revision ... conflicts with durable revision ...` or its propagated mutation error.
- A follow-up rerun after a monotonic-clock experiment still failed 259 passed / 9 failed, including fixed-revision peer fixtures. The experiment was reverted; no unverified production change was retained.
- Status: blocker remains; no acceptance claim is made.

## WL-ARCH-008

Command: `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=arch008-browser-control-20260812 MCNF_BUILD_SHAPE=small install-helpers/xcp-build.sh cargo test --manifest-path install-helpers/browser-vm-production-control/Cargo.toml --locked`

- Result: helper cargo test could not start cleanly because Cargo attempted to update the standalone helper lockfile while `--locked` forbade it; the workspace compile then proceeded but the command is not accepted as a green locked gate.
- Independent local boundary self-test: `install-helpers/lint-browser-vm-boundary.sh --self-test` passed.
- Status: locked standalone helper gate needs a lockfile synchronization decision before it can be accepted.
