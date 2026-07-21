# Auto Mode — the Ford SYNC 3 in-vehicle interface

**Goal (operator, 2026-07-20):** a first-class **Auto Mode** (Car Mode) across every
application relevant in a vehicle, skinned to match the **Ford SYNC 3 dark interface**
(black / white / blue), driven by a **physical USB keyboard** with keys assigned directly
to actions, plus a **Key Mapping** page in Settings to configure those bindings.

This is an *evolution*, not a greenfield build: the shell already has a `LayoutProfile::Car`
profile (Touch density, persisted to disk), a 5-tile Car HUD overlay (`mount_car_hud`), and a
hardcoded physical-key→surface router (`apply_car_keyboard_routes` / `CarKeyRoute`). Auto Mode
lifts each of those from scaffold to product.

## Design language — Ford SYNC 3 dark (black / white / blue)

SYNC 3's dark interface is a **near-black ground** (glare-free at night), **pure-white
glanceable text**, and a **bright sky/cyan blue accent** on tiles, headers, and active
states. The reconciliation with the platform's Quasar-dark lock (which is "dark only, no
switcher") is that Auto Mode is a **mode-scoped skin**, not a global theme: it is installed
*only* while `LayoutProfile::Car` is active and reverts to the operator's Dark/Light pick on
exit. It is therefore deliberately **absent from the Personalization theme picker**
(`StyleColorScheme::ALL`) — Car Mode installs it, the operator never picks it manually.

The skin lives as a third `StyleColorScheme::AutoSync3` in `mde_egui::style`, beside the
existing `Dark` (Construct) and `Light` (Windows-2000) schemes — the same proven
install→visuals→per-shape-remap path. Palette tokens (`SYNC3_*`):

| Token | Value | Role |
|---|---|---|
| `SYNC3_BG` | `#04070B` | near-black ground (blacker than Construct `#16161A`) |
| `SYNC3_SURFACE` | `#12171E` | raised tile / card |
| `SYNC3_SURFACE_HI` | `#1C242E` | hovered tile |
| `SYNC3_BORDER` | `#2B3540` | cool hairline |
| `SYNC3_TEXT` | `#FFFFFF` | pure-white glanceable text |
| `SYNC3_TEXT_DIM` | `#A6B4C2` | cool secondary text |
| `SYNC3_ACCENT` | `#2E9BE6` | bright Ford SYNC blue |
| `SYNC3_ACCENT_HI` | `#5FB8F2` | accent highlight (pressed ring) |

Density is already handled: `LayoutProfile::Car` maps to `Density::Touch` (44px hit targets,
1.5× spacing) — the glanceable-at-speed sizing comes for free.

## The apps that matter in a vehicle

Auto Mode curates the existing surfaces into a driver-first set. The seed is the existing
`CarKeyRoute` table:

| Auto app | Surface | Car treatment |
|---|---|---|
| **Navigation** | `MapsLocation` | already the Drive HUD (Waze/GMaps grammar, live MG90 fold); collapse the tab rail under `is_car()` so it is full-bleed |
| **Media** | `Media` / `Music` | enlarge the existing transport (`player_view`) — big play/pause/skip, big now-playing |
| **Phone** | `Voice` | large dialer + already-large Answer/Decline; favorites |
| **Comms** | `Communications` | the Alerts inbox + persistent call bar as the glanceable pieces |
| **Vehicle** | `MapsLocation` › Vehicle | live MG90 telematics (voltage / ignition / cellular) |
| **Settings** | `System` | Car Mode + Key Mapping pages |

## Physical keyboard → action

Car Mode assumes a **physical USB keyboard** mounted in the vehicle with keys assigned to
actions (jump to Nav, play/pause, answer/decline a call, …). egui distinguishes letters,
digits, and **F1–F35** cleanly (media keys need the evdev `hostkeys` side-channel), so the
binding surface is those physical keys. The current router hardcodes `Num1-5 / F1-5`; Auto
Mode replaces that with a **persisted, editable binding map**.

