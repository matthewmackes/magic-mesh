//! Versioned, fail-closed catalog records for guest-owned Flatpak apps.
//!
//! The catalog is data, not a launcher: no field in this contract is an
//! executable, mount point, environment, or host socket. Consumers must
//! validate the catalog before projecting it into Front Door or creating an
//! [`crate::vdi_session::AppVmLaunchRequest`].

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

/// The only catalog schema currently admitted by the App VM path.
pub const FLATPAK_CATALOG_SCHEMA_VERSION: u16 = 1;
/// Schema admitted for runtime Flatpak catalogs.
pub const FLATPAK_RUNTIME_CATALOG_SCHEMA_VERSION: u16 = 1;
const MAX_ID_BYTES: usize = 255;
const MAX_TEXT_BYTES: usize = 1024;
const MAX_CATALOG_ENTRIES: usize = 512;
const MAX_LIST_ITEMS: usize = 32;
const MAX_SEARCH_TERMS: usize = 24;
const MAX_SEARCH_TERM_BYTES: usize = 96;
const MAX_SEARCH_QUERY_BYTES: usize = 256;
const MAX_VERSION_BYTES: usize = 128;
const MAX_RUNTIME_CATALOG_TTL_MS: u64 = 24 * 60 * 60 * 1_000;
const MAX_SEARCH_WEIGHT: u16 = 1_000;
const MAX_RUNTIME_CATALOG_WIRE_BYTES: usize = 512 * 1024;

/// A versioned set of curated guest applications.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlatpakAppCatalog {
    /// Schema discriminator for deterministic consumer behavior.
    pub schema_version: u16,
    /// Monotonic catalog revision selected by the provider.
    pub revision: String,
    /// Catalog rows, with unique app IDs after validation.
    pub entries: Vec<FlatpakCatalogEntry>,
}

/// One catalog row. The row contains only an identity and approved policy
/// metadata; guest provisioning resolves the profile through its own allowlist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlatpakCatalogEntry {
    /// Stable reverse-DNS Flatpak identity.
    pub app_id: String,
    /// User-facing application name.
    pub display_name: String,
    /// Bounded user-facing summary.
    pub summary: String,
    /// Non-executable icon reference resolved by the guest/UI catalog.
    pub icon_reference: String,
    /// Approved source revision for this app.
    pub source_revision: String,
    /// Capabilities admitted by the guest profile policy.
    pub declared_capabilities: Vec<String>,
    /// Named guest profile, never an image path or command.
    pub guest_profile: String,
    /// Actions exposed by the curated guest declaration.
    pub supported_actions: Vec<String>,
    /// Source provenance.
    pub provenance: FlatpakCatalogProvenance,
    /// Explicit install/readiness state.
    pub state: FlatpakInstallState,
}

/// Provenance needed before a catalog row can become launchable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlatpakCatalogProvenance {
    /// Curated provider or repository identity.
    pub source: String,
}

/// Installation/readiness is explicit so missing or stale content is never a
/// launchable-looking Front Door result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlatpakInstallState {
    /// Guest content is installed and may be launchable.
    Installed,
    /// Catalog metadata exists but guest content is not installed.
    Available,
    /// Installed content no longer matches the admitted catalog revision.
    Stale,
    /// The guest provider cannot currently supply the app.
    Unavailable,
}

