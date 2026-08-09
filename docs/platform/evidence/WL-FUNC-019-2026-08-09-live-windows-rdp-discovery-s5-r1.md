# WL-FUNC-019 live Windows RDP discovery — 2026-08-09

- Scope: live address and service discovery only; no authenticated RDP login or
  rendered desktop claim.
- Cause confirmed: the active seat is on `172.20.0.0/16`, which is deliberately
  too wide for automatic enumeration, and RDP does not advertise its endpoint.
  The installed observation group emitted the actionable
  `/root/.config/mde/probe-targets.toml` diagnostic.
- Correction: Basement seat 15 (`172.20.0.15`) now has the bounded explicit
  target `172.20.146.54`; only `mackesd-observation.service` was restarted.
- Live proof: TCP 3389 was reachable from both the development host and seat 15.
  The first corrected probe published 10 hosts to the shared inventory, where
  `172.20.146.54` appears with service `ms-wbt-server`, port `3389`.
- Recovery: the observation group returned `active`; the target configuration
  is a root-owned mode-0600 regular file.
- Remaining: authenticated connection/render proof and publisher-attestation
  credential distribution remain required before universal-resource acceptance.
