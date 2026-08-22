# WL-FUNC-023 leftover — bootstrap SSH dest identity env-file bind — r1

Date: 2026-08-22  
Classification: leftover-honesty / dest env-file bind; **not** live enroll,
offboard, bearer mint, login-env mutation, or freeze-bar closure  
Worklist unit: `qu0018be`  
Source revision: `8ae25f0bd`  
Control host: `rocky9-kvm2`  
`production_admitted: false`  
`enroll_succeeded: false`

This unit bound already-provisioned dest identity files as a sourceable
env file. It does not claim that a live enroll succeeded.

## Authority

- Worklist: `docs/platform/WORKLIST.md` `WL-FUNC-023`.
- Operator lock 2026-08-22: final freeze waits on a real-seat
  enroll/offboard over SSH under red `AI-GENERATED-ALERT` + 5s.
- This unit was authorized only for leftover (2): write a no-replace env
  file whose body is the two dest-path assignments. No seat mutation.
  No live SSH to Dell / Seat 15 / Surface. No mint of a production
  bearer. Login env was not mutated.

## Helper

`install-helpers/bind-bootstrap-ssh-env.py` admits dest identity files
as singly-used regular files (missing, empty, or symlink refused) and
writes a no-replace env file:

```text
MACKESD_BOOTSTRAP_SSH_KEY=<dest-key>
MACKESD_BOOTSTRAP_KNOWN_HOSTS=<dest-known-hosts>
```

- Default dest parent `/root/mcnf-private/`. Default env file
  `bootstrap-ssh.env`, mode `0400`.
- Dest env file already exists: refuse. Dest env file inside the helper
  git worktree (or farm-synced workspace root): refuse.
- Sidecar kind `mcnf-bootstrap-ssh-env`. Sidecar names path, mode, size,
  and sha256 of the env file only. Sidecar must land outside Git.
- Stdout is that sidecar JSON. Key bytes are never printed and dest
  identity files are never read.
- The helper never exports `MACKESD_BOOTSTRAP_SSH_KEY` or
  `MACKESD_BOOTSTRAP_KNOWN_HOSTS` and never claims enroll succeeded.

Hostile tests in
`install-helpers/test-bind-bootstrap-ssh-env.py`: dest-key / dest
known-hosts symlink refuse; dest-env-exists refuse; dest-inside-worktree
refuse; happy path writes env file + sidecar outside the repo; env file
contains only the two path assignments; process env stays unset. No
network. No live SSH.

## Local result

```text
python3 install-helpers/test-bind-bootstrap-ssh-env.py
PASS
```

Control-host login env after the suite: both bootstrap env vars unset.

## Farm result (light slot only)

Host `172.20.0.196` slot `0` (`MCNF_BUILD_SHAPE=small`). Not BigBoy.
Admission: `9516064 KiB` free (required `8388608 KiB`). Sync:

```text
MCNF_BUILD_HOST=172.20.0.196 MCNF_BUILD_SLOT=0 MCNF_BUILD_SHAPE=small \
  install-helpers/xcp-build.sh sync
```

Remote workspace `~/magic-mesh-farm-0` (rsync excludes `.git`). Test:

```text
python3 install-helpers/test-bind-bootstrap-ssh-env.py
PASS
```

Farm VMs did not receive operator identity files. Tests used temp dirs
only. Remote process env stayed unset.

## Control-host env bind (paths and modes only)

Preconditions: `/root/mcnf-private/bootstrap-ssh-key` is a regular file
mode `0600` nlink `1`; `/root/mcnf-private/bootstrap-known-hosts` is a
regular file mode `0400` nlink `1`. The helper did not replace those
files and did not read their bytes.

Dest files written (not in Git; not printed):

| path | type | mode | nlink | bytes |
|---|---|---|---|---|
| `/root/mcnf-private/bootstrap-ssh.env` | regular file | `0400` | 1 | 134 |
| `/root/mcnf-private/bootstrap-ssh-env.json` | regular file | `0400` | 1 | 326 |

Env file body is exactly the two dest-path assignments. Sidecar kind
`mcnf-bootstrap-ssh-env`. Sidecar sha256 of the env file stays in that
private file. Evidence does not copy dest identity bytes.

Login env after bind (2026-08-22T23:02:27Z on `rocky9-kvm2`):

```text
MACKESD_BOOTSTRAP_SSH_KEY unset
MACKESD_BOOTSTRAP_KNOWN_HOSTS unset
```

This unit did not `export` those vars and did not source the env file
into this process. A later enroll worker may source
`/root/mcnf-private/bootstrap-ssh.env` (or bind the named systemd
credentials) for the live act.

## Non-claims

- Live SSH enroll did **not** succeed. It was not attempted.
- Offboard, reenroll, wipe, join, and package install were not attempted.
- A production enroll bearer was not minted.
- Login env remains unset. `SshBootstrap` remains `NotWired` on the
  login until a later worker sources or otherwise binds those vars.
- Seat 15 (`172.20.0.15` / `Basement-Test-Workstation`) is already
  enrolled; first-enroll of that IP is not the leftover.
- Freeze bar is **not** closed.

## Leftover freeze bar

1. Mint a real 43-character enroll bearer through the existing lifecycle
   authority. `{{JOIN_TOKEN}}` is not a bearer.
2. Source or otherwise bind `MACKESD_BOOTSTRAP_SSH_KEY` and
   `MACKESD_BOOTSTRAP_KNOWN_HOSTS` for the live enroll worker. The dest
   identity files and the sourceable env file now exist on this control
   host; the login env is still unset.
3. Live enroll a not-yet-enrolled authorized seat, **or**
   offboard/reenroll an already-enrolled authorized seat, under the red
   `AI-GENERATED-ALERT` and the five-second delay.

## Blocker

`WL-FUNC-023` stays `Remaining`. The freeze bar stays open until an
authorized worker mints a real bearer, sources/binds the dest identity
paths for the live enroll worker, and completes live enroll or
authorized offboard/reenroll under red alert + 5s. An env file without
login/worker bind, and without the live act, is not a substitute.
