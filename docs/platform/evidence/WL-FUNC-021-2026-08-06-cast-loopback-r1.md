# WL-FUNC-021 — cast renderer loopback witness (2026-08-06)

Status: bounded protocol evidence only; physical renderer, Chromecast,
mesh-owner, and seat-handoff acceptance remain open.

## Command and result

```text
install-helpers/verify-music-cast-loopback.sh --self-test
verify-music-cast-loopback: self-test passed
```

The bounded default exchange completed against a disposable `127.0.0.1`
renderer and required successful discovery, device description,
`SetAVTransportURI`, `Play`, and finite `Seek` in order. Malformed XML and
non-finite seek input were refused with HTTP 400. The listener, worker thread,
and temporary state were cleaned up before exit. `bash -n` also passed.

## Limitations

The helper intentionally does not claim SSDP on a physical LAN, a real DLNA
renderer, Chromecast control, mesh-owner selection, or position-continuous
seat transfer. Those remain open acceptance boundaries under WL-FUNC-021.
