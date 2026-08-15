# WL-FUNC-021 Cast adapter foundation r1

Date: 2026-08-15

Implemented `crates/services/mde-musicd/src/cast.rs` and added the pinned
`rust_cast` 0.21 dependency. The daemon-side seam validates numeric operator
addresses and bounded names, connects to CASTV2 on port 8009 using the device
protocol's certificate mode, and does not expose sockets or third-party types
to the UI.

Farm validation:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=func021-cast-farm6 \
  install-helpers/xcp-build.sh cargo test -p mde-musicd cast -- --nocapture
```

Result: 5 tests passed, 0 failed. This is adapter-foundation evidence only;
media load/play/seek, receiver discovery projection, and renderer ownership
commit remain implementation work.
