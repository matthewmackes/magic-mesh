//! `mde-egui` — the MCNF **E12 "Construct"** egui harness.
//!
//! E12 retires the Cosmic-era toolkit and rewrites every UI surface from
//! the iced-based stack to **egui** (governance §4/§5/§6). This crate is the shared
//! foundation every surface is built on — the three things lock 14 says ship
//! first:
//!
//! 1. [`runner::run_client`] — the **eframe Wayland-client runner**. One call
//!    stands a surface up as an `eframe` (egui + winit + wgpu) Wayland client with
//!    the shared look already installed.
//! 2. [`style::Style`] — the **single source of look** (`Style`/`Visuals`). A Rust
//!    module, not a token crate: there is no raw-literal lint gate (E12 lock 9, the
//!    §0-Simple lever), so `Style` *is* the discipline. Surfaces never hand-roll
//!    colours or spacing — they read `Style` and call [`Style::install`].
//! 3. [`motion::Motion`] — the small shared **duration/easing table** wrapping
//!    egui's built-in `animate_bool` (E12 lock 10 — no bespoke motion engine, no
//!    motion lint gate).
//!
//! **Accessibility is opt-in**: the `accesskit` feature enables egui/eframe's
//! AccessKit tree generation for shell surfaces that need screen-reader semantics.
//! [`a11y`] carries the runtime consumer seam (a11y-01) — the [`a11y::AccessKitSink`]
//! the bare-DRM present loop drains each frame into, plus the [`a11y::A11yBridge`] that
//! turns tree generation on (gated, default OFF) so the shipped seat actually exports a
//! tree instead of only doing so from `#[cfg(test)]`. The windowed eframe fallback
//! ([`run_client`]) gets AccessKit for free from eframe's own AT-SPI adapter, which
//! lazily activates on the first assistive-technology request.
//!
//! The crate has **zero retired-toolkit (Cosmic/iced) dependencies** — it depends
//! only on `egui`/`eframe`. Both are re-exported so every surface resolves to the one
//! harness-pinned egui version (no cross-surface version skew).

pub mod a11y;
pub mod carbon;
pub mod code;
pub mod display;
pub mod focus;
pub mod fonts;
pub mod formfactor;
pub mod gestures;
pub mod hostkeys;
pub mod input_policy;
pub mod menubar;
pub mod motion;
pub mod runner;
pub mod search_omnibox;
pub mod style;
pub mod toast;
pub mod touch;
pub mod video_plane;
pub mod widgets;

// E12-2: the bare-seat DRM/KMS backend (no compositor), behind `feature = "drm"`.
#[cfg(feature = "drm")]
pub mod drm;

pub use carbon::{
    CarbonRaster, carbon_icon, carbon_names, carbon_raster, carbon_svg_bytes, carbon_texture,
    paint_carbon,
};
pub use code::CodeToken;
pub use display::{
    DisplayController, EdidError, EdidPanel, HeadlessModeset, ModeClass, ModesetError, ModesetSeam,
    PanelInfo, PanelMode, build_mode_list, fractional_scale, panel_dpi, parse_edid,
    scale_for_panel, select_mode,
};
pub use formfactor::{
    AccelSensor, AutoRotate, Formfactor, FormfactorDebounce, HeadlessRotate, Orientation,
    RotateCommand, RotateError, RotationApply, SensorError, SwitchState, SysfsAccel,
    apply_rotation, drain_formfactor, orientation_from_accel, push_formfactor, request_rotation,
    take_rotation_commands,
};
pub use gestures::{
    Edge, Gesture, GestureConfig, GestureRecognizer, drain_edge_swipes, push_edge_swipe,
};
pub use input_policy::{InputPolicy, input_policy, pointer_button, set_input_policy};
pub use menubar::{
    ChipTone, Entry, Item, Menu, MenuBar, MenuBarModel, StatusChip, resolve_mnemonics,
};
pub use motion::{
    Animated, AnimatedColor, AnimatedOpacity, AnimatedRect, AnimatedScalar, AnimatedScale,
    AnimatedSize, AnimatedVec2, Motion, MotionEasing, MotionMode, MotionOpacity, MotionPreset,
    MotionScale, MotionSpec, MotionValue, Phase, StatusMotion,
};
pub use runner::run_client;
pub use style::{Density, GradeBand, LayoutProfile, Style, StyleColorScheme, StylePalette};
pub use toast::{
    ChyronInteraction, Dwell, OsdKind, OsdLevel, Severity, Tier, Toast, ToastAction, ToastHost,
};
pub use touch::{RawContact, Rotation, TouchTransform, TouchTranslator};
pub use video_plane::{
    FakeCatalog, FallbackReason, FbToken, PaneRect, Placement, PlaneCatalog, PlaneInfo, PlaneKind,
    PlaneSet, RecordingScanout, VideoPath, VideoPlaneError, VideoPlanePlan, VideoScanout,
    clamp_and_crop, fit_rect, plane_placement, present_frame,
};
pub use widgets::{
    OperationProgressView, field, muted_note, operation_progress_text, operation_progress_value,
    paint_operation_progress_badge, status_dot,
};

#[cfg(feature = "drm")]
pub use drm::{
    DrmVideoScanout, PrimeImportLiveness, probe_primary_video_plane, probe_prime_import_liveness,
    probe_video_plane, run_drm,
};

// Re-export the toolkit so surfaces depend on `mde-egui` alone and share one
// egui/eframe resolution.
pub use eframe;
pub use egui;
