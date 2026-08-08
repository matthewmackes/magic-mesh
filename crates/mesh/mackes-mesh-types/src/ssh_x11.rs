//! WL-FUNC-019 — bounded SSH/SFTP and X11 resource admission.
//!
//! This module is the typed boundary for the SSH-shaped resources that the
//! universal resource browser may eventually expose. It deliberately models
//! three different things instead of collapsing them into an executable
//! "remote session" string:
//!
//! * SFTP browsing over an SSH connection, with a typed browse root and path
//!   segments;
//! * one SSH-forwarded X11 application; and
//! * an explicit remote X11 desktop display.
//!
//! There is no command, executable path, URL, password, private-key path, or
//! raw X11 display string in this contract. Secret material is represented
//! only by the existing opaque [`crate::resources::SecretReference`]. An X11
//! resource may remain valid evidence while its local DRM seat reports
//! [`DrmSeatX11State::Unavailable`]; this is important because MCNF's DRM
//! shell does not grow an X server merely to make an X11 card look launchable.

use crate::resources::SecretReference;
use serde::{Deserialize, Deserializer, Serialize, de};
use std::fmt;

/// Schema version for the bounded SSH/X11 admission contract.
pub const SSH_X11_SCHEMA_VERSION: u16 = 1;
/// Maximum JSON body admitted by [`SshX11Resource::from_json`].
pub const MAX_SSH_X11_WIRE_BYTES: usize = 64 * 1024;
/// Maximum host field size in bytes.
pub const MAX_SSH_X11_HOST_BYTES: usize = 255;
/// Maximum SSH login-user field size in bytes.
pub const MAX_SSH_X11_USER_BYTES: usize = 64;
/// Maximum stable application/resource token size in bytes.
pub const MAX_SSH_X11_IDENTIFIER_BYTES: usize = 128;
/// Maximum number of path segments in one SFTP browse cursor.
pub const MAX_SFTP_PATH_SEGMENTS: usize = 32;
/// Maximum size of one typed SFTP path segment in bytes.
pub const MAX_SFTP_PATH_SEGMENT_BYTES: usize = 64;
/// Maximum X11 display number accepted by v1.
pub const MAX_X11_DISPLAY_NUMBER: u16 = 255;
/// Maximum X11 screen number accepted by v1.
pub const MAX_X11_SCREEN_NUMBER: u8 = 31;

/// Failure returned before an SSH/X11 resource becomes launchable evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshX11AdmissionError {
    /// The JSON body is larger than the wire allocation bound.
    PayloadTooLarge,
    /// The body is not valid JSON for the closed contract.
    MalformedWire,
    /// A resource carried a schema version this crate cannot interpret.
    UnsupportedSchema {
        /// Version found on the untrusted wire.
        found: u16,
    },
    /// A bounded field is empty, malformed, or uses a forbidden grammar.
    InvalidField(&'static str),
    /// A bounded string exceeded its wire limit.
    FieldTooLong(&'static str),
    /// A collection exceeded its explicit capacity.
    CapacityExceeded {
        /// Collection that exceeded its bound.
        field: &'static str,
        /// Maximum entries admitted by v1.
        max: usize,
    },
    /// A valid field was paired with the wrong typed discriminator.
    InvalidRelationship(&'static str),
}

impl fmt::Display for SshX11AdmissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PayloadTooLarge => f.write_str("SSH/X11 resource body is too large"),
            Self::MalformedWire => f.write_str("malformed SSH/X11 resource body"),
            Self::UnsupportedSchema { found } => {
                write!(f, "unsupported SSH/X11 schema version {found}")
            }
            Self::InvalidField(field) => write!(f, "invalid SSH/X11 field: {field}"),
            Self::FieldTooLong(field) => write!(f, "SSH/X11 field is too long: {field}"),
            Self::CapacityExceeded { field, max } => {
                write!(f, "SSH/X11 collection {field} exceeds {max} entries")
            }
            Self::InvalidRelationship(field) => {
                write!(f, "invalid SSH/X11 field relationship: {field}")
            }
        }
    }
}

impl std::error::Error for SshX11AdmissionError {}

