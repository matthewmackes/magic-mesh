# WL-FUNC-023 lifecycle-authority farm evidence — 2026-08-16

- Source revision: `d8e811e2a8a77ffc428790ed6e7c6401651c8c23`
- Farm host: `172.20.0.130` (BigBoy)
- Farm slot: `wl-func023-lifecycle-warm-20260816`
- Command: `target/debug/deps/mackesd_core-2996e67e4f6aede8 lifecycle_authority --nocapture`
- Result: `17 passed, 0 failed, 5004 filtered out`

The focused tests cover target/generation binding, exclusive authority,
atomic checkpoints, interruption/resume, correction planning, fleet report
truthfulness, pinned and unsigned artifact admission, commissioning capsule
retry/revocation, confirmation scope, readiness warnings, terminal progress,
and offboarding receipt completion. This is product-core evidence only; live
SSH bootstrap, live Bus acknowledgement, package integration, and physical
seat acceptance remain open.

## Follow-up verification — 2026-08-17

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=1 \
  install-helpers/xcp-build.sh cargo test -p mackesd lifecycle_authority \
  --features async-services -- --nocapture
```

- BigBoy `172.20.0.130`, warmed slot `1`.
- Result: `17 passed, 0 failed, 5006 filtered out`.
- The complete lifecycle-authority test target passed, including capsule
  retry/revocation, artifact and confirmation binding, interruption/resume,
  fleet-report truthfulness, readiness, and offboarding receipt tests.
- This remains product-core evidence and does not satisfy live/provider,
  package/first-boot, or physical-seat acceptance.

## Package/first-boot structural follow-up — 2026-08-17

The two focused package gates passed from the current worktree:

```text
install-helpers/test-rpm-seat-service-activation.sh
  contract passed; shell syntax passed
install-helpers/test-boot-status-upgrade.sh
  Workloads owner upgrade transaction passed; rejected 4 hostile fixtures
  RPM and bootc ordering/status contracts plus retired-unit cleanup present
```

These checks prove the shipped script/unit contracts and hostile refusal
fixtures, but do not claim an installed RPM/bootc first boot or physical-seat
acceptance.

## Unified onboard surface follow-up — 2026-08-17

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=1 \
  install-helpers/xcp-build.sh cargo test -p mackesd onboard \
  --features async-services -- --nocapture
```

- BigBoy `172.20.0.130`, warmed slot `1`.
- Result: `231 passed, 0 failed, 4792 filtered out`.
- The gate covered first-desktop, invite/join, mesh creation/DNS/network,
  role provisioning, self-test, service-add, lighthouse spawning, remote push,
  and onboard/service worker authorization, replay, retry, and recovery paths.
- This is comprehensive local/farm contract evidence; live provider/Bus,
  installed package/first-boot, and physical-seat acceptance remain open.

## Renderer/wizard projection follow-up — 2026-08-17

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=1 \
  install-helpers/xcp-build.sh cargo test -p mde-enroll --locked -- --nocapture
```

- BigBoy `172.20.0.130`, warmed slot `1`.
- Result: `34 passed, 0 failed` (library tests; binary and doc-test targets
  also completed with zero failures).
- The gate covers the shared renderer-neutral lifecycle projection’s target
  scope and generation binding, plus the `magic-setup` state machine and typed
  action argv contracts.
- This closes local S4 contract coverage only; installed/live acceptance and
  physical-seat proof remain outside this evidence.

## Canonical renderer plan follow-up — 2026-08-17

- Source revision: `6b23a8488631329bfd355d5909a34fa07b3214b9`
- Farm host/slot: `172.20.0.196` / `1`
- Command:

  ```text
  MCNF_BUILD_HOST=172.20.0.196 MCNF_BUILD_SLOT=1 \
    install-helpers/xcp-build.sh cargo test -p mde-enroll lifecycle_controller \
    --locked -- --nocapture
  ```

- Result: `1 passed, 0 failed, 35 filtered out` after a cold 3m51s build.
- `LifecycleController` now derives each GUI/TUI plan from the bounded
  `LifecycleIntentV1::default_steps()` contract. This removes its untyped,
  invalid `cordon`/`drain`/`erase` vocabulary and proves both the Offboard and
  ResetAndOnboard projections validate as `LifecyclePlanV1` values. This does
  not claim lifecycle executor, package, provider, or live-seat completion.

## Unattended lifecycle CLI boundary — 2026-08-17

- Source revision: `afa79aade10b56cef0447e5652dd25c22950aa8a`
- Farm host/slot: `172.20.0.196` / `1`
- Command:

  ```text
  MCNF_BUILD_HOST=172.20.0.196 MCNF_BUILD_SLOT=1 \
    install-helpers/xcp-build.sh cargo test -p mackesd --bin mackesd \
    lifecycle_cli_boundary --locked -- --nocapture
  ```

- Result: `2 passed, 0 failed, 71 filtered out`.
- The public lifecycle commands now derive all steps from
  `LifecycleIntentV1::default_steps()`. They reject caller-provided `--step`
  values and no longer expose `lifecycle-complete`, so a user or renderer cannot
  claim a side effect completed without an authority-owned executor action.
  This is a safety boundary only; it does not claim the remaining turnkey
  provider, package, fleet, or live-seat execution work is complete.

## Completed offboarding receipt erasure boundary — 2026-08-17

- Source revision: `e2bb66b5e193469face9b340fe04a4b2b85a8663`
- Contract gate:

  ```text
  MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=1 \
    install-helpers/xcp-build.sh cargo test -p mackes-mesh-types lifecycle \
    --locked -- --nocapture
  ```

  Result: `21 passed, 0 failed, 519 filtered out`.

- Authority gate:

  ```text
  MCNF_BUILD_HOST=172.20.0.196 MCNF_BUILD_SLOT=1 \
    install-helpers/xcp-build.sh cargo test -p mackesd --lib \
    lifecycle_authority --locked -- --nocapture
  ```

  Result: `17 passed, 0 failed, 5006 filtered out` after a cold 3m35s build.

- A valid completed `OffboardingReceiptV1` now requires an empty
  `retained_resources` set, and the authority provides no caller-controlled
  retention argument. A receipt can therefore no longer represent a completed
  erase while preserving a reusable identity or local resource. This contract
  proof does not replace the S14 real drain/revoke/erase executor or its
  installed-seat evidence.
