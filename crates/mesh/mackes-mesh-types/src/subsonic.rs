//! Bounded OpenSubsonic-compatible media admission contracts.
//!
//! This module is the shared seam for Navidrome, Airsonic-compatible, and
//! other compatible Subsonic providers. It is deliberately a card/admission
//! contract, not an HTTP client: endpoint identity and credentials are opaque
//! references, while capabilities and actions are closed enums. There is no
//! URL, command, cookie, token, or password field to project into a card.

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeSet;
use std::fmt;

use crate::resources::SecretReference;

/// The only Subsonic admission schema currently understood by consumers.
pub const SUBSONIC_CONTRACT_VERSION: u16 = 1;

const MAX_RESOURCE_ID_BYTES: usize = 128;
const MAX_DISPLAY_NAME_BYTES: usize = 192;
const MAX_OPAQUE_REFERENCE_BYTES: usize = 128;
const MAX_CAPABILITIES: usize = 16;
const MAX_ACTIONS: usize = 16;

/// A validation failure at the typed Subsonic admission boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubsonicValidationError {
    /// The consumer does not implement the supplied contract version.
    UnsupportedSchema(u16),
    /// A bounded field is blank, malformed, or contains an unsafe value.
    InvalidField(&'static str),
    /// A bounded field exceeds its wire limit.
    FieldTooLong(&'static str),
    /// A bounded collection exceeds its maximum size.
    CapacityExceeded(&'static str, usize),
    /// A set-like collection contains a repeated value.
    Duplicate(&'static str),
    /// Fields that are individually valid form an unsafe or contradictory
    /// relationship.
    InvalidRelationship(&'static str),
}

impl fmt::Display for SubsonicValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchema(version) => {
                write!(formatter, "unsupported Subsonic schema version {version}")
            }
            Self::InvalidField(field) => write!(formatter, "invalid Subsonic field: {field}"),
            Self::FieldTooLong(field) => {
                write!(formatter, "Subsonic field is too long: {field}")
            }
            Self::CapacityExceeded(field, max) => {
                write!(
                    formatter,
                    "Subsonic collection {field} exceeds {max} entries"
                )
            }
            Self::Duplicate(field) => write!(formatter, "duplicate Subsonic value: {field}"),
            Self::InvalidRelationship(field) => {
                write!(formatter, "invalid Subsonic relationship: {field}")
            }
        }
    }
}

impl std::error::Error for SubsonicValidationError {}

/// Provider family used for attribution without changing the client seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubsonicProviderFamily {
    /// Navidrome, including its `OpenSubsonic` extensions.
    Navidrome,
    /// Airsonic or an Airsonic-compatible deployment.
    AirsonicCompatible,
    /// Another server implementing the admitted Subsonic/OpenSubsonic shape.
    SubsonicCompatible,
}

/// API profile admitted by the typed adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubsonicApiProfile {
    /// The `OpenSubsonic` JSON/XML-compatible contract.
    OpenSubsonic,
    /// A legacy Subsonic-compatible JSON/XML contract accepted by the adapter.
    SubsonicCompatible,
}

/// Stable identity of a cataloged Subsonic resource.
///
/// This is an opaque adapter/catalog identifier, not a URL, path, host name,
/// command, or credential. Its restricted grammar also makes accidental
/// promotion of a locator into a resource card fail closed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct SubsonicResourceId(String);

impl SubsonicResourceId {
    /// Validate and construct a resource identity.
    ///
    /// # Errors
    ///
    /// Returns an error when `value` is blank, malformed, or exceeds the
    /// contract's size limit.
    pub fn new(value: impl Into<String>) -> Result<Self, SubsonicValidationError> {
        let value = value.into();
        validate_opaque_identifier("resource_id", &value, MAX_RESOURCE_ID_BYTES)?;
        Ok(Self(value))
    }

    /// Borrow the opaque resource identity.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for SubsonicResourceId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// Opaque reference to an approved endpoint record in the secret/config
/// substrate. The actual URL is resolved only by the typed adapter.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct SubsonicEndpointRef(String);

impl SubsonicEndpointRef {
    /// Validate and construct an endpoint-record reference.
    ///
    /// # Errors
    ///
    /// Returns an error when `value` is blank, malformed, or exceeds the
    /// contract's size limit.
    pub fn new(value: impl Into<String>) -> Result<Self, SubsonicValidationError> {
        let value = value.into();
        validate_opaque_identifier("endpoint_ref", &value, MAX_OPAQUE_REFERENCE_BYTES)?;
        Ok(Self(value))
    }

