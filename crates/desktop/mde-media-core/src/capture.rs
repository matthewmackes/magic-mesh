//! MEDIA-13: v4l2 capture devices (webcams, TV tuners, capture cards) as sources.
//!
//! Design lock (`docs/design/mesh-media-player.md`): the player watches **capture
//! inputs** — a Linux v4l2 `/dev/videoN` node (a USB webcam, a PCIe/USB TV tuner, an
//! HDMI capture card) opened as a live source. mpv/ffmpeg opens such a node natively
//! through its `av://v4l2:` demuxer, so the device URL is handed straight to the
//! existing [`Player::load`] — the same play path the local + stream sources use, no
//! re-derived player (§6 glue).
//!
//! The seam mirrors the MEDIA-12 [`YtDlpResolver`](crate::ytdlp::YtDlpResolver) /
//! `YtDlpCli` idiom (and the `stream.rs` classifier): one narrow, injectable trait
//! ([`CaptureEnumerator`]) with a real subprocess implementation ([`V4l2Cli`]) and a
//! **pure, fixture-tested parser** ([`parse_v4l2_listing`]) between them, so every
//! byte of device projection is exercised with *no real `/dev/video` and no v4l2
//! tooling* — the airgap-safe path.
//!
//! - [`CaptureEnumerator`] is the interface the surface drives: [`is_available`]
//!   probes the tooling, [`enumerate`] lists the local capture devices.
//! - [`V4l2Cli`] is the real implementation. It shells out to `v4l2-ctl
//!   --list-devices` — no compile-time dependency and no linking, so it is **always
//!   compiled** (unlike the feature-gated `mpv` engine). It is **honest-gated at
//!   runtime**: absent v4l2 tooling surfaces as [`CaptureError::ToolMissing`] and no
//!   devices surfaces as an empty list — never a stub / fake device (§7).
//! - [`parse_v4l2_listing`] projects `v4l2-ctl --list-devices` output into typed
//!   [`CaptureDevice`]s. Pure — the live client feeds it the captured stdout; the
//!   tests feed it recorded listings.
//! - [`v4l2_play_url`] builds the `av://v4l2:/dev/videoN` URL handed to the player.
//!
//! [`Player::load`]: crate::Player::load
//! [`is_available`]: CaptureEnumerator::is_available
//! [`enumerate`]: CaptureEnumerator::enumerate

use std::process::Command;

/// The `v4l2-ctl` executable name (from `v4l-utils`), resolved on `PATH`.
pub const V4L2_CTL_BIN: &str = "v4l2-ctl";

/// The mpv/ffmpeg URL scheme prefix for a v4l2 capture node — a device path
/// `/dev/videoN` becomes `av://v4l2:/dev/videoN`, which mpv opens natively.
pub const V4L2_URL_PREFIX: &str = "av://v4l2:";

/// Build the mpv play URL for a v4l2 device-node path
/// (`/dev/video0` → `av://v4l2:/dev/video0`).
///
/// The one place the capture URL is constructed — the [`Player`](crate::Player)
/// then loads it verbatim (mpv's `av://v4l2:` demuxer opens the live node). Pure, so
/// the URL construction is unit-tested with no real device.
#[must_use]
pub fn v4l2_play_url(dev_path: &str) -> String {
    format!("{V4L2_URL_PREFIX}{dev_path}")
}

/// The kind of a v4l2 device node, classified from its `/dev/...` path.
///
/// Only a [`Video`](Self::Video) node is a playable *capture* node; the others
/// (`media` control, `v4l-subdev`, `vbi` teletext, `radio`) are exposed so a
/// device's real capabilities are visible, but they are not handed to the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureNodeKind {
    /// A `/dev/videoN` streaming node — the playable capture node.
    Video,
    /// A `/dev/vbiN` vertical-blanking-interval node (teletext / closed captions).
    Vbi,
    /// A `/dev/radioN` radio-tuner node (audio-only, no video capture).
    Radio,
    /// A `/dev/mediaN` media-controller node (topology, not a stream).
    Media,
    /// A `/dev/v4l-subdevN` sub-device node (sensor / bridge, not a stream).
    SubDevice,
    /// Any other `/dev/...` node kind not classified above.
    Other,
}

