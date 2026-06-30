//! Connection configuration for an RDP session.

/// The RDP standard port.
pub const DEFAULT_PORT: u16 = 3389;
/// Default desktop width when the caller does not pick one.
pub const DEFAULT_WIDTH: u16 = 1280;
/// Default desktop height when the caller does not pick one.
pub const DEFAULT_HEIGHT: u16 = 720;

/// Everything needed to open an RDP session to a single host.
///
/// Built directly by the shell from a discovered desktop (mesh registry /
/// DATACENTER inventory, design lock 22). The credential fields are plaintext in
/// memory only for the duration of the connect handshake; the shell sources them
/// from the sealed cred vault (lock 23) and should zeroise its copy after.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RdpConfig {
    /// Host to connect to (mesh overlay address or hostname).
    pub host: String,
    /// TCP port — [`DEFAULT_PORT`] unless the host runs RDP elsewhere.
    pub port: u16,
    /// Guest-OS username.
    pub username: String,
    /// Guest-OS password.
    pub password: String,
    /// Optional Windows domain / workgroup.
    pub domain: Option<String>,
    /// Negotiated desktop width in pixels.
    pub width: u16,
    /// Negotiated desktop height in pixels.
    pub height: u16,
}

/// Why an [`RdpConfig`] cannot be used to open a session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// The host string is empty.
    EmptyHost,
    /// The port is zero.
    ZeroPort,
    /// A desktop dimension is outside the RDP-legal range.
    BadDimensions {
        /// The rejected `(width, height)`.
        size: (u16, u16),
    },
}

impl core::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EmptyHost => write!(f, "RDP host is empty"),
            Self::ZeroPort => write!(f, "RDP port is zero"),
            Self::BadDimensions { size } => {
                write!(f, "desktop size {size:?} outside the legal RDP range")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

impl RdpConfig {
    /// RDP requires a desktop between 200 and 8192 px on each axis (and the width
    /// must be even). These bounds match the protocol's `desktopWidth` /
    /// `desktopHeight` capability range.
    pub const MIN_DIM: u16 = 200;
    /// Upper bound on a desktop dimension.
    pub const MAX_DIM: u16 = 8192;

    /// A config with the standard port and the default desktop size.
    #[must_use]
    pub fn new(
        host: impl Into<String>,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        Self {
            host: host.into(),
            port: DEFAULT_PORT,
            username: username.into(),
            password: password.into(),
            domain: None,
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
        }
    }

    /// Set the desktop resolution (builder style).
    #[must_use]
    pub const fn with_resolution(mut self, width: u16, height: u16) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    /// Set the TCP port (builder style).
    #[must_use]
    pub const fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Set the Windows domain / workgroup (builder style).
    #[must_use]
    pub fn with_domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = Some(domain.into());
        self
    }

    /// Validate the config before a connect attempt.
    ///
    /// # Errors
    /// [`ConfigError`] if the host is empty, the port is zero, or a desktop
    /// dimension is out of the legal RDP range / the width is odd.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.host.trim().is_empty() {
            return Err(ConfigError::EmptyHost);
        }
        if self.port == 0 {
            return Err(ConfigError::ZeroPort);
        }
        let dims_ok = (Self::MIN_DIM..=Self::MAX_DIM).contains(&self.width)
            && (Self::MIN_DIM..=Self::MAX_DIM).contains(&self.height)
            && self.width % 2 == 0;
        if !dims_ok {
            return Err(ConfigError::BadDimensions {
                size: (self.width, self.height),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{ConfigError, RdpConfig, DEFAULT_HEIGHT, DEFAULT_PORT, DEFAULT_WIDTH};

    #[test]
    fn new_uses_standard_defaults() {
        let c = RdpConfig::new("10.42.0.9", "Administrator", "hunter2");
        assert_eq!(c.port, DEFAULT_PORT);
        assert_eq!(c.width, DEFAULT_WIDTH);
        assert_eq!(c.height, DEFAULT_HEIGHT);
        assert_eq!(c.domain, None);
        assert!(c.validate().is_ok());
    }

    #[test]
    fn builders_compose() {
        let c = RdpConfig::new("host", "u", "p")
            .with_port(3390)
            .with_resolution(1920, 1080)
            .with_domain("CORP");
        assert_eq!(c.port, 3390);
        assert_eq!((c.width, c.height), (1920, 1080));
        assert_eq!(c.domain.as_deref(), Some("CORP"));
        assert!(c.validate().is_ok());
    }

    #[test]
    fn empty_host_is_rejected() {
        let c = RdpConfig::new("   ", "u", "p");
        assert_eq!(c.validate(), Err(ConfigError::EmptyHost));
    }

    #[test]
    fn zero_port_is_rejected() {
        let c = RdpConfig::new("host", "u", "p").with_port(0);
        assert_eq!(c.validate(), Err(ConfigError::ZeroPort));
    }

    #[test]
    fn out_of_range_or_odd_dimensions_are_rejected() {
        let too_small = RdpConfig::new("host", "u", "p").with_resolution(100, 100);
        assert!(matches!(
            too_small.validate(),
            Err(ConfigError::BadDimensions { .. })
        ));
        let too_big = RdpConfig::new("host", "u", "p").with_resolution(9000, 9000);
        assert!(matches!(
            too_big.validate(),
            Err(ConfigError::BadDimensions { .. })
        ));
        let odd_width = RdpConfig::new("host", "u", "p").with_resolution(1281, 720);
        assert!(matches!(
            odd_width.validate(),
            Err(ConfigError::BadDimensions { .. })
        ));
    }
}
