//! Typed AOSP starter-app catalog and per-Android-VM inventory contract.
//!
//! This module deliberately describes Android launcher intents rather than host
//! commands. Package identities, actions, and categories are closed enums, so a
//! catalog or inventory record cannot smuggle an executable, shell fragment,
//! arbitrary component, URI, or intent extra across the Workloads boundary.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The only AOSP starter catalog schema currently admitted.
pub const AOSP_STARTER_CATALOG_SCHEMA_VERSION: u16 = 1;

/// The only Android image-manifest schema currently admitted.
pub const ANDROID_IMAGE_MANIFEST_SCHEMA_VERSION: u16 = 1;

/// The only pinned Android image/package-manifest schema currently admitted.
pub const ANDROID_IMAGE_PACKAGE_MANIFEST_SCHEMA_VERSION: u16 = 1;

/// The only version of the per-Android-VM guest inventory currently admitted.
pub const ANDROID_GUEST_INVENTORY_SCHEMA_VERSION: u16 = 2;

/// Maximum age a producer may put on one Android guest observation.
pub const MAX_ANDROID_OBSERVATION_AGE_MS: u64 = 30 * 24 * 60 * 60 * 1_000;

/// Number of applications in the governed AOSP starter set.
pub const AOSP_STARTER_APP_COUNT: usize = 9;

const MAX_WORKLOAD_ID_BYTES: usize = 128;
const MAX_ANDROID_IMAGE_ID_BYTES: usize = 128;
const MAX_ANDROID_IMAGE_PROVENANCE_ID_BYTES: usize = 128;
const MAX_ANDROID_PACKAGE_VERSION_BYTES: usize = 128;

/// Stable identity of an application in the governed AOSP starter set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AospStarterApp {
    /// AOSP `Browser`.
    Browser,
    /// AOSP `Calendar`.
    Calendar,
    /// AOSP `Camera2`.
    Camera,
    /// AOSP `DeskClock`.
    Clock,
    /// AOSP `Contacts`.
    Contacts,
    /// AOSP `DocumentsUI` file browser.
    Files,
    /// AOSP `Gallery2` / Photos.
    Gallery,
    /// AOSP `ExactCalculator`.
    Calculator,
    /// Android Settings.
    Settings,
}

impl AospStarterApp {
    /// Complete, stable starter-set order used by contracts and the UI.
    pub const ALL: [Self; AOSP_STARTER_APP_COUNT] = [
        Self::Browser,
        Self::Calendar,
        Self::Camera,
        Self::Clock,
        Self::Contacts,
        Self::Files,
        Self::Gallery,
        Self::Calculator,
        Self::Settings,
    ];

    /// Human-readable application name.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Browser => "Browser",
            Self::Calendar => "Calendar",
            Self::Camera => "Camera",
            Self::Clock => "Clock",
            Self::Contacts => "Contacts",
            Self::Files => "Files",
            Self::Gallery => "Gallery / Photos",
            Self::Calculator => "Calculator",
            Self::Settings => "Settings",
        }
    }

    /// Stable AOSP package identity for this application.
    #[must_use]
    pub const fn package_id(self) -> AospPackageId {
        match self {
            Self::Browser => AospPackageId::Browser,
            Self::Calendar => AospPackageId::Calendar,
            Self::Camera => AospPackageId::Camera,
            Self::Clock => AospPackageId::Clock,
            Self::Contacts => AospPackageId::Contacts,
            Self::Files => AospPackageId::Files,
            Self::Gallery => AospPackageId::Gallery,
            Self::Calculator => AospPackageId::Calculator,
            Self::Settings => AospPackageId::Settings,
        }
    }

    /// Product category for this application.
    #[must_use]
    pub const fn category(self) -> AndroidAppCategory {
        match self {
            Self::Browser => AndroidAppCategory::Web,
            Self::Calendar | Self::Contacts => AndroidAppCategory::Productivity,
            Self::Camera | Self::Gallery => AndroidAppCategory::CameraAndPhotos,
            Self::Clock | Self::Calculator => AndroidAppCategory::Utilities,
            Self::Files => AndroidAppCategory::Files,
            Self::Settings => AndroidAppCategory::System,
        }
    }

    /// Closed `MAIN` + `LAUNCHER` intent for this application.
    #[must_use]
    pub const fn launch_intent(self) -> AndroidLaunchIntent {
        AndroidLaunchIntent {
            package_id: self.package_id(),
            action: AndroidIntentAction::Main,
            category: AndroidIntentCategory::Launcher,
        }
    }

    /// Complete immutable descriptor for this starter application.
    #[must_use]
    pub const fn descriptor(self) -> AndroidStarterAppDescriptor {
        AndroidStarterAppDescriptor {
            app: self,
            package_id: self.package_id(),
            category: self.category(),
            launch_intent: self.launch_intent(),
        }
    }
}

/// Closed set of stable package identities admitted by the starter catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum AospPackageId {
    /// `com.android.browser`.
    #[serde(rename = "com.android.browser")]
    Browser,
    /// `com.android.calendar`.
    #[serde(rename = "com.android.calendar")]
    Calendar,
    /// `com.android.camera2`.
    #[serde(rename = "com.android.camera2")]
    Camera,
    /// `com.android.deskclock`.
    #[serde(rename = "com.android.deskclock")]
    Clock,
    /// `com.android.contacts`.
    #[serde(rename = "com.android.contacts")]
    Contacts,
    /// `com.android.documentsui`.
    #[serde(rename = "com.android.documentsui")]
    Files,
    /// `com.android.gallery3d`.
    #[serde(rename = "com.android.gallery3d")]
    Gallery,
    /// `com.android.calculator2`.
    #[serde(rename = "com.android.calculator2")]
    Calculator,
    /// `com.android.settings`.
    #[serde(rename = "com.android.settings")]
    Settings,
}

impl AospPackageId {
    /// Reverse-DNS package identity used by the Android package manager.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Browser => "com.android.browser",
            Self::Calendar => "com.android.calendar",
            Self::Camera => "com.android.camera2",
            Self::Clock => "com.android.deskclock",
            Self::Contacts => "com.android.contacts",
            Self::Files => "com.android.documentsui",
            Self::Gallery => "com.android.gallery3d",
            Self::Calculator => "com.android.calculator2",
            Self::Settings => "com.android.settings",
        }
    }
}

/// Closed product category used to group starter apps in Workloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AndroidAppCategory {
    /// Web browsing.
    Web,
    /// Calendar and contact productivity tools.
    Productivity,
    /// Camera and photo management.
    CameraAndPhotos,
    /// File browsing and document selection.
    Files,
    /// Clock and calculator utilities.
    Utilities,
    /// Guest system configuration.
    System,
}

impl AndroidAppCategory {
    /// Short user-facing category label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Web => "Web",
            Self::Productivity => "Productivity",
            Self::CameraAndPhotos => "Camera & photos",
            Self::Files => "Files",
            Self::Utilities => "Utilities",
            Self::System => "System",
        }
    }
}

/// Android intent action admitted by the starter-app launcher contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AndroidIntentAction {
    /// Android's `android.intent.action.MAIN` launcher action.
    Main,
}

/// Android intent category admitted by the starter-app launcher contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AndroidIntentCategory {
    /// Android's `android.intent.category.LAUNCHER` category.
    Launcher,
}

/// A launch request that can name only an admitted package and launcher action.
///
/// There are intentionally no command, component, URI, flag, environment, or
/// extras fields. A future Android worker must resolve this value through the
/// guest package manager rather than interpolate it into a shell command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AndroidLaunchIntent {
    /// Closed package identity.
    pub package_id: AospPackageId,
    /// Closed Android action.
    pub action: AndroidIntentAction,
    /// Closed Android category.
    pub category: AndroidIntentCategory,
}

/// Immutable identity and launch metadata for one starter app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AndroidStarterAppDescriptor {
    /// Stable catalog identity.
    pub app: AospStarterApp,
    /// Stable Android package identity.
    pub package_id: AospPackageId,
    /// Product grouping.
    pub category: AndroidAppCategory,
    /// Closed launcher intent.
    pub launch_intent: AndroidLaunchIntent,
}

impl AndroidStarterAppDescriptor {
    fn validate(self) -> Result<(), AndroidAppContractError> {
        if self == self.app.descriptor() {
            Ok(())
        } else {
            Err(AndroidAppContractError::DescriptorMismatch(self.app))
        }
    }
}

/// Versioned governed catalog projected into the Android Workloads view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AospStarterCatalog {
    /// Schema discriminator.
    pub schema_version: u16,
    /// Complete, ordered starter descriptors.
    pub entries: Vec<AndroidStarterAppDescriptor>,
}

impl AospStarterCatalog {
    /// Construct the canonical v1 starter catalog.
    #[must_use]
    pub fn v1() -> Self {
        Self {
            schema_version: AOSP_STARTER_CATALOG_SCHEMA_VERSION,
            entries: AospStarterApp::ALL
                .into_iter()
                .map(AospStarterApp::descriptor)
                .collect(),
        }
    }

    /// Validate schema, descriptor mappings, uniqueness, and completeness.
    ///
    /// # Errors
    ///
    /// Returns [`AndroidAppContractError`] when the schema or governed starter
    /// set is unsupported, incomplete, duplicated, or internally inconsistent.
    pub fn validate(&self) -> Result<(), AndroidAppContractError> {
        validate_schema_and_starter_set(
            self.schema_version,
            AOSP_STARTER_CATALOG_SCHEMA_VERSION,
            self.entries
                .iter()
                .map(|entry| (entry.app, entry.validate())),
        )
    }

    /// Admit a catalog received across a provider boundary.
    ///
    /// # Errors
    ///
    /// Returns the same validation failures as [`Self::validate`].
    pub fn admitted(self) -> Result<Self, AndroidAppContractError> {
        self.validate()?;
        Ok(self)
    }
}

impl Default for AospStarterCatalog {
    fn default() -> Self {
        Self::v1()
    }
}

/// Immutable provenance binding for a governed Android image.
///
/// The manifest carries the complete [`AospStarterCatalog`] rather than a
/// caller-controlled list of package strings. Consumers must admit the
/// manifest before projecting any Android application, and must use
/// [`Self::validate_at`] when checking a record against a clock. There are no
/// command, path, URL, ADB, component, or intent-extra fields in this wire
/// contract; the nested catalog contains only the closed launcher metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AndroidImageManifest {
    /// Schema discriminator.
    pub schema_version: u16,
    /// Bounded stable image identity, not a registry path or URL.
    pub image_id: String,
    /// Immutable lowercase `sha256:<64 hex>` image digest.
    pub image_digest: String,
    /// Bounded source/build provenance identity.
    pub source_revision: String,
    /// Bounded revision of the governed starter catalog used by the image.
    pub catalog_revision: String,
    /// Unix epoch milliseconds when this manifest was issued.
    pub issued_at_unix_ms: u64,
    /// Unix epoch milliseconds when this manifest was observed/admitted.
    pub observed_at_unix_ms: u64,
    /// Complete governed nine-app catalog bound to the image.
    pub catalog: AospStarterCatalog,
}

