# WL-CRIT-007 — runtime overlay claimant publication (2026-08-05)

Telemetry now derives the lease-backed overlay claim from validated local public
identity facts and atomically publishes it with the peer row. The active Nebula
certificate name and address must exactly match the node and live overlay
address. Machine and boot IDs are emitted only as certificate-scoped,
domain-separated SHA-256 digests, so copied certificates remain detectable
without creating stable cross-identity machine pseudonyms.

The generation-backed `identity/current` layout is accepted only through its
owner-controlled relative generation switch; an unsafe or broken switch cannot
fall back to the legacy flat certificate. The already-admitted public certificate
bytes are staged in a root-owned mode-0600 create-new/O_NOFOLLOW runtime file for
`/usr/bin/nebula-cert`. Parser output and runtime are bounded, and oversized or
hung parser process groups are killed and reaped. Missing or malformed identity
facts suppress both peer and claim publication; there is no legacy hostname-only
etcd fallback when claimant authority is expected.

## Verification

- BigBoy `.130`, slot `wl-crit007-runtime-claim-r2`:
  `cargo test -p mackesd --lib telemetry::tests -- --nocapture`.
- Result: `18 passed; 0 failed; 4464 filtered out`.

## Remaining acceptance edge

The pre-Nebula collision guard still needs an authenticated local snapshot of
active lease claims before the overlay starts. Packaging/systemd activation and
multi-seat cold-boot collision validation remain deliberately unclaimed.
