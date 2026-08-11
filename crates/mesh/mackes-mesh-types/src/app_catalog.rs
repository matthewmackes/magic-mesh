//! Versioned, fail-closed catalog records for guest-owned Flatpak apps.
//!
//! The catalog is data, not a launcher: no field in this contract is an
//! executable, mount point, environment, or host socket. Consumers must
//! validate the catalog before projecting it into Front Door or creating an
//! [`crate::vdi_session::AppVmLaunchRequest`].

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

/// The only catalog schema currently admitted by the App VM path.
pub const FLATPAK_CATALOG_SCHEMA_VERSION: u16 = 1;
/// Schema admitted for cryptographically signed Flatpak catalogs.
pub const SIGNED_FLATPAK_CATALOG_SCHEMA_VERSION: u16 = 1;
const MAX_ID_BYTES: usize = 255;
const MAX_TEXT_BYTES: usize = 1024;
const MAX_CATALOG_ENTRIES: usize = 512;
const MAX_LIST_ITEMS: usize = 32;
const MAX_SEARCH_TERMS: usize = 24;
const MAX_SEARCH_TERM_BYTES: usize = 96;
const MAX_SEARCH_QUERY_BYTES: usize = 256;
const MAX_VERSION_BYTES: usize = 128;
const MAX_SIGNER_ID_BYTES: usize = 128;
const MAX_SIGNED_CATALOG_TTL_MS: u64 = 24 * 60 * 60 * 1_000;
const MAX_SEARCH_WEIGHT: u16 = 1_000;
const MAX_SIGNED_CATALOG_WIRE_BYTES: usize = 512 * 1024;
const FLATPAK_CATALOG_SIGNATURE_DOMAIN: &str = "magic-mesh/flatpak-app-catalog/v1";

/// A signed, versioned set of curated guest applications.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlatpakAppCatalog {
    /// Schema discriminator for deterministic consumer behavior.
    pub schema_version: u16,
    /// Monotonic catalog revision selected by the signed provider.
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
    /// Source and signature provenance.
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
    /// Detached signature or equivalent signed-evidence reference.
    pub signature: Option<String>,
}

/// Installation/readiness is explicit so missing or stale content is never a
/// launchable-looking Front Door result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlatpakInstallState {
    /// Guest content is installed and may be launchable if signed.
    Installed,
    /// Catalog metadata exists but guest content is not installed.
    Available,
    /// Installed content no longer matches the admitted catalog revision.
    Stale,
    /// The row lacks trusted provenance.
    Unsigned,
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
        if let Some(signature) = &self.provenance.signature {
            validate_text("provenance.signature", signature, MAX_TEXT_BYTES)?;
        }
        Ok(())
    }

    /// Only installed, signed rows that explicitly grant the typed launch
    /// action can be handed to the App VM launch layer.
    #[must_use]
    pub fn is_launchable(&self) -> bool {
        self.validate().is_ok()
            && self.state == FlatpakInstallState::Installed
            && self
                .supported_actions
                .iter()
                .any(|action| action.eq_ignore_ascii_case("launch"))
            && self
                .provenance
                .signature
                .as_deref()
                .is_some_and(|signature| !signature.trim().is_empty())
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

/// Stable source identities bound into a signed catalog payload.
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

/// One deterministic result from a validated signed catalog search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlatpakSearchMatch<'a> {
    /// Admitted catalog row; no metadata is synthesized by search.
    pub entry: &'a SignedFlatpakCatalogEntry,
    /// Stable score derived only from signed fields and the normalized query.
    pub score: u32,
}

/// One complete row in the signed catalog. All launch-relevant metadata is
/// covered by the envelope signature rather than supplied by a UI or importer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedFlatpakCatalogEntry {
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

/// Canonical unsigned payload covered by a Flatpak catalog signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedFlatpakCatalogPayload {
    /// Signed-catalog schema discriminator.
    pub schema_version: u16,
    /// Stable catalog identity.
    pub catalog_id: String,
    /// Positive monotonic provider revision.
    pub revision: u64,
    /// Signature issue time in Unix epoch milliseconds.
    pub issued_at_unix_ms: u64,
    /// Signature expiry time in Unix epoch milliseconds.
    pub expires_at_unix_ms: u64,
    /// Stable provider/repository provenance.
    pub origin: FlatpakCatalogOrigin,
    /// Canonically ordered application rows.
    pub entries: Vec<SignedFlatpakCatalogEntry>,
}

