# PLATFORM-INTERFACES — Construct + Car, the two platform interfaces (Apple-HIG-principled)

> **THE interface design authority.** Design locks from the 50-question operator
> survey, 2026-07-22. This document defines the requirements for the platform's
> only two interfaces — **Construct** (the workstation) and **Car** — under the
> platform design standard, **Apple's Human Interface Guidelines applied as
> principles** (<https://developer.apple.com/design/human-interface-guidelines>).
> Authority: `AI_GOVERNANCE.md` §4 (which names this doc), §0 (Secure, Simple,
> No-Fixed-Center), §7 (Definition of Done). Supersedes `win10-taskbar.md`
> (WIN10-HYBRID chrome), `auto-mode-sync3.md` (SYNC 3 Car look), and the
> paradigm docs listed in §5 — all bannered + archived to `docs/design-archive/`.
> Build epics: **WL-UX-006** (Construct) + **WL-UX-007** (Car) in
> `docs/platform/WORKLIST.md`.

⚠️ **Design-lock reversal, intentional and operator-approved (2026-07-22).**
The Win10-structure chrome direction (win10-taskbar.md, itself a reversal of
VDOCK) is retired by this document. Do **not** re-implement a bottom taskbar,
tray flyouts, or a Start-style panel. The front-door doc's iPadOS-home locks
(Q86/89 there) are *revived and re-scoped here* against the live egui shell.
Two prior locks are explicitly reversed **in lockstep** by this survey:
front-door Q86/89's "no dock" is **confirmed** (Construct has no dock), and
NAVBAR-W10's "kill the top status bar" is **reversed** (Construct has a slim
top status bar).

---

## 1. The standard — Apple HIG, distilled for egui implementers

The HIG is applied as **principles, not pixels** (survey Q1). We do not imitate
Apple's appearance; we hold every surface to the HIG's quality bar, implemented
through the shared `mde-egui` `Style`/`Motion` modules (the sole look source,
§4) and the Mackes-Carbon icon set (kept, operator lock). The airgapped-safe
distillation below is the in-repo statement of the standard; per-requirement
citations name the HIG section they derive from (survey Q43: both).

**P1 — Hierarchy & deference.** Content first; chrome recedes. Persistent
chrome earns its pixels (one slim status bar, nothing else). Elevation and
grouping express hierarchy, not decoration. *(HIG › Foundations › Layout)*

**P2 — Clarity of type.** One semantic type ramp (Large Title → Caption),
used by role, never by ad-hoc size. Text is legible at its viewing distance —
glance-range in Car, arm's length on a seat. *(HIG › Foundations › Typography)*

**P3 — Consistency of components.** One NavigationBar, one Toolbar, one
Sidebar, one Sheet, one Popover — shared components adopted everywhere, so a
user who learns one app has learned them all. *(HIG › Components)*

**P4 — Direct manipulation & feedback.** Every interaction acknowledges
immediately (pressed states, live drags, interruptible transitions). Nothing
blocks without progress; nothing succeeds silently that the user meant to see.
*(HIG › Foundations › Feedback; Motion)*

**P5 — Deliberate modality.** Modal UI is rare, purposeful, and dismissible
by gesture/Escape. Sheets for scoped tasks, popovers for transient choices,
alerts only for consequences. *(HIG › Patterns › Modality)*

**P6 — Fluid, honest motion.** Motion clarifies spatial relationships
(zoom-from-tile says "this app came from here"). Springs are interruptible;
reduced-motion is respected; nothing animates that carries no meaning.
*(HIG › Foundations › Motion)*

**P7 — Input parity.** Touch and pointer/keyboard are both first-class
(survey Q2: iPadOS structure + macOS pointer manners). Every gesture has a
keyboard/pointer equivalent of equal rank. *(HIG › Inputs: Gestures,
Pointing devices, Keyboards)*

**P8 — Honest data.** Absent data reads as absent ("—", plain descriptors),
never fabricated. (House rule predating this doc; the HIG's "clarity" applied
to live systems.)

