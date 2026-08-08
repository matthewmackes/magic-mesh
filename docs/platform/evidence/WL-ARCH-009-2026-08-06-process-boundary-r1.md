# WL-ARCH-009 process-boundary validator — 2026-08-06

## Boundary implemented

`install-helpers/verify-mackesd-process-boundary.py` validates the intended
ARCH-009 packaging contract without creating fake runtime services. A passing
fixture requires `mackesd.target`, six group units, group-specific
`/usr/bin/mackesd serve --group ...` entrypoints, and exactly one
`--sqlite-writer` on `mackesd-data.service`. A monolithic launcher is an
explicit failure.

## Verification

- Local self-test and `py_compile`: passed.
- Big farm slot `.50`, `mackesd-process-boundary-20260806-r1`: fixture
  self-test and syntax probe passed.
- Source validation against this checkout: intentionally failed with
  actionable diagnostics: the six group units and target are absent and the
  monolithic `mackesd.service` still launches `mackesd serve`. This is honest
  remaining-gap evidence, not a claim of process isolation.
- Source SHA-256:
  `53694a383a96322edbe2c8649d82b8674177133da60cda4a69fd87680ec4ee5c`.

## Remaining gap

ARCH-009 S4 still requires the real six-process runtime split, shutdown/retry
behavior, cgroup/resource tests, and package proof. No Dell runtime change was
made.
