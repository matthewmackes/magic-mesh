# WL-CRIT-006 remote provenance execution — 2026-08-10

## Outcome

The first native Fedora 44 release-30 cut from revision `f1c77546` was stopped
and rejected before packaging. The builder printed `env: ‘export’: No such file
or directory`: `xcp-build.sh` had placed a recipe beginning with the shell
builtin `export` directly after `env`. The following semicolon then allowed the
workspace build to continue without the immutable revision variables.

No RPM from that cut was accepted or deployed. The orphaned builder process
group was terminated after resolving its exact PGID (`1680`).

`remote()` now passes every complete recipe as one percent-quoted `bash -lc`
program after the bounded environment assignments. The immutable revision,
promotable marker, and source epoch therefore remain shell exports in the same
program that performs Cargo and RPM generation; `env` can no longer interpret
`export` as an executable.

## Focused verification

- Local syntax check: `bash -n install-helpers/xcp-build.sh`.
- Local `xcp-build.sh --route-test`: all routing, Cargo argument, and nested
  export-execution assertions passed.
- Local `xcp-build.sh --rpm-target-test`: passed.
- Farm host `.170`, slot `provenance-remote-wrapper-r1`: the same syntax,
  route, nested export-execution, and Fedora-target assertions passed.
- F44 builder `.131` was checked after termination; no Cargo, rustc, or
  generate-rpm process from the rejected cut remained.

## Remaining release work

A new native Fedora 44 candidate must be cut from the committed revision that
contains this correction. Package, signature, seat, lighthouse, recovery, and
production-promotion evidence remain mandatory and are not claimed here.
