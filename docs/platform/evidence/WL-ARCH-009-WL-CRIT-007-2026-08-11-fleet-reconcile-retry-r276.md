# Fleet reconcile corrected-forward retry evidence — 2026-08-11

- Scope: the fleet reconcile worker records its 15-minute convergence cadence
  only after `magic-fleet reconcile` exits successfully.
- Recovery behavior: command absence and nonzero exits remain immediately due,
  so restart and peer-return failures retry on the next bounded worker poll
  instead of suppressing correction for a full cadence.
- Hostile regression: an injected `/usr/bin/false` attempt remains due, while a
  successful attempt advances the success timestamp.
- Farm gate: `.50`, slot 2: **1 passed, 0 failed, 4,822 filtered**.
- Scoped `git diff --check`: passed.
