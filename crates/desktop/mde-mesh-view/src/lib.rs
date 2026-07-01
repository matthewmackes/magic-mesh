//! `mde-mesh-view` — a live, procedural **mesh-state canvas** widget (MCNF E12
//! "Quasar"). The egui reincarnation of the old MESHMAP.
//!
//! [`MeshView`] draws the *current* mesh — nodes by role + health, the links
//! between them, the elected leader, and per-link activity — **procedurally with
//! [`egui::Painter`]** (`line_segment` / `circle_filled` / `circle_stroke` /
//! `text`; no pre-rendered images), animated every frame off the egui frame
//! clock like the [egui clock demo](https://www.egui.rs/#clock). Everything is
//! themed through the shared [`mde_egui::Style`] / [`mde_egui::Motion`] — there
//! is no raw colour and no bespoke motion engine (§4).
//!
//! The widget renders **only** the [`MeshState`] it is handed — there is no
//! embedded demo data in the render path. An empty state (no nodes) paints an
//! honest "waiting for mesh" `EmptyState`, never a blank canvas or fabricated
//! peers (§6/§7). The runnable sample lives in `examples/mesh_view.rs`:
//!
//! ```text
//! cargo run -p mde-mesh-view --example mesh_view
//! ```
//!
//! ## Shape
//! - [`mod@state`] — the plain input data ([`MeshState`] / [`MeshNode`] /
//!   [`MeshLink`] / [`Role`] / [`Health`]). No egui context, no mesh-substrate
//!   dependency.
//! - [`mod@layout`] — the **pure** layout math (radial auto-placement, screen
//!   mapping, pulse interpolation), unit-tested without a GPU.
//! - [`MeshView`] — the painter over those results.
//!
//! Tier (§6): desktop-shell. Depends only on the shared `mde-egui` harness — the
//! edge points inward, so the mesh substrate stays headless-capable.

// This crate paints a canvas: integer node counts/indices and the egui frame
// clock are converted to `f32`/`f64` for trigonometry and pixel positions.
// Those numeric casts are inherent to canvas math, so the pedantic cast lints
// are allowed crate-wide with this documented rationale (rather than scattering
// per-site `#[allow]`s). `suboptimal_flops` is allowed for the same reason: the
// easing/alpha expressions read far clearer as `a - b * c.cos()` than as the
// `mul_add` rewrite, and the precision/throughput gain is irrelevant for a few
// pixel positions per frame.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::suboptimal_flops
)]

pub mod layout;
pub mod state;
mod view;

pub use state::{Health, MeshLink, MeshNode, MeshState, Role};
pub use view::MeshView;
