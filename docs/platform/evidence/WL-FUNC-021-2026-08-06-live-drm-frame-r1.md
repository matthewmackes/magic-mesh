# WL-FUNC-021 direct DRM frame proof (2026-08-06)

Seat 15 produced a real direct-DRM EGL readback from the running
`mde-shell-egui` service. The capture used a temporary systemd drop-in with
the proof-only controls `MDE_DRM_PROOF_READBACK` and
`MDE_DRM_PROOF_SETTLE_MS=30000`; the drop-in was removed, systemd reloaded,
and the service restarted successfully afterward.

## Readback result

The service wrote a 1920x1080 PNG with metadata:

```json
{"source":"direct-drm-egl-readback","width":1920,"height":1080,"gbm_format":"DrmFourcc(XR30)"}
```

The captured PNG had SHA-256
`3a7ec14c51a5a46dde509c2b6c57cba5920cdfb8af5da19917d20a385ff5a199`,
61299 bytes, luma range 0..255, 576 distinct RGB values, and 12,653 pixels
with luma greater than 24. Visual inspection showed the Music surface with
transport/volume controls and the Construct shell chrome, rather than a blank
or uniform buffer.

The generic repository verifier was also run:

```text
python3 install-helpers/verify-shell-pixel-proof.py \
  --profile construct-home --png /tmp/mde-music-drm-proof-20260806.png
FAIL: bottom band has too many full-width black rows; this looks taskbar-shaped, not a floating nav pill
```

That failure is an honest profile mismatch: the verifier is locked to the
Construct Home floating-navigation profile, while this frame is the installed
Music surface with its taskbar. It is not treated as a passing Construct Home
acceptance. The service restoration proof was:

```text
ActiveState=active
NRestarts=0
no proof drop-in PASS
remote DRM artifact cleanup PASS
```

A Music-specific artifact verifier was then run against the same PNG:

```text
./install-helpers/verify-music-drm-proof.py --self-test
verify-music-drm-proof: self-test passed
./install-helpers/verify-music-drm-proof.py /tmp/mde-music-drm-proof-20260806.png
{"background_rgb":[24,24,24],"dimensions":"1920x1080","foreground_components":15,"height":1080,"luma_max":255,"luma_min":0,"luma_spread":255,"non_background_pixels":11094,"non_background_ratio":0.00535,"sha256":"3a7ec14c51a5a46dde509c2b6c57cba5920cdfb8af5da19917d20a385ff5a199","status":"passed","width":1920}
```

The helper rejects black, uniform, near-uniform, compact splash, malformed,
oversized, and symlink fixtures. Thus direct DRM readback and a Music-specific
nonblank artifact gate are proven; full rendered Music acceptance, provider
network-loss resume, handoff, and authenticated mutations remain open.
