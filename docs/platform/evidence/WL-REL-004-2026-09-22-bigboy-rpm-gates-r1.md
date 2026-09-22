# WL-REL-004 BigBoy RPM size gates and matrix revision — r1

Date: 2026-09-22
Classification: farm-package checks on the freeze-SHA signed RPMs.
Not publication. Not `production_admitted`. Not a live-seat gate.
`published: false`

## Dom0

`XEN-BIGBOY` `172.20.145.165`: 12 cores, 32689 MiB. `mcnf-build-f44`
stays halted. `mcnf-build-52` is running with 12 vCPUs and 20481 MiB.
`memory-static-max` is 21474836480, so the spare ~8 GiB cannot be added
without halting the guest. The in-flight `cargo test -p mackesd` was left
running. Guest credit weight is 2048.

## RPM size gates

On `.130`, three parallel `verify-rpm-payload.sh size` runs against
`/home/mm/mcnf-signed-rpms-42035dcbd`:

| Role | Result |
|---|---|
| Workstation | PASS, 89.6 MiB |
| Server | PASS, 55.4 MiB |
| Lighthouse | PASS, 14.9 MiB |

Workstation `payload` also passed. Lighthouse and Server `payload` runs
report missing workstation browser files; the matrix lighthouse command
is the size gate, which passed.

## S2

```
verify-release-gate-matrix: FAIL: source_revision does not match expected revision 42035dcbd76b03b8323399892052b21a96e2e233
```

`install-helpers/release-gate-matrix.json` is still bound to `46ea21b6…`.
