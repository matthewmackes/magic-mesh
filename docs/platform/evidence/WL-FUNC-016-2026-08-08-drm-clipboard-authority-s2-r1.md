# WL-FUNC-016 direct-DRM clipboard authority — 2026-08-08

The direct DRM runner now owns one bounded seat-local Clipboard V2 authority.
It retains richest-first MIME offers and exact-generation selections, revokes
stale selections on focus/app changes and lock-curtain engagement, and preserves
the existing normalized UTF-8 text shortcut behavior. The shell is a client of
that authority rather than a second selection store.

Bus access moved to a bounded worker channel. Render frames perform only
nonblocking enqueue/poll operations, and one Ctrl+V completes when asynchronous
materialization arrives without requiring a second keypress. Rich HTML and
plain-text offers remain intact when the exact text representation is selected.

## Focused farm verification

- `.50`, focused `mde-egui --features drm clipboard` suite: 19 passed, 0 failed.
- BigBoy `.130`, DRM-enabled shell MIME-preservation test: 1 passed, 0 failed.
- Exact Bus text compatibility test: 1 passed, 0 failed.
- Scoped `git diff --check`: passed.

## Source hashes

```text
b3125e5ca4d7d68a98477ce7e3e5fa6f22aa266a8e432e1efb384ed4958b8dc4  crates/shared/mde-egui/src/clipboard.rs
3a8e787a8c8c8cadb5deed6320c3173bdcbdf9ca7c8ff39c59e7b70a94510a38  crates/shared/mde-egui/src/drm.rs
693c76c5ff4e2d39223d000408eea011dbecfb5e522b0b8b3053fb2eb9b412f4  crates/desktop/mde-shell-egui/src/communications/mod.rs
c192ba954265276d2ebcf086d170941cea2668a1139ee3210a0f4d33ec29c9fa  crates/desktop/mde-shell-egui/src/main.rs
```

## Remaining acceptance gap

This is a farm-proven local authority slice, not live direct-seat or five-seat
proof. Rich mesh payload/CAS integration, VDI guest transport, permission UX,
package policy, live DRM focus/lock behavior, and cleanup evidence remain.
FUNC-016 remains `Remaining`.
