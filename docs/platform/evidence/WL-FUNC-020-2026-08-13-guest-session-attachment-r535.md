# WL-FUNC-020 guest session attachment — r535

Date: 2026-08-13

## Production gap closed

The governed Cuttlefish role installed authenticated guest binaries, but its
packaged readiness-relay unit executed `/usr/libexec` while the role copied the
binaries to `/usr/local/libexec`. The unit also required
`/etc/mcnf/cuttlefish-guest.conf`, which provisioning never created, and the
role never enabled the relay after starting Cuttlefish. A successful backend
start could therefore leave Android without a typed Remote Sessions attachment.

The production role now:

- refuses before side effects unless mesh host, WebRTC port, and session ID are
  present and bounded;
- installs authenticated binaries at the exact paths used by the packaged
  systemd units;
- writes the session attachment contract as root-owned, group-readable private
  configuration for the unprivileged `cvd` runtime; and
- enables/starts the authenticated readiness relay only after Cuttlefish is
  already running or has started successfully, restarting it when its binding
  changes.

`packaging/android/verify-contract.sh` now fails if the package paths, mandatory
attachment inputs, environment binding, relay activation, or admission-before-
backend ordering drift.

## Verification

- `.130`, slot `func020-guest-attach-build-r1`:
  `CARGO_INCREMENTAL=0 cargo build -p mcnf-cuttlefish-guest --all-targets` —
  passed.
- `.170`, slot `func020-guest-attach-clippy-r1`:
  `CARGO_INCREMENTAL=0 cargo clippy -p mcnf-cuttlefish-guest --all-targets -- -D warnings`
  — passed.
- `.196`, slot `func020-guest-attach-contract-r1`:
  `packaging/android/verify-contract.sh` — passed, including the Android
  manifest, signed guest-payload, placement-readiness, image-receipt, and real
  ephemeral GPG/GPGV contract fixtures.
- `.50`, slot `func020-guest-attach-fmt-r1`:
  `cargo fmt -p mcnf-cuttlefish-guest -- --check` — passed.
- `.196` did not have `ansible-playbook` installed, so the farm syntax probe
  exited 127 before parsing. The permitted local parse-only fallback,
  `ANSIBLE_ROLES_PATH=automation/ansible/roles ansible-playbook --syntax-check -i localhost, automation/ansible/playbooks/site.yml`,
  passed. No live execution or proof claim is derived from that syntax check.
- `git diff --check` — passed for the owned patch.

## Residual WL-FUNC-020 acceptance

- Produce and consume the real signed AOSP/Cuttlefish image and deterministic
  guest DEBs in the first full release.
- Complete any remaining typed cancel/retry/resource-reclamation and input
  cleanup gaps found by the final lifecycle audit.
- After release, run the deferred non-blocking nested-KVM Android boot,
  installation/launch/stop, VDI audio/input/reconnect, SELinux/cgroup/device
  isolation, upgrade, and one-node recovery acceptance on the governed seat.
- Record explicit unavailable diagnostics if the release seat lacks nested KVM
  or a usable Cuttlefish provider; tooling or static contract success must not
  be promoted to live Android readiness.
