# WL-FUNC-033 live negative and corrected-forward evidence — 2026-08-20

## S1 fleet negative

The three required physical seats were reachable over bounded SSH and checked
read-only before correction:

| Seat | Address | Before correction |
|---|---|---|
| Seat 15 / Basement-Test-Workstation | `172.20.0.15` | both units inactive/disabled; `mde-voice-config` absent |
| Dell / DELL-LAPTOP | `172.20.146.225` | both units inactive/disabled; `mde-voice-config` absent |
| Surface / SURFACE | `172.20.146.79` | both units activating/enabled; `mde-voice-config` absent |

The Surface positive was not treated as a negative. Its unit metadata showed
the retired `mackesd voice render-config` pre-start path and an auto-restart
loop.

## Corrected-forward mutation

Per seat-mutation governance, an `AI-GENERATED-ALERT` was emitted and the
operator-required five-second hold completed before changing Surface. The
target was `SURFACE (172.20.146.79)`. The corrective command stopped and
disabled both retired units, removed their installed systemd unit files, ran
`systemctl daemon-reload`, and reset stale failure state.

Post-correction result on Surface:

```text
kamailio-mde.service inactive / not-found
rtpengine-mde.service inactive / not-found
no kamailio or rtpengine process
```

## S2/S3 source sweep

Source deletion is in commit `c3b589da`:
`Land feature completion and release hardening`. The commit removes the
retired systemd units, package references, role/config paths, and retired
policy rows. The remaining source scan is limited to historical ledger or
archive references.

This evidence proves the required live negative and corrected-forward
deployment state. The broader full mackesd farm gate remains separately
blocked by unrelated app-catalog, role-provisioning, and worker-registry
regressions.