/// Why catalog validation rejected an untrusted record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlatpakCatalogError {
    /// The consumer does not implement this schema version.
    UnsupportedSchema(u16),
    /// A bounded identity or text field is blank or contains controls.
    InvalidField(&'static str),
    /// A bounded field exceeds its wire limit.
    FieldTooLong(&'static str),
    /// The app ID is not a reverse-DNS identity.
    InvalidAppId,
    /// A capability/action list contains an unsafe or repeated value.
    InvalidListValue(&'static str),
    /// Two catalog rows claim the same stable app ID.
    DuplicateAppId,
    /// The selected App VM profile does not implement this capability safely.
    UnsupportedCapability(String),
    /// The catalog contains more rows than an importer may safely process.
    TooManyEntries,
}

impl FlatpakAppCatalog {
    /// Validate the complete catalog before it crosses a provider boundary.
    ///
    /// # Errors
    ///
    /// Returns an error when the schema, bounded fields, entry count, entry
    /// contents, or app-ID uniqueness constraint is invalid.
    pub fn validate(&self) -> Result<(), FlatpakCatalogError> {
        if self.schema_version != FLATPAK_CATALOG_SCHEMA_VERSION {
            return Err(FlatpakCatalogError::UnsupportedSchema(self.schema_version));
        }
        validate_text("revision", &self.revision, MAX_ID_BYTES)?;
        if self.entries.len() > MAX_CATALOG_ENTRIES {
            return Err(FlatpakCatalogError::TooManyEntries);
        }
        let mut app_ids = BTreeSet::new();
        for entry in &self.entries {
            entry.validate()?;
            if !app_ids.insert(&entry.app_id) {
                return Err(FlatpakCatalogError::DuplicateAppId);
            }
        }
        Ok(())
    }

    /// Return a deterministic, validated catalog from untrusted input.
    ///
    /// # Errors
    ///
    /// Returns the validation error produced by [`Self::validate`].
    pub fn admitted(self) -> Result<Self, FlatpakCatalogError> {
        self.validate()?;
        Ok(self)
    }
}

impl FlatpakCatalogEntry {
    fn validate(&self) -> Result<(), FlatpakCatalogError> {
        if !is_flatpak_app_id(&self.app_id) {
            return Err(FlatpakCatalogError::InvalidAppId);
        }
        validate_text("display_name", &self.display_name, MAX_TEXT_BYTES)?;
        validate_text("summary", &self.summary, MAX_TEXT_BYTES)?;
        validate_text("icon_reference", &self.icon_reference, MAX_ID_BYTES)?;
        validate_text("source_revision", &self.source_revision, MAX_ID_BYTES)?;
        validate_text("guest_profile", &self.guest_profile, MAX_ID_BYTES)?;
        validate_list(&self.declared_capabilities, "declared_capabilities")?;
        for capability in &self.declared_capabilities {
            if !crate::cloud::APP_VM_ALLOWED_CAPABILITIES.contains(&capability.as_str()) {
                return Err(FlatpakCatalogError::UnsupportedCapability(
                    capability.clone(),
                ));
            }
        }
        validate_list(&self.supported_actions, "supported_actions")?;
        validate_text("provenance.source", &self.provenance.source, MAX_ID_BYTES)?;
        Ok(())
    }

    /// Only installed rows that explicitly grant the typed launch
    /// action can be handed to the App VM launch layer.
    #[must_use]
    pub fn is_launchable(&self) -> bool {
        self.validate().is_ok()
            && self.state == FlatpakInstallState::Installed
            && self
                .supported_actions
                .iter()
                .any(|action| action.eq_ignore_ascii_case("launch"))
    }
}

fn validate_text(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), FlatpakCatalogError> {
    if value.trim().is_empty()
        || value.chars().any(char::is_control)
        || value.contains('/')
        || value.contains('\\')
    {
        return Err(FlatpakCatalogError::InvalidField(field));
    }
    if value.len() > max_bytes {
        return Err(FlatpakCatalogError::FieldTooLong(field));
    }
    Ok(())
}

fn validate_list(values: &[String], field: &'static str) -> Result<(), FlatpakCatalogError> {
    if values.len() > MAX_LIST_ITEMS {
        return Err(FlatpakCatalogError::InvalidListValue(field));
    }
    let mut seen = BTreeSet::new();
    for value in values {
        if value.trim().is_empty()
            || value
                .chars()
                .any(|character| character.is_control() || character.is_whitespace())
            || value.contains('/')
            || value.contains('\\')
            || value.len() > MAX_ID_BYTES
            || !seen.insert(value)
        {
            return Err(FlatpakCatalogError::InvalidListValue(field));
        }
    }
    Ok(())
}

/// Stable source identities bound into a catalog document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlatpakCatalogOrigin {
    /// Stable provider identity selected by policy, never a URL or path.
    pub provider_id: String,
    /// Stable repository identity selected by policy, never a URL or path.
    pub repository_id: String,
}

/// Canonical inputs used by consumers to rank matching catalog rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlatpakSearchMetadata {
    /// Lowercase, sorted, unique terms; consumers must not derive hidden inputs.
    pub terms: Vec<String>,
    /// Provider-selected tie-break weight in the inclusive range 0..=1000.
    pub weight: u16,
}

