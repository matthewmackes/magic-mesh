//! `mde-vdi-spice` — render a remote **SPICE** desktop into an egui texture.
//!
//! MCNF 12.0 "Construct" is a mesh-native thin-client desktop OS whose entire
//! interface is egui (`docs/design/quasar-vdi-desktop.md`). The Desktop Chooser
//! (`docs/design/desktop-chooser.md`, lock CHOOSER-5) presents SPICE consoles
//! alongside RDP/VNC; SPICE is the native console for a QEMU/KVM guest, so this
//! crate connects to a guest's SPICE console over Nebula and **renders it
//! egui-native** — the remote framebuffer becomes an [`egui::ColorImage`] the
//! shell uploads to a `TextureHandle`, with **no external viewer**, exactly like
//! [`mde-vdi-rdp`](https://docs.rs/mde-vdi-rdp) / `mde-vdi-vnc`.
//!
//! # Airgap outcome (CHOOSER-5)
//!
//! The design doc allowed an honest VNC-compat fallback if a SPICE stack were not
//! airgap-obtainable. It **is**: the pure-Rust [`spice-client`](https://crates.io/crates/spice-client)
//! stack fetches + builds `--locked` on the farm with default features only (the
//! optional `backend-gtk4` C libraries stay off — the shell owns rendering). So
//! this is a **first-class SPICE client**, not the fallback. `spice-client` drives
//! the wire protocol (main/display/inputs channels + image decode); this crate
//! owns the egui-facing surface.
//!
//! # Shape
//!
//! ```text
//!   egui::Event ──▶ input::map_event ──▶ SpiceInputEvent ─▶ connect ─▶ spice-client inputs channel
//!                                              ▲                              │
//!   SpiceSession ──────────────────────────────┘                              ▼
//!       │  apply_surface  ◀── connect::pump_frame ◀── spice-client DisplaySurface (decoded)
//!       ▼
//!   frame() ──▶ egui::ColorImage ──▶ shell TextureHandle
//! ```
//!
//! The **egui-facing surface** — the display-surface→[`egui::ColorImage`] decode
//! ([`pixel`]) and the [`egui::Event`]→SPICE-input mapping ([`input`]), tied
//! together by [`SpiceSession`] — is transport-free and fully unit-tested with
//! synthetic inputs (governance §7: the tested logic is real, not mocked). The
//! live connection + channel pump that talks to a real console is the
//! `spice-client`-dependent layer ([`connect`]); its connect path is proven
//! **headless** against a closed loopback port (`tests/loopback_spice.rs` — the
//! real connect runs end-to-end and surfaces failure as a typed error, never a
//! hang), and the full connect→frame→input round-trip is env-gated against a real
//! KVM SPICE console (`tests/live_spice.rs`). The pump feeds the same
//! [`SpiceSession::apply_surface`] the unit "a decoded surface → a frame" test
//! drives, so the tested path and the shipped path do not diverge.
//!
//! egui itself is re-exported from the shared `mde-egui` harness so every surface
//! resolves to the one harness-pinned egui (no cross-surface version skew, §4).

// P2 perf-12: this crate decodes UNTRUSTED remote input (the SPICE wire framebuffer /
// image / channel stream). A stray `.unwrap()`/`.expect()` on a decode path is a
// remote-triggerable panic (DoS-adjacent), so deny both in non-test code. Test code
// keeps them for terse assertions.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

// Re-export the toolkit through the harness so the shell and this backend share
// exactly one egui resolution.
pub use mde_egui::egui;

pub mod config;
pub mod connect;
pub mod input;
pub mod pixel;
pub mod session;

pub use config::{ConfigError, SpiceConfig};
pub use connect::{BlockingSpiceTransport, ConnectError, SpiceTransport};
pub use input::{map_event, scancode_for, MouseButton, Scancode, SpiceInputEvent};
pub use pixel::{Framebuffer, FramebufferError, SurfaceFormat};
pub use session::SpiceSession;
