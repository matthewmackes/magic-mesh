# WL-FUNC-018 App VM image/profile contract S2 — 2026-08-08

The repository contains one immutable `wayland-standard` App VM image contract.
The guest owns Sway, portals, PipeWire, Flatpak, and a fixed supervisor. Runtime
inputs are bounded stable identities, never commands or package paths. The
launcher independently validates those inputs, proves the curated system remote
and admitted Flatpak are present, starts the guest compositor and application,
publishes monotonic readiness evidence, and terminates both on shutdown.

The image build binds a complete base-image digest and source revision into
labels and guest-readable provenance. It does not add public Flathub or another
unsigned remote. The runtime probe fails unavailable for missing compositor,
audio/portal tooling, curated remote, application, malformed identity, stale
generation, or readiness failure rather than claiming a connected guest.

## Verification

- `.170`, slot `func018-app-vm-contract-audit-s2-r1`:
  `packaging/app-vm/verify-contract.sh` passed.
- The gate exercised the image provenance/readiness self-tests and complete
  executable guest contract fixtures.

## Remaining acceptance gap

This is contract proof, not a current image artifact or boot trace. S2 still
requires a current immutable image build, recorded image/base/source hashes,
curated signed remote provisioning, and live Sway/Flatpak/PipeWire/VDI readiness.
Typed lifecycle, Front Door UX, sandbox/package proof, and five-seat acceptance
also remain, so FUNC-018 stays `Remaining`.
