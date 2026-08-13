# WL-ARCH-008 — Browser RDP Workloads publication (r523)

## Scope

The canonical `browser-vm` request helper now asks the sole Workloads plane for
an RDP `StartAndAttach` lease. The node-local Workloads actuator admits that
route only for the canonical Browser VM on libvirt; generic VMs and every other
remote protocol remain refused.

The first-release profile is the operator-approved one-node topology. The lease
digest binds the exact operation request, canonical workload, next generation,
and the same authenticated mesh node in both serving and client roles. The
consumer still derives the only endpoint as that mesh node on fixed port 3389;
no caller-provided URL, host command, guest credential, or second catalog is
admitted.

Publication remains unavailable until all of these are simultaneously true:

- libvirt reports the exact Workload generation running;
- the fixed Browser RDP endpoint resolves to a bounded address and accepts a
  bounded two-second TCP probe;
- the journal still owns the exact request/generation lease; and
- the lease remains inside its bounded 15-minute operation authority.

Stop, cancellation, terminal failure, generation replacement, deadline expiry,
and malformed recovery all remove the durable attachment before the shell can
continue consuming it. Daemon restart may recover only the byte-identical
journaled lease, while the VM is still running and the fixed RDP endpoint is
again reachable. RDP owns no Display1 socket; Display1 remains the local KMS
authority and the Workloads reconciler remains the only lifecycle authority.

## Verification

- `.130`, slot `arch008-browser-rdp-test-r523`: focused mackesd library tests
  `browser_rdp_publication*` passed 2/2 with 4,960 filtered.
- `.170`, slot `arch008-browser-rdp-clippy-r523b`: strict all-target mackesd
  Clippy with `-D warnings` reached the owning crate with no Browser diagnostic,
  but the branch-wide command remains red on the independently committed
  Android-only test alias at `cuttlefish_guest.rs:23` (`unused_imports`). This
  slice does not alter or waive that external diagnostic.
- `.50`, slot `arch008-browser-rdp-helper-r523`: request-helper self-test and
  ShellCheck passed.
- Local `bash -n`, helper self-test, and `git diff --check` passed.

No image was built and no live RDP, DRM, guest, or release proof is claimed.

## Remaining ARCH-008 acceptance

Produce and release-sign the real Lighthouse RPM, Browser base/image receipts,
and immutable Browser VM image; run the first full release package/provenance
verification; then perform the deferred non-blocking one-node attachment,
restart, stop/replacement revocation, audio, migration, performance, and visual
proof on the released payload.
