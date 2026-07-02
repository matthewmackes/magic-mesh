//! The DRM connector prober — a **read-only** enumeration of every connector
//! and its mode list on the host's DRM devices (lock 6, Displays).
//!
//! This is deliberately probe-only: the shell's `mde-egui` DRM runner is the one
//! DRM **master** (it drives the modeset); a second master would fight it.
//! Enable/mode/arrange lands in the runner as E12-18's multi-CRTC work — this
//! prober feeds the System surface's honest inventory until (and after) then.
//!
//! Pure-Rust ioctls via the same `drm` crate the runner pins; no unsafe, no GBM/
//! EGL. A host with no `/dev/dri` (headless CI) answers with a typed
//! [`SeatError::Unavailable`], never a fake connector (§7).

use std::fs::{File, OpenOptions};
use std::os::fd::{AsFd, BorrowedFd};
use std::path::PathBuf;

use drm::control::{connector, Device as ControlDevice, ModeTypeFlags};
use drm::Device as BasicDevice;

use crate::error::{Backend, SeatError};

/// One display mode a connector advertises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayMode {
    /// Horizontal resolution (px).
    pub width: u16,
    /// Vertical resolution (px).
    pub height: u16,
    /// Vertical refresh (Hz, as the kernel reports it).
    pub refresh_hz: u32,
    /// The connector's preferred (native) mode.
    pub preferred: bool,
}

impl DisplayMode {
    /// The operator-facing mode line, e.g. `1920x1080 @ 60 Hz`.
    #[must_use]
    pub fn label(&self) -> String {
        format!("{}x{} @ {} Hz", self.width, self.height, self.refresh_hz)
    }
}

/// A connector's physical link state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorStatus {
    /// A display is attached.
    Connected,
    /// Nothing attached.
    Disconnected,
    /// The kernel could not tell.
    Unknown,
}

/// One DRM connector (a physical output: `HDMI-A-1`, `eDP-1`, …).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connector {
    /// The kernel-style connector name (`<interface>-<id>`, prefixed with its
    /// card when the host has more than one DRM device).
    pub name: String,
    /// Link state.
    pub status: ConnectorStatus,
    /// The attached display's physical size in mm, when the kernel knows it.
    pub size_mm: Option<(u32, u32)>,
    /// Advertised modes (kernel order; the preferred mode is flagged).
    pub modes: Vec<DisplayMode>,
}

impl Connector {
    /// The preferred mode, when the connector advertises one.
    #[must_use]
    pub fn preferred_mode(&self) -> Option<&DisplayMode> {
        self.modes.iter().find(|m| m.preferred)
    }
}

/// The display-probe seam. Production impl: [`DrmProber`]; tests inject fakes.
pub trait DisplayProber: Send {
    /// Enumerate every connector on every DRM device.
    ///
    /// # Errors
    /// Typed: [`SeatError::Unavailable`] when the host has no DRM device.
    fn connectors(&self) -> Result<Vec<Connector>, SeatError>;
}

/// A DRM node wrapped so it implements the `drm` device traits — the same
/// wrapper the `mde-egui` runner uses, read-only here (no modeset, no master).
struct Card(File);

impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}
impl BasicDevice for Card {}
impl ControlDevice for Card {}

/// The production prober over `/dev/dri/card*`. The root is injectable
/// ([`DrmProber::with_root`]) so the no-device path is testable headless.
pub struct DrmProber {
    dri_root: PathBuf,
}

impl DrmProber {
    /// A prober over the real `/dev/dri`.
    #[must_use]
    pub fn new() -> Self {
        Self::with_root("/dev/dri")
    }

    /// A prober over an alternate device root (the test seam).
    #[must_use]
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self {
            dri_root: root.into(),
        }
    }
}

impl Default for DrmProber {
    fn default() -> Self {
        Self::new()
    }
}

