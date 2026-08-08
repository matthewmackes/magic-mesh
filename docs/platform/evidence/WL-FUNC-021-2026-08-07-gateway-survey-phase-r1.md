# WL-FUNC-021 — gateway survey startup phase and live release (2026-08-07)

## Implementation

- `airspace::AirspaceWorker` now delays its first MG90 root-SSH/`iw` survey by
  a deterministic host-derived phase capped at 250 ms and capped again by the
  configured poll interval. Shutdown remains cancellation-aware.
- `vehicle::VehicleWorker` now applies the same bounded deterministic phase to
  its first MG90 current-status batch. Pending snapshots and heartbeats remain
  available immediately; only the expensive gateway batch is delayed.

## Farm verification

- BigBoy `.130`: `cargo check -p mackesd --bin mackesd --features async-services
  --locked` passed.
- Farm `.50`: focused airspace phase regression passed 1/1.
- Farm `.90`: focused vehicle phase regression passed 1/1.
- Existing mde-musicd, Music UI, Media UI, MPRIS, and mackesd source gates
  remained green. The build emitted existing warning classes only.

## F44 release and live deployment

The native F44 full build completed on BigBoy. Base and lighthouse payload gates
passed. The base artifact used for both targets is:

```text
magic-mesh-12.1.6-5.x86_64.rpm
sha256 b824c63fd45de45b7a3a627137a4cb58065cd902bd162222911e467e17714193
```

The RPM requires the F44 media sonames (`libavcodec.so.62`,
`libavformat.so.62`, `libavutil.so.60`, `libswresample.so.6`,
`libswscale.so.9`, `libplacebo.so.360`, and `libmpv.so.2`). Each target passed
the documented `rpm -Uvh --test --replacepkgs --force --nosignature` transaction
before installation.

- Seat15 `172.20.0.15`: replaced `12.1.6-4` with `12.1.6-5`; restarted
  `mackesd.service` and the user `mde-musicd.service`. Live Music verification
  passed, including RPM-owned executable provenance, ping, state, album list,
  payload, and `rpm -V`. The final combined ten-second CPU proof passed with max
  `216`‰ and mean `181`‰ of one CPU.
- Dell `172.20.146.225`: same-NVR force replacement completed; restarted
  `mackesd.service` and the user `mde-musicd.service`. Its graceful daemon stop
  stalled, so only the exact `mackesd.service` cgroup was SIGKILLed before a
  clean start; no other service or browser VM domain was targeted. Live Music
  verification passed. The final combined ten-second CPU proof passed with max
  `367`‰ and mean `283`‰ of one CPU.

## Remaining boundary

The two-seat package/provenance/CPU result does not close the epic. Physical
renderer and Chromecast acceptance, provider-loss continuity, cross-seat owner
handoff, five-seat CPU/NWS recovery, and the remaining live auth/rotation
acceptance still require their respective reachable fixtures and evidence.
