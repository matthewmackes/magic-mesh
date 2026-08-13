# WL-ARCH-008 Browser profile attachment authority — r535

Date: 2026-08-13

## Production change

The shell's Browser RDP alternate now admits an attachment only when both
authorities identify the hard-cut Browser VM:

- the Browser surface advertises the exact `browser-vm` workload as reachable
  and `active`/`running`; and
- the validated Workloads projection reports the exact
  `browser-vm-chromium` image with the governed Dell-safe 3-vCPU, 8192-MiB,
  64-GiB profile, libvirt backend, running power, ready application state, and
  an unexpired generation-matched RDP lease.

A ready libvirt row with a substituted App VM image, altered resources, or an
unreachable Browser target can no longer acquire Browser presentation/input
authority. The adapter still launches no host Browser process and gains no VM
lifecycle authority.

## Files

- `crates/desktop/mde-shell-egui/src/vdi/browser_transport.rs`
- `crates/desktop/mde-shell-egui/src/vdi/tests.rs`

## Farm gates

- `172.20.0.170`, slot 2:
  `cargo test -p mde-shell-egui browser_rdp_alternate -- --nocapture`
  passed 2/2 in 6m37s.
- BigBoy `172.20.0.130`, slot 1:
  `cargo clippy -p mde-shell-egui --all-targets --features live-vdi -- -D warnings`
  passed in 6m08s.
- `172.20.0.196`, slot 1:
  `cargo fmt -p mde-shell-egui -- --check` passed.
- Local scoped `git diff --check` passed; no local build or test ran.

The initially scheduled `.90` focused test was canceled while starved behind
unrelated cold builds and rerouted to `.170`. The initially scheduled `.90`
Clippy gate was canceled before execution and rerouted to BigBoy when capacity
recovered. These canceled attempts are not acceptance evidence.

## Remaining WL-ARCH-008 acceptance

- Preserve and bind the standalone old-Browser repository's immutable revision
  and clean-clone build evidence.
- Complete two-pass portable profile migration, including downloads and the
  explicit secret-exclusion contract.
- Keep the negative host-engine/package reachability scan green in a fresh
  checkout.
- Produce the reproducible Browser guest image/profile and readiness artifact
  from release inputs.
- Finish Browser VDI focused-input, audio, clipboard, reconnect, and explicit
  transport-preference behavior without guest-chrome mirroring.
- After the first full release, run the deferred non-blocking five-tab,
  frame/damage, latency, guest-audio, package cleanup, corrected-forward
  recovery, and selected-seat live acceptance.
