# PLATFORM-INTERFACES — Construct + Car, the two platform interfaces (Apple-HIG-principled)

> **THE interface design authority.** Design locks from the 50-question operator
> survey, 2026-07-22, amended by the 2026-07-26/30 authority cleanup. This
> document defines the requirements for the platform's
> only two interfaces — **Construct** (the workstation) and **Car** — under the
> platform design standard, **Apple's Human Interface Guidelines applied as
> principles** (<https://developer.apple.com/design/human-interface-guidelines>).
> Authority: `AI_GOVERNANCE.md` §4 (which names this doc), §0 (Secure, Simple,
> No-Fixed-Center), §7 (Definition of Done). Supersedes `win10-taskbar.md`
> (WIN10-HYBRID chrome), `auto-mode-sync3.md` (SYNC 3 Car look), and the
> paradigm docs listed in §5 — all bannered + archived to `docs/design-archive/`.
> Active build epics: **WL-UX-009** (shared Construct language),
> **WL-UX-012** (Construct taskbar/Home), and **WL-FUNC-017** (Maps/Car) in
> `docs/platform/WORKLIST.md`; WL-UX-006/007 are archived predecessors.

⚠️ **Construct chrome authority update, intentional and operator-directed.**
The 2026-07-29/30 operator taskbar review, recorded in WL-UX-012, supersedes
the bottom-centered Dock prohibition for **Construct only**. Construct now has
a full-width, 48px, Windows 11-inspired taskbar: icon-only Start/Search, Back,
and Home controls at left; user-managed workspace pins centered on the physical
screen; and placement at right. Start opens the existing Front Door search and
is not a Start menu. The taskbar is not a tray flyout or a second launcher
surface. Car chrome and focused-VDI full-pixel behavior remain unchanged. The
icon-free Bing-wallpaper Home and the slim top status bar remain authoritative.

---

## 1. The standard — Apple HIG, distilled for egui implementers

The HIG is applied as **principles, not pixels** (survey Q1). We do not imitate
Apple's appearance; we hold every surface to the HIG's quality bar, implemented
through the shared `mde-egui` `Style`/`Motion` modules (the sole look source,
§4) and the shared icon registry. Carbon is neither a theme nor an icon
requirement; retained Mackes-Carbon-compatible SVGs are one optional registry
source, alongside license-cleared replacements. The 2026-07-26 icon V2 survey
sets the icon treatment: core platform app/service/role icons use simple
linework with at most three adapted product/service-associated colors and no
plates; toolbar/action/status icons remain monochrome and are color-coded by
semantic state. The airgapped-safe
distillation below is the in-repo statement of the standard; per-requirement
citations name the HIG section they derive from (survey Q43: both).

**P1 — Hierarchy & deference.** Content first; chrome recedes. Persistent
chrome earns its pixels: one slim status bar and the reserved Construct
taskbar (or the Left rail while taskbar placement is moved).
Elevation and grouping express hierarchy, not decoration. *(HIG › Foundations ›
Layout)*

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
(zoom-from-tile says "this app came from here"). Apple-like springs are
interruptible, expressive motion is the default, and nothing animates that
carries no meaning. Reduced-motion handling is optional compatibility work, not
a core product feature.
*(HIG › Foundations › Motion)*

**P7 — Input parity.** Touch and pointer/keyboard are both first-class
(survey Q2: iPadOS structure + macOS pointer manners). Every gesture has a
keyboard/pointer equivalent of equal rank. *(HIG › Inputs: Gestures,
Pointing devices, Keyboards)*

**P8 — Honest data.** Absent data reads as absent ("—", plain descriptors),
never fabricated. (House rule predating this doc; the HIG's "clarity" applied
to live systems.)

**P9 — Consistent appearance.** Quazar Dark and production Quazar Light both
ship. HIG appearance guidance governs contrast, elevation, and semantic color in
both schemes; Car remains an explicit always-dark AutoSync3 profile. *(HIG ›
Foundations › Dark Mode, Color)*

**P10 — Glanceability under motion (Car).** In the vehicle, information is
consumable in a glance and interaction depth is capped; the interface defers
to driving. *(HIG › Platforms › Designing for CarPlay — as principles)*

---

## 2. Part I — Construct (the workstation interface)

Construct is the seat experience of `mde-shell-egui`: DRM-native, egui-only,
full-screen-first. Structure is iPadOS-derived; pointer/keyboard manners are
macOS-derived (Q2). Identity is Quazar: Dark starts from `Style::BG #16161A`,
Light is a production first-class appearance, both share azure accent
`#5B8CFF`, the categorical group accents, and registry-backed glyphs.

### 2.1 Foundation locks

| # | Lock | HIG anchor |
|---|---|---|
| Q1 | HIG as **principles**, not a pixel clone | Foundations (all) |
| Q2 | **iPadOS structure + macOS pointer manners** | Designing for iPadOS / macOS |
| Q3 | **Quazar Dark + production Quazar Light** appearances | Appearance / Color |
| Q4 | **Kdam Thmor Pro** carries the HIG type ramp as the standard platform UI face; **IBM Plex Mono** remains for code/terminal content, with Inter retained only as a proportional fallback | Typography |

### 2.2 Home — the springboard (Q5–Q9)

- **Q5 — Persistent Home.** One untitled icon-free Home is the **base layer**:
  the seat boots to it, and leaving any app lands on it. It draws the daily
  Microsoft Bing wallpaper through the platform wallpaper policy/cache with
  cached and bundled fallbacks. The collapsed "session EmptyState" is retired.
  *(Designing for iPadOS › The Home Screen)*
- **Q8 — One desktop, taxonomy accents.** The single Home has no icon grid,
  pages, free arrangement, folders, or arrangement state. Surface taxonomy still
  exists as the canonical launcher catalog: it colors taskbar cells, Front Door
  search grouping, switcher affordances, and compile-time "every Surface exactly
  once" guards.
- **Q6/Q7/Q9 — No widgets, no live-data cards.** Home is wallpaper plus passive
  system identity; live data lives in the surfaces that own it (Maps, Workloads,
  Media, Mesh Teams) and in governed overlays such as Control Center.
- **Q10 — Persistent Construct taskbar.** A full-width, 48px, reserved-space
  taskbar is always visible in Construct except where a focused immersive surface
  has an explicit full-pixel guarantee (for example focused VDI). Start opens
  Front Door search; it never opens a Start menu. Every launchable surface remains
  reachable through the searchable pin catalog and overflow; user pin order is
  authoritative.
- Taskbar treatment (Q22): square screen-edge geometry, 40px targets, icon-only
  persistent controls, a single focused-workspace underline, shared Dark/Light
  material, and the Windows-style clock/icon tray. The optional Left placement
  keeps the same pin order and detailed top status strip. *(App Icons — as
  principles: one silhouette language, no photorealism)*
- No page indicator dots or page swipe; Tab/arrow keys, taskbar activation, and
  Front Door search open the complete launcher set directly.

### 2.3 Persistent chrome (Q11–Q12)

- **Q12 — Slim top status bar (~24px).** Clock + date left; mesh grade, network,
  power, alert count right — fed by the existing `status.rs` StatusSegments
  rollups. Surfaces may declare full-screen auto-hide (VDI always does).
  *(Status Bars)* **This reverses NAVBAR-W10's top-bar kill, deliberately.**
- **Q12b — Reserved taskbar/rail space.** Construct body layout reserves exactly
  48px for the Bottom taskbar or the Left rail width so navigation never overlays
  workspace controls. Immersive VDI can still request the full native-resolution
  exception named in Q28.
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
  `Toolbar`, `Sidebar` in `mde-egui`; **all canonical surfaces adopt** (farm
  sweep).
  *(Navigation Bars; Toolbars; Sidebars)*
- **Q20 — Sheets + popovers everywhere.** Shared `Sheet` (detents,
  drag-to-dismiss) and `Popover`; all surface dialogs migrate in the sweep.
  *(Sheets; Popovers)*

### 2.6 Visual system (Q21–Q24)

- **Q21 — Scrim materials.** Overlays sit on layered translucency (semi-opaque
  dark scrims); **no live blur** on the GLES/DRM path. *(Materials — honest to
  the render budget)*
- **Q23 — Radii ladder** ~6/10/16/26 with a concentric-nesting rule.
- **Q24 — Full HIG transitions:** zoom-from-tile open/close, the navigation-bar
  slide/melt morph, and sheet detent physics — on the existing MOTION-DRM spring
  substrate. Expressive Apple-like motion is the normal path; reduced-motion is
  optional compatibility work. *(Motion)*

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

- **Construct (WL-UX-009/WL-UX-012):** screenshot/pixel proof on the `.15` DRM
  seat — the untitled icon-free Bing-wallpaper Home, reserved full-width taskbar,
  status bar, Control Center, Notification Center, Spotlight, app switcher with
  real snapshots, zoom and navigation-bar transitions, Quazar Dark and Light,
  and VDI full-resolution with auto-hidden bar. Operator visual signoff.
- **Car (WL-FUNC-017):** live proof with the MG90 vehicle mirror online —
  dashboard cards live, instrument strip **fresh on every Car screen**,
  soft in-motion limits engage above threshold, one-tap toggle. Operator
  signoff.
- Both: workspace build + tests + clippy/fmt green; post-cutover grep gate
  (zero retired Win10-taskbar compatibility identifiers and no duplicate Start
  menu); `lint-style-leaks`,
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
- **Browser boundary (Q40):** the temporary host Browser controller may retain
  its local Material-3 direction while WL-ARCH-008 moves it to `browser-vm`.
  Construct owns the connection, unavailable, and diagnostic surfaces around
  the VM only; guest Chromium pixels and chrome receive no Construct styling.
  HIG principles govern every other Construct-owned surface.
- **Governance (Q41):** §4 names the HIG-principles standard and this doc;
  ADR appended to `docs/DECISIONS.md`. Carbon is not a required theme or icon
  source; every retained or replacement asset goes through the shared registry
  and license audit.

## 6. Delivery (Q45–Q49)

Full implementation fan-out (Q45) now follows the active worklist epics
**WL-UX-009/WL-UX-012/WL-FUNC-017**, with the archived WL-UX-006/007 plan used
only as historical evidence. Parallel tracks keep the shared `mde-egui`
foundation, taskbar/Home cutover, and Car/MG90 work independently verifiable;
the current Construct taskbar has no legacy Win10 compatibility flag or second
Start-menu path. WL-UX-001/005 are superseded-retired predecessors.

### 6.1 WL-UX-009 launchable-egui visual inventory

This is a design-authority evidence snapshot, not a second worklist. Its
machine-checked source is `Surface::ALL` and `SURFACE_VISUAL_INVENTORY` in
`crates/desktop/mde-shell-egui/src/surfaces.rs`: a new launchable surface cannot
skip this classification. `Adopted` means a governed primitive is in use;
`Partial` and `Gap` are remaining work, not completion claims. Shared tooltip
use is `Adopted` across the catalog because the style-leak gate forbids raw egui
hover text. Registry licensing remains a `Gap` until the shared asset audit is
complete, and Dark/Light proof remains `Partial` until deterministic renders
cover every listed surface.

| Launchable surface | Frame / nav / states / dialogs | Tooltips / icons / motion / lists | Appearance, licensing, and governed boundary |
|---|---|---|---|
| Fleet & Mesh | Shared app frame adopted; other chrome partial | Tooltip adopted; icon, motion, and dense-list migration partial | Dark/Light partial; licensing gap; normal Construct workspace |
| Infra as Code | Shared app frame adopted; other chrome partial | Tooltip adopted; icon, motion, and dense-list migration partial | Dark/Light partial; licensing gap; normal Construct workspace |
| Remote Sessions | Frame exception while focused | Tooltip adopted; picker/state/motion/list migration partial | Focused VDI keeps full guest pixels; Dark/Light and licensing proof partial/gap |
| Music | Shared app frame adopted; other chrome partial | Tooltip adopted; icon, motion, and dense-list migration partial | Dark/Light partial; licensing gap; normal Construct workspace |
| Media | Shared app frame adopted; other chrome partial | Tooltip adopted; icon, motion, and dense-list migration partial | Dark/Light partial; licensing gap; normal Construct workspace |
| Files | Shared app frame adopted; other chrome partial | Tooltip adopted; icon, motion, and dense-list migration partial | Dark/Light partial; licensing gap; normal Construct workspace |
| Browser | Shared app frame adopted; guest viewport boundary retained | Tooltip adopted; local controller states/icons/motion/lists partial | Construct owns Browser connection/unavailable/diagnostic chrome; `browser-vm` Chromium remains outside Construct styling |
| Bookmarks | Shared app frame adopted; other chrome partial | Tooltip adopted; icon, motion, and dense-list migration partial | Dark/Light partial; licensing gap; normal Construct workspace |
| Maps & Location | Shared app frame adopted; content-color exception | Tooltip adopted; shell chrome, motion, and lists partial | Maps content may retain its palette; Car and Dark/Light proof partial; licensing gap |
| Terminal | Shared app frame adopted; internal chrome partial | Tooltip adopted; tabs/toolbars/palettes/motion/lists partial | Dark/Light partial; licensing gap; normal Construct workspace |
| Phones | Shared app frame adopted; other chrome partial | Tooltip adopted; icon, motion, and dense-list migration partial | Dark/Light partial; licensing gap; normal Construct workspace |
| This Node | Shared app frame adopted; other chrome partial | Tooltip adopted; state/dialog/motion/list migration partial | Dark/Light partial; licensing gap; normal Construct workspace |
| Mesh Teams | Shared app frame adopted; other chrome partial | Tooltip adopted; frame/state/dialog/motion/list migration partial | Dark/Light partial; licensing gap; normal Construct workspace |

`System`, `Storage`, and `About` are legacy aliases that normalize to `This
Node`; they are not separate launchable surfaces. The Editor is a Documents-mode
embed in Mesh Teams, exposed through the always-visible shared Editor taskbar icon
as a direct launcher alias rather than a second surface authority. The inventory names only
the three approved rendering boundaries: focused VDI pixels, Maps content color,
and the Browser VM guest. No other row may claim an exception without updating
this authority, the registry, and its focused test.

### 6.2 Approved Construct / guest rendering boundaries

These are rendering ownership boundaries, not carve-outs from the shared
Construct language. Every non-exempt Construct-owned control still uses the
shared frame, typography, state, tooltip, icon, and motion primitives.

| Surface | Construct owns | Boundary owner | Guardrail |
|---|---|---|---|
| Focused VDI | Session picker, attach/reconnect flow, unavailable and diagnostic states, and input/session policy before focus | The attached guest framebuffer | Once focused, guest pixels are full-screen. Construct does not place a frame, status strip, taskbar, tint, or overlay over the guest image; the taskbar auto-hides. |
| Maps & Location | Workspace frame, rails, controls, sheets, alerts, and provider-state presentation | Map, route, and data-content colours | Cartographic/content colours remain legible and semantically truthful. This does not exempt Maps chrome or its loading, stale, offline, error, and destructive states from Construct primitives. |
| Browser VM | Browser VM launch/resume, connection, unavailable, reconnect, and diagnostics states | Chromium inside `browser-vm` | Construct never wraps, restyles, or recreates the guest viewport, tabs, toolbar, page UI, media, or dialogs. No host Browser engine/chrome is reintroduced. |

`SurfaceVisualInventory` is the machine-checked companion to this table. Any
new exception must update both it and the focused test; otherwise it is a normal
Construct-owned workspace and must converge on the shared primitives.
