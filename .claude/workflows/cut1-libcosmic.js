export const meta = {
  name: 'cut1-libcosmic',
  description: 'CUT-1: port mde-workbench (+ mde-mesh-wallpaper bin) from crates.io iced 0.14 onto the libcosmic fork',
  phases: [
    { title: 'Foundation', detail: 'Cargo.toml swap + cosmic_compat + local object_card' },
    { title: 'Transform', detail: 'parallel mechanical port of leaf files + app shell + wallpaper' },
    { title: 'Build-fix', detail: 'serial build → parallel per-file fixers, looped to green' },
  ],
}

// The cut1-libcosmic worktree. All agents operate ONLY here.
const WT = '/home/mm/magic-mesh-cut1'
const CRATE = `${WT}/crates/workbench/mde-workbench`
const REV = 'cca48bc29ef7a9f22160c0ab6ba117ab22d1ae87'

// Shared transform rule-set, handed to every porting agent so they apply
// identical mechanics. The proven pattern is the mde-files GUI-7 port.
const RULES = `
PORT RULES (apply mechanically; the foundation commit already swapped Cargo.toml
and created src/cosmic_compat.rs):
1. Imports: \`use iced\` → \`use cosmic::iced\`. Bare path refs \`iced::X\` →
   \`cosmic::iced::X\`. \`iced_layershell::*\` is gone (wallpaper bin only).
2. \`mde_iced_components::object_card\` (and any \`c::object_card\` /
   \`crate::panel_chrome::object_card\` re-export) → \`crate::cosmic_compat::object_card\`.
   The mde-iced-components dep is dropped.
3. Per-widget styling: \`.style(closure)\` → \`.sty(closure)\` (the cosmic_compat
   ContainerSty/ButtonSty/SvgSty extension traits resolve by receiver type;
   container delegates to native .style). For \`text(..).color(c)\` or
   \`text(..).style(|t| ... color ...)\` use \`.colr(c)\` from TextSty.
4. \`mde_theme::Rgba::into_iced_color()\` STILL WORKS — cosmic_compat provides an
   IntoIcedColor extension trait with that exact method returning
   cosmic::iced::Color. Just ensure the trait is in scope.
5. Add ONE import line near the top of any file that uses .sty/.colr/
   into_iced_color/object_card: \`use crate::cosmic_compat::prelude::*;\`
   (idempotent — skip if already present).
6. Element/Renderer type params: cosmic widgets default to \`cosmic::Theme\`.
   Where code names \`iced::Element<'a, M>\` it becomes
   \`cosmic::iced::Element<'a, M, cosmic::Theme>\` (or \`cosmic::Element\`); fix only
   if the type was explicitly iced::Theme-bound.
7. DO NOT touch logic, message enums, or business code. DO NOT edit files outside
   your assignment. DO NOT run cargo (the build-fix phase handles compilation).
8. Carbon §4: never introduce raw hex / from_rgb literals — colors come from
   mde_theme tokens through into_iced_color()/the cosmic_compat helpers.
`

const COMPAT_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['ok', 'api', 'notes'],
  properties: {
    ok: { type: 'boolean', description: 'true if Cargo.toml swapped + cosmic_compat.rs written + object_card ported' },
    api: { type: 'string', description: 'the exact public names in cosmic_compat (traits, methods, object_card signature, prelude path)' },
    notes: { type: 'string', description: 'anything the transform/shell agents must know' },
  },
}

const BUILD_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['errorCount', 'files', 'globalErrors'],
  properties: {
    errorCount: { type: 'integer', description: 'total distinct compiler errors (E-codes + type errors), 0 when build succeeds' },
    files: {
      type: 'array',
      description: 'errors grouped by the source file they point at',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['path', 'errors'],
        properties: {
          path: { type: 'string', description: 'repo-relative path, e.g. crates/workbench/mde-workbench/src/panels/peers.rs' },
          errors: { type: 'array', items: { type: 'string' }, description: 'the error lines (code + message + line numbers) for this file' },
        },
      },
    },
    globalErrors: { type: 'array', items: { type: 'string' }, description: 'errors not attributable to one file (lockfile, missing dep, feature resolution)' },
  },
}