    /// Borrow the opaque endpoint reference.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for SubsonicEndpointRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// Opaque reference to a credential held by the approved secret store.
///
/// The credential value never enters this contract. The reference is retained
/// only so the typed adapter can request the secret at the action boundary.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SubsonicCredentialRef(SecretReference);

impl SubsonicCredentialRef {
    /// Validate and construct a secret-store reference.
    ///
    /// # Errors
    ///
    /// Returns an error when `value` is not a valid secret-store reference.
    pub fn new(value: impl Into<String>) -> Result<Self, SubsonicValidationError> {
        SecretReference::new(value)
            .map(Self)
            .map_err(|_| SubsonicValidationError::InvalidField("auth.credential_ref"))
    }

    /// Borrow the opaque secret-store reference.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl Serialize for SubsonicCredentialRef {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SubsonicCredentialRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// Authentication state visible to the admission contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum SubsonicAuthState {
    /// The provider can be used without a credential.
    NotRequired,
    /// The provider needs an approved credential before launch.
    Required,
    /// An approved credential reference is available to the adapter.
    Authorized {
        /// Opaque secret-store reference; never the credential value.
        credential_ref: SubsonicCredentialRef,
    },
    /// The required auth provider or secret-store integration is unavailable.
    Unavailable,
}

impl SubsonicAuthState {
    const fn can_launch(&self) -> bool {
        matches!(self, Self::NotRequired | Self::Authorized { .. })
    }
}

/// Features the typed `OpenSubsonic` adapter may expose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubsonicCapability {
    /// Browse libraries and albums through the native client.
    Browse,
    /// Search the provider's library.
    Search,
    /// Resolve and play provider-owned media through the native client.
    Stream,
    /// Read and update provider playlists through typed operations.
    Playlists,
    /// Submit playback progress through a typed scrobble operation.
    Scrobble,
    /// Retrieve provider-owned cover art through the native client.
    CoverArt,
    /// Read/write typed annotations such as stars and ratings.
    Annotations,
    /// Browse podcast channels and episodes.
    Podcasts,
    /// Browse audiobook/chapter media exposed by the provider.
    Audiobooks,
    /// Read/write bookmark and playback-position records.
    Bookmarks,
    /// Browse provider radio stations.
    Radio,
    /// Manage offline downloads through the admitted adapter.
    Downloads,
    /// Synchronize a queue revision with the provider/mesh daemon.
    QueueSync,
}

/// Closed, safe actions a Subsonic resource card may advertise.
///
/// There is intentionally no URL, shell, executable, arbitrary endpoint, or
/// provider-command variant. Every action is dispatched through the named
/// adapter and its admission policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubsonicAction {
    /// Show typed diagnostics and provider attribution.
    Inspect,
    /// Open the typed auth/configuration flow.
    Authenticate,
    /// Launch the native Subsonic media client.
    Launch,
    /// Browse the provider library.
    Browse,
    /// Search the provider library.
    Search,
    /// Play an admitted provider item through the native client.
    Play,
    /// Manage provider playlists through typed operations.
    ManagePlaylists,
    /// Submit progress through a typed scrobble operation.
    Scrobble,
    /// Manage stars/ratings/annotations.
    ManageAnnotations,
    /// Browse podcast channels and episodes.
    BrowsePodcasts,
    /// Browse audiobooks and chapters.
    BrowseAudiobooks,
    /// Read/write playback bookmarks.
    ManageBookmarks,
    /// Browse provider radio stations.
    BrowseRadio,
    /// Start/cancel/remove a managed download.
    ManageDownloads,
    /// Synchronize the typed queue revision.
    SyncQueue,
}

/// Honest reasons why a discovered provider remains visible but unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubsonicUnavailableReason {
    /// The endpoint did not answer within the bounded adapter probe.
    ProviderUnreachable,
    /// An approved credential is required before the client may launch.
    AuthenticationRequired,
    /// The auth or secret-store dependency is not available.
    AuthenticationUnavailable,
    /// No approved native Subsonic client is available on this seat.
    NoCompatibleClient,
    /// The observed API profile is not admitted by the typed client.
    UnsupportedApi,
    /// Local trust or provider policy blocks the action.
    PolicyDenied,
    /// The retained observation is too old to admit a launch.
    StaleObservation,
}

/// Whether the typed media resource can be handed to the native client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SubsonicAdmissionState {
    /// The endpoint, auth state, client, and required capabilities passed
    /// admission and may be handed to the typed client.
    Launchable,
    /// The card is retained as evidence, but no launch action is permitted.
    Unavailable {
        /// Closed diagnostic reason; no provider response or secret text.
        reason: SubsonicUnavailableReason,
    },
}