**P9 — Consistent appearance.** Dark, always (survey Q3). HIG dark-mode
guidance governs contrast and elevation within the single Quazar-dark
appearance. *(HIG › Foundations › Dark Mode, Color)*

**P10 — Glanceability under motion (Car).** In the vehicle, information is
consumable in a glance and interaction depth is capped; the interface defers
to driving. *(HIG › Platforms › Designing for CarPlay — as principles)*

---

## 2. Part I — Construct (the workstation interface)

Construct is the seat experience of `mde-shell-egui`: DRM-native, egui-only,
full-screen-first. Structure is iPadOS-derived; pointer/keyboard manners are
macOS-derived (Q2). Identity is Quazar-dark (Q1, Q3): `Style::BG #16161A`,
azure accent `#5B8CFF`, the 8 categorical group accents, Carbon glyphs.

### 2.1 Foundation locks

| # | Lock | HIG anchor |
|---|---|---|
| Q1 | HIG as **principles**, not a pixel clone | Foundations (all) |
| Q2 | **iPadOS structure + macOS pointer manners** | Designing for iPadOS / macOS |
| Q3 | **Dark-only** appearance (Quazar-dark) | Dark Mode |
| Q4 | **Inter** carries the HIG type ramp (SF stand-in); **IBM Plex Mono** for code/terminal content | Typography |

### 2.2 Home — the springboard (Q5–Q9)

- **Q5 — Persistent home.** A paged icon grid is the **base layer**: the seat
  boots to it, and leaving any app lands on it. It draws over the existing
  wallpaper/backdrop. The collapsed "session EmptyState" is retired.
  *(Designing for iPadOS › The Home Screen)*
- **Q8 — Pages ARE the groups.** Home pages are generated from the 8
  `LAUNCHER_GROUPS` (Mesh Control · Desktop & Session · Media · Files & Data ·
  Web · Developer Tools · Comms · System), one page per group, in taxonomy
  order. **No free arrangement, no folders, no arrangement state.** The
  compile-time "every Surface exactly once" guard becomes the page guard.
- **Q6/Q7/Q9 — No widgets, no live-data cards.** Home is pure icons. Live data
  lives in the surfaces that own it (Maps, Workbench) — nothing new.
- **Q10 — No dock.** (Confirms front-door Q86/89.) Pinned-app state
  (`launcher_pins`) retires with it.
- Tile treatment (Q22): rounded-rect plate, per-group accent background, white
  Carbon glyph, label beneath. *(App Icons — as principles: one silhouette
  language, no photorealism)*
- Page indicator dots; swipe / Page keys / click-drag to page.

### 2.3 Persistent chrome (Q11–Q12)

- **Q12 — Slim top status bar (~24px).** Clock + date left; mesh grade, network,
  power, alert count right — fed by the existing `status.rs` StatusSegments
  rollups. Surfaces may declare full-screen auto-hide (VDI always does).
  *(Status Bars)* **This reverses NAVBAR-W10's top-bar kill, deliberately.**
- **Q11 — The system gesture contract, with pointer parity:**

| Intent | Touch | Pointer / keys |
|---|---|---|
| Home | bottom-edge swipe up | **Super** tap |
| App switcher | bottom-edge swipe up + hold | **Super+Tab** (hold to browse) |
| Spotlight | pull-down on home grid | **Super** (on home) / type-to-search |
| Control Center | top-right pull-down | click status-bar right cluster |
| Notification Center | top-left/center pull-down | click status-bar clock |

  One contract table, one drain site (`gestures.rs` edge-swipe channel). The
  taskbar-reveal hot edge retires. Over a focused VDI session, edge gestures
  require dwell (second-swipe confirm); Super chords always work.
  *(Gestures; Pointing devices)*

### 2.4 System overlays (Q13–Q16)

- **Q13 — Control Center** (full): volume, display, network/mesh, bluetooth,
  Construct↔Car toggle, VDI session controls, file-operation progress. Replaces
  every tray flyout. Scrim material, sheet-style dismiss.
- **Q14 — Notifications:** HIG banners (top, transient, reusing toast plumbing)
  + a pull-down **Notification Center** with grouped history and clear-all.
  *(Notifications)*
