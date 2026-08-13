# WL-FUNC-021 — Music fresh-install activation authority (r520)

Date: 2026-08-13

Branch: `agent/drain-worklist-20260725`

## Gap closed

The RPM enabled the Music credential materializer and separately attempted to
start the seat-owned `mde-musicd`, but it did not establish an ordering edge.
On a fresh image the selected seat user's systemd manager could also be absent.
Because `mde-musicd` loads its public authorization key once at startup, that
race left every Music mutation disabled until a later restart.

The RPM lifecycle now invokes the packaged materializer directly and observes
its result, requires a non-empty public key, enables lingering, starts the
selected seat user's manager, and requires its Bus socket before granting
activation authority. If any prerequisite is absent, the selected daemon is
disabled instead of starting without authorization. The Music package contract
also verifies the daemon, materializer, credential configuration, system unit,
and user unit are all shipped together and checks their lifecycle ordering.

## Farm evidence

- BigBoy `.130`, slot `firstrel-rpm-full-selftest-r520b`:
  `bash install-helpers/verify-rpm-payload.sh --self-test` passed every hostile
  assertion, including daemon-before-credential rejection.
- `.90`, slot `func021-activation-hostile-r520b`: a temporary manifest with the
  direct credential-materializer invocation removed was rejected by
  `music-package` with `Music activation token missing`; the focused hostile
  gate passed.
- `.50`, slot `rpm-verifier-shellcheck-r520b`:
  `shellcheck -e SC2016,SC2053,SC2254,SC2015
  install-helpers/verify-rpm-payload.sh` passed. The exclusions are established
  findings in untouched verifier lines; no owned finding was excluded.
- Local `bash -n` and scoped `git diff --check` passed.

## Remaining epic acceptance

The first full release must verify the five Music assets in the built base RPM
and exercise the lifecycle against the release image. Installed one-seat proof
for authorized actions, offline playback, provider switching/loss, restart,
and audible continuity remains deferred and non-blocking until after release.
