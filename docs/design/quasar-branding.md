# QBRAND — Quazar platform branding (professional icons + version placement)

Operator-locked 2026-07-03 (2-round `/plan` survey). Branding for the **DRM-native
egui platform** (`mde-shell-egui` + surfaces + RPM). Distinct from — and superseding,
for the egui world — the retired Cosmic/GTK/Qt system-theming design in
[`branding.md`](branding.md) (BRAND-1..10, which themed a Cosmic desktop: Plymouth,
LightDM greeter, GTK/Qt icon themes). The E12 Quazar pivot retired libcosmic/iced, so
the shell now paints its own UI and owns its branding directly.

## Locked decisions

| # | Decision | Lock |
|---|----------|------|
| 1 | Icon types in scope | **All four:** product mark + wordmark lockup · DRM boot-splash logo · dock + surface glyphs · per-role + mesh-node badges. |
| 2 | Icon delivery | **Inline SVG → egui vectors** — SVG rasterized by egui at the exact DPI (crisp at any scale/zoom), tint-able from the `mde-theme` Carbon tokens. No GTK/freedesktop icon theme (the shell self-paints). |
| 3 | Icon style | **Monochrome Carbon line-art** — single-weight line glyphs on the Carbon grid, tinted per state (accent / dim / warn) from the tokens; IBM-Carbon-grade consistency with the shell. |
| 4 | Brand mechanism | **Extend `mde-theme`** — a `brand` submodule inside the theme crate (icons + logo + version/build-info) alongside the color tokens. One source of truth; every surface + the RPM + `--version` read from it. |
| 5 | Version placement | **All four:** shell chrome/status bar · About/System panel · DRM boot-splash · Mesh Map / fleet (per-node version, so fleet version-skew is visible). |
| 6 | Version format | **Semver + codename** — `12.0.0 "Quazar"` in chrome + splash; the About panel additionally shows the full build-info. |
| 7 | Build identity | **Baked via `build.rs`** — version + short git hash + build date + release channel + codename stamped at compile time into `mde-theme::brand::build`; `--version` and every surface read it. |
| 8 | Boot-splash | **Logo + wordmark + version** — centered product mark + wordmark + the version line on the Carbon field while the shell initializes. |

## Official artwork + placement map (operator-locked 2026-07-03, round 2)

The operator supplied the official brand artwork (`assets/brand/MDE-QUAZAR-*.png`,
commit `35fef34`) and locked its placement in a 2-round survey:

| # | Decision | Lock |
|---|----------|------|
| 9 | **Canonical codename spelling = "Quazar"** (Z, per the artwork) | The 12.x codename is **Quazar** — version line `12.0.0 "Quazar"`; supersedes the earlier "Quasar" spelling in code/docs/release notes. |
| 10 | Product name (user-facing) | **"MDE Quazar — Mackes Display Environment"** in About/splash/chrome; `magic-mesh` stays the infra/mesh + package name underneath (GNOME vs gnome-shell split). |
| 11 | DRM boot-splash image | **`MDE-QUAZAR-WALLPAPER1.png`** (centered mark + wordmark + the loading bar) — the shell animates real progress along the artwork's bar, version line beneath. |
| 12 | Default desktop wallpaper | **`MDE-QUAZAR-WALLPAPER4.png`**; **all five wallpapers ship** in the RPM as a selectable set. |
| 13 | About / System panel | **`MDE-QUAZAR-MAIN.png`** lockup at the top; build-info + links beneath. |
| 14 | README banner | **`MDE-QUAZAR-WALLPAPER2.png`** replaces `readme-banner-dark/light.svg`. |
| 15 | Product icon | **Crop the round mesh-node mark** from the artwork into the square app icon (16–512px rasters, replaces `app-icon.png`/favicon via QBRAND-9) **AND vector-trace it** so the in-shell dock/chrome mark is tintable + DPI-perfect. |
| 16 | SVG glyph set reconciliation | The QBRAND-10 set's 17 surface/role/node glyphs are KEPT; its placeholder `mark.svg` + `wordmark.svg` (authored pre-artwork) are **replaced by faithful traces of the official mark + MDE Quazar lockup**. |

