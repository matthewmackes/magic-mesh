# WL-FUNC-023 leftover (3) — dest-backed offboard + reenroll — r1

Date: 2026-08-23  
Classification: live leftover (3) on Seat 15; **not** freeze or
`production_admitted`  
Control host: `rocky9-kvm2`  
Seat: `Basement-Test-Workstation` `172.20.0.15`  
Mesh-id (existing, not invented): `mcnf-clean-20260728`  
`production_admitted: false`

## Authority

Operator 2026-08-23: create the confirmation dest (no other source),
acquire REL-006 open-source inputs, and run live-seat leftover (3).

## Dests authored (hashes only)

| Dest | Mode | Note |
|---|---|---|
| `/root/mcnf-private/lifecycle-confirmation-ed25519` | `0600` | Ed25519 seed; never printed |
| `/root/mcnf-private/lifecycle-confirmation.json` | `0400` | `verifying_key_sha256=bc37d10b7d79a825ce4859384692f31bf573c7811d19a599b1b07606b0aeb4a2` |
| `/root/mcnf-private/enroll-endpoint-fp-der` | `0400` | SHA-256 of enroll-endpoint **DER** (the join pin) |

PEM `sha256sum` of the cert file is not the join pin.

## Warning

Seat 15 has `mde-bus`. `seat-update-warning.sh` ran on the seat
(`AI-GENERATED-ALERT` + 5s) before leave.

## Leave

Installed `mackesd leave --yes` accepted a dest-signed
`LifecycleConfirmationV1` (`FORCE OFFBOARD 1 SYSTEMS`). After leave:
no host cert, no CA, no role pin. `remove-peer` was not used.

## Join

Installed `enroll --token-stdin` still hits the retired replicated-file
path. Current-tree `join` with `?fp=<DER sha256>` completed network
enroll:

- mesh `mcnf-clean-20260728`
- node `peer:Basement-Test-Workstation`
- overlay `10.42.0.5` (same underlay address as before leave)
- `/etc/nebula/ca.crt` and `identity/current/host.crt` present
- `nebula.service` is **active** after `reset-failed` (identity-guard PASS)
- `mackesd.service` unit is still absent on this seat (pre-existing)

Bearer dest was not replaced. Token never printed. Seat temp copies were
removed.

## Non-claims

- `production_admitted` was not flipped.
- Dell, Surface, Eagle, and T480 were not offboarded.
- This is not a 13.0.0 freeze or publish.