- `CarAction` — a unified action enum: surface jumps (Nav/Media/Phone/Comms/Vehicle/Home) +
  transport (play-pause/next/prev) + call (answer/decline/hang-up), dispatched through the
  existing `apply_nav` / `apply_hotkey` effect executors.
- `CarKeyBindings` — `egui::Key → CarAction`, serialized by **stable string names** (since
  `Surface`/`HotkeyAction` are not `serde`), persisted to `settings-car-keys.json` via the
  atomic tmp+rename idiom used by `AppearanceConfig`. Defaults reproduce the current
  `Num1-5 / F1-5` map so behavior is unchanged until the operator rebinds.
- **Key Mapping** settings page — `SettingsSection::KeyMapping`, an editable grid evolved
  from the read-only `hotkeys_section`, with a **press-a-key-to-bind capture control** (the
  one genuinely-new widget) + reset-to-default.

## Units (E12-POLISH · Auto Mode)

Foundation is shared-kit (serializes); the shell spine concentrates in `main.rs` +
`system/mod.rs` (serializes on those files); per-app treatments are disjoint crates (fan out).

- **AUTO-THEME-1** ✅ — `StyleColorScheme::AutoSync3` + `SYNC3_*` palette + all match arms +
  test. `mde-egui/style.rs`.
- **AUTO-THEME-2** — install `AutoSync3` when `layout_profile.is_car()`. `system/mod.rs`
  `apply_appearance`.
- **AUTO-HOME** — evolve `mount_car_hud` into a glanceable, **clickable** Sync-3 home
  (Nav/Media/Phone/Comms/Vehicle/Settings tiles routing `nav.surface`, live glance data).
  `main.rs`.
- **AUTO-KEYMAP-MODEL** — `CarAction` + `CarKeyBindings` + `settings-car-keys.json` persist +
  defaults. New `car_keymap.rs`.
- **AUTO-KEYMAP-INPUT** — rewire `apply_car_keyboard_routes` to the persisted bindings.
  `main.rs`.
- **AUTO-KEYMAP-SETTINGS** — `SettingsSection::KeyMapping` + editable grid + key-capture
  widget. `system/mod.rs` (+ a shared capture widget in `mde-egui/widgets.rs`).
- **AUTO-MEDIA** — Car transport in `mde-media-egui` `player_view` under `is_car()`.
- **AUTO-VOICE** — Car dialer/favorites in `mde-voice-egui`.
- **AUTO-MAPS** — collapse the rail under `is_car()` so the Drive HUD is full-bleed.
- **AUTO-COMMS** — Car Alerts + call-bar treatment in `mde-collab-egui`.

## Verification

Per `/polish` §7 + the visual gate: each crate farm-green (`build` + `test` + style-leak grep
clean); the shell launches and Car Mode installs Sync-3 (black ground, white text, blue
accent) with Touch density; the Key Mapping page rebinds a key and it persists across restart;
finally deploy to the physical `.15` seat, enter Car Mode, and judge the black/white/blue
glanceable look on the dash against how Ford SYNC 3 actually looks — with the live MG90 fold
driving the Nav/Vehicle glance data.

## Key files

- `crates/shared/mde-egui/src/style.rs` — `StyleColorScheme::AutoSync3` + `SYNC3_*` (done).
- `crates/desktop/mde-shell-egui/src/system/mod.rs` — theme scoping (`apply_appearance`) +
  the Key Mapping / Car Mode settings sections.
- `crates/desktop/mde-shell-egui/src/main.rs` — `mount_car_hud` (→ Auto Home) +
  `apply_car_keyboard_routes` / `CarKeyRoute` (→ persisted bindings).
- `crates/desktop/mde-shell-egui/src/car_keymap.rs` — NEW: `CarAction` / `CarKeyBindings`.
- `crates/desktop/mde-{media,voice,maps-location,collab}-egui/` — per-app Car treatments.
