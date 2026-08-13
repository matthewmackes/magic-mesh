# WL-CRIT-007 S2 final convergence attestation — 2026-08-13

## Production correction

`install-helpers/mesh-peer-recovery.sh` previously treated successful XDG bind
repair or shell activation as the final recovery result. Those operations run
as external systemd transactions, so Nebula, configured etcd/Syncthing, or a
grouped worker could disappear during the desktop phase while the helper still
published `recovered` or `already-recovered` from stale pre-desktop readiness.

The recovery publication boundary now re-attests the physical network, Nebula
identity, configured substrate, all six grouped workers, and the Workstation
shell. An offline transition remains a clean deferred retry, while loss of an
authenticated service on an online link fails the recovery so systemd can
retry. Neither path can retain a false corrected-forward success publication.

## Hostile fixture

The dedicated recovery fixture now removes active etcd authority during XDG
repair. It proves that only the XDG mutation occurred, the helper reports
`substrate-lost-after-desktop`, exits unsuccessfully, and emits neither success
state.

## Farm gates

- `.90`, slot `crit007-final-convergence`: complete
  `sudo bash install-helpers/test-mesh-peer-recovery.sh` fixture passed,
  including the new post-XDG coordination-loss case and every existing
  offline/online, ordering, retry, duplicate-session, and trigger case.
- `.170`, slot `crit007-shellcheck`: `bash -n` passed for the production helper
  and dedicated fixture. ShellCheck was not installed on that farm VM and was
  therefore explicitly reported unavailable rather than claimed.
- Local orchestration-only checks: `git diff --check` and the same two-file
  `bash -n` check passed.

## Remaining acceptance

Per the release sequencing decision, direct one-node boot, resume, and network
return traces are deferred until after the first full release and are
non-blocking for pre-release coding. Those traces must still demonstrate one
authenticated peer, one shell/session, synchronized substrate, and truthful
recovery state from the installed release.
