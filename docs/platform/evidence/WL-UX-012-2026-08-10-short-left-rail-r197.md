# WL-UX-012 — short Left-rail geometry admission (r197)

Date: 2026-08-10

## Selected gap

Left placement previously emitted its fixed Start/Search/Workloads/Back/Home
cluster without checking the available vertical display extent. A short
portrait or remote viewport could therefore receive hit rectangles below the
owned rail.

## Implementation

`crates/desktop/mde-shell-egui/src/nav_bar.rs` now admits the fixed Left-rail
controls one at a time through the existing `docked_control_fits` boundary.
Controls that cannot fit are omitted before catalog controls are considered;
the bounded catalog/More path remains the owner of lower-priority entries.

## Farm proof

Command:

```text
MCNF_BUILD_HOST=172.20.0.90 \
MCNF_BUILD_SLOT=ux012-short-left-rail-r197c \
install-helpers/xcp-build.sh cargo test -p mde-shell-egui --bin mde-shell-egui \
  nav_bar::tests::short_left_rail_admits_only_controls_inside_its_display_rect \
  -- --exact --nocapture
```

Result: `1 passed; 0 failed; 0 ignored; 0 measured; 1547 filtered out`.

The regression uses a 320×160 viewport with a connected session and maximum
chooser pins, asserting every admitted control is inside the rail and that no
two controls intersect.

## Live limits

No direct-DRM capture, physical-seat review, large-text review, multi-display
proof, or three-seat taskbar acceptance was run. Those remain required for the
UX-012 release gate.