/// Closed protocol discriminator for the three SSH/X11 resource forms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SshX11Protocol {
    /// SFTP browsing over an authenticated SSH connection.
    SftpOverSsh,
    /// One application whose display is allocated by SSH X11 forwarding.
    SshX11Forward,
    /// A complete remote X11 desktop bound to an explicit display number.
    X11Desktop,
}

/// Closed SSH authentication declaration.
///
/// Credential material never enters this type. Key and password auth carry
/// only an opaque secret-store reference; the adapter resolves it after a
/// separate local/mesh authorization step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
pub enum SshAuthentication {
    /// Use the already-authenticated MCNF mesh identity.
    MeshIdentity,
    /// Resolve an SSH private key from the approved secret store.
    SshKey {
        /// Opaque secret-store name, never a filesystem path or key body.
        credential_ref: SecretReference,
    },
    /// Resolve an SSH password from the approved secret store.
    Password {
        /// Opaque secret-store name, never plaintext password material.
        credential_ref: SecretReference,
    },
}

impl SshAuthentication {
    fn validate(&self) -> Result<(), SshX11AdmissionError> {
        match self {
            Self::MeshIdentity => Ok(()),
            Self::SshKey { credential_ref } | Self::Password { credential_ref } => {
                if credential_ref.as_str().is_empty() {
                    Err(SshX11AdmissionError::InvalidField(
                        "endpoint.auth.credential_ref",
                    ))
                } else {
                    Ok(())
                }
            }
        }
    }
}

/// Bounded SSH network/login target shared by all three resource kinds.
///
/// The host is a host token, not a URL. The user is a login identity, not a
/// shell fragment. Port zero is rejected and the auth declaration is closed
/// above.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshEndpoint {
    /// DNS name or IP literal; schemes, paths, and user-info are forbidden.
    pub host: String,
    /// SSH listener port.
    pub port: u16,
    /// Remote login user.
    pub user: String,
    /// Typed authentication selector.
    pub auth: SshAuthentication,
}

impl SshEndpoint {
    /// Construct a validated SSH endpoint.
    pub fn new(
        host: impl Into<String>,
        port: u16,
        user: impl Into<String>,
        auth: SshAuthentication,
    ) -> Result<Self, SshX11AdmissionError> {
        let endpoint = Self {
            host: host.into(),
            port,
            user: user.into(),
            auth,
        };
        endpoint.validate()?;
        Ok(endpoint)
    }

    /// Validate host, port, login identity, and authentication shape.
    pub fn validate(&self) -> Result<(), SshX11AdmissionError> {
        validate_host(&self.host)?;
        if self.port == 0 {
            return Err(SshX11AdmissionError::InvalidField("endpoint.port"));
        }
        validate_token(
            "endpoint.user",
            &self.user,
            MAX_SSH_X11_USER_BYTES,
            is_user_character,
        )?;
        self.auth.validate()
    }
}

/// Closed root from which an SFTP browse cursor may start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SftpBrowseRoot {
    /// The authenticated user's home directory.
    Home,
    /// The platform's explicitly shared SFTP root.
    Shared,
}

/// One safe SFTP path segment.
///
/// A segment is not an arbitrary path. It cannot contain a separator,
/// traversal marker, whitespace, shell metacharacter, or control character;
/// adapters may join validated segments using their own SFTP API.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct SftpPathSegment(String);

impl SftpPathSegment {
    /// Construct one validated, separator-free SFTP path segment.
    pub fn new(value: impl Into<String>) -> Result<Self, SshX11AdmissionError> {
        let value = value.into();
        validate_sftp_segment(&value)?;
        Ok(Self(value))
    }

    /// Borrow the validated segment for an SFTP API adapter.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for SftpPathSegment {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// Typed SFTP browse cursor with no raw path string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SftpBrowsePath {
    /// Allowlisted browse root.
    pub root: SftpBrowseRoot,
    /// Safe, bounded path segments below `root`.
    segments: Vec<SftpPathSegment>,
}

