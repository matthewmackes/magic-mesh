# WL-REL-007 leftover park — coordinator cannot fill farm with workspace cargo — r1

Date: 2026-08-22  
Classification: leftover-honesty / park; **not** freeze, **not** publication  
Source revision: `bacbecf81`  
`production_admitted: false`

`farm-jobs.sh active` still listed `cargo test --workspace` for this
epic plus already-green `mackesd` / `mde-enroll` units on FUNC-023 and
FUNC-033. Those crates are compile-green. Governance forbids grinding
`cargo test --workspace` on BigBoy as filler.

Owning leftovers that the coordinator cannot execute:

| Epic | Leftover | Why the coordinator is blocked |
|---|---|---|
| FUNC-023 | live mint + enroll/offboard+reenroll | No unpublished signed candidate; helpers refuse production dest and mutation argv |
| FUNC-033 | keep `own_nebula_ip` | Keep lint is in-tree; do not archive |
| REL-006 | parked | freeze / catalog refs / RPM secret / TEST-002 |

Unblock: unpublished signed candidate + red alert + 5s live enroll
(FUNC-023), then REL-001 freeze. Do not dispatch the workspace cargo
unit as demand while this epic is Blocked.