/// Ed25519-signed Flatpak catalog admitted by a future importer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedFlatpakAppCatalog {
    /// Stable key-rotation identity selected by local trust policy.
    pub signer_id: String,
    /// Canonical catalog payload.
    pub payload: SignedFlatpakCatalogPayload,
    /// Lowercase 64-byte Ed25519 signature encoded as 128 hex characters.
    pub signature: String,
}

/// Why a signed Flatpak catalog failed closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignedFlatpakCatalogError {
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
    /// The envelope signer does not exactly match local trust policy.
    UntrustedSigner,
    /// Two rows claim the same stable application identity.
    DuplicateAppId,
    /// A signed list is not strictly sorted and unique.
    NonCanonicalOrder(&'static str),
    /// A collection exceeds its item limit.
    ResourceLimitExceeded(&'static str),
    /// The App VM profile cannot safely provide a requested permission.
    UnsupportedPermission(String),
    /// Signature text is not exactly lowercase Ed25519 hex.
    MalformedSignature,
    /// Signature verification failed for the canonical payload.
    SignatureMismatch,
    /// The untrusted JSON body exceeds the pre-parse allocation bound.
    WirePayloadTooLarge,
    /// The untrusted JSON body is malformed, duplicated, or otherwise closed.
    MalformedJson,
    /// The validated payload could not be serialized for signing.
    CanonicalEncodingFailed,
}

impl SignedFlatpakCatalogPayload {
    /// Validate intrinsic bounds and canonical ordering before signing.
    pub fn validate(&self) -> Result<(), SignedFlatpakCatalogError> {
        if self.schema_version != SIGNED_FLATPAK_CATALOG_SCHEMA_VERSION {
            return Err(SignedFlatpakCatalogError::UnsupportedSchema(
                self.schema_version,
            ));
        }
        validate_stable_identity("catalog_id", &self.catalog_id, MAX_ID_BYTES)?;
        if self.revision == 0
            || self.issued_at_unix_ms == 0
            || self.expires_at_unix_ms <= self.issued_at_unix_ms
            || self.expires_at_unix_ms - self.issued_at_unix_ms > MAX_SIGNED_CATALOG_TTL_MS
        {
            return Err(SignedFlatpakCatalogError::InvalidValidityWindow);
        }
        validate_stable_identity("origin.provider_id", &self.origin.provider_id, MAX_ID_BYTES)?;
        validate_stable_identity(
            "origin.repository_id",
            &self.origin.repository_id,
            MAX_ID_BYTES,
        )?;
        if self.entries.len() > MAX_CATALOG_ENTRIES {
            return Err(SignedFlatpakCatalogError::ResourceLimitExceeded("entries"));
        }

        let mut previous_app_id: Option<&str> = None;
        for entry in &self.entries {
            entry.validate()?;
            if previous_app_id.is_some_and(|previous| previous >= entry.app_id.as_str()) {
                return Err(if previous_app_id == Some(entry.app_id.as_str()) {
                    SignedFlatpakCatalogError::DuplicateAppId
                } else {
                    SignedFlatpakCatalogError::NonCanonicalOrder("entries")
                });
            }
            previous_app_id = Some(&entry.app_id);
        }
        Ok(())
    }

    fn signing_bytes(&self) -> Result<Vec<u8>, SignedFlatpakCatalogError> {
        self.validate()?;
        let payload = serde_json::to_vec(self)
            .map_err(|_| SignedFlatpakCatalogError::CanonicalEncodingFailed)?;
        let mut signed =
            Vec::with_capacity(FLATPAK_CATALOG_SIGNATURE_DOMAIN.len() + payload.len() + 1);
        signed.extend_from_slice(FLATPAK_CATALOG_SIGNATURE_DOMAIN.as_bytes());
        signed.push(0);
        signed.extend_from_slice(&payload);
        Ok(signed)
    }

    /// Stable lowercase SHA-256 address of the canonical signed payload.
    pub fn content_digest(&self) -> Result<String, SignedFlatpakCatalogError> {
        Ok(format!(
            "sha256:{}",
            encode_hex(&Sha256::digest(self.signing_bytes()?))
        ))
    }

    /// Search validated rows using only signed deterministic ranking inputs.
    /// Match class sorts before provider weight; stable app identity breaks ties.
    pub fn search(
        &self,
        query: &str,
    ) -> Result<Vec<FlatpakSearchMatch<'_>>, SignedFlatpakCatalogError> {
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
            } else if searchable_term(&query) {
                1
            } else {
                0
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

impl SignedFlatpakCatalogEntry {
    fn validate(&self) -> Result<(), SignedFlatpakCatalogError> {
        if !is_flatpak_app_id(&self.app_id) {
            return Err(SignedFlatpakCatalogError::InvalidField("app_id"));
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
                return Err(SignedFlatpakCatalogError::UnsupportedPermission(
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
            return Err(SignedFlatpakCatalogError::InvalidField("search.terms"));
        }
        if self.search.weight > MAX_SEARCH_WEIGHT {
            return Err(SignedFlatpakCatalogError::InvalidField("search.weight"));
        }
        Ok(())
    }
}

impl SignedFlatpakAppCatalog {
    /// Sign an intrinsically valid payload with an offline/provider key.
    pub fn sign(
        signer_id: impl Into<String>,
        payload: SignedFlatpakCatalogPayload,
        signing_key: &SigningKey,
    ) -> Result<Self, SignedFlatpakCatalogError> {
        let signer_id = signer_id.into();
        validate_stable_identity("signer_id", &signer_id, MAX_SIGNER_ID_BYTES)?;
        let signature = signing_key.sign(&payload.signing_bytes()?);
        Ok(Self {
            signer_id,
            payload,
            signature: encode_hex(&signature.to_bytes()),
        })
    }

    /// Verify exact signer trust, freshness, signature, bounds, and canonicality.
    pub fn admit(
        self,
        trusted_signer_id: &str,
        verifying_key: &VerifyingKey,
        now_unix_ms: u64,
    ) -> Result<Self, SignedFlatpakCatalogError> {
        validate_stable_identity("signer_id", &self.signer_id, MAX_SIGNER_ID_BYTES)?;
        validate_stable_identity("trusted_signer_id", trusted_signer_id, MAX_SIGNER_ID_BYTES)?;
        if self.signer_id != trusted_signer_id {
            return Err(SignedFlatpakCatalogError::UntrustedSigner);
        }
        if now_unix_ms < self.payload.issued_at_unix_ms {
            return Err(SignedFlatpakCatalogError::NotYetValid);
        }
        if now_unix_ms >= self.payload.expires_at_unix_ms {
            return Err(SignedFlatpakCatalogError::StaleCatalog);
        }
        let signature_bytes =
            decode_hex_64(&self.signature).ok_or(SignedFlatpakCatalogError::MalformedSignature)?;
        verifying_key
            .verify(
                &self.payload.signing_bytes()?,
                &Signature::from_bytes(&signature_bytes),
            )
            .map_err(|_| SignedFlatpakCatalogError::SignatureMismatch)?;
        Ok(self)
    }

    /// Bound untrusted input before parsing, then perform complete admission.
    pub fn decode_and_admit_json(
        body: &[u8],
        trusted_signer_id: &str,
        verifying_key: &VerifyingKey,
        now_unix_ms: u64,
    ) -> Result<Self, SignedFlatpakCatalogError> {
        if body.len() > MAX_SIGNED_CATALOG_WIRE_BYTES {
            return Err(SignedFlatpakCatalogError::WirePayloadTooLarge);
        }
        let catalog: Self =
            serde_json::from_slice(body).map_err(|_| SignedFlatpakCatalogError::MalformedJson)?;
        catalog.admit(trusted_signer_id, verifying_key, now_unix_ms)
    }
}

fn validate_display_text(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), SignedFlatpakCatalogError> {
    if value.trim().is_empty()
        || value.trim() != value
        || value.chars().any(char::is_control)
        || contains_secret_material(value)
    {
        return Err(SignedFlatpakCatalogError::InvalidField(field));
    }
    if value.len() > max_bytes {
        return Err(SignedFlatpakCatalogError::FieldTooLong(field));
    }
    Ok(())
}

fn normalize_search_query(query: &str) -> Result<String, SignedFlatpakCatalogError> {
    if query.len() > MAX_SEARCH_QUERY_BYTES {
        return Err(SignedFlatpakCatalogError::FieldTooLong("search_query"));
    }
    if query.chars().any(char::is_control) || contains_secret_material(query) {
        return Err(SignedFlatpakCatalogError::InvalidField("search_query"));
    }
    let normalized = query
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    if normalized.is_empty() {
        return Err(SignedFlatpakCatalogError::InvalidField("search_query"));
    }
    Ok(normalized)
}

fn validate_stable_identity(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), SignedFlatpakCatalogError> {
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
        return Err(SignedFlatpakCatalogError::InvalidField(field));
    }
    if value.len() > max_bytes {
        return Err(SignedFlatpakCatalogError::FieldTooLong(field));
    }
    Ok(())
}

fn validate_canonical_list(
    values: &[String],
    field: &'static str,
    max_items: usize,
    max_bytes: usize,
) -> Result<(), SignedFlatpakCatalogError> {
    if values.len() > max_items {
        return Err(SignedFlatpakCatalogError::ResourceLimitExceeded(field));
    }
    let mut previous: Option<&str> = None;
    for value in values {
        validate_stable_identity(field, value, max_bytes)?;
        if previous.is_some_and(|prior| prior >= value.as_str()) {
            return Err(SignedFlatpakCatalogError::NonCanonicalOrder(field));
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

fn decode_hex_64(value: &str) -> Option<[u8; 64]> {
    if value.len() != 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return None;
    }
    let mut output = [0_u8; 64];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        output[index] = (decode_nibble(chunk[0])? << 4) | decode_nibble(chunk[1])?;
    }
    Some(output)
}

fn decode_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
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
                signature: Some("sig-42".into()),
            },
            state: FlatpakInstallState::Installed,
        }
    }

    fn signed_entry(app_id: &str) -> SignedFlatpakCatalogEntry {
        SignedFlatpakCatalogEntry {
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

    fn signed_payload() -> SignedFlatpakCatalogPayload {
        SignedFlatpakCatalogPayload {
            schema_version: SIGNED_FLATPAK_CATALOG_SCHEMA_VERSION,
            catalog_id: "flatpak-curated".into(),
            revision: 42,
            issued_at_unix_ms: NOW - 1_000,
            expires_at_unix_ms: NOW + 60_000,
            origin: FlatpakCatalogOrigin {
                provider_id: "mcnf-curated".into(),
                repository_id: "flathub-stable".into(),
            },
            entries: vec![
                signed_entry("org.example.Editor"),
                signed_entry("org.example.Terminal"),
            ],
        }
    }

    #[test]
    fn catalog_admits_unique_signed_installed_rows() {
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
    fn unsigned_or_uninstalled_rows_are_not_launchable() {
        let mut row = entry("org.example.Editor");
        row.provenance.signature = None;
        assert!(!row.is_launchable());
        row.provenance.signature = Some("sig-42".into());
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
            "installed and signed metadata is not implicit launch authority"
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
    fn signed_catalog_admits_exact_trusted_fresh_untampered_payload() {
        let signing_key = SigningKey::from_bytes(&[7_u8; 32]);
        let verifying_key = signing_key.verifying_key();
        let catalog =
            SignedFlatpakAppCatalog::sign("flatpak-release-2026", signed_payload(), &signing_key)
                .expect("valid signed catalog");

        assert_eq!(catalog.signature.len(), 128);
        assert!(catalog
            .payload
            .content_digest()
            .unwrap()
            .starts_with("sha256:"));
        assert!(catalog
            .admit("flatpak-release-2026", &verifying_key, NOW)
            .is_ok());
    }

    #[test]
    fn signed_catalog_rejects_time_signer_and_signature_failures() {
        let signing_key = SigningKey::from_bytes(&[8_u8; 32]);
        let verifying_key = signing_key.verifying_key();
        let catalog =
            SignedFlatpakAppCatalog::sign("trusted", signed_payload(), &signing_key).unwrap();

        assert_eq!(
            catalog.clone().admit("other", &verifying_key, NOW),
            Err(SignedFlatpakCatalogError::UntrustedSigner)
        );
        assert_eq!(
            catalog.clone().admit(
                "trusted",
                &verifying_key,
                catalog.payload.issued_at_unix_ms - 1
            ),
            Err(SignedFlatpakCatalogError::NotYetValid)
        );
        assert_eq!(
            catalog.clone().admit(
                "trusted",
                &verifying_key,
                catalog.payload.expires_at_unix_ms
            ),
            Err(SignedFlatpakCatalogError::StaleCatalog)
        );

        let mut tampered = catalog;
        tampered.payload.entries[0].display_name = "Tampered".into();
        assert_eq!(
            tampered.admit("trusted", &verifying_key, NOW),
            Err(SignedFlatpakCatalogError::SignatureMismatch)
        );
    }

    #[test]
    fn signed_catalog_rejects_duplicate_noncanonical_and_oversized_metadata() {
        let mut duplicate = signed_payload();
        duplicate.entries[1].app_id = duplicate.entries[0].app_id.clone();
        assert_eq!(
            duplicate.validate(),
            Err(SignedFlatpakCatalogError::DuplicateAppId)
        );

        let mut unordered = signed_payload();
        unordered.entries.swap(0, 1);
        assert_eq!(
            unordered.validate(),
            Err(SignedFlatpakCatalogError::NonCanonicalOrder("entries"))
        );

        let mut oversized = signed_payload();
        oversized.entries[0].search.terms = (0..=MAX_SEARCH_TERMS)
            .map(|index| format!("term-{index:02}"))
            .collect();
        assert_eq!(
            oversized.validate(),
            Err(SignedFlatpakCatalogError::ResourceLimitExceeded(
                "search.terms"
            ))
        );

        let mut unstable_ranking = signed_payload();
        unstable_ranking.entries[0].search.terms = vec!["text".into(), "editor".into()];
        assert_eq!(
            unstable_ranking.validate(),
            Err(SignedFlatpakCatalogError::NonCanonicalOrder("search.terms"))
        );
    }

    #[test]
    fn signed_catalog_rejects_locators_secrets_and_unsupported_permissions() {
        for (field, poison) in [
            ("version", "https://repo.invalid/app"),
            ("icon_id", "/var/lib/icons/editor.svg"),
            ("guest_profile", "file:///etc/passwd"),
            ("version", "token=super-secret"),
        ] {
            let mut payload = signed_payload();
            match field {
                "version" => payload.entries[0].version = poison.into(),
                "icon_id" => payload.entries[0].icon_id = poison.into(),
                "guest_profile" => payload.entries[0].guest_profile = poison.into(),
                _ => unreachable!(),
            }
            assert_eq!(
                payload.validate(),
                Err(SignedFlatpakCatalogError::InvalidField(field))
            );
        }

        let mut unsupported = signed_payload();
        unsupported.entries[0].permissions = vec!["host-filesystem".into()];
        assert_eq!(
            unsupported.validate(),
            Err(SignedFlatpakCatalogError::UnsupportedPermission(
                "host-filesystem".into()
            ))
        );
    }

    #[test]
    fn signed_catalog_json_refuses_unknown_fields_and_schema_skew() {
        let mut value = serde_json::to_value(signed_payload()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("download_url".into(), serde_json::json!("https://invalid"));
        assert!(serde_json::from_value::<SignedFlatpakCatalogPayload>(value).is_err());

        let mut skewed = signed_payload();
        skewed.schema_version += 1;
        assert_eq!(
            skewed.validate(),
            Err(SignedFlatpakCatalogError::UnsupportedSchema(2))
        );

        let legacy_unknown = r#"{"schema_version":1,"revision":"r","entries":[],"extra":true}"#;
        assert!(serde_json::from_str::<FlatpakAppCatalog>(legacy_unknown).is_err());
    }

    #[test]
    fn signed_catalog_bounds_json_before_parsing_and_rejects_duplicate_keys() {
        let signing_key = SigningKey::from_bytes(&[9_u8; 32]);
        let verifying_key = signing_key.verifying_key();
        let catalog =
            SignedFlatpakAppCatalog::sign("trusted", signed_payload(), &signing_key).unwrap();
        let body = serde_json::to_vec(&catalog).unwrap();
        assert!(SignedFlatpakAppCatalog::decode_and_admit_json(
            &body,
            "trusted",
            &verifying_key,
            NOW
        )
        .is_ok());

        let oversized = vec![b' '; MAX_SIGNED_CATALOG_WIRE_BYTES + 1];
        assert_eq!(
            SignedFlatpakAppCatalog::decode_and_admit_json(
                &oversized,
                "trusted",
                &verifying_key,
                NOW
            ),
            Err(SignedFlatpakCatalogError::WirePayloadTooLarge)
        );

        let duplicate =
            br#"{"signer_id":"trusted","signer_id":"trusted","payload":{},"signature":"00"}"#;
        assert_eq!(
            SignedFlatpakAppCatalog::decode_and_admit_json(
                duplicate,
                "trusted",
                &verifying_key,
                NOW
            ),
            Err(SignedFlatpakCatalogError::MalformedJson)
        );
    }

    #[test]
    fn signed_catalog_search_is_validated_ranked_and_stably_tied() {
        let mut payload = signed_payload();
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
            Err(SignedFlatpakCatalogError::InvalidField("search_query"))
        );
        assert_eq!(
            payload.search(&"q".repeat(MAX_SEARCH_QUERY_BYTES + 1)),
            Err(SignedFlatpakCatalogError::FieldTooLong("search_query"))
        );
    }
}
