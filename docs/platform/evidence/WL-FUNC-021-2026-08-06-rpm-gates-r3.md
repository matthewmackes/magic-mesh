# WL-FUNC-021 current-tree RPM gates r3 — 2026-08-06

This is the fresh package proof after the Music authorization provisioner
dependency correction. The build and package checks ran on BigBoy
(`172.20.0.130`) in isolated slot
`music-reconnect-auth-rpm-20260806-r1`. No RPM was installed and no seat or
Dell runtime was restarted or rebooted.

## Build and artifacts

The farm release command completed with warnings only:

```text
cargo build --workspace --release --locked
  Finished release profile in 13m 54s
cargo build --release --locked -p mde-shell-egui --features drm,live-vdi,media-mpv
  Finished release profile in 6m 24s
cargo generate-rpm -p crates/mesh/mackesd
cargo generate-rpm -p crates/mesh/mackesd --variant lighthouse
```

```text
magic-mesh-12.1.6-4.x86_64.rpm
  87,308,656 bytes (83.3 MiB)
  SHA-256 bab611c47e4fe93127e9db24bbde84b8f67d8b9bac412f387e2852198cd0f774

magic-mesh-lighthouse-12.1.6-4.x86_64.rpm
  12,466,223 bytes (11.9 MiB)
  SHA-256 dd82fc744845e3aa4ddfa559d2f1c1cc4b4e5ae17d6a80c04548c7a05e557182
```

## Dependency and payload checks

The base RPM's actual `rpm -qp --requires` header contains both newly declared
command providers:

```text
curl
libcurl.so.4()(64bit)
openssl
```

The lighthouse variant contains neither workstation-only command provider.
The base `verify-rpm-payload.sh payload` gate passed all manifest assets,
including `mde-musicd`, the Music action provisioner, its credential config,
and provisioning unit. `verify-rpm-payload.sh requirements` and `size` passed;
the base package remains below the 90 MiB cut limit. The farm package size
checks also passed for both artifacts.

The repository-wide `verify-rpm-payload.sh all` dead-surface warning for the
unrelated pre-existing `mde-panel-egui` remains separate and open; it is not a
Music payload failure.

Installed-seat provisioning, generated-key loading, authorized mutation, and
rotation proof remain open. This artifact gate does not imply those runtime
claims.
