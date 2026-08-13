# WL-CRIT-006 — canonical bootc digest receipt consumption (r522)

Date: 2026-08-13

The canonical `xcp-build.sh rpm` release path no longer accepts an unauthenticated
raw bootc digest. It requires a durable bootc digest receipt plus the expected
image reference, registry architecture, and unified release role. Before source
sync, vendoring, dependency installation, compilation, or RPM mutation, the
release-input preflight invokes the existing receipt inspector and binds those
values to the already-admitted source revision and commit epoch.

The raw `MCNF_BOOTC_BASE_DIGEST` interface has been removed from this path. The
canonical environment handoff is now:

- `MCNF_BOOTC_BASE_DIGEST_RECEIPT`
- `MCNF_BOOTC_BASE_IMAGE_REFERENCE`
- `MCNF_BOOTC_BASE_ARCHITECTURE`
- `MCNF_BOOTC_RELEASE_ROLE`

No other mandatory release input was weakened or removed, and the receipt
producer was not changed.

## Farm evidence

- `.50`, slot `crit006-bootc-consume-test-r522`:
  `./install-helpers/test-release-input-preflight.sh` passed. The positive case
  invoked the real `produce-bootc-digest-receipt.py inspect` implementation
  against an exact Git fixture and matched revision, epoch, architecture, role,
  and image reference. The hostile case substituted the architecture and was
  refused before the build marker. Missing inputs and owning-verifier refusal
  also remained fail-closed, and the static entry-point assertion proved that
  preflight precedes immutable source sync, vendoring, and build execution.
- `.50`, same slot after the behavioral gate completed:
  `shellcheck -e SC2016 install-helpers/release-input-preflight.sh install-helpers/test-release-input-preflight.sh install-helpers/xcp-build.sh`
  passed. `SC2016` excludes only two documented pre-existing intentional remote
  shell literals in untouched `xcp-build.sh` lines.
- `.170`, slot `crit006-bootc-consume-syntax-r522`:
  `bash -n install-helpers/release-input-preflight.sh install-helpers/test-release-input-preflight.sh install-helpers/xcp-build.sh`
  passed.
- Local `git diff --check` passed.

An initial `.90` ShellCheck dispatch was not claimed because that farm image
does not provide ShellCheck. It was rerouted to `.50`; no product result depends
on the unavailable-tool attempt.

## Remaining WL-CRIT-006 inputs

- Produce this receipt for the exact first-release revision, commit epoch,
  registry image reference, architecture, and unified release role.
- Supply the real UX-014 package assets, App VM catalog trust and immutable base,
  signed Cuttlefish declaration and exact guest artifacts, RPM signing identity
  receipt, and immutable App VM/Cuttlefish image digests.
- Run the first full release build, then verify generated RPM payloads,
  signatures, package headers, and artifact integrity.
- Perform the deferred, non-blocking post-release one-node installed acceptance,
  recovery, and corrected-forward proof.