impl SftpBrowsePath {
    /// Construct a browse cursor from a closed root and validated segments.
    pub fn new(
        root: SftpBrowseRoot,
        segments: Vec<SftpPathSegment>,
    ) -> Result<Self, SshX11AdmissionError> {
        let path = Self { root, segments };
        path.validate()?;
        Ok(path)
    }

    /// Construct the user's home root.
    #[must_use]
    pub fn home() -> Self {
        Self {
            root: SftpBrowseRoot::Home,
            segments: Vec::new(),
        }
    }

    /// Construct the explicitly shared root.
    #[must_use]
    pub fn shared() -> Self {
        Self {
            root: SftpBrowseRoot::Shared,
            segments: Vec::new(),
        }
    }

    /// Borrow the validated segments in order.
    #[must_use]
    pub fn segments(&self) -> &[SftpPathSegment] {
        &self.segments
    }

    /// Validate segment count and each segment's grammar.
    pub fn validate(&self) -> Result<(), SshX11AdmissionError> {
        if self.segments.len() > MAX_SFTP_PATH_SEGMENTS {
            return Err(SshX11AdmissionError::CapacityExceeded {
                field: "sftp.browse_path.segments",
                max: MAX_SFTP_PATH_SEGMENTS,
            });
        }
        for segment in &self.segments {
            validate_sftp_segment(segment.as_str())?;
        }
        Ok(())
    }
}

/// A numeric X11 display and screen; no `DISPLAY` environment string is
/// accepted on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct X11DisplayNumber {
    /// X11 display number (`:0` is represented as `number: 0`).
    pub number: u16,
    /// X11 screen number.
    pub screen: u8,
}

impl X11DisplayNumber {
    /// Construct a bounded numeric X11 display.
    pub fn new(number: u16, screen: u8) -> Result<Self, SshX11AdmissionError> {
        let display = Self { number, screen };
        display.validate()?;
        Ok(display)
    }

    /// Validate numeric display and screen bounds.
    pub fn validate(&self) -> Result<(), SshX11AdmissionError> {
        if self.number > MAX_X11_DISPLAY_NUMBER {
            return Err(SshX11AdmissionError::InvalidField("x11.display.number"));
        }
        if self.screen > MAX_X11_SCREEN_NUMBER {
            return Err(SshX11AdmissionError::InvalidField("x11.display.screen"));
        }
        Ok(())
    }
}

/// Closed display binding for an X11 resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum X11DisplayBinding {
    /// Let the SSH server allocate the forwarded display.
    SshForwarded,
    /// Bind to one explicit remote X11 display and screen.
    Explicit {
        /// Numeric remote display; never a raw `DISPLAY` string.
        display: X11DisplayNumber,
    },
}

impl X11DisplayBinding {
    fn validate(&self) -> Result<(), SshX11AdmissionError> {
        if let Self::Explicit { display } = self {
            display.validate()?
        }
        Ok(())
    }
}

/// Why the DRM seat cannot provide a local X11 server to an X11 client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrmSeatX11UnavailableReason {
    /// The DRM seat has no X server, which is the normal Construct state.
    NoXServerOnDrmSeat,
    /// The seat is not currently usable for a display client.
    SeatNotReady,
    /// The seat reported an X11 mode the adapter cannot consume.
    UnsupportedDisplay,
}

/// Honest local X11 availability for a DRM-owned Construct seat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum DrmSeatX11State {
    /// A local X server exists and is bound to this numeric display.
    Available {
        /// Numeric display owned by the local X server.
        display: X11DisplayNumber,
    },
    /// No local X11 client can be claimed from this seat.
    Unavailable {
        /// Closed reason; no free-form diagnostic or fallback display.
        reason: DrmSeatX11UnavailableReason,
    },
}

impl DrmSeatX11State {
    /// Construct the normal no-X-server state for a direct DRM seat.
    #[must_use]
    pub const fn unavailable_no_x_server() -> Self {
        Self::Unavailable {
            reason: DrmSeatX11UnavailableReason::NoXServerOnDrmSeat,
        }
    }

    /// Whether this seat can host an X11 client.
    #[must_use]
    pub const fn is_available(&self) -> bool {
        matches!(self, Self::Available { .. })
    }