/// Strict serde wire helper for [`AndroidImageManifest`]. Semantic validation
/// is performed immediately after decoding, while unknown fields fail closed.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AndroidImageManifestWire {
    schema_version: u16,
    image_id: String,
    image_digest: String,
    source_revision: String,
    catalog_revision: String,
    issued_at_unix_ms: u64,
    observed_at_unix_ms: u64,
    catalog: AospStarterCatalog,
}

impl<'de> Deserialize<'de> for AndroidImageManifest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = AndroidImageManifestWire::deserialize(deserializer)?;
        let manifest = Self {
            schema_version: wire.schema_version,
            image_id: wire.image_id,
            image_digest: wire.image_digest,
            source_revision: wire.source_revision,
            catalog_revision: wire.catalog_revision,
            issued_at_unix_ms: wire.issued_at_unix_ms,
            observed_at_unix_ms: wire.observed_at_unix_ms,
            catalog: wire.catalog,
        };
        manifest
            .validate()
            .map_err(|error| serde::de::Error::custom(format!("{error:?}")))?;
        Ok(manifest)
    }
}

impl AndroidImageManifest {
    /// Construct and intrinsically validate a v1 Android image manifest.
    pub fn new(
        image_id: impl Into<String>,
        image_digest: impl Into<String>,
        source_revision: impl Into<String>,
        catalog_revision: impl Into<String>,
        issued_at_unix_ms: u64,
        observed_at_unix_ms: u64,
        catalog: AospStarterCatalog,
    ) -> Result<Self, AndroidAppContractError> {
        let manifest = Self {
            schema_version: ANDROID_IMAGE_MANIFEST_SCHEMA_VERSION,
            image_id: image_id.into(),
            image_digest: image_digest.into(),
            source_revision: source_revision.into(),
            catalog_revision: catalog_revision.into(),
            issued_at_unix_ms,
            observed_at_unix_ms,
            catalog,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    /// Validate schema, provenance identities, timestamps, digest, and the
    /// complete governed starter catalog without consulting a wall clock.
    pub fn validate(&self) -> Result<(), AndroidAppContractError> {
        if self.schema_version != ANDROID_IMAGE_MANIFEST_SCHEMA_VERSION {
            return Err(AndroidAppContractError::UnsupportedSchema(
                self.schema_version,
            ));
        }
        if !is_valid_android_manifest_identity(&self.image_id, MAX_ANDROID_IMAGE_ID_BYTES) {
            return Err(AndroidAppContractError::InvalidImageIdentity);
        }
        if !is_valid_android_sha256_digest(&self.image_digest) {
            return Err(AndroidAppContractError::InvalidImageDigest);
        }
        if !is_valid_android_manifest_identity(
            &self.source_revision,
            MAX_ANDROID_IMAGE_PROVENANCE_ID_BYTES,
        ) {
            return Err(AndroidAppContractError::InvalidSourceRevision);
        }
        if !is_valid_android_manifest_identity(
            &self.catalog_revision,
            MAX_ANDROID_IMAGE_PROVENANCE_ID_BYTES,
        ) {
            return Err(AndroidAppContractError::InvalidCatalogRevision);
        }
        if self.issued_at_unix_ms == 0 {
            return Err(AndroidAppContractError::InvalidManifestTimestamp(
                "issued_at_unix_ms",
            ));
        }
        if self.observed_at_unix_ms == 0 || self.observed_at_unix_ms < self.issued_at_unix_ms {
            return Err(AndroidAppContractError::InvalidManifestTimestamp(
                "observed_at_unix_ms",
            ));
        }
        self.catalog.validate()
    }

    /// Validate intrinsic fields and reject records issued or observed after
    /// the supplied Unix epoch millisecond admission time.
    pub fn validate_at(&self, now_unix_ms: u64) -> Result<(), AndroidAppContractError> {
        self.validate()?;
        if now_unix_ms < self.issued_at_unix_ms {
            return Err(AndroidAppContractError::FutureManifestTimestamp {
                field: "issued_at_unix_ms",
                now_unix_ms,
                timestamp_unix_ms: self.issued_at_unix_ms,
            });
        }
        if now_unix_ms < self.observed_at_unix_ms {
            return Err(AndroidAppContractError::FutureManifestTimestamp {
                field: "observed_at_unix_ms",
                now_unix_ms,
                timestamp_unix_ms: self.observed_at_unix_ms,
            });
        }
        Ok(())
    }

    /// Admit a manifest after intrinsic validation.
    pub fn admitted(self) -> Result<Self, AndroidAppContractError> {
        self.validate()?;
        Ok(self)
    }

    /// Admit a manifest after intrinsic and clock-freshness validation.
    pub fn admitted_at(self, now_unix_ms: u64) -> Result<Self, AndroidAppContractError> {
        self.validate_at(now_unix_ms)?;
        Ok(self)
    }
}

/// The bounded immutable image identity carried by a guest inventory.
///
/// This is the small provenance binding that an inventory needs to retain. The
/// complete [`AndroidImageManifest`] remains the admission source; reducing it
/// to these four identities avoids copying the governed catalog into every
/// package observation while still preventing an inventory from floating free
/// of the image that produced it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AndroidImageProvenance {
    /// Stable image identity, not a registry path or URL.
    pub image_id: String,
    /// Immutable lowercase `sha256:<64 hex>` image digest.
    pub image_digest: String,
    /// Source/build revision that produced the image.
    pub source_revision: String,
    /// Governed starter-catalog revision bound into the image.
    pub catalog_revision: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AndroidImageProvenanceWire {
    image_id: String,
    image_digest: String,
    source_revision: String,
    catalog_revision: String,
}

impl<'de> Deserialize<'de> for AndroidImageProvenance {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = AndroidImageProvenanceWire::deserialize(deserializer)?;
        let provenance = Self {
            image_id: wire.image_id,
            image_digest: wire.image_digest,
            source_revision: wire.source_revision,
            catalog_revision: wire.catalog_revision,
        };
        provenance
            .validate()
            .map_err(|error| serde::de::Error::custom(format!("{error:?}")))?;
        Ok(provenance)
    }
}

impl AndroidImageProvenance {
    /// Construct a validated inventory provenance binding.
    pub fn new(
        image_id: impl Into<String>,
        image_digest: impl Into<String>,
        source_revision: impl Into<String>,
        catalog_revision: impl Into<String>,
    ) -> Result<Self, AndroidAppContractError> {
        let provenance = Self {
            image_id: image_id.into(),
            image_digest: image_digest.into(),
            source_revision: source_revision.into(),
            catalog_revision: catalog_revision.into(),
        };
        provenance.validate()?;
        Ok(provenance)
    }

    /// Reduce an admitted image manifest to the binding retained by inventory.
    pub fn from_manifest(manifest: &AndroidImageManifest) -> Result<Self, AndroidAppContractError> {
        manifest.validate()?;
        Self::new(
            manifest.image_id.clone(),
            manifest.image_digest.clone(),
            manifest.source_revision.clone(),
            manifest.catalog_revision.clone(),
        )
    }

    /// Validate all bounded provenance identities.
    pub fn validate(&self) -> Result<(), AndroidAppContractError> {
        if !is_valid_android_manifest_identity(&self.image_id, MAX_ANDROID_IMAGE_ID_BYTES) {
            return Err(AndroidAppContractError::InvalidImageIdentity);
        }
        if !is_valid_android_sha256_digest(&self.image_digest) {
            return Err(AndroidAppContractError::InvalidImageDigest);
        }
        if !is_valid_android_manifest_identity(
            &self.source_revision,
            MAX_ANDROID_IMAGE_PROVENANCE_ID_BYTES,
        ) {
            return Err(AndroidAppContractError::InvalidSourceRevision);
        }
        if !is_valid_android_manifest_identity(
            &self.catalog_revision,
            MAX_ANDROID_IMAGE_PROVENANCE_ID_BYTES,
        ) {
            return Err(AndroidAppContractError::InvalidCatalogRevision);
        }
        Ok(())
    }
}

/// One package pinned into the immutable AOSP starter image.
///
/// This is build-time image content, not guest inventory evidence. In
/// particular, it intentionally has no `installed`, readiness, launcher, or
/// transport field. A package manifest can prove what an image was built to
/// contain, but it cannot make a running guest claim that the package is
/// installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AndroidImagePackage {
    /// Closed governed starter identity.
    pub app: AospStarterApp,
    /// Stable Android package identity; must match [`Self::app`].
    pub package_id: AospPackageId,
    /// Pinned package-manager version from the image build.
    pub version: AndroidPackageVersion,
}

impl AndroidImagePackage {
    /// Construct a package entry from the canonical package identity.
    #[must_use]
    pub fn for_app(app: AospStarterApp, version: AndroidPackageVersion) -> Self {
        Self {
            app,
            package_id: app.package_id(),
            version,
        }
    }

    fn validate(&self) -> Result<(), AndroidAppContractError> {
        if self.package_id != self.app.package_id() {
            return Err(AndroidAppContractError::DescriptorMismatch(self.app));
        }
        self.version.validate()
    }
}

/// Strict, deterministic package manifest for the pinned AOSP starter image.
///
/// The manifest is an immutable build artifact. It binds exactly the governed
/// nine package identities and their pinned versions to the already-admitted
/// image provenance. It is deliberately not an [`AndroidAppInventory`]: a
/// consumer must still obtain a guest-owned inventory before reporting an
/// application as installed or launchable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AndroidImagePackageManifest {
    /// Schema discriminator.
    pub schema_version: u16,
    /// Immutable image identity and source/catalog provenance.
    pub image_provenance: AndroidImageProvenance,
    /// Complete packages in the canonical [`AospStarterApp::ALL`] order.
    pub packages: Vec<AndroidImagePackage>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AndroidImagePackageManifestWire {
    schema_version: u16,
    image_provenance: AndroidImageProvenance,
    packages: Vec<AndroidImagePackage>,
}

impl<'de> Deserialize<'de> for AndroidImagePackageManifest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = AndroidImagePackageManifestWire::deserialize(deserializer)?;
        let manifest = Self {
            schema_version: wire.schema_version,
            image_provenance: wire.image_provenance,
            packages: wire.packages,
        };
        manifest
            .validate()
            .map_err(|error| serde::de::Error::custom(format!("{error:?}")))?;
        Ok(manifest)
    }
}