/// One deterministic result from a validated runtime catalog search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlatpakSearchMatch<'a> {
    /// Admitted catalog row; no metadata is synthesized by search.
    pub entry: &'a FlatpakCatalogItem,
    /// Stable score derived only from validated fields and the normalized query.
    pub score: u32,
}

/// One complete row in the runtime catalog. All launch-relevant metadata is
/// structurally validated rather than supplied by a UI launcher.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlatpakCatalogItem {
    /// Stable reverse-DNS Flatpak application identity.
    pub app_id: String,
    /// Bounded user-facing application name.
    pub display_name: String,
    /// Bounded user-facing summary.
    pub summary: String,
    /// Stable package version/revision, not a package locator.
    pub version: String,
    /// Stable icon identity resolved from an admitted local icon catalog.
    pub icon_id: String,
    /// Sorted, unique permissions admitted by the App VM policy.
    pub permissions: Vec<String>,
    /// Stable App VM profile identity, never an image or executable path.
    pub guest_profile: String,
    /// Sorted, unique typed actions exposed by this row.
    pub supported_actions: Vec<String>,
    /// Explicit deterministic ranking inputs.
    pub search: FlatpakSearchMetadata,
    /// Explicit install/readiness state.
    pub state: FlatpakInstallState,
}

/// Canonical runtime catalog document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlatpakCatalogDocument {
    /// Runtime-catalog schema discriminator.
    pub schema_version: u16,
    /// Stable catalog identity.
    pub catalog_id: String,
    /// Positive monotonic provider revision.
    pub revision: u64,
    /// Publication time in Unix epoch milliseconds.
    pub issued_at_unix_ms: u64,
    /// Freshness expiry time in Unix epoch milliseconds.
    pub expires_at_unix_ms: u64,
    /// Stable provider/repository provenance.
    pub origin: FlatpakCatalogOrigin,
    /// Canonically ordered application rows.
    pub entries: Vec<FlatpakCatalogItem>,
}

/// Content-addressed Flatpak catalog admitted by the runtime importer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlatpakRuntimeCatalog {
    /// Canonical catalog payload.
    pub payload: FlatpakCatalogDocument,
}

