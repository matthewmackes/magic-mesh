//! Bounded Google Cast (CASTV2) adapter for Music renderer targets.
//!
//! Cast devices expose a TLS/protobuf control channel on TCP 8009.  This
//! module keeps that protocol behind a small daemon-owned seam; callers never
//! receive a socket or a third-party protocol object.  The adapter is
//! intentionally synchronous because `rust_cast::CastDevice` is not `Send`;
//! the daemon must invoke it only from its blocking provider lane.

use std::io;
use std::net::IpAddr;
use std::str::FromStr;

/// The standard CASTV2 control port.
pub const CASTV2_PORT: u16 = 8009;

/// The bounded commands that the Music daemon may send to a Cast receiver.
/// Media execution remains behind the blocking provider lane; the UI and bus
/// never receive protocol-specific command objects.
#[derive(Debug, Clone, PartialEq)]
pub enum CastCommand {
    Load { url: String, content_type: String, start_seconds: f64 },
    Play,
    Pause,
    Seek { position_seconds: f64 },
}

impl CastCommand {
    /// Admit only remote HTTP(S) media and finite, non-negative positions.
    pub fn load(url: &str, content_type: &str, start_seconds: f64) -> io::Result<Self> {
        let scheme = url.split_once("://").map(|(scheme, _)| scheme);
        if !matches!(scheme, Some("http" | "https")) || url.len() > 2048 || url.chars().any(char::is_control) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Cast media URL must be a bounded HTTP(S) URL"));
        }
        if content_type.is_empty() || content_type.len() > 128 || content_type.chars().any(char::is_control) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid Cast media content type"));
        }
        if !start_seconds.is_finite() || start_seconds < 0.0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid Cast start position"));
        }
        Ok(Self::Load { url: url.to_owned(), content_type: content_type.to_owned(), start_seconds })
    }

    pub fn seek(position_seconds: f64) -> io::Result<Self> {
        if !position_seconds.is_finite() || position_seconds < 0.0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid Cast seek position"));
        }
        Ok(Self::Seek { position_seconds })
    }
}

/// A validated, operator-admitted Cast endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CastTarget {
    /// Numeric address; hostnames are deliberately not accepted at this
    /// boundary so a target cannot silently resolve to another network.
    pub address: IpAddr,
    /// Friendly name supplied by the discovery projection.
    pub name: String,
}

impl CastTarget {
    /// Validate a target address and bounded display name.
    pub fn new(address: &str, name: impl Into<String>) -> io::Result<Self> {
        let address = IpAddr::from_str(address.trim())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid Cast address"))?;
        let name = name.into();
        if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid Cast target name",
            ));
        }
        Ok(Self { address, name })
    }
}

/// Establish a CASTV2 TLS/protobuf session and return the receiver identity.
///
/// Host certificate verification is intentionally disabled only for the Cast
/// device protocol: Cast endpoints use a device certificate that is not part
/// of the workstation trust store. The address is still validated as a
/// numeric operator-admitted endpoint before any connection is attempted.
pub fn verify_castv2(target: &CastTarget) -> io::Result<()> {
    let device = rust_cast::CastDevice::connect_without_host_verification(
        target.address.to_string(),
        CASTV2_PORT,
    )
    .map_err(|error| io::Error::other(format!("Cast connection failed: {error}")))?;
    drop(device);
    Ok(())
}

