# WL-FUNC-021 — live-seat package diagnostic boundary (2026-08-06)

This note covers only the read-only package identity, payload, and integrity
diagnostics in `install-helpers/verify-music-live-seat.sh`. It does not install
an RPM, restart a service, reboot a seat, or mutate Dell or seat runtime.

## Expected artifact and installed-seat observation

The current source declares platform version `12.1.6` and base RPM release `5`.
The farm-cut base artifact is:

```text
magic-mesh-12.1.6-5.x86_64.rpm
sha256 c72b9de16f7e0cb9355f092c902f1b44eedd12c751f2e2be4b7246dc754c9ebe
```

The verifier now reports the individual failed field and the exact expected
artifact. A bounded read-only run against seat `172.20.0.15` produced:

```text
[OK] mde-musicd.service active (NRestarts=0)
[OK] mde-musicd ping answered
[OK] action/music/get-state answered on /run/mde-bus
[OK] action/music/list-albums answered on /run/mde-bus
[FAIL] installed RPM release is magic-mesh-12.1.6-4.x86_64; expected RELEASE=5 from artifact magic-mesh-12.1.6-5.x86_64; install that release before rerunning
[OK] installed magic-mesh payload includes mde-musicd and mde-shell-egui
[OK] rpm -V magic-mesh reports only the approved mutable secret-helper difference
[FAIL] installed magic-mesh package proof is incomplete; expected release artifact magic-mesh-12.1.6-5.x86_64 (rpm rc=0/0/1)
```

That pre-install result was intentionally red: the seat still had release 4
while the current artifact was release 5. After the operator-authorized native
Fedora 44 release-5 installation and daemon restart, Dell was rerun and
passed:

```text
[OK] mde-musicd.service active (NRestarts=0)
[OK] mde-musicd ping answered
[OK] action/music/get-state answered on /run/mde-bus
[OK] action/music/list-albums answered on /run/mde-bus
[OK] rpm -V magic-mesh reports no installed-file differences
[OK] installed magic-mesh-12.1.6-5.x86_64 matches declared version 12.1.6/RPM release 5 and verifies mde-musicd and mde-shell-egui payloads
verify-music-live-seat: PASS
```

## Diagnostic and self-test changes

- Identity validation now checks the expected `NAME`, `VERSION`, `RELEASE`, and
  `x86_64` architecture, and prints a sanitized field-level mismatch.
- Payload failures identify missing `/usr/bin/mde-musicd` or
  `/usr/bin/mde-shell-egui` paths.
- RPM verification failures report the count of unexpected differences without
  printing secret contents; the existing secret-helper exception remains
  accepted.
- `./install-helpers/verify-music-live-seat.sh --self-test` passed without SSH.
- `bash -n install-helpers/verify-music-live-seat.sh` passed, including an
  independent syntax check of the embedded remote script.

The second canonical seat still requires the release-5 installation and fresh
read-only verification. This note does not claim physical renderer, provider
loss/recovery, or two-seat handoff proof.
