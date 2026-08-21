# FARM — fleet repair and capacity correction (2026-08-21)

Operator directive: "correct or recreate ANY node that creates an issue. You
control DOM0 for all hosts." All five dom0s were reachable with root over the
mesh key, so every fix below was a correction; **no node needed recreating**.

Scope is farm build VMs only. No installed seat, mesh identity, or Nebula
config was touched.

## Starting state — one node down, four disk-starved

`install-helpers/farm-topology.sh table` exited non-zero: `.196` UNREACHABLE,
so the fleet had never reported 5/5.

| Node | cap | free /home | state |
|---|---|---|---|
| `.50` | 2 | 15G | light-only |
| `.90` | 2 | 33G | light-only |
| `.130` (BigBoy) | 3 | 16G | light-only — earlier the same day it hit **548K** |
| `.170` | 2 | **7.3G** | below even the light floor |
| `.196` | 1 | ? | **unreachable** |

## `.196` — wedged guest, not a dark VM and not a missing toolchain

Symptom: answered `ping` and accepted TCP on 22, then
`Connection timed out during banner exchange`. So sshd was alive but could not
complete a login. This is **not** the documented "dark VM" (that VM never gets
an IP); `.196` had passed job `56644bb14a6c` at 14:11 the same day.

Host was healthy — 24461 MB total with 4132 MB free, the VM holding its full
7168 MB — so the fault was inside the guest. Clean `xe vm-shutdown` hung,
consistent with a wedged guest; `force=true` then `vm-start` recovered it, SSH
answering 30s after boot.

Two findings from the post-boot inspection:

1. **Cause was disk**: `~/magic-mesh` held 54G on a 59G root.
2. **The toolchain was never missing.** `cargo` and `g++` were both present.
   The earlier `TOOLCH bare` reading was a *false negative* — the probe could
   not complete over the wedged SSH, which is indistinguishable from an absent
   toolchain unless the probe is bounded. Acting on that reading would have
   meant reinstalling a toolchain that was already fine.

Fixed in the dispatcher: `toolchained()` and `free_kib()` are now wrapped in
`timeout` (`MCNF_DISPATCH_PROBE_TIMEOUT`, default 20s), because ssh's
`ConnectTimeout` covers only the TCP connect and not a stalled banner. A
half-dead node previously hung a probe *while holding a slot reservation*.

## Reclaimed 74G of retired and stale trees

The dispatcher now builds in `~/magic-mesh-farm-d<N>`, so the shared
`~/magic-mesh/target` is retired — and it was invisible to
`farm-slot-gc.sh`, which only globs `magic-mesh-farm-*`. Removed it (40G on
`.170`, 53G on `.196`) plus month-old July agent scratch (`wl-*`,
`func016-*`). Surface build inputs (`dev-snapshot-*`, `magic-mesh-guest-*`)
were deliberately left in place as possibly-governed inputs.

> Honest note: the ad-hoc cleanup's `pgrep -c cargo` guard misfired (it
> compared a two-line string and errored, so it fell through to the delete
> instead of skipping). It was safe only because a survey moments earlier had
> confirmed `cargo=0` fleet-wide. The dispatcher's own `reclaim_on` does not
> share this flaw — it is lock-based and never touches a reserved slot.

## Grew three root disks — 175 GiB provisioned

BigBoy has 12 vCPU and cap 3, but its three warm slot workspaces alone used
39G of an 80G disk. Deleting warm targets repeatedly would only trade disk for
rebuild time, so the disks were sized to the declared caps instead.

Procedure per node: hold the node's dispatcher slot locks with `flock` so
nothing lands mid-window, confirm `pgrep -c cargo` is 0, `xe vm-shutdown` →
`xe vdi-resize` → `xe vm-start`, then `btrfs filesystem resize max /home`.
Cloud-init's growpart had already extended `xvda4` on boot, so `growpart`
reported `NOCHANGE` — expected, not a failure.

| Node | root disk | dom0 SR | free after |
|---|---|---|---|
| `.130` BigBoy | 80 → **180 GiB** | 398 GiB (225 free) | 16G → **117G** |
| `.90` | 80 → **130 GiB** | 192 GiB (79 free) | 33G → **86G** |
| `.50` | 80 → **105 GiB** | 192 GiB (50 free) | 15G → **57G** |
| `.170` | 64 GiB — unchanged | **75 GiB, 53 free** | 7.3G → 34G (reclaim only) |
| `.196` | 60 GiB — unchanged | **75 GiB, 59 free** | → 48G (reclaim only) |

`.170` and `.196` were **deliberately not grown**: their VDIs already nearly
fill 75 GiB SRs, so growing them would overcommit the storage and trade a guest
ENOSPC for a worse SR-level one. They remain `light-only` by design, which the
disk-shaped admission envelope already routes around.

## Verified end state

```
  NODE             REACH  TOOLCH  FREE      SLOTS   STATE
  172.20.0.130     up     ready   116.8G    3/3     ready
  172.20.0.170     up     ready   33.7G     2/2     light-only
  172.20.0.50      up     ready   56.5G     2/2     ready
  172.20.0.90      up     ready   85.5G     2/2     ready
  172.20.0.196     up     ready   47.0G     0/1     saturated (building)
  TOTAL_FREE=9 of 10 slots
```

`farm-topology.sh table` now exits **0 — 5/5 nodes up**, the first clean fleet
health verdict. `.196` reports `ready` toolchain and is actively building. Four
of five nodes can now admit a heavy whole-workspace job; before this, none
could.

Offline contract checks: `farm-dispatch.sh --self-test` (36 assertions),
`farm-reconcile.sh --self-test`, `farm-jobs.sh --self-test`,
`xcp-build.sh --route-test` all pass; `shellcheck -S warning` clean.

## Residual

- `.170` / `.196` cannot host heavy jobs until their dom0 SRs grow. That is a
  hardware/storage decision, not a scheduler one.
- No shared sccache on the nodes (`~/.cache/sccache` absent on BigBoy), so cold
  slot rebuilds pay full cost. Tracked as WL-BUILD-002; verify with
  `install-helpers/farm-sccache-proof.sh status` before claiming cache behavior.
- BigBoy carries two halted VMs (`mcnf-build-f44` 24576 MB, `mcnf-build`
  16384 MB) consuming SR. Left untouched — `mcnf-build-f44` may be the F44
  target builder, and neither could start given host memory anyway.