impl DisplayProber for DrmProber {
    fn connectors(&self) -> Result<Vec<Connector>, SeatError> {
        let mut out = Vec::new();
        let mut cards_seen = 0_u32;
        let mut last_err = format!("no {}/card* nodes", self.dri_root.display());

        for idx in 0..8 {
            let path = self.dri_root.join(format!("card{idx}"));
            if !path.exists() {
                continue;
            }
            cards_seen += 1;
            // Read-only probing needs no DRM master; write access is not
            // required for the GET ioctls, so a plain read open keeps this
            // safely un-privileged beside the running seat.
            let card = match OpenOptions::new().read(true).open(&path) {
                Ok(f) => Card(f),
                Err(e) => {
                    last_err = format!("{}: {e}", path.display());
                    continue;
                }
            };
            let res = match card.resource_handles() {
                Ok(r) => r,
                Err(e) => {
                    last_err = format!("{}: resource_handles: {e}", path.display());
                    continue;
                }
            };
            for &handle in res.connectors() {
                let Ok(info) = card.get_connector(handle, false) else {
                    continue;
                };
                // Prefix the card only on multi-GPU hosts so single-GPU names
                // stay the familiar kernel spelling (`HDMI-A-1`).
                let prefix = if idx == 0 {
                    String::new()
                } else {
                    format!("card{idx}/")
                };
                out.push(fold_connector(&prefix, &info));
            }
        }

        if out.is_empty() && cards_seen == 0 {
            return Err(SeatError::Unavailable {
                backend: Backend::Display,
                reason: format!("no DRM device: {last_err}"),
            });
        }
        Ok(out)
    }
}

/// Fold one kernel connector into the typed model.
fn fold_connector(name_prefix: &str, info: &connector::Info) -> Connector {
    let status = match info.state() {
        connector::State::Connected => ConnectorStatus::Connected,
        connector::State::Disconnected => ConnectorStatus::Disconnected,
        connector::State::Unknown => ConnectorStatus::Unknown,
    };
    let modes = info
        .modes()
        .iter()
        .map(|m| {
            let (width, height) = m.size();
            DisplayMode {
                width,
                height,
                refresh_hz: m.vrefresh(),
                preferred: m.mode_type().contains(ModeTypeFlags::PREFERRED),
            }
        })
        .collect();
    Connector {
        name: format!(
            "{name_prefix}{}-{}",
            info.interface().as_str(),
            info.interface_id()
        ),
        status,
        size_mm: info.size(),
        modes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_labels_read_like_a_mode_line() {
        let m = DisplayMode {
            width: 2560,
            height: 1440,
            refresh_hz: 144,
            preferred: true,
        };
        assert_eq!(m.label(), "2560x1440 @ 144 Hz");
    }

    #[test]
    fn preferred_mode_is_found_by_flag_not_position() {
        let c = Connector {
            name: "HDMI-A-1".to_owned(),
            status: ConnectorStatus::Connected,
            size_mm: Some((600, 340)),
            modes: vec![
                DisplayMode {
                    width: 1024,
                    height: 768,
                    refresh_hz: 60,
                    preferred: false,
                },
                DisplayMode {
                    width: 1920,
                    height: 1080,
                    refresh_hz: 60,
                    preferred: true,
                },
            ],
        };
        assert_eq!(
            c.preferred_mode().map(DisplayMode::label).as_deref(),
            Some("1920x1080 @ 60 Hz")
        );
    }

    #[test]
    fn no_modes_means_no_preferred_mode_not_an_invented_one() {
        let c = Connector {
            name: "DP-2".to_owned(),
            status: ConnectorStatus::Disconnected,
            size_mm: None,
            modes: Vec::new(),
        };
        assert_eq!(c.preferred_mode(), None);
    }

    #[test]
    fn a_root_without_cards_is_typed_unavailable() {
        // The headless-CI path: a directory with no card* nodes must answer
        // with the typed not-available state, never fake connectors (§7).
        let dir = std::env::temp_dir().join(format!("mde-seat-dri-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dri root");
        let e = DrmProber::with_root(&dir)
            .connectors()
            .expect_err("no cards must not enumerate");
        assert_eq!(e.backend(), Backend::Display);
        assert!(matches!(e, SeatError::Unavailable { .. }), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_real_prober_on_this_host_answers_typed_never_panics() {
        // Whatever GPU (or none) the build host has, the probe is typed.
        match DrmProber::new().connectors() {
            Ok(connectors) => {
                for c in connectors {
                    assert!(!c.name.is_empty());
                }
            }
            Err(e) => assert_eq!(e.backend(), Backend::Display),
        }
    }
}