    /// Validate the nested display when the state is available.
    fn validate(&self) -> Result<(), SshX11AdmissionError> {
        if let Self::Available { display } = self {
            display.validate()?
        }
        Ok(())
    }
}

/// Stable identity for an approved remote X11 application.
///
/// The ID is an adapter/catalog key, not a desktop file, executable, shell
/// expression, URL, or filesystem path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct X11ApplicationId(String);

impl X11ApplicationId {
    /// Construct a bounded application/catalog identity.
    pub fn new(value: impl Into<String>) -> Result<Self, SshX11AdmissionError> {
        let value = value.into();
        validate_token(
            "x11.application_id",
            &value,
            MAX_SSH_X11_IDENTIFIER_BYTES,
            is_identifier_character,
        )?;
        Ok(Self(value))
    }

    /// Borrow the catalog identity.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for X11ApplicationId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// One SSH/SFTP browsing resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshSftpBrowser {
    /// Contract schema discriminator.
    pub schema_version: u16,
    /// Must be [`SshX11Protocol::SftpOverSsh`].
    pub protocol: SshX11Protocol,
    /// Typed SSH target and auth reference.
    pub endpoint: SshEndpoint,
    /// Typed browse root and safe path segments.
    pub browse_path: SftpBrowsePath,
}

impl SshSftpBrowser {
    /// Construct a validated SFTP browser resource.
    pub fn new(
        endpoint: SshEndpoint,
        browse_path: SftpBrowsePath,
    ) -> Result<Self, SshX11AdmissionError> {
        let resource = Self {
            schema_version: SSH_X11_SCHEMA_VERSION,
            protocol: SshX11Protocol::SftpOverSsh,
            endpoint,
            browse_path,
        };
        resource.validate()?;
        Ok(resource)
    }

    /// Validate the SFTP-specific admission shape.
    pub fn validate(&self) -> Result<(), SshX11AdmissionError> {
        validate_schema(self.schema_version)?;
        require_protocol(self.protocol, SshX11Protocol::SftpOverSsh, "sftp.protocol")?;
        self.endpoint.validate()?;
        self.browse_path.validate()
    }
}

/// One SSH-forwarded X11 application session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshForwardedX11Application {
    /// Contract schema discriminator.
    pub schema_version: u16,
    /// Must be [`SshX11Protocol::SshX11Forward`].
    pub protocol: SshX11Protocol,
    /// Typed SSH target and auth reference.
    pub endpoint: SshEndpoint,
    /// Approved application/catalog identity; never a command.
    pub application_id: X11ApplicationId,
    /// Must be [`X11DisplayBinding::SshForwarded`].
    pub display: X11DisplayBinding,
    /// Local DRM-seat X11 capability, including honest unavailable state.
    pub drm_seat: DrmSeatX11State,
}

impl SshForwardedX11Application {
    /// Construct a validated SSH-forwarded X11 application resource.
    pub fn new(
        endpoint: SshEndpoint,
        application_id: X11ApplicationId,
        drm_seat: DrmSeatX11State,
    ) -> Result<Self, SshX11AdmissionError> {
        let resource = Self {
            schema_version: SSH_X11_SCHEMA_VERSION,
            protocol: SshX11Protocol::SshX11Forward,
            endpoint,
            application_id,
            display: X11DisplayBinding::SshForwarded,
            drm_seat,
        };
        resource.validate()?;
        Ok(resource)
    }

    /// Validate the forwarded-application admission shape.
    pub fn validate(&self) -> Result<(), SshX11AdmissionError> {
        validate_schema(self.schema_version)?;
        require_protocol(
            self.protocol,
            SshX11Protocol::SshX11Forward,
            "x11_application.protocol",
        )?;
        self.endpoint.validate()?;
        validate_token(
            "x11_application.application_id",
            self.application_id.as_str(),
            MAX_SSH_X11_IDENTIFIER_BYTES,
            is_identifier_character,
        )?;
        self.display.validate()?;
        if !matches!(self.display, X11DisplayBinding::SshForwarded) {
            return Err(SshX11AdmissionError::InvalidRelationship(
                "x11_application.display_requires_ssh_forward",
            ));
        }
        self.drm_seat.validate()
    }

