# WL-FUNC-021 Cast target availability r1

Date: 2026-08-15

The operator supplied the authorized local Cast target `172.20.146.150`.
Read-only endpoint checks from the build host recorded:

- DIAL descriptor: `http://172.20.146.150:8008/ssdp/device-desc.xml`
- Friendly name: `Family Room TV`
- Manufacturer/model: Xiaomi MIBOX4
- Google Cast control channel: TCP 8009 reachable
- Cast HTTPS endpoint: TCP 8443 reachable
- CastV2 TLS handshake: completed on 2026-08-15 with the device's
  self-signed certificate (CN/UUID `bc67b7d7-1d52-26a4-0353-76b269ef4b3c`)

This proves target availability and the live CastV2 TLS channel only. It does
not claim that the Music Rust adapter has loaded media, played, sought, or
committed renderer ownership. Those remain implementation work; no additional
seat is required.
