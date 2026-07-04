# SETTINGS — the world-class Settings redesign (master–detail, tinted, Carbon)

Operator-locked 2026-07-04 (5-Q `/plan` survey). The Settings surface (`Surface::System`,
`crates/desktop/mde-shell-egui/src/system.rs`) is **full-featured but organized poorly**:
today it is a **single vertical column** of stacked titled cards — Mixer, Bluetooth,
Displays, Power & Battery, Wallpaper, Hotkeys — that wastes the page width and gives no
visual separation between unrelated areas. This epic makes it **world-class**: a
master–detail layout that uses the full width, with per-domain color tint + Carbon
elevation for separation, all on Carbon Design rules (§4 tokens, no raw hex).

## Locked decisions (5)

| # | Area | Lock |
|---|------|------|
| 1 | Layout | **Master–detail rail** — a left Carbon UI-shell **side-nav** of categories + a right **detail pane** that uses the full remaining width (the macOS/Win11/GNOME idiom). The detail pane gets the whole right side instead of a narrow centered column. |
| 2 | Tinting | **Categorical accent + Carbon layers** — each domain group carries a subtle **accent tint** (reusing the PICKER-2 / EXPLORER-15 accent tokens for ONE color language across the shell) on its rail header + the active detail header; cards sit on **Carbon elevation layers** (layer-01 page → layer-02 cards) with hairline borders. Both hue + tonal separation. |
| 3 | Grouping | **By domain** — three parent groups: **Devices** (Displays, Audio/Mixer, Bluetooth, Power & Battery) · **Personalization** (Wallpaper, Hotkeys, Theme) · **Mesh & System** (Identity, Role, Pairing, Network). Scales as sections grow. |
| 4 | Density | **Carbon expressive (roomy)** — larger type + generous whitespace; the detail pane breathes. Carbon 2x-grid spacing tokens (`SP_*`), expressive type scale. |
| 5 | Findability | **Rail nav only, no search** — the master–detail rail + the three domain groups ARE the navigation; active section highlighted in its group accent. No search box. |

## The taxonomy (locked)

Every existing section is placed exactly once; new sections are honest-empty until wired:

| Group | Accent | Sections |
|---|---|---|
| **Devices** | accent A | Displays · Audio (Mixer) · Bluetooth · Power & Battery |
| **Personalization** | accent B | Wallpaper · Hotkeys · Theme *(new)* |
| **Mesh & System** | accent C | Identity · Role · Pairing · Network |

Notes: Audio = the existing `mixer_section` (renamed "Audio" in the rail). Power & Battery =
existing `power_section` + `power_settings.rs`. Pairing folds in the existing
`sync_pairing_agent` (the KDC/SEC-4 pairing the surface already drives when expanded).
Identity/Role/Network surface the mesh identity + role pin + overlay/underlay facts the
node already knows (honest-`unknown` where unprobed, §7). Theme is a new Personalization
section (accent/appearance/text-scale, reusing EXPLORER-18 accessibility posture).

## Architecture (mde-shell-egui)

- **`system.rs` → master–detail** replacing the single `ScrollArea` stack:
  - A left **rail** (`SidePanel::left`, expressive width ≈ `SP_XL*` band) rendering the 3
    domain groups; each group = an accent-tinted header + its section rows. A selected
    section row shows the **active** state (group-accent left-marker + layer wash). Rail
    state = a `SettingsNav { group, section }` enum persisted in the shell config so the
    surface reopens where you left it.
  - A right **detail pane** filling the remaining width: renders ONLY the selected
    section's body via the existing per-section fns (`mixer_section`, `bluetooth_section`,
    `displays_section`, `power_section`, `wallpaper_section`, `hotkeys_section`, …) —
    **reuse the section bodies + their `apply()`/`SysAction` seams verbatim (§6)**, just
    re-hosted in a wide, expressive layout. No behavior change to the controls; a
    presentation/routing pass.
- **Accent + layers (`Style`/`mde-theme`)**: three domain accent tokens — REUSE the
  existing `Style::ACCENT_*` set (PICKER-2) and **coordinate with EXPLORER-15** so the
  category color language is defined ONCE and both epics consume it (no second token set,
  §4). Page = layer-01, cards = layer-02, hairline borders from the border token. No raw
  hex, no scattered metric literals.
- **Expressive width use in the bodies**: each section's body reflows to the wide pane —
  Displays lays outputs in a **row** of cards (not stacked), Bluetooth puts adapters +
  devices in **columns**, Mixer spreads channels **across**, Power shows battery + profiles
  **side by side**. The narrow-column single-file layout is retired. (Bodies stay the same
  logic; only their internal `ui.horizontal`/grid changes.)
- **`main.rs` (`Surface::System` arm, ~line 631)**: calls the new master–detail entry
  (`system::settings_panel(ui, snap, &mut nav, &mut actions)`) instead of the old stacked
  render. The `sync_pairing_agent(expanded && surface==System)` gate carries over (now keyed
  to the Mesh & System → Pairing section being visible).

## Acceptance (runtime-observable; per task in the worklist)

- Settings opens as a **left rail + wide right detail pane** (NOT a single narrow column);
  the rail lists the 3 domain groups with their sections; clicking a section shows it in the
  detail pane using the full width; the active section is highlighted in its group accent.
- Every existing control (mixer/bluetooth/displays/power/wallpaper/hotkeys) is reachable
  **exactly once** and its actions still dispatch (a Bluetooth toggle, a display mode change
  still fire the real `SysAction`).
- Domain groups are visually separated by **accent tint on headers + Carbon elevation
  layers** on cards; all colors/metrics via `Style` tokens (§4 — grep-clean of raw hex).
- Expressive density: the detail pane uses roomy spacing + larger type; wide sections
  (Displays/Bluetooth/Mixer/Power) lay their items **across** the width, not stacked.
- The Mesh & System group surfaces identity/role/pairing/network from real node state
  (honest-`unknown` where unprobed); Pairing drives the existing agent.
- Nav position persists across surface switches + restart.

## Risks

- **Shared accent tokens** — must coordinate the 3 domain accents with PICKER-2 (landed) +
  EXPLORER-15 (open) so there is ONE token set, not three. Define in `Style`/`mde-theme`,
  both consume.
- **`main.rs` coordination** — the `Surface::System` arm + the in-flight browser live-helper
  wiring both touch `main.rs`; serialize the two shell edits (SETTINGS-1 vs the browser
  wiring) on each other's landed base to avoid a hand-merge (see the serialize-same-file
  discipline).
- **Section-body reuse** — re-hosting the existing section fns must NOT fork their logic;
  the `apply()`/`SysAction` seams stay the single source of truth (a presentation pass, not
  a rewrite, §6/§7).
- **Expressive on a small seat** — a low-res DRM seat must still fit the rail + detail; the
  rail collapses to icons/short labels below a width threshold (graceful, not clipped).

## Out of scope (v1)

- A settings search box (#5 = rail only).
- A responsive compact↔expressive auto-switch (#4 = expressive; a small-seat rail-collapse
  is the only concession).
- New device backends — this is a re-layout of the EXISTING sections + honest-empty new
  Mesh & System / Theme sections, not new hardware control.

## Tasks → `docs/WORKLIST.md` SETTINGS-1..6.
