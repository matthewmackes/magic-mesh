# WL-ARCH-010 VDI reconnect generation guard — 2026-08-06

`mackes-mesh-types` now rejects generation-zero `Reconnecting` runtime
evidence. A delayed legacy observation therefore cannot reopen a newer runtime
incarnation; numbered reconnect evidence remains valid. The change also keeps
App VM admission identity and capability checks on the typed boundary.

Verification:

- BigBoy `.130`, slot `arch010-vdi-reconnect-generation-20260806-r2`:
  `cargo test -p mackes-mesh-types
  reconnect_runtime_evidence_rejects_legacy_generation_replays -- --nocapture`
  passed **1/1**.
- The first farm sync reached BigBoy `ENOSPC`; disposable completed state was
  cleaned and the rerun passed.
- `git diff --check` passed.
- Source SHA-256:
  `2f655a6705ffd2d2c69313dcfecb96bd90c6c8171cbf3c8fe9c156a04f570513`.

Generation validation is not live reconnect proof. Native display attachment,
seat recovery, packaging, and Dell acceptance remain open. Dell runtime was
not modified.
