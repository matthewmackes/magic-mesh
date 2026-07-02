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
//! **Accessibility is deferred** (E12 lock 11): the `accesskit` eframe feature is
//! intentionally not enabled. egui/eframe keep an accesskit path to wire in a
//! post-stabilization a11y epic; until then this harness ships without it.
//!
//! The crate has **zero retired-toolkit (Cosmic/iced) dependencies** — it depends
//! only on `egui`/`eframe`. Both are re-exported so every surface resolves to the one
//! harness-pinned egui version (no cross-surface version skew).

pub mod display;
pub mod fonts;
pub mod hostkeys;
pub mod motion;
pub mod runner;
pub mod style;
pub mod toast;
pub mod widgets;

// E12-2: the bare-seat DRM/KMS backend (no compositor), behind `feature = "drm"`.
#[cfg(feature = "drm")]
pub mod drm;

pub use display::{
    build_mode_list, fractional_scale, panel_dpi, parse_edid, scale_for_panel, select_mode,
    DisplayController, EdidError, EdidPanel, HeadlessModeset, ModeClass, ModesetError, ModesetSeam,
    PanelInfo, PanelMode,
};
pub use motion::Motion;
pub use runner::run_client;
pub use style::Style;
pub use toast::{
    ChyronInteraction, Dwell, OsdKind, OsdLevel, Severity, Tier, Toast, ToastAction, ToastHost,
};
pub use widgets::{field, muted_note, status_dot};

#[cfg(feature = "drm")]
pub use drm::run_drm;

// Re-export the toolkit so surfaces depend on `mde-egui` alone and share one
// egui/eframe resolution.
pub use eframe;
pub use egui;
