# WL-FUNC-021 five-seat CPU proof — partial live result (2026-08-07)

## Scope

This is a bounded, read-only live probe of the canonical five physical seats
for the remaining WL-FUNC-021 CPU/NWS acceptance. It does not install
packages, restart services, induce provider loss, or mutate playback state.

Canonical seat endpoints:

| Seat | Host | Overlay | Probe result |
| --- | --- | --- | --- |
| T480 | `172.20.146.68` | `10.42.0.8` | SSH key refused |
| Eagle | `172.20.146.88` | `10.42.0.6` | reachable; stale package |
| seat 15 | `172.20.0.15` | `10.42.0.5` | release-5 CPU pass |
| Dell | `172.20.146.225` | `10.42.0.4` | release-5 CPU pass |
| Microsoft Surface | `172.20.146.79` | `10.42.0.7` | SSH key refused |

## Exact probe

```sh
MUSIC_CPU_PROOF_HOSTS=172.20.146.88,172.20.0.15,172.20.146.225 \
MUSIC_CPU_PROOF_OBSERVE_SECONDS=10 \
MUSIC_CPU_PROOF_SAMPLE_INTERVAL_SECONDS=2 \
MUSIC_CPU_PROOF_SSH_TIMEOUT_SECONDS=30 \
./install-helpers/verify-music-cpu-proof.sh
```

The verifier expected `magic-mesh-12.1.6-5.x86_64` and the default CPU
thresholds (`max <= 850‰`, `mean <= 500‰`). It returned exit code `3`
because one requested seat did not satisfy the release/provenance gate.

Observed results:

- Eagle (`172.20.146.88`): refusal before sampling; installed
  `magic-mesh-12.1.6-2.x86_64`, not the expected release 5.
- seat 15 (`172.20.0.15`): samples `[316, 305, 316, 312, 327]`‰;
  maximum `327‰`, mean `315‰`; `NRestarts` `0 -> 0`; pass.
- Dell (`172.20.146.225`): samples `[442, 334, 345, 329, 420]`‰;
  maximum `442‰`, mean `374‰`; `NRestarts` `0 -> 0`; pass.

This advances the hard acceptance by proving the current release-5 CPU gate
on two seats and reducing the remaining CPU proof blockers to deployment and
SSH access. It is not a five-seat pass: T480 and Surface were not authorized
by the configured key, and Eagle is reachable but is still on release 2.

## Next five-seat CPU command

After release 5 is installed on Eagle and the configured `mm` SSH key is
authorized on T480 and Surface, run the same bounded verifier across all five:

```sh
MUSIC_CPU_PROOF_HOSTS=172.20.146.68,172.20.146.88,172.20.0.15,172.20.146.225,172.20.146.79 \
MUSIC_CPU_PROOF_OBSERVE_SECONDS=30 \
MUSIC_CPU_PROOF_SAMPLE_INTERVAL_SECONDS=2 \
MUSIC_CPU_PROOF_SSH_TIMEOUT_SECONDS=150 \
./install-helpers/verify-music-cpu-proof.sh
```

The live NWS recovery and cross-seat handoff criteria remain unproven. The
existing NWS tests are source/farm tests, and the existing handoff tests are
source/fixture tests; this file makes no claim that either runtime acceptance
is complete.
