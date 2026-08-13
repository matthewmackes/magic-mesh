# WL-ARCH-008 — Browser RDP Workloads attachment authority (r521)

Date: 2026-08-13

## Result

The production Browser surface now consumes the documented RDP alternate when
the authoritative Workloads projection publishes a ready, unexpired
`WorkloadAttachmentProtocol::Rdp` lease for `browser-vm`. Sunshine remains
honestly unavailable because this tree has no seat-side Moonlight decoder.

The Browser adapter binds the exact Workloads request id, workload id,
generation, lease id, protocol, expiry, serving node, and local client mesh
identity before handing the existing IronRDP transport a bounded mesh endpoint.
It never turns the one-use lease nonce into a plaintext guest password. Existing
VDI reconnect remains bounded to the same retained request. A changed, missing,
malformed, expired, wrong-protocol, or generation-substituted projection revokes
the installed transport and its frame/input authority before any replacement is
admitted. The adapter publishes no lifecycle operation, runs no host command,
and owns no VM or Browser catalog state.

## Owned files

- `crates/desktop/mde-shell-egui/src/vdi/browser_transport.rs`
- `crates/desktop/mde-shell-egui/src/vdi/mod.rs`
- `crates/desktop/mde-shell-egui/src/vdi/tests.rs`
- `crates/desktop/mde-shell-egui/src/web/mod.rs`
- `crates/desktop/mde-shell-egui/src/main.rs`

## Farm evidence

- BigBoy `.130`, slot `arch008-browser-rdp-test-r521d`:
  `cargo test -p mde-shell-egui browser_rdp_alternate_requires_exact_live_workloads_lease_and_revokes_on_replacement -- --nocapture`
  passed 1/1 with 1,598 filtered tests.
- `.170`, slot `arch008-browser-rdp-clippy-r521d`:
  `cargo clippy -p mde-shell-egui --bin mde-shell-egui -- -D warnings`
  passed.
- `.196`, slot `arch008-browser-rdp-fmt-r521e`:
  `cargo fmt -p mde-shell-egui -- --check` passed.
- Local `git diff --check` passed.

No live Browser VM, RDP server, Sunshine server, Moonlight client, or release
artifact was claimed or exercised.

## Remaining acceptance

- The first release must produce and package the immutable Browser VM image and
  Lighthouse RPM, and verify the image manifest/provenance.
- The first release integration must publish the real ready RDP attachment lease
  from the Workloads authority for the selected Browser session.
- Post-release, non-blocking one-node proof remains for live Browser attachment,
  reconnect/revocation, audio, migration, performance, and visual behavior.
- A native Sunshine path remains optional until an actual bounded Moonlight
  decoder is selected and shipped; it is not represented as available today.