    /// Whether this resource can be handed to an X11 client on this seat.
    #[must_use]
    pub const fn is_available(&self) -> bool {
        self.drm_seat.is_available()
    }
}

/// One explicit full remote X11 desktop endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteX11DesktopEndpoint {
    /// Contract schema discriminator.
    pub schema_version: u16,
    /// Must be [`SshX11Protocol::X11Desktop`].
    pub protocol: SshX11Protocol,
    /// Typed SSH target and auth reference.
    pub endpoint: SshEndpoint,
    /// Must be [`X11DisplayBinding::Explicit`].
    pub display: X11DisplayBinding,
    /// Local DRM-seat X11 capability, including honest unavailable state.
    pub drm_seat: DrmSeatX11State,
}

impl RemoteX11DesktopEndpoint {
    /// Construct a validated full remote X11 desktop endpoint.
    pub fn new(
        endpoint: SshEndpoint,
        remote_display: X11DisplayNumber,
        drm_seat: DrmSeatX11State,
    ) -> Result<Self, SshX11AdmissionError> {
        let resource = Self {
            schema_version: SSH_X11_SCHEMA_VERSION,
            protocol: SshX11Protocol::X11Desktop,
            endpoint,
            display: X11DisplayBinding::Explicit {
                display: remote_display,
            },
            drm_seat,
        };
        resource.validate()?;
        Ok(resource)
    }

    /// Validate the explicit-desktop admission shape.
    pub fn validate(&self) -> Result<(), SshX11AdmissionError> {
        validate_schema(self.schema_version)?;
        require_protocol(
            self.protocol,
            SshX11Protocol::X11Desktop,
            "x11_desktop.protocol",
        )?;
        self.endpoint.validate()?;
        self.display.validate()?;
        if !matches!(self.display, X11DisplayBinding::Explicit { .. }) {
            return Err(SshX11AdmissionError::InvalidRelationship(
                "x11_desktop.display_requires_explicit_remote_display",
            ));
        }
        self.drm_seat.validate()
    }

    /// Whether this endpoint can be handed to an X11 client on this seat.
    #[must_use]
    pub const fn is_available(&self) -> bool {
        self.drm_seat.is_available()
    }
}

/// The three separately admitted SSH/X11 resource forms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum SshX11Resource {
    /// SSH/SFTP browsing resource.
    SftpBrowser(SshSftpBrowser),
    /// One SSH-forwarded X11 application.
    SshForwardedX11Application(SshForwardedX11Application),
    /// Explicit full remote X11 desktop endpoint.
    RemoteX11Desktop(RemoteX11DesktopEndpoint),
}

impl SshX11Resource {
    /// Validate one already-decoded resource before it reaches a consumer.
    pub fn validate(&self) -> Result<(), SshX11AdmissionError> {
        match self {
            Self::SftpBrowser(resource) => resource.validate(),
            Self::SshForwardedX11Application(resource) => resource.validate(),
            Self::RemoteX11Desktop(resource) => resource.validate(),
        }
    }

    /// Admit a bounded JSON resource from an untrusted discovery boundary.
    pub fn from_json(body: &str) -> Result<Self, SshX11AdmissionError> {
        if body.len() > MAX_SSH_X11_WIRE_BYTES {
            return Err(SshX11AdmissionError::PayloadTooLarge);
        }
        let resource: Self =
            serde_json::from_str(body).map_err(|_| SshX11AdmissionError::MalformedWire)?;
        resource.validate()?;
        Ok(resource)
    }

    /// Serialize a validated resource for a typed bus/catalog body.
    pub fn to_json(&self) -> Result<String, SshX11AdmissionError> {
        self.validate()?;
        serde_json::to_string(self).map_err(|_| SshX11AdmissionError::MalformedWire)
    }

    /// Whether the selected native/adapter client can currently use the
    /// resource. SFTP has no local X11-seat dependency; X11 resources report
    /// the DRM-seat truth rather than pretending a display exists.
    #[must_use]
    pub const fn is_available(&self) -> bool {
        match self {
            Self::SftpBrowser(_) => true,
            Self::SshForwardedX11Application(resource) => resource.is_available(),
            Self::RemoteX11Desktop(resource) => resource.is_available(),
        }
    }
}

