# WL-FUNC-019 — truthful desktop clipboard capability (r139)

Date: 2026-08-10

Base revision: `d0fc3c10`

## Result

The universal Remote Sessions adapter now advertises the bounded text
clipboard channel already implemented by each shipped native desktop client:
RDP CLIPRDR, VNC ClientCutText/ServerCutText, and the SPICE agent. The typed
capability previously declared only display, keyboard, and pointer support, so
resource consumers could not discover clipboard support even after an exact
desktop action was authorized.

This changes only capability projection and its fingerprint. It does not turn
an unavailable endpoint into a ready action, bypass local approval, widen a
transport endpoint, or claim image/file support from adapters that do not
provide it.

## Live seat-15 observation

Seat 15 (`172.20.0.15`) was inspected read-only before this source correction.
It reports `magic-mesh-12.1.6-32.x86_64`; all six grouped daemon services, the
resource-publisher credential helper, and the shell are active. TCP
`172.20.146.54:3389` is reachable.

The latest catalog revision
`peer-basement-test-workstation-1786374283820` contains the available
`Remote Desktop · 172.20.146.54` card with an RDP transport and the exact
approval-gated `connect-rdp-0` action. Its installed capability still lists
only display, keyboard, and pointer, directly demonstrating the projection gap
corrected here. Detection is therefore working; authenticated Windows
login/render remains the live boundary.

## Focused farm proof

Machine 193 build VM `.90` (`172.20.0.90`), isolated slot
`func019-desktop-cap-r139`:

```text
cargo test -p mackesd --lib --features async-services \
  workers::desktop_sources::tests::desktop_resource_capabilities_include_the_live_text_clipboard_channel \
  -- --exact --nocapture
```

Result: 1 passed, 0 failed, 4,679 filtered. The exact adapter regression covers
RDP, VNC, and SPICE cards. Focused rustfmt and `git diff --check` pass.

An initial invocation omitted `--lib` and was stopped while compiling; it is
not counted. No unrelated integration or binary test target is part of this
proof.

## Remaining boundary

This checkpoint does not deploy a new package, enter a Windows password, or
claim an authenticated RDP frame/input/clipboard round trip. Rich image/file
capability vocabulary and live Windows login/render proof remain.
