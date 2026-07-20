# OpenStack create→verify→delete live test (WL-TEST-001)

The offline OpenStack contract tests
(`crates/mesh/mackesd/src/workers/openstack/client/contract.rs`) pin the client's
request shapes and response parsers against canonical fixtures — they never touch
a cloud. This runbook covers the one test that *does*: a live-gated
**create → verify → delete** round-trip that proves the resource-*mutating* client
path (auth → `POST /stacks` → poll → `DELETE`) against a real OpenStack endpoint.

- **Test:** `openstack::client::contract::live_openstack_create_verify_delete`
- **Lane:** `install-helpers/openstack-live-test.sh`
- **Read-only companion:** `live_openstack_catalog_and_resources` (authenticate,
  catalog, per-service health, list Nova servers — see the same module).

## Status: GATED (stays `#[ignore]`)

Live execution needs a **farm OpenStack endpoint** plus a **throwaway-project
quota** that does not yet exist. Until that endpoint lands, the test carries
`#[ignore]` so it never runs in the normal suite, and the lane script **refuses to
run** until the endpoint env is set. Both are wired now so the day the endpoint
exists it is a one-command run — no code change.

## What it creates and deletes

A single Heat stack named `mde-livetest-<nanos>` whose only resource is one
`OS::Heat::RandomString`. That resource is generated **entirely inside Heat** — it
allocates no Nova server, no Neutron port, no Cinder volume — so the round-trip
exercises the create/poll/delete path without standing up (or risking a leak of)
real cloud infrastructure. Even in the worst case, a leaked RandomString stack
costs nothing.

The test:

1. authenticates via the target `clouds.yaml`;
2. `heat_create`s the stack and **arms cleanup immediately** (before any
   assertion);
3. polls `heat_show` until `CREATE_COMPLETE` (failing fast on `CREATE_FAILED` or a
   ~180s timeout) — this is the "it exists / reached a ready state" assertion;
4. `heat_delete`s the stack and best-effort confirms it is gone.

## The cleanup guarantee (load-bearing)

A leaked live cloud resource is the failure mode this test refuses to allow, so
cleanup is guaranteed **even when a mid-test assertion panics**:

- A `StackTeardown` Drop-guard is armed the instant the stack is created.
- `cargo test` unwinds on an assertion failure (panic = unwind, not abort), so the
  guard's `Drop` runs on that unwind path and still issues the `DELETE`.
- The happy path calls an explicit `teardown()` that issues the `DELETE`, asserts
  it, and **disarms** the guard — so cleanup runs *exactly once* on every exit
  path (success or panic).
- If the panic-path delete itself fails, the guard logs
  `[cleanup-on-panic] FAILED … MANUAL CLEANUP REQUIRED` with the stack name+id.

## Pointing it at an endpoint

Two environment variables are required — the lane refuses to run without both:

| Variable                    | Meaning |
|-----------------------------|---------|
| `MDE_OPENSTACK_LIVE_TARGET` | Path to the `clouds.yaml` to authenticate with. Exported to the test as the openstacksdk-standard `OS_CLIENT_CONFIG_FILE`. **Point it at a throwaway project.** |
| `MDE_OPENSTACK_LIVE_MUTATE` | Must be exactly `1`. The explicit opt-in that this run may create+delete real resources. Without it the test SKIPs even when the target is set — so a stray `--ignored` run can never mutate a cloud. |

Optional: `OS_CLOUD` selects the context when the `clouds.yaml` holds more than
one; `MCNF_BUILD_HOST` picks the farm build node for `xcp-build.sh`.

### Run it (farm lane)

```sh
MDE_OPENSTACK_LIVE_TARGET=/etc/openstack/clouds.yaml \
MDE_OPENSTACK_LIVE_MUTATE=1 \
  install-helpers/openstack-live-test.sh
```

### Run it directly

```sh
MDE_OPENSTACK_LIVE_TARGET=/etc/openstack/clouds.yaml MDE_OPENSTACK_LIVE_MUTATE=1 \
  ./install-helpers/xcp-build.sh cargo test -p mackesd --lib \
  openstack::client::contract::live_openstack_create_verify_delete -- \
  --ignored --nocapture --test-threads=1
```

The build is farm-only (local `cargo` is a disabled shim). `--ignored` promotes
the test; `--test-threads=1` keeps the live round-trip serial and its
`--nocapture` log readable.