impl CaptureNodeKind {
    /// Classify a device-node path by its basename prefix (the trailing index
    /// digits are stripped), e.g. `/dev/video0` → [`Video`](Self::Video).
    #[must_use]
    pub fn from_path(path: &str) -> Self {
        let base = path.rsplit('/').next().unwrap_or(path);
        let prefix = base.trim_end_matches(|c: char| c.is_ascii_digit());
        match prefix {
            "video" => Self::Video,
            "vbi" => Self::Vbi,
            "radio" => Self::Radio,
            "media" => Self::Media,
            "v4l-subdev" => Self::SubDevice,
            _ => Self::Other,
        }
    }

    /// A short human label for the node kind (for the Sources detail line / tests).
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Video => "video",
            Self::Vbi => "vbi",
            Self::Radio => "radio",
            Self::Media => "media",
            Self::SubDevice => "subdev",
            Self::Other => "other",
        }
    }

    /// Whether this node is a playable capture node — a `/dev/videoN` stream.
    #[must_use]
    pub const fn is_capture(self) -> bool {
        matches!(self, Self::Video)
    }
}

/// One device node of a v4l2 capture device — a `/dev/...` path and its classified
/// [`CaptureNodeKind`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureNode {
    /// The device-node path, e.g. `/dev/video0`.
    pub path: String,
    /// The classified kind of the node.
    pub kind: CaptureNodeKind,
}

impl CaptureNode {
    /// Build a node from a `/dev/...` path, classifying its [`CaptureNodeKind`].
    #[must_use]
    pub fn new(path: impl Into<String>) -> Self {
        let path = path.into();
        let kind = CaptureNodeKind::from_path(&path);
        Self { path, kind }
    }

    /// Whether this node is a playable capture node (a `/dev/videoN`).
    #[must_use]
    pub const fn is_capture(&self) -> bool {
        self.kind.is_capture()
    }

    /// The mpv play URL for this node (`av://v4l2:/dev/videoN`).
    #[must_use]
    pub fn play_url(&self) -> String {
        v4l2_play_url(&self.path)
    }
}

/// A v4l2 capture device — a hardware input (webcam / TV tuner / capture card) with
/// a display name, an optional bus-info token, and its enumerated [`CaptureNode`]s.
///
/// A single hardware device commonly exposes several nodes (a streaming
/// `/dev/videoN`, a `/dev/mediaN` controller, a `/dev/vbiN` teletext node); the
/// **playable** node is the first `/dev/videoN` ([`capture_node`](Self::capture_node)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureDevice {
    /// The human display name (`v4l2-ctl`'s device header, e.g. `"UVC Camera"`).
    pub name: String,
    /// The bus-info token from the header (`"usb-0000:00:14.0-1"`), if present.
    pub bus_info: Option<String>,
    /// The device's enumerated nodes, in listing order.
    pub nodes: Vec<CaptureNode>,
}

impl CaptureDevice {
    /// The primary capture node — the first `/dev/videoN` streaming node — or
    /// [`None`] when the device exposes no playable capture node.
    #[must_use]
    pub fn capture_node(&self) -> Option<&CaptureNode> {
        self.nodes.iter().find(|node| node.is_capture())
    }

    /// Whether this device exposes a playable capture node.
    #[must_use]
    pub fn is_playable(&self) -> bool {
        self.capture_node().is_some()
    }

    /// The primary capture node's `/dev/...` path, if any.
    #[must_use]
    pub fn path(&self) -> Option<&str> {
        self.capture_node().map(|node| node.path.as_str())
    }

    /// The mpv play URL for the primary capture node (`av://v4l2:/dev/videoN`), or
    /// [`None`] when there is no playable node — what the surface hands to
    /// [`Player::load`](crate::Player::load).
    #[must_use]
    pub fn play_url(&self) -> Option<String> {
        self.capture_node().map(CaptureNode::play_url)
    }

    /// The distinct node kinds this device exposes, in first-seen order — the
    /// device's real capability set (e.g. `[Video, Vbi]` for a tuner with teletext).
    #[must_use]
    pub fn capabilities(&self) -> Vec<CaptureNodeKind> {
        let mut kinds: Vec<CaptureNodeKind> = Vec::new();
        for node in &self.nodes {
            if !kinds.contains(&node.kind) {
                kinds.push(node.kind);
            }
        }
        kinds
    }
}

