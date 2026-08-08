# WL-CRIT-007 — rejoin identity teardown guard

Status: helper safety checkpoint complete; live rejoin and runtime convergence
remain `Remaining`.

## Change

`install-helpers/rejoin-v11-mesh.sh` now:

- allows only the supported `lighthouse` and `workstation` role values;
- requires `mackesd leave --yes` to complete before joining;
- refuses to join if the old Nebula certificate, key, role pin, or broken
  identity symlink remains; and
- provides a hermetic `--self-test` for empty, stale, broken-symlink, stale
  role, and hostile-role cases.

This keeps the helper as a verifier of the daemon-owned teardown boundary; it
does not remove identity files itself.

## Verification

Local `bash -n`, `--self-test`, invalid-role, and `git diff --check` checks
passed. Farm `.50` completed the self-test in slot
`crit007-rejoin-selftest-20260806-r1`.

The systemd verification from the source checkout remained inconclusive because
installed service executables are absent and existing documentation-path
warnings are present. No destructive live rejoin was performed.

## Source hash at capture

```text
89d651ca2fe039eaafed9ae07a1ed3386aedb84387342a42b0fefd0f28bc00e3  install-helpers/rejoin-v11-mesh.sh
```
