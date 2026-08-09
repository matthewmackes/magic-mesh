# WL-FUNC-011 legacy route admission hard cut (r2)

Date: 2026-08-09

The shell's central `shell/goto/*` resolver no longer aliases the retired Chat,
Voice, Editor, Code, Teams, or Mesh Teams product routes to the native
Collaboration surface. Those stale publisher contracts now fail closed, while
the explicit `collaboration` / `collab` names and the current
`communications` / `comms` compatibility names resolve to the one native
surface. Notifications and clipboard remain content-mode aliases rather than
independent product routes.

Success-critical coverage asserts every retired route is rejected and the
canonical Collaboration route is admitted. Verification ran on BigBoy
`172.20.0.130` in slot `func011-legacy-route-hardcut-r2-20260809`:

- complete `toast_bridge::tests` suite: 25 passed, 0 failed
- focused hostile route-admission test: 1 passed, 0 failed
- focused canonical-name admission test: 1 passed, 0 failed
- changed-file `rustfmt --check`: passed
- `git diff --check`: passed
