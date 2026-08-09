# WL-FUNC-019 resource credential readiness — 2026-08-09

## Outcome

The packaged resource-publisher credential oneshot no longer masks a failed
materialization command. When the approved `resource/publisher-hmac` mesh
secret is absent, unreadable, malformed, or cannot be encrypted into the
host-bound systemd credential, the bounded helper exits nonzero and systemd
records `mcnf-resource-publisher-credential.service` as failed. The unit remains
a non-required `Wants=` dependency, so this honest readiness state does not
block boot or suppress the read-only resource catalog.

Once the approved secret is available, the existing idempotent path stages the
encrypted credential and exact shell drop-in and the oneshot becomes active.
No secret is generated, embedded in a unit, written to this evidence, or
included in diagnostics.

## Verification

- Farm machine 193 (`172.20.0.90`), slot
  `func019-creds-r4-20260809`.
- Credential helper `bash -n` and focused `--self-test`: passed.
- Isolated-root `systemd-analyze verify`: passed (exit 0); the analyzer emitted
  only the farm image's unrelated dangling dracut aliases.
- Focused readiness assertion proved the unit has exactly one unmasked
  `ExecStart=` and rejects the former `ExecStart=-` shape.
- Helper SHA-256:
  `9ca6aa4ba91d02ad9fce5b075af51f998188fce6ca01524d9cdc3838cf0e0fbf`.
- Unit SHA-256:
  `711e465e2f37c144ed9eef0ce17e1d5760f2da7fae4b4f6d6e26ae6d641c851b`.
- Base revision: `5febd06dc0f2063b6d55953845e498061b80aeac`.

## Remaining live limitation

The explicit Windows RDP target `172.20.146.54:3389` is discovered and
installed on Basement seat 15, but that seat still lacks the replicated
`resource/publisher-hmac` secret. This correction makes the missing credential
observable as failed readiness; it does not fabricate a key or claim an
authenticated RDP connection/render proof.