/// Execute one admitted command against the default media receiver.
///
/// This is deliberately a blocking operation: CASTV2 request/response traffic
/// is kept out of async/UI code, and a failed receiver operation is returned to
/// the caller instead of being reported as successful playback.
pub fn execute_cast_command(target: &CastTarget, command: &CastCommand) -> io::Result<()> {
    use rust_cast::channels::media::{Media, LoadOptions, StreamType};
    use rust_cast::channels::receiver::CastDeviceApp;

    let device = rust_cast::CastDevice::connect_without_host_verification(
        target.address.to_string(),
        CASTV2_PORT,
    )
    .map_err(|error| io::Error::other(format!("Cast connection failed: {error}")))?;
    let app = device
        .receiver
        .launch_app(&CastDeviceApp::DefaultMediaReceiver)
        .map_err(|error| io::Error::other(format!("Cast receiver launch failed: {error}")))?;
    device
        .connection
        .connect(app.transport_id.as_str())
        .map_err(|error| io::Error::other(format!("Cast media channel failed: {error}")))?;

    let result = match command {
        CastCommand::Load { url, content_type, start_seconds } => device
            .media
            .load_with_opts(
                app.transport_id.as_str(),
                app.session_id.as_str(),
                &Media {
                    content_id: url.clone(),
                    content_type: content_type.clone(),
                    stream_type: StreamType::Buffered,
                    duration: None,
                    metadata: None,
                },
                LoadOptions { current_time: *start_seconds, autoplay: true },
            )
            .map(|_| ()),
        CastCommand::Play | CastCommand::Pause | CastCommand::Seek { .. } => {
            let status = device
                .media
                .get_status(app.transport_id.as_str(), None)
                .map_err(|error| io::Error::other(format!("Cast media status failed: {error}")))?;
            let entry = status.entries.first().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "Cast receiver has no active media")
            })?;
            match command {
                CastCommand::Play => device.media.play(app.transport_id.as_str(), entry.media_session_id).map(|_| ()),
                CastCommand::Pause => device.media.pause(app.transport_id.as_str(), entry.media_session_id).map(|_| ()),
                CastCommand::Seek { position_seconds } => device
                    .media
                    .seek(
                        app.transport_id.as_str(),
                        entry.media_session_id,
                        Some(*position_seconds as f32),
                        None,
                    )
                    .map(|_| ()),
                CastCommand::Load { .. } => unreachable!(),
            }
        }
    };
    result.map_err(|error| io::Error::other(format!("Cast media command failed: {error}")))
}

#[cfg(test)]
mod tests {
    use super::{execute_cast_command, verify_castv2, CastCommand, CastTarget, CASTV2_PORT};
    use std::io;

    #[test]
    fn target_requires_numeric_address_and_bounded_name() {
        assert!(CastTarget::new("172.20.146.150", "Family Room TV").is_ok());
        assert_eq!(CASTV2_PORT, 8009);
        assert_eq!(CastTarget::new("cast.local", "TV").unwrap_err().kind(), io::ErrorKind::InvalidInput);
        assert_eq!(CastTarget::new("172.20.146.150", "\n").unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn commands_reject_local_or_unbounded_media_inputs() {
        assert!(CastCommand::load("http://music.example/song.mp3", "audio/mpeg", 0.0).is_ok());
        assert!(CastCommand::load("file:///secret.mp3", "audio/mpeg", 0.0).is_err());
        assert!(CastCommand::load("http://127.0.0.1/song.mp3", "audio/mpeg", 0.0).is_ok());
        assert!(CastCommand::seek(f64::NAN).is_err());
        assert!(CastCommand::seek(-1.0).is_err());
    }

    #[test]
    fn live_castv2_connection_when_operator_supplies_target() {
        let Ok(address) = std::env::var("MDE_CAST_LIVE_TARGET") else {
            return;
        };
        let target = CastTarget::new(&address, "operator-supplied Cast target").unwrap();
        verify_castv2(&target).expect("operator-supplied Cast target must accept CASTV2 connection");
    }

    #[test]
    fn live_cast_media_load_and_pause_when_operator_supplies_url() {
        let (Ok(address), Ok(url)) = (
            std::env::var("MDE_CAST_LIVE_TARGET"),
            std::env::var("MDE_CAST_LIVE_MEDIA_URL"),
        ) else {
            return;
        };
        let target = CastTarget::new(&address, "operator-supplied Cast target").unwrap();
        let load = CastCommand::load(&url, "video/mp4", 0.0).unwrap();
        execute_cast_command(&target, &load).expect("Cast receiver must accept media load");
        execute_cast_command(&target, &CastCommand::Pause).expect("Cast receiver must accept pause");
    }
}
