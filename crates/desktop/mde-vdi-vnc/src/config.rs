//! Connection configuration for a VNC/RFB session.

/// The RFB display-0 port (`5900 + display`).
pub const DEFAULT_PORT: u16 = 5900;
/// Default framebuffer width before the server's `ServerInit` is seen.
pub const DEFAULT_WIDTH: u16 = 1024;
/// Default framebuffer height before the server's `ServerInit` is seen.
pub const DEFAULT_HEIGHT: u16 = 768;

/// Everything needed to open a VNC/RFB session to a single host.
///
/// Built by the shell from a discovered desktop (mesh service registry /
/// DATACENTER inventory, lock 22) — typically an XCP-ng host's XAPI/`Xvnc`
/// console, the universal fallback when a guest has no RDP (lock 21). The mesh
/// cert gates the connection (lock 23); the optional [`VncConfig::password`] is
/// the guest's RFB (VNC-auth) secret, sourced from the sealed cred vault.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VncConfig {
    /// Host to connect to (mesh overlay address or hostname).
    pub host: String,
    /// TCP port — [`DEFAULT_PORT`] for display 0.
    pub port: u16,
    /// Optional RFB (VNC-auth) password. Classic VNC auth uses up to 8 bytes;
    /// a longer secret is truncated by the DES challenge step on connect.
    pub password: Option<String>,
    /// Whether to ask the server to keep other clients connected (RFB
    /// `ClientInit` shared-flag).
    pub shared: bool,
    /// Initial framebuffer width; the server's `ServerInit` is authoritative and
    /// the session resizes to it.
    pub width: u16,
    /// Initial framebuffer height; resized to the server's `ServerInit`.
    pub height: u16,
}

/// Why a [`VncConfig`] cannot be used to open a session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// The host string is empty.
    EmptyHost,
    /// The port is zero.
    ZeroPort,
    /// A framebuffer dimension is outside the supported range.
    BadDimensions {
        /// The rejected `(width, height)`.
        size: (u16, u16),
    },
}

impl core::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EmptyHost => write!(f, "VNC host is empty"),
            Self::ZeroPort => write!(f, "VNC port is zero"),
            Self::BadDimensions { size } => {
                write!(f, "framebuffer size {size:?} outside the supported range")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

impl VncConfig {
    /// Smallest framebuffer dimension the session will allocate.
    pub const MIN_DIM: u16 = 16;
    /// Largest framebuffer dimension the session will allocate.
    pub const MAX_DIM: u16 = 8192;

    /// A config for `host` on the standard port with no password, an unshared
    /// session, and the default framebuffer size.
    #[must_use]
    pub fn new(host: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            port: DEFAULT_PORT,
            password: None,
            shared: false,
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
        }
    }

    /// Set the RFB password (builder style).
    #[must_use]
    pub fn with_password(mut self, password: impl Into<String>) -> Self {
        self.password = Some(password.into());
        self
    }

    /// Set the TCP port (builder style).
    #[must_use]
    pub const fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Ask to share the session with other clients (builder style).
    #[must_use]
    pub const fn shared(mut self, shared: bool) -> Self {
        self.shared = shared;
        self
    }

    /// Set the initial framebuffer size (builder style).
    #[must_use]
    pub const fn with_size(mut self, width: u16, height: u16) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    /// Validate the config before a connect attempt.
    ///
    /// # Errors
    /// [`ConfigError`] if the host is empty, the port is zero, or a framebuffer
    /// dimension is out of the supported range. (Unlike RDP, RFB does not require
    /// an even width.)
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.host.trim().is_empty() {
            return Err(ConfigError::EmptyHost);
        }
        if self.port == 0 {
            return Err(ConfigError::ZeroPort);
        }
        let dims_ok = (Self::MIN_DIM..=Self::MAX_DIM).contains(&self.width)
            && (Self::MIN_DIM..=Self::MAX_DIM).contains(&self.height);
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
    use super::{ConfigError, VncConfig, DEFAULT_HEIGHT, DEFAULT_PORT, DEFAULT_WIDTH};

    #[test]
    fn new_uses_standard_defaults() {
        let c = VncConfig::new("10.42.0.9");
        assert_eq!(c.port, DEFAULT_PORT);
        assert_eq!(c.width, DEFAULT_WIDTH);
        assert_eq!(c.height, DEFAULT_HEIGHT);
        assert_eq!(c.password, None);
        assert!(!c.shared);
        assert!(c.validate().is_ok());
    }

    #[test]
    fn builders_compose() {
        let c = VncConfig::new("host")
            .with_port(5901)
            .with_password("hunter2")
            .with_size(1920, 1080)
            .shared(true);
        assert_eq!(c.port, 5901);
        assert_eq!((c.width, c.height), (1920, 1080));
        assert_eq!(c.password.as_deref(), Some("hunter2"));
        assert!(c.shared);
        assert!(c.validate().is_ok());
    }

    #[test]
    fn empty_host_is_rejected() {
        assert_eq!(
            VncConfig::new("   ").validate(),
            Err(ConfigError::EmptyHost)
        );
    }

    #[test]
    fn zero_port_is_rejected() {
        assert_eq!(
            VncConfig::new("host").with_port(0).validate(),
            Err(ConfigError::ZeroPort)
        );
    }

    #[test]
    fn out_of_range_dimensions_are_rejected() {
        let too_small = VncConfig::new("host").with_size(8, 8);
        assert!(matches!(
            too_small.validate(),
            Err(ConfigError::BadDimensions { .. })
        ));
        let too_big = VncConfig::new("host").with_size(9000, 9000);
        assert!(matches!(
            too_big.validate(),
            Err(ConfigError::BadDimensions { .. })
        ));
        // Odd width is fine for RFB (unlike RDP).
        assert!(VncConfig::new("host")
            .with_size(801, 600)
            .validate()
            .is_ok());
    }
}
