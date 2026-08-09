# WL-FUNC-021 — nonblocking Chromecast discovery (r12)

## Production change

The Media cast picker now performs native `_googlecast._tcp.local.` discovery
through `mdns-sd` and projects only resolved records with a bounded identity,
nonzero port, and concrete address. The live mesh-roster, SSDP, and Chromecast
probes run on one named worker thread; egui actions only start that worker and
return. The frame pump polls a result channel, coalesces duplicate starts, drops
an expired receiver after six seconds, retains the last target snapshot on
timeout, and schedules 50 ms repaints only while discovery is pending.

Discovery does not confer cast authority. A discovered Chromecast remains
explicitly non-castable: `NetworkCaster` returns `CastError::Gated` naming the
missing authenticated CASTV2 launch handshake.

## Focused verification

Farm machine 9 (`172.20.0.50`), slot `func021-chromecast-async-r12`:

```text
cargo test --locked -p mde-media-core cast::tests::resolved_ -- --nocapture
2 passed; 0 failed

cargo test --locked -p mde-media-egui model::tests::live_cast_discovery_ -- --nocapture
2 passed; 0 failed

cargo test --locked -p mde-media-core \
  cast::tests::chromecast_and_mesh_casts_are_typed_gates_naming_what_they_need \
  -- --exact --nocapture
1 passed; 0 failed
```

`git diff --check` passed. No physical Chromecast was present for a live launch;
this checkpoint claims real production discovery wiring and the authenticated
launch refusal, not CASTV2 playback or hardware acceptance.