function chunk(arr, n) {
  const out = []
  for (let i = 0; i < arr.length; i += n) out.push(arr.slice(i, i + n))
  return out
}

// ---- Phase A: Foundation (barrier — everything else depends on it) ----
phase('Foundation')
const compat = await agent(
  `You are porting the mde-workbench crate to libcosmic (CUT-1). Work ONLY under ${CRATE}.

This is the FOUNDATION step the rest of the port depends on. Do exactly:

(1) Edit ${CRATE}/Cargo.toml:
    - Replace the \`iced = { version = "0.14", ... }\` line AND the
      \`iced_layershell = "0.18"\` line with a single libcosmic dep:
      libcosmic = { git = "https://github.com/pop-os/libcosmic.git", rev = "${REV}", default-features = false, features = ["tokio", "winit", "wgpu", "a11y", "wayland", "multi-window"] }
    - Remove the \`mde-iced-components = { path = ... }\` dependency line (it is
      being absorbed; see step 3).
    - On the \`mde-theme = { path = "...", features = ["iced", "serde"] }\` line,
      DROP "iced" from features (keep "serde") — the crates.io iced_core that
      feature pulls will NOT unify with the fork; into_iced_color is reprovided
      locally in step 2. Result: features = ["serde"].
    - Leave the [features] wayland/x11 block, [[bin]], everything else intact.

(2) Create ${CRATE}/src/cosmic_compat.rs. Start from this exact reference
    (the proven mde-files shim) and then ADD the two extra items below:
${'```'}rust
//! CUT-1 — libcosmic compat shims for the Workbench port. Bridges the
//! iced-style per-widget style closures the panels were written against to
//! libcosmic's class-based theming, plus a local IntoIcedColor (replacing the
//! mde-theme "iced" feature) and a local object_card (replacing mde-iced-components).
use cosmic::iced::widget::{button, container, svg};
use cosmic::iced::widget::{Button, Container, Svg, Text};
use cosmic::iced::Color;
use cosmic::Theme;

pub trait ContainerSty<'a, M: 'a> { #[must_use] fn sty(self, f: impl Fn(&Theme) -> container::Style + 'a) -> Self; }
impl<'a, M: 'a> ContainerSty<'a, M> for Container<'a, M, Theme> { fn sty(self, f: impl Fn(&Theme) -> container::Style + 'a) -> Self { self.style(f) } }

pub trait ButtonSty<'a, M: 'a> { #[must_use] fn sty(self, f: impl Fn(&Theme, button::Status) -> button::Style + 'static) -> Self; }
impl<'a, M: 'a> ButtonSty<'a, M> for Button<'a, M, Theme> { fn sty(self, f: impl Fn(&Theme, button::Status) -> button::Style + 'static) -> Self { self.class(cosmic::theme::iced::Button::Custom(Box::new(f))) } }

pub trait SvgSty<'a> { #[must_use] fn sty(self, f: impl Fn(&Theme) -> svg::Style + 'static) -> Self; }
impl<'a> SvgSty<'a> for Svg<'a, Theme> { fn sty(self, f: impl Fn(&Theme) -> svg::Style + 'static) -> Self { self.class(cosmic::theme::iced::Svg::custom(f)) } }

pub trait TextSty<'a> { #[must_use] fn colr(self, color: impl Into<Color>) -> Self; }
impl<'a> TextSty<'a> for Text<'a, Theme> { fn colr(self, color: impl Into<Color>) -> Self { self.class(cosmic::theme::iced::Text::Color(color.into())) } }
${'```'}
    ADD to that file:
    (a) An IntoIcedColor extension trait providing the SAME method name the
        panels already call, so the ~632 \`x.into_iced_color()\` sites compile
        unchanged:
          pub trait IntoIcedColor { fn into_iced_color(self) -> Color; }
          impl IntoIcedColor for mde_theme::Rgba { fn into_iced_color(self) -> Color { Color { r: self.r as f32/255.0, g: self.g as f32/255.0, b: self.b as f32/255.0, a: self.a } } }
        Also impl for &mde_theme::Rgba if Rgba is not Copy (check the type).
    (b) A local \`object_card\` + its public types, ported from
        ${WT}/crates/shared/mde-iced-components/src/lib.rs — copy that file's
        public surface (object_card fn, ObjectCard struct, overlay_white_on,
        overlay_color_on, with_alpha, and any helpers it needs) into
        cosmic_compat, converting every \`iced::\` → \`cosmic::iced::\` and
        \`Element<'a, Message>\` → \`cosmic::iced::Element<'a, Message, Theme>\`.
        Keep the exact public fn signature \`object_card<'a, Message: 'a>(card: ObjectCard, palette: Palette) -> Element<...>\`.
    (c) A prelude re-export so transform agents add one import per file:
          pub mod prelude { pub use super::{ContainerSty, ButtonSty, SvgSty, TextSty, IntoIcedColor, object_card, ObjectCard}; }

(3) In ${CRATE}/src/lib.rs add \`pub mod cosmic_compat;\` near the other top-level
    \`pub mod\` lines (do NOT do the cosmic::Application conversion — that is a
    separate agent; only add the module declaration).

Read the actual mde-iced-components file before porting object_card so you copy
its real contents. Do not run cargo. Report the exact public API you created.`,
  { schema: COMPAT_SCHEMA, phase: 'Foundation', label: 'foundation' }
)
log(`foundation: ok=${compat?.ok} — ${compat?.api?.slice(0, 120)}`)

// ---- Phase B+C: parallel mechanical transform + app shell + wallpaper ----
phase('Transform')
const leaf = (args && args.leafFiles) || []
const batches = chunk(leaf, 7)
log(`transform: ${leaf.length} leaf files in ${batches.length} batches + app-shell + wallpaper`)

const transformThunks = batches.map((b, i) => () =>
  agent(
    `Mechanically port these mde-workbench source files to libcosmic. Work ONLY under ${CRATE}.
Files (relative to ${CRATE}/src/): ${b.join(', ')}

The foundation commit is already in place. cosmic_compat API: ${compat?.api}
${RULES}
Edit each file in place. Report which files you changed and any spot that needed
a non-mechanical judgement call (the build-fix phase will catch the rest).`,
    { phase: 'Transform', label: `xform-${i}` }
  )
)

const shellThunk = () =>
  agent(
    `Convert the mde-workbench APP SHELL to a \`cosmic::Application\`. Work ONLY under ${CRATE}.
Files: src/app.rs, src/lib.rs, src/main.rs.

Use the EXISTING mde-files port as the reference for the cosmic::Application
shape — read ${WT}/crates/services/mde-files/src/app.rs and src/main.rs to see
how it implements: \`impl cosmic::Application for App\` with type Executor/Flags/
Message, const APP_ID, fn core/core_mut, fn init(core, flags) -> (Self, Task),
fn update, fn view, fn view_window (if multi-window), and how main.rs calls
\`cosmic::app::run::<App>(settings, flags)\`.

Port app.rs's current iced Application/Sandbox impl to that shape. Preserve the
custom titlebar / suppressed-headerbar approach mde-files uses. Apply the same
${RULES}
The wallpaper bin is handled by another agent — don't touch src/bin/. Don't run cargo.`,
    { phase: 'Transform', label: 'app-shell' }
  )

const wallpaperThunk = () =>
  agent(
    `Port the layer-shell wallpaper binary to the libcosmic fork's native
wlr-layer-shell. Work ONLY under ${CRATE}. File: src/bin/mde-mesh-wallpaper.rs.

It currently uses \`iced_layershell\` 0.18 (now removed). The libcosmic fork's
vendored iced ships native layer-shell. Reference the fork's working examples in
the local checkout:
  /home/mm/.cargo/git/checkouts/libcosmic-*/cca48bc/iced/examples/sctk_todos/src/main.rs
  /home/mm/.cargo/git/checkouts/libcosmic-*/cca48bc/iced/examples/sctk_lazy/src/main.rs
which use \`cosmic::iced::platform_specific::...wayland::layer_surface::{get_layer_surface, SctkLayerSurfaceSettings}\`,
Anchor, Layer::Background, and KeyboardInteractivity::None for a click-through
background surface. Port the wallpaper to create a Background layer surface with
KeyboardInteractivity::None (clicks pass through — the PD-10 contract), reusing
the shared MapProgram from src/panels/peers_map.rs.
Apply the ${RULES}. Don't run cargo.`,
    { phase: 'Transform', label: 'wallpaper' }
  )

await parallel([...transformThunks, shellThunk, wallpaperThunk])

// ---- Phase D: build-fix loop (serial build, parallel per-file fixers) ----
phase('Build-fix')
const BUILD_CMD = `cd ${WT} && cargo build -p mde-workbench 2>&1`
let round = 0
let lastReport = null
while (round < 10) {
  const rep = await agent(
    `Run the mde-workbench build and report errors as structured data. Run exactly:
    ${BUILD_CMD}
Then parse the compiler output. Return errorCount=0 ONLY if the build fully
succeeds. Group every error under the source file its primary span points at
(repo-relative path). Put dependency/lockfile/feature-resolution errors that
aren't tied to one source file in globalErrors. Do NOT edit anything — you are
only the reporter.`,
    { schema: BUILD_SCHEMA, phase: 'Build-fix', label: `build-r${round}` }
  )
  lastReport = rep
  log(`round ${round}: ${rep.errorCount} errors across ${rep.files.length} files; ${rep.globalErrors.length} global`)
  if (rep.errorCount === 0) break

  if (rep.globalErrors.length && rep.files.length === 0) {
    await agent(
      `Fix these workspace/dependency-level build errors for mde-workbench under ${WT}.
Errors:\n${rep.globalErrors.join('\n')}\n
Likely causes: a leftover \`iced\`/\`iced_layershell\`/\`mde_iced_components\` reference,
a missing feature on the libcosmic dep, or a Cargo.toml typo. Fix minimally and
correctly. Do not run cargo.`,
      { phase: 'Build-fix', label: `globalfix-r${round}` }
    )
  } else {
    const fixThunks = rep.files
      .filter((f) => f.errors && f.errors.length)
      .map((fe) => () =>
        agent(
          `Fix the libcosmic-port compiler errors in ONE file: ${fe.path} (under ${WT}).
Errors from the build:
${fe.errors.join('\n')}

Context: this file was mechanically ported from crates.io iced to the libcosmic
fork. The cosmic_compat shims (src/cosmic_compat.rs) provide .sty()/.colr()/
into_iced_color()/object_card via \`use crate::cosmic_compat::prelude::*;\`.
${RULES}
Common fork API drift to expect: widget builder signature changes (Space, Id,
progress_bar, AbsoluteOffset), Palette field names, listen_with/subscription
event types, Element theme type params. Fix THIS file only; if the error truly
originates elsewhere, say so in your reply instead of editing other files.
Do not run cargo.`,
          { phase: 'Build-fix', label: `fix:${fe.path.split('/').pop()}-r${round}` }
        )
      )
    await parallel(fixThunks)
  }
  round++
}

// Final verification build.
const final = await agent(
  `Run \`${BUILD_CMD}\` and report the final result.`,
  { schema: BUILD_SCHEMA, phase: 'Build-fix', label: 'build-final' }
)
log(`FINAL: ${final.errorCount} errors`)
return {
  rounds: round,
  finalErrorCount: final.errorCount,
  finalFiles: final.files.map((f) => f.path),
  green: final.errorCount === 0,
}
