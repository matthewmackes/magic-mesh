# WL-CRIT-007 — overlay collision authority audit (2026-08-05)

The first collision guard and hostile fixtures validate bounded certificate and
snapshot inputs, but integration review proved that the existing authority
cannot support deployment yet. Active peers publish one leased
`/mesh/peers/<hostname>` record. Two machines using the same copied Nebula
identity therefore overwrite the same key; the record carries no distinct
machine/boot claimant identity. The unleased IP allocation record also cannot
prove concurrent claimants.

Cold boot has a second dependency cycle: authoritative etcd binds to and starts
after Nebula, while a Nebula `ExecStartPre` guard would require a fresh etcd
snapshot before Nebula starts. A slow boot can outlive any previously cached
snapshot. Packaging that drop-in would therefore risk preventing the overlay
and its authority from starting together.

The materializer now fails closed without invoking certificate/etcd tools or
changing a prior snapshot, its service has no install activation, and no timer
or RPM asset was added.

## Verification

- Farm `.170`, slot `wl-crit007-claims-materializer-r1`:
  `./install-helpers/test-overlay-identity-claims-materializer.sh`.
- Result: `PASS overlay identity authority blocker: copied-identity
  overwrite=proven; producer=fail-closed; cold-boot/package activation=absent;
  py_compile=ok`.
- The earlier guard fixture suite on farm `.50`, slot
  `wl-crit007-overlay-collision-r1`, passed 31 hostile fixtures. That proves
  parser/guard behavior only; it does not overcome the authority gaps above.

## Required forward path

Deployment requires both a lease-backed keyspace that retains distinct
enrollment-bound machine/boot claimants for one Nebula identity and a trusted
pre-Nebula path to that authority. Until both exist, the guard/drop-in remains
source-only and must not ship or be enabled on seats.
