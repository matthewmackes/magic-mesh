# Construct Brand - platform identity, icons, and version placement

Operator-locked 2026-07-03 (2-round `/plan` survey). Branding for the **DRM-native
egui platform** (`mde-shell-egui` + surfaces + RPM). Distinct from — and superseding,
for the egui world — the retired Cosmic/GTK/Qt system-theming design in
[`branding.md`](branding.md) (BRAND-1..10, which themed a Cosmic desktop: Plymouth,
LightDM greeter, GTK/Qt icon themes). The E12 Construct pivot retired libcosmic/iced, so
the shell now paints its own UI and owns its branding directly.

## Locked decisions

| # | Decision | Lock |
|---|----------|------|
| 1 | Icon types in scope | **All four:** product mark + wordmark lockup · DRM boot-splash logo · dock + surface glyphs · per-role + mesh-node badges. |
| 2 | Icon delivery | **Inline SVG → egui vectors** — SVG rasterized by egui at the exact DPI (crisp at any scale/zoom), tint-able from the `mde-theme` Carbon tokens. No GTK/freedesktop icon theme (the shell self-paints). |
| 3 | Icon style | **Monochrome Carbon line-art** — single-weight line glyphs on the Carbon grid, tinted per state (accent / dim / warn) from the tokens; IBM-Carbon-grade consistency with the shell. |
| 4 | Brand mechanism | **Extend `mde-theme`** — a `brand` submodule inside the theme crate (icons + logo + version/build-info) alongside the color tokens. One source of truth; every surface + the RPM + `--version` read from it. |
| 5 | Version placement | **All four:** shell chrome/status bar · About/System panel · DRM boot-splash · Mesh Map / fleet (per-node version, so fleet version-skew is visible). |
| 6 | Version format | **Internal semver + codename** — `12.0.0 "Construct"` remains the build identity; visible chrome/splash copy uses the Construct product labels and release line. |
| 7 | Build identity | **Baked via `build.rs`** — version + short git hash + build date + release channel + codename stamped at compile time into `mde-theme::brand::build`; `--version` and every surface read it. |
| 8 | Boot-splash | **Logo + wordmark + version** — centered product mark + wordmark + the version line on the Carbon field while the shell initializes. |

## Current implementation note

As of 2026-07-18, Construct is the visible product brand, while default platform
surface/status/tray/action glyphs resolve through
the bundled YAMIS monochrome icon theme. Native egui surfaces embed YAMIS SVGs
through `mde_theme::brand::icons::IconId`; the RPM also installs the full YAMIS
freedesktop icon theme under `/usr/share/icons/YAMIS` for toolkit and XDG
consumers.

## Official artwork + placement map

The operator supplied the Construct brand artwork through the 2026-07-18 Gemini
share and locked its placement:

| # | Decision | Lock |
|---|----------|------|
| 9 | **Canonical codename = "Construct"** | The 12.x codename is **Construct** — version line `12.0.0 "Construct"`; supersedes the earlier legacy naming in current code/docs and release notes. |
| 10 | Product name (user-facing) | **"Construct"** in About/splash/chrome, with **"Software Studio: MDE"** and **"Release 1.0 BETA"** as supporting visible identity lines where space allows; `magic-mesh` stays the infra/mesh + package name underneath (GNOME vs gnome-shell split). |
| 11 | DRM boot-splash image | **`CONSTRUCT-WALLPAPER1.png`** is the source splash asset; the shell paints native Construct text and token progress over the shell field. |
| 12 | Default desktop wallpaper | **`CONSTRUCT-WALLPAPER4.png`**; **all five generated Construct wallpapers ship** in the RPM as a selectable set. |
| 13 | About / System panel | **`CONSTRUCT-MAIN.png`** is the canonical Construct lockup source; About renders the text identity from `brand::logo`. |
| 14 | README banner | **`logo-lockup.png`** is regenerated from the Construct source image. |
| 15 | Product icon | **Crop the round mesh-node mark** from the artwork into the square app icon (16–512px rasters, replacing `app-icon.png`/favicon) **AND vector-trace it** so the in-shell dock/chrome mark is tintable + DPI-perfect. |
| 16 | SVG glyph set reconciliation | The Construct icon set's 17 surface/role/node glyphs are KEPT; its placeholder `mark.svg` + `wordmark.svg` (authored pre-artwork) are **replaced by faithful traces of the official mark + Construct lockup**. |

## Architecture

**`crates/shared/mde-theme` gains a `brand` submodule** (single source of truth,
sibling to the existing color-token modules):

- **`brand::build`** — a `build.rs` stamps `BuildInfo { version, codename, git_hash,
  build_date, channel }` at compile time (env vars → `include!`/`env!`). Internal
  helpers keep `version_line()` → `12.0.0 "Construct"` and `full()` → the complete
  build-info string for diagnostics and `--version`; visible product UI reads
  `brand::logo` Construct constants instead. Handles the no-git case (release
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
- **Boot-splash** — a startup frame (before the dock mounts) paints the Construct
  product name, studio line, and release line centered on the platform background.
- **Chrome / status bar** — Construct product identity and the node's role badge where
  space allows; diagnostics still have access to the internal build stamp.
- **About / System surface** — Construct identity + `brand::build::full()` (version · hash ·
  date · channel) in the build section + license/NOTICE links; the canonical "about this
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
- A Workstation boots to a platform boot-splash showing Construct, Software Studio:
  MDE, and Release 1.0 BETA, then the dock.
- The shell chrome and product surfaces avoid old brand names; opening About shows
  Construct identity + full build-info (version · git hash · date · channel) + links.
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

## Tasks -> see `docs/platform/WORKLIST.md` brand and visible-identity items.
