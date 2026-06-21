# Carbon criteria — the machine-checkable rubric

The objective function for `refining-carbon-gui`. Each criterion is a **pass/fail
assertion** plus the **nearest-token fix**. Values are sourced from the
`@carbon/layout`, `@carbon/type`, `@carbon/motion` package source (GitHub), not
the truncation-prone docs site. In MCNF every value below lives in
`crates/shared/mde-theme` — surface code references tokens, never literals (§4).

## Contents
1. Spacing
2. Grid
3. Type
4. Theme / color tokens
5. Contrast (hard gate)
6. Motion
7. Choreography
8. State coverage
9. Focus ring
10. Objective function

---

## 1. Spacing
Every margin / padding / gap reduces to one of the **13 spacing tokens**:
`2, 4, 8, 12, 16, 24, 32, 40, 48, 64, 80, 96, 160` px.
- **FAIL** any off-scale value → suggest the nearest token.
- `spacing-05` = **16px** is the workhorse default.
- All values live in `mde-theme::spacing` (`Space::for_density`), never raw in
  surface code (§4). MCNF's `Space` is the density-scaled realization of this scale.

## 2. Grid
- Layout dimensions reduce to multiples of **8px** (4px for fine detail).
- Containers use the **16-column 2x grid**.
- **CORRECTION (verified):** the default 2x-grid **gutter is 32px**, not 16px —
  16px is the per-column padding / half-gutter. Flag designs that assume a 16px
  gutter.

## 3. Type
Every text element uses a **named type token**. **FAIL** any font-size not on the
12–92px recursive scale:
`12, 14, 16, 18, 20, 24, 28, 32, 36, 42, 48, 54, 60, 68, 76, 84, 92` px,
and any weight / line-height / tracking combo that doesn't match its token (e.g. a
heading weight on body text).
- **CORRECTION (verified):** `heading-07` is **54px / 1.199 line-height**, not
  60px (60px is a fluid/display step).
- MCNF: use `mde-theme::typography` named tokens; flag literal font sizes.

## 4. Theme / color tokens
- **No literal hex anywhere outside `crates/shared/mde-theme`** —
  `Color::from_rgb` / `from_rgb8` outside `mde-theme` without a `// carbon-ok:
  <reason>` marker is a **hard fail** (`install-helpers/lint-carbon-tokens.sh`).
- Use **semantic tokens** (`$background`, `$layer`, `$text-primary`,
  `$border-subtle`, `$focus`) so all themes re-theme cleanly.
- Themes: **Gray 100** (default dark) · **Gray 90** (alt dark) · **Gray 10**
  (light). (White exists upstream; MCNF ships the three grays per §4.)

## 5. Contrast (hard gate)
- **Body / standard text** ≥ **4.5:1**.
- **Large text** (≥24px regular/light OR ≥19px semibold) ≥ **3:1**.
- **Non-text indicators** (borders, focus rings, icons) ≥ **3:1** vs the adjacent
  color.
- **Fast pre-check:** two Carbon grays **≥50 steps apart** are accessible (e.g.
  Gray 100 text-zone vs Gray 30 text). Use this to triage before computing exact
  ratios.

## 6. Motion
Every transition maps to a Carbon **duration token** AND one of the **six exact
cubic-beziers**. Flag `linear` or ad-hoc curves.
- Durations: **70, 110, 150, 240, 400, 700** ms. Duration scales with the size of
  the change.
- Easings:
  - Productive — standard `(0.2, 0, 0.38, 0.9)` · entrance `(0, 0, 0.38, 0.9)` ·
    exit `(0.2, 0, 1, 0.9)`
  - Expressive — standard `(0.4, 0.14, 0.3, 1)` · entrance `(0, 0, 0.3, 1)` ·
    exit `(0.4, 0.14, 1, 1)`
- **Productive** mode for frequent/utility feedback; **expressive** for
  rare/important moments.
- MCNF: these are already in `mde-theme::motion` (`DURATION_*`, `EASING_*`,
  `Motion::*` presets). A transition not built from those is a finding.

## 7. Choreography
Multi-element reveals (lists / tables / grids) **stagger ~20ms per item, total
≤500ms** — no all-at-once reveal. MCNF tokens: `mde-theme::motion::list`
(`STAGGER_STEP_MS = 20`, cap 8). Flag a list that pops in simultaneously.

## 8. State coverage
Every interactive element defines **hover, focus, active, selected, disabled** via
the `-hover` / `-active` / `-selected` token suffixes. A missing state is a
finding.
- **Disabled** = reduced opacity, no hover/focus, `not-allowed` cursor; disabled
  never receives focus or hover.

## 9. Focus ring
Every interactive element has a visible **2px `$focus` border** (Blue 60 on light
themes / White on dark) at **≥3:1** contrast. **Never strip the outline.** Add
`$focus-inset` where 3:1 against the fill is hard. **This is the single
most-checkable a11y rule** — `score-surface.sh` flags any focusable widget with no
focus style.

## 10. Objective function
Each round optimizes toward the four Carbon principles — **clarity, efficiency,
consistency, beauty** — with **"is this consistent with the rest of the app?"** as
the default review question. When in doubt, prefer the choice that matches an
existing MCNF surface. Source exact token values from the `@carbon/*` package
source, not the docs site.