## Architecture

**`crates/shared/mde-theme` gains a `brand` submodule** (single source of truth,
sibling to the existing color-token modules):

- **`brand::build`** — a `build.rs` stamps `BuildInfo { version, codename, git_hash,
  build_date, channel }` at compile time (env vars → `include!`/`env!`). Display
  helpers: `version_line()` → `12.0.0 "Quazar"` (chrome/splash) and `full()` → the
  complete build-info string (About + `--version`). Handles the no-git case (release
  tarball) with a sentinel hash. Codename is keyed off the workspace version's epoch.
- **`brand::icons`** — the monochrome Carbon line-art SVG set as inline `&str`
  consts, plus an egui loader (`icon(id, size, tint) -> TextureHandle`) that
  rasterizes the SVG at the requested DPI and tints it from a `Style` token. Ids
  cover: the product mark, ~12 dock/surface glyphs (Workbench/Instances/Desktop/
  Music/Media/Files/Voice/Browser/Terminal/Chat/System/Storage/MeshView), 3 role
  badges (Workstation/Server/Lighthouse), and node/peer glyphs (health-tinted).
- **`brand::logo`** — the product mark + wordmark lockup (for the boot-splash + About),
  same SVG-→-egui path.

**`crates/desktop/mde-shell-egui`** consumes it:
- **Boot-splash** — a startup frame (before the dock mounts) paints `brand::logo`
  lockup + `brand::build::version_line()` centered on the Carbon background.
- **Chrome / status bar** — a subtle `version_line()` tag + the node's role badge.
- **About / System surface** — logo lockup + `brand::build::full()` (version · hash ·
  date · channel) + codename + license/NOTICE links; the canonical "about this
  platform" screen.
- **Dock / surface glyphs** — each `Surface` pulls its glyph from `brand::icons`
  (replaces any ad-hoc per-surface glyphs), tinted by state.
- **Mesh Map / fleet** — each node renders its role badge + running version (reuse the
  peer version already reported via `CARGO_PKG_VERSION`; surface it per-node so skew
  shows at a glance).

**Packaging** — the product icon is single-sourced (from the brand assets /
`regen-app-icon.sh`) into the RPM `.desktop` launcher + favicon + app icon, so the
mark is identical everywhere.

## Acceptance (runtime-observable, per task)
- A Workstation boots to a Carbon boot-splash showing the product mark + wordmark +
  `12.0.0 "Quazar"`, then the dock.
- The shell chrome shows a subtle version tag + the node's role badge; opening About
  shows the logo lockup + full build-info (version · git hash · date · channel) + codename + links.
- Every dock surface renders its monochrome Carbon glyph from `brand::icons`, crisp at
  200% zoom / HiDPI, tinted by the active/inactive token.
- The Mesh Map shows each node's role badge + its running version; a node on an older
  build is visibly different.
- `mde-shell-egui --version` (and `mackesd --version`) prints the baked build-info line.
- `build.rs` stamps version + git short-hash + build date + channel at compile; a
  no-git build still produces a valid line.

## Risks
- **SVG rasterization in egui** — needs `resvg`/`usvg` (or a pre-rasterize step);
  confirm it builds on the airgapped farm and the added dep is acceptable. Fallback: a
  build-time SVG→PNG bake.
- **`build.rs` git-hash on the farm** — git is present at build; handle the release
  tarball / shallow-clone case with a sentinel.
- **Icon art is real design work** — the monochrome Carbon glyph set (product mark,
  wordmark, ~12 surface glyphs, 3 role badges, node glyphs) must be drawn; the existing
  `assets/icons/carbon/` (22 glyphs) + `assets/brand/*` + `assets/heroes/*` are the
  starting point, not the finished set.

## Out of scope
- The retired Cosmic/GTK/Qt system theming ([`branding.md`](branding.md) BRAND-1..10) —
  superseded for the egui platform.
- A brand/theme picker or per-user override (single platform brand).
- A revert path (branding is intrinsic to the shell, not a one-way system mutation).

## Tasks → see `docs/WORKLIST.md` QBRAND-1..10.
