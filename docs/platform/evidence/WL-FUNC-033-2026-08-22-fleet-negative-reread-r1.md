# WL-FUNC-033 leftover — read-only fleet-negative reread — r1

Date: 2026-08-22  
Classification: leftover-honesty / read-only fleet reconfirm; **not** epic
closure, **not** seat mutation, and **not** a delete of `own_nebula_ip`  
Source revision: `1b5282123139e0f5cbfe3b3e4dea020c1dd309e7`  
Control host: `rocky9-kvm2`  
Observed: `2026-08-22T23:34:41Z`  
`production_admitted: false`

This unit re-read systemd unit and process state on the three acceptance
seats. It did not start, stop, disable, install, or remove anything. It
does not close WL-FUNC-033. Leftover remains keep `own_nebula_ip` in lib
`voip_rtt.rs`.

## Authority

- Worklist: `docs/platform/WORKLIST.md` `WL-FUNC-033` S1.
- Prior live-negative: `WL-FUNC-033-2026-08-20-live-negative-r1.md`
  (Surface was then activating; corrected-forward that day).
- This unit: hostname + `systemctl is-active` / `is-enabled` + `pgrep`
  + `command -v mde-voice-config` only. Control key
  `/root/.ssh/mackes_mesh_ed25519`. No `AI-GENERATED-ALERT` because no
  mutation ran.

## Command

```text
ssh -i /root/.ssh/mackes_mesh_ed25519 -o BatchMode=yes \
  -o IdentitiesOnly=yes -o ConnectTimeout=10 \
  -o StrictHostKeyChecking=yes mm@<ip> \
  'hostname; systemctl is-active kamailio-mde.service; \
   systemctl is-enabled kamailio-mde.service; \
   systemctl is-active rtpengine-mde.service; \
   systemctl is-enabled rtpengine-mde.service; \
   command -v mde-voice-config || echo mde-voice-config-absent; \
   pgrep -a -x kamailio || echo no-kamailio-proc; \
   pgrep -a -x rtpengine || echo no-rtpengine-proc'
```

## Observed

| Seat | Address | hostname | kamailio-mde | rtpengine-mde | binary / process |
|---|---|---|---|---|---|
| Seat 15 | `172.20.0.15` | `Basement-Test-Workstation` | inactive / disabled | inactive / disabled | `mde-voice-config` absent; no kamailio or rtpengine process |
| Dell | `172.20.146.225` | `DELL-LAPTOP` | inactive / disabled | inactive / disabled | same |
| Surface | `172.20.146.79` | `SURFACE` | inactive / not-found | inactive / not-found | same |

Surface `not-found` matches the 2026-08-20 corrected-forward removal of
the installed unit files. No positive finding. No unit was started.

## Leftover

Keep `own_nebula_ip` in `crates/mesh/mackesd/src/voip_rtt.rs`. This
record does not archive the epic and does not claim production
admission.