/// Why a Flatpak runtime catalog failed validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlatpakRuntimeCatalogError {
    /// The consumer does not implement the payload schema.
    UnsupportedSchema(u16),
    /// A required identity or text field is malformed or unsafe.
    InvalidField(&'static str),
    /// A bounded field exceeds its byte limit.
    FieldTooLong(&'static str),
    /// Issue/expiry values are zero, reversed, or exceed the maximum TTL.
    InvalidValidityWindow,
    /// The catalog issue time is later than the admission time.
    NotYetValid,
    /// The catalog has reached or passed its expiry time.
    StaleCatalog,
    /// Two rows claim the same stable application identity.
    DuplicateAppId,
    /// A canonical list is not strictly sorted and unique.
    NonCanonicalOrder(&'static str),
    /// A collection exceeds its item limit.
    ResourceLimitExceeded(&'static str),
    /// The App VM profile cannot safely provide a requested permission.
    UnsupportedPermission(String),
    /// The untrusted JSON body exceeds the pre-parse allocation bound.
    WirePayloadTooLarge,
    /// The untrusted JSON body is malformed, duplicated, or otherwise closed.
    MalformedJson,
    /// The validated payload could not be serialized canonically.
    CanonicalEncodingFailed,
}

impl FlatpakCatalogDocument {
    /// Validate intrinsic bounds and canonical ordering.
    ///
    /// # Errors
    ///
    /// Returns an error when the schema, validity window, bounded fields,
    /// entry contents, or canonical ordering is invalid.
    pub fn validate(&self) -> Result<(), FlatpakRuntimeCatalogError> {
        if self.schema_version != FLATPAK_RUNTIME_CATALOG_SCHEMA_VERSION {
            return Err(FlatpakRuntimeCatalogError::UnsupportedSchema(
                self.schema_version,
            ));
        }
        validate_stable_identity("catalog_id", &self.catalog_id, MAX_ID_BYTES)?;
        if self.revision == 0
            || self.issued_at_unix_ms == 0
            || self.expires_at_unix_ms <= self.issued_at_unix_ms
            || self.expires_at_unix_ms - self.issued_at_unix_ms > MAX_RUNTIME_CATALOG_TTL_MS
        {
            return Err(FlatpakRuntimeCatalogError::InvalidValidityWindow);
        }
        validate_stable_identity("origin.provider_id", &self.origin.provider_id, MAX_ID_BYTES)?;
        validate_stable_identity(
            "origin.repository_id",
            &self.origin.repository_id,
            MAX_ID_BYTES,
        )?;
        if self.entries.len() > MAX_CATALOG_ENTRIES {
            return Err(FlatpakRuntimeCatalogError::ResourceLimitExceeded("entries"));
        }

        let mut previous_app_id: Option<&str> = None;
        for entry in &self.entries {
            entry.validate()?;
            if previous_app_id.is_some_and(|previous| previous >= entry.app_id.as_str()) {
                return Err(if previous_app_id == Some(entry.app_id.as_str()) {
                    FlatpakRuntimeCatalogError::DuplicateAppId
                } else {
                    FlatpakRuntimeCatalogError::NonCanonicalOrder("entries")
                });
            }
            previous_app_id = Some(&entry.app_id);
        }
        Ok(())
    }

    fn canonical_bytes(&self) -> Result<Vec<u8>, FlatpakRuntimeCatalogError> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|_| FlatpakRuntimeCatalogError::CanonicalEncodingFailed)
    }

    /// Stable lowercase SHA-256 address of the canonical catalog content.
    ///
    /// # Errors
    ///
    /// Returns an error when the payload fails validation or cannot be
    /// serialized into its canonical representation.
    pub fn content_digest(&self) -> Result<String, FlatpakRuntimeCatalogError> {
        Ok(format!(
            "sha256:{}",
            encode_hex(&Sha256::digest(self.canonical_bytes()?))
        ))
    }

    /// Search validated rows using only deterministic ranking inputs.
    /// Match class sorts before provider weight; stable app identity breaks ties.
    ///
    /// # Errors
    ///
    /// Returns an error when the catalog is invalid or the query is blank,
    /// malformed, or exceeds its bounded search-input form.
    pub fn search(
        &self,
        query: &str,
    ) -> Result<Vec<FlatpakSearchMatch<'_>>, FlatpakRuntimeCatalogError> {
        self.validate()?;
        let query = normalize_search_query(query)?;
        let query_terms: Vec<&str> = query.split_whitespace().collect();
        let mut matches = Vec::new();

        for entry in &self.entries {
            let app_id = entry.app_id.to_ascii_lowercase();
            let display_name = entry.display_name.to_ascii_lowercase();
            let summary = entry.summary.to_ascii_lowercase();
            let searchable_term = |needle: &str| {
                app_id.contains(needle)
                    || display_name.contains(needle)
                    || summary.contains(needle)
                    || entry.search.terms.iter().any(|term| term.contains(needle))
            };
            let match_class = if app_id == query {
                5
            } else if display_name == query {
                4
            } else if app_id.starts_with(&query) || display_name.starts_with(&query) {
                3
            } else if query_terms.iter().all(|term| searchable_term(term)) {
                2
            } else {
                u32::from(searchable_term(&query))
            };
            if match_class != 0 {
                matches.push(FlatpakSearchMatch {
                    entry,
                    score: match_class * 10_000 + u32::from(entry.search.weight),
                });
            }
        }

        matches.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| left.entry.app_id.cmp(&right.entry.app_id))
        });
        Ok(matches)
    }
}

impl FlatpakCatalogItem {
    fn validate(&self) -> Result<(), FlatpakRuntimeCatalogError> {
        if !is_flatpak_app_id(&self.app_id) {
            return Err(FlatpakRuntimeCatalogError::InvalidField("app_id"));
        }
        validate_display_text("display_name", &self.display_name, MAX_TEXT_BYTES)?;
        validate_display_text("summary", &self.summary, MAX_TEXT_BYTES)?;
        validate_stable_identity("version", &self.version, MAX_VERSION_BYTES)?;
        validate_stable_identity("icon_id", &self.icon_id, MAX_ID_BYTES)?;
        validate_stable_identity("guest_profile", &self.guest_profile, MAX_ID_BYTES)?;
        validate_canonical_list(
            &self.permissions,
            "permissions",
            MAX_LIST_ITEMS,
            MAX_ID_BYTES,
        )?;
        for permission in &self.permissions {
            if !crate::cloud::APP_VM_ALLOWED_CAPABILITIES.contains(&permission.as_str()) {
                return Err(FlatpakRuntimeCatalogError::UnsupportedPermission(
                    permission.clone(),
                ));
            }
        }
        validate_canonical_list(
            &self.supported_actions,
            "supported_actions",
            MAX_LIST_ITEMS,
            MAX_ID_BYTES,
        )?;
        validate_canonical_list(
            &self.search.terms,
            "search.terms",
            MAX_SEARCH_TERMS,
            MAX_SEARCH_TERM_BYTES,
        )?;
        if self
            .search
            .terms
            .iter()
            .any(|term| term != &term.to_ascii_lowercase())
        {
            return Err(FlatpakRuntimeCatalogError::InvalidField("search.terms"));
        }
        if self.search.weight > MAX_SEARCH_WEIGHT {
            return Err(FlatpakRuntimeCatalogError::InvalidField("search.weight"));
        }
        Ok(())
    }
}

