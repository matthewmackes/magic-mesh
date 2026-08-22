# WL-FUNC-023 leftover honesty — live SSH enroll/offboard prereq — r1

Date: 2026-08-22  
Classification: leftover-honesty / prerequisite record; **not** live enroll,
offboard, or freeze-bar closure  
Worklist unit: `qu0008ev`  
Source revision: `57db746db`  
Control host: `rocky9-kvm2`  
`production_admitted: false`

This record reconfirms the operator 2026-08-22 freeze bar and names the
remaining live act. It does not claim that a live enroll succeeded.

## Authority

- Worklist: `docs/platform/WORKLIST.md` `WL-FUNC-023`.
- Operator lock 2026-08-22 (`operator-survey-2026-08-22-block-lift.md`):
  final freeze waits on a real-seat enroll/offboard over SSH. Seats may be
  mutated only with the red `AI-GENERATED-ALERT` and a five-second delay
  when an unpublished candidate exists.
- This unit was **not** authorized to start that mutation. No enroll,
  offboard, wipe, join, package install, or `MACKESD_BOOTSTRAP_SSH_KEY`
  provision ran here.

## Reconfirm — control host (no mutation)

`MACKESD_BOOTSTRAP_SSH_KEY` is unset on the control host:

```text
printf 'MACKESD_BOOTSTRAP_SSH_KEY_set='; \
  if [ -z "${MACKESD_BOOTSTRAP_SSH_KEY+x}" ]; then echo no; else echo yes; fi
env | awk -F= '$1=="MACKESD_BOOTSTRAP_SSH_KEY"{print; found=1}
  END{if(!found) print "env: MACKESD_BOOTSTRAP_SSH_KEY absent"}'
```

Observed 2026-08-22T22:09:17Z on `rocky9-kvm2`:

```text
MACKESD_BOOTSTRAP_SSH_KEY_set=no
env: MACKESD_BOOTSTRAP_SSH_KEY absent
```

`SshBootstrap` is designed to return typed `RemotePushError::NotWired`
when that key is missing (`resolve_bootstrap_identity` in
`crates/mesh/mackesd/src/onboard/remote_push.rs`). The transport refuses
`{{JOIN_TOKEN}}` as a bearer (`validate_bootstrap_bearer`) and refuses an
already-enrolled peer (`Target::Enrolled`). This unit did not invoke
`SshBootstrap` and did not set the key.

## Reconfirm — Seat 15 hostname only (no mutation)

Seat 15 already answers as a named enrolled workstation. Control key
`/root/.ssh/mackes_mesh_ed25519` (mode `0600`) ran hostname only:

```text
ssh -i /root/.ssh/mackes_mesh_ed25519 -o BatchMode=yes \
  -o IdentitiesOnly=yes -o ConnectTimeout=10 \
  -o StrictHostKeyChecking=yes mm@172.20.0.15 hostname
```

Observed:

```text
Basement-Test-Workstation
```

No other remote command ran. First-enroll of `172.20.0.15` is therefore
not the remaining freeze-bar act unless the operator later chooses
authorized offboard plus reenroll of that already-named seat.

## Leftover freeze bar

The leftover is live SSH enroll/offboard after the following prerequisites,
not a farm-contract or `{{JOIN_TOKEN}}` refuse gap:

1. Mint a real enroll bearer through the existing lifecycle authority.
   The command-template placeholder `{{JOIN_TOKEN}}` is not a bearer.
2. Provision `MACKESD_BOOTSTRAP_SSH_KEY` (named credential or explicit
   regular identity file). This unit did not set that variable.
3. Then live enroll a not-yet-enrolled authorized seat, **or**
   offboard/reenroll an already-enrolled authorized seat, under the red
   `AI-GENERATED-ALERT` and the five-second delay.

Seat 15 (`172.20.0.15` / `Basement-Test-Workstation`) is already enrolled.
An unpublished signed candidate plus operator mutation authorization remain
required before any of those live acts.

## Non-claims

- Live SSH enroll did **not** succeed. It was not attempted.
- Offboard, reenroll, wipe, join, and package install were not attempted.
- Farm fixture / `join_argv` refuse of `{{JOIN_TOKEN}}` is prior work; it
  does not close this leftover.
- `WL-TEST-002` installed-seat acceptance is not claimed here.

## Blocker

`WL-FUNC-023` stays `Remaining`. The freeze bar stays open until an
authorized worker mints a real bearer, provisions the bootstrap key, and
completes live enroll or authorized offboard/reenroll under red alert + 5s.
Missing key, missing minted bearer, or mutation of Seat 15 as if it were a
fresh box are not substitutes.