/// Admit one bounded JSON SSH/X11 resource.
pub fn admit_json(body: &str) -> Result<SshX11Resource, SshX11AdmissionError> {
    SshX11Resource::from_json(body)
}

fn validate_schema(found: u16) -> Result<(), SshX11AdmissionError> {
    if found != SSH_X11_SCHEMA_VERSION {
        return Err(SshX11AdmissionError::UnsupportedSchema { found });
    }
    Ok(())
}

fn require_protocol(
    found: SshX11Protocol,
    expected: SshX11Protocol,
    field: &'static str,
) -> Result<(), SshX11AdmissionError> {
    if found != expected {
        return Err(SshX11AdmissionError::InvalidRelationship(field));
    }
    Ok(())
}

fn validate_host(value: &str) -> Result<(), SshX11AdmissionError> {
    if value.len() > MAX_SSH_X11_HOST_BYTES {
        return Err(SshX11AdmissionError::FieldTooLong("endpoint.host"));
    }
    if value.is_empty()
        || value.trim() != value
        || value.contains("://")
        || value.contains("..")
        || value.starts_with('.')
        || value.ends_with('.')
        || !value.is_ascii()
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '.' | ':' | '[' | ']')
        })
    {
        return Err(SshX11AdmissionError::InvalidField("endpoint.host"));
    }
    Ok(())
}

fn validate_token(
    field: &'static str,
    value: &str,
    max_bytes: usize,
    allowed: fn(char) -> bool,
) -> Result<(), SshX11AdmissionError> {
    if value.len() > max_bytes {
        return Err(SshX11AdmissionError::FieldTooLong(field));
    }
    if value.is_empty() || value.trim() != value || !value.is_ascii() || !value.chars().all(allowed)
    {
        return Err(SshX11AdmissionError::InvalidField(field));
    }
    Ok(())
}

fn validate_sftp_segment(value: &str) -> Result<(), SshX11AdmissionError> {
    if value.len() > MAX_SFTP_PATH_SEGMENT_BYTES {
        return Err(SshX11AdmissionError::FieldTooLong(
            "sftp.browse_path.segment",
        ));
    }
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.chars().all(|character| character == '.')
        || !value.is_ascii()
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | '@' | '+')
        })
    {
        return Err(SshX11AdmissionError::InvalidField(
            "sftp.browse_path.segment",
        ));
    }
    Ok(())
}

fn is_user_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
}

