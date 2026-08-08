# WL-FUNC-021 Music UI Dell completeness and release-5 deployment (2026-08-07)

## Finding

Dell was running a valid embedded Music daemon snapshot, but the Music top bar
derived its connection badge from the optional legacy worker command handle.
Embedded construction intentionally has no worker, so a reachable source could
be displayed as `Connect source`. The compact workspace also did not expose the
daemon-owned Sources, Queue, or Now Playing views as routes, and a daemon-owned
current track could lack Now Playing metadata when it was present only in a
bookmark or queue entry.

## Implementation

- `crates/desktop/mde-music-egui/src/app.rs` now derives the status badge from
  retained `MusicWorkspaceSnapshotV1` source reachability, distinguishes
  `Connected`, `Source unavailable`, and `Connecting`, and adds explicit
  Sources, Queue, and Now Playing routes to the compact navigation and library
  rail.
- The Sources view renders the daemon's source capabilities, authentication
  requirement, feature list, and playback targets without inventing provider
  state. Queue and Now Playing are reachable without the legacy worker.
- `crates/desktop/mde-music-egui/src/model.rs` materializes Now Playing from
  catalog, collection, bookmark, and queue metadata, retaining honest fallback
  labels and durations from the typed daemon snapshot.
- Focused regression coverage verifies daemon-truth connection status, source
  capabilities/targets, and bookmark-backed Now Playing metadata.

## Farm gates

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-ui-dell-completeness-r4 \
  ./install-helpers/xcp-build.sh cargo test -p mde-music-egui --locked -- --nocapture
PASS: 58 passed, 0 failed; main and doctest lanes passed

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=music-ui-dell-completeness-fmt-r4 \
  ./install-helpers/xcp-build.sh cargo fmt -p mde-music-egui -- --check
PASS

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=music-handoff-authority-r2 \
  ./install-helpers/xcp-build.sh cargo test -p mde-musicd handoff_completion --locked -- --nocapture
PASS: 3 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=musicd-handoff-full-r2 \
  ./install-helpers/xcp-build.sh cargo test -p mde-musicd --locked -- --nocapture
PASS: 192 passed, 0 failed
```

The final Fedora 44 full RPM was rebuilt on BigBoy in slot
`music-current-release5-ui-rail-fix-r5`. The package passed the RPM payload
size gate and contains both `/usr/bin/mde-musicd` and
`/usr/bin/mde-shell-egui`:

```text
magic-mesh-12.1.6-5.x86_64.rpm
size 87549583 bytes
SHA-256 85694a7cd8608e0462b3705b949f5f32d02aa46050488924b6303d204d726352
```

## Live deployment

The exact SHA-256-matching artifact was copied, dry-run checked, installed with
`rpm -Uvh --replacepkgs --force`, and restarted on both seats:

```text
172.20.146.225  DELL-LAPTOP
172.20.0.15      Basement-Test-Workstation (seat 15)
```

`install-helpers/verify-music-live-seat.sh` passed on each host. Both report:

```text
mde-musicd.service active (NRestarts=0)
RPM-owned /usr/bin/mde-musicd
mde-musicd ping answered
action/music/get-state answered on /run/mde-bus
action/music/list-albums answered on /run/mde-bus
rpm -V magic-mesh reports no installed-file differences
verify-music-live-seat: PASS
```

Dell's first post-install restart left an old deleted executable inode behind;
the service was stopped and started explicitly, then the verifier passed with
the live process resolved to the RPM-owned `/usr/bin/mde-musicd`. The final
seat check therefore proves the running daemon, not only the installed payload.

The retained workspace snapshots also show a reachable daemon source on both
seats. Dell is at revision 237 and seat 15 at revision 17; each has one source,
one reachable source, one shelf, one collection, one target, and no active
playback at the read-only checkpoint.

## Direct Dell DRM proof

Dell has no compositor-backed X/Wayland/Sunshine capture path, so the final
visual proof used the shell's direct DRM EGL readback. With a temporary,
proof-only `require_login_at_boot:false` fixture, the Music route was captured
at 1366x768 after a bounded 10-second settle. The health modal was closed by a
single bounded Escape input before capture; no application or provider state
was changed.

```text
metadata: {"source":"direct-drm-egl-readback","width":1366,"height":768,"gbm_format":"DrmFourcc(XR30)"}
size: 100822 bytes
SHA-256: a60ca9a2c09c430ba392f89d396c2033b8a02d3c15659d1b4407b83747a2fa0f
verify-music-drm-proof.py: passed
```

Visual inspection confirmed the green `Connected` badge, Music header/search,
library content, Now Playing/Queue/Sources navigation, and a vertical library
rail with no overlap. The temporary power fixture, systemd drop-in, proof PNG,
and metadata were removed afterward; Dell returned active with `NRestarts=0`,
no `MDE_DRM_PROOF_*` environment, and the secure boot-lock default restored.

## Boundary

This closes the identified Dell incomplete/false-disconnected UI path and
deploys it to both live seats, with bounded direct-DRM pixel proof on Dell.
Provider-loss continuity, live cross-seat handoff, and five-seat CPU/NWS
acceptance remain open WL-FUNC-021 work.
