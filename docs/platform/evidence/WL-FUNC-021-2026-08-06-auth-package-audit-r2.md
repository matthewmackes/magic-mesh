# WL-FUNC-021 Music authorization package audit r2 — 2026-08-06

The fresh BigBoy package cut closes the source-to-RPM dependency gap found in
the prior audit. The base RPM now declares and actually carries the hard
requirements needed by the packaged provisioner path:

```text
rpm -qp --requires magic-mesh-12.1.6-4.x86_64.rpm
  curl
  libcurl.so.4()(64bit)
  openssl
```

Fresh artifact:

```text
magic-mesh-12.1.6-4.x86_64.rpm
  87,308,656 bytes
  SHA-256 bab611c47e4fe93127e9db24bbde84b8f67d8b9bac412f387e2852198cd0f774
```

The package payload gate found the Music daemon, provisioner, credential
configuration, and systemd service. The farm release, payload, requirements,
and size checks passed. The prior package audit's `openssl`/`curl` blocker is
therefore resolved at the package-header level.

This remains package proof only. No installed workstation was mutated, no
encrypted credential was provisioned, and no generated public key was loaded
by a running `mde-musicd`. Authorized mutation, wrong-key/replay rejection on
an installed seat, and rotation proof remain required. Dell was not installed
or restarted.
