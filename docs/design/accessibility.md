# Accessibility — the AccessKit seam (a11y-01) and the screen-reader epic (a11y-02)

**Status:** a11y-01 LANDED (the enabling slice — this doc's first half is as-built).
a11y-02 SCOPED, not built (this doc's second half).
**Scope of this doc:** the runtime AccessKit plumbing that a11y-01 shipped, and the
architecture + sequencing of the a11y-02 in-process screen reader that plugs into it.

## Background — why the tree was empty at runtime

Every E12 shell surface is richly AccessKit-annotated: live regions (`Role::Status` /
`Role::Alert` with `Live::Polite` / `Live::Assertive`), roles, and labels are set on the
egui widgets throughout `mde-shell-egui`. But egui only *generates* the AccessKit tree
after `ctx.enable_accesskit()` has been called, and only *emits* it as
`full_output.platform_output.accesskit_update` on the frames the tree is rebuilt.

The bug (**a11y-01**): the only calls to `enable_accesskit()` in the whole tree were
inside `#[cfg(test)]` modules (25 of them, all in shell test code). The two production
runners never called it and never drained the update:

- `mde_egui::run_drm` — the bare-DRM/KMS present loop (no compositor, no winit) — owns a
  plain `egui::Context` and threw `full_output.platform_output.accesskit_update` away
  every frame.
- `mde_egui::run_client` — the windowed eframe fallback — relied on eframe's adapter but
  the crate doc claimed accessibility was "deferred… not wired here".

So on the shipped seat the product exported **zero** accessibility tree at runtime. The
annotation work was real but dead outside tests.

## a11y-01 — the enabling slice (as-built)

### The consumer seam — `mde_egui::a11y`

```
pub trait AccessKitSink {
    fn ingest(&mut self, update: egui::accesskit::TreeUpdate);
    fn wants_refresh(&mut self) -> bool { false }   // default: never drives a render
}

pub struct LatestTree { /* holds the most-recent TreeUpdate + a one-shot refresh flag */ }
impl AccessKitSink for LatestTree { … }

pub struct A11yBridge { /* enabled flag + Box<dyn AccessKitSink> */ }
```

`A11yBridge` is the loop-facing façade. It compiles two ways:

- **`--features accesskit` on** (the shell always sets it): the real bridge — `enable()`
  turns egui tree generation on, `drain(&mut full_output)` takes
  `platform_output.accesskit_update` and hands it to the sink, `wants_render()` surfaces
  the sink's refresh request.
- **feature off** (a plain `mde-egui --features drm` build): a zero-cost no-op struct with
  the same method surface, so `run_drm`'s body carries no `#[cfg]`.

### Enablement — gated, default OFF, production-reachable

`run_drm` builds the bridge from the environment and enables it once at startup:

```
let mut a11y = crate::a11y::A11yBridge::from_env();   // MDE_A11Y=1 → enabled, else OFF
a11y.enable(&egui_ctx);
```

`MDE_A11Y` mirrors the `MDE_DRM_ESC_QUIT` env idiom already in the same loop. Default
(unset) is OFF — zero cost for seats that don't need it — but it is reachable in
production, not test-only. (a11y-02 adds a live hotkey toggle; see below.)

### Keeping the event-driven loop accessibility-fresh

perf-1 rewrote `run_drm` to be event-driven: it blocks in `poll(2)` and only renders when
`wake::should_render(first_frame, has_input, force_render, repaint_due)` says so. A tree
that is only rebuilt on render would go stale while the loop idles. a11y-01 folds the
consumer's refresh request into `force_render`, mirroring the seat-side rotation /
formfactor / host-key wakes:

```
if a11y.wants_render() { force_render = true; }   // just before the should_render gate
…
let mut full_output = egui_ctx.run(raw_input, |ctx| { ui(ctx); /* cursor */ });
a11y.drain(&mut full_output);                      // hand the tree to the sink
```

So an AT client connecting (or the reader asking for a re-scan) via `wants_refresh()`
wakes a render, the fresh tree flows through `drain`, and the loop returns to idle — the
tree stays live without spinning.

### The windowed fallback (`run_client`)

No code change was needed: when the `accesskit` feature is on, eframe initialises an
`accesskit_winit` adapter on the window and *lazily* calls `enable_accesskit()` the moment
an assistive-technology client requests the tree (`InitialTreeRequested`), routes
`ActionRequested` back to egui, and `disable_accesskit()` on `AccessibilityDeactivated`.
The tree stays empty and zero-cost until a screen reader connects. The runner doc was
corrected to state this; the feature flag (`eframe/accesskit`) already existed.

### Proof it's live (headless self-tests)

- `mde-egui` (`a11y.rs`, `--features accesskit`): the seam unit tests — `LatestTree`
  retains the latest tree and the refresh flag is one-shot; an `A11yBridge` built like
  `run_drm`'s drains a real rendered frame's non-trivial tree through a pluggable sink
  and the refresh wake is one-shot; a `from_env` bridge with `MDE_A11Y` unset is inert
  (generates no tree).
- `mde-shell-egui` (`main.rs`): `a11y01_production_accesskit_stream_is_live_through_the_run_drm_seam`
  drives the **same** `A11yBridge` (enable + drain) against the **real** shell fixture and
  asserts the drained `TreeUpdate` has a resolvable root and a non-trivial node set — the
  "it's actually reachable now" proof.

### What still needs a live AT client

The headless tests prove the *enablement + update stream*. Two things can only be
confirmed against a live assistive-technology client on a real seat, and are deferred:

1. The windowed fallback's eframe AT-SPI adapter actually handshaking with a running
   Orca / AT-SPI bus (the lazy `InitialTreeRequested` path).
2. A screen reader *announcing* the tree (that is a11y-02's whole job).

## a11y-02 — the in-process screen reader + TTS (SCOPED, not built)

### Goal

An airgap-friendly, in-process screen reader for the bare-DRM seat: no external AT-SPI
bus, no cloud TTS. It consumes the AccessKit tree a11y-01 already streams and speaks the
focus / live-region changes through a local, offline TTS engine.

### Architecture

```
 run_drm present loop
      │  full_output.platform_output.accesskit_update  (a11y-01, already flowing)
      ▼
 A11yBridge.drain ──► AccessKitSink  ◄── a11y-02 implements this seam
                          │
                          ▼
                 accesskit_consumer::Tree            (apply each TreeUpdate)
                          │  diff focus + live-region nodes vs. the previous tree
                          ▼
                    Announcement queue                (text + politeness: polite/assertive)
                          │
                          ▼
                 Local TTS engine (piper | espeak-ng) ──► mde-seat audio sink (the mixer)
```

- **The sink:** a11y-02 provides a `ScreenReader` type implementing
  `mde_egui::a11y::AccessKitSink`. `ingest()` feeds each `TreeUpdate` into an
  `accesskit_consumer::Tree` (the crate is AccessKit's own read-side; it applies updates
  and gives a walkable, queryable tree). This is the entire integration point with
  a11y-01 — **no change to `run_drm`'s loop** beyond swapping the default `LatestTree`
  sink for the `ScreenReader` (via `A11yBridge::with_sink`, which already exists).
- **What to announce:** diff the new tree against the retained previous one:
  - **Focus move** → announce the newly focused node's accessible name + role (+ value
    for editable/selected controls).
  - **Live regions** → `Role::Status` / `Role::Alert` nodes whose text changed, honouring
    `Live::Polite` (queue behind current speech) vs `Live::Assertive` (interrupt). The
    shell already annotates these (NOTIF-11/13), so notifications/alerts speak for free.
  - Coalesce rapid changes; drop stale polite items when a newer assertive one arrives.
- **`wants_refresh()`:** the `ScreenReader` returns `true` when it needs a fresh tree it
  doesn't yet have (e.g. just toggled on, or a coarse re-scan) so a11y-01's loop renders
  one — the wake seam is already wired.
- **TTS engine:** an offline, airgap-safe engine — **piper** (neural, better voice, ships
  a self-contained ONNX voice model) preferred, **espeak-ng** (tiny, formant, always
  available) as the guaranteed fallback. Gate the live engine behind a cargo feature the
  same way `media-mpv` / `live-vdi` gate their native stacks; the default build degrades
  to no audio (honest §7). Audio routes through `mde-seat`'s mixer (the one hardware-audio
  library, lock 1) — the reader does not open its own PCM device.
- **Crate shape:** a new `mde-a11y-reader` desktop-shell-tier lib (`ScreenReader` +
  announcement model + a `Tts` trait with piper/espeak backends). The shell owns it and
  hands it to `run_drm` as the sink. The tree-diff → announcement logic is pure and
  unit-tested headlessly (feed synthetic `TreeUpdate`s, assert the announcement queue);
  only the TTS backend is hardware/model-gated.

### The hotkey toggle

Add a `ScreenReader` action to the fixed compiled-in hotkey table
(`crates/desktop/mde-seat/src/hotkeys.rs`) — a new `HotkeyAction::ScreenReaderToggle`
with a `label()` ("Toggle screen reader") and a chord (proposed `Super+r`; verify no
collision with the existing `HOTKEYS` entries — the table has a uniqueness self-test).
The table is a compile-time constant by design (lock 9: auditable, no persistence, no
untyped verbs), so this is one typed row. The shell's hotkey dispatcher flips the reader
on/off and, on enable, calls `enable_accesskit()` on the live context + persists a seat
accessibility setting so it survives a restart (the persisted setting also seeds
`A11yBridge::from_env`, letting a seat boot with the reader already on). First
announcement on toggle-on: "Screen reader on".

### Sequencing

1. **a11y-02a** — `mde-a11y-reader` crate: the `accesskit_consumer` tree-diff →
   announcement model + the `Tts` trait, with a headless no-audio backend. Pure, unit-
   tested. Implements `mde_egui::a11y::AccessKitSink`.
2. **a11y-02b** — the espeak-ng backend (always-available fallback) behind a feature; wire
   audio through `mde-seat`'s mixer.
3. **a11y-02c** — the piper neural backend + bundled voice model (airgap: model ships in
   the RPM assets; verify with `rpm -qlp`).
4. **a11y-02d** — the `HotkeyAction::ScreenReaderToggle` row + shell dispatch + persisted
   seat setting + `A11yBridge::with_sink(ScreenReader)` wired into `run_drm`'s call site.
5. **a11y-02e** — live-verify on a real seat with a running screen-reader session
   (announce focus + a live alert); this is the deferred live-AT-client step a11y-01
   flagged.

### Explicit non-goals for a11y-02

- No external AT-SPI/Orca bridge (airgap; the reader is in-process). The eframe windowed
  fallback keeps its own AT-SPI adapter for dev hosts, but the shipped bare-DRM reader
  does not depend on an AT-SPI bus.
- No braille display, no magnifier — separate later epics if scoped.
