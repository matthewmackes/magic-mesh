# WL-FUNC-020 — signed Cuttlefish guest-payload handoff (r516)

Date: 2026-08-13

## Gap closed

The production `cuttlefish_host` role previously passed configured `.deb` paths
directly to `apt`, asserted `cvd`, and could start the VNC backend without an
authenticated declaration binding those package bytes to the readiness relay
and VDI agent. The later v3 readiness consumer authenticated release evidence,
but that did not prevent substituted bytes from entering the image first.

The role now fails before package or backend effects unless the project-signed
release declaration names and hashes every guest package plus the exact
readiness-relay and VDI-agent payloads. The verifier reads only single-link
regular files through non-following descriptors, rejects identity changes,
copies the admitted bytes into a private stage, verifies the detached signature
against the pinned project signer, and publishes the stage only after the whole
set passes. `apt` and both installed guest executables consume only those staged
copies. This reuses the existing project release signer and the existing
Cuttlefish/Workloads readiness path; it does not introduce another lifecycle.

Hostile fixtures reject a missing package, same-name substituted package,
substituted VDI agent, hard-link-aliased readiness relay, changed signed
declaration, malformed or incomplete declaration, and any rejected payload that
would otherwise escape into an installable stage.

## Owned files

- `automation/ansible/roles/cuttlefish_host/defaults/main.yml`
- `automation/ansible/roles/cuttlefish_host/tasks/main.yml`
- `packaging/android/verify-guest-payload.sh`
- `packaging/android/verify-contract.sh`

## Farm evidence

- `.50`, slot `func020-guest-payload-contract`:
  `packaging/android/verify-contract.sh` passed. This runs the signed hostile
  payload fixtures, existing Android manifest/readiness self-tests, real GPG
  fixture, RPM dependency check, and production-role ordering/wiring checks.
- `.50`, slot `func020-guest-payload-shellcheck`:
  `shellcheck -x packaging/android/verify-guest-payload.sh
  packaging/android/verify-contract.sh` passed with no findings.
- `.170`, slot `func020-cuttlefish-ansible-syntax`:
  `ANSIBLE_ROLES_PATH=automation/ansible/roles ansible-playbook -i localhost,
  --syntax-check /dev/stdin` with the production `cuttlefish_host` role passed.
- Local orchestration-only `git diff --check` passed.

An initial `.170` ShellCheck attempt could not start because that host lacks the
tool; it was not claimed. It was rerouted to the second in-cap `.50` lane. A
`.196` YAML parse passed but was superseded by the stronger `.170` Ansible role
syntax gate and is not used as acceptance evidence.

## Remaining WL-FUNC-020 acceptance

The first release still must provide the real signed Cuttlefish declaration and
matching package/relay/agent artifacts to this role and verify their inclusion
in the release package/image. Per operator direction, installed one-node
Cuttlefish boot, inventory/launcher readiness, VDI attach, app launch, restart,
provider-loss, and visual proof are deferred until after that release and are
non-blocking for pre-release coding.