impl FlatpakRuntimeCatalog {
    /// Validate freshness, bounds, and canonicality.
    ///
    /// # Errors
    ///
    /// Returns an error when freshness, bounds, or canonicality validation fails.
    pub fn admit(self, now_unix_ms: u64) -> Result<Self, FlatpakRuntimeCatalogError> {
        self.payload.validate()?;
        if now_unix_ms < self.payload.issued_at_unix_ms {
            return Err(FlatpakRuntimeCatalogError::NotYetValid);
        }
        if now_unix_ms >= self.payload.expires_at_unix_ms {
            return Err(FlatpakRuntimeCatalogError::StaleCatalog);
        }
        Ok(self)
    }

    /// Bound untrusted input before parsing, then perform complete admission.
    ///
    /// # Errors
    ///
    /// Returns an error when the body exceeds the wire limit, is malformed, or
    /// fails catalog admission.
    pub fn decode_and_admit_json(
        body: &[u8],
        now_unix_ms: u64,
    ) -> Result<Self, FlatpakRuntimeCatalogError> {
        if body.len() > MAX_RUNTIME_CATALOG_WIRE_BYTES {
            return Err(FlatpakRuntimeCatalogError::WirePayloadTooLarge);
        }
        let catalog: Self =
            serde_json::from_slice(body).map_err(|_| FlatpakRuntimeCatalogError::MalformedJson)?;
        catalog.admit(now_unix_ms)
    }
}

fn validate_display_text(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), FlatpakRuntimeCatalogError> {
    if value.trim().is_empty()
        || value.trim() != value
        || value.chars().any(char::is_control)
        || contains_secret_material(value)
    {
        return Err(FlatpakRuntimeCatalogError::InvalidField(field));
    }
    if value.len() > max_bytes {
        return Err(FlatpakRuntimeCatalogError::FieldTooLong(field));
    }
    Ok(())
}

fn normalize_search_query(query: &str) -> Result<String, FlatpakRuntimeCatalogError> {
    if query.len() > MAX_SEARCH_QUERY_BYTES {
        return Err(FlatpakRuntimeCatalogError::FieldTooLong("search_query"));
    }
    if query.chars().any(char::is_control) || contains_secret_material(query) {
        return Err(FlatpakRuntimeCatalogError::InvalidField("search_query"));
    }
    let normalized = query
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    if normalized.is_empty() {
        return Err(FlatpakRuntimeCatalogError::InvalidField("search_query"));
    }
    Ok(normalized)
}

fn validate_stable_identity(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), FlatpakRuntimeCatalogError> {
    if value.trim().is_empty()
        || value.trim() != value
        || value.chars().any(char::is_control)
        || value.chars().any(char::is_whitespace)
        || value.contains('/')
        || value.contains('\\')
        || value.contains("://")
        || value.to_ascii_lowercase().starts_with("file:")
        || contains_secret_material(value)
    {
        return Err(FlatpakRuntimeCatalogError::InvalidField(field));
    }
    if value.len() > max_bytes {
        return Err(FlatpakRuntimeCatalogError::FieldTooLong(field));
    }
    Ok(())
}

