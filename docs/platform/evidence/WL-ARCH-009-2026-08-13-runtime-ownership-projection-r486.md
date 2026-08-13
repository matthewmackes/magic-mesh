# WL-ARCH-009 — runtime ownership projection fails closed (r486)

Date: 2026-08-13

## Result

The canonical `WorkerSpec` registry already assigns each worker to one of the
six supervised process groups, but its neutral runtime-contract projection
previously admitted a row whose state, health, or action owner named a different
group. That allowed future registry drift to publish a valid-looking contract
which crossed the process namespace boundary even though the launch owner was
unchanged.

`runtime_ownership` now rejects each cross-group relationship before returning
an admitted `WorkerContract`. The existing hostile-row test covers state,
health, and action ownership independently, proving that none can be delegated
away from the worker's canonical process group. Cleanup bounds remain checked
after the ownership relationship is established.

## Farm verification

Farm topology before the gate wave reported 5/5 nodes up and 0/10 heavy slots
active.

BigBoy `.130`, slot `arch009-owner-projection-test-r486`:

```text
MCNF_BUILD_HOST=172.20.0.130 \
MCNF_BUILD_SLOT=arch009-owner-projection-test-r486 \
install-helpers/xcp-build.sh cargo test -p mackesd --locked --lib \
  neutral_worker_contract_projection_rejects_incomplete_or_hostile_rows \
  -- --nocapture

test worker_role::tests::neutral_worker_contract_projection_rejects_incomplete_or_hostile_rows ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4925 filtered out
```

Build VM `.90`, slot `arch009-owner-projection-clippy-r486`:

```text
MCNF_BUILD_HOST=172.20.0.90 \
MCNF_BUILD_SLOT=arch009-owner-projection-clippy-r486 \
install-helpers/xcp-build.sh cargo clippy -p mackesd --locked --lib -- -D warnings

Finished `dev` profile [unoptimized + debuginfo] target(s) in 3m 28s
```

The package-wide formatter gate on `.50`, slot
`arch009-owner-projection-fmt-r486`, was not green because it reported existing
format drift in unrelated mackesd files and older registry rows outside this
slice. A file-scoped check identified only those older registry rows; the new
ownership projection and hostile assertions produced no formatter diff. No
unrelated formatting was changed.

## Remaining boundary

This closes cross-group ownership drift in the runtime-contract projection. It
does not claim completion of the wider epic: remaining acceptance still includes
the full Workers UI cutover/removal of duplicate surfaces and post-release
fleet, package, process, and live-seat proof for the six isolated services.
