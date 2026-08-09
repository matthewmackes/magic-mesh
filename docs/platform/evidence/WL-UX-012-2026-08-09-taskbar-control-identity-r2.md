# WL-UX-012 taskbar control identity — 2026-08-09

Connected-session targets and pinned-desktop targets are independent taskbar
projections, but their zero-based indices previously shared the same egui ID.
The shell now includes the typed control kind in indexed taskbar IDs, preventing
a session and pinned desktop at the same index from sharing click, keyboard
focus, or accessibility state. Their existing Bottom and Left hit regions remain
disjoint.

Production source SHA-256:
`5b052e85dc5959a41e6c57871ad4309e36fc6a7862e4b26f6ec4299e0edf977d`.

## Verification

- BigBoy `172.20.0.130`, slot
  `ux012-taskbar-control-identity-r1-20260809`: the exact mixed-projection
  identity/hit-region regression passed 1/1.
- The complete `nav_bar::tests` suite passed 49/49 in the same slot.
- Exact-file `rustfmt --edition 2021 --check` passed on BigBoy.
- Scoped `git diff --check` passed.

Live-seat and broader responsive/package proof remain, so WL-UX-012 remains
`Remaining`.
