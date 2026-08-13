# WL-ARCH-009 — Fleet and This Node alias hard cut (r552)

Date: 2026-08-13

Commit under test: `9d1df3cb` (`preserve typed Workers routes across legacy aliases`)

## Production result

`Shell::normalize_surface_aliases` no longer gives the retired
`Surface::FleetMesh` or `Surface::ThisNode` variants authority to select a
Workers tab or destination. Both aliases now do exactly one thing: collapse the
surface to `Surface::Workers`. The exact typed destination and corresponding tab
selected by `open_workers_destination` survive unchanged.

The hostile regression covers four substitutions:

- typed Fleet followed by stale Fleet & Mesh;
- typed This Node / Storage followed by stale This Node;
- typed Action Console followed by stale Fleet & Mesh; and
- typed Action Console followed by stale This Node.

This removes the reachable legacy overview activation. It does not add a
compatibility route or a second navigation authority.

## Farm evidence

All authoritative Rust gates used an isolated detached worktree at
`9d1df3cb`, because unrelated Front Door, VDI, Music, Files, and Worker edits
were active in the shared worktree.

- BigBoy `172.20.0.130`, slot `arch009-fleet-thisnode-test`:
  `cargo test -p mde-shell-egui retired_fleet_and_this_node_surfaces_cannot_override_typed_workers_destinations -- --nocapture`
  discovered and passed exactly 1 test (`1 passed; 1619 filtered out`).
- `.90`, slot `arch009-fleet-thisnode-build`:
  `cargo build -p mde-shell-egui --all-targets --all-features` passed in
  4 minutes 30 seconds.
- `.196`, slot `arch009-fleet-thisnode-clippy`:
  strict all-target/all-feature Clippy reached `mde-shell-egui` and stopped at
  the pre-existing `communications/mod.rs:608` `while_let_loop` warning under
  `-D warnings`. It reported no finding in this slice.
- Farm Rustfmt reported only pre-existing/unrelated drift in Front Door and two
  older `main.rs` test/import regions; no changed hunk from this slice appeared.
- Owned-scope `git diff --check` passed before commit.

The first attempted shared-tree gate was discarded after it correctly exposed a
concurrent Front Door source-revision mismatch. No claim relies on that run.

## Remaining ARCH-009 acceptance

- Cut any remaining reachable Fleet, This Node, Network Operations, or other
  sibling-surface activations into exact typed Workers leaves.
- Finish production Bus consumption and authenticated provider ownership for
  generation-bound Worker change sets; Commit remains fail-closed until then.
- Complete the provider/action ownership inventory and remaining worker-role
  gate debt.
- Package the first release, then perform the deferred non-blocking one-node
  installed-state, responsive, crash-isolation, and lifecycle acceptance.
