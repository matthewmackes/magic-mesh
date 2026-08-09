# WL-FUNC-020 S2/S5 release-artifact admission — 2026-08-09

The production Cuttlefish placement gate and its sole shipped guest-tool receipt
producer now share exact schema v2. Placement remains `unavailable` unless the
configured release artifact is a stable regular file with the admitted digest,
the Android package manifest bytes match their admitted digest, and fresh
guest-owned evidence binds the same image, release, package manifest,
architecture, OS compatibility, and installed `cvd`/`adb` payload digest.

The producer derives the installed payload digest from a canonical manifest of
the resolved guest executables: fixed lexicographic tool order, name, decimal
size, and each file's SHA-256. It accepts no caller-provided payload digest and
retains bounded-size plus before/after file-identity checks. Hostile fixtures
independently mutate `cvd` and `adb` from the same baseline and prove each
changes the emitted digest.
The verifier refuses absent/substituted release artifacts, changed package
manifests, legacy schema, payload substitution, and compatibility mismatch; none
can produce `ready_for_provisioning`.

Verification used machine 193 (`172.20.0.90`), slot
`func020-artifact-r5`, at source revision
`780d0d88a34485e3e3a764ef482c84939b6b399b`:

- `bash packaging/android/verify-contract.sh`: passed.
- `python3 install-helpers/verify-cuttlefish-readiness.py --self-test`: passed.
- Local exact-file syntax compilation and `git diff --check`: passed.

No signed production Cuttlefish release artifact or nested-KVM Android guest is
installed on a live target. Consequently this correction records no live boot,
Android package inventory, display, audio/input, or five-seat readiness claim;
those states remain explicitly unavailable.
