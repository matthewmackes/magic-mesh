//! `mde-vdi-vnc` — render a remote **VNC/RFB** desktop into an egui texture.
//!
//! MCNF 12.0 "Quasar" is a mesh-native thin-client desktop OS whose entire
//! interface is egui (`docs/design/quasar-vdi-desktop.md`). VNC/RFB is the
//! **universal fallback** remote-desktop protocol (lock 21): when a guest has no
//! RDP — a bare XCP-ng/XAPI `Xvnc` console, a guest mid-boot, any OS state — the
//! desktop is reached over Nebula and **rendered egui-native**. The remote
//! framebuffer is decoded into an [`egui::ColorImage`] the shell uploads to a
//! `TextureHandle`, with **no external viewer**.
//!
//! Unlike the RDP backend, which delegates the wire decode to `ironrdp`, this
//! crate is a **pure-Rust RFB client with no external protocol dependency** — the
//! framebuffer decoder is ours, which is exactly why VNC is the universal path.
//!
//! # Shape
//!
//! ```text
//!   egui::Event ──▶ input::map_event ──▶ VncInputEvent ─▶ session ─▶ RfbClientMessage ─▶ wire bytes
//!                                                            ▲                                │
//!   VncSession ─────────────────────────────────────────────┘                                ▼
//!       │  apply_framebuffer_update / apply_rect  ◀── encoding::decode ◀── RFB FramebufferUpdate
//!       ▼
//!   frame() ──▶ egui::ColorImage ──▶ shell TextureHandle
//! ```
//!
//! The **egui-facing surface** is fully unit-tested with synthetic inputs
//! (governance §7 — the tested logic is real, not mocked):
//!
//! * [`pixel`] — the [`PixelFormat`] (RFB true-colour) → RGBA conversion and the
//!   [`Framebuffer`] the rectangles accumulate into.
//! * [`encoding`] — the pure-Rust Raw / `CopyRect` / RRE / Hextile rectangle
//!   decoders and the `FramebufferUpdate` parser.
//! * [`input`] — the [`egui::Event`] → [`VncInputEvent`] mapping (X11 keysyms +
//!   the pointer button model).
//! * [`wire`] — the [`RfbClientMessage`] (`PointerEvent` / `KeyEvent`) and
//!   [`RfbControlMessage`] (`SetPixelFormat` / `SetEncodings`) byte encoders.
//! * [`session`] — [`VncSession`] tying decode + input together.
//! * The **adaptive codec (E12-10)**: [`link`] holds the protocol-neutral
//!   link-quality estimator + the hysteresis [`QualityTier`] ladder (a weak
//!   link degrades fast, a recovered one upgrades slowly), and [`tier`] maps
//!   each tier onto the RFB knobs this client really has — pixel depth
//!   (32-bpp → RGB565 → BGR233), update-request pacing, and the encoding
//!   preference. RFB is client-steered at runtime, so tier changes apply
//!   **live** ([`TierApplication::Live`]) through the session's control queue.
//!
//! The live RFB transport — the handshake (`ProtocolVersion` / security /
//! `ServerInit`) plus the TCP read pump that fills the framebuffer and flushes
//! the input queue onto the Nebula link — is the integration-gated layer: a live
//! connect needs a server, so it is exercised by an integration test, never the
//! unit path. It calls these same methods, so the tested path and the shipped
//! path do not diverge.
//!
//! egui itself is re-exported from the shared `mde-egui` harness so every surface
//! resolves to the one harness-pinned egui (no cross-surface version skew, §4).

// Re-export the toolkit through the harness so the shell and this backend share
// exactly one egui resolution.
pub use mde_egui::egui;

pub mod config;
pub mod encoding;
pub mod input;
pub mod link;
pub mod pixel;
pub mod session;
pub mod tier;
pub mod wire;

pub use config::{ConfigError, VncConfig};
pub use encoding::{
    decode_framebuffer_update, decode_rect, parse_pixel_format, parse_rectangle_header,
    DecodeError, Encoding, Reader, Rectangle,
};
pub use input::{
    keysym_for, keysym_for_char, map_button, map_event, map_text, Button, ModifierState,
    VncInputEvent,
};
pub use link::{
    LadderConfig, LinkEstimate, LinkEstimator, LinkGrade, LinkThresholds, QualityLadder,
    QualityMode, QualityTier, TierApplication, TierChange,
};
pub use pixel::{Framebuffer, FramebufferError, PixelFormat};
pub use session::VncSession;
pub use tier::{VncTierSettings, PREFERRED_ENCODINGS};
pub use wire::{RfbClientMessage, RfbControlMessage};