fn validate_canonical_list(
    values: &[String],
    field: &'static str,
    max_items: usize,
    max_bytes: usize,
) -> Result<(), FlatpakRuntimeCatalogError> {
    if values.len() > max_items {
        return Err(FlatpakRuntimeCatalogError::ResourceLimitExceeded(field));
    }
    let mut previous: Option<&str> = None;
    for value in values {
        validate_stable_identity(field, value, max_bytes)?;
        if previous.is_some_and(|prior| prior >= value.as_str()) {
            return Err(FlatpakRuntimeCatalogError::NonCanonicalOrder(field));
        }
        previous = Some(value);
    }
    Ok(())
}

fn contains_secret_material(value: &str) -> bool {
    let lowercase = value.to_ascii_lowercase();
    [
        "-----begin private key",
        "-----begin openssh private key",
        "password=",
        "passwd=",
        "token=",
        "secret=",
        "authorization: bearer",
        "private_key",
    ]
    .iter()
    .any(|marker| lowercase.contains(marker))
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn is_flatpak_app_id(value: &str) -> bool {
    if value.len() > MAX_ID_BYTES || value.trim() != value {
        return false;
    }
    let mut components = value.split('.');
    let mut count = 0;
    components.all(|component| {
        count += 1;
        !component.is_empty()
            && component.len() <= 63
            && component.chars().all(|character| {
                character.is_ascii_alphanumeric() || character == '_' || character == '-'
            })
            && component
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_alphabetic() || character == '_')
    }) && count >= 2
}