- **Q15 — Spotlight:** the Front Door engine (producers, ranking, keyboard
  flow **byte-identical**) reskinned as a centered floating search field.
  *(Searching)*
- **Q16 — App switcher:** card grid of open surfaces with **snapshot-on-leave**
  previews (never live-render; plate fallback when no snapshot), flick-up to
  close (= leaves recents), Super+Tab / swipe-up-hold.
  *(Multitasking — as principles)*

### 2.5 Apps (Q17–Q20)

- **Q17/Q18 — Full-screen only.** One surface per frame (the engine's native
  model). No Split View, no Slide Over.
- **Q19 — Shared nav components.** `NavigationBar` (title + back + actions),
  `Toolbar`, `Sidebar` in `mde-egui`; **all 17 surfaces adopt** (farm sweep).
  *(Navigation Bars; Toolbars; Sidebars)*
- **Q20 — Sheets + popovers everywhere.** Shared `Sheet` (detents,
  drag-to-dismiss) and `Popover`; all surface dialogs migrate in the sweep.
  *(Sheets; Popovers)*

### 2.6 Visual system (Q21–Q24)

- **Q21 — Scrim materials.** Overlays sit on layered translucency (semi-opaque
  dark scrims); **no live blur** on the GLES/DRM path. *(Materials — honest to
  the render budget)*
- **Q23 — Radii ladder** ~6/10/16/26 with a concentric-nesting rule.
- **Q24 — Full HIG transitions:** zoom-from-tile open/close, interruptible
  spring page swipes, sheet detent physics — on the existing MOTION-DRM spring
  substrate. Reduced-motion respected throughout. *(Motion)*

### 2.7 System surfaces (Q25–Q28, Q50)

- **Q25 — Curtain (lock):** minimal restyle, tokens only. **Security/auth
  behavior is SACRED — zero logic diffs.**
- **Q26 — OSK:** HIG restyle (caps, radii, type); raise/dismiss behavior kept.
  *(Virtual keyboards)*
- **Q27 — System = HIG Settings:** grouped sidebar → detail pane, inline
  search; built from the Q19 components. Profile picker shows the two profiles.
- **Q28 — VDI session = an app.** Full-screen in the switcher, home gesture
  leaves it, status bar auto-hides over it. **The full-native-resolution
  guarantee is SACRED** (zero reserved chrome over a focused session). The
  quasar-vdi-desktop "thin chrome bar" lock is re-expressed as status bar +
  Control Center.
- **Q42 — Profiles:** `LayoutProfile` = **Construct + Car** only. Tablet folds
  into Construct (hardware formfactor flips keep adjusting density/OSK *within*
  Construct; formfactor ≠ profile). Persisted `"workstation"`/`"tablet"`
  configs migrate silently via serde aliases.

---

## 3. Part II — Car

Car is the in-vehicle mode: **CarPlay-principled** (Q29) — dark, glanceable,
capped-depth — while keeping this platform's two differentiators: the
**physical-keyboard-first contract** (a driver never needs the touchscreen)
and the **always-visible instrument cluster** (operator lock, Q32).

### 3.1 Identity & structure

- **Q30 — Palette:** the SYNC3 dark tokens are KEPT as the Car appearance
  (`SYNC3_BG #04070B`, surfaces `#12171E/#1C242E`, accent `#2E9BE6`), installed
  only while Car is active, absent from the theme picker. (The SYNC 3 *doc* is
  superseded; its palette survives here.)
- **Q34 — Always dark.** No day/night flip; the Nav map may choose its own
  day/night tile styles independently. *(Designing for CarPlay › Appearance)*
- **Q33 — Instrument strip:** the left driver's-third strip — digital
  speedometer above, selectable engine/status tiles below (48-item catalog,
  persisted selection) — renders on **every** Car screen. **Requirement: its
  telemetry folds fresh on every Car-mode frame** (the fold self-throttles at
  ~2 Hz), never only while a Maps surface is focused. Speed + engine reporting
  are always visible (Q32).
