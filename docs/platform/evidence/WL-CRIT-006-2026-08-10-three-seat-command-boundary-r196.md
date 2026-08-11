# WL-CRIT-006 required three-seat command boundary — 2026-08-10

## Outcome

The required seat matrix is now closed against both forms of optional-seat
inspection argument. A gate command containing `--inspect-seat eagle` or
`--inspect-seat=eagle` is refused, so optional Eagle/T480 inspection cannot be
promoted into the required Dell/seat-15/Surface baseline by command-line
spelling changes.

## Farm proof

The focused command ran on build VM `172.20.0.90`, slot
`crit006-seat-command-boundary-r196`:

```text
MCNF_BUILD_HOST=172.20.0.90
MCNF_BUILD_SLOT=crit006-seat-command-boundary-r196
install-helpers/xcp-build.sh sync
python3 install-helpers/verify-release-gate-matrix.py --self-test
install-helpers/release-evidence.sh --self-test
CRIT006_FARM_RESULT:0
```

Result: shell syntax passed; the matrix verifier accepted one canonical matrix
and rejected 17 hostile fixtures, including the equals-form optional-seat
injection; the release-evidence deterministic binding/validation self-test
also passed.

## Live-proof limits

No physical seat, lighthouse, signed artifact, GitHub required-check run, or
production deployment was performed here. WL-CRIT-006 remains `Remaining`
until the complete signed evidence bundle and live acceptance matrix exist.
