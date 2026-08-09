# WL-FUNC-019 probed RDP resource projection — 2026-08-09

## Universal-catalog correction

An nmap-confirmed Windows RDP listener reached the unified service mirror but
remained a generic non-connectable Service card. Remote Sessions therefore
could not present a Windows host even after discovery succeeded.

A fresh Probe-attested private/link-local TCP 3389 endpoint now projects to one
typed Desktop resource with a trusted-LAN RDP transport. Catalog provenance
truthfully identifies the authenticated mesh service mirror that delivered the
record; the service shape does not retain a probing interface, so the adapter
does not fabricate direct-LAN provenance. Connect requires local approval
because no credential identity is inferred. Published-only, malformed,
wrong-port, public, and stale candidates remain generic non-connectable
services. A probe-reported unavailable endpoint remains visible but has no
ready Connect action.

## Verification

- Machine 9 (`172.20.0.50`), warm slot `arch009-responder-registry-r1`:
  `cargo test -p mackesd --lib workers::service_catalog::tests::rdp_ --features async-services --locked -- --nocapture`
  passed 3/3 on the integrated tree.
- Machine 196 (`172.20.0.196`), slot `integrated-rustfmt-final-r7`: scoped
  `rustfmt --check` passed for `service_catalog.rs`; `git diff --check` passed.

## Source hash

```text
7156a6b4c7727e7f4e8c1b515739dd92267f3dfe08913c922ad304eaf7ed035b  crates/mesh/mackesd/src/workers/service_catalog.rs
```

## Remaining acceptance gap

The live Windows host is on a `/16`, does not advertise RDP, and was absent from
the bounded neighbor set. Its address or a new governed bounded address source
is still required for installed discovery/connect proof. Five-seat recovery
proof also remains, so FUNC-019 stays `Remaining`.
