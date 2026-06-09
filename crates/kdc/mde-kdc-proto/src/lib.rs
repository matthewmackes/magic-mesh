//! KDC2-2 — greenfield Rust KDE Connect protocol library.
//!
//! v2.1 lock (2026-05-22): pure-library re-implementation of the
//! KDE Connect wire protocol. Wire-compatible with the upstream
//! protocol (stock Android/iOS clients pair without modification),
//! but with one MDE-specific extension: a capability-negotiation
//! header on every handshake. Two MDE peers exchanging that header
//! unlock MDE-only features (mesh-relay, peer-card-probe-share,
//! richer Notification dual-send semantics). Stock clients ignore
//! the header — graceful fallback.
//!
//! Why a stand-alone crate (instead of part of mde-kdc)?
//!
//! - `mde-kdc` (host integration, KDC2-3) needs a D-Bus host, a
//!   filesystem pairing store, and a networking stack. Keeping
//!   them out of this crate means the protocol layer compiles +
//!   tests in seconds and stays embeddable.
//! - `cargo fuzz` corpus and the in-process loopback harness
//!   (KDC2-2 acceptance) live next to the code they exercise
//!   without dragging in tokio + zbus + rustls just to run a
//!   fuzz iteration.
//! - Strict layer boundaries per
//!   `~/.claude/projects/.../memory/project_v2_1_kdc2_native.md`:
//!   `Protocol → Router → Daemon API → Surface`. The protocol
//!   layer never knows the mesh or peer-card exist.
//!
//! ## Module map
//!
//! - [`codec`] — frame encoding/decoding. The wire's
//!   newline-delimited JSON frame structure + framing helpers.
//! - [`crypto`] — RSA-2048 pairing handshake + AES-256-GCM
//!   session key. Trait surface only here; impls land in
//!   KDC2-2.4.
//! - [`discovery`] — UDP-broadcast peer-discovery announcements +
//!   the mesh-shunt synthetic-mDNS injection point (KDC2-4).
//! - [`plugins`] — per-feature plugin trait + the canonical
//!   plugin registry (ping, clipboard, share, notification,
//!   findmyphone, battery, mpris, sms, telephony).
//! - [`wire`] — top-level message types + `Packet` envelope +
//!   the [`CapabilitiesHeader`] every handshake carries.
//!
//! Per the v2.1 KDC2 lock the v13.0 `mackes-kdc` crate's schema
//! (re-exported as [[project_v13_kdeconnect]]) is **superseded**;
//! the new wire types in this crate are the single source of
//! truth.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod codec;
pub mod crypto;
pub mod discovery;
pub mod dispatch;
pub mod plugins;
pub mod wire;

/// Protocol version this crate implements.
///
/// Mirrors the KDE Connect upstream wire protocol's version
/// integer that ships in every `kdeconnect.identity` handshake
/// packet. The number matches upstream (currently 7) so handshake
/// negotiation with stock clients works without translation. The
/// MDE capability-negotiation header in [`wire::CapabilitiesHeader`]
/// is layered ON TOP — same version, extra optional header.
pub const PROTOCOL_VERSION: u32 = 7;

/// Stable identifier for an MDE deployment of the KDC protocol.
///
/// Surfaced in the `kdeconnect.identity.deviceName` field's
/// `[mde]` suffix (where stock clients show `Pixel 8` for a
/// phone, an MDE peer shows `lab-01 [mde]`). Lets the operator
/// tell at a glance from the Android KDE Connect device list
/// which peers are MDE vs. stock.
pub const MDE_DEVICE_NAME_SUFFIX: &str = "[mde]";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_version_matches_kdc_upstream() {
        // Upstream's `kdeconnect.identity.protocolVersion` has been
        // 7 since the v22.04 release. Bumping this requires a
        // matching survey + a new memory note recording the
        // wire-compat implications.
        assert_eq!(PROTOCOL_VERSION, 7);
    }

    #[test]
    fn device_name_suffix_is_short_and_stable() {
        // Append-safe: a device name like "lab-01" + this suffix
        // stays under KDC's 64-byte device-name limit.
        assert!(MDE_DEVICE_NAME_SUFFIX.starts_with('['));
        assert!(MDE_DEVICE_NAME_SUFFIX.ends_with(']'));
        assert!(MDE_DEVICE_NAME_SUFFIX.len() < 16);
    }
}