impl AndroidImagePackageManifest {
    /// Construct and intrinsically validate a v1 image package manifest.
    pub fn new(
        image_provenance: AndroidImageProvenance,
        packages: Vec<AndroidImagePackage>,
    ) -> Result<Self, AndroidAppContractError> {
        let manifest = Self {
            schema_version: ANDROID_IMAGE_PACKAGE_MANIFEST_SCHEMA_VERSION,
            image_provenance,
            packages,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    /// Validate the schema, provenance, exact package set, identity mapping,
    /// canonical order, and pinned versions.
    pub fn validate(&self) -> Result<(), AndroidAppContractError> {
        if self.schema_version != ANDROID_IMAGE_PACKAGE_MANIFEST_SCHEMA_VERSION {
            return Err(AndroidAppContractError::UnsupportedSchema(
                self.schema_version,
            ));
        }
        self.image_provenance.validate()?;
        if self.packages.len() != AOSP_STARTER_APP_COUNT {
            return Err(AndroidAppContractError::WrongStarterSetSize);
        }

        let mut seen_apps = BTreeSet::new();
        let mut seen_packages = BTreeSet::new();
        for (index, package) in self.packages.iter().enumerate() {
            if !seen_apps.insert(package.app) {
                return Err(AndroidAppContractError::DuplicateApp(package.app));
            }
            if !seen_packages.insert(package.package_id) {
                return Err(AndroidAppContractError::DuplicatePackage(
                    package.package_id,
                ));
            }
            let expected_app = AospStarterApp::ALL[index];
            if package.app != expected_app {
                return Err(AndroidAppContractError::UnexpectedPackageOrder {
                    expected: expected_app,
                    actual: package.app,
                });
            }
            package.validate()?;
        }
        for app in AospStarterApp::ALL {
            if !seen_apps.contains(&app) {
                return Err(AndroidAppContractError::MissingApp(app));
            }
        }
        Ok(())
    }

    /// Admit a package manifest received from an image or packaging boundary.
    pub fn admitted(self) -> Result<Self, AndroidAppContractError> {
        self.validate()?;
        Ok(self)
    }
}

/// Closed state of the inner Android guest at inventory observation time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AndroidGuestBootState {
    /// No guest observation has been admitted yet.
    Pending,
    /// The outer VM is present and the inner guest is booting.
    Booting,
    /// The guest is booted and its package inventory was observed.
    Ready,
    /// The guest cannot currently provide an inventory.
    Unavailable,
}

impl Default for AndroidGuestBootState {
    fn default() -> Self {
        Self::Pending
    }
}

impl AndroidGuestBootState {
    /// Honest user-facing boot-state label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Pending => "guest pending",
            Self::Booting => "guest booting",
            Self::Ready => "guest ready",
            Self::Unavailable => "guest unavailable",
        }
    }
}

/// Closed reason for an unavailable guest or starter application.
///
/// A reason is an enum rather than operator text so an unavailable state cannot
/// smuggle a command, component, URI, or arbitrary diagnostic payload through
/// the inventory boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AndroidUnavailableReason {
    /// The admitted image cannot provide the requested app or guest.
    ImageUnavailable,
    /// The package is absent from an observed guest inventory.
    PackageMissing,
    /// The guest is not currently reachable or running.
    GuestUnavailable,
    /// The inner Android guest failed to boot.
    GuestBootFailed,
    /// The package manager could not provide a trustworthy observation.
    PackageManagerUnavailable,
    /// The package has no resolvable closed launcher entry.
    LauncherUnresolvable,
    /// The provider did not answer with an admitted inventory.
    ProviderUnavailable,
    /// Placement capacity prevented the guest from becoming usable.
    CapacityUnavailable,
    /// The guest display/transport needed by the app is unavailable.
    TransportUnavailable,
    /// The retained observation is outside the consumer freshness window.
    ObservationStale,
}

impl AndroidUnavailableReason {
    /// Stable user-facing reason label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::ImageUnavailable => "image unavailable",
            Self::PackageMissing => "package missing",
            Self::GuestUnavailable => "guest unavailable",
            Self::GuestBootFailed => "guest boot failed",
            Self::PackageManagerUnavailable => "package manager unavailable",
            Self::LauncherUnresolvable => "launcher unresolvable",
            Self::ProviderUnavailable => "provider unavailable",
            Self::CapacityUnavailable => "capacity unavailable",
            Self::TransportUnavailable => "transport unavailable",
            Self::ObservationStale => "observation stale",
        }
    }
}

/// Whether the closed MAIN + LAUNCHER entry was resolved in the guest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AndroidLauncherResolvability {
    /// No guest package/launcher observation has arrived.
    Pending,
    /// The package manager resolved the canonical launcher entry.
    Resolved,
    /// The package exists or was expected, but its launcher is not usable.
    Unavailable,
}

impl Default for AndroidLauncherResolvability {
    fn default() -> Self {
        Self::Pending
    }
}

/// Bounded package version evidence from the Android guest package manager.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AndroidPackageVersion {
    /// Package-manager versionName, without whitespace or path syntax.
    pub version_name: String,
    /// Positive package-manager versionCode.
    pub version_code: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AndroidPackageVersionWire {
    version_name: String,
    version_code: u64,
}

impl<'de> Deserialize<'de> for AndroidPackageVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = AndroidPackageVersionWire::deserialize(deserializer)?;
        let version = Self {
            version_name: wire.version_name,
            version_code: wire.version_code,
        };
        version
            .validate()
            .map_err(|error| serde::de::Error::custom(format!("{error:?}")))?;
        Ok(version)
    }
}

impl AndroidPackageVersion {
    /// Construct a validated package version observation.
    pub fn new(
        version_name: impl Into<String>,
        version_code: u64,
    ) -> Result<Self, AndroidAppContractError> {
        let version = Self {
            version_name: version_name.into(),
            version_code,
        };
        version.validate()?;
        Ok(version)
    }

    /// Validate bounded version evidence without interpreting it as a command.
    pub fn validate(&self) -> Result<(), AndroidAppContractError> {
        if self.version_code == 0
            || self.version_name.is_empty()
            || self.version_name.len() > MAX_ANDROID_PACKAGE_VERSION_BYTES
            || self.version_name.trim() != self.version_name
            || !self.version_name.is_ascii()
            || !self.version_name.chars().all(|character| {
                character.is_ascii_alphanumeric()
                    || matches!(character, '-' | '_' | '.' | '+' | ':')
            })
        {
            return Err(AndroidAppContractError::InvalidPackageVersion);
        }
        Ok(())
    }
}

/// Whether a package has been observed in the target Android guest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AndroidAppAvailability {
    /// No guest package-manager inventory has been received yet.
    InventoryPending,
    /// The expected package is installed in the guest.
    Installed,
    /// A guest inventory was received and the package was absent.
    Missing,
    /// The selected image cannot provide the package.
    ImageUnavailable,
}

impl AndroidAppAvailability {
    /// Honest user-facing status label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::InventoryPending => "inventory pending",
            Self::Installed => "installed",
            Self::Missing => "missing",
            Self::ImageUnavailable => "image unavailable",
        }
    }
}

/// Whether an observed package is ready to accept a launcher intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AndroidAppReadiness {
    /// Guest readiness/inventory evidence has not arrived.
    GuestPending,
    /// The package manager and launcher report the app ready.
    Ready,
    /// The app cannot currently become ready.
    Unavailable,
}

impl AndroidAppReadiness {
    /// Honest user-facing readiness label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::GuestPending => "guest pending",
            Self::Ready => "ready",
            Self::Unavailable => "unavailable",
        }
    }
}

/// Readiness of the typed Workloads-to-Android launch path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AndroidLaunchReadiness {
    /// The typed intent exists, but live guest dispatch has not landed.
    IntegrationPending,
    /// Live dispatch is available for this ready package.
    Ready,
    /// Launch is impossible for the observed package/image state.
    Unavailable,
}

impl AndroidLaunchReadiness {
    /// Honest user-facing launch label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::IntegrationPending => "launch integration pending",
            Self::Ready => "launch ready",
            Self::Unavailable => "launch unavailable",
        }
    }
}

/// One per-VM starter-app inventory row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AndroidAppInventoryEntry {
    /// Immutable catalog descriptor.
    pub descriptor: AndroidStarterAppDescriptor,
    /// Guest package availability.
    pub availability: AndroidAppAvailability,
    /// Package-manager version evidence; absent until the package is observed.
    #[serde(default)]
    pub package_version: Option<AndroidPackageVersion>,
    /// Guest application readiness.
    pub readiness: AndroidAppReadiness,
    /// Whether the canonical package launcher was resolved in the guest.
    #[serde(default)]
    pub launcher_resolvability: AndroidLauncherResolvability,
    /// Workloads-to-guest launch-path readiness.
    pub launch_readiness: AndroidLaunchReadiness,
    /// Closed reason when this app cannot currently be used.
    #[serde(default)]
    pub unavailable_reason: Option<AndroidUnavailableReason>,
}

impl AndroidAppInventoryEntry {
    /// Construct an honest pre-observation row for a starter descriptor.
    #[must_use]
    pub const fn pending(descriptor: AndroidStarterAppDescriptor) -> Self {
        Self {
            descriptor,
            availability: AndroidAppAvailability::InventoryPending,
            package_version: None,
            readiness: AndroidAppReadiness::GuestPending,
            launcher_resolvability: AndroidLauncherResolvability::Pending,
            launch_readiness: AndroidLaunchReadiness::IntegrationPending,
            unavailable_reason: None,
        }
    }

    /// Whether this row has enough evidence for a future dispatcher to launch.
    #[must_use]
    pub fn is_launchable(&self) -> bool {
        self.clone().validate().is_ok()
            && self.availability == AndroidAppAvailability::Installed
            && self.readiness == AndroidAppReadiness::Ready
            && self.launcher_resolvability == AndroidLauncherResolvability::Resolved
            && self.launch_readiness == AndroidLaunchReadiness::Ready
    }