/// One bounded OpenSubsonic-compatible media resource card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubsonicResourceCard {
    /// Contract schema discriminator.
    pub schema_version: u16,
    /// Stable opaque catalog identity.
    pub resource_id: SubsonicResourceId,
    /// Bounded user-facing name; not used as a locator.
    pub display_name: String,
    /// Provider family attribution.
    pub provider: SubsonicProviderFamily,
    /// API profile admitted by the typed adapter.
    pub api_profile: SubsonicApiProfile,
    /// Opaque endpoint-record reference; never a URL.
    pub endpoint_ref: SubsonicEndpointRef,
    /// Credential state and optional opaque secret-store reference.
    pub auth: SubsonicAuthState,
    /// Deduplicated capabilities admitted by this provider observation.
    pub capabilities: Vec<SubsonicCapability>,
    /// Deduplicated typed actions exposed by the card.
    pub actions: Vec<SubsonicAction>,
    /// Final availability gate for the native client handoff.
    pub admission: SubsonicAdmissionState,
}

impl SubsonicResourceCard {
    /// Validate all bounded fields and cross-field admission relationships.
    ///
    /// # Errors
    ///
    /// Returns an error when a bounded field or cross-field admission
    /// relationship is invalid.
    pub fn validate(&self) -> Result<(), SubsonicValidationError> {
        if self.schema_version != SUBSONIC_CONTRACT_VERSION {
            return Err(SubsonicValidationError::UnsupportedSchema(
                self.schema_version,
            ));
        }
        validate_opaque_identifier(
            "resource_id",
            self.resource_id.as_str(),
            MAX_RESOURCE_ID_BYTES,
        )?;
        validate_display_name(&self.display_name)?;
        validate_opaque_identifier(
            "endpoint_ref",
            self.endpoint_ref.as_str(),
            MAX_OPAQUE_REFERENCE_BYTES,
        )?;
        validate_capabilities(&self.capabilities)?;
        validate_actions(self)?;

        match (&self.auth, &self.admission) {
            (SubsonicAuthState::Required, SubsonicAdmissionState::Unavailable { reason })
                if *reason == SubsonicUnavailableReason::AuthenticationRequired => {}
            (SubsonicAuthState::Unavailable, SubsonicAdmissionState::Unavailable { reason })
                if *reason == SubsonicUnavailableReason::AuthenticationUnavailable => {}
            (auth, SubsonicAdmissionState::Launchable) if auth.can_launch() => {}
            (_, SubsonicAdmissionState::Launchable) => {
                return Err(SubsonicValidationError::InvalidRelationship(
                    "admission.launchable_auth",
                ));
            }
            (_, SubsonicAdmissionState::Unavailable { .. }) => {}
        }

        Ok(())
    }

    /// Validate and retain a provider card at an adapter boundary.
    ///
    /// # Errors
    ///
    /// Returns an error when the card fails validation.
    pub fn admitted(self) -> Result<Self, SubsonicValidationError> {
        self.validate()?;
        Ok(self)
    }

    /// Return true only when the complete typed handoff is launchable.
    #[must_use]
    pub fn is_launchable(&self) -> bool {
        self.validate().is_ok() && matches!(self.admission, SubsonicAdmissionState::Launchable)
    }
}

fn validate_capabilities(
    capabilities: &[SubsonicCapability],
) -> Result<(), SubsonicValidationError> {
    if capabilities.len() > MAX_CAPABILITIES {
        return Err(SubsonicValidationError::CapacityExceeded(
            "capabilities",
            MAX_CAPABILITIES,
        ));
    }
    let mut seen = BTreeSet::new();
    for capability in capabilities {
        if !seen.insert(capability) {
            return Err(SubsonicValidationError::Duplicate("capabilities"));
        }
    }
    Ok(())
}

