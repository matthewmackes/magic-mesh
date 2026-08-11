# WL-FUNC-020 signed Android desired definition — 2026-08-11

- Scope: `android-provision` reloads the durable last-good catalog under the
  pinned Ed25519 trust policy and current validity window, binds its exact image
  and package manifests to the outer Android Workload definition, verifies the
  configured artifact digest, host capacity, and libvirt provider, then
  atomically persists package provenance before authorizing desired state.
- Failure boundary: substituted artifact bytes and a provenance replacement of
  an existing definition fail closed. A package manifest staged before a later
  desired-row failure authorizes no lifecycle effect; the inverse unsafe state
  is never written.
- Farm: BigBoy `172.20.0.130`, slot `2`.
- Focused gate: `install-helpers/xcp-build.sh cargo test -p mackesd --features
  async-services
  workers::cloud::verbs::android::tests::signed_release_provenance_gates_the_persisted_android_definition
  -- --exact --nocapture`.
- Result: PASS, 1 passed, 0 failed, 4,812 filtered out. The first `.90` run hit
  ENOSPC before test execution and was discarded; the proof is the clean
  BigBoy rerun.
- Remaining proof: quarantine legacy provenance-unbound desired rows, then run
  Cuttlefish launch/VDI and stop/cancel/retry on live nested-KVM hardware.
