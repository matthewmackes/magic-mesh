# WL-FUNC-021 Dell media-runtime capability evidence — 2026-08-06

## Read-only probe

The review seat was inspected without installing packages, restarting units, or
changing runtime state:

- Host: `DELL-LAPTOP`.
- Installed package: `magic-mesh-12.1.6-4.x86_64`.
- `/usr/bin/mde-shell-egui` is linked against `libmpv.so.2`, `libpipewire-0.3.so.0`, and `libavcodec.so.62`.
- `pipewire.service` and `wireplumber.service` are active for user `mm`.
- The PipeWire default core is version `1.6.8`; the ALSA sink
  `alsa_output.pci-0000_00_1f.3.analog-stereo` is present and `RUNNING`.
- `/dev/dri/renderD128` is readable and writable by the review user.
- No standalone `mpv` command is installed and `mpv-libs-devel` is absent;
  only the runtime `mpv-libs` package is present.
- `mde-shell-egui.service` is active with `/usr/bin/mde-shell-egui` as its
  main process.

## Interpretation

The Dell seat has the packaged runtime and live audio/DRM prerequisites for a
future direct playback probe. This does not prove a nonblank decoded frame,
audible output, seek/end/network-loss recovery, or rendered visual acceptance;
those still require a controlled media fixture through the running shell and
operator-reviewed capture/audio evidence.
