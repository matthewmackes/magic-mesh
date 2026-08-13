# WL-FUNC-020 deterministic guest DEB release/package boundary — r527

Date: 2026-08-13

## Result

The first full-release input preflight now requires exactly
`mcnf-cuttlefish-readiness-relay.deb` and
`mcnf-cuttlefish-vdi-agent.deb` from one manifest-bearing directory. It invokes
the canonical deterministic-DEB verifier with the exact source revision and
the already admitted relay/agent bytes. Missing, renamed, cross-directory,
substituted, stale-revision, malformed, or verifier-rejected package sets stop
before the build command.

The Workstation and server RPM manifests now independently own the Cuttlefish
runtime stager, signed-payload verifier, deterministic-DEB verifier, and both
guest systemd service definitions. The Android RPM payload gate pins each exact
source and destination and checks the real `rpm -qlp` list when given a built
candidate. The guest DEBs remain governed release artifacts and are not replaced
with repository placeholders.

Post-release one-node runtime/security proof remains deferred and non-blocking;
this change does not weaken that proof.

## Farm evidence

- `.50`, slot `func020-deb-preflight-r527`:
  `bash install-helpers/test-release-input-preflight.sh` — PASS. The valid
  two-DEB set reached the build boundary and invoked the owning verifier;
  incomplete and mismatched inputs did not.
- `.170`, slot `func020-rpm-payload-r527`:
  `bash install-helpers/verify-rpm-payload.sh android-vm-payload` — PASS for
  every exact Workstation/server source and destination plus the executable
  Ansible contract.
- `.50`, slot `func020-final-r527`:
  `bash install-helpers/verify-rpm-payload.sh --self-test` — PASS, including the
  hostile case where the complete legacy Android payload lacks only the new
  deterministic-DEB verifier.
- `.50`, slot `func020-final-r527`:
  strict ShellCheck of the two changed preflight scripts and package verifier,
  with only the verifier's pre-existing intentional-expression codes excluded
  (`SC2016`, `SC2053`, `SC2015`) — PASS.
- Local orchestration-only checks: Bash syntax and `git diff --check` — PASS.

`.196` was initially selected for ShellCheck but did not have `shellcheck`
installed; it returned before evaluating source. The static gate was rerouted to
`.50`, which has the provisioned tool.

## Remaining acceptance

1. Build the two real deterministic DEBs for the exact first-release revision.
2. Sign the schema-v3 Cuttlefish declaration over those DEBs, the admitted
   runtime binaries, and the immutable Cuttlefish image receipt.
3. Supply the real governed inputs to the first full RPM/release build and run
   this payload gate against the built Workstation and server RPMs.
4. After release, perform the deferred non-blocking one-node Cuttlefish,
   SELinux/device-isolation, VDI, reconnect, and recovery proof.
