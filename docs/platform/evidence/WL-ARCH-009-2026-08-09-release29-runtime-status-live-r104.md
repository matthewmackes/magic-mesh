# WL-ARCH-009 release 29 runtime-status correction — r104

Date: 2026-08-09
Source revision: `b6bfd5f0` (`fix runtime status alias admission`)

## Defect and correction

Release 28 exposed a live mismatch between `Supervisor::accepts_worker` and the
worker-runtime sampler. The supervisor deliberately accepts a small set of
legacy kebab-case diagnostic names through canonical snake_case registry rows,
but the sampler performed only an exact lookup. Control, Observation, Data,
and Integrations consequently rejected the complete sample as containing an
unregistered worker. Only Actions and Compute refreshed group files, and the
health path repeatedly cycled otherwise-healthy grouped daemons.

Release 29 makes the sampler use the same exact-first, underscore-alias-second
admission as the supervisor. Unknown names still fail closed. A clean detached
farm run on `172.20.0.90` passed all 18 focused
`worker_runtime_status::tests`, including the new kebab-alias regression.

## Artifact and deployment

The native Fedora 44 BigBoy cut produced
`magic-mesh-12.1.6-29.x86_64.rpm` (90,472,077 bytes) with SHA-256:

```text
3e0d8b213c56e65de773e7ab9817e845797932c4c0182d6f696aa1e343280ff6
```

The payload and 90 MiB size gates passed. The normal BigBoy Fedora 42 builder
was restored and the Fedora 44 builder halted after the cut.

Seat 15, Dell, T480, and Eagle independently matched the staged hash and passed
separate warning-gated RPM dry-runs and installs. T480 and Eagle already had
the release-28 hard dependency correction (`qemu-ui-dbus`). The exact release
28 and 29 staging files were removed after verification; the canonical
release-29 artifact remains on the development host.

## Live acceptance

Every upgraded seat reported all of the following against current release-29
processes:

- `rpm -V magic-mesh` clean;
- all six grouped services and `mackesd.target` active;
- six fresh files: `control.json`, `observation.json`, `actions.json`,
  `data.json`, `compute.json`, and `integrations.json`;
- zero `supervisor status contains an unregistered worker` records from the
  six current service PIDs;
- Construct `Result=success`, `NRestarts=0`; and
- Nebula active.

Dell's retained `browser-vm` remained running, persistent, autostart-enabled,
and configured with 8 GiB RAM after both upgrades.

The Microsoft Surface was read-only reachable as `mm` and remains on
`magic-mesh-12.1.6-12.x86_64`; Construct is active and `mackesd.target` is
inactive. Its configured SSH key does not grant passwordless sudo and the
root-only shared lab credential is not its sudo password, so no package or
privileged state was changed there. A verified Surface sudo credential remains
required before the fifth seat can join this corrected rollout.
