# Crown-jewel live etcd/Nebula harness (WL-TEST-002)

The offline crown-jewel suites
(`crates/mesh/mackesd/tests/substrate_etcd.rs`,
`crates/mesh/mackesd/tests/integration_testcontainers.rs`) prove the real-etcd
leader election and the real-Nebula overlay path against **disposable** podman
containers they spin up and tear down themselves. They never touch the fleet.

This runbook covers the other half of WL-TEST-002: the harness's **first named
target — the existing live lighthouses**. A single `#[ignore]` probe points the
production `mackesd_core` substrate client at a running etcd + a running
lighthouse and asserts the plane is actually alive.

- **Test:** `live_fleet_etcd_quorum_and_nebula_overlay`
  (`crates/mesh/mackesd/tests/live_fleet.rs`)
- **Lane:** `automation/testing/crown-jewel-live.sh`
- **Offline companions (unchanged):** `substrate_etcd.rs` (real-etcd election)
  and `integration_testcontainers.rs` (real-Nebula overlay) — the
  `--features docker-tests` disposable-container suites.

## Status: GATED (stays `#[ignore]`)

Live execution needs a **running fleet reachable over the overlay** from the run
host. Until an operator runs it against the live lighthouses, the test carries
`#[ignore]` so it never runs in the normal suite, and the lane script **refuses
to run** until the fleet coordinates are set in the environment. Both are wired
now so the day the operator points it at the fleet it is a one-command run — no
code change. **This session did not run it against live infra.**

## What it asserts

Against the running fleet, in order (each prints a `CROWN-JEWEL-LIVE:` line):

1. **etcd is reachable + quorate** — the production client `connect`s, a real
   `member_list` returns ≥1 member, and `status` reports a **non-zero Raft
   leader** (a converged cluster, not a split-brain / no-leader substrate).
2. **the Nebula overlay is live** — the replicated `/mesh/peers` directory is
   non-empty and ≥1 peer carries an **overlay IP**. A node can only publish
   itself there by heartbeating *over the overlay*, so this is overlay proof
   without needing the mesh CA.
3. **the lighthouse underlay is up** — a UDP probe to the lighthouse's Nebula
   port. On a **closed** port Linux returns ICMP port-unreachable
   (`ConnectionRefused`) → hard fail; a silent drop/timeout is the healthy case
   (Nebula ignores unauthenticated packets) and is recorded, not asserted.

## Fail-loud, never a false green (load-bearing)

This is the ONBOARD-6 audit-gap net: a fleet whose unit tests are green but whose
substrate is dead. Two properties make a green result trustworthy:

- **Armed ⇒ fail loud.** When `MCNF_LIVE_ETCD` / `MCNF_LIVE_LH` are set, an
  unreachable target **panics** with typed evidence — it never self-skips green.
  It `#[ignore]`-skips (green) only when the env is unset, i.e. in the normal
  suite.
- **The `xcp-build.sh` `remote()` false-green guard.** `remote()` runs
  `cargo …` over SSH with **no `SendEnv`/inline env**, so routing this test
  through `xcp-build.sh cargo` would **strip** `MCNF_LIVE_*` before the remote
  cargo process — the test would self-skip and cargo would exit 0: a green run
  that never touched the fleet (and a farm build VM is not on the overlay
  anyway). The lane therefore runs `cargo` **directly** (env in-process) and, as
  a backstop, greps the test's sentinel: if the log shows
  `CROWN-JEWEL-LIVE: SKIP` (env didn't arrive) or lacks `CROWN-JEWEL-LIVE: PASS`,
  the lane **fails loud** instead of reporting a false green.

## What the lane captures

Every run writes a timestamped forensic bundle to
`$MCNF_CJ_ARTIFACT_DIR/crown-jewel-live-<UTC>/`
(default `~/mcnf-crown-jewel-artifacts/…`):

| File | Contents |
|------|----------|
| `context.txt`               | Run context — stamp, host, repo root, targets. |
| `etcd-member-list.txt`      | `etcdctl member list -w table` (read-only). |
| `etcd-endpoint-health.txt`  | `etcdctl endpoint health` per endpoint. |
| `etcd-endpoint-status.txt`  | `etcdctl endpoint status -w table` (leader, db size, revision). |
| `mesh-peers-directory.txt`  | `etcdctl get --prefix /mesh/peers/` — the live Nebula membership view. |
| `nebula-iface.txt`          | `ip -4 addr show nebula1` — this host's overlay address, if it is a mesh member. |
| `test-output.txt`           | The full `--nocapture` probe log (the `CROWN-JEWEL-LIVE:` lines + any panic). |

Captures are best-effort: a missing `etcdctl` / `ip` is noted in-file and does
not abort the lane, so the test log is always produced.

## Pointing it at the fleet

Two environment variables are required — the lane refuses to run without both:

| Variable         | Meaning |
|------------------|---------|
| `MCNF_LIVE_ETCD` | Comma/space/newline list of etcd client endpoints on the overlay, e.g. `http://10.42.0.1:2379,http://10.42.0.2:2379`. Read-only. |
| `MCNF_LIVE_LH`   | Lighthouse Nebula underlay address `host[:port]` (port defaults to `4242`), e.g. `10.42.0.1:4242`. |

Optional: `MCNF_CJ_ARTIFACT_DIR` (artifact parent dir), `ETCDCTL` (etcdctl
binary for the captures), `MCNF_CJ_CARGO` (cargo invocation — **keep it local**;
do not point it at an SSH/xcp-build wrapper or the sentinel guard will reject the
resulting stripped-env false green).

**Run host:** a mesh member (lighthouse / seat / control node) with real `cargo`
and overlay reachability to the endpoints + lighthouse — **not** a farm build VM
(off-overlay, and `xcp-build.sh` would strip the env).

### Run it (lane)

```sh
MCNF_LIVE_ETCD=http://10.42.0.1:2379,http://10.42.0.2:2379 \
MCNF_LIVE_LH=10.42.0.1:4242 \
  automation/testing/crown-jewel-live.sh
```

### Run it directly

```sh
MCNF_LIVE_ETCD=http://10.42.0.1:2379 MCNF_LIVE_LH=10.42.0.1:4242 \
  cargo test -p mackesd --test live_fleet -- \
  --ignored --nocapture --test-threads=1
```

`--ignored` promotes the test; `--test-threads=1` keeps the live probe serial and
its `--nocapture` log readable. Run on a mesh host — the local `cargo` shim on a
farm build VM is a no-op, and its env would not be on the overlay.

## Beyond this slice

WL-TEST-002's full acceptance (spin disposable mesh nodes, run election →
overlay → enroll → recovery, tear down / revert snapshots, emit artifacts) is the
larger, farm-capacity + destructive-boundary gated build. This lane is the
live-lighthouse target of that harness plus its artifact-capture contract; the
disposable-node fixtures remain the `docker-tests` suites above.
