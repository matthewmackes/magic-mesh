# WL-FUNC-020 Android VM RPM payload enforcement — r498

Date: 2026-08-13

## Boundary closed

The base and server RPM manifests ship the cloud Ansible playbooks and roles
through broad globs. The previous package verifier proved only that those glob
destinations were non-empty. An RPM could therefore retain unrelated automation
while omitting the Android specialization that bootstraps and runs Cuttlefish.

`install-helpers/verify-rpm-payload.sh` now fails closed unless both Android-capable
RPM roles cover these exact files at their canonical installed paths:

- `automation/ansible/playbooks/site.yml`, the `delivery_android_vm` bootstrap;
- `cuttlefish_host/defaults/main.yml`, the unprivileged `cvd`/KVM/network profile;
- `cuttlefish_host/meta/main.yml`, the packaged role identity; and
- `cuttlefish_host/tasks/main.yml`, the `/dev/kvm` admission, `cvd version`
  readiness check, and `cvd start --start_vnc_server` runtime.

Focused `android-vm-payload` mode checks source existence, exact base/server
manifest coverage, operational contract markers, and—when given a built
RPM—every exact `rpm -qlp` path. The same boundary runs from the complete static
payload verifier and from real base/server RPM verification.

The hostile self-test preserves a non-empty Ansible payload containing the
cloud role plus the Android playbook/profile/metadata, but removes only the
Cuttlefish task file. Exact verification rejects that prior false-green shape.
These assertions are distinct from the App VM cloud-init checks.

## Farm evidence

- `172.20.0.130`, slot `func020-android-payload-selftest-r498`:
  `bash install-helpers/verify-rpm-payload.sh --self-test` passed every
  assertion, including `non-empty Ansible payload cannot hide a missing
  Cuttlefish runtime`.
- `172.20.0.170`, slot `func020-android-payload-syntax-r498`:
  `bash -n install-helpers/verify-rpm-payload.sh` passed.
- `172.20.0.90`, slot `func020-android-payload-static-r498`:
  `bash install-helpers/verify-rpm-payload.sh payload` passed the complete
  static package verifier, including exact Android source, base/server manifest,
  bootstrap, profile, and runtime assertions.

All five farm nodes were reachable for the final gate wave. The three commands
above were unique; no duplicate or filler gate was run.

## Remaining acceptance

The release artifact boundary is now statically enforced. Building and
installing the first full release, then proving the packaged role on a real
nested-KVM Cuttlefish guest, remote-session attachment, application launch,
input/audio, reconnect, cleanup, isolation, and upgrade remains deferred and
non-blocking until after that release under the current operator direction.
