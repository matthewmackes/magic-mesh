# WL-CRIT-006 release-binding parent transaction — 2026-08-13 r506

## Implemented invariant

`install-helpers/release-evidence.sh write-binding` now pins the admitted output
directory by inode for the complete revision/artifact/farm-binding transaction.
The staged descriptor is created and cleaned through that open directory, the
directory identity and output type are rechecked immediately before publication,
and final rename uses exact-target semantics. A pathname replacement therefore
cannot redirect a verified binding into a different candidate directory.

Failure cleanup for both `write-binding` and `write` now uses an `EXIT` trap.
The former `RETURN` traps did not run when `die` terminated the helper, allowing
private `.release-binding.*` or `.release-evidence.*` files to survive a failed
transaction.

## Hostile regression

The self-test replaces the binding output directory after the private descriptor
is complete but before publication. The command must fail, publish no descriptor
through the replacement path, and remove all owned staged files through the
pinned directory descriptor.

## Verification

- Local: `bash -n install-helpers/release-evidence.sh` — passed.
- Local: `git diff --check -- install-helpers/release-evidence.sh` — passed.
- Farm `.90`, slot `crit006-binding-parent`: syntax passed and the hostile
  parent-replacement/cleanup assertion passed; execution advanced through the
  later hostile descriptor, evidence replacement, topology, and production
  fixtures.
- The full helper self-test then stopped at an unrelated pre-existing canonical
  matrix drift: `gates[2].command must validate the revision-bound Workloads RPM
  transaction evidence`. The package/gate manifest is outside this slice's
  authorized scope and was not edited.

No release build, signing operation, promotion, or acceptance run was performed.
