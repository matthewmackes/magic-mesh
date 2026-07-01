# Notification Hub redesign (operator-locked 2026-06-30)

Locked via the `/loop /polish /ship /no-flinch` session surveys (Round 1 + Round 2
+ the 10-question viewer survey). Builds on the shipped **NOTIFY-STATUS-STRIP**
(Carbon-icon header/severity/lighthouse-shield footer). All changes land in
`crates/workbench/mde-workbench/src/bin/mde-notify-center.rs` (+ its helper
modules) — so they ship as SEQUENTIAL units (one mde-workbench-bin editor at a
time), not in parallel.

## Build sequence
- **NOTIFY-REDESIGN-A** — tabs + message-first flat list *(in flight)*
- **NOTIFY-REDESIGN-B** — Voice & Music icon in the footer
- **NOTIFY-REDESIGN-C** — the click-opened notification viewer

---

## A — Tabs + message-first list (Round 1)
- **Tabs at the TOP**, a **segmented/pill** control: **Notifications** (default) +
  **Clipboard**. The existing Clipboard section (`clips`/`ClipRow`) MOVES into the
  Clipboard tab; the Notifications tab holds the notification list.
- **Status footer persists on BOTH tabs** — the severity strip + the
  `lighthouses_footer` (+ the B Voice&Music icon) stay visible regardless of tab.
- **Rows = MESSAGE-FIRST**: the message/body is the prominent primary line. At rest
  a row is message-only (severity icon + message). **No hover-reveal** — the source
  detail moves to the C viewer (clicking a row opens it; for A, click is a
  no-op/select).
- **FLATTEN the grouping**: drop the per-source group headers AND the same-source
  stacking/collapse — render ONE chronological newest-first flat list. (Operator
  accepted losing same-source stacking.) Remove the now-dead grouping/stack code.

## B — Voice & Music icon (Round 2)
Convert the Music (`MusicNow`) + Voice (`VoiceStatus`) status BARS into
lighthouse-style ICONS, placed to the RIGHT of the Lighthouses under a **"Voice &
Music"** heading (the footer becomes two labeled sections side by side).
- **Two icons** (Voice + Music), each its own state. Reconciliation of the two
  survey answers ("hide whichever is idle" + "always visible, muted when idle"):
  **show greyed/muted when the service is present-but-idle; HIDE only when truly
  offline/absent** (no music daemon / no voice agent). The "Voice & Music" section
  follows the lighthouse-footer convention (hide if BOTH are absent).
- **Rich state by colour + motion**: Music — playing = accent (gentle pulse),
  paused = accent static, idle = muted, offline = hidden. Voice — in-call = accent
  (blink), registered/ready = ok-tone, idle = muted, offline = hidden. All via
  `mde-theme` tokens + the established beacon/motion idiom.
- **Music click → a small popover** with the now-playing track + transport
  (prev / play-pause / next) — keeps the transport reachable in-Hub. **Voice click →
  open the Voice HUD** (or a small call-status popover).

## C — Reusable hub detail viewer (10-question survey + reuse directive)

**REUSE (operator 2026-06-30): this is a GENERIC detail viewer reused for EVERY hub
item that can show more detail — not notification-specific.** Build it as one
component: a `DetailContent { title, severity (optional → tints the header band),
fields: [(label, value)], body, raw (mono), actions: [Action] }` + the shared
center-modal shell (hero band + elevation, scrim+blur, layered Esc/scrim/X,
responsive ~70%/≤760px, raw-block-mono, selectable + Copy-all/Copy-raw). Each item
type provides its own `DetailContent`; clicking the item opens the shared shell.

**Consumers (each item type → what the viewer shows + its actions):**
- **Notification** (primary): the full spec below (title/body/severity/source/host/
  ts/id + raw; Copy, Mark read/unread, Dismiss, Open source, Mute source).
- **Clipboard row** (`ClipRow`): full verbatim clip text in the mono raw block +
  source-node + age; actions: Copy (→ `wl-copy`), Dismiss/clear-this.
- **Lighthouse beacon**: host, health tier, relay/direct path, last-seen; action:
  Open the Lighthouses tab (the existing `OpenLighthouse` deep-link).
- **Voice / Music** (the B icons): now-playing track / call detail in the body;
  actions = the transport (prev/play-pause/next) / call controls + Open the
  Music player / Voice HUD. (This SUBSUMES B's "popover" — the reused viewer IS
  the detail surface; a lighter inline popover is optional if the full viewer feels
  heavy for quick transport.)

Build order: the viewer shell + the **notification** consumer first (C1); wire the
clip / lighthouse / voice-music consumers into it next (C2) — all sequential in the
same bin.

### Notification consumer detail
Clicking a notification row opens the shared viewer with this content.
- **Trigger**: click anywhere on the row (the hover-peek is dropped — see A).
- **Modal**: centered, **dimming scrim + blur** backdrop (fall back to scrim if a
  cheap blur isn't available in the iced/cosmic stack). **Dismiss layered**: X
  button + scrim-click + Esc all close JUST the viewer; a SECOND Esc (viewer
  already closed) closes the Hub.
- **Size**: responsive ~70% of the window, capped ~760px wide, sensible min.
- **Hero look**: a **severity-coloured header band** (large severity icon + title)
  over an **elevated card with a drop shadow**.
- **Content** (everything): title, full message body, severity, source, host,
  absolute + relative timestamp, id, PLUS a **raw/structured block** (the verbatim
  event).
- **Mono font**: ONLY the raw block is monospace; the rest is Carbon sans (but all
  text is selectable).
- **Copy**: all text selectable, PLUS a **Copy-all** button (full detail as text)
  and a **Copy-raw** button (the raw block). Reuse the `wl-copy` path the clipboard
  feature uses.
- **Actions — full set** (shown only when applicable): Copy-all, Copy-raw, **Mark
  read/unread** (per-item, using the `read` field), **Dismiss this** (remove by id),
  **Open source** (deep-link to the owning Workbench panel where the `source` maps —
  e.g. a lighthouse alert → the Lighthouses tab), **Mute source** (suppress future
  from this source via the SoundSettings muted-sources store).
- **Action layout**: a **footer button row** (Open source · Mark read · Dismiss);
  the **Copy-all / Copy-raw** buttons sit by the content/raw block; X top-right.

## Gates (each unit, §7)
Farm-built (`xcp-build.sh`, mde-workbench): `cargo test -p mde-workbench --lib` +
`--bin mde-notify-center`, `cargo clippy --all-targets`, rustfmt, +
`lint-carbon-tokens.sh` / `lint-motion.sh`. §4 tokens (no raw hex/metrics), §6
reuse the existing model/`mde_icon`/`wl-copy`, no new bus paths.
