# WL-FUNC-023 leftover — bootstrap SSH identity file provision — r1

Date: 2026-08-22  
Classification: leftover-honesty / dest-file provision; **not** live enroll,
offboard, bearer mint, or freeze-bar closure  
Worklist unit: `qu0014bs`  
Source revision: `37fd8fef4`  
Control host: `rocky9-kvm2`  
`production_admitted: false`  
`enroll_succeeded: false`

This unit provisioned regular dest files for `SshBootstrap` identity
resolution. It does not claim that a live enroll succeeded.

## Authority

- Worklist: `docs/platform/WORKLIST.md` `WL-FUNC-023`.
- Operator lock 2026-08-22: final freeze waits on a real-seat
  enroll/offboard over SSH under red `AI-GENERATED-ALERT` + 5s.
- This unit was authorized only for leftover (2): copy a source identity
  and a source known-hosts file onto dest regular files. No seat
  mutation. No live SSH to Dell / Seat 15 / Surface. No mint of a
  production bearer. Login env was not mutated.

## Helper

`install-helpers/provision-bootstrap-ssh-identity.py` copies two source
regular files onto dest regular files.

- No-replace. Source or dest symlink refused. Empty files refused.
- Dest inside the helper git worktree (or farm-synced workspace root)
  refused.
- Dest key mode `0600` (OpenSSH `ssh-keygen -y` accepts it). Dest
  known-hosts and sidecar mode `0400`.
- Sidecar kind `mcnf-bootstrap-ssh-identity`. Sidecar names dest paths,
  modes, sizes, and sha256 of dest files only. Sidecar must land outside
  Git.
- Stdout is that sidecar JSON. Key bytes are never printed.
- The helper never sets `MACKESD_BOOTSTRAP_SSH_KEY` or
  `MACKESD_BOOTSTRAP_KNOWN_HOSTS` and never claims enroll succeeded.

Hostile tests in
`install-helpers/test-provision-bootstrap-ssh-identity.py`: symlink
refuse; dest-exists refuse; empty source refuse; dest-inside-worktree
refuse; happy path writes dest + sidecar outside the repo. No network.
No live SSH.

## Local result

```text
python3 install-helpers/test-provision-bootstrap-ssh-identity.py
PASS
```

Control-host login env after the suite: both bootstrap env vars unset.

## Farm result (light slot only)

Host `172.20.0.196` slot `0` (`MCNF_BUILD_SHAPE=small`). Not BigBoy.
Admission: `9517808 KiB` free (required `8388608 KiB`). Sync:

```text
MCNF_BUILD_HOST=172.20.0.196 MCNF_BUILD_SLOT=0 MCNF_BUILD_SHAPE=small \
  install-helpers/xcp-build.sh sync
```

Remote workspace `~/magic-mesh-farm-0` (rsync excludes `.git`). Test:

```text
python3 install-helpers/test-provision-bootstrap-ssh-identity.py
PASS
```

Farm VMs did not receive operator identity files. Tests used temp dirs
only.

## Control-host dest provision (paths and modes only)

Preconditions: `/root/mcnf-private` is a real directory;
`/root/.ssh/mackes_mesh_ed25519` is a regular file mode `0600`;
`/root/.ssh/known_hosts` is a regular file and `ssh-keygen -F 172.20.0.15`
returned three host-key lines (comments dropped). The filtered
known-hosts source was a temp regular file under `/root/mcnf-private`
and was removed after the copy.

Dest files written (not in Git; not printed):

| path | type | mode | nlink | bytes |
|---|---|---|---|---|
| `/root/mcnf-private/bootstrap-ssh-key` | regular file | `0600` | 1 | 419 |
| `/root/mcnf-private/bootstrap-known-hosts` | regular file | `0400` | 1 | 831 |
| `/root/mcnf-private/bootstrap-ssh-identity.json` | regular file | `0400` | 1 | 509 |

Sidecar kind `mcnf-bootstrap-ssh-identity`. Sidecar sha256 values stay
in that private file. Evidence does not copy them.

Login env after provision:

```text
MACKESD_BOOTSTRAP_SSH_KEY unset
MACKESD_BOOTSTRAP_KNOWN_HOSTS unset
```

A later enroll worker may point those env vars (or systemd credentials
`bootstrap-ssh-key` / `bootstrap-known-hosts`) at the dest regular files
above. This unit did not export them.

## Non-claims

- Live SSH enroll did **not** succeed. It was not attempted.
- Offboard, reenroll, wipe, join, and package install were not attempted.
- A production enroll bearer was not minted.
- `SshBootstrap` remains `NotWired` on the login until the env vars or
  systemd credentials are bound to the dest files.
- Seat 15 (`172.20.0.15` / `Basement-Test-Workstation`) is already
  enrolled; first-enroll of that IP is not the leftover.
- Freeze bar is **not** closed.

## Leftover freeze bar

1. Mint a real 43-character enroll bearer through the existing lifecycle
   authority. `{{JOIN_TOKEN}}` is not a bearer.
2. Bind `MACKESD_BOOTSTRAP_SSH_KEY` and `MACKESD_BOOTSTRAP_KNOWN_HOSTS`
   (or the named systemd credentials) to singly-used regular dest files.
   The dest files now exist on this control host; the login env is still
   unset.
3. Live enroll a not-yet-enrolled authorized seat, **or**
   offboard/reenroll an already-enrolled authorized seat, under the red
   `AI-GENERATED-ALERT` and the five-second delay.

## Blocker

`WL-FUNC-023` stays `Remaining`. The freeze bar stays open until an
authorized worker mints a real bearer, binds the dest identity files,
and completes live enroll or authorized offboard/reenroll under red
alert + 5s. Provisioned dest files without env/credential binding, and
without the live act, are not a substitute.
