# WL-ARCH-009 live cgroup enforcement — 2026-08-08

Release 23 on the physical Fedora 44 seat 15 proves that all six grouped
daemons run inside distinct cgroup-v2 boundaries with the packaged memory,
CPU, task, and I/O limits effective in the kernel.

## Effective grouped limits

`systemctl show` and the matching files beneath each unit's reported
`ControlGroup` agreed exactly:

| Group | Memory high/max | CPU quota/period | Tasks | I/O weight |
| --- | --- | --- | --- | --- |
| control | `384M / 512M` | `100000 / 100000` | `512` | `200` |
| observation | `256M / 384M` | `75000 / 100000` | `512` | `100` |
| actions | `384M / 512M` | `100000 / 100000` | `512` | `200` |
| data | `512M / 768M` | `100000 / 100000` | `512` | `300` |
| compute | `512M / 768M` | `150000 / 100000` | `1024` | `300` |
| integrations | `384M / 512M` | `100000 / 100000` | `768` | `200` |

Every service was active with a nonzero main PID. The cgroup reads came from
`memory.high`, `memory.max`, `cpu.max`, and `pids.max` under the exact path
reported by systemd, rather than from source declarations alone.

## Bounded refusal proof

A separate transient refusal probe on the same host used `MemoryMax=16M`,
`MemorySwapMax=0`, and `TasksMax=16`, then attempted one 128 MiB allocation.
The kernel/systemd result was:

```text
Finished with result: oom-kill
Main processes terminated with: code=killed, status=9/KILL
Memory peak: 16M (swap: 0B)
REFUSAL rc=1 result=oom-kill code=2 status=9 peak=16777216
RESOURCE_REFUSAL_PASS
```

The probe unit was reset afterward. `mackesd.target` and all six grouped
services remained active, so the refusal did not disturb the production
daemon boundaries.

## Remaining acceptance gap

Declared cgroup values and host-level hard refusal are proven, in addition to
the separate per-group crash recovery evidence. Optional unconfigured-worker
quiescence, ownership/provider completion, responsive Workers captures, and
fleet convergence remain; ARCH-009 stays `Remaining`.
