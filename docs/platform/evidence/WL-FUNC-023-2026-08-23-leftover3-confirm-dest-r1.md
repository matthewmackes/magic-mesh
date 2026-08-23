# WL-FUNC-023 leftover (3) — confirmation dest hunt — r1

Date: 2026-08-23  
Classification: leftover-honesty; **not** live enroll, offboard, freeze, or
`production_admitted`  
Source revision: `7a3b52ccd` plus this record  
Control host: `rocky9-kvm2`  
`production_admitted: false`  
`enroll_succeeded: false`

## Authority

Operator 2026-08-23: execute the drain. Leftover (3) is live enroll or
authorized offboard+reenroll under red `AI-GENERATED-ALERT` + 5s.

## Hunt (no mutation)

`mackesd leave --yes` requires a signed `LifecycleConfirmationV1` and a
64-hex verifying key. `remove-peer` bans re-enroll. `mackesd reenroll`
refreshes a local SQLite fingerprint only and is not leftover (3).

Dest names under `/root/mcnf-private` (unread except path/mode): bootstrap
SSH dests, enroll-bearer dest, unpublished signed candidate. No
lifecycle/confirmation/authority signing dest. Unpublished cut listing
has `full-pull`, `install-2026-08-22.log`, `server-pull`, `signed` only.

Seat 15 has `/var/lib/mackesd/lifecycle` (checkpoint dir, not a signing
dest). LH1 has `relay-trust-authority.ed25519` (relay trust, not
lifecycle confirmation). Dell find for authority/confirm/lifecycle dests
returned empty. No `/root/.mcnf` dest tree.

`SshBootstrap` refuses `Target::Enrolled`. Bootstrap known-hosts dest
pins Seat 15, which is already enrolled. Eagle and T480 have
`/etc/nebula/host.crt` symlinks and `magic-mesh-13.0.0-35`; they are not
fresh-box targets. No spare DO droplet exists besides the three
lighthouses and a powered-off Asterisk box.

LH1 `:4243` remains reachable from Seat 15 and Dell. Overlay ping to
`10.42.0.1` still fails.

## Non-claims

- No confirmation key was invented or generated.
- `leave`, `remove-peer`, `join`, `offboard`, and `SshBootstrap` were not
  executed.
- The leftover-(1) bearer dest was not read or replaced.
- `production_admitted` was not flipped.

## Blocker

`WL-FUNC-023` stays `Remaining`. Leftover (3) waits on a dest-backed
confirmation signing key, or a not-yet-enrolled authorized seat whose
bootstrap identity dest admits. Neither exists on this control host.