    fn validate(self) -> Result<(), AndroidAppContractError> {
        self.descriptor.validate()?;
        let valid = match self.availability {
            AndroidAppAvailability::InventoryPending => {
                self.package_version.is_none()
                    && self.readiness == AndroidAppReadiness::GuestPending
                    && self.launcher_resolvability == AndroidLauncherResolvability::Pending
                    && self.launch_readiness == AndroidLaunchReadiness::IntegrationPending
                    && self.unavailable_reason.is_none()
            }
            AndroidAppAvailability::Installed => {
                let version_is_valid = self
                    .package_version
                    .as_ref()
                    .is_some_and(|version| version.validate().is_ok());
                version_is_valid
                    && match self.readiness {
                        AndroidAppReadiness::GuestPending => {
                            self.launcher_resolvability == AndroidLauncherResolvability::Pending
                                && self.launch_readiness != AndroidLaunchReadiness::Ready
                                && self.unavailable_reason.is_none()
                        }
                        AndroidAppReadiness::Ready => {
                            self.launcher_resolvability == AndroidLauncherResolvability::Resolved
                                && match self.launch_readiness {
                                    AndroidLaunchReadiness::IntegrationPending
                                    | AndroidLaunchReadiness::Ready => {
                                        self.unavailable_reason.is_none()
                                    }
                                    AndroidLaunchReadiness::Unavailable => {
                                        self.unavailable_reason.is_some()
                                    }
                                }
                        }
                        AndroidAppReadiness::Unavailable => {
                            self.launcher_resolvability == AndroidLauncherResolvability::Unavailable
                                && self.launch_readiness == AndroidLaunchReadiness::Unavailable
                                && self.unavailable_reason.is_some()
                        }
                    }
            }
            AndroidAppAvailability::Missing => {
                self.package_version.is_none()
                    && self.readiness == AndroidAppReadiness::Unavailable
                    && self.launcher_resolvability == AndroidLauncherResolvability::Unavailable
                    && self.launch_readiness == AndroidLaunchReadiness::Unavailable
                    && self.unavailable_reason == Some(AndroidUnavailableReason::PackageMissing)
            }
            AndroidAppAvailability::ImageUnavailable => {
                self.package_version.is_none()
                    && self.readiness == AndroidAppReadiness::Unavailable
                    && self.launcher_resolvability == AndroidLauncherResolvability::Unavailable
                    && self.launch_readiness == AndroidLaunchReadiness::Unavailable
                    && self.unavailable_reason == Some(AndroidUnavailableReason::ImageUnavailable)
            }
        };
        if valid {
            Ok(())
        } else {
            Err(AndroidAppContractError::InvalidState(self.descriptor.app))
        }
    }
}

/// Versioned inventory for one reported `android_vm` workload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AndroidAppInventory {
    /// Schema discriminator for this guest-inventory contract.
    pub schema_version: u16,
    /// Stable Android VM identity. This is the existing Workloads identity and
    /// is the key for every package observation in this record.
    pub workload_id: String,
    /// Image provenance admitted before the guest observation was accepted.
    #[serde(default)]
    pub image_provenance: Option<AndroidImageProvenance>,
    /// Closed inner-guest boot state.
    #[serde(default)]
    pub guest_boot_state: AndroidGuestBootState,
    /// Millisecond Unix observation time, or `None` before any observation.
    pub observed_at_unix_ms: Option<u64>,
    /// Producer-reported age of the observation, paired with its timestamp.
    #[serde(default)]
    pub observation_age_ms: Option<u64>,
    /// Closed reason when the guest cannot currently provide usable evidence.
    #[serde(default)]
    pub unavailable_reason: Option<AndroidUnavailableReason>,
    /// Complete starter set with explicit per-app state.
    pub entries: Vec<AndroidAppInventoryEntry>,
}

impl AndroidAppInventory {
    /// Construct the honest initial inventory for a reported Android VM.
    #[must_use]
    pub fn pending(workload_id: impl Into<String>) -> Self {
        Self {
            schema_version: ANDROID_GUEST_INVENTORY_SCHEMA_VERSION,
            workload_id: workload_id.into(),
            image_provenance: None,
            guest_boot_state: AndroidGuestBootState::Pending,
            observed_at_unix_ms: None,
            observation_age_ms: None,
            unavailable_reason: None,
            entries: pending_starter_entries(),
        }
    }

    /// Construct and validate an observed inventory with all required evidence.
    pub fn observed(
        workload_id: impl Into<String>,
        image_provenance: AndroidImageProvenance,
        guest_boot_state: AndroidGuestBootState,
        observed_at_unix_ms: u64,
        observation_age_ms: u64,
        entries: Vec<AndroidAppInventoryEntry>,
    ) -> Result<Self, AndroidAppContractError> {
        let inventory = Self {
            schema_version: ANDROID_GUEST_INVENTORY_SCHEMA_VERSION,
            workload_id: workload_id.into(),
            image_provenance: Some(image_provenance),
            guest_boot_state,
            observed_at_unix_ms: Some(observed_at_unix_ms),
            observation_age_ms: Some(observation_age_ms),
            unavailable_reason: None,
            entries,
        };
        inventory.validate()?;
        Ok(inventory)
    }

    /// Alias for the stable identity used as the inventory key.
    #[must_use]
    pub fn android_vm_id(&self) -> &str {
        &self.workload_id
    }

    /// Validate identity, provenance, observation semantics, starter-set
    /// completeness, and all per-app evidence states.
    ///
    /// # Errors
    ///
    /// Returns [`AndroidAppContractError`] when identity, timestamp, age,
    /// provenance, starter-set, descriptor, or availability/readiness
    /// invariants are violated.
    pub fn validate(&self) -> Result<(), AndroidAppContractError> {
        validate_workload_id(&self.workload_id)?;
        if let Some(provenance) = &self.image_provenance {
            provenance.validate()?;
        }
        validate_observation_pair(self.observed_at_unix_ms, self.observation_age_ms)?;
        if self.observed_at_unix_ms.is_none()
            && self.entries.iter().any(|entry| {
                entry.availability != AndroidAppAvailability::InventoryPending
                    || entry.readiness != AndroidAppReadiness::GuestPending
            })
        {
            return Err(AndroidAppContractError::MissingObservation);
        }

        let entries = validate_schema_and_starter_set(
            self.schema_version,
            ANDROID_GUEST_INVENTORY_SCHEMA_VERSION,
            self.entries
                .iter()
                .map(|entry| (entry.descriptor.app, entry.clone().validate())),
        );
        entries?;

        let all_pending = self.entries.iter().all(|entry| {
            entry.availability == AndroidAppAvailability::InventoryPending
                && entry.readiness == AndroidAppReadiness::GuestPending
                && entry.launcher_resolvability == AndroidLauncherResolvability::Pending
                && entry.package_version.is_none()
                && entry.unavailable_reason.is_none()
        });
        let has_pending = self.entries.iter().any(|entry| {
            entry.availability == AndroidAppAvailability::InventoryPending
                || entry.readiness == AndroidAppReadiness::GuestPending
        });
        let has_ready_app = self.entries.iter().any(|entry| {
            entry.availability == AndroidAppAvailability::Installed
                && entry.readiness == AndroidAppReadiness::Ready
        });

        let valid_guest_state = match self.guest_boot_state {
            AndroidGuestBootState::Pending => {
                self.observed_at_unix_ms.is_none()
                    && self.observation_age_ms.is_none()
                    && self.unavailable_reason.is_none()
                    && all_pending
            }
            AndroidGuestBootState::Booting => {
                self.observed_at_unix_ms.is_some()
                    && self.observation_age_ms.is_some()
                    && self.unavailable_reason.is_none()
                    && all_pending
            }
            AndroidGuestBootState::Ready => {
                self.image_provenance.is_some()
                    && self.observed_at_unix_ms.is_some()
                    && self.observation_age_ms.is_some()
                    && self.unavailable_reason.is_none()
                    && !has_pending
            }
            AndroidGuestBootState::Unavailable => {
                self.unavailable_reason
                    .is_some_and(is_guest_unavailable_reason)
                    && !has_ready_app
            }
        };
        if valid_guest_state {
            Ok(())
        } else {
            Err(AndroidAppContractError::InvalidGuestState)
        }
    }

    /// Validate intrinsic fields plus the observation against a wall clock.
    pub fn validate_at(&self, now_unix_ms: u64) -> Result<(), AndroidAppContractError> {
        self.validate()?;
        if let Some(observed_at_unix_ms) = self.observed_at_unix_ms {
            if now_unix_ms < observed_at_unix_ms {
                return Err(AndroidAppContractError::FutureObservationTimestamp {
                    now_unix_ms,
                    timestamp_unix_ms: observed_at_unix_ms,
                });
            }
            if let Some(observation_age_ms) = self.observation_age_ms {
                if observation_age_ms > now_unix_ms.saturating_sub(observed_at_unix_ms) {
                    return Err(AndroidAppContractError::ObservationAgeAheadOfClock {
                        now_unix_ms,
                        observed_at_unix_ms,
                        observation_age_ms,
                    });
                }
            }
        }
        Ok(())
    }

    /// Admit an inventory received across the Workloads state boundary.
    ///
    /// # Errors
    ///
    /// Returns the same validation failures as [`Self::validate`].
    pub fn admitted(self) -> Result<Self, AndroidAppContractError> {
        self.validate()?;
        Ok(self)
    }
}

/// Build the complete starter inventory in the explicit pre-observation state.
#[must_use]
pub fn pending_starter_entries() -> Vec<AndroidAppInventoryEntry> {
    AospStarterApp::ALL
        .into_iter()
        .map(|app| AndroidAppInventoryEntry::pending(app.descriptor()))
        .collect()
}