/// A failure from the v4l2 capture-enumeration seam (MEDIA-13).
///
/// Every variant is honest + recoverable: the caller surfaces it and carries on
/// (nothing is opened). An absent tool / device is a state, not a crash (§7).
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum CaptureError {
    /// The v4l2 tooling (`v4l2-ctl`) is not installed / not on `PATH` — the honest
    /// tool-absent gate. Distinct from "installed but found no devices" (an empty
    /// list), so the surface can hint how to enable it.
    #[error("v4l2 tooling (v4l2-ctl) is not installed")]
    ToolMissing,
    /// The enumeration ran but failed (a spawn error other than a missing binary);
    /// carries the underlying message.
    #[error("v4l2 device enumeration failed: {0}")]
    Failed(String),
}

/// The injectable v4l2 capture-enumeration seam (MEDIA-13).
///
/// Production impl is [`V4l2Cli`] (the real subprocess); tests inject a fake that
/// returns a recorded listing + a scripted availability, so the enumeration →
/// open → play glue is exercised with no real `/dev/video` and no v4l2 tooling.
pub trait CaptureEnumerator {
    /// Whether the v4l2 tooling is available to run — the surface honest-gates the
    /// "no capture devices" hint on this.
    fn is_available(&self) -> bool;

    /// Enumerate the local v4l2 capture devices.
    ///
    /// An empty list is the honest "no capture devices present" state.
    ///
    /// # Errors
    /// [`CaptureError::ToolMissing`] when the tooling is absent; [`CaptureError::Failed`]
    /// on any other enumeration failure.
    fn enumerate(&self) -> Result<Vec<CaptureDevice>, CaptureError>;
}

/// Parse `v4l2-ctl --list-devices` output into typed [`CaptureDevice`]s (MEDIA-13).
///
/// Pure + tolerant (§6 glue — no re-implementation of v4l2). The listing groups
/// each hardware device under a non-indented header line ending in `:` (the device
/// name, with a trailing `(bus-info)` token), followed by its indented `/dev/...`
/// node paths:
///
/// ```text
/// UVC Camera (046d:0825) (usb-0000:00:14.0-1):
///     /dev/video0
///     /dev/video1
///     /dev/media0
/// ```
///
/// Devices that carry no nodes (a malformed header) are dropped; every other device
/// is returned with its nodes classified by [`CaptureNodeKind::from_path`]. An empty
/// / whitespace listing yields an empty list — the honest no-devices state.
#[must_use]
pub fn parse_v4l2_listing(listing: &str) -> Vec<CaptureDevice> {
    let mut devices: Vec<CaptureDevice> = Vec::new();
    for line in listing.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if line.starts_with([' ', '\t']) {
            // An indented node line under the current device header.
            let path = line.trim();
            if path.starts_with("/dev/") {
                if let Some(device) = devices.last_mut() {
                    device.nodes.push(CaptureNode::new(path));
                }
            }
        } else {
            // A device header: "Name (bus-info):".
            let header = line.trim().trim_end_matches(':').trim();
            let (name, bus_info) = split_header(header);
            devices.push(CaptureDevice {
                name,
                bus_info,
                nodes: Vec::new(),
            });
        }
    }
    devices.retain(|device| !device.nodes.is_empty());
    devices
}

/// Split a `v4l2-ctl` device header into its display name and the trailing bus-info
/// token: the *last* parenthesized group is the bus info, any earlier ones stay in
/// the name (some cameras carry a `(vendor:product)` id before the bus group).
fn split_header(header: &str) -> (String, Option<String>) {
    if header.ends_with(')') {
        if let Some(open) = header.rfind('(') {
            let bus = header[open + 1..header.len() - 1].trim();
            let name = header[..open].trim();
            if !name.is_empty() && !bus.is_empty() {
                return (name.to_owned(), Some(bus.to_owned()));
            }
        }
    }
    (header.to_owned(), None)
}

