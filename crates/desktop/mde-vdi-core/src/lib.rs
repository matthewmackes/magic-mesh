//! `mde-vdi-core` — the transport-neutral core the Quasar VDI backends share.
//!
//! MCNF 12.0 "Quasar" renders remote desktops **egui-native**: `mde-vdi-rdp`,
//! `mde-vdi-vnc` and `mde-vdi-spice` each turn a wire framebuffer into an
//! [`egui::ColorImage`] and forward [`egui::Event`]s back as protocol input. Three
//! transports, but the *egui-facing* seam is the same shape — so it was
//! byte-for-byte duplicated across all three (arch-8): the same coordinate clamp,
//! the same PC/AT set-1 scancode map, the same Shift/Ctrl/Alt diff, the same RGBA8
//! desktop surface. A single scancode fix had to be hand-applied three times.
//!
//! This crate is the one home for the genuinely-shared pieces:
//!
//! * [`input`] — the egui coordinate clamp ([`clamp_u16`]), the wheel dominant-axis
//!   pick ([`dominant_axis`]), the PC/AT **set-1** scancode identity ([`Scancode`])
//!   and map ([`scancode_for`]) that RDP + SPICE share, and the transport-generic
//!   modifier-diff tracker ([`ModifierTracker`] / [`ModKey`]).
//! * [`pixel`] — the [`RgbaSurface`]: the persistent, tightly-packed RGBA8 desktop
//!   store (opaque-black init + [`egui::ColorImage`] hand-off) each transport's
//!   `Framebuffer` wraps and paints into with its own protocol-specific blit.
//! * [`damage`] — the per-frame damage rectangles ([`DamageRect`] / [`DamageLog`] /
//!   [`FrameDamage`]) a transport records as it blits, plus the pure slice math
//!   ([`sub_color_image`]) the shell partial-uploads with, so a changed frame moves
//!   only its damaged sub-rectangles to the GPU instead of the whole framebuffer.
//!
//! What legitimately **differs** stays in each transport crate: VNC's X11 keysym
//! map + DES challenge + RFB pixel format, SPICE's surface-format tags + scancode
//! wire packing, RDP's pixel format, and every transport's own framebuffer
//! mutation. This core force-merges nothing that diverges.
//!
//! It is transport-free and fully unit-tested with synthetic data (governance §7):
//! the tested logic is the shipped logic. egui is re-exported from the shared
//! `mde-egui` harness so every surface resolves to the one harness-pinned egui (no
//! cross-surface version skew, §4).

// P2 perf-12: the transport-neutral core the VDI backends share turns UNTRUSTED wire
// framebuffers into egui surfaces (pixel blit / damage / coordinate clamp). A stray
// `.unwrap()`/`.expect()` on that path is a remote-triggerable panic (DoS-adjacent),
// so deny both in non-test code. Test code keeps them for terse assertions.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

// Re-export the toolkit through the harness so this core and the three backends
// share exactly one egui resolution.
pub use mde_egui::egui;

pub mod damage;
pub mod input;
pub mod pixel;

pub use damage::{paint_sub_image, sub_color_image, DamageLog, DamageRect, FrameDamage};
pub use input::{
    clamp_u16, dominant_axis, scancode_for, ModKey, ModifierTracker, Scancode, ALT_SCANCODE,
    CTRL_SCANCODE, SHIFT_SCANCODE,
};
pub use pixel::RgbaSurface;
