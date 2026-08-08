# Work pause handoff — 2026-08-05

Work paused at `2026-08-05T10:16:17-04:00` on branch
`agent/drain-worklist-20260725` at unchanged HEAD `e52322ec048c`. The worktree
remains intentionally dirty with the operator's accumulated changes, including
the pre-existing deletion of `crates/desktop/mde-vdi-rdp/src/connect.rs`. No
commit, push, deployment, cleanup, reboot, or seat operation was performed at
the pause boundary.

## Farm and agents

All four farm VMs were reachable and toolchain-ready. Read-only process census
reported zero `cargo`, `rustc`, `rpmbuild`, or `xcp-build` processes on
`172.20.0.50`, `.90`, `.130`, and `.170`. All three delegated agents reached a
clean break and were closed.

## Completed slices

- WL-ARCH-009: the canonical registry now covers all 145 production starts and
  removes the non-tiered/dynamic exception lists. The exact drift test passed on
  `.50`: 1 passed, 0 failed, 4,481 filtered.
- WL-CRIT-006: GitHub farm-artifact binding and its verifier passed shell,
  self-test, and workflow checks on `.90`. Production release signing now has a
  fail-closed ResourcePublisherAttestation HMAC consumer, strict credential
  handling, revision equality, and unchanged-evidence checks; only local syntax,
  import/help, and whitespace checks were completed for that follow-up.
- WL-UX-013: host power, maintenance, and network-transition callers publish
  admitted availability state through the durable sink. The focused
  `node_availability` farm suite passed on `.130`: 19 passed, 0 failed, 4,473
  filtered.
- WL-CRIT-007: authenticated overlay-claim snapshot and collision-guard assets
  are packaged in the base/server/lighthouse manifests, with a dedicated `0700`
  state directory. The producer and Nebula drop-in remain deliberately inert.
- The canonical worklist lint, documentation-supersession lint, and scoped
  whitespace check all pass at the pause boundary.

## Resume without repeating work

1. Run the new production HMAC signing hostile self-test on a farm slot; then
   provision an existing publisher HMAC credential through a private explicit
   file or `CREDENTIALS_DIRECTORY`. Do not invent a key.
2. Run the focused `host_state` and `netstate_apply` caller tests, followed by
   live logind/NetworkManager and hardware transition proof.
3. Run the BigBoy package/manifest gate for the overlay-identity assets. Keep
   activation blocked until authenticated current-boot authority exists before
   Nebula startup (`pre-nebula-current-authority-transport-unavailable`).
4. Run the broader ARCH-009 worker-role suite after the integrated tree is free
   of unrelated compile failures; do not repeat the already-passing exact drift
   test as a filler retest.
5. No seat currently has a deployable artifact from these paused slices. Keep
   seats idle until a uniquely identified build and live acceptance target are
   ready.

The active goal and all 15 worklist epics remain open; pause is an operator
control state, not completion or a worklist blocker.
