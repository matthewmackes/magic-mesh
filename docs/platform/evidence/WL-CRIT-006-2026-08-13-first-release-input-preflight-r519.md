# WL-CRIT-006 — first-release input preflight (r519)

Date: 2026-08-13

The canonical native RPM entry point now admits all mandatory first-release
inputs before immutable-source sync or any build mutation. The preflight calls
the owning Kiron, App VM catalog-trust, Cuttlefish signed-payload, and source
receipt verifiers. It additionally requires the unique RPM secret-key identity
and non-null immutable bootc, App VM base, and Cuttlefish image digests.

No placeholder is generated and no release was run. Missing, mismatched, or
null inputs refuse before the test fixture's build-command marker can be
created.

## Farm evidence

- `.50`, slot `crit006-input-preflight-test-r519`:
  `./install-helpers/test-release-input-preflight.sh`
  passed the valid fixture and hostile missing-input, owning-verifier-refusal,
  and null-digest cases.
- `.50`, slot `crit006-input-preflight-shellcheck-r519`:
  `shellcheck -e SC2016 install-helpers/release-input-preflight.sh install-helpers/test-release-input-preflight.sh install-helpers/xcp-build.sh`
  passed. `SC2016` is excluded only for the two pre-existing intentional remote
  shell literals at `xcp-build.sh` lines 87 and 532; an unfiltered run reported
  no finding in either new script.
- Local orchestration-only checks: `bash -n` on all three shell files and
  `git diff --check` passed.

The release was not executed. Remaining WL-CRIT-006 work is the actual first
full release cut with real admitted inputs, RPM/package/signing/artifact checks,
followed by the explicitly deferred non-blocking one-node installed acceptance,
recovery, and corrected-forward evidence.
