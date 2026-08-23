# WL-FUNC-023 leftover (2) — live child-only dest-env run — r1

Date: 2026-08-23  
Classification: leftover (2) live dest-env child proof; **not** leftover (3)
enroll/offboard, freeze, or `production_admitted`  
Worklist unit: operator drain authorization  
Source revision: `de89cb277f50` plus this record  
Control host: `rocky9-kvm2`  
`production_admitted: false`  
`enroll_succeeded: false`

## Authority

- Worklist: `docs/platform/WORKLIST.md` `WL-FUNC-023` leftover (2).
- Operator 2026-08-23: overcome blockers and drain the worklist.

## Act

`install-helpers/run-with-bootstrap-ssh-env.py` sourced
`/root/mcnf-private/bootstrap-ssh.env` for one child only. The child was
`/usr/bin/python3` writing booleans to `/tmp/mcnf-leftover2-child-status.txt`.
No join, enroll, offboard, or mint ran.

Observed:

```text
parent_before_KEY=false parent_before_HOSTS=false parent_before_JOIN=false
CHILD_RC=0
key_set=True hosts_set=True join_set=False key_is_dest=True
parent_after_KEY=false parent_after_HOSTS=false parent_after_JOIN=false
```

Sidecar `/root/mcnf-private/bootstrap-ssh-env-live-r2.json` mode `0400`,
kind `mcnf-bootstrap-ssh-env-run`, `production_admitted: false`,
`enroll_succeeded: false`. Command argv did not carry dest-path assignment
values. Login env remains unset.

Existing bootstrap dests and the leftover-(1) enroll-bearer dest were not
replaced.

## Non-claims

- Live enroll, join, offboard, reenroll, wipe, and package install were
  not attempted.
- `production_admitted` was not flipped.
- The helper self-test still contains a fixture that expects
  `unpublished signed candidate is absent` after a fixture dest override;
  this live run used the dest-backed runner, not that fixture.

## Blocker

`WL-FUNC-023` stays `Remaining`. Leftover (2) dest-env child proof exists.
Freeze bar is leftover (3): live enroll or authorized offboard+reenroll
under red `AI-GENERATED-ALERT` + 5s. Seat 15 remains enrolled. LH1
`enroll :4243` is reachable from Seat 15 and Dell.
