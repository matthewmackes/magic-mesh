//! `mde-egui` — the MCNF **E12 "Quasar"** egui harness.
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
pub mod code;
pub mod display;
pub mod fonts;
pub mod formfactor;
pub mod gestures;
pub mod hostkeys;
pub mod menubar;
pub mod motion;
pub mod runner;
pub mod style;
pub mod toast;
pub mod touch;
pub mod video_plane;
pub mod widgets;

// E12-2: the bare-seat DRM/KMS backend (no compositor), behind `feature = "drm"`.
#[cfg(feature = "drm")]
pub mod drm;

pub use code::CodeToken;
pub use display::{
    build_mode_list, fractional_scale, panel_dpi, parse_edid, scale_for_panel, select_mode,
    DisplayController, EdidError, EdidPanel, HeadlessModeset, ModeClass, ModesetError, ModesetSeam,
    PanelInfo, PanelMode,
};
pub use formfactor::{
    apply_rotation, drain_formfactor, orientation_from_accel, push_formfactor, request_rotation,
    take_rotation_commands, AccelSensor, AutoRotate, Formfactor, FormfactorDebounce,
    HeadlessRotate, Orientation, RotateCommand, RotateError, RotationApply, SensorError,
    SwitchState, SysfsAccel,
};
pub use gestures::{
    drain_edge_swipes, push_edge_swipe, Edge, Gesture, GestureConfig, GestureRecognizer,
};
pub use menubar::{
    resolve_mnemonics, ChipTone, Entry, Item, Menu, MenuBar, MenuBarModel, StatusChip,
};
pub use motion::{Motion, StatusMotion};
pub use runner::run_client;
pub use style::{Density, GradeBand, Style};
pub use toast::{
    ChyronInteraction, Dwell, OsdKind, OsdLevel, Severity, Tier, Toast, ToastAction, ToastHost,
};
pub use touch::{RawContact, Rotation, TouchTransform, TouchTranslator};
pub use video_plane::{
    clamp_and_crop, fit_rect, plane_placement, present_frame, FakeCatalog, FallbackReason, FbToken,
    PaneRect, Placement, PlaneCatalog, PlaneInfo, PlaneKind, PlaneSet, RecordingScanout, VideoPath,
    VideoPlaneError, VideoPlanePlan, VideoScanout,
};
pub use widgets::{field, muted_note, status_dot};

#[cfg(feature = "drm")]
pub use drm::{
    probe_primary_video_plane, probe_prime_import_liveness, probe_video_plane, run_drm,
    DrmVideoScanout, PrimeImportLiveness,
};

// Re-export the toolkit so surfaces depend on `mde-egui` alone and share one
// egui/eframe resolution.
pub use eframe;
pub use egui;
