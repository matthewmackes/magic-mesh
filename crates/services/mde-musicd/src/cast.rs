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

#[cfg(test)]
mod tests {
    use super::{CastTarget, CASTV2_PORT};
    use std::io;

    #[test]
    fn target_requires_numeric_address_and_bounded_name() {
        assert!(CastTarget::new("172.20.146.150", "Family Room TV").is_ok());
        assert_eq!(CASTV2_PORT, 8009);
        assert_eq!(CastTarget::new("cast.local", "TV").unwrap_err().kind(), io::ErrorKind::InvalidInput);
        assert_eq!(CastTarget::new("172.20.146.150", "\n").unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }
}
