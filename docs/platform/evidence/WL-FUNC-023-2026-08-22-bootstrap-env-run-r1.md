# WL-FUNC-023 leftover — bootstrap SSH dest env child-only runner — r1

Date: 2026-08-22  
Classification: leftover-honesty / child-only dest-env bind; **not** live enroll,
offboard, bearer mint, login-env mutation, or freeze-bar closure  
Worklist unit: `qu0024be`  
Source revision: `cbf6a8275`  
Control host: `rocky9-kvm2`  
`production_admitted: false`  
`enroll_succeeded: false`

This unit added a helper that sources already-bound dest identity paths
for a child enroll worker process only. It does not claim that a live
enroll succeeded.

## Authority

- Worklist: `docs/platform/WORKLIST.md` `WL-FUNC-023`.
- Operator lock 2026-08-22: final freeze waits on a real-seat
  enroll/offboard over SSH under red `AI-GENERATED-ALERT` + 5s.
- This unit was authorized only for leftover (2): source/bind the dest
  env file for a live enroll *worker process* without exporting those
  vars into the login environment. No seat mutation. No live SSH to
  Dell / Seat 15 / Surface. No mint of a production bearer. Login env
  was not mutated.

## Helper

`install-helpers/run-with-bootstrap-ssh-env.py` admits one dest env file
(default `/root/mcnf-private/bootstrap-ssh.env`) as a singly-used regular
file, mode `0400`, not a symlink, not inside the helper git worktree.
The body must be exactly two ASCII assignments:

```text
MACKESD_BOOTSTRAP_SSH_KEY=<dest-key>
MACKESD_BOOTSTRAP_KNOWN_HOSTS=<dest-known-hosts>
```

It then admits both dest paths as singly-used regular files (dest key
mode `0600`, dest known-hosts mode `0400`, distinct, safe charset) and
runs `--` command argv as a child with a copied environment plus those
two vars. The helper process `os.environ` is never assigned those names.
`{{JOIN_TOKEN}}` in the env file or dest paths is refused.

Optional `--print-sidecar PATH` writes a no-replace mode-`0400` JSON
sidecar of kind `mcnf-bootstrap-ssh-env-run` with dest-file sha256
hashes and command argv (not env values). `enroll_succeeded` and
`production_admitted` stay false. Sidecar already exists: refuse.
Key bytes and the env-file body are never printed.

Hostile tests in
`install-helpers/test-run-with-bootstrap-ssh-env.py`: symlink env file;
extra/missing keys; dest missing or symlink; env file inside the git
worktree; `{{JOIN_TOKEN}}` refuse; helper process environ stays unset
after a successful child run; child sees both vars at the fixture
paths; no `BEGIN OPENSSH` / key markers printed; sidecar no-replace.
Tests use temp dirs only and never touch `/root/mcnf-private/`. No
network. No live SSH. No bearer mint.

## Local result

```text
python3 install-helpers/test-run-with-bootstrap-ssh-env.py
PASS
```

Control-host login env after the suite: both bootstrap env vars unset.

## Farm result (light slot only)

Host `172.20.0.50` slot `0` (`MCNF_BUILD_SHAPE=small`). Not BigBoy.
Admission: `71287692 KiB` free (required `8388608 KiB`). Sync:

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=0 MCNF_BUILD_SHAPE=small \
  install-helpers/xcp-build.sh sync
```

Remote workspace `~/magic-mesh-farm-0` (rsync excludes `.git`). Test:

```text
python3 install-helpers/test-run-with-bootstrap-ssh-env.py
PASS
```

Farm VMs did not receive operator identity files. Tests used temp dirs
only. Remote process env stayed unset.

## Control-host dests (paths and modes only; unread)

This unit did not replace or read production dest bytes. Preconditions
observed after the suite (2026-08-22T23:27:27Z on `rocky9-kvm2`):

| path | type | mode | bytes |
|---|---|---|---|
| `/root/mcnf-private/bootstrap-ssh-key` | regular file | `0600` | 419 |
| `/root/mcnf-private/bootstrap-known-hosts` | regular file | `0400` | 831 |
| `/root/mcnf-private/bootstrap-ssh.env` | regular file | `0400` | 134 |

Login env after the runner suite:

```text
MACKESD_BOOTSTRAP_SSH_KEY unset
MACKESD_BOOTSTRAP_KNOWN_HOSTS unset
```

This unit did not `export` those vars and did not source the env file
into this process. A later enroll worker may invoke
`install-helpers/run-with-bootstrap-ssh-env.py --env-file /root/mcnf-private/bootstrap-ssh.env -- <worker>`
so only that child sees the dest paths.

## Non-claims

- Live SSH enroll did **not** succeed. It was not attempted.
- Offboard, reenroll, wipe, join, and package install were not attempted.
- A production enroll bearer was not minted.
- Login env remains unset. `SshBootstrap` remains unwired on the login
  until a later worker uses the child-only runner for the live act.
- Seat 15 (`172.20.0.15` / `Basement-Test-Workstation`) is already
  enrolled; first-enroll of that IP is not the leftover.
- No unpublished signed candidate exists.
- Freeze bar is **not** closed.

## Leftover freeze bar

1. Mint a real 43-character enroll bearer through the existing lifecycle
   authority. `{{JOIN_TOKEN}}` is not a bearer.
2. Child-only runner `install-helpers/run-with-bootstrap-ssh-env.py` now
   sources dests for a worker process only. Login env remains unset.
3. Live enroll a not-yet-enrolled authorized seat, **or**
   offboard/reenroll an already-enrolled authorized seat, under the red
   `AI-GENERATED-ALERT` and the five-second delay.

## Blocker

`WL-FUNC-023` stays `Remaining`. The freeze bar stays open until an
authorized worker mints a real bearer and completes live enroll or
authorized offboard/reenroll under red alert + 5s. A child-only runner
without the live act is not a substitute.
