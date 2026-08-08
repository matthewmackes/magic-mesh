# WL-ARCH-009 Workers, Phones, and Eastern clock checkpoint

Date: 2026-08-07
Scope: navigation cutover and timezone correction only. This checkpoint does
not close WL-ARCH-009; the six supervised worker groups and the remaining
typed Workers action surfaces are still outstanding.

## Implementation

- `Surface::Workers` is the canonical owner for node operations, network state,
  discovery, and local-node resources.
- Fleet & Mesh, Workbench, Mesh View, Explorer, This Node, System, Storage,
  About, and Phones deep links normalize into Workers compatibility routes.
- Phones renders as the Workers → Phones subtab and is absent from the launcher,
  taskbar pin catalog, and canonical launch surface inventory.
- Construct clocks now apply the configured U.S. Eastern daylight-saving offset
  to current and retained timestamps. The August 2026 Eastern offset is UTC−4;
  January timestamps remain UTC−5.

## Farm evidence

All gates used the build farm through `install-helpers/xcp-build.sh`:

- BigBoy `172.20.0.130`, slot `workers-unify-check-r1`: 1,453 shell tests
  passed; five unrelated baseline tests failed (Car Home pixel proof, three
  IaC tests, and switcher pixel proof).
- BigBoy `172.20.0.130`, slot `workers-unify-surfaces-r1`: surface taxonomy
  gate passed, 1/1.
- `172.20.0.90`, slot `workers-unify-storage-r1`: storage menubar coverage
  passed, 3/3.
- `172.20.0.50`, slot `workers-unify-toast-r1`: toast route reachability
  passed, 1/1.
- BigBoy `172.20.0.130`, slot `workers-unify-check-r1`: Front Door route group
  passed, 100/100; Eastern DST rule test passed, 1/1.

## Package and seat deployment

- BigBoy `172.20.0.130`, slot `workers-unify-rpm-r1`: the Fedora 44 container
  lane built and payload-checked `magic-mesh-12.1.6-5.x86_64.rpm` (83.5 MiB).
- Artifact SHA-256: `7ad3561f105c5e7f26440e8e7fc0828659db7b9b18bc3e23f71e06ac48d4aad8`.
  RPM requirements include Fedora 44 FFmpeg sonames `libavcodec.so.62`,
  `libavformat.so.62`, and `libavutil.so.60`.
- The artifact checksum matched on Dell `172.20.146.225` and seat 15
  `172.20.0.15`; separate RPM transaction dry-runs passed, and the same-NVR
  replacement was installed on both seats.
- `rpm -V magic-mesh`, `mackesd`, and `mde-shell-egui` were clean/active on
  both seats after the shell restart. The post-restart seat probes reported
  `2026-08-07 10:32 EDT -0400`. No screenshot capture was available in this
  deployment pass; the UI clock proof is the farm DST test plus the installed
  binary/service provenance.

## Source anchors

- `crates/desktop/mde-shell-egui/src/surfaces.rs`
- `crates/desktop/mde-shell-egui/src/nav_bar.rs`
- `crates/desktop/mde-shell-egui/src/main.rs`
- `crates/desktop/mde-shell-egui/src/timers.rs`
- `crates/desktop/mde-shell-egui/src/chat/render.rs`
- `docs/design/platform-interfaces.md`
