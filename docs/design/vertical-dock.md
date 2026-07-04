# VDOCK — the left vertical auto-hide dock (replaces the bottom bar)

Operator-locked 2026-07-04 (25-Q `/plan` survey). A pivot: the horizontal Win10 bottom
taskbar (NAVBAR-W10 + PICKER-1/2) is **replaced entirely** by a **left-edge, full-height,
auto-hide vertical dock**. The clock is removed (→ a Timers & Alarms feature), status
icons become **2×2 quads**, and Settings/Show-Desktop join a matching 2×2 system quad.

## Locked decisions (25)

| # | Area | Lock |
|---|------|------|
| 1 | Placement | **Left edge, full height, auto-hide** |
| 2 | Width/cols | **Single-column app picker; 2×2 grids only for the status/system clusters** (~48px wide) |
| 3 | Groups | **Stacked top→bottom** in order (Comms→…→Media); single-column icons per Q2 |
| 4 | Labels | **Horizontal labels above each group** (no rotation — the dock is vertical) |
| 5 | Clock | **Removed as a clock** → replaced by a **Timers & Alarms** feature (#16/#20) |
| 6 | Status grid | **2×2 quads, stacked** — Chat/BT/Vol/Batt as one quad, Status/Signal/Peers/Sessions as the next |
| 7 | System quad | **A 2×2 quad: Settings · Show-Desktop · Lock · Power** (#17), sized to match the status icons |
| 8 | Cluster place | **Bottom** — Workbench lead pinned top, app groups scroll/overflow in the middle, status+system quads anchored bottom |
| 9 | Auto-hide | **Hotkey + pin only** (NO hover-reveal) — hidden by default |
| 10 | Active mark | **Left-edge accent bar** on the active surface's icon (+ subtle fill) |
| 11 | App icons | **Icon-only, no tooltip** (the group's horizontal label names the section) |
| 12 | Icon sizes | **App icons larger (~24px), quad icons smaller (~18px)** |
| 13 | Hotkey | **Super (Meta) toggles** the dock |
| 14 | Motion | **Slide in from the left** (~200ms Carbon Motion) |
| 15 | Status click | **Routes to the owning surface** (no flyouts — Chat→Chat, Vol→System audio, …) |
| 16 | Clock content | **Timers & Alarms** (create countdown timers + set alarms; alerts via CHAT-FIX-2) |
| 17 | System quad | Settings, Show-Desktop, Lock, Power |
| 18 | Power | **A Lock/Suspend/Reboot/Shutdown menu** off the Power cell; reboot/shutdown typed-armed |
| 19 | Notifications | **On the Chat status icon** — its badge counts unread local events (CHAT-FIX-2) + peer messages; click → Chat feed |
| 20 | Timers home | **A clock-glyph icon that IS the Timers & Alarms surface** (reads as a clock, opens Timers on click) |
| 21 | Accents | **Both** — the group's horizontal label + divider take the group accent (PICKER-2 tokens) AND a thin accent left-rail stripe per group |
| 22 | Overflow | **'…' more-popup** at the bottom of the app area when groups exceed the height |
| 23 | Dimensions | **~48px wide, ~24px app icons, ~18px quad icons** |
| 24 | Background | **Solid Carbon-dark panel** + a hairline right-edge divider |
| 25 | Transition | **Replace the bottom bar entirely** — rip out the horizontal taskbar; the vertical dock IS the shell chrome |

## Reconciliation notes
- **Q2 vs Q3:** the APP picker is a single vertical column of icons; the 2-wide grids apply
  only to the status + system CLUSTERS (the quads). Groups stack single-column with a
  horizontal accent header + a left-rail accent stripe.
- **Q15 (route, no flyouts):** the vertical dock drops W10-4's tray flyouts — status icons
  route to their full surface. (The flyout code can be retired or kept dormant.)
- Surfaces, routing, accent tokens (PICKER-2), the battery-fill/glyph logic, and the
  grouped-picker membership (Workbench lead + 6 groups; Settings/Desktop out to the system
  quad) all carry over from the horizontal work — this is a **re-layout**, not a rebuild.

## Architecture (mde-shell-egui)
- **`dock.rs` → a vertical `dock()`** replacing `taskbar()`: a left `SidePanel`/`Area`
  ~48px wide, full height, solid Carbon-dark, hairline right divider. Zones: Workbench lead
  (top) · app groups (middle, single-column, stacked, horizontal accent labels + left-rail
  stripe + divider, left-edge active accent bar, '…' overflow) · status quads + system quad
  (bottom). Icon-only, no tooltips; app 24px / quad 18px.
- **Auto-hide + reveal** (`main.rs`): the dock is a slide-in `Area` hidden off the left by
  default; **Super toggles** it (in the hotkey path), a **pin** toggle holds it open; slide
  motion via the Motion tokens. The central content fills the full width when hidden.
- **Status quads** (rework `tray.rs`): the tray becomes stacked 2×2 quads (Chat[badge]/BT/
  Vol/Batt, then Status/Signal/Peers/Sessions), each cell routing to its surface (#15);
  Chat's badge counts CHAT-FIX-2 unread (#19). No flyouts.
- **System quad**: Settings·Show-Desktop·Lock·Power (2×2). Power opens the armed
  Lock/Suspend/Reboot/Shutdown menu (#18); Lock drops the curtain; Show-Desktop = the
  existing route; Settings → System.
- **Timers & Alarms surface** (new): a clock-glyph status cell (shows time as its glyph)
  opens a Timers & Alarms surface — countdown timers + alarms that fire notifications via
  the CHAT-FIX-2 producer. A new `Surface::Timers` + panel.
- **Rip out the horizontal bar** (#25): remove the bottom `TopBottomPanel`; the shell body
  fills top-to-bottom with the vertical dock on the left. Update the shell's layout tests.

## Acceptance (runtime-observable)
- The shell shows NO bottom bar; a left vertical dock slides in on Super (and stays with the
  pin), hidden otherwise; content fills the freed space when hidden.
- App groups stack single-column with horizontal accent labels + left-rail stripes; the
  active surface shows a left-edge accent bar; groups overflow into a '…' popup.
- Status icons are stacked 2×2 quads that route to their surface; Chat's badge reflects
  CHAT-FIX-2 unread; the system quad (Settings/Desktop/Lock/Power) matches the quad size;
  Power opens the armed menu.
- The clock-glyph opens Timers & Alarms; a timer fires a notification.
- All Carbon tokens (§4); every surface still reachable exactly once.

## Risks
- **Big re-layout of dock.rs/tray.rs/main.rs** — coordinate; the layout harness (PICKER-3)
  is reusable to pin the vertical geometry. Serialize dock work.
- **Auto-hide + DRM seat** — the slide-in Area over a DRM shell must not steal focus/input
  from surfaces when hidden; verify input passthrough when the dock is off-screen.
- **Super as the toggle** — Super is also the leader chord (hotkeys.rs); reconcile tap-to-
  toggle vs hold-as-leader so they don't collide.
- **Timers reliability** — alarms must fire even when the dock/surface is closed (a daemon
  or shell-side timer that survives surface switches).

## Out of scope (v1)
- A settings toggle back to the bottom bar (#25 = replace entirely).
- Multi-monitor dock placement; right/top docks.
- Rich timer features (repeating schedules, world clocks) beyond countdowns + alarms.

## Tasks → `docs/WORKLIST.md` VDOCK-1..6.
