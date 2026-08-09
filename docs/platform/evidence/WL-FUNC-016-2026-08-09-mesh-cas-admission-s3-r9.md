# WL-FUNC-016 S3 mesh CAS admission — 2026-08-09

No governed two-daemon authenticated Bus/CAS fixture was available on the
build farm, so this slice makes no physical cross-node claim. The production
mesh clipboard adapter was corrected instead.

Files-backed offers now resolve their opaque `FileRefId` through retained
`state/collab/file-references/*` authority, bind the source peer, and verify the
derived canonical CAS object against the signed byte count and SHA-256 before a
frame is sent or accepted. Missing replicated bytes defer the cursor until they
arrive or the envelope expires. Hash/size/projection disagreement fails closed.
Expiry cleanup releases only the clipboard transport lease; canonical content
remains owned by the collaboration purge gate.

The Files-topic capacity bound applies only to the first 257 Files identity
topics. A hostile fixture with 257 unrelated Bus topics plus one valid Files
topic still passes. Retained Files JSON is duplicate-key checked before serde;
a duplicate `space` field is refused before authority use and emits no frame.

## Bounded fixture and results

- Target peer: `target`; source peer: `source`; target seat: `seat0`.
- Exact non-secret payload: 49 bytes, PNG signature plus
  `non-secret bounded rich clipboard fixture`.
- Payload SHA-256:
  `bab17c8e502f02cbdc802f9ee348a462c033b279e27541fc5363d233819d6019`.
- Envelope generation: 1; created: 1000 ms; expiry: 11000 ms.
- Exact CAS admission, delivery, replay denial, hash mismatch, unavailable CAS,
  dedupe, lease cleanup, and canonical-object preservation passed.

BigBoy `.130`, slot `func016-crossnode-r9`:

```text
cargo test -p mackesd --lib --features async-services \
  clipboard_sync::mesh::tests -- --nocapture
8 passed; 0 failed

cargo test -p mackesd --lib --features async-services \
  workers::clipboard_sync::mesh::tests::files_cas_projection_with_duplicate_field_is_refused_before_authority_use \
  -- --exact --nocapture
1 passed; 0 failed
```

Source SHA-256:

```text
f5303ec73dda171733079614b89dbd3fcfdc0bbf53a20096204d013140886b11  crates/mesh/mackesd/src/workers/clipboard_sync/mesh.rs
f5d78b9d66256cf6b7d9b13d4733fc61ad47d00fa691e30b16d3a85f7cfef8dd  crates/mesh/mackesd/src/workers/clipboard_sync.rs
```

`git diff --check` passed for both production files. The explicit farm slot was
removed after verification.

## Remaining acceptance gap

Real cross-node Bus/CAS replication, live DRM-seat rich materialization, VDI
guest round-trip/reconnect, five-seat cleanup/memory proof, and package/live
permission evidence remain open. The current seat handoff is still text-only,
so Files-backed and non-plain rich MIME need their governed compositor/VDI
materializers before WL-FUNC-016 can close.
