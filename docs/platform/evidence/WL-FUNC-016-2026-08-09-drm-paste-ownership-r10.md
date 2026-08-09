# WL-FUNC-016 DRM paste ownership boundary — 2026-08-09

Base revision: `ee5f356794f2042edef13fec663a709b1be68291`.

## Production audit and correction

The production path is `mde-shell-egui::Shell::render` setting the focused
surface through `set_drm_clipboard_owner`, followed by
`run_drm_with_clipboard_and_display1` consuming that marker in the sole direct
DRM/libinput loop. The loop owns one `LocalClipboardAuthority`; the injected
client only polls and publishes completed provider work.

The existing authority admits at most `MAX_CLIPBOARD_OFFERS`, validates each
canonical Clipboard V2 MIME payload, keeps only one current offer, invalidates
its generation on focus/app changes, and maps Ctrl+X/C/V to egui cut, copy, and
exact plain-text paste events. Rich MIME order remains intact in the provider
when egui consumes the plain-text representation.

The missing production boundary was an unleased asynchronous Ctrl+V request.
An unavailable provider kept the DRM loop waking every 10 ms indefinitely, and
a focus/app switch could let the original request complete into the newly
focused surface. The direct runner now expires a request after one second and
cancels it whenever clipboard focus changes or is released. Provider methods
remain nonblocking, expiry emits no fabricated paste event, and an app switch
does not discard the explicit-copy rich MIME offer from the global provider; a
new app must issue its own Ctrl+V.

## Focused farm verification

Farm target: XEN-194 build VM `mcnf-build-xen-194` (`172.20.0.170`), slot
`func016-drm-runtime-r10`.

```text
MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=func016-drm-runtime-r10 \
  install-helpers/xcp-build.sh cargo test -p mde-egui --features drm drm_clipboard_ -- --nocapture

12 passed; 0 failed; 0 ignored; 315 filtered out
```

The focused set includes the new app-switch/generation test and the unavailable
provider expiry/no-spin test, alongside existing cut/copy/paste, provider poll,
clear, normalization, output bound, and no-fabrication checks. Exact-file
`cargo fmt --check` and `git diff --check` passed.

Source hashes after verification:

```text
374deab6cad5e6d3a17144b307a5d2b11536037aa680dee55492cab689900b38  crates/shared/mde-egui/src/drm.rs
b3125e5ca4d7d68a98477ce7e3e5fa6f22aa266a8e432e1efb384ed4958b8dc4  crates/shared/mde-egui/src/clipboard.rs
```

## Live-seat blocker

The `.170` farm VM exposes a virtual QEMU DRM/input seat, not a physical
Workstation seat; the `mm` test user cannot read its input devices. This gate
therefore proves the production-reachable state machine and DRM-feature build,
not physical focus switching or native selection ownership. Live direct-seat
copy/cut/paste across local apps and the five-seat cleanup proof remain required
before WL-FUNC-016 can close.
