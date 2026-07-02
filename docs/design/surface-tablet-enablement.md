# Microsoft Surface tablet/laptop enablement (SURFACE-1..11)

> **Status: LOCKED 2026-07-02** — a two-round operator survey (Round 1: 10 Q hardware
> enablement; Round 2: 6 Q display + touch). Full Microsoft Surface (Pro/Book/Laptop/Go)
> enablement — **installation, testing, verification** surfaced as a model-gated
> **"Surface / Hardware Enablement"** card under the **This Node** plane in the Workbench,
> plus the display (native LCD / HD) + touch-in-shell work the DRM-native egui shell needs
> to be a real tablet.

## The locks

### Round 1 — hardware enablement

| # | Fork | Lock |
|---|------|------|
| 1 | Delivery | **Bake linux-surface into the bootc Workstation image.** The patched kernel/modules + iptsd + firmware + surface-control ship as an image layer (update = image swap, E12's model). No day-2 kernel surgery; the Workbench drives activate/verify/config, not a from-scratch install. |
| 2 | Scope | **The full linux-surface matrix** — touchscreen + pen/stylus (iptsd), Type Cover kbd/trackpad, Surface Aggregator Module (SAM: battery/thermal/perf), auto-rotation + accelerometer, cameras, WiFi/BT quirks, S0ix suspend, fingerprint/Hello where supported. Each a probed line item. |
| 3 | Detection | **DMI auto-detect → per-model profile.** A mackesd probe reads `/sys/class/dmi/id` (vendor 'Microsoft Corporation' + product) and matches a built-in model table (Pro/Book/Laptop/Go + generation); the card auto-appears with that model's exact subsystem checklist. Non-Surface nodes never see it. |
| 4 | Install action | **A `surface_enable` mackesd worker** (typed verbs, no raw shell): enable/start iptsd, apply per-model config (SAM perf profile, iptsd calibration, rotation hints), and drive Secure-Boot MOK enrollment. Injectable seam; live actions integration-gated with honest typed errors, never faked. |
| 5 | Verify | **Per-subsystem live probes → green/red/degraded board** (OW-10 self-test idiom): touch reports events, pen reports pressure/tilt, Type Cover enumerated, SAM battery+thermal readable, accelerometer yields orientation, camera opens a frame, WiFi/BT up, S0ix residency advances after suspend, fingerprint enrolls. Interactive-gesture probes (pen/suspend) prompt the operator honestly. |
| 6 | Secure Boot | **Guided MOK enrollment.** Detect Secure-Boot state; if modules are blocked, stage the key (`mokutil --import`), **typed-arm the reboot**, and show exactly what the blue MOK-Manager firmware screen will ask (the one-time password). Post-reboot verify the key enrolled + modules load. Honest about the manual firmware step no software can automate. |
| 7 | Fleet view | **Local-first + a compact mesh summary.** The full install/verify/config area lives under This Node; each Surface node also publishes `state/hardware/surface/<node>` (model, enablement %, any red subsystem) so the Controller/fleet rollup shows which Surfaces are healthy. Visibility only — no remote control (that would need OW-15). |
| 8 | Updates | **fwupd/LVFS firmware panel + enablement rides the image.** A bootc update carries enablement forward (no reapply); separately the area surfaces device firmware via fwupd/LVFS (UEFI, touch controller, SAM — current/available versions, typed-armed `fwupdmgr` apply). Verify re-runs after a firmware/image change. |
| 9 | 2-in-1 | **Detect + drive the shell reaction here.** Watch SW_TABLET_MODE + Type Cover attach/detach → publish `event/hardware/formfactor` (Tablet/Laptop); AND implement the shell's tablet-mode UX (auto-rotate, OSK, touch layout) in this epic (operator scoped it in). |
| 10 | Placement | **A model-gated "Surface / Hardware Enablement" card in the This Node plane** with three tabs — **Install** (activate/MOK/firmware), **Test** (the probe board), **Config** (iptsd sensitivity, rotation lock, SAM perf, tablet-mode behavior). One Workstation bootc image; a `surface-tools` group + linux-surface layer conditionally present, inert on non-Surface hardware. |

### Round 2 — display + touch

| # | Fork | Lock |
|---|------|------|
| 11 | Native res | **DRM/EDID native detect + fractional HiDPI scale.** Read the connector's preferred mode + physical size, set the framebuffer to native, compute a fractional egui `pixels_per_point` (~2.0–2.25) so UI is crisp + correctly sized; scale shown + adjustable on the card. |
| 12 | HD mode | **A real DRM mode picker.** List the connector's real KMS modes (native + 1080p + others from EDID); choosing HD does an actual modeset to 1920×1080 — fewer pixels for wgpu AND for VDI streaming (less to encode/ship). Revertible; active mode shown. |
| 13 | Touch input | **libinput/evdev multitouch in the shell's DRM seat → egui touch/pointer events.** Extend the seat's input path to read the touchscreen via libinput (kernel evdev the iptsd stack feeds); translate contacts into egui `Event::Touch` + synthesized pointer, coordinate-transformed to the active mode/rotation. Multitouch preserved for gestures. |
| 14 | On-screen kbd | **A native egui OSK** (Quasar tokens) as a shell overlay, auto-raised when formfactor=Tablet AND a text field has focus, injecting into the same input pipeline; layout + compact/numeric modes; dismissable + manually toggleable. No external IM dependency (bare-DRM shell). |
| 15 | Auto-rotate | **Accelerometer → KMS rotation + matching touch-matrix transform**, auto on orientation change (display + touch rotate as one), with a rotation-lock toggle (honoring a hardware lock if present). |
| 16 | Touch UX | **Full gesture set + touch-mode layout adaptation** — two-finger scroll, pinch-zoom, long-press=right-click, edge-swipes (dock/tablet bar); PLUS in tablet mode the shell bumps hit-target sizes/spacing (a touch density in `Style`). Auto-engages on the Tablet signal, reverts in laptop mode. |

## Architecture

```
This Node plane (mde-shell-egui) ─ "Surface / Hardware Enablement" card (model-gated)
  ├ Install tab   → action/hardware/surface/* (activate · MOK-enroll · fwupd apply)
  ├ Test tab      → state/hardware/surface/<node>/probes (the tri-state board)
  └ Config tab    → iptsd/rotation/SAM/tablet-behavior + the DRM mode picker + scale

mackesd  ── surface_detect (DMI→model profile)         ── typed verbs, no raw shell
         ── surface_enable worker (iptsd activate, per-model config, MOK flow)
         ── surface_verify (per-subsystem live probes, tri-state)
         ── surface_firmware (fwupd/LVFS list + apply)
         └ publishes state/hardware/surface/<node> (compact fleet summary)
                         event/hardware/formfactor (Tablet|Laptop)

DRM shell seat (mde-egui drm.rs + mde-shell-egui) ── consumes the above:
  · EDID/native mode + fractional scale + KMS mode picker (native↔HD)
  · libinput/evdev multitouch → egui touch/pointer (mode/rotation transform)
  · accelerometer → KMS rotation + touch matrix (+ lock)
  · native egui OSK (auto on Tablet+text-focus)
  · gestures + touch-density layout (on the Tablet formfactor signal)

bootc Workstation image ── conditional linux-surface layer + surface-tools group + fwupd
```

- **One Workstation image** (lock 1/10): the linux-surface kernel/modules + iptsd +
  firmware + surface-control + a `surface-tools` package group ship as a conditional
  layer, inert on non-Surface hardware. No separate variant/role.
- **mackesd owns the hardware truth** (locks 3–8): detect → enable → verify → firmware,
  each a typed verb behind an injectable seam; live actions integration-gated (never
  faked); publishes the compact fleet summary + the formfactor signal.
- **The DRM shell consumes** (locks 11–16): the seat gains native-mode/scale + a KMS mode
  picker + multitouch + rotation + OSK + gestures. §6-clean: the shell reacts to mackesd's
  hardware signals; mackesd never reaches into the shell.

## The units (SURFACE-1..11)

- **SURFACE-1 — bootc image layer + packaging.** Conditionally bake the linux-surface
  kernel/modules + iptsd + firmware + surface-control + a `surface-tools` group + fwupd
  into the Workstation image (Containerfile + RPM requires). *(Serialize after E12-19/E12-23
  packaging — shared Cargo.toml `[generate-rpm.requires]` + Containerfile.)*
- **SURFACE-2 — DMI detection + per-model profile.** A mackesd `surface_detect` probe:
  is-Surface (DMI) + a built-in model table (Pro/Book/Laptop/Go + gen) each carrying its
  subsystem checklist; gates the card's visibility + the verify expectations.
- **SURFACE-3 — the `surface_enable` worker + guided MOK.** Activate/config iptsd + per-model
  SAM/rotation config via typed verbs; the guided MOK flow (`mokutil --import` → typed-armed
  reboot → post-reboot key+modules verify). Injectable seam, live integration-gated.
- **SURFACE-4 — per-subsystem verify probes + fleet publish.** The tri-state
  (green/red/degraded) probe board across the full matrix; interactive probes prompt
  honestly; publish the compact `state/hardware/surface/<node>` summary for the fleet rollup.
- **SURFACE-5 — fwupd/LVFS firmware panel.** List updatable Surface components + versions;
  typed-armed `fwupdmgr` apply behind the seam; verify re-runs after.
- **SURFACE-6 — the This Node "Surface / Hardware Enablement" card.** mde-shell-egui,
  model-gated, three tabs (Install/Test/Config) rendering the worker state (no demo_data);
  §4 tokens; headless mount test.
- **SURFACE-7 — native LCD resolution + HD mode.** DRM/EDID native detect + fractional
  HiDPI scale (adjustable) + the real KMS mode picker (native↔HD, fewer pixels) in the DRM
  shell runner; the active mode/scale shown on the Config tab.
- **SURFACE-8 — touch input in the DRM seat.** libinput/evdev multitouch → egui
  `Event::Touch` + synthesized pointer, coordinate-transformed to the active mode/rotation;
  the one input pipeline for kbd/mouse/touch.
- **SURFACE-9 — formfactor signal + auto-rotation.** Watch SW_TABLET_MODE + Type Cover
  attach/detach → `event/hardware/formfactor`; accelerometer → KMS rotation + matching
  touch-matrix transform, auto with a rotation-lock.
- **SURFACE-10 — the native egui OSK.** A shell-overlay on-screen keyboard (Quasar tokens),
  auto-raised on Tablet + text-focus, injecting into the input pipeline; layout +
  compact/numeric; dismissable/toggleable.
- **SURFACE-11 — gestures + touch-density layout.** Two-finger scroll, pinch-zoom,
  long-press=right-click, edge-swipes on the multitouch pipeline; a touch density in `Style`
  (bigger targets/spacing) auto-engaged on the Tablet signal, reverted in laptop mode.

**Serialization**: SURFACE-2→3→4 (mackesd workers, share `workers/mod.rs`; 3 needs 2's
profile, 4 needs 3's activation). SURFACE-1 after the E12-19/E12-23 packaging settles.
SURFACE-6 after 3/4 (renders their state). SURFACE-7→8→9→10→11 serialize on the DRM
shell seat/runner (shared input path); 8 underpins 9/11 (multitouch), 9's formfactor
signal drives 10/11's auto-engage. SURFACE-5 parallelizes with 6 once 2/3 land.

## Acceptance (epic-level, runtime-observable)

1. Booting a Surface Pro shows the model-gated card in This Node with its exact subsystem
   checklist; a non-Surface node never shows it.
2. The Install tab activates iptsd + walks MOK enrollment (typed-armed reboot, honest
   firmware-prompt copy); post-reboot the modules load and the card reflects it.
3. The Test tab's probe board shows each subsystem green/red/degraded with the real reason;
   touch/pen actually produce input events, SAM battery/thermal read live, the accelerometer
   yields orientation.
4. The fleet rollup shows each Surface node's compact enablement summary; a red subsystem is
   visible without visiting the node.
5. The panel runs at native resolution with crisp fractional-scaled UI; switching to HD does
   a real modeset (fewer pixels) and back.
6. Touch works in the shell: tap/scroll/pinch/long-press/edge-swipe; detaching the Type Cover
   raises the OSK and text entry works; rotating the device rotates display + touch together
   (taps land correctly); a rotation-lock holds it.
7. fwupd lists Surface firmware and a typed-armed apply updates it; verify re-runs after.

## Risks / out of scope

- **Risks**: MOK enrollment's manual firmware step (mitigate: honest guided copy + post-reboot
  verify); fingerprint/Hello + cameras are the flakiest Linux subsystems (verify reports
  degraded honestly rather than faking); DRM modeset/rotation correctness with the touch
  matrix (the touch-transform test is the guard); linux-surface kernel tracking across bootc
  base bumps (the image layer pins it).
- **Out of scope**: remote-driving another node's Surface enablement (visibility only, lock 7 —
  would need OW-15); non-Surface convertibles (Surface-specific model table for v1); a separate
  Surface image/role (one image + conditional layer, lock 10); pen palm-rejection tuning UI
  beyond iptsd defaults; Windows-Hello face auth (IR camera stack is out of reach).
