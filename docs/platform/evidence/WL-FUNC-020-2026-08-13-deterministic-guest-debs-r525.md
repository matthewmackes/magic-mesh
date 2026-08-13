# WL-FUNC-020 deterministic guest DEBs — r525

Date: 2026-08-13

## Result

The release packaging surface now produces two deterministic Debian artifacts
from the already-admitted Cuttlefish guest runtime stage:

- `mcnf-cuttlefish-vdi-agent.deb`
- `mcnf-cuttlefish-readiness-relay.deb`

Each package binds the full source revision and build identity in Debian control
metadata, installs exactly one stage-identical binary and one systemd unit, and
declares its runtime dependencies. The relay package requires the exact agent
package version. Archive payloads are root-owned with binary mode `0555` and
unit mode `0444`. A canonical JSON package manifest binds the complete ordered
DEB set by name, size, and SHA-256.

The builder uses the admitted stage as its sole binary input, derives its epoch
from the source commit, normalizes payload mtimes, and uses fixed compression.
It neither builds a Cuttlefish image nor signs or publishes a release
declaration.

## Farm evidence

- `.130`, slot `func020-guest-deb-build-r525`:
  `packaging/android/test-guest-debs.sh` built the real release relay/agent,
  produced the DEB set twice, proved every output byte-identical, verified exact
  control metadata, dependencies, root ownership/modes, payload allowlist, and
  stage binary identity, then rejected substituted package bytes and a stale
  source identity. Passed.
- `.50`, slot `func020-guest-deb-shellcheck-r525`:
  strict ShellCheck over builder, verifier, and hostile test. Passed with no
  findings.
- `.170`, slot `func020-guest-deb-units-r525`:
  `systemd-analyze verify` over both units with staged executable/config paths.
  Passed.
- Local `bash -n` and `git diff --check`: passed.

The initial `.130` attempt identified that `dpkg-deb` was absent from the Fedora
build VM. The farm dependency was installed and the exact gate rerun; no result
from the failed attempt is claimed.

## Remaining FUNC-020 inputs

- Publish the real immutable Cuttlefish image and produce its governed receipt.
- Build this package set for the exact first-release revision.
- Produce the signed schema-v3 declaration over these exact DEBs, relay/agent
  bytes, and admitted image identity.
- Include and verify the exact artifacts in the first full release.
- Run the deferred, non-blocking post-release one-node nested-KVM, readiness,
  VDI, launch, restart, provider-loss, and visual acceptance.
