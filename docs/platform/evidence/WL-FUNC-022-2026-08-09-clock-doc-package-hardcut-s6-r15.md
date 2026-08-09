# WL-FUNC-022 S6 — Clock documentation and package hard cut (r15)

Date: 2026-08-09

Base migration commit: `b317acfa` (`migrate clock display zones to iana`)

Farm host: machine 9 (`172.20.0.50`)

Farm slot: `func022-clock-doc-hardcut-r15`

## Result

- `docs/design/construct-clock.md` is the authoritative Clock-specific design:
  the visible clock opens `Surface::Clock`, the bell opens Notification Center,
  and `mackesd` alone owns schedules, deadlines, ringing, recovery, and
  replicated stopwatch state.
- `docs/design/platform-interfaces.md` carries the same unambiguous Construct
  route and authority contract.
- The retired Timers surface and shell scheduling/store authority have no
  compatibility route. `timers-alarms.json` is explicitly never imported and
  remains untouched for manual rollback.
- Clock has no separate executable, launcher, service, or data payload. The
  workstation package carries Clock inside `mde-shell-egui` and scheduling
  inside `mackesd`.
- `install-helpers/lint-clock-cutover.sh` rejects a planted `Surface::Timers`,
  shell `AlarmStore`, stale clock-to-Notification-Center prose, retired package
  manifest asset, and retired installed RPM payload. It also requires the live
  `Surface::Clock` and presentation-only shell boundary.
- The maintained CI policy suite and `verify-rpm-payload.sh payload [RPM]` invoke
  the gate, so both source changes and built package file lists fail closed.

## Focused verification

Only the requested static/package checks ran; no Cargo suite or broad gate ran.

| Check | Result |
|---|---|
| `bash -n` on the new lint and its two integration scripts | PASS |
| `lint-clock-cutover.sh --self-test` | PASS |
| `lint-clock-cutover.sh` against the synced current tree | PASS |
| Machine-9 trailing-whitespace/conflict-marker scan over changed lane files | PASS |
| Repository-aware local `git diff --check` | PASS |

The literal Git diff command cannot run in the farm slot because
`xcp-build.sh sync` intentionally excludes `.git`; the machine-9 content check
covers the same whitespace/conflict classes, while the repository-aware command
ran in the orchestrator checkout.

## Remaining WL-FUNC-022 scope

This lane does not claim package construction, installed-payload inspection of
a newly cut RPM, deployment, or live-fleet Clock proof. Those remain required
before the epic can close.