fn is_identifier_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint() -> SshEndpoint {
        SshEndpoint::new(
            "workstation.mesh",
            22,
            "mde",
            SshAuthentication::MeshIdentity,
        )
        .expect("typed endpoint")
    }

    fn key_endpoint() -> SshEndpoint {
        SshEndpoint::new(
            "files.mesh",
            2222,
            "operator",
            SshAuthentication::SshKey {
                credential_ref: SecretReference::new("secret:ssh-files").expect("secret ref"),
            },
        )
        .expect("typed key endpoint")
    }

    fn available_seat() -> DrmSeatX11State {
        DrmSeatX11State::Available {
            display: X11DisplayNumber::new(0, 0).expect("display"),
        }
    }

    #[test]
    fn sftp_browser_uses_typed_root_and_segments_without_a_raw_path() {
        let segment = SftpPathSegment::new("Projects").expect("segment");
        let path = SftpBrowsePath::new(SftpBrowseRoot::Home, vec![segment]).expect("path");
        let resource = SshSftpBrowser::new(key_endpoint(), path).expect("SFTP resource");

        assert_eq!(resource.protocol, SshX11Protocol::SftpOverSsh);
        assert_eq!(resource.browse_path.segments()[0].as_str(), "Projects");
        assert!(resource.validate().is_ok());
    }

    #[test]
    fn forwarded_application_and_full_desktop_have_distinct_closed_protocols_and_displays() {
        let application = SshForwardedX11Application::new(
            endpoint(),
            X11ApplicationId::new("org.example.Editor").expect("application id"),
            available_seat(),
        )
        .expect("forwarded application");
        let desktop = RemoteX11DesktopEndpoint::new(
            endpoint(),
            X11DisplayNumber::new(1, 0).expect("remote display"),
            available_seat(),
        )
        .expect("remote desktop");

        assert_eq!(application.protocol, SshX11Protocol::SshX11Forward);
        assert!(matches!(
            application.display,
            X11DisplayBinding::SshForwarded
        ));
        assert_eq!(desktop.protocol, SshX11Protocol::X11Desktop);
        assert!(matches!(
            desktop.display,
            X11DisplayBinding::Explicit { .. }
        ));
    }

    #[test]
    fn no_x_server_is_valid_evidence_but_never_claims_x11_availability() {
        let seat = DrmSeatX11State::unavailable_no_x_server();
        let application = SshForwardedX11Application::new(
            endpoint(),
            X11ApplicationId::new("org.example.Editor").expect("application id"),
            seat.clone(),
        )
        .expect("evidence remains representable");
        let desktop = RemoteX11DesktopEndpoint::new(
            endpoint(),
            X11DisplayNumber::new(1, 0).expect("remote display"),
            seat,
        )
        .expect("evidence remains representable");

        assert!(!application.drm_seat.is_available());
        assert!(!application.is_available());
        assert!(!desktop.is_available());
        assert!(matches!(
            application.drm_seat,
            DrmSeatX11State::Unavailable {
                reason: DrmSeatX11UnavailableReason::NoXServerOnDrmSeat
            }
        ));
    }

    #[test]
    fn all_three_resource_forms_round_trip_through_the_bounded_wire() {
        let resources = [
            SshX11Resource::SftpBrowser(
                SshSftpBrowser::new(key_endpoint(), SftpBrowsePath::shared())
                    .expect("SFTP resource"),
            ),
            SshX11Resource::SshForwardedX11Application(
                SshForwardedX11Application::new(
                    endpoint(),
                    X11ApplicationId::new("org.example.Editor").expect("application id"),
                    available_seat(),
                )
                .expect("forwarded application"),
            ),
            SshX11Resource::RemoteX11Desktop(
                RemoteX11DesktopEndpoint::new(
                    endpoint(),
                    X11DisplayNumber::new(2, 1).expect("remote display"),
                    available_seat(),
                )
                .expect("remote desktop"),
            ),
        ];

        for resource in resources {
            let body = resource.to_json().expect("serialize admitted resource");
            let decoded = SshX11Resource::from_json(&body).expect("decode admitted resource");
            assert_eq!(decoded, resource);
        }
    }

    #[test]
    fn secret_auth_round_trip_contains_only_the_opaque_reference() {
        let resource = SshX11Resource::SftpBrowser(
            SshSftpBrowser::new(key_endpoint(), SftpBrowsePath::home()).expect("SFTP resource"),
        );
        let body = resource.to_json().expect("serialize");

        assert!(body.contains("secret:ssh-files"));
        assert!(!body.contains("BEGIN"));
        assert!(!body.contains("private_key"));
        assert_eq!(
            SshX11Resource::from_json(&body).expect("round trip"),
            resource
        );
    }

    #[test]
    fn hostile_urls_commands_paths_and_traversal_are_rejected() {
        assert!(
            SshEndpoint::new(
                "ssh://workstation.mesh/launch?cmd=id",
                22,
                "mde",
                SshAuthentication::MeshIdentity,
            )
            .is_err()
        );
        assert!(
            SshEndpoint::new(
                "workstation.mesh",
                22,
                "mde;id",
                SshAuthentication::MeshIdentity,
            )
            .is_err()
        );
        assert!(SftpPathSegment::new("../secrets").is_err());
        assert!(SftpPathSegment::new("$(id)").is_err());
        assert!(X11ApplicationId::new("/usr/bin/xterm").is_err());
        assert!(X11ApplicationId::new("https://evil.invalid").is_err());
    }

    #[test]
    fn hostile_wire_rejects_unknown_fields_and_open_ended_protocol_auth_display_values() {
        let unknown_field = r#"{
            "kind":"sftp_browser",
            "data":{
                "schema_version":1,
                "protocol":"sftp_over_ssh",
                "endpoint":{
                    "host":"files.mesh",
                    "port":22,
                    "user":"mde",
                    "auth":{"method":"mesh_identity"},
                    "command":"id"
                },
                "browse_path":{"root":"home","segments":[]}
            }
        }"#;
        assert_eq!(
            SshX11Resource::from_json(unknown_field),
            Err(SshX11AdmissionError::MalformedWire)
        );

        let open_protocol = r#"{
            "kind":"sftp_browser",
            "data":{
                "schema_version":1,
                "protocol":"telnet",
                "endpoint":{
                    "host":"files.mesh",
                    "port":22,
                    "user":"mde",
                    "auth":{"method":"mesh_identity"}
                },
                "browse_path":{"root":"home","segments":[]}
            }
        }"#;
        assert_eq!(
            SshX11Resource::from_json(open_protocol),
            Err(SshX11AdmissionError::MalformedWire)
        );

        let open_auth = r#"{
            "kind":"sftp_browser",
            "data":{
                "schema_version":1,
                "protocol":"sftp_over_ssh",
                "endpoint":{
                    "host":"files.mesh",
                    "port":22,
                    "user":"mde",
                    "auth":{"method":"shell","value":"id"}
                },
                "browse_path":{"root":"home","segments":[]}
            }
        }"#;
        assert_eq!(
            SshX11Resource::from_json(open_auth),
            Err(SshX11AdmissionError::MalformedWire)
        );

        let open_display = r#"{
            "kind":"ssh_forwarded_x11_application",
            "data":{
                "schema_version":1,
                "protocol":"ssh_x11_forward",
                "endpoint":{
                    "host":"workstation.mesh",
                    "port":22,
                    "user":"mde",
                    "auth":{"method":"mesh_identity"}
                },
                "application_id":"org.example.Editor",
                "display":"${DISPLAY}",
                "drm_seat":{"state":"unavailable","reason":"no_x_server_on_drm_seat"}
            }
        }"#;
        assert_eq!(
            SshX11Resource::from_json(open_display),
            Err(SshX11AdmissionError::MalformedWire)
        );
    }

    #[test]
    fn oversized_wire_and_path_depth_are_rejected_before_admission() {
        let oversized = "x".repeat(MAX_SSH_X11_WIRE_BYTES + 1);
        assert_eq!(
            SshX11Resource::from_json(&oversized),
            Err(SshX11AdmissionError::PayloadTooLarge)
        );

        let segments = (0..=MAX_SFTP_PATH_SEGMENTS)
            .map(|index| SftpPathSegment::new(format!("segment{index}")))
            .collect::<Result<Vec<_>, _>>()
            .expect("individual segments are valid");
        assert_eq!(
            SftpBrowsePath::new(SftpBrowseRoot::Home, segments),
            Err(SshX11AdmissionError::CapacityExceeded {
                field: "sftp.browse_path.segments",
                max: MAX_SFTP_PATH_SEGMENTS,
            })
        );
    }

    #[test]
    fn protocol_and_display_relationships_cannot_be_swapped() {
        let mut application = SshForwardedX11Application::new(
            endpoint(),
            X11ApplicationId::new("org.example.Editor").expect("application id"),
            available_seat(),
        )
        .expect("application");
        application.protocol = SshX11Protocol::X11Desktop;
        assert_eq!(
            application.validate(),
            Err(SshX11AdmissionError::InvalidRelationship(
                "x11_application.protocol",
            ))
        );

        let mut desktop = RemoteX11DesktopEndpoint::new(
            endpoint(),
            X11DisplayNumber::new(1, 0).expect("display"),
            available_seat(),
        )
        .expect("desktop");
        desktop.display = X11DisplayBinding::SshForwarded;
        assert_eq!(
            desktop.validate(),
            Err(SshX11AdmissionError::InvalidRelationship(
                "x11_desktop.display_requires_explicit_remote_display",
            ))
        );
    }
}
