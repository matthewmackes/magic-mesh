# WL-FUNC-023 leftover — mint-enroll-bearer helper — r1

Date: 2026-08-22  
Classification: leftover-honesty / helper-only extract; **not** live mint,
enroll, offboard, SSH, login-env mutation, or freeze-bar closure  
Worklist unit: `qu0026mb`  
Source revision: `7af62ace5`  
Control host: `rocky9-kvm2`  
`production_admitted: false`  
`enroll_succeeded: false`

This unit added a helper that wraps a caller-supplied `mackesd enroll-token`
binary and writes a 43-character URL-safe bearer to a dest file. It does
not claim that a production bearer was minted.

## Authority

- Worklist: `docs/platform/WORKLIST.md` `WL-FUNC-023`.
- Operator lock 2026-08-22: leftover (1) is a real 43-char enroll bearer
  through existing lifecycle authority. This unit was authorized only for
  leftover (1) **helper glue**. No live mint. No SSH to Dell / Seat 15 /
  Surface. No enroll. Login env was not mutated.

## Helper

`install-helpers/mint-enroll-bearer.py` admits `--mackesd` as a regular
executable (symlink refused) and runs:

```text
mackesd enroll-token --mesh-id <id> [--lighthouse …] [--note …]
```

Optional `--workgroup-root` is passed only as `MDE_WORKGROUP_ROOT` (the
env `enroll-token` already honors). The helper does not invent a
`--workgroup-root` CLI flag or a second ledger.

Stdout from `mackesd` is captured and never printed. The join-token shape
`mesh:<id>@<host>:<port>#<bearer>` with optional `?fp=hex` is parsed.
The bearer is refused unless it is exactly 43 URL-safe characters
(`A-Za-z0-9_-`) and is not `{{JOIN_TOKEN}}`. Stdout that is not exactly
one token line, or that contains more than one token, is refused. A
non-zero `mackesd` exit is refused.

`--output` is a no-replace regular file, mode `0600`, dest outside the
helper git worktree, parent not group/other writable. Symlink or
existing dest is refused. Default dest parent may be `/root/mcnf-private`;
self-test never writes there.

Optional `--sidecar` writes a no-replace mode-`0400` JSON record of kind
`mcnf-enroll-bearer-mint`, `schema_version` 1, with
`production_admitted: false`, `enroll_succeeded: false`,
`bearer_sha256` (hash of dest bytes), dest path/mode/bytes, `mesh_id`,
and note length only. The sidecar never includes the bearer or join
token.

Hostile tests in
`install-helpers/test-mint-enroll-bearer.py`: successful extract + dest
mode `0600` + sidecar hash match + helper stdout has no bearer;
`{{JOIN_TOKEN}}` refuse; bearer length 42 / 44 refuse; extra stdout
garbage or two tokens refuse; dest already exists refuse; dest inside
the git worktree refuse; fake `mackesd` non-zero exit refuse; never
touch `/root/mcnf-private/`. Tests use a fake `mackesd` script in a
temp dir. No network. No live enroll-token against a real workgroup.

## Local result

```text
python3 install-helpers/test-mint-enroll-bearer.py
PASS
```

Control-host login env after the suite: both bootstrap env vars unset.

## Farm result (light slot only)

Host `172.20.0.90` slot `0` (`MCNF_BUILD_SHAPE=small`). Not BigBoy.
Peer FUNC-033 uses `.50` slot 0; this unit did not share that slot.
Admission: `102733464 KiB` free (required `8388608 KiB`). Sync:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=0 MCNF_BUILD_SHAPE=small \
  install-helpers/xcp-build.sh sync
```

Remote workspace `~/magic-mesh-farm-0` (rsync excludes `.git`). Test:

```text
python3 install-helpers/test-mint-enroll-bearer.py
PASS
```

Farm user `mm` cannot read `/root/mcnf-private` (`PermissionError`); the
suite treats that as "never touch" and writes only temp dirs. No live
`enroll-token` and no cargo.

## Control-host dests (paths and modes only; unread)

This unit did not replace, read, or write production dest bytes. It did
not invoke Seat 15 `mackesd`. Preconditions observed after the suite
(2026-08-22T23:32:22Z on `rocky9-kvm2`): login env

```text
MACKESD_BOOTSTRAP_SSH_KEY unset
MACKESD_BOOTSTRAP_KNOWN_HOSTS unset
```

`/root/mcnf-private` listing was unchanged by the self-test. No
production enroll-bearer dest was written.

## Non-claims

- A production enroll bearer was **not** minted. Live `mackesd enroll-token`
  against a real workgroup was not attempted.
- Live SSH enroll did not succeed. It was not attempted.
- Offboard, reenroll, wipe, join, and package install were not attempted.
- Login env remains unset.
- Seat 15 (`172.20.0.15` / `Basement-Test-Workstation`) already has
  `mackesd`; this unit did not invoke it. First-enroll of that IP is
  not the leftover.
- No unpublished signed candidate exists.
- Freeze bar is **not** closed.

## Leftover freeze bar

1. Mint a real 43-character enroll bearer through the existing live
   lifecycle authority. The helper exists; leftover (1) is still that
   production mint. `{{JOIN_TOKEN}}` is not a bearer.
2. Child-only runner `install-helpers/run-with-bootstrap-ssh-env.py`
   sources dests for a worker process only. Login env remains unset.
3. Live enroll a not-yet-enrolled authorized seat, **or**
   offboard/reenroll an already-enrolled authorized seat, under the red
   `AI-GENERATED-ALERT` and the five-second delay.

## Blocker

`WL-FUNC-023` stays `Remaining`. The freeze bar stays open until an
authorized worker mints a real bearer through live lifecycle authority
and completes live enroll or authorized offboard/reenroll under red
alert + 5s. A helper without the live mint is not a substitute.
