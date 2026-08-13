# WL-FUNC-017 — Maps weather native-root runtime requirement (r520)

Date: 2026-08-13

Branch: `agent/drain-worklist-20260725`

## Gap closed

The daemon-owned NWS/weather HTTPS client loads Fedora's native certificate
roots through `rustls-native-certs`. Source and farm images supplied that trust
store ambiently, but the base RPM did not require it. A minimal first install
could therefore ship a working binary whose trusted weather requests failed.

The base RPM now directly hard-requires `ca-certificates`. The payload verifier
reads only direct keys from `[package.metadata.generate-rpm.requires]`, rejects
a missing dependency in its hostile fixture, and cannot be satisfied by a weak
recommendation or a similarly named package.

## Farm evidence

- BigBoy `.130`, slot `firstrel-rpm-full-selftest-r520b`:
  `bash install-helpers/verify-rpm-payload.sh --self-test` passed every hostile
  assertion, including rejection of absent Maps trust roots.
- `.170`, slot `func017-ca-metadata-r520b`:
  `bash install-helpers/verify-rpm-payload.sh requirements` passed and reported
  `maps hard-requires ca-certificates`. In the same isolated workspace,
  `cargo metadata --no-deps --format-version 1` was parsed and confirmed the
  `mackesd` package metadata contains the exact direct requirement
  `ca-certificates = "*"`.
- `.50`, slot `rpm-verifier-shellcheck-r520b`:
  `shellcheck -e SC2016,SC2053,SC2254,SC2015
  install-helpers/verify-rpm-payload.sh` passed. The exclusions are established
  findings in untouched verifier lines; no owned finding was excluded.
- Local `bash -n` and scoped `git diff --check` passed.

## Remaining epic acceptance

The first full release must verify `ca-certificates` in the built base RPM's
actual Requires header and package the Maps/weather surface. Installed one-seat
offline map/route, provider-loss, restart, sleep/rejoin, MG90, weather, and
visual proof remains deferred and non-blocking until after that release.
