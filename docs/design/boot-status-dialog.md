# BOOT-STATUS — desktop boot mesh-services status dialog

Design survey locked 2026-06-17 (10-question `/plan`), prompted by the operator
wanting an informative, at-boot view of how the mesh fabric + app daemons come
up — the "connection handshake, setup, pings" sequence — surfaced in the
Workbench **HOME** tab. Pairs with [BOOT-PEERS-1] (the peers-settling indicator):
this is the richer, whole-fabric version.

## Locks (10)

| # | Decision | Lock |
|---|----------|------|
| Q1 | Launch | **Auto-popup at desktop session start + a persistent HOME-tab panel** (same view both places) |
| Q2 | When all-green | **Shrink to a glanceable "mesh ready" status chip** (re-expandable); don't fully close |
| Q3 | Per-service detail | **Full handshake sub-steps always shown** (installer-style checklist), not just a pill |
| Q4 | Service scope | **Fabric + app daemons** (Nebula, mackesd, mde-bus broker, QNM-Shared mount, lighthouse, directory **+** musicd, voice-hud, KDC, netdata) |
| Q5 | Data source | **A new mackesd `boot-readiness` aggregator worker** probes each service (steps + pings) and publishes ONE snapshot to the bus; the dialog renders it |
| Q6 | Liveness | **Continuous pings/RTT to the lighthouse(s) + peers** while open |
| Q7 | Layout | **Boot-sequence dependency chain** — ordered the way the mesh actually comes up, showing what blocks what |
| Q8 | On failure | **Inline remediation actions** per failed step (Retry / Restart service / View journal) |
| Q9 | Refresh | **Real-time bus stream** (sub-second repaint as each step completes) |
| Q10 | Node scope | **This node in full detail + a compact roll-up row per other mesh node** (ready / degraded / down) |

## Architecture

```
  desktop session start ──► auto-popup (BOOT-STATUS dialog)  ◄── also in Workbench ▸ HOME
                                      │  renders + streams
                                      ▼
                       bus topic: state/boot-readiness   (real-time)
                                      ▲  publishes one snapshot/tick
                          mackesd `boot_readiness` worker
                            probes, IN DEPENDENCY ORDER:
   ① Nebula up + cert loaded  ② overlay IP assigned  ③ mackesd serving
   ④ mde-bus broker bound (overlay IP)  ⑤ QNM-Shared mounted + writable
   ⑥ peer directory replicated (peer count)  ⑦ lighthouse reachable (ping/RTT)
   ⑧ app daemons: musicd / voice-hud / KDC / netdata (active + reachable)
                            + continuous ping RTT to LH + each peer
                            + a compact readiness verdict per peer (roll-up)
```

- **Each step is a struct** `{ id, label, group, status: pending|ok|fail|degraded,
  detail, blocked_by, since_ms, remediation? }`. The worker emits the ordered
  list; the dialog draws the dependency chain + per-step expandable detail.
- **Single authoritative source (Q5):** the worker owns the probing; the GUI,
  the applet chip, and any CLI all render the same snapshot. Works headless
  (a Server/Lighthouse has the same readiness data even with no desktop).
- **Streaming (Q9):** the worker publishes every tick during boot (fast cadence
  while anything is `pending`, slowing once steady); the dialog subscribes and
  repaints live, so you watch the handshake happen.
- **Chip (Q2):** once every step is `ok`, the dialog collapses to a green "mesh
  ready" chip; any later regression re-expands / re-alerts.
- **Remediation (Q8):** a failed step carries a `remediation` verb the dialog
  renders as a button → routed to the existing surfaces (`systemctl` restart via
  the Mesh Services path, a journal tail, or a re-probe).

## Tasks → BOOT-STATUS-1..6 (see WORKLIST)

1. **BOOT-STATUS-1** — mackesd `boot_readiness` worker: probe the fabric steps in
   dependency order, publish the ordered snapshot to `state/boot-readiness`.
2. **BOOT-STATUS-2** — extend the worker with app-daemon probes (musicd, voice-hud,
   KDC, netdata) + continuous lighthouse/peer ping RTT.
3. **BOOT-STATUS-3** — peer roll-up: each node's readiness verdict aggregated for
   the "other nodes" rows.
4. **BOOT-STATUS-4** — Workbench HOME panel: render the dependency chain + always-on
   sub-steps, streaming live; collapse to the ready-chip when all-green.
5. **BOOT-STATUS-5** — auto-popup at session start (a .desktop autostart that opens
   the dialog), dismiss/minimize behavior.
6. **BOOT-STATUS-6** — inline remediation actions (Retry / Restart / View journal)
   wired to the existing service-control + journal paths.

## Acceptance (runtime-observable)
- On a fresh desktop login the dialog auto-opens and shows each fabric step
  transition pending → ok in dependency order, with live ping RTT to the
  lighthouse + peers; it collapses to a "mesh ready" chip once all-green.
- A deliberately-stopped daemon (e.g. `systemctl --user stop mde-musicd`) shows
  that step `fail` with a working **Restart** button that brings it back.
- Killing the overlay shows ② overlay-IP + everything downstream go `fail`/
  `blocked_by`, and recovers live when Nebula returns — no manual refresh.
- The panel is reachable any time from Workbench ▸ HOME and renders the same
  state as the auto-popup.
- Other mesh nodes appear as compact ready/degraded/down roll-up rows.

## Risks
- **Probe cost** — continuous pings + per-tick probing must stay cheap on the
  947 MB lighthouses (reuse existing liveness data; back off once steady) so it
  never becomes a netdata-style load source.
- **Auto-popup nuisance** — must collapse to the chip quickly on a healthy boot
  so it isn't a daily speed bump; the chip, not a modal, is the steady state.
- **Step model drift** — the dependency order must track the real boot chain;
  keep it single-sourced in the worker, not duplicated in the GUI.

## Out of scope (v1)
- Historical boot-time trends / a boot-trace timeline (Q7 chose dependency-chain
  over timeline; a timeline view is a later add).
- Remediation beyond restart/retry/journal (no config editing from the dialog).
