# WL-ARCH-010 Display1 damage delivery — 2026-08-09

## Outcome

QEMU `UpdateDMABUF` callbacks are no longer discarded by the daemon. The
lease-bound Display1 relay now validates each non-empty damage rectangle
against the most recently accepted scanout and forwards bounded metadata to
the shell without another descriptor. Peer credentials and lease freshness
are rechecked before each nonblocking send.

The shell rejects damage before a frame, stale workload/lease generations,
unexpected descriptors, zero or overflowing geometry, and rectangles outside
the retained frame. Accepted damage reaches the DRM loop as a typed poll
result and refreshes the retained native framebuffer through page flips on all
active heads. Damage without a native scanout fails closed.

## Farm verification

- BigBoy (`172.20.0.130`), slot
  `arch010-display1-damage-daemon-r1`: focused mackesd Display1 broker tests
  passed 9/9 after the exact synchronized rerun.
- BigBoy (`172.20.0.130`), slot
  `arch010-display1-damage-shell-r1`: focused shell Display1 tests passed 5/5.
- Machine 193 (`172.20.0.90`), slot
  `arch010-display1-damage-drm-r1`: focused mde-egui Display1 tests passed 3/3.
- Machine 196 (`172.20.0.196`), slot
  `arch010-display1-damage-fmt-r2`: exact `rustfmt --check` passed for the
  three changed Rust files. Whole-workspace formatting remains affected by
  unrelated concurrent work and was not used as evidence for this slice.

## Source hashes

- `94b2bee8f60a282b336d6d31cc4e5915738b157e46e81690c4e332355fadf58d`
  — `crates/mesh/mackesd/src/display1_broker.rs`
- `4b4987b9a024877ca3a87a1b02d21c745c262f77eaaad03b9c625d30d808d046`
  — `crates/desktop/mde-shell-egui/src/display1_client.rs`
- `f79d396bcc6aa221eabdcccbfd60f2824b2dd4a9ccffc982ae9afc0c7bf8b97c`
  — `crates/shared/mde-egui/src/drm.rs`

## Remaining boundary

This checkpoint wires bounded same-buffer damage through the existing local
Display1 transport; it does not close WL-ARCH-010. Live Dell/seat-15 native
presentation proof is still required. The inherited Unix stream transport's
multi-envelope framing/coalescing behavior also needs an explicit hostile
proof or a packetized transport before this path can be called complete.
