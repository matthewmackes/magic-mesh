//! Typed AOSP starter-app catalog and per-Android-VM inventory contract.
//!
//! This module deliberately describes Android launcher intents rather than host
//! commands. Package identities, actions, and categories are closed enums, so a
//! catalog or inventory record cannot smuggle an executable, shell fragment,
//! arbitrary component, URI, or intent extra across the Workloads boundary.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The only AOSP starter catalog/inventory schema currently admitted.
pub const AOSP_STARTER_CATALOG_SCHEMA_VERSION: u16 = 1;

/// Number of applications in the governed AOSP starter set.
pub const AOSP_STARTER_APP_COUNT: usize = 9;

const MAX_WORKLOAD_ID_BYTES: usize = 128;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AndroidAppInventoryEntry {
    /// Immutable catalog descriptor.
    pub descriptor: AndroidStarterAppDescriptor,
    /// Guest package availability.
    pub availability: AndroidAppAvailability,
    /// Guest application readiness.
    pub readiness: AndroidAppReadiness,
    /// Workloads-to-guest launch-path readiness.
    pub launch_readiness: AndroidLaunchReadiness,
}

impl AndroidAppInventoryEntry {
    /// Construct an honest pre-observation row for a starter descriptor.
    #[must_use]
    pub const fn pending(descriptor: AndroidStarterAppDescriptor) -> Self {
        Self {
            descriptor,
            availability: AndroidAppAvailability::InventoryPending,
            readiness: AndroidAppReadiness::GuestPending,
            launch_readiness: AndroidLaunchReadiness::IntegrationPending,
        }
    }

    /// Whether this row has enough evidence for a future dispatcher to launch.
    #[must_use]
    pub fn is_launchable(&self) -> bool {
        self.validate().is_ok()
            && self.availability == AndroidAppAvailability::Installed
            && self.readiness == AndroidAppReadiness::Ready
            && self.launch_readiness == AndroidLaunchReadiness::Ready
    }

    fn validate(self) -> Result<(), AndroidAppContractError> {
        self.descriptor.validate()?;
        let valid = match self.availability {
            AndroidAppAvailability::InventoryPending => {
                self.readiness == AndroidAppReadiness::GuestPending
                    && self.launch_readiness == AndroidLaunchReadiness::IntegrationPending
            }
            AndroidAppAvailability::Installed => match self.readiness {
                AndroidAppReadiness::GuestPending => {
                    self.launch_readiness != AndroidLaunchReadiness::Ready
                }
                AndroidAppReadiness::Ready => true,
                AndroidAppReadiness::Unavailable => {
                    self.launch_readiness == AndroidLaunchReadiness::Unavailable
                }
            },
            AndroidAppAvailability::Missing | AndroidAppAvailability::ImageUnavailable => {
                self.readiness == AndroidAppReadiness::Unavailable
                    && self.launch_readiness == AndroidLaunchReadiness::Unavailable
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
    /// Schema discriminator.
    pub schema_version: u16,
    /// Stable Workloads identity of the outer Android VM.
    pub workload_id: String,
    /// Millisecond Unix observation time, or `None` before any guest inventory.
    pub observed_at_unix_ms: Option<u64>,
    /// Complete starter set with explicit per-app state.
    pub entries: Vec<AndroidAppInventoryEntry>,
}

impl AndroidAppInventory {
    /// Construct the honest initial inventory for a reported Android VM.
    #[must_use]
    pub fn pending(workload_id: impl Into<String>) -> Self {
        Self {
            schema_version: AOSP_STARTER_CATALOG_SCHEMA_VERSION,
            workload_id: workload_id.into(),
            observed_at_unix_ms: None,
            entries: pending_starter_entries(),
        }
    }

    /// Validate identity, observation semantics, starter-set completeness, and state.
    ///
    /// # Errors
    ///
    /// Returns [`AndroidAppContractError`] when identity, timestamp, starter-set,
    /// descriptor, or availability/readiness invariants are violated.
    pub fn validate(&self) -> Result<(), AndroidAppContractError> {
        validate_workload_id(&self.workload_id)?;
        if self.observed_at_unix_ms == Some(0) {
            return Err(AndroidAppContractError::InvalidObservationTime);
        }
        if self.observed_at_unix_ms.is_none()
            && self.entries.iter().any(|entry| {
                entry.availability != AndroidAppAvailability::InventoryPending
                    || entry.readiness != AndroidAppReadiness::GuestPending
            })
        {
            return Err(AndroidAppContractError::MissingObservation);
        }
        validate_schema_and_starter_set(
            self.schema_version,
            self.entries
                .iter()
                .map(|entry| (entry.descriptor.app, entry.validate())),
        )
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

/// Why an AOSP catalog or Android VM inventory was rejected.
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
    /// Availability, readiness, and launch readiness contradict one another.
    InvalidState(AospStarterApp),
}

fn validate_schema_and_starter_set<I>(
    schema_version: u16,
    entries: I,
) -> Result<(), AndroidAppContractError>
where
    I: IntoIterator<Item = (AospStarterApp, Result<(), AndroidAppContractError>)>,
{
    if schema_version != AOSP_STARTER_CATALOG_SCHEMA_VERSION {
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
        assert!(inventory.observed_at_unix_ms.is_none());
        assert!(inventory.entries.iter().all(|entry| {
            entry.availability == AndroidAppAvailability::InventoryPending
                && entry.readiness == AndroidAppReadiness::GuestPending
                && entry.launch_readiness == AndroidLaunchReadiness::IntegrationPending
                && !entry.is_launchable()
        }));
    }

    #[test]
    fn observed_installed_ready_entry_can_be_launchable() {
        let mut inventory = AndroidAppInventory::pending("android-vm-01");
        inventory.observed_at_unix_ms = Some(1_786_000_000_000);
        inventory.entries[0].availability = AndroidAppAvailability::Installed;
        inventory.entries[0].readiness = AndroidAppReadiness::Ready;
        inventory.entries[0].launch_readiness = AndroidLaunchReadiness::Ready;
        assert!(inventory.validate().is_ok());
        assert!(inventory.entries[0].is_launchable());
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
        let mut inventory = AndroidAppInventory::pending("android-vm-01");
        inventory.observed_at_unix_ms = Some(1_786_000_000_000);
        inventory.entries[0].availability = AndroidAppAvailability::Installed;
        inventory.entries[0].readiness = AndroidAppReadiness::Unavailable;
        assert_eq!(
            inventory.validate(),
            Err(AndroidAppContractError::InvalidState(
                AospStarterApp::Browser
            ))
        );
        inventory.entries[0].launch_readiness = AndroidLaunchReadiness::Unavailable;
        assert!(inventory.validate().is_ok());
    }

    #[test]
    fn observed_state_requires_a_timestamp() {
        let mut inventory = AndroidAppInventory::pending("android-vm-01");
        inventory.entries[1].availability = AndroidAppAvailability::Missing;
        inventory.entries[1].readiness = AndroidAppReadiness::Unavailable;
        inventory.entries[1].launch_readiness = AndroidLaunchReadiness::Unavailable;
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