/// Validate one reverse-DNS Flatpak identity at a launch boundary without
/// requiring a complete catalog row.
#[must_use]
pub fn is_valid_flatpak_app_id(value: &str) -> bool {
    is_flatpak_app_id(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000_000;

    fn entry(app_id: &str) -> FlatpakCatalogEntry {
        FlatpakCatalogEntry {
            app_id: app_id.into(),
            display_name: "Editor".into(),
            summary: "A guest-owned editor".into(),
            icon_reference: "icon:editor".into(),
            source_revision: "flathub:42".into(),
            declared_capabilities: vec!["audio".into(), "clipboard".into()],
            guest_profile: "wayland-standard".into(),
            supported_actions: vec!["launch".into(), "resume".into()],
            provenance: FlatpakCatalogProvenance {
                source: "curated".into(),
            },
            state: FlatpakInstallState::Installed,
        }
    }

    fn runtime_catalog_item(app_id: &str) -> FlatpakCatalogItem {
        FlatpakCatalogItem {
            app_id: app_id.into(),
            display_name: "Editor".into(),
            summary: "A guest-owned editor".into(),
            version: "42.1".into(),
            icon_id: "icon:editor".into(),
            permissions: vec!["audio".into(), "clipboard".into()],
            guest_profile: "wayland-standard-v1".into(),
            supported_actions: vec!["launch".into(), "resume".into()],
            search: FlatpakSearchMetadata {
                terms: vec!["edit".into(), "editor".into(), "text".into()],
                weight: 500,
            },
            state: FlatpakInstallState::Installed,
        }
    }

    fn runtime_catalog_document() -> FlatpakCatalogDocument {
        FlatpakCatalogDocument {
            schema_version: FLATPAK_RUNTIME_CATALOG_SCHEMA_VERSION,
            catalog_id: "flatpak-curated".into(),
            revision: 42,
            issued_at_unix_ms: NOW - 1_000,
            expires_at_unix_ms: NOW + 60_000,
            origin: FlatpakCatalogOrigin {
                provider_id: "mcnf-curated".into(),
                repository_id: "flathub-stable".into(),
            },
            entries: vec![
                runtime_catalog_item("org.example.Editor"),
                runtime_catalog_item("org.example.Terminal"),
            ],
        }
    }

    #[test]
    fn catalog_admits_unique_installed_rows() {
        let catalog = FlatpakAppCatalog {
            schema_version: FLATPAK_CATALOG_SCHEMA_VERSION,
            revision: "catalog-42".into(),
            entries: vec![entry("org.example.Editor"), entry("org.example.Terminal")],
        };
        assert!(catalog.clone().admitted().is_ok());
        assert!(catalog.entries[0].is_launchable());
        let body = serde_json::to_string(&catalog).expect("catalog JSON");
        assert!(body.contains("guest_profile"));
        assert!(!body.contains("command"));
    }

    #[test]
    fn catalog_rejects_duplicate_or_malformed_rows() {
        let duplicate = FlatpakAppCatalog {
            schema_version: FLATPAK_CATALOG_SCHEMA_VERSION,
            revision: "catalog-42".into(),
            entries: vec![entry("org.example.Editor"), entry("org.example.Editor")],
        };
        assert_eq!(
            duplicate.admitted(),
            Err(FlatpakCatalogError::DuplicateAppId)
        );

        let mut malformed = entry("org.example.Editor");
        malformed.guest_profile = "/tmp/image".into();
        assert_eq!(
            FlatpakAppCatalog {
                schema_version: FLATPAK_CATALOG_SCHEMA_VERSION,
                revision: "catalog-42".into(),
                entries: vec![malformed],
            }
            .admitted(),
            Err(FlatpakCatalogError::InvalidField("guest_profile"))
        );
    }

    #[test]
    fn catalog_rejects_capabilities_the_app_vm_profile_cannot_serve() {
        let mut unsupported = entry("org.example.Editor");
        unsupported.declared_capabilities = vec!["gpu".into()];
        assert_eq!(
            FlatpakAppCatalog {
                schema_version: FLATPAK_CATALOG_SCHEMA_VERSION,
                revision: "catalog-42".into(),
                entries: vec![unsupported],
            }
            .admitted(),
            Err(FlatpakCatalogError::UnsupportedCapability("gpu".into()))
        );
    }

    #[test]
    fn uninstalled_rows_are_not_launchable() {
        let mut row = entry("org.example.Editor");
        row.state = FlatpakInstallState::Stale;
        assert!(!row.is_launchable());
    }

    #[test]
    fn hostile_catalog_row_without_launch_action_cannot_authorize_app_vm_launch() {
        let mut row = entry("org.example.Editor");
        row.supported_actions = vec!["resume".into()];

        assert!(row.validate().is_ok(), "the row remains discoverable");
        assert!(
            !row.is_launchable(),
            "installed metadata without a launch action is not implicit launch authority"
        );
    }

    #[test]
    fn malformed_installed_rows_are_not_launchable_before_catalog_projection() {
        let mut row = entry("org.example.Editor");
        row.guest_profile = "/tmp/image".into();
        assert!(!row.is_launchable());

        row.guest_profile = "wayland-standard".into();
        row.declared_capabilities = vec!["gpu".into()];
        assert!(!row.is_launchable());
    }

    #[test]
    fn runtime_catalog_admits_fresh_validated_content() {
        let catalog = FlatpakRuntimeCatalog {
            payload: runtime_catalog_document(),
        };
        assert!(catalog
            .payload
            .content_digest()
            .unwrap()
            .starts_with("sha256:"));
        assert!(catalog.admit(NOW).is_ok());
    }

    #[test]
    fn runtime_catalog_rejects_invalid_freshness_and_addresses_content_changes() {
        let catalog = FlatpakRuntimeCatalog {
            payload: runtime_catalog_document(),
        };
        assert_eq!(
            catalog.clone().admit(catalog.payload.issued_at_unix_ms - 1),
            Err(FlatpakRuntimeCatalogError::NotYetValid)
        );
        assert_eq!(
            catalog.clone().admit(catalog.payload.expires_at_unix_ms),
            Err(FlatpakRuntimeCatalogError::StaleCatalog)
        );

        let original_digest = catalog.payload.content_digest().unwrap();
        let mut tampered = catalog;
        tampered.payload.entries[0].display_name = "Tampered".into();
        assert!(tampered.clone().admit(NOW).is_ok());
        assert_ne!(tampered.payload.content_digest().unwrap(), original_digest);
    }

    #[test]
    fn runtime_catalog_rejects_duplicate_noncanonical_and_oversized_metadata() {
        let mut duplicate = runtime_catalog_document();
        duplicate.entries[1].app_id = duplicate.entries[0].app_id.clone();
        assert_eq!(
            duplicate.validate(),
            Err(FlatpakRuntimeCatalogError::DuplicateAppId)
        );

        let mut unordered = runtime_catalog_document();
        unordered.entries.swap(0, 1);
        assert_eq!(
            unordered.validate(),
            Err(FlatpakRuntimeCatalogError::NonCanonicalOrder("entries"))
        );

        let mut oversized = runtime_catalog_document();
        oversized.entries[0].search.terms = (0..=MAX_SEARCH_TERMS)
            .map(|index| format!("term-{index:02}"))
            .collect();
        assert_eq!(
            oversized.validate(),
            Err(FlatpakRuntimeCatalogError::ResourceLimitExceeded(
                "search.terms"
            ))
        );

        let mut unstable_ranking = runtime_catalog_document();
        unstable_ranking.entries[0].search.terms = vec!["text".into(), "editor".into()];
        assert_eq!(
            unstable_ranking.validate(),
            Err(FlatpakRuntimeCatalogError::NonCanonicalOrder(
                "search.terms"
            ))
        );
    }

    #[test]
    fn runtime_catalog_rejects_locators_secrets_and_unsupported_permissions() {
        for (field, poison) in [
            ("version", "https://repo.invalid/app"),
            ("icon_id", "/var/lib/icons/editor.svg"),
            ("guest_profile", "file:///etc/passwd"),
            ("version", "token=super-secret"),
        ] {
            let mut payload = runtime_catalog_document();
            match field {
                "version" => payload.entries[0].version = poison.into(),
                "icon_id" => payload.entries[0].icon_id = poison.into(),
                "guest_profile" => payload.entries[0].guest_profile = poison.into(),
                _ => unreachable!(),
            }
            assert_eq!(
                payload.validate(),
                Err(FlatpakRuntimeCatalogError::InvalidField(field))
            );
        }

        let mut unsupported = runtime_catalog_document();
        unsupported.entries[0].permissions = vec!["host-filesystem".into()];
        assert_eq!(
            unsupported.validate(),
            Err(FlatpakRuntimeCatalogError::UnsupportedPermission(
                "host-filesystem".into()
            ))
        );
    }

    #[test]
    fn runtime_catalog_json_refuses_unknown_fields_and_schema_skew() {
        let mut value = serde_json::to_value(runtime_catalog_document()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("download_url".into(), serde_json::json!("https://invalid"));
        assert!(serde_json::from_value::<FlatpakCatalogDocument>(value).is_err());

        let mut skewed = runtime_catalog_document();
        skewed.schema_version += 1;
        assert_eq!(
            skewed.validate(),
            Err(FlatpakRuntimeCatalogError::UnsupportedSchema(2))
        );

        let legacy_unknown = r#"{"schema_version":1,"revision":"r","entries":[],"extra":true}"#;
        assert!(serde_json::from_str::<FlatpakAppCatalog>(legacy_unknown).is_err());
    }

    #[test]
    fn runtime_catalog_bounds_json_before_parsing_and_rejects_duplicate_keys() {
        let catalog = FlatpakRuntimeCatalog {
            payload: runtime_catalog_document(),
        };
        let body = serde_json::to_vec(&catalog).unwrap();
        assert!(FlatpakRuntimeCatalog::decode_and_admit_json(&body, NOW).is_ok());

        let oversized = vec![b' '; MAX_RUNTIME_CATALOG_WIRE_BYTES + 1];
        assert_eq!(
            FlatpakRuntimeCatalog::decode_and_admit_json(&oversized, NOW),
            Err(FlatpakRuntimeCatalogError::WirePayloadTooLarge)
        );

        let duplicate = br#"{"payload":{},"payload":{}}"#;
        assert_eq!(
            FlatpakRuntimeCatalog::decode_and_admit_json(duplicate, NOW),
            Err(FlatpakRuntimeCatalogError::MalformedJson)
        );
    }

    #[test]
    fn runtime_catalog_search_is_validated_ranked_and_stably_tied() {
        let mut payload = runtime_catalog_document();
        let tied = payload.search("  EDITOR  ").expect("normalized query");
        assert_eq!(tied.len(), 2);
        assert_eq!(tied[0].score, tied[1].score);
        assert_eq!(tied[0].entry.app_id, "org.example.Editor");
        assert_eq!(tied[1].entry.app_id, "org.example.Terminal");

        payload.entries[1].search.weight = 900;
        let weighted = payload.search("editor").expect("weighted query");
        assert_eq!(weighted[0].entry.app_id, "org.example.Terminal");
        assert!(weighted[0].score > weighted[1].score);

        assert_eq!(
            payload.search("token=do-not-search"),
            Err(FlatpakRuntimeCatalogError::InvalidField("search_query"))
        );
        assert_eq!(
            payload.search(&"q".repeat(MAX_SEARCH_QUERY_BYTES + 1)),
            Err(FlatpakRuntimeCatalogError::FieldTooLong("search_query"))
        );
    }
}