- **Q31 — Dashboard home:** the remaining two-thirds is a CarPlay-Dashboard-
  style screen of **persistent split cards** — Nav map card, Media/now-playing
  card, glance card — plus a smaller app strip to open the full apps.
  *(Designing for CarPlay › Dashboard — as principles)*

### 3.2 App roster (Q32)

Six apps (was 7 tiles): **Nav** (MapsLocation), **Media**, **Music** (new,
split from Media), **Comms** (Phone merged in; calls + alerts + messages),
**Vehicle** (MapsLocation › Vehicle), **Settings** (System). The Airspace
*tile* is dropped (Airspace remains a Maps tab, reachable from Nav). Key
bindings (`CarAction`) re-map accordingly; Music gains media-transport keys.

### 3.3 Behavior (Q35–Q36)

- **Q35 — Glance rules + soft in-motion limits.** Codified requirements:
  44px+ targets, glance-range type sizes, interaction depth ≤2 while moving.
  When MG90-reported speed exceeds the threshold: lists shorten, the OSK is
  suppressed, destructive prompts defer until stopped. **No hard lockouts** —
  the keyboard-first stance is the safety model.
- **Q36 — Entry/exit:** one-tap Construct↔Car toggle (+ persisted boot
  profile). **No auto-enter, no auto-suggest.**
- Honesty (P8): no fabricated readings — GPS tiles fix-gated, "—" without
  data, simulated seed only when no mirror exists (never presented as live).

---

## 4. Acceptance (Q48)

- **Construct (WL-UX-006):** screenshot/pixel proof on the `.15` DRM seat —
  springboard pages (all 8), status bar, Control Center, Notification Center,
  Spotlight, app switcher with real snapshots, zoom transitions, VDI
  full-resolution with auto-hidden bar. Operator visual signoff.
- **Car (WL-UX-007):** live proof with the MG90 vehicle mirror online —
  dashboard cards live, instrument strip **fresh on every Car screen**,
  soft in-motion limits engage above threshold, one-tap toggle. Operator
  signoff.
- Both: workspace build + tests + clippy/fmt green; post-cutover grep gate
  (zero `taskbar` identifiers in production code); `lint-style-leaks`,
  `lint-doc-supersession`, `lint-worklist` green.

## 5. Supersessions & the design-reference purge (Q37–Q41, Q44)

- This doc is the **one combined authority** (Q37): Part I Construct, Part II
  Car. Interface names are **"Construct"** and **"Car"** (Q44).
- **Purged (banner + move to `docs/design-archive/`, Q38/Q39):** the pure
  paradigm docs — win10-taskbar, win7-desktop-survey, vertical-dock,
  dock-accent, front-door, app-launcher-rethink, apps-launcher,
  start-menu-redesign (+ .dc.html.note), planes, picker-groups, motion-audit,
  motion-guide, motion-system, cosmic-magic-mesh-egui, platform-survey
  (+ answers), navigation-interface, auto-mode-sync3.
- **Re-anchored:** chrome-shaped feature docs keep their feature content and
  gain banners pointing their look-and-feel sections here. Subsystem docs lose
  foreign-paradigm framing opportunistically as they are next touched.
- **Kept (Q40):** the Browser Material-3 carve-out in `AI_GOVERNANCE.md` §4 —
  the Browser remains the one Material island; HIG principles govern
  everything else.
- **Governance (Q41):** §4 names the HIG-principles standard and this doc;
  ADR appended to `docs/DECISIONS.md`. Carbon **icon set** kept platform-wide.

## 6. Delivery (Q45–Q49)

Full implementation fan-out (Q45): epics **WL-UX-006/007** (Q46), 28 units + 2
operator gates — plan of record
`/root/.claude/plans/the-workstation-interface-should-cozy-minsky.md`.
Parallel tracks (Q49): docs + `mde-egui` foundation immediately; the atomic
shell cutover (which **deletes** the Win10 chrome, Q47 — no legacy flag) waits
for in-flight same-crate work to land. WL-UX-001 is superseded-retired;
WL-UX-005 folds into WL-UX-006.
