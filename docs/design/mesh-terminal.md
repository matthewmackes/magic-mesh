# mde-term — a Terminator-class terminal emulator for the mesh

*Operator directive 2026-07-02: "Create a Terminal Emulator for the platform that
matches the capabilities of Terminator (github.com/gnome-terminator/terminator)."
Locked via a 16-question survey.*

A new egui-native terminal surface with Terminator's full feature set — arbitrarily
nested split panes, tabs, broadcast/grouped input, saved layouts, zoom, drag-rearrange
— extended for the mesh: any pane can be a shell on a **remote mesh node** over the
Nebula overlay, authenticated by the shared mesh SSH key.

## Locks (16-Q survey)

| # | Decision | Lock |
|---|---|---|
| 1 | VT/ANSI engine | **A mature VT crate** (`alacritty_terminal` or `termwiz`) — full xterm/VT100 fidelity, no reinvention (§6). Added to `Cargo.lock` + vendored (network at lock-gen, then airgap-buildable — the path BM3's `rusqlite` took). |
| 2 | Surface architecture | **New `mde-term-egui` crate** — a sibling of `mde-files-egui`/`mde-music-egui`/`mde-bookmarks-egui` (mde-egui harness + mde-theme tokens); its own binary + surface entry. |
| 3 | PTY layer | **openpty (libc/rustix, in-lock) for local** shells + a **mackesd PTY-broker worker for mesh** shells over the overlay, reusing FILEMGR-6's `mesh-ssh-key` (`MESH_SSH_KEY_REF`). Airgap-clean + mesh-native. |
| 4 | Split model | **Arbitrary nested H/V splits** — a binary tree of panes, split to any depth, every divider draggable (Terminator's exact model). |
| 5 | Broadcast input | **Full grouping** — broadcast typed input to ALL panes, to a NAMED GROUP, or OFF, with a visible indicator on broadcasting panes. |
| 6 | Tabs | **Tabs, each holding its own split tree** — every tab is an independent nested-split layout. |
| 7 | Saved layouts | **Named layouts, mesh-synced** — serialize (split tree + per-pane cwd + optional launch command + target mesh node); sync via Syncthing so any node can launch a layout. |
| 8 | Pane manipulation | **Zoom-a-pane** (maximize/restore within the window) **+ drag-to-rearrange** the split tree. |
| 9 | Remote-terminal UX | **Peer picker + manual entry** — a "new terminal on → <peer>" picker driven by the mesh roster + presence pips (offline greyed), plus a manual host/overlay-address escape hatch. |
| 10 | Local shell | The user's **`$SHELL` as a login shell, inheriting cwd** + the platform env. |
| 11 | Scrollback | **Unlimited (soft-capped) + in-scrollback search with regex** + match highlighting. |
| 12 | Selection/clipboard | **Smart:** selection + copy/paste; detected **URLs open in the BOOKMARKS mesh browser**, detected **filesystem paths open in the Files surface**; optional copy-on-select + paste-on-middle-click. |
| 13 | Profiles | **One Carbon-derived look + a few knobs** (font size, cursor style) — NOT a full named-profile system. |
| 14 | Content palette | **Carbon-derived default 16-color palette + bundled classic presets** (Solarized/Gruvbox/Nord/…), user-pickable. The chrome (tabs/titlebars/splitters) stays pure Carbon §4. |
| 15 | Keybindings | **Terminator-compatible defaults, fully rebindable** (Ctrl+Shift+E/O split, Alt+arrows navigate, etc.). |
| 16 | Per-terminal extras | **Editable titles + activity/silence watch (→ the Chat/notification path) + configurable bell.** |
| 17 | Mouse reporting | **Full SGR (1006) mouse reporting** so TUI apps (vim/htop/tmux/mc) get click/drag/scroll/hover; **hold Shift to bypass** into native text selection. |
| 18 | Session persistence | **Remote panes persist + reattach; local ephemeral.** A shell on a mesh node keeps running if the surface closes/crashes — the mackesd broker holds it and the pane can reattach later. Local shells end on close (Terminator behavior). |
| 19 | Right-click actions | **User-defined custom commands** (run on the selection, Terminator parity) **+ built-in mesh actions**: send-selection-to-Chat, open-path-in-Files, open-URL-in-browser, new-terminal-here. |
| 20 | Rendering fidelity | **Bundled monospace + programming ligatures + 24-bit true-color** (and 256-color). Inline images (sixel/kitty) deferred to a follow-up. |

## Architecture

- **`crates/desktop/mde-term-egui/`** (new) — the surface. Follows the sibling egui idiom
  (mde-egui harness, mde-theme Carbon `Style` tokens for all chrome). Owns the tab bar,
  the split tree, the pane widgets, broadcast routing, the palette/knobs, keybindings.
- **VT engine** — `alacritty_terminal` (or `termwiz`) provides the grid, scrollback, and
  ANSI/xterm parsing; we render its cell grid through egui (GPU glyphs) and feed it PTY
  bytes. Added to the workspace deps + vendored (§6 — never re-implement VT parsing).
- **Local PTY** — `rustix`/`libc` `openpty` spawns the user's `$SHELL` (login, inherited
  cwd/env); a reader thread pumps PTY→engine, writes go engine→PTY; SIGWINCH on resize.
- **Mesh PTY** — a new **mackesd worker** (`pty_broker`) opens a shell on a remote peer
  over the overlay using the `mesh-ssh-key`; typed Bus verbs `action/pty/<peer>`
  (open/write/resize/close) + `state/pty/<id>` (bytes/exit). §6 mesh-side; honest typed
  gating when a peer is unreachable (never fakes a session).
- **Split tree** — a `Pane = Leaf(Terminal) | Split{dir, ratio, a, b}` bin-tree per tab;
  egui renders draggable splitters; zoom overlays one leaf; DnD reparents leaves.
- **Broadcast** — an input router fans a keystroke to the focused pane, all panes, or a
  named group; broadcasting panes carry a Carbon-token border indicator.
- **Layouts** — serde of the tab/tree + per-pane {cwd, cmd, node}; a mesh-synced store
  (Syncthing share, like bookmarks/adfilter); "launch layout" rebuilds it (local + remote
  panes).
- **Smart clipboard** — URL/path regex on selection → dispatch to the BOOKMARKS browser /
  Files surface via the existing surface-launch path (§6 reuse).
- **Notifications** — activity/silence watchers publish into the Chat/notification path
  (the platform's notification owner post NOTIFY-CHAT cutover).

## Worklist (TERM-1..12)

Decomposed file-disjoint where possible: TERM-1/2 (engine+local PTY, the core),
TERM-3 (widget), TERM-4/5/6 (splits/tabs/broadcast — surface), TERM-7 (mackesd worker,
mesh-side), TERM-8 (remote UX), TERM-9 (scrollback/clipboard), TERM-10 (layouts),
TERM-11 (palette/look), TERM-12 (keys/titles/watch). TERM-7 is mesh-side (mackesd) and
parallels the surface work; the rest are `mde-term-egui`.

## Acceptance (top-level, per §7 runtime-observable)

- Launch `mde-term-egui`: a real shell runs in a pane, renders through the VT engine,
  accepts input, shows output + colors; the surface is reachable from the dock/shell.
- Split H/V to arbitrary depth; drag dividers; zoom a pane; drag-rearrange; tabs each
  hold their own tree.
- Broadcast to all / a named group / off, with the indicator.
- Open a terminal **on a remote mesh peer** from the roster picker — a real shell on that
  node over the overlay (or an honest typed error if unreachable).
- Save + relaunch a named layout (incl. a remote pane); it syncs to another node.
- Unlimited scrollback + regex search; URLs open the mesh browser, paths open Files.
- Carbon-derived palette default + switch to a classic preset; chrome is pure Carbon
  (no raw hex); Terminator-default keybinds work + rebind; editable titles + activity
  watch fires a notification.
- Per unit: `build/test/clippy -p <crate>` green, `lint-layered-tiers.sh` clean.

## Out of scope

- Terminator's **Python plugin API** (no embedded interpreter; a native extension story,
  if ever, is a separate epic).
- GTK/GNOME-specific integrations (drag-out-to-new-window across processes, GTK theming).
- A full named-profile manager (locked out at Q13 — one look + palette picker + knobs).
- Truly unbounded scrollback (soft cap for memory safety; "unlimited" in practice).

## Risks

- **Airgap dep add** (Q1): the VT crate must be fetchable at lock-gen then vendored;
  verify early (TERM-1) — if genuinely impossible, fall back to `vte` (parser-only) or a
  hand-rolled model, and flag it (do not silently reduce fidelity).
- **Mesh PTY latency/half-open** (Q3/9): the broker must honest-timeout + clean up a
  dropped remote session (mirror mesh_mount's typed-gating discipline), never wedge a pane.
- **egui glyph performance** at large grids/scrollback — upload cells per paint, not per
  frame; cache the font atlas.