fn validate_actions(card: &SubsonicResourceCard) -> Result<(), SubsonicValidationError> {
    if card.actions.len() > MAX_ACTIONS {
        return Err(SubsonicValidationError::CapacityExceeded(
            "actions",
            MAX_ACTIONS,
        ));
    }
    let mut seen = BTreeSet::new();
    for action in &card.actions {
        if !seen.insert(action) {
            return Err(SubsonicValidationError::Duplicate("actions"));
        }

        let required_capability = match action {
            SubsonicAction::Inspect | SubsonicAction::Authenticate => None,
            SubsonicAction::Launch | SubsonicAction::Browse => Some(SubsonicCapability::Browse),
            SubsonicAction::Search => Some(SubsonicCapability::Search),
            SubsonicAction::Play => Some(SubsonicCapability::Stream),
            SubsonicAction::ManagePlaylists => Some(SubsonicCapability::Playlists),
            SubsonicAction::Scrobble => Some(SubsonicCapability::Scrobble),
            SubsonicAction::ManageAnnotations => Some(SubsonicCapability::Annotations),
            SubsonicAction::BrowsePodcasts => Some(SubsonicCapability::Podcasts),
            SubsonicAction::BrowseAudiobooks => Some(SubsonicCapability::Audiobooks),
            SubsonicAction::ManageBookmarks => Some(SubsonicCapability::Bookmarks),
            SubsonicAction::BrowseRadio => Some(SubsonicCapability::Radio),
            SubsonicAction::ManageDownloads => Some(SubsonicCapability::Downloads),
            SubsonicAction::SyncQueue => Some(SubsonicCapability::QueueSync),
        };
        if required_capability.is_some_and(|capability| !card.capabilities.contains(&capability)) {
            return Err(SubsonicValidationError::InvalidRelationship(
                "action.capability",
            ));
        }
    }

    if !card.actions.contains(&SubsonicAction::Inspect) {
        return Err(SubsonicValidationError::InvalidRelationship(
            "actions.inspect",
        ));
    }
    if card.actions.contains(&SubsonicAction::Authenticate)
        && !matches!(&card.auth, SubsonicAuthState::Required)
    {
        return Err(SubsonicValidationError::InvalidRelationship(
            "actions.authenticate",
        ));
    }
    if card.actions.contains(&SubsonicAction::Launch)
        && (!matches!(card.admission, SubsonicAdmissionState::Launchable)
            || !card.capabilities.contains(&SubsonicCapability::Stream)
            || !card.auth.can_launch())
    {
        return Err(SubsonicValidationError::InvalidRelationship(
            "actions.launch",
        ));
    }
    if matches!(card.admission, SubsonicAdmissionState::Launchable)
        && !card.actions.contains(&SubsonicAction::Launch)
    {
        return Err(SubsonicValidationError::InvalidRelationship(
            "admission.launch_action",
        ));
    }
    if matches!(&card.auth, SubsonicAuthState::Required)
        && !card.actions.contains(&SubsonicAction::Authenticate)
    {
        return Err(SubsonicValidationError::InvalidRelationship(
            "auth.authenticate_action",
        ));
    }
    Ok(())
}

fn validate_display_name(value: &str) -> Result<(), SubsonicValidationError> {
    if value.len() > MAX_DISPLAY_NAME_BYTES {
        return Err(SubsonicValidationError::FieldTooLong("display_name"));
    }
    if value.is_empty()
        || value.trim() != value
        || value.chars().any(char::is_control)
        || ["://", "?", "#", "\\"]
            .iter()
            .any(|marker| value.contains(marker))
        || [';', '|', '&', '$', '`']
            .iter()
            .any(|marker| value.contains(*marker))
        || looks_like_secret(value)
    {
        return Err(SubsonicValidationError::InvalidField("display_name"));
    }
    Ok(())
}

fn validate_opaque_identifier(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), SubsonicValidationError> {
    if value.len() > max_bytes {
        return Err(SubsonicValidationError::FieldTooLong(field));
    }
    if value.is_empty()
        || value.trim() != value
        || !value.is_ascii()
        || value.chars().any(char::is_control)
        || value.starts_with('.')
        || value.ends_with('.')
        || value.contains("..")
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
        || looks_like_secret(value)
    {
        return Err(SubsonicValidationError::InvalidField(field));
    }
    Ok(())
}