/// The real v4l2 capture enumerator — shells out to `v4l2-ctl --list-devices`
/// (MEDIA-13).
///
/// No compile-time dependency (a subprocess, not a linked library), so it is always
/// compiled — airgap-safe to *build*. It is **honest-gated at runtime**: on a host
/// with no `v4l2-ctl` on `PATH`, [`is_available`](CaptureEnumerator::is_available)
/// is `false` and [`enumerate`](CaptureEnumerator::enumerate) returns
/// [`CaptureError::ToolMissing`]; a host with no capture hardware yields an empty
/// list. The live enumeration is exercised only where the tool + a device exist; the
/// [`parse_v4l2_listing`] projection it composes is fully tested with recorded
/// listings.
#[derive(Debug, Clone, Copy, Default)]
pub struct V4l2Cli;

impl V4l2Cli {
    /// Map a failed `Command` spawn to a typed error: a missing binary is the honest
    /// [`CaptureError::ToolMissing`]; anything else is [`CaptureError::Failed`].
    fn spawn_error(e: &std::io::Error) -> CaptureError {
        if e.kind() == std::io::ErrorKind::NotFound {
            CaptureError::ToolMissing
        } else {
            CaptureError::Failed(e.to_string())
        }
    }
}

impl CaptureEnumerator for V4l2Cli {
    fn is_available(&self) -> bool {
        Command::new(V4L2_CTL_BIN)
            .arg("--version")
            .output()
            .is_ok_and(|out| out.status.success())
    }

