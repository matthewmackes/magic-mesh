# WL-ARCH-010 — independent Workload admission/placement/recovery safety proof (2026-08-06)

Status: helper and fixture proof complete; S5/S8 remain `Remaining`. This
checkpoint adds no Workload actuator, Bus publisher, systemd unit, or live
mutation.

## Scope and ownership

This slice owns only:

- `install-helpers/verify-workloads-live-proof.py`
- this evidence file

The helper is observation-only. It reads the retained Bus index and message
files, reads the pinned role file without following symlinks, and uses only
read-only service/provider probes already present in the helper. It does not
publish an operation, start/stop a service, invoke a mutating `virsh` or
`podman` verb, alter placement, or read a credential's secret bytes.

## Independent contract checks

The helper now independently validates the current `state/workloads/<node>`
contract instead of treating an arbitrary JSON object as Workload evidence:

- schema version, exact node-scoped topic identity, freshness, bounded snapshot
  size, unique workload identities, and bounded identifiers are required;
- backend, phase, power, readiness, health, pressure, and resource fields are
  checked against the closed wire vocabulary and current numeric bounds
  (`vcpu` 1–64, memory 512–262144 MiB, disk 1–4096 GiB);
- generation, retry attempt (`0..32`), retry schedule, failure diagnostics,
  and optional attachment leases are bounded; a terminal status cannot remain
  retryable or carry a future retry schedule;
- a present attachment must belong to the status workload, use a known
  protocol, have a positive generation, and not already be expired;
- the role pin is read with bounded, no-follow semantics. `workstation` and
  the contract's legacy Workstation aliases are admitted; `lighthouse`, an
  unknown role, a missing role, or a malformed role is refused.

The new switches are:

```text
--require-workload-state
--require-workload-placement
--require-workload-admission
--require-workload-recovery
--expect-workload-id ID --expect-workload-phase PHASE
```

`--require-workload-admission` requires a workload observed at or beyond the
typed `admitting` phase. `--require-workload-recovery` requires persisted
adapter-attempt evidence (`attempt > 0`) and validates the bounded retry state.
Both remain fail-closed and imply the placement/state seams they depend on.

## Self-test evidence

The helper's temporary Bus fixture covers:

- a valid Workload projection with an admitted/reconciling phase, bounded
  resources, and a persisted retry attempt;
- node/topic mismatch and an out-of-contract 65-vCPU request;
- a valid Workstation role and refusal of a Lighthouse role.

Commands:

```text
python3 -c "import ast; ast.parse(open('install-helpers/verify-workloads-live-proof.py', encoding='utf-8').read()); print('syntax: pass')"
result: syntax: pass

python3 install-helpers/verify-workloads-live-proof.py --self-test
result: verify-workloads-live-proof: self-test passed

git diff --check -- install-helpers/verify-workloads-live-proof.py
result: pass
```

No farm lane was used: this ownership slice contains only a small Python
verifier and its hermetic self-test, not a build or heavy test gate.

## Live observation

On the current Rocky development host (`rocky9-kvm2`), the non-required
inventory observed Podman `5.8.2` with `podman.socket` active. The Workload
runtime proof was unavailable: `mackesd.service` was inactive with
`NRestarts=0`, `/var/lib/mde/role.toml` was absent, and `/run/mde-bus` was
missing. The strict command therefore returned the expected refusal:

```text
python3 install-helpers/verify-workloads-live-proof.py \
  --require-workload-state --require-workload-placement \
  --require-workload-admission --require-workload-recovery --json
result: exit 2
required blockers: mackesd inactive; role pin absent; typed Workload Bus root absent
```

No live Workload operation was issued and no provider or systemd state was
changed.

## Exact remaining proof gaps

The current Workload wire snapshot contains `resources`, but it does not carry
the live `HostCapacity` sample used by the reconciler. This helper therefore
does **not** claim that CPU, memory, storage, or the mandatory host reserve was
actually measured and admitted; it only proves that any observed projection
uses bounded resource fields and an admitted/reconciliation phase. A live
capacity sample plus an oversubscription/refusal run remains required for S5.

The snapshot also has no event or sequence field proving that a daemon restart,
provider loss, adapter re-observation, suspend/rejoin, reboot, or corrected-
forward upgrade occurred. A nonzero persisted attempt proves only bounded retry
state was published; it does **not** prove restart/crash recovery or successful
provider recovery. Those S8 scenarios, plus Dell and seat-15 acceptance,
native attachment/render/input/audio/clipboard proof, required lighthouse
matrix, and package/install/upgrade proof remain open and keep WL-ARCH-010
`Remaining`.

## Source hash at capture

```text
8a8499c7231225d9b2b3981697e87d92be70e7e6cf7a9ac6c1806e1756df26e8  install-helpers/verify-workloads-live-proof.py
```
