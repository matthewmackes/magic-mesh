# WL-FUNC-020 — Cuttlefish host readiness hardening (2026-08-05)

The placement verifier now hashes the governed base image and rejects a digest
mismatch or an image changed while hashing. It verifies effective read/write
access to the KVM character device and requires the configured libvirt pool and
network to be active. The nested-host receipt producer opens KVM read/write
before recording `kvm_access: true`; absence or denied access remains an honest
unavailable result.

## Verification

- Farm `.170`, slot `wl-func020-host-runtime-r1`:
  `packaging/android/verify-contract.sh`.
- Result: Android/Cuttlefish packaging contract checks passed, including
  tampered-image, denied-KVM, and inactive-libvirt hostile fixtures.
- This proves host/tool readiness only. No Cuttlefish guest boot, ADB package
  inventory, display/input/audio, reconnect, or starter-app launch is claimed.
