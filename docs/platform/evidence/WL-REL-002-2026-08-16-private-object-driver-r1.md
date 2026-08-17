# WL-REL-002 private-object driver evidence — 2026-08-16

- Source revision: `2d3853db` plus the working-tree release-driver change
- Command: `install-helpers/test-run-first-full-release.sh`
- Result: `PASS`

The hostile phase-boundary suite now covers both accepted prepare inputs: the
existing mode-0400 derived argv array and a mode-0400 private preflight object
converted by `release-input-argv.py` into the driver-owned argv file. Both
paths produced promotion-forbidden unsigned handoffs; the existing refusal and
verified seven-role resume checks also passed.
