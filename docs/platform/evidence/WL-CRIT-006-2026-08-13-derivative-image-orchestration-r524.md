# WL-CRIT-006 — first-release derivative image orchestration (r524)

Date: 2026-08-13

## Result

`install-helpers/build-release-derivative-images.sh` is the canonical
post-signing, pre-promotion pipeline for the first App VM and Browser VM image
derivatives. It accepts the exact release source revision, signed Workstation
and Lighthouse RPMs, their governed candidate manifests, admitted base-image
receipts, and App VM catalog trust. It then:

1. refuses a stale Browser profile before invoking either builder;
2. snapshots both signed RPMs and candidate manifests into a private read-only
   stage, preserving caller-owned signed RPMs on every outcome;
3. invokes the existing App VM and Browser RPM admission verifiers;
4. invokes the existing App VM and Browser VM builders for qcow2 outputs;
5. re-runs the existing Browser artifact-manifest verifier;
6. emits the missing bounded App VM disk manifest and records complete SHA-256
   and size identities for both disks and both immutable image manifests; and
7. atomically publishes one mode-0500 candidate collection only after every
   derivative succeeds.

The collection declares `promotion: forbidden`. The helper does not sign,
publish, promote, build Android artifacts, or invent release inputs.

No `xcp-build.sh` integration was made in this slice. Its canonical `rpm` path
currently stops after collecting and size-checking unsigned RPMs, while its
preflight and argument surface were concurrently owned by Android image work.
Calling the derivative stage there would violate the required post-signing
boundary. The helper is ready for the canonical signing pipeline to invoke
after `sign-release.sh --prepare-rpms` (or its eventual release-cut successor)
has produced both governed signed RPM candidates.

## Hostile coverage

`install-helpers/test-build-release-derivative-images.sh` proves that:

- both builders receive only private snapshots and all admitted inputs;
- App VM failure prevents Browser execution and publishes no collection;
- a rejected Browser manifest publishes no collection;
- signed RPM input digests remain unchanged after success and failures;
- a stale Browser profile invokes no verifier or builder; and
- success emits the non-promotable immutable collection manifest.

## Gates

- `.50`, slot `crit006-derivative-selftest-r524d`:
  `install-helpers/test-build-release-derivative-images.sh` passed the complete
  hostile orchestration suite.
- `.50`, slot `crit006-derivative-shellcheck-r524d`:
  strict ShellCheck passed for the orchestration helper and hostile self-test.
- Local Bash syntax and scoped `git diff --check` passed.

The initial `.90` ShellCheck dispatch was not claimed because that host does
not install ShellCheck. An initial `.50` run exposed Fedora-specific sealing
and test-cleanup defects; those were corrected, and only the clean `r524d`
reruns above are acceptance evidence.

## Remaining CRIT-006 acceptance

- Update the Browser profile to the exact first-release revision as part of the
  governed release input freeze.
- Produce and sign the real Workstation and Lighthouse RPMs and produce their
  matching candidate manifests.
- Supply real App VM/Browser base receipts and App VM catalog trust material.
- Invoke this pipeline from the canonical signing/release cut once that cut has
  an actual post-signing boundary.
- Supply and admit the independent Android and bootc inputs.
- Run the first full release, verify all RPM/image manifests and signatures,
  and retain the derivative collection without publication until promotion is
  separately authorized.
- Perform deferred, non-blocking post-release one-node acceptance and recovery
  proof.
