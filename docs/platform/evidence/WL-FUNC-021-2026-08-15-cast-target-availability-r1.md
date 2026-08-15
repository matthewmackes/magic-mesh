# WL-FUNC-021 Cast target availability r1

Date: 2026-08-15

The operator supplied the authorized local Cast target `172.20.146.150`.
Read-only endpoint checks from the build host recorded:

- DIAL descriptor: `http://172.20.146.150:8008/ssdp/device-desc.xml`
- Friendly name: `Family Room TV`
- Manufacturer/model: Xiaomi MIBOX4
- Google Cast control channel: TCP 8009 reachable
- Cast HTTPS endpoint: TCP 8443 reachable

This proves target availability only. It does not claim that the Music Rust
adapter can yet establish the Cast TLS/protobuf session, load media, seek, or
commit renderer ownership. Those remain implementation work; no additional
seat is required.