/// Why an AOSP catalog, Android VM inventory, or image manifest was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AndroidAppContractError {
    /// The consumer does not implement this schema version.
    UnsupportedSchema(u16),
    /// The Workloads identity is blank, unbounded, or contains unsafe bytes.
    InvalidWorkloadId,
    /// A present observation timestamp must be non-zero.
    InvalidObservationTime,
    /// Observed app state was supplied without an observation timestamp.
    MissingObservation,
    /// The catalog/inventory does not contain exactly the governed starter set.
    WrongStarterSetSize,
    /// A starter app occurs more than once.
    DuplicateApp(AospStarterApp),
    /// A required starter app is absent.
    MissingApp(AospStarterApp),
    /// Package, category, or intent does not match the stable app identity.
    DescriptorMismatch(AospStarterApp),
    /// A package identity occurs more than once in an image package manifest.
    DuplicatePackage(AospPackageId),
    /// An image package manifest is not in the canonical starter-app order.
    UnexpectedPackageOrder {
        /// Package expected at this canonical position.
        expected: AospStarterApp,
        /// Package actually supplied at this canonical position.
        actual: AospStarterApp,
    },
    /// Availability, readiness, and launch readiness contradict one another.
    InvalidState(AospStarterApp),
    /// A package-manager version is missing, malformed, or oversized.
    InvalidPackageVersion,
    /// Guest boot state, observation fields, and app states contradict one another.
    InvalidGuestState,
    /// An observation timestamp and age were not supplied as a pair or the age
    /// exceeded the bounded retention window.
    InvalidObservationAge,
    /// An observation age claims more elapsed time than the admission clock can
    /// support.
    ObservationAgeAheadOfClock {
        /// Admission time in Unix epoch milliseconds.
        now_unix_ms: u64,
        /// Observation timestamp in Unix epoch milliseconds.
        observed_at_unix_ms: u64,
        /// Supplied observation age in milliseconds.
        observation_age_ms: u64,
    },
    /// An observation timestamp is later than the supplied admission clock.
    FutureObservationTimestamp {
        /// Admission time in Unix epoch milliseconds.
        now_unix_ms: u64,
        /// Observation timestamp in Unix epoch milliseconds.
        timestamp_unix_ms: u64,
    },
    /// The image identity is blank, oversized, or unsafe for an identity field.
    InvalidImageIdentity,
    /// The image digest is not a full, lowercase, non-zero SHA-256 reference.
    InvalidImageDigest,
    /// The image source/build revision is blank, oversized, or unsafe.
    InvalidSourceRevision,
    /// The governed starter-catalog revision is blank, oversized, or unsafe.
    InvalidCatalogRevision,
    /// A manifest timestamp is zero or internally out of order.
    InvalidManifestTimestamp(&'static str),
    /// A manifest timestamp is later than the supplied admission clock.
    FutureManifestTimestamp {
        /// Manifest timestamp field that is in the future.
        field: &'static str,
        /// Admission time in Unix epoch milliseconds.
        now_unix_ms: u64,
        /// Timestamp supplied by the manifest.
        timestamp_unix_ms: u64,
    },
}

/// Version of the typed Android guest request/response boundary.
pub const ANDROID_GUEST_BOUNDARY_SCHEMA_VERSION: u16 = 1;

/// A typed request sent to the Android guest provider.
///
/// The adjacent `op`/`payload` shape gives inventory and launch different
/// closed payloads while keeping the request envelope deterministic. The
/// payloads contain no command, component, URI, flag, or extra fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "op",
    content = "payload",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum AndroidGuestRequest {
    /// Ask the provider for the complete governed package inventory.
    Inventory(AndroidGuestInventoryRequest),
    /// Ask the provider to dispatch one canonical launcher intent.
    Launch(AndroidGuestLaunchRequest),
}

/// Inventory request payload for [`AndroidGuestRequest::Inventory`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AndroidGuestInventoryRequest {
    /// Boundary schema discriminator.
    pub schema_version: u16,
    /// Stable request correlation identity.
    pub request_id: String,
    /// Stable Android VM workload identity.
    pub workload_id: String,
}

impl AndroidGuestInventoryRequest {
    /// Construct and validate an inventory request.
    pub fn new(
        request_id: impl Into<String>,
        workload_id: impl Into<String>,
    ) -> Result<Self, AndroidGuestBoundaryError> {
        let request = Self {
            schema_version: ANDROID_GUEST_BOUNDARY_SCHEMA_VERSION,
            request_id: request_id.into(),
            workload_id: workload_id.into(),
        };
        request.validate()?;
        Ok(request)
    }

    /// Validate a request received from an untrusted boundary.
    pub fn validate(&self) -> Result<(), AndroidGuestBoundaryError> {
        validate_guest_boundary_header(self.schema_version, &self.request_id, &self.workload_id)
    }
}

/// Launch request payload for [`AndroidGuestRequest::Launch`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AndroidGuestLaunchRequest {
    /// Boundary schema discriminator.
    pub schema_version: u16,
    /// Stable request correlation identity.
    pub request_id: String,
    /// Stable Android VM workload identity.
    pub workload_id: String,
    /// Closed starter-catalog identity.
    pub app: AospStarterApp,
    /// Closed guest launcher intent; it must match `app` exactly.
    pub intent: AndroidLaunchIntent,
}

impl AndroidGuestLaunchRequest {
    /// Construct and validate a launch request with an explicit intent.
    ///
    /// Keeping the intent explicit lets the receiver re-check the package and
    /// launcher identity after deserialization instead of trusting a caller's
    /// app enum alone.
    pub fn new(
        request_id: impl Into<String>,
        workload_id: impl Into<String>,
        app: AospStarterApp,
        intent: AndroidLaunchIntent,
    ) -> Result<Self, AndroidGuestBoundaryError> {
        let request = Self {
            schema_version: ANDROID_GUEST_BOUNDARY_SCHEMA_VERSION,
            request_id: request_id.into(),
            workload_id: workload_id.into(),
            app,
            intent,
        };
        request.validate()?;
        Ok(request)
    }

    /// Construct the canonical launcher request for one starter app.
    pub fn for_app(
        request_id: impl Into<String>,
        workload_id: impl Into<String>,
        app: AospStarterApp,
    ) -> Result<Self, AndroidGuestBoundaryError> {
        Self::new(request_id, workload_id, app, app.launch_intent())
    }

    /// Validate a request received from an untrusted boundary.
    pub fn validate(&self) -> Result<(), AndroidGuestBoundaryError> {
        validate_guest_boundary_header(self.schema_version, &self.request_id, &self.workload_id)?;
        if self.intent == self.app.launch_intent() {
            Ok(())
        } else {
            Err(AndroidGuestBoundaryError::LaunchIdentityMismatch(self.app))
        }
    }
}

impl AndroidGuestRequest {
    /// Construct a validated inventory request envelope.
    pub fn inventory(
        request_id: impl Into<String>,
        workload_id: impl Into<String>,
    ) -> Result<Self, AndroidGuestBoundaryError> {
        Ok(Self::Inventory(AndroidGuestInventoryRequest::new(
            request_id,
            workload_id,
        )?))
    }

    /// Construct a validated canonical launch request envelope.
    pub fn launch(
        request_id: impl Into<String>,
        workload_id: impl Into<String>,
        app: AospStarterApp,
    ) -> Result<Self, AndroidGuestBoundaryError> {
        Ok(Self::Launch(AndroidGuestLaunchRequest::for_app(
            request_id,
            workload_id,
            app,
        )?))
    }

    /// Re-check the complete request before provider dispatch.
    pub fn validate(&self) -> Result<(), AndroidGuestBoundaryError> {
        match self {
            Self::Inventory(request) => request.validate(),
            Self::Launch(request) => request.validate(),
        }
    }

    /// Admit a request received from a wire boundary.
    pub fn admitted(self) -> Result<Self, AndroidGuestBoundaryError> {
        self.validate()?;
        Ok(self)
    }

    /// Stable request correlation identity.
    #[must_use]
    pub fn request_id(&self) -> &str {
        match self {
            Self::Inventory(request) => &request.request_id,
            Self::Launch(request) => &request.request_id,
        }
    }

    /// Stable Android VM workload identity.
    #[must_use]
    pub fn workload_id(&self) -> &str {
        match self {
            Self::Inventory(request) => &request.workload_id,
            Self::Launch(request) => &request.workload_id,
        }
    }
}

/// Closed result returned after a typed Android launcher dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AndroidGuestLaunchOutcome {
    /// The guest accepted and started the requested launcher intent.
    Started,
    /// The requested app session was already running and remains usable.
    AlreadyRunning,
    /// The guest or package cannot currently serve the request.
    Unavailable,
    /// The guest rejected the request without claiming a launch.
    Rejected,
}

/// Inventory response payload for [`AndroidGuestResponse::Inventory`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AndroidGuestInventoryResponse {
    /// Boundary schema discriminator.
    pub schema_version: u16,
    /// Echoed request correlation identity.
    pub request_id: String,
    /// Echoed Android VM workload identity.
    pub workload_id: String,
    /// Complete, validated guest package inventory.
    pub inventory: AndroidAppInventory,
}

impl AndroidGuestInventoryResponse {
    /// Build a correlated inventory response for one request.
    pub fn new(
        request: &AndroidGuestInventoryRequest,
        inventory: AndroidAppInventory,
    ) -> Result<Self, AndroidGuestBoundaryError> {
        request.validate()?;
        let response = Self {
            schema_version: ANDROID_GUEST_BOUNDARY_SCHEMA_VERSION,
            request_id: request.request_id.clone(),
            workload_id: request.workload_id.clone(),
            inventory,
        };
        response.validate()?;
        Ok(response)
    }

    /// Validate the response independently before correlation.
    pub fn validate(&self) -> Result<(), AndroidGuestBoundaryError> {
        validate_guest_boundary_header(self.schema_version, &self.request_id, &self.workload_id)?;
        self.inventory
            .validate()
            .map_err(AndroidGuestBoundaryError::InvalidInventory)?;
        if self.inventory.workload_id == self.workload_id {
            Ok(())
        } else {
            Err(AndroidGuestBoundaryError::InventoryWorkloadMismatch)
        }
    }
}

/// Launch response payload for [`AndroidGuestResponse::Launch`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AndroidGuestLaunchResponse {
    /// Boundary schema discriminator.
    pub schema_version: u16,
    /// Echoed request correlation identity.
    pub request_id: String,
    /// Echoed Android VM workload identity.
    pub workload_id: String,
    /// Echoed closed starter-catalog identity.
    pub app: AospStarterApp,
    /// Echoed canonical launcher intent.
    pub intent: AndroidLaunchIntent,
    /// Closed guest dispatch result.
    pub outcome: AndroidGuestLaunchOutcome,
}

impl AndroidGuestLaunchResponse {
    /// Build a correlated launch response for one request.
    pub fn new(
        request: &AndroidGuestLaunchRequest,
        outcome: AndroidGuestLaunchOutcome,
    ) -> Result<Self, AndroidGuestBoundaryError> {
        request.validate()?;
        let response = Self {
            schema_version: ANDROID_GUEST_BOUNDARY_SCHEMA_VERSION,
            request_id: request.request_id.clone(),
            workload_id: request.workload_id.clone(),
            app: request.app,
            intent: request.intent,
            outcome,
        };
        response.validate()?;
        Ok(response)
    }

    /// Validate the echoed package and launcher identity independently.
    pub fn validate(&self) -> Result<(), AndroidGuestBoundaryError> {
        validate_guest_boundary_header(self.schema_version, &self.request_id, &self.workload_id)?;
        if self.intent == self.app.launch_intent() {
            Ok(())
        } else {
            Err(AndroidGuestBoundaryError::LaunchIdentityMismatch(self.app))
        }
    }
}

/// A typed response returned by the Android guest provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "op",
    content = "payload",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum AndroidGuestResponse {
    /// Complete package inventory for an inventory request.
    Inventory(AndroidGuestInventoryResponse),
    /// Correlated result for a launch request.
    Launch(AndroidGuestLaunchResponse),
}

