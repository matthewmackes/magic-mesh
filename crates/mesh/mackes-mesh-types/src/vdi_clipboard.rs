//! VDI clipboard capability status shared by desktop backends and broker records.
//!
//! WL-FUNC-016 accepts RDP/SPICE clipboard work only when the backend either
//! drives the protocol's real clipboard channel or reports an explicit unsupported
//! state. This type is that shared status surface: it is serializable for retained
//! Bus records and also cheap for the RDP/SPICE session crates to expose directly.

use serde::{Deserialize, Serialize};

/// RDP's real text clipboard channel is CLIPRDR. The current backend has not wired
/// that virtual channel, so both directions must report unsupported explicitly.
pub const RDP_CLIPBOARD_UNSUPPORTED_REASON: &str =
    "RDP CLIPRDR clipboard channel is not implemented in mde-vdi-rdp";

/// SPICE text clipboard rides the vdagent/main-channel clipboard messages. The
/// current backend has not wired that path, so both directions must report
/// unsupported explicitly.
pub const SPICE_CLIPBOARD_UNSUPPORTED_REASON: &str =
    "SPICE vdagent clipboard channel is not implemented in mde-vdi-spice";

/// The protocol-native channel backing a supported VDI clipboard lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VdiClipboardChannel {
    /// RDP CLIPRDR virtual channel.
    RdpCliprdr,
    /// SPICE vdagent clipboard messages.
    SpiceVdagent,
}

/// One directional clipboard lane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum VdiClipboardLaneStatus {
    /// The lane is backed by a real protocol clipboard channel.
    Supported {
        /// The protocol channel used for this direction.
        channel: VdiClipboardChannel,
    },
    /// The lane is not available and the reason is operator-visible.
    Unsupported {
        /// Human-readable reason. This must name the missing protocol path.
        reason: String,
    },
}

impl VdiClipboardLaneStatus {
    /// A directional unsupported status.
    #[must_use]
    pub fn unsupported(reason: impl Into<String>) -> Self {
        Self::Unsupported {
            reason: reason.into(),
        }
    }

    /// Whether this lane has a real protocol channel behind it.
    #[must_use]
    pub const fn is_supported(&self) -> bool {
        matches!(self, Self::Supported { .. })
    }
}

/// Bidirectional text clipboard capability for a VDI endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VdiClipboardStatus {
    /// Host/mesh clipboard materialization into the guest.
    pub host_to_guest: VdiClipboardLaneStatus,
    /// Guest clipboard publication back to the host/mesh lane.
    pub guest_to_host: VdiClipboardLaneStatus,
}

impl VdiClipboardStatus {
    /// A bidirectional unsupported report using the same explicit reason for both
    /// lanes.
    #[must_use]
    pub fn unsupported(reason: impl Into<String>) -> Self {
        let reason = reason.into();
        Self {
            host_to_guest: VdiClipboardLaneStatus::unsupported(reason.clone()),
            guest_to_host: VdiClipboardLaneStatus::unsupported(reason),
        }
    }

    /// Current RDP status: display/input are live, but CLIPRDR clipboard is absent.
    #[must_use]
    pub fn rdp_unsupported() -> Self {
        Self::unsupported(RDP_CLIPBOARD_UNSUPPORTED_REASON)
    }

    /// Current SPICE status: display/input are live, but vdagent clipboard is absent.
    #[must_use]
    pub fn spice_unsupported() -> Self {
        Self::unsupported(SPICE_CLIPBOARD_UNSUPPORTED_REASON)
    }

    /// Whether both directions are backed by real protocol clipboard channels.
    #[must_use]
    pub fn is_bidirectional(&self) -> bool {
        self.host_to_guest.is_supported() && self.guest_to_host.is_supported()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rdp_unsupported_names_cliprdr_in_both_directions() {
        let status = VdiClipboardStatus::rdp_unsupported();
        assert!(!status.is_bidirectional());
        for lane in [&status.host_to_guest, &status.guest_to_host] {
            match lane {
                VdiClipboardLaneStatus::Unsupported { reason } => {
                    assert!(reason.contains("CLIPRDR"));
                    assert!(reason.contains("mde-vdi-rdp"));
                }
                other => panic!("expected unsupported RDP lane, got {other:?}"),
            }
        }
    }

    #[test]
    fn spice_unsupported_names_vdagent_in_both_directions() {
        let status = VdiClipboardStatus::spice_unsupported();
        assert!(!status.is_bidirectional());
        for lane in [&status.host_to_guest, &status.guest_to_host] {
            match lane {
                VdiClipboardLaneStatus::Unsupported { reason } => {
                    assert!(reason.contains("vdagent"));
                    assert!(reason.contains("mde-vdi-spice"));
                }
                other => panic!("expected unsupported SPICE lane, got {other:?}"),
            }
        }
    }

    #[test]
    fn wire_shape_is_stable_and_explicit() {
        let body = serde_json::to_string(&VdiClipboardStatus::rdp_unsupported())
            .expect("serialize status");
        assert!(body.contains(r#""host_to_guest":{"state":"unsupported""#));
        assert!(body.contains(r#""guest_to_host":{"state":"unsupported""#));
        assert!(body.contains("CLIPRDR"));

        let back: VdiClipboardStatus = serde_json::from_str(&body).expect("round-trip");
        assert_eq!(back, VdiClipboardStatus::rdp_unsupported());
    }
}