    fn enumerate(&self) -> Result<Vec<CaptureDevice>, CaptureError> {
        // `--list-devices` prints the grouped device → node listing to stdout. When
        // no devices exist it prints nothing (and may exit non-zero), so an empty
        // stdout is treated as the honest no-devices state, not an error — only a
        // failed *spawn* (a missing binary) is an error.
        let output = Command::new(V4L2_CTL_BIN)
            .arg("--list-devices")
            .output()
            .map_err(|e| Self::spawn_error(&e))?;
        let listing = String::from_utf8_lossy(&output.stdout);
        Ok(parse_v4l2_listing(&listing))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trimmed but shape-faithful `v4l2-ctl --list-devices` document: a USB webcam
    /// (with a vendor id + bus group), a `PCIe` TV tuner (with a VBI teletext node),
    /// and a platform codec device.
    const LISTING: &str = "UVC Camera (046d:0825) (usb-0000:00:14.0-1):\n\
        \t/dev/video0\n\
        \t/dev/video1\n\
        \t/dev/media0\n\
        \n\
        Hauppauge WinTV-HVR (PCI:0000:03:00.0):\n\
        \t/dev/video2\n\
        \t/dev/vbi0\n\
        \n\
        bcm2835-codec-decode (platform:bcm2835-codec):\n\
        \t/dev/video10\n\
        \t/dev/media1\n";

    /// A recorded enumerator: a scripted availability + a captured listing, so the
    /// seam is driven with no real tool and no `/dev/video` — the fixture path the
    /// acceptance asks for.
    struct RecordedEnumerator {
        available: bool,
        listing: String,
    }

    impl CaptureEnumerator for RecordedEnumerator {
        fn is_available(&self) -> bool {
            self.available
        }

        fn enumerate(&self) -> Result<Vec<CaptureDevice>, CaptureError> {
            if !self.available {
                return Err(CaptureError::ToolMissing);
            }
            Ok(parse_v4l2_listing(&self.listing))
        }
    }

    #[test]
    fn parses_a_recorded_v4l2_listing() {
        let devices = parse_v4l2_listing(LISTING);
        assert_eq!(devices.len(), 3);

        // The webcam: name split from the trailing bus group, the earlier
        // (046d:0825) vendor id kept in the name; three nodes classified.
        let cam = &devices[0];
        assert_eq!(cam.name, "UVC Camera (046d:0825)");
        assert_eq!(cam.bus_info.as_deref(), Some("usb-0000:00:14.0-1"));
        assert_eq!(cam.nodes.len(), 3);
        assert_eq!(cam.nodes[0].kind, CaptureNodeKind::Video);
        assert_eq!(cam.nodes[2].kind, CaptureNodeKind::Media);
        // Its primary capture node + play URL.
        assert_eq!(cam.path(), Some("/dev/video0"));
        assert_eq!(cam.play_url().as_deref(), Some("av://v4l2:/dev/video0"));
        assert!(cam.is_playable());

        // The tuner: a video capture node + a VBI teletext node → both capabilities.
        let tuner = &devices[1];
        assert_eq!(tuner.name, "Hauppauge WinTV-HVR");
        assert_eq!(tuner.bus_info.as_deref(), Some("PCI:0000:03:00.0"));
        assert_eq!(tuner.path(), Some("/dev/video2"));
        assert_eq!(
            tuner.capabilities(),
            vec![CaptureNodeKind::Video, CaptureNodeKind::Vbi]
        );
    }

    #[test]
    fn empty_or_whitespace_listing_is_the_honest_no_devices_state() {
        assert!(parse_v4l2_listing("").is_empty());
        assert!(parse_v4l2_listing("   \n\t\n  ").is_empty());
    }

    #[test]
    fn node_before_a_header_and_headerless_nodes_are_ignored() {
        // A stray indented node with no preceding header is dropped, not attributed.
        let devices = parse_v4l2_listing("\t/dev/video9\nReal Cam (usb-1):\n\t/dev/video0\n");
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].path(), Some("/dev/video0"));
    }

    #[test]
    fn play_url_is_the_av_v4l2_scheme() {
        assert_eq!(v4l2_play_url("/dev/video0"), "av://v4l2:/dev/video0");
        assert_eq!(v4l2_play_url("/dev/video7"), "av://v4l2:/dev/video7");
        assert_eq!(
            CaptureNode::new("/dev/video3").play_url(),
            "av://v4l2:/dev/video3"
        );
    }

    #[test]
    fn node_kind_is_classified_from_the_dev_path() {
        assert_eq!(
            CaptureNodeKind::from_path("/dev/video0"),
            CaptureNodeKind::Video
        );
        assert_eq!(
            CaptureNodeKind::from_path("/dev/vbi0"),
            CaptureNodeKind::Vbi
        );
        assert_eq!(
            CaptureNodeKind::from_path("/dev/radio1"),
            CaptureNodeKind::Radio
        );
        assert_eq!(
            CaptureNodeKind::from_path("/dev/media0"),
            CaptureNodeKind::Media
        );
        assert_eq!(
            CaptureNodeKind::from_path("/dev/v4l-subdev2"),
            CaptureNodeKind::SubDevice
        );
        assert_eq!(
            CaptureNodeKind::from_path("/dev/swradio0"),
            CaptureNodeKind::Other
        );
        assert!(CaptureNodeKind::Video.is_capture());
        assert!(!CaptureNodeKind::Media.is_capture());
    }

    #[test]
    fn a_device_with_no_video_node_is_not_playable() {
        // A device exposing only a media-controller node has no playable capture.
        let devices = parse_v4l2_listing("Bridge (platform:x):\n\t/dev/media0\n");
        assert_eq!(devices.len(), 1);
        let device = &devices[0];
        assert!(!device.is_playable());
        assert_eq!(device.capture_node(), None);
        assert_eq!(device.path(), None);
        assert_eq!(device.play_url(), None);
    }

    #[test]
    fn header_without_a_bus_group_keeps_the_whole_name() {
        let devices = parse_v4l2_listing("My Webcam:\n\t/dev/video0\n");
        assert_eq!(devices[0].name, "My Webcam");
        assert_eq!(devices[0].bus_info, None);
    }

    #[test]
    fn recorded_enumerator_available_lists_the_captured_devices() {
        let enumerator = RecordedEnumerator {
            available: true,
            listing: LISTING.to_owned(),
        };
        assert!(enumerator.is_available());
        let devices = enumerator.enumerate().expect("enumerate");
        assert_eq!(devices.len(), 3);
        assert_eq!(devices[0].path(), Some("/dev/video0"));
    }

    #[test]
    fn recorded_enumerator_tool_absent_is_honest_tool_missing() {
        let enumerator = RecordedEnumerator {
            available: false,
            listing: String::new(),
        };
        assert!(!enumerator.is_available());
        assert_eq!(enumerator.enumerate(), Err(CaptureError::ToolMissing));
    }

    #[test]
    fn recorded_enumerator_present_but_no_hardware_is_an_empty_list() {
        // The tool is installed but the host has no capture hardware → empty, honest.
        let enumerator = RecordedEnumerator {
            available: true,
            listing: String::new(),
        };
        assert!(enumerator.is_available());
        assert!(enumerator.enumerate().expect("enumerate").is_empty());
    }
}
