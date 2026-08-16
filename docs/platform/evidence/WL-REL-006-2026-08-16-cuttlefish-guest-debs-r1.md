# WL-REL-006 Cuttlefish guest DEBs — current revision

BigBoy built and verified the two required Cuttlefish guest packages from the
source-bound release stage.

- Source revision: `43d909b498feb1bb49096507e9c4eb8bd2441553`
- Farm host: `172.20.0.130`
- Farm slot: `rel006-cuttlefish`
- Stage gate: `packaging/android/stage-guest-runtime-artifacts.sh` — PASS
- Package gate: `packaging/android/verify-guest-debs.sh` — PASS
- Version: `13.0.0-1.git43d909b498fe`
- Guest manifest SHA-256: `b85909fa1203aeb1eb0b99d736ea8e3d6d1206d65efdcf00f6ad27142de13feb`
- `mcnf-cuttlefish-readiness-relay.deb`: 275480 bytes,
  SHA-256 `240a1c317a8655689de15dedefa86773343a9c8ab7ec02ee60fc694cb7c9c9f2`
- `mcnf-cuttlefish-vdi-agent.deb`: 263152 bytes,
  SHA-256 `9baecfc209551c1af6d7f6f7ea3b79b75f2b6df15044e911fdb7ea4dd567741d`

The stage gate built both release ELF binaries, checked x86-64 identity and
debug stripping, and bound their hashes to the guest manifest. The package
gate then verified Debian metadata, dependency identity, architecture, and
exact package hashes.

This advances the guest-package portion of WL-REL-006 S4. It does not close S4:
the Android/Cuttlefish image artifact and its immutable image receipt are still
absent, so no guest declaration or production preflight admission is claimed.
