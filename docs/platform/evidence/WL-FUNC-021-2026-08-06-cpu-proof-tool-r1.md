# WL-FUNC-021 — post-install CPU proof helper (2026-08-06)

## Scope

`install-helpers/verify-music-cpu-proof.sh` is a bounded, read-only acceptance
helper for the common-seat CPU-spike investigation. It refuses to sample when
the installed `magic-mesh` RPM does not exactly match the checked-out platform
version, RPM release, and `x86_64` architecture. During an eligible run it
requires an active `mackesd.service`, stable `NRestarts`, and samples
`/proc/<MainPID>/stat` against `/proc/stat` over a bounded interval. CPU is
reported as permille of one host CPU, with explicit maximum and mean limits.

The helper does not install packages, restart services, interrupt providers,
change playback, or mutate a seat. It is intended to be run after the
operator-authorized release-5 installation on each canonical seat; an old
package must not be used to claim source-level CPU improvement.

## Verification

- `bash -n install-helpers/verify-music-cpu-proof.sh` — passed.
- `install-helpers/verify-music-cpu-proof.sh --self-test` — passed; no SSH was
  attempted.
- `shellcheck install-helpers/verify-music-cpu-proof.sh` — passed.
- Before installation, the helper correctly refused the Dell seat because it
  was `magic-mesh-12.1.6-4.x86_64` while the checked-out artifact was release 5.
- After the operator-authorized F44 installation and an explicit
  `systemctl restart mackesd.service` to load the new daemon, the Dell run
  passed with 15 samples over 30 seconds: maximum `437‰`, mean `218‰`, and
  stable restarts `0→0`. The earlier pre-restart sample was intentionally not
  retained as a release-5 result: it measured the 10-hour-old release-4
  process and recorded maximum `1149‰`, mean `1092‰`.

## Remaining acceptance

The second canonical seat still requires the same release-5 installation and
bounded proof. This evidence does not claim provider-loss recovery, physical
renderer proof, or live two-seat owner-yield/resume.