impl AndroidGuestResponse {
    /// Build a validated inventory response envelope.
    pub fn inventory(
        request: &AndroidGuestInventoryRequest,
        inventory: AndroidAppInventory,
    ) -> Result<Self, AndroidGuestBoundaryError> {
        Ok(Self::Inventory(AndroidGuestInventoryResponse::new(
            request, inventory,
        )?))
    }

    /// Build a validated launch response envelope.
    pub fn launch(
        request: &AndroidGuestLaunchRequest,
        outcome: AndroidGuestLaunchOutcome,
    ) -> Result<Self, AndroidGuestBoundaryError> {
        Ok(Self::Launch(AndroidGuestLaunchResponse::new(
            request, outcome,
        )?))
    }

    /// Validate the response before correlating it to a request.
    pub fn validate(&self) -> Result<(), AndroidGuestBoundaryError> {
        match self {
            Self::Inventory(response) => response.validate(),
            Self::Launch(response) => response.validate(),
        }
    }

    /// Validate and correlate a response with its exact request.
    pub fn validate_against(
        &self,
        request: &AndroidGuestRequest,
    ) -> Result<(), AndroidGuestBoundaryError> {
        request.validate()?;
        self.validate()?;
        match (request, self) {
            (AndroidGuestRequest::Inventory(request), Self::Inventory(response))
                if request.request_id == response.request_id
                    && request.workload_id == response.workload_id =>
            {
                Ok(())
            }
            (AndroidGuestRequest::Launch(request), Self::Launch(response))
                if request.request_id == response.request_id
                    && request.workload_id == response.workload_id
                    && request.app == response.app
                    && request.intent == response.intent =>
            {
                Ok(())
            }
            _ => Err(AndroidGuestBoundaryError::RequestResponseMismatch),
        }
    }

    /// Admit a response only when it matches the originating request.
    pub fn admitted_against(
        self,
        request: &AndroidGuestRequest,
    ) -> Result<Self, AndroidGuestBoundaryError> {
        self.validate_against(request)?;
        Ok(self)
    }
}

/// Why an Android guest request or response was rejected at the typed boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AndroidGuestBoundaryError {
    /// The consumer does not implement this boundary schema version.
    UnsupportedSchema(u16),
    /// The request correlation identity is blank, unsafe, or oversized.
    InvalidRequestId,
    /// The target workload identity is invalid.
    InvalidWorkloadId,
    /// The response contained an invalid or incomplete package inventory.
    InvalidInventory(AndroidAppContractError),
    /// The inventory's workload identity differs from its response envelope.
    InventoryWorkloadMismatch,
    /// The echoed package/intent pair is not the canonical pair for the app.
    LaunchIdentityMismatch(AospStarterApp),
    /// The response operation, correlation ids, or launch identity differs from
    /// the originating request.
    RequestResponseMismatch,
}

fn validate_guest_boundary_header(
    schema_version: u16,
    request_id: &str,
    workload_id: &str,
) -> Result<(), AndroidGuestBoundaryError> {
    if schema_version != ANDROID_GUEST_BOUNDARY_SCHEMA_VERSION {
        return Err(AndroidGuestBoundaryError::UnsupportedSchema(schema_version));
    }
    if !is_valid_guest_request_id(request_id) {
        return Err(AndroidGuestBoundaryError::InvalidRequestId);
    }
    validate_workload_id(workload_id).map_err(|_| AndroidGuestBoundaryError::InvalidWorkloadId)
}

fn is_valid_guest_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_WORKLOAD_ID_BYTES
        && value.trim() == value
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':')
        })
}

fn validate_schema_and_starter_set<I>(
    schema_version: u16,
    expected_schema_version: u16,
    entries: I,
) -> Result<(), AndroidAppContractError>
where
    I: IntoIterator<Item = (AospStarterApp, Result<(), AndroidAppContractError>)>,
{
    if schema_version != expected_schema_version {
        return Err(AndroidAppContractError::UnsupportedSchema(schema_version));
    }
    let entries: Vec<_> = entries.into_iter().collect();
    if entries.len() != AOSP_STARTER_APP_COUNT {
        return Err(AndroidAppContractError::WrongStarterSetSize);
    }
    let mut seen = BTreeSet::new();
    for (app, validation) in entries {
        validation?;
        if !seen.insert(app) {
            return Err(AndroidAppContractError::DuplicateApp(app));
        }
    }
    for app in AospStarterApp::ALL {
        if !seen.contains(&app) {
            return Err(AndroidAppContractError::MissingApp(app));
        }
    }
    Ok(())
}

fn validate_workload_id(value: &str) -> Result<(), AndroidAppContractError> {
    let valid = !value.is_empty()
        && value.len() <= MAX_WORKLOAD_ID_BYTES
        && value.trim() == value
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':')
        });
    if valid {
        Ok(())
    } else {
        Err(AndroidAppContractError::InvalidWorkloadId)
    }
}

fn validate_observation_pair(
    observed_at_unix_ms: Option<u64>,
    observation_age_ms: Option<u64>,
) -> Result<(), AndroidAppContractError> {
    match (observed_at_unix_ms, observation_age_ms) {
        (None, None) => Ok(()),
        (Some(0), _) => Err(AndroidAppContractError::InvalidObservationTime),
        (Some(_), None) | (None, Some(_)) => Err(AndroidAppContractError::MissingObservation),
        (Some(_), Some(age)) if age > MAX_ANDROID_OBSERVATION_AGE_MS => {
            Err(AndroidAppContractError::InvalidObservationAge)
        }
        (Some(_), Some(_)) => Ok(()),
    }
}

fn is_guest_unavailable_reason(reason: AndroidUnavailableReason) -> bool {
    !matches!(
        reason,
        AndroidUnavailableReason::PackageMissing | AndroidUnavailableReason::LauncherUnresolvable
    )
}

fn is_valid_android_manifest_identity(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value.trim() == value
        && value.is_ascii()
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':')
        })
}

