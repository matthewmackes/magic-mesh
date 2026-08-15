# WL-TEST-002 — five-seat MG90 connectivity preparation (2026-08-15)

This is pre-release device-readiness evidence, not installed-release acceptance.
It does not consume or expand the two-physical-seat acceptance limit.

The authorized MG90 at `172.20.0.25` identified as ESN
`ND84720078011035` and firmware `4.3.0.1`. Its HTTP, SSH, status, and device
service ports were reachable. The grouped `mackesd-integrations.service` was
configured on each known physical seat with the same explicit gateway and ESN.
MG90 credentials remain root-owned mode `0600`; the pinned host-key file and
systemd drop-in are root-owned mode `0644`.

Every seat mutation was preceded by
`/usr/libexec/mackesd/seat-update-warning`, which published the visible
AI-generated-change alert and completed its five-second wait. At
2026-08-15 13:32 America/New_York, all five integrations services were active
and each seat had published a fresh, ESN-qualified vehicle mirror:

| Seat | Address | Mirror manager |
|---|---|---|
| Basement Test Workstation | `172.20.0.15` | `Basement-Test-Workstation` |
| Dell Laptop | `172.20.146.225` | `DELL-LAPTOP` |
| Surface | `172.20.146.79` | `SURFACE` |
| T480 | `172.20.146.68` | `T480` |
| Eagle | `172.20.146.88` | `T470S-EAGLE` |

The observed paths were under
`/run/mde-bus/state/vehicle/<manager>/ND84720078011035/*.json`, with newest
timestamps within seconds of the fleet sweep. This proves current authorized
transport and mirror publication on every known seat. It does not claim GNSS
fix, radio enrichment, manager-loss/reconnect, reboot recovery, or exact
release-package acceptance; those remain WL-TEST-002 post-publication tests.
