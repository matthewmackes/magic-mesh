# WL-FUNC-020 governed Android Workloads UX S4 — 2026-08-08

Workloads now projects governed Android app cards from an admitted catalog,
provider preflight, guest package/launcher inventory, lifecycle receipt, and
typed VDI source. Cards expose bounded package, permission, capability, and
evidence summaries plus explicit Start, Stop, Cancel, and Retry states.

Before showing policy or a signer claim, polling binds the writable Bus payload
digest to the daemon's root-owned admitted cache under
`/var/lib/mackesd/android-catalog/<host>.json`. Cache admission is bounded,
no-follow, regular-file-only, root-owned, and rejects group/world-writable
files. Spoofed or mismatched projections expose no cards. Rendering performs no
disk, Bus, network, clock, or backend I/O.

Lifecycle requests carry only closed operation/app enums, workload identity,
and exact generation through the existing arming authority. A ready WebRTC
source produces a typed handoff without parsing or launching a raw endpoint.
The shell now consumes that handoff through the one Remote Sessions broker,
preserves the exact Android generation/session/provenance, and publishes the
authorized open/close lifecycle. Failed authorization retains an actionable
refusal but cannot install a request or spawn a transport.

## Verification

- `.170`, slot `func020-android-ux-s4-r1`: focused governed-Android tests passed
  6/6 (1,471 unrelated tests filtered).
- Fixtures covered cache/Bus trust mismatch, policy and preflight refusal,
  Start/Cancel/Retry generation state, armed closed publication,
  narrow/largest-text rendering, and typed VDI handoff.
- Scoped rustfmt and `git diff --check` passed.
- No operational tests were removed.
- `.170`, slot `func020-android-vdi-s4-r2`: Remote Sessions handoff and
  authorization-refusal tests passed 2/2 (1,477 unrelated tests filtered).
- The same `.170` source passed the feature-gated `live-vdi` authorization
  refusal 1/1 (1,511 unrelated tests filtered), proving no transport install in
  the production transport configuration.

## Remaining acceptance gap

No live nested-KVM Android VM, package launch, WebRTC decoder attachment,
responsive seat capture, RPM upgrade, or hostile live guest was exercised, so
FUNC-020 stays `Remaining`.
