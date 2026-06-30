//! `mde-vdi-rdp` — render a remote **RDP** desktop into an egui texture.
//!
//! MCNF 12.0 "Quasar" is a mesh-native thin-client desktop OS whose entire
//! interface is egui (`docs/design/quasar-vdi-desktop.md`). RDP is the **primary**
//! remote-desktop protocol (lock 21): a desktop on another mesh peer is connected
//! over Nebula and **rendered egui-native** — the remote framebuffer is decoded
//! into an [`egui::ColorImage`] the shell uploads to a `TextureHandle`, with
//! **no external viewer**. The pure-Rust [`ironrdp`](https://crates.io/crates/ironrdp)
//! stack (Devolutions) drives the wire protocol.
//!
//! # Shape
//!
//! ```text
//!   egui::Event ──▶ input::map_event ──▶ RdpInputEvent ─▶ wire ─▶ ironrdp input PDU
//!                                            ▲                         │
//!   RdpSession ────────────────────────────┘                         ▼
//!       │  apply_rect / apply_full_frame  ◀── wire ◀── ironrdp bitmap/codec decode
//!       ▼
//!   frame() ──▶ egui::ColorImage ──▶ shell TextureHandle
//! ```
//!
//! The **egui-facing surface** — the framebuffer→[`egui::ColorImage`] decode
//! ([`pixel`]) and the [`egui::Event`]→RDP-input mapping ([`input`]), tied
//! together by [`RdpSession`] — is `ironrdp`-free and fully unit-tested with
//! synthetic inputs (governance §7: the tested logic is real, not mocked). The
//! live connection sequence + session pump that talks to a real peer is the
//! `ironrdp`-dependent layer; a live connect needs a server, so it is exercised
//! by an example / integration test, never the unit path.
//!
//! egui itself is re-exported from the shared `mde-egui` harness so every surface
//! resolves to the one harness-pinned egui (no cross-surface version skew, §4).

// Re-export the toolkit through the harness so the shell and this backend share
// exactly one egui resolution.
pub use mde_egui::egui;

pub mod config;
pub mod input;
pub mod pixel;
pub mod session;

pub use config::{ConfigError, RdpConfig};
pub use input::{map_event, map_text, scancode_for, MouseButton, RdpInputEvent, Scancode};
pub use pixel::{Framebuffer, FramebufferError, PixelFormat};
pub use session::RdpSession;
