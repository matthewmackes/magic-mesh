# WL-FUNC-022 — Clock UI farm gate

- Date: 2026-08-14
- Revision: `5b682e38`
- Farm: BigBoy `172.20.0.130`, slot `clock-ui-audit`
- Command: `cargo test -p mde-shell-egui --bin mde-shell-egui timers`
- Result: 15 passed, 0 failed, 1,606 filtered

The targeted Clock surface gate covers the explicit four-section model,
generation-bound signed actions, banner action routing, IANA/DST handling,
peer stopwatch/timer authority, lock-summary freshness, and the absence of
shell scheduling/store authority. The package emitted one unrelated existing
`mde-vdi-rdp` dead-code warning; no Clock test failed.

Installed package, physical audio, and release-seat evidence remain under
`WL-TEST-001` and do not create a two-seat requirement for WL-FUNC-022.
