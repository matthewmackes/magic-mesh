# WL-REL-001 S1 input-generation candidate — r1

Date: 2026-08-22  
Classification: input-generation candidate record; **not** final freeze,
preflight admission, dest bind, RPM sign, or live enroll  
Worklist: `WL-REL-001` S1 (candidate phase)  
Control host: `rocky9-kvm2`  
`final_freeze: false`  
`production_admitted: false`

Operator 2026-08-22 (~21:47 UTC-4) authorized the next honest step after
dest-create was refused. This record names one clean pushed SHA as the
input-generation candidate. It does not close S1 freeze, admit REL-006
inputs, or authorize Seat mutation.

## Authority

- Worklist two-phase S1: record HEAD / upstream HEAD / epoch / Fedora /
  version as the input candidate; final freeze waits on live FUNC-023
  enroll, then REL-006 admission and reconfirmation of the same SHA.
- Operator survey 2026-08-22: PR #71 Ready names this branch HEAD the
  input-generation candidate. Ready is not freeze.
- Newer lock: operator authorized moving to this next step. That lock
  does not authorize a fabricated dest, unsigned 12.1.x RPMs, or signing
  with the control-host key `E6C820DAFBD1B07A`.

## Observed identity (this host, 2026-08-22)

```text
install-helpers/source-revision-receipt.sh --repo .
# 2872293b1393fdb6d645170cea30fc7d1682569d	1787447942

git rev-parse HEAD
# 2872293b1393fdb6d645170cea30fc7d1682569d

git rev-parse '@{u}'
# 2872293b1393fdb6d645170cea30fc7d1682569d

git diff --quiet && git diff --cached --quiet && echo worktree_and_index_clean
# worktree_and_index_clean

git log -1 --format='%H %ci %s'
# 2872293b1393fdb6d645170cea30fc7d1682569d 2026-08-22 21:19:02 -0400
# lifecycle: refuse dest-env role-pin and CA mint without dest admit
```

| Field | Value |
|---|---|
| Source revision | `2872293b1393fdb6d645170cea30fc7d1682569d` |
| Source epoch | `1787447942` |
| Upstream HEAD | same as local HEAD |
| Worktree | clean |
| Workspace version | `13.0.0` (`Cargo.toml` workspace package) |
| Fedora target | 44 / x86_64 (`docs/RELEASE-VERSIONING.md`) |
| Branch | `agent/drain-worklist-20260725` |
| PR | https://github.com/matthewmackes/magic-mesh/pull/71 |

A later evidence commit that cites this SHA is **not** the candidate.
New release inputs bind to `2872293b1` / `1787447942`. They do not bind
to superseded `1dfe6906609d71da9ee2ce20c860912a09b32855` / `1786813297`.

## What this does not claim

- Final S1/S4 freeze (`WL-REL-001-source-freeze-r1.md` is still due).
- REL-006 preflight pass, Maps `production_admitted`, or S7 without
  `REPLACE_*`.
- An unpublished signed 13.0.0 three-RPM dest.
- Live FUNC-023 enroll / offboard / reenroll.
- GitHub required-check authority for this SHA.

## Next honest acts

1. Bind new REL-006 receipts to this SHA/epoch only.
2. Operator fills remaining `REPLACE_*` (catalog refs, RPM signer after
   freeze, Maps production object) in a new private preflight file.
3. Live FUNC-023 enroll on an admitted dest, then reconfirm this same
   SHA before calling it the freeze.