fn looks_like_secret(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "authorization:",
        "bearer ",
        "password",
        "passwd",
        "token",
        "api_key",
        "apikey",
        "client_secret",
        "private_key",
        "-----begin",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
        || lower.contains("://")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launchable_card() -> SubsonicResourceCard {
        SubsonicResourceCard {
            schema_version: SUBSONIC_CONTRACT_VERSION,
            resource_id: SubsonicResourceId::new("subsonic-navidrome-primary")
                .expect("valid resource id"),
            display_name: "Navidrome · Music".to_string(),
            provider: SubsonicProviderFamily::Navidrome,
            api_profile: SubsonicApiProfile::OpenSubsonic,
            endpoint_ref: SubsonicEndpointRef::new("endpoint-navidrome-primary")
                .expect("valid endpoint ref"),
            auth: SubsonicAuthState::Authorized {
                credential_ref: SubsonicCredentialRef::new("media/subsonic/readonly")
                    .expect("valid credential ref"),
            },
            capabilities: vec![
                SubsonicCapability::Browse,
                SubsonicCapability::Search,
                SubsonicCapability::Stream,
                SubsonicCapability::Playlists,
            ],
            actions: vec![
                SubsonicAction::Inspect,
                SubsonicAction::Launch,
                SubsonicAction::Browse,
                SubsonicAction::Search,
                SubsonicAction::Play,
                SubsonicAction::ManagePlaylists,
            ],
            admission: SubsonicAdmissionState::Launchable,
        }
    }

    #[test]
    fn launchable_card_validates_with_only_opaque_refs_and_typed_actions() {
        let card = launchable_card();
        assert!(card.validate().is_ok());
        assert!(card.is_launchable());

        let encoded = serde_json::to_string(&card).expect("serialize card");
        assert!(encoded.contains("endpoint-navidrome-primary"));
        assert!(encoded.contains("media/subsonic/readonly"));
        assert!(encoded.contains("\"launch\""));
        for forbidden in ["https://", "http://", "curl ", "password=", "secret-value"] {
            assert!(!encoded.contains(forbidden), "found forbidden {forbidden}");
        }

        let decoded: SubsonicResourceCard = serde_json::from_str(&encoded).expect("decode card");
        assert_eq!(decoded, card);
    }

    #[test]
    fn required_auth_is_visible_but_unavailable_and_not_launchable() {
        let mut card = launchable_card();
        card.auth = SubsonicAuthState::Required;
        card.capabilities = vec![SubsonicCapability::Browse, SubsonicCapability::Stream];
        card.actions = vec![SubsonicAction::Inspect, SubsonicAction::Authenticate];
        card.admission = SubsonicAdmissionState::Unavailable {
            reason: SubsonicUnavailableReason::AuthenticationRequired,
        };

        assert!(card.validate().is_ok());
        assert!(!card.is_launchable());
    }

    #[test]
    fn unavailable_cards_cannot_smuggle_a_launch_action() {
        let mut card = launchable_card();
        card.admission = SubsonicAdmissionState::Unavailable {
            reason: SubsonicUnavailableReason::ProviderUnreachable,
        };

        assert_eq!(
            card.validate(),
            Err(SubsonicValidationError::InvalidRelationship(
                "actions.launch"
            ))
        );
        assert!(!card.is_launchable());
    }

    #[test]
    fn typed_actions_require_their_declared_capability() {
        let mut card = launchable_card();
        card.capabilities
            .retain(|capability| !matches!(capability, SubsonicCapability::Stream));
        assert_eq!(
            card.validate(),
            Err(SubsonicValidationError::InvalidRelationship(
                "action.capability"
            ))
        );
    }

    #[test]
    fn opaque_references_reject_locators_commands_and_secret_shapes() {
        for value in [
            "https://music.example.test",
            "http://provider",
            "../../provider",
            "bash -c provider",
            "provider;rm",
            "token-plaintext",
        ] {
            assert!(
                SubsonicEndpointRef::new(value).is_err(),
                "accepted unsafe endpoint ref {value}"
            );
        }
        for value in [
            "media/token=value",
            "media/password=plaintext",
            "https://provider/token",
        ] {
            assert!(
                SubsonicCredentialRef::new(value).is_err(),
                "accepted unsafe credential ref {value}"
            );
        }
    }

    #[test]
    fn duplicate_or_unknown_wire_values_fail_closed() {
        let mut duplicate = launchable_card();
        duplicate.capabilities.push(SubsonicCapability::Browse);
        assert_eq!(
            duplicate.validate(),
            Err(SubsonicValidationError::Duplicate("capabilities"))
        );

        let mut wire = serde_json::to_value(launchable_card()).expect("serialize card");
        wire["actions"] = serde_json::json!(["inspect", "execute"]);
        assert!(serde_json::from_value::<SubsonicResourceCard>(wire).is_err());
    }

    #[test]
    fn required_auth_must_offer_the_typed_auth_action() {
        let mut card = launchable_card();
        card.auth = SubsonicAuthState::Required;
        card.actions = vec![SubsonicAction::Inspect];
        card.admission = SubsonicAdmissionState::Unavailable {
            reason: SubsonicUnavailableReason::AuthenticationRequired,
        };
        assert_eq!(
            card.validate(),
            Err(SubsonicValidationError::InvalidRelationship(
                "auth.authenticate_action"
            ))
        );
    }
}
