# WL-CRIT-007 Eagle additive grouped recovery — r8

Date: 2026-08-09
Source base: `bcd5e6e01e7aa03260286733f4322fc161ebd79d`

## Live defect

Read-only inspection of Eagle (`T470S-EAGLE`, `172.20.146.88`) found the
correct `magic-mesh-12.1.6-29.x86_64` package with clean RPM verification, but
`mackesd.target` was inactive. Control repeatedly remained active while
Observation stopped and Actions, Data, Compute, and Integrations stayed down.

The journal showed repeated peer-recovery runs. Each run queued a non-blocking
`restart mackesd.target`, briefly observed all six children active, published
`recovered`, and then left the still-draining target restart to stop the groups
again. The cycle repeated roughly every minute. This was a false recovery
claim caused by observing child activity before the target's asynchronous stop
transaction completed.

## Correction

`mesh-peer-recovery.sh` no longer restarts the grouped target during network or
resume recovery. It now:

1. starts `mackesd.target` without stopping any existing process;
2. checks each of the six grouped services;
3. starts only groups that are not active; and
4. publishes `recovered` only after the existing bounded six-group readiness
   poll succeeds.

The hostile fixture models an active target with only Observation missing. It
requires exactly the XDG check, an additive target start, and an Observation
start, and rejects any target restart.

## Farm verification

Farm node `172.20.0.50`, slot `peer-recovery-additive-r20`:

- `bash -n` passed for the helper and fixture;
- the full root fixture passed every offline, identity, Lighthouse, substrate,
  boot-race, delayed-group, healthy-repeat, partial-group, lock, and trigger
  case; and
- the new partial-group case passed with no `mackesd.target` restart.

Source hashes:

- `fdd61c13bb505ea348e6da24ddd85b36eed284784dd00076978512e88fa61c4f`
  — `install-helpers/mesh-peer-recovery.sh`
- `3b1cac0c2300f3f8ba49faf9456964d4e412284bf34c1101fe71b1ce7b586970`
  — `install-helpers/test-mesh-peer-recovery.sh`

## Live deployment boundary

No Eagle mutation was performed. The configured `mm` key does not have
non-interactive sudo and direct root SSH is refused, so the installed
release-29 helper remains unchanged. A signed package carrying this correction
and a warning-gated privileged deployment are still required before Eagle can
claim live recovery. WL-CRIT-007 remains `Remaining`.
