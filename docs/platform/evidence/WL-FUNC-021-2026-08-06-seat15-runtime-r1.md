# WL-FUNC-021 seat-15 runtime/package probe — 2026-08-06

This is a read-only live probe of the canonical Basement workstation seat
(`172.20.0.15`). It records runtime/package facts without installing, rebooting,
or changing the seat.

## Observed green boundaries

- Host: `Basement-Test-Workstation`; role file pins `workstation`.
- Installed package: `magic-mesh-12.1.6-4.x86_64`.
- `mde-musicd.service` is active with `MainPID=1288`, `NRestarts=0`, and
  `ExecMainStatus=0`.
- `mde-shell-egui.service` is active with `NRestarts=0`; its boot journal
  reports `mde-shell-egui starting` with `drm:true`, followed by the `seat`,
  `surfaces`, `mesh_snapshot`, and splash-handoff milestones.
- `/dev/dri/renderD128` exists and `ldd` resolves the shell's `libmpv.so.2`,
  FFmpeg `avcodec`/`avformat`/`avutil`/`swresample`/`swscale`, PipeWire, and
  JACK libraries; no `not found` entries were reported.
- The logged-in `mm` user session has PipeWire, PipeWire-Pulse, and WirePlumber
  active. `pactl info` reports PulseAudio on PipeWire 1.6.8 with a real analog
  default sink/source, and the graph contains `alsa_playback.mde-musicd` plus
  the analog sink/source.

## Remaining boundary

The installed binary identifies itself as `12.1.6 "Construct" · nogit ·
2026-08-05 · dev`, so this is package/runtime acceptance for the installed
review payload, not proof that the current dirty source state has been shipped.
The daemon journal also reports `music mutation authorization unavailable;
mutations are disabled`; live provider credentials, real provider playback,
mid-track network-loss resume, target/cast handoff, and rendered/audio capture
remain open. No success is inferred from the active services alone.

## Reproduction

All commands were run read-only over SSH as `mm` using the configured mesh key:

```text
ssh -i /root/.ssh/mackes_mesh_ed25519 mm@172.20.0.15 \
  'rpm -q magic-mesh; systemctl is-active mde-musicd.service mde-shell-egui.service; \
   systemctl --user is-active pipewire pipewire-pulse wireplumber; \
   pactl info; pw-cli ls Node; ldd /usr/bin/mde-shell-egui /usr/bin/mackesd'
```

The probe was performed against source worktree `e52322ec048c1b5736897c249b2279393e36d5f0`; it did not mutate that worktree or the seat.
