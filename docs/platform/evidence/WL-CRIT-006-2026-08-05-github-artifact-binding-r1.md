# WL-CRIT-006 — GitHub/farm final-artifact binding (2026-08-05)

The authoritative `farm-gate` workflow now cuts final RPM bytes only after the
exact farm result is green. It binds those files to the candidate revision,
GitHub run/attempt job identity, BigBoy host, and run-scoped farm slot through
the existing deterministic `write-binding` and authenticated `bind-release`
contract. The binding, status, authenticated log, manifest, and exact RPM copies
cross the GitHub artifact boundary together.

The separate `github-required` job reconstructs the bound absolute artifact
paths in its own run-scoped temporary directory and fails closed on a missing,
extra, substituted, renamed, symlinked, resized, or digest-changed artifact. It
also re-verifies the revision/job/host/slot identity and the binding digest
inside the authenticated farm log. The workflow has top-level
`contents: read` permission and does not publish a release or mutate repository
contents.

## Verification

The dedicated verifier was checked on farm node `172.20.0.90`, slot
`wl-crit006-github-binding-r1`:

```text
bash -n install-helpers/verify-github-release-binding.sh
install-helpers/verify-github-release-binding.sh --self-test
install-helpers/verify-github-release-binding.sh check-workflow .github/workflows/ci.yml
```

All commands exited zero; hostile artifact, identity, set, symlink, path, and
unbound-log fixtures were rejected. A separate local read-only parse confirmed
that `ci.yml` remains valid YAML.

No release was published and GitHub was not contacted. The first real
`mcnf-farm` GitHub run still must prove the full RPM upload/download round trip.
Production signing, live topology/recovery evidence, and fleet promotion remain
separate gates.
