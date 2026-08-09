# WL-FUNC-020 Android VDI readiness revocation — 2026-08-09

The production Cuttlefish provider now treats its retained guest snapshot and
WebRTC source as a readiness lease, not durable VM identity. Every libvirt
refresh revokes the retained source before querying the outer VM. The source is
restored only when that same refresh observes an active outer VM and the guest
relay returns a newly admitted inventory/VDI pair. A stopped or replaced VM,
provider failure, or failed guest observation can therefore no longer leave the
previous Android session attachable through the provider registry.

## Focused farm verification

Machine 193 (`172.20.0.90`), slot `func020-r4-20260809`:

```text
cargo test -p mackesd --lib --features async-services \
  outer_vm_readiness_loss_revokes_retained_vdi_source -- --nocapture
```

Result: `1 passed; 0 failed`; the hostile fixture begins with a retained
generation-1 VDI source, reports the outer VM as stopped, and proves the source
is no longer returned.

The warmed adjacent provider slice then passed:

```text
cargo test -p mackesd --lib --features async-services \
  workers::cloud::verbs::android::cuttlefish::tests -- --nocapture
```

Result: `6 passed; 0 failed`. Exact-file `rustfmt --check` and scoped
`git diff --check` passed. Source SHA-256:
`c1ae7e7ac48d54e38f18f1d82bce70ffe6832de31632469dd5ab1d0c9fbf4e9a`.

## Remaining live limitation

No signed Cuttlefish image/guest relay is installed on a nested-KVM target, so
real package launch, WebRTC first frame, audio/input, reconnect, RPM upgrade,
host-isolation inspection, and five-seat acceptance remain unproven. This is a
daemon lifecycle/readiness correction only; it does not claim those live gates.
