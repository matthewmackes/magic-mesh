# WL-FUNC-016 S3 authenticated mesh transport — 2026-08-08

The clipboard worker now adapts canonical signed rich envelopes into bounded,
target-specific mesh frames. Each frame binds the source and target peer,
session, generation, expiry, exact canonical bytes, and enrollment-pinned
Ed25519 identity. Admission rejects unknown schema, duplicate JSON keys,
oversize frames, raw path fields, stale data, replay, unavailable peers, and
source-key mismatches with finite typed reasons.

The worker drains at most 32 frames per tick and retains at most 256
payload-free replay lanes. It commits a replay marker only after the canonical
Bus write succeeds, so transient forwarding failure remains retryable. On
restart or cursor loss it seeds high-water marks from retained canonical
envelopes before inspecting retained target frames, preventing duplicate
forwarding without tail-skipping a still-fresh transfer.

## Farm verification

Host `.50`, slot `func016-s3-mesh-r1`:

```text
cargo test --locked -p mackesd clipboard_sync::mesh::tests -- --nocapture
7 passed; 0 failed; 4367 filtered out

cargo test --locked -p mde-collab-types -- --nocapture
72 passed; 0 failed
```

The mesh suite covers exact-byte sender/receiver transfer, unauthorized peer,
identity mismatch, replay, restart seeding, flood/oversize/raw-path refusal,
stale and unavailable peers, and expiry cleanup. The first current-snapshot
compile exposed an undefined startup clock helper; the corrected candidate was
resynced and the complete focused suite passed.

## Source hashes

```text
52f555ed4635ba1663e44cda3ccd61d317172fae16cbc3573801d5990eee8918  crates/mesh/mackesd/src/workers/clipboard_sync/mesh.rs
c50c1429cf35f355b62158280459b4056431f63e2555a192aecbd40189502192  crates/mesh/mackesd/src/workers/clipboard_sync.rs
03ff63b7b2d89cdf8f6c9b2b11c4249f7be31e4c053c104d95f986d7e60c84c8  crates/shared/mde-collab-types/src/clipboard_v2.rs
```

## Remaining acceptance gap

This is deterministic authenticated transport proof, not physical cross-node
or five-seat evidence. File offers still need the shared CAS lifecycle rather
than inline transfer, and local DRM ownership, VDI guest transport, permission
UI, live cleanup, and release proof remain S2, S4, and S5 work.