fn is_valid_android_sha256_digest(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64
        && hex
            .chars()
            .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
        && hex.chars().any(|character| character != '0')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starter_catalog_locks_names_packages_categories_and_order() {
        let catalog = AospStarterCatalog::v1();
        assert!(catalog.validate().is_ok());
        assert_eq!(catalog.entries.len(), AOSP_STARTER_APP_COUNT);
        let expected = [
            ("Browser", "com.android.browser"),
            ("Calendar", "com.android.calendar"),
            ("Camera", "com.android.camera2"),
            ("Clock", "com.android.deskclock"),
            ("Contacts", "com.android.contacts"),
            ("Files", "com.android.documentsui"),
            ("Gallery / Photos", "com.android.gallery3d"),
            ("Calculator", "com.android.calculator2"),
            ("Settings", "com.android.settings"),
        ];
        for (entry, (name, package_id)) in catalog.entries.iter().zip(expected) {
            assert_eq!(entry.app.display_name(), name);
            assert_eq!(entry.package_id.as_str(), package_id);
        }
        let packages: BTreeSet<_> = catalog
            .entries
            .iter()
            .map(|entry| entry.package_id)
            .collect();
        assert_eq!(packages.len(), AOSP_STARTER_APP_COUNT);
    }

    #[test]
    fn starter_catalog_round_trips_with_stable_package_ids() {
        let catalog = AospStarterCatalog::v1();
        let body = serde_json::to_string(&catalog).expect("serialize starter catalog");
        assert!(body.contains("com.android.camera2"));
        assert!(body.contains("com.android.documentsui"));
        assert!(body.contains("com.android.calculator2"));
        let decoded: AospStarterCatalog =
            serde_json::from_str(&body).expect("deserialize starter catalog");
        assert_eq!(decoded, catalog);
    }

    fn valid_android_image_manifest() -> AndroidImageManifest {
        AndroidImageManifest::new(
            "aosp-cuttlefish-2026-08",
            "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "aosp-source-2026-08",
            "starter-catalog-v1",
            1_786_000_000_000,
            1_786_000_000_100,
            AospStarterCatalog::v1(),
        )
        .expect("valid Android image manifest")
    }

    fn valid_android_image_provenance() -> AndroidImageProvenance {
        AndroidImageProvenance::from_manifest(&valid_android_image_manifest())
            .expect("valid Android image provenance")
    }

    fn valid_android_image_package_manifest() -> AndroidImagePackageManifest {
        let packages = AospStarterApp::ALL
            .into_iter()
            .map(|app| {
                AndroidImagePackage::for_app(
                    app,
                    AndroidPackageVersion::new("2026.08.1", 1).expect("valid image package"),
                )
            })
            .collect();
        AndroidImagePackageManifest::new(valid_android_image_provenance(), packages)
            .expect("valid Android image package manifest")
    }

    fn ready_android_inventory() -> AndroidAppInventory {
        let mut entries = pending_starter_entries();
        for entry in &mut entries {
            entry.availability = AndroidAppAvailability::Installed;
            entry.package_version =
                Some(AndroidPackageVersion::new("1.0.0", 1).expect("valid package version"));
            entry.readiness = AndroidAppReadiness::Ready;
            entry.launcher_resolvability = AndroidLauncherResolvability::Resolved;
            entry.launch_readiness = AndroidLaunchReadiness::Ready;
        }
        AndroidAppInventory::observed(
            "android-vm-01",
            valid_android_image_provenance(),
            AndroidGuestBootState::Ready,
            1_786_000_000_000,
            100,
            entries,
        )
        .expect("valid observed Android inventory")
    }

    #[test]
    fn android_image_manifest_round_trips_and_binds_the_complete_catalog() {
        let manifest = valid_android_image_manifest();
        assert!(manifest.validate().is_ok());
        assert!(manifest.validate_at(1_786_000_000_200).is_ok());
        assert_eq!(manifest.catalog.entries.len(), AOSP_STARTER_APP_COUNT);
        assert_eq!(manifest.catalog, AospStarterCatalog::v1());

        let body = serde_json::to_string(&manifest).expect("serialize Android image manifest");
        assert!(body.contains("sha256:0123456789abcdef"));
        assert!(body.contains("com.android.documentsui"));
        assert!(!body.contains("command"));
        assert!(!body.contains("adb"));
        let decoded: AndroidImageManifest =
            serde_json::from_str(&body).expect("deserialize Android image manifest");
        assert_eq!(decoded, manifest);
        assert!(decoded.admitted_at(1_786_000_000_200).is_ok());
    }

    #[test]
    fn android_image_manifest_rejects_unknown_command_shaped_fields() {
        let body = serde_json::to_string(&valid_android_image_manifest())
            .expect("serialize Android image manifest");
        for field in ["command", "path", "url", "adb", "intent_extra"] {
            let hostile = body.replacen('{', &format!(r#"{{"{field}":"not-admitted", "#), 1);
            assert!(
                serde_json::from_str::<AndroidImageManifest>(&hostile).is_err(),
                "manifest unexpectedly accepted hostile field {field}"
            );
        }
    }

    #[test]
    fn android_image_manifest_rejects_malformed_or_zero_digest_and_unsafe_identity() {
        let mut malformed_digest = valid_android_image_manifest();
        malformed_digest.image_digest = "sha256:not-a-digest".into();
        assert_eq!(
            malformed_digest.validate(),
            Err(AndroidAppContractError::InvalidImageDigest)
        );

        let mut zero_digest = valid_android_image_manifest();
        zero_digest.image_digest = format!("sha256:{}", "0".repeat(64));
        assert_eq!(
            zero_digest.validate(),
            Err(AndroidAppContractError::InvalidImageDigest)
        );

        let mut uppercase_digest = valid_android_image_manifest();
        uppercase_digest.image_digest =
            "sha256:0123456789ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef".into();
        assert_eq!(
            uppercase_digest.validate(),
            Err(AndroidAppContractError::InvalidImageDigest)
        );

        let mut unsafe_image_id = valid_android_image_manifest();
        unsafe_image_id.image_id = "../android-image".into();
        assert_eq!(
            unsafe_image_id.validate(),
            Err(AndroidAppContractError::InvalidImageIdentity)
        );

        let mut unsafe_source_revision = valid_android_image_manifest();
        unsafe_source_revision.source_revision = "https://source.example/aosp".into();
        assert_eq!(
            unsafe_source_revision.validate(),
            Err(AndroidAppContractError::InvalidSourceRevision)
        );

        let mut oversized_catalog_revision = valid_android_image_manifest();
        oversized_catalog_revision.catalog_revision =
            "r".repeat(MAX_ANDROID_IMAGE_PROVENANCE_ID_BYTES + 1);
        assert_eq!(
            oversized_catalog_revision.validate(),
            Err(AndroidAppContractError::InvalidCatalogRevision)
        );
    }

    #[test]
    fn android_image_manifest_rejects_duplicate_missing_and_invalid_timestamps() {
        let mut duplicate = valid_android_image_manifest();
        duplicate.catalog.entries[1] = duplicate.catalog.entries[0];
        assert_eq!(
            duplicate.validate(),
            Err(AndroidAppContractError::DuplicateApp(
                AospStarterApp::Browser
            ))
        );

        let mut missing = valid_android_image_manifest();
        missing.catalog.entries.pop();
        assert_eq!(
            missing.validate(),
            Err(AndroidAppContractError::WrongStarterSetSize)
        );

        let mut zero_issued = valid_android_image_manifest();
        zero_issued.issued_at_unix_ms = 0;
        assert_eq!(
            zero_issued.validate(),
            Err(AndroidAppContractError::InvalidManifestTimestamp(
                "issued_at_unix_ms"
            ))
        );

        let mut zero_observed = valid_android_image_manifest();
        zero_observed.observed_at_unix_ms = 0;
        assert_eq!(
            zero_observed.validate(),
            Err(AndroidAppContractError::InvalidManifestTimestamp(
                "observed_at_unix_ms"
            ))
        );

        let mut reversed = valid_android_image_manifest();
        reversed.observed_at_unix_ms = reversed.issued_at_unix_ms - 1;
        assert_eq!(
            reversed.validate(),
            Err(AndroidAppContractError::InvalidManifestTimestamp(
                "observed_at_unix_ms"
            ))
        );

        let future = valid_android_image_manifest();
        assert_eq!(
            future.validate_at(future.issued_at_unix_ms - 1),
            Err(AndroidAppContractError::FutureManifestTimestamp {
                field: "issued_at_unix_ms",
                now_unix_ms: future.issued_at_unix_ms - 1,
                timestamp_unix_ms: future.issued_at_unix_ms,
            })
        );
        assert_eq!(
            future.validate_at(future.observed_at_unix_ms - 1),
            Err(AndroidAppContractError::FutureManifestTimestamp {
                field: "observed_at_unix_ms",
                now_unix_ms: future.observed_at_unix_ms - 1,
                timestamp_unix_ms: future.observed_at_unix_ms,
            })
        );
    }

    #[test]
    fn android_image_package_manifest_round_trips_provenance_and_exact_packages() {
        let manifest = valid_android_image_package_manifest();
        assert!(manifest.validate().is_ok());
        assert_eq!(manifest.packages.len(), AOSP_STARTER_APP_COUNT);
        assert_eq!(
            manifest
                .packages
                .iter()
                .map(|package| package.package_id)
                .collect::<BTreeSet<_>>()
                .len(),
            AOSP_STARTER_APP_COUNT
        );

        let body =
            serde_json::to_string(&manifest).expect("serialize Android image package manifest");
        assert!(body.contains("aosp-source-2026-08"));
        assert!(body.contains("com.android.documentsui"));
        assert!(!body.contains("\"installed\""));
        assert!(!body.contains("\"readiness\""));
        let decoded: AndroidImagePackageManifest =
            serde_json::from_str(&body).expect("deserialize Android image package manifest");
        assert_eq!(decoded, manifest);
    }

    #[test]
    fn android_image_package_manifest_rejects_omitted_unknown_duplicate_and_reordered_packages() {
        let mut omitted = valid_android_image_package_manifest();
        omitted.packages.pop();
        assert_eq!(
            omitted.validate(),
            Err(AndroidAppContractError::WrongStarterSetSize)
        );

        let body = serde_json::to_string(&valid_android_image_package_manifest())
            .expect("serialize Android image package manifest");
        let omitted_identity = body.replacen("\"package_id\":\"com.android.browser\",", "", 1);
        assert!(
            serde_json::from_str::<AndroidImagePackageManifest>(&omitted_identity).is_err(),
            "omitted package identity must fail closed"
        );
        let unknown = body.replacen("com.android.browser", "com.android.unknown", 1);
        assert!(
            serde_json::from_str::<AndroidImagePackageManifest>(&unknown).is_err(),
            "unknown package identity must fail closed"
        );

        let mut duplicate = valid_android_image_package_manifest();
        duplicate.packages[1].package_id = duplicate.packages[0].package_id;
        assert_eq!(
            duplicate.validate(),
            Err(AndroidAppContractError::DuplicatePackage(
                AospPackageId::Browser
            ))
        );

        let mut reordered = valid_android_image_package_manifest();
        reordered.packages.swap(0, 1);
        assert_eq!(
            reordered.validate(),
            Err(AndroidAppContractError::UnexpectedPackageOrder {
                expected: AospStarterApp::Browser,
                actual: AospStarterApp::Calendar,
            })
        );

        let hostile = body.replacen(
            "{\"schema_version\":1",
            "{\"schema_version\":1,\"installed\":true",
            1,
        );
        assert!(
            serde_json::from_str::<AndroidImagePackageManifest>(&hostile).is_err(),
            "installed state must not be accepted in the image manifest"
        );
    }

    #[test]
    fn android_image_package_manifest_rejects_bad_versions_and_identity_mapping() {
        let mut bad_version = valid_android_image_package_manifest();
        bad_version.packages[0].version.version_code = 0;
        assert_eq!(
            bad_version.validate(),
            Err(AndroidAppContractError::InvalidPackageVersion)
        );

        let mut mismatched = valid_android_image_package_manifest();
        mismatched.packages[0].package_id = AospPackageId::Calendar;
        assert_eq!(
            mismatched.validate(),
            Err(AndroidAppContractError::DescriptorMismatch(
                AospStarterApp::Browser
            ))
        );
    }

    #[test]
    fn launch_intent_is_closed_and_rejects_command_shaped_extensions() {
        let intent = AospStarterApp::Calendar.launch_intent();
        let body = serde_json::to_string(&intent).expect("serialize intent");
        assert_eq!(
            body,
            r#"{"package_id":"com.android.calendar","action":"main","category":"launcher"}"#
        );
        assert!(!body.contains("command"));
        assert!(serde_json::from_str::<AndroidLaunchIntent>(
            r#"{"package_id":"com.android.calendar","action":"main","category":"launcher","command":"sh"}"#
        )
        .is_err());
        assert!(serde_json::from_str::<AndroidLaunchIntent>(
            r#"{"package_id":"org.example.Arbitrary","action":"main","category":"launcher"}"#
        )
        .is_err());
    }

    #[test]
    fn pending_inventory_is_complete_valid_and_not_launchable() {
        let inventory = AndroidAppInventory::pending("android-vm-01");
        assert!(inventory.validate().is_ok());
        assert_eq!(
            inventory.schema_version,
            ANDROID_GUEST_INVENTORY_SCHEMA_VERSION
        );
        assert_eq!(inventory.android_vm_id(), "android-vm-01");
        assert_eq!(inventory.guest_boot_state, AndroidGuestBootState::Pending);
        assert!(inventory.observed_at_unix_ms.is_none());
        assert!(inventory.observation_age_ms.is_none());
        assert!(inventory.entries.iter().all(|entry| {
            entry.availability == AndroidAppAvailability::InventoryPending
                && entry.readiness == AndroidAppReadiness::GuestPending
                && entry.launcher_resolvability == AndroidLauncherResolvability::Pending
                && entry.launch_readiness == AndroidLaunchReadiness::IntegrationPending
                && !entry.is_launchable()
        }));
    }

    #[test]
    fn observed_installed_ready_entry_can_be_launchable() {
        let inventory = ready_android_inventory();
        assert!(inventory.validate().is_ok());
        assert!(inventory.entries[0].is_launchable());
    }

    #[test]
    fn guest_inventory_v2_round_trips_all_bounded_evidence() {
        let inventory = ready_android_inventory();
        assert!(inventory.validate_at(1_786_000_000_100).is_ok());
        assert_eq!(
            inventory.image_provenance.as_ref().unwrap().image_digest,
            "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );
        assert_eq!(
            inventory.entries[0].descriptor.package_id,
            AospPackageId::Browser
        );
        assert_eq!(
            inventory.entries[0]
                .package_version
                .as_ref()
                .unwrap()
                .version_name,
            "1.0.0"
        );
        assert_eq!(
            inventory.entries[0].launcher_resolvability,
            AndroidLauncherResolvability::Resolved
        );

        let body = serde_json::to_string(&inventory).expect("serialize guest inventory");
        assert!(body.contains("guest_boot_state\":\"ready\""));
        assert!(body.contains("observation_age_ms\":100"));
        assert!(body.contains("version_code\":1"));
        assert!(!body.contains("command"));
        assert!(!body.contains("adb"));
        assert!(!body.contains("component"));
        let decoded: AndroidAppInventory =
            serde_json::from_str(&body).expect("deserialize guest inventory");
        assert_eq!(decoded, inventory);
        assert!(decoded.admitted().is_ok());
    }

    #[test]
    fn guest_inventory_rejects_hostile_duplicate_malformed_and_oversized_evidence() {
        let inventory = ready_android_inventory();
        let body = serde_json::to_string(&inventory).expect("serialize guest inventory");
        for field in ["command", "component", "uri", "flags", "adb"] {
            let hostile = body.replacen('{', &format!(r#"{{"{field}":"not-admitted", "#), 1);
            assert!(
                serde_json::from_str::<AndroidAppInventory>(&hostile).is_err(),
                "inventory unexpectedly accepted hostile field {field}"
            );
        }

        let mut duplicate = inventory.clone();
        duplicate.entries[1] = duplicate.entries[0].clone();
        assert_eq!(
            duplicate.validate(),
            Err(AndroidAppContractError::DuplicateApp(
                AospStarterApp::Browser
            ))
        );

        let mut malformed_version = inventory.clone();
        malformed_version.entries[0].package_version = Some(AndroidPackageVersion {
            version_name: "../adb".into(),
            version_code: 0,
        });
        assert_eq!(
            malformed_version.validate(),
            Err(AndroidAppContractError::InvalidState(
                AospStarterApp::Browser
            ))
        );

        let malformed_wire = body.replacen("\"version_code\":1", "\"version_code\":0", 1);
        assert!(serde_json::from_str::<AndroidAppInventory>(&malformed_wire).is_err());

        let mut oversized_age = inventory.clone();
        oversized_age.observation_age_ms = Some(MAX_ANDROID_OBSERVATION_AGE_MS + 1);
        assert_eq!(
            oversized_age.validate(),
            Err(AndroidAppContractError::InvalidObservationAge)
        );

        let mut unresolved_launcher = inventory.clone();
        unresolved_launcher.entries[0].launcher_resolvability =
            AndroidLauncherResolvability::Pending;
        assert_eq!(
            unresolved_launcher.validate(),
            Err(AndroidAppContractError::InvalidState(
                AospStarterApp::Browser
            ))
        );

        let mut wrong_reason = inventory;
        wrong_reason.entries[0].availability = AndroidAppAvailability::Missing;
        wrong_reason.entries[0].package_version = None;
        wrong_reason.entries[0].readiness = AndroidAppReadiness::Unavailable;
        wrong_reason.entries[0].launcher_resolvability = AndroidLauncherResolvability::Unavailable;
        wrong_reason.entries[0].launch_readiness = AndroidLaunchReadiness::Unavailable;
        wrong_reason.entries[0].unavailable_reason =
            Some(AndroidUnavailableReason::ImageUnavailable);
        assert_eq!(
            wrong_reason.validate(),
            Err(AndroidAppContractError::InvalidState(
                AospStarterApp::Browser
            ))
        );
    }

    #[test]
    fn guest_boundary_round_trips_deterministically_and_correlates() {
        let request =
            AndroidGuestRequest::launch("request-01", "android-vm-01", AospStarterApp::Browser)
                .expect("canonical launch request");
        assert_eq!(request.request_id(), "request-01");
        assert_eq!(request.workload_id(), "android-vm-01");
        let body = serde_json::to_string(&request).expect("serialize launch request");
        assert_eq!(
            body,
            r#"{"op":"launch","payload":{"schema_version":1,"request_id":"request-01","workload_id":"android-vm-01","app":"browser","intent":{"package_id":"com.android.browser","action":"main","category":"launcher"}}}"#
        );
        let decoded: AndroidGuestRequest =
            serde_json::from_str(&body).expect("deserialize launch request");
        assert_eq!(decoded, request);
        assert!(decoded.validate().is_ok());

        let launch_request = match &request {
            AndroidGuestRequest::Launch(request) => request,
            AndroidGuestRequest::Inventory(_) => panic!("expected launch request"),
        };
        let response =
            AndroidGuestResponse::launch(launch_request, AndroidGuestLaunchOutcome::Started)
                .expect("correlated launch response");
        let response_body = serde_json::to_string(&response).expect("serialize launch response");
        assert_eq!(
            response_body,
            r#"{"op":"launch","payload":{"schema_version":1,"request_id":"request-01","workload_id":"android-vm-01","app":"browser","intent":{"package_id":"com.android.browser","action":"main","category":"launcher"},"outcome":"started"}}"#
        );
        let decoded_response: AndroidGuestResponse =
            serde_json::from_str(&response_body).expect("deserialize launch response");
        assert_eq!(decoded_response, response);
        assert!(decoded_response.validate_against(&request).is_ok());
    }

    #[test]
    fn guest_inventory_response_is_typed_to_the_requested_workload() {
        let request = AndroidGuestRequest::inventory("request-02", "android-vm-02")
            .expect("canonical inventory request");
        let inventory_request = match &request {
            AndroidGuestRequest::Inventory(request) => request,
            AndroidGuestRequest::Launch(_) => panic!("expected inventory request"),
        };
        let response = AndroidGuestResponse::inventory(
            inventory_request,
            AndroidAppInventory::pending("android-vm-02"),
        )
        .expect("correlated inventory response");
        assert!(response.validate_against(&request).is_ok());

        assert_eq!(
            AndroidGuestResponse::inventory(
                inventory_request,
                AndroidAppInventory::pending("android-vm-other"),
            ),
            Err(AndroidGuestBoundaryError::InventoryWorkloadMismatch)
        );
    }

    #[test]
    fn guest_boundary_rejects_identity_tampering_and_unknown_fields() {
        let mut request =
            AndroidGuestRequest::launch("request-03", "android-vm-03", AospStarterApp::Browser)
                .expect("canonical launch request");
        if let AndroidGuestRequest::Launch(request) = &mut request {
            request.intent.package_id = AospPackageId::Calendar;
        }
        assert_eq!(
            request.validate(),
            Err(AndroidGuestBoundaryError::LaunchIdentityMismatch(
                AospStarterApp::Browser
            ))
        );

        let canonical =
            AndroidGuestRequest::launch("request-04", "android-vm-04", AospStarterApp::Browser)
                .expect("canonical launch request");
        let body = serde_json::to_string(&canonical).expect("serialize canonical request");
        let body = body.replacen(
            "\"payload\":{",
            "\"payload\":{\"command\":\"adb shell\",",
            1,
        );
        assert!(serde_json::from_str::<AndroidGuestRequest>(&body).is_err());

        let launch_request = match &canonical {
            AndroidGuestRequest::Launch(request) => request,
            AndroidGuestRequest::Inventory(_) => panic!("expected launch request"),
        };
        let mut response =
            AndroidGuestResponse::launch(launch_request, AndroidGuestLaunchOutcome::AlreadyRunning)
                .expect("correlated launch response");
        if let AndroidGuestResponse::Launch(response) = &mut response {
            response.request_id = "other-request".into();
        }
        assert_eq!(
            response.validate_against(&canonical),
            Err(AndroidGuestBoundaryError::RequestResponseMismatch)
        );
    }

    #[test]
    fn launch_ready_cannot_fabricate_guest_readiness() {
        let mut inventory = AndroidAppInventory::pending("android-vm-01");
        inventory.entries[0].launch_readiness = AndroidLaunchReadiness::Ready;
        assert_eq!(
            inventory.validate(),
            Err(AndroidAppContractError::InvalidState(
                AospStarterApp::Browser
            ))
        );
    }

    #[test]
    fn unavailable_guest_state_cannot_leave_launch_pending() {
        let mut inventory = ready_android_inventory();
        inventory.entries[0].availability = AndroidAppAvailability::Installed;
        inventory.entries[0].readiness = AndroidAppReadiness::Unavailable;
        assert_eq!(
            inventory.validate(),
            Err(AndroidAppContractError::InvalidState(
                AospStarterApp::Browser
            ))
        );
        inventory.entries[0].launcher_resolvability = AndroidLauncherResolvability::Unavailable;
        inventory.entries[0].launch_readiness = AndroidLaunchReadiness::Unavailable;
        inventory.entries[0].unavailable_reason =
            Some(AndroidUnavailableReason::ProviderUnavailable);
        assert!(inventory.validate().is_ok());
    }

    #[test]
    fn observed_state_requires_a_timestamp() {
        let mut inventory = AndroidAppInventory::pending("android-vm-01");
        inventory.entries[1].availability = AndroidAppAvailability::Missing;
        inventory.entries[1].package_version = None;
        inventory.entries[1].readiness = AndroidAppReadiness::Unavailable;
        inventory.entries[1].launcher_resolvability = AndroidLauncherResolvability::Unavailable;
        inventory.entries[1].launch_readiness = AndroidLaunchReadiness::Unavailable;
        inventory.entries[1].unavailable_reason = Some(AndroidUnavailableReason::PackageMissing);
        assert_eq!(
            inventory.validate(),
            Err(AndroidAppContractError::MissingObservation)
        );
    }

    #[test]
    fn catalog_rejects_duplicate_and_mismatched_descriptors() {
        let mut duplicate = AospStarterCatalog::v1();
        duplicate.entries[1] = duplicate.entries[0];
        assert_eq!(
            duplicate.validate(),
            Err(AndroidAppContractError::DuplicateApp(
                AospStarterApp::Browser
            ))
        );

        let mut mismatched = AospStarterCatalog::v1();
        mismatched.entries[0].package_id = AospPackageId::Calendar;
        assert_eq!(
            mismatched.validate(),
            Err(AndroidAppContractError::DescriptorMismatch(
                AospStarterApp::Browser
            ))
        );
    }

    #[test]
    fn structs_reject_unknown_wire_fields() {
        let body = serde_json::to_string(&AospStarterCatalog::v1()).expect("catalog JSON");
        let body = body.replacen("{", r#"{"command":"not-admitted","#, 1);
        assert!(serde_json::from_str::<AospStarterCatalog>(&body).is_err());
    }

    #[test]
    fn workload_identity_is_bounded_and_path_safe() {
        assert!(AndroidAppInventory::pending("android:vm_01.example")
            .validate()
            .is_ok());
        assert_eq!(
            AndroidAppInventory::pending("../android-vm").validate(),
            Err(AndroidAppContractError::InvalidWorkloadId)
        );
        assert_eq!(
            AndroidAppInventory::pending("android vm").validate(),
            Err(AndroidAppContractError::InvalidWorkloadId)
        );
    }
}
