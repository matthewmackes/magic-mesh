# WL-FUNC-016 mesh clipboard cross-process fixture (r11)

Date: 2026-08-09

## Scope

This slice advances S3 without adding another clipboard authority or claiming a
physical cross-node deployment. A focused fixture now drives the production
`send_envelope` and `receive_frame` adapters through the real `Persist` SQLite
bus and the production read-only `SqliteMeshClipboardPeerDirectory` from
independent process invocations.

The sender and receiver processes independently reopen the same bus and peer
database. Later receiver processes reopen the persisted cursor and retained
canonical lane. The fixture proves:

- exact UTF-8 plain-text and HTML offers survive unchanged;
- exact non-UTF-8 PNG/CAS bytes remain digest-, size-, and Files-projection-bound;
- the source Ed25519 key must match the enrollment-pinned SQLite identity;
- a retained admitted generation is refused as replay after process restart;
- an expired in-flight frame is refused as stale;
- terminal stale and unauthorized frames advance the payload-free cursor; and
- a final independent reopen sees no rows after that cursor and exactly one
  canonical delivery.

## Focused farm proof

Machine 193 (`172.20.0.90`), slot `func016-mesh-xproc-r11`:

```text
cargo test -p mackesd --lib --features async-services \
  mesh_cross_process_persist_sqlite_preserves_rich_payload_and_security_state \
  -- --nocapture

test result: ok. 1 passed; 0 failed; 0 ignored; 4377 filtered out
```

The one outer test completed six successful independent child-process roles:
sender, receiver, replay-after-reopen, expired-frame receiver, forged-identity
receiver, and final forward-only reopen. The exact file produced no rustfmt
diff; package-wide formatting still reports unrelated pre-existing files.

## Remaining blocker

This is a production-reachable cross-process persistence and authentication
fixture, not a physical network claim. S3 still needs a governed two-node live
seat fixture with enrolled node credentials, real mesh transport delivery, DRM
focus/ownership interaction, expiry, and disconnect/reconnect cleanup evidence.
