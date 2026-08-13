//! Workloads U9 — the `android-provision` verb handler (two-layer Cuttlefish).
//!
//! Android is delivered as a **two-layer** stack: an L1 Linux (Debian) VM sized for
//! nested virtualization (`cpu host-passthrough`), inside which the
//! `cuttlefish_host` Ansible role (a separate unit) runs `cvd start
//! --start_vnc_server` to boot the Android guest under crosvm. This handler owns
//! only the FIRST layer's declaration: it constructs an [`DeliveryType::AndroidVm`]
//! [`WorkloadSpec`] sized for Cuttlefish and persists it as desired state for the
//! typed Workload reconciler. This handler writes no OpenTofu input or live VM.
//! The Android screen
//! lives inside crosvm-inside-the-L1-VM (invisible to `virsh domdisplay`), so its
//! console is the in-guest VNC/WebRTC endpoint `cvd` serves, not a libvirt display.
//!
//! Fallback: on a host WITHOUT nested-KVM the same spec is realized as
//! Android-x86-in-KVM (a direct KVM guest, no Cuttlefish layer) — a `modules/android`
//! concern; the spec this handler mints is identical either way.
//!
//! Honest routing (§7): the spec is routed through the reconcile/set-desired seam
//! (shared with `set-desired`) and the reply is explicitly desired-state-only.
//! A typed Workload row operation must realize the persisted declaration;
//! `android-provision` itself never claims a live VM.

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
    sync::Arc,
};

use serde::{Deserialize, Serialize};

use mackes_mesh_types::android_apps::{
    AndroidAppContractError, AndroidAppInventory, AndroidGuestBoundaryError,
    AndroidGuestInventoryRequest, AndroidGuestInventoryResponse, AndroidGuestLaunchOutcome,
    AndroidGuestLaunchRequest, AndroidGuestRequest, AndroidGuestResponse, AndroidImageManifest,
    AndroidImagePackageManifest, AndroidSignedCatalog,
};
use mackes_mesh_types::android_provider::AndroidVdiSource;
use mackes_mesh_types::cloud::{CloudReply, DeliveryType, HealthState, WorkloadSpec};

use super::super::android_provider::{
    configured_image_path, preflight, AndroidHostProbe, AndroidPreflightInput,
};
use super::super::CloudWorker;
use super::super::{reconcile, runner};
use super::CloudActionBody;

#[path = "cuttlefish.rs"]
mod cuttlefish;
#[path = "cuttlefish_guest.rs"]
mod cuttlefish_guest;

pub(crate) use cuttlefish::{
    CuttlefishOuterWorkloadObservation, CuttlefishProviderAdapter, CuttlefishProviderClient,
    CuttlefishProviderError, WorkloadCuttlefishProviderClient,
};

/// Cuttlefish L1-VM minimum virtual CPUs (nested-KVM Android needs headroom).
const CUTTLEFISH_MIN_VCPU: u16 = 4;
/// Cuttlefish L1-VM minimum memory (MiB) — 8 GiB.
const CUTTLEFISH_MIN_MEMORY_MB: u32 = 8192;
/// Cuttlefish L1-VM minimum root disk (GiB) — the Debian base + AOSP images.
const CUTTLEFISH_MIN_DISK_GB: u32 = 80;

/// Handle one `action/cloud/android-provision` request → a typed [`CloudReply`].
pub(super) fn handle(w: &CloudWorker, verb_name: &str, body: &CloudActionBody) -> CloudReply {
    let now_ms = now_unix_ms();
    let catalog = match crate::workers::android_catalog::load_admitted_catalog(&w.host, now_ms) {
        Ok(catalog) => catalog,
        Err(error) => {
            return refusal(
                verb_name,
                format!("Android release admission is unavailable: {error}"),
            )
        }
    };
    let artifact = configured_image_path();
    let provider_healthy = w.runner.probe_tool(runner::TOOL_LIBVIRT).state == HealthState::Up;
    build_reply(
        &w.state_root,
        verb_name,
        body,
        &catalog,
        artifact.as_deref(),
        w.android_host_probe.as_ref(),
        provider_healthy,
        now_ms,
    )
}

pub(super) fn authorization_target(body: &CloudActionBody) -> String {
    workload_name(body, body.node.trim())
}

/// Typed provider-side boundary for the inner Android guest.
///
/// Implementations return only the already-bounded Android inventory and launch
/// outcome types. [`dispatch_guest_request`] owns request admission, response
/// construction, and request/response correlation so an adapter cannot smuggle
/// a command, arbitrary intent, or response for another workload across this
/// seam.
pub(crate) trait AndroidGuestProvider: Send + Sync {
    /// Observe the complete governed starter inventory for one Android VM.
    fn inventory(&self, request: &AndroidGuestInventoryRequest) -> AndroidAppInventory;

    /// Dispatch the canonical launcher intent for one governed starter app.
    fn launch(&self, request: &AndroidGuestLaunchRequest) -> AndroidGuestLaunchOutcome;

    /// Return a truthful guest-owned display source for one generation.
    fn vdi_source(&self, _generation: u64) -> Option<AndroidVdiSource> {
        None
    }
}

/// The honest provider used until a Cuttlefish package-manager adapter is
/// installed. It gives Workloads a complete, typed pending inventory and never
/// claims that a guest launch occurred.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct UnconfiguredAndroidGuestProvider;

impl AndroidGuestProvider for UnconfiguredAndroidGuestProvider {
    fn inventory(&self, request: &AndroidGuestInventoryRequest) -> AndroidAppInventory {
        AndroidAppInventory::pending(request.workload_id.clone())
    }

    fn launch(&self, _request: &AndroidGuestLaunchRequest) -> AndroidGuestLaunchOutcome {
        AndroidGuestLaunchOutcome::Unavailable
    }
}

/// Dispatch one admitted guest request through a typed provider adapter.
///
/// The provider receives only an admitted closed request. The response envelope
/// is then built from that exact request and admitted against it again, making
/// workload/request/app identity and operation mismatches fail closed before a
/// caller can consume the result.
pub(super) fn dispatch_guest_request<P: AndroidGuestProvider + ?Sized>(
    provider: &P,
    request: AndroidGuestRequest,
) -> Result<AndroidGuestResponse, AndroidGuestBoundaryError> {
    let request = request.admitted()?;
    let response = match &request {
        AndroidGuestRequest::Inventory(request) => {
            AndroidGuestResponse::inventory(request, provider.inventory(request))?
        }
        AndroidGuestRequest::Launch(request) => {
            AndroidGuestResponse::launch(request, provider.launch(request))?
        }
    };
    response.admitted_against(&request)
}

/// Run a guest request through the current provider seam without touching a
/// live seat or host command runner. This is intentionally fail-closed until a
/// real Cuttlefish provider is wired into the cloud worker.
#[cfg(test)]
pub(super) fn handle_guest_request(
    request: AndroidGuestRequest,
) -> Result<AndroidGuestResponse, AndroidGuestBoundaryError> {
    dispatch_guest_request(&UnconfiguredAndroidGuestProvider, request)
}

/// Maximum number of process-local Android guest providers retained by the
/// registry. This deliberately matches the bounded inventory retention seam;
/// neither registry admission nor lookup creates an unbounded workload map.
pub(crate) const MAX_RETAINED_ANDROID_GUEST_PROVIDERS: usize = MAX_RETAINED_ANDROID_INVENTORIES;

/// Typed failure from the process-local Android guest provider registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AndroidGuestProviderRegistryError {
    /// The registry key is not a valid stable Android workload identity.
    InvalidWorkloadId { workload_id: String },
    /// A provider is already registered for this stable workload identity.
    DuplicateWorkloadId { workload_id: String },
    /// No provider is registered for this stable workload identity.
    MissingWorkloadId { workload_id: String },
    /// Registration would exceed the fixed process-local bound.
    CapacityExceeded { max_workloads: usize },
    /// The typed Cuttlefish adapter could not be admitted for this workload.
    CuttlefishAdapter(CuttlefishProviderError),
}

/// Bounded process-local provider selection keyed by validated Android VM
/// workload identity.
///
/// The registry owns only provider adapters. It does not discover guests,
/// execute commands, contact adb/Cuttlefish, or infer a provider from an
/// unvalidated request. A missing registration is intentionally handled by
/// [`AndroidGuestProviderRegistry::dispatch`] with the existing typed pending/
/// unavailable provider.
#[derive(Default)]
pub(crate) struct AndroidGuestProviderRegistry {
    providers: BTreeMap<String, Arc<dyn AndroidGuestProvider>>,
}

impl AndroidGuestProviderRegistry {
    /// Construct an empty process-local provider registry.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Register one provider under a validated stable Android workload id.
    pub(crate) fn register(
        &mut self,
        workload_id: impl Into<String>,
        provider: Arc<dyn AndroidGuestProvider>,
    ) -> Result<(), AndroidGuestProviderRegistryError> {
        let workload_id = workload_id.into();
        validate_provider_workload_id(&workload_id)?;
        if self.providers.contains_key(&workload_id) {
            return Err(AndroidGuestProviderRegistryError::DuplicateWorkloadId { workload_id });
        }
        if self.providers.len() >= MAX_RETAINED_ANDROID_GUEST_PROVIDERS {
            return Err(AndroidGuestProviderRegistryError::CapacityExceeded {
                max_workloads: MAX_RETAINED_ANDROID_GUEST_PROVIDERS,
            });
        }
        self.providers.insert(workload_id, provider);
        Ok(())
    }

    /// Admit one Cuttlefish provider adapter into the existing Workloads
    /// registry. Construction validates the stable VM identity, image
    /// provenance, package manifest, and retained generation before the
    /// adapter becomes reachable by CloudWorker or ActionWorker dispatch.
    pub(crate) fn register_cuttlefish_provider<C: CuttlefishProviderClient + 'static>(
        &mut self,
        workload_id: impl Into<String>,
        target: mackes_mesh_types::android_provider::CuttlefishVmTarget,
        package_manifest: mackes_mesh_types::android_apps::AndroidImagePackageManifest,
        observation: mackes_mesh_types::android_provider::CuttlefishVmObservation,
        client: C,
    ) -> Result<(), AndroidGuestProviderRegistryError> {
        let workload_id = workload_id.into();
        let provider = CuttlefishProviderAdapter::new(
            workload_id.clone(),
            target,
            package_manifest,
            observation,
            client,
        )
        .map_err(AndroidGuestProviderRegistryError::CuttlefishAdapter)?;
        self.register(workload_id, Arc::new(provider))
    }

    /// Find the provider for one validated stable Android workload id.
    pub(crate) fn provider(
        &self,
        workload_id: &str,
    ) -> Result<&dyn AndroidGuestProvider, AndroidGuestProviderRegistryError> {
        validate_provider_workload_id(workload_id)?;
        self.providers
            .get(workload_id)
            .map(Arc::as_ref)
            .ok_or_else(|| AndroidGuestProviderRegistryError::MissingWorkloadId {
                workload_id: workload_id.to_owned(),
            })
    }

    /// Remove a provider whose current production preflight no longer passes.
    pub(crate) fn unregister(&mut self, workload_id: &str) {
        self.providers.remove(workload_id);
    }

    /// Number of registered workload providers.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.providers.len()
    }

    /// Dispatch through the provider selected by the request's admitted
    /// workload identity. An absent registration deliberately falls back to
    /// the unconfigured provider so inventory remains pending and launches
    /// remain explicitly unavailable.
    pub(crate) fn dispatch(
        &self,
        request: AndroidGuestRequest,
    ) -> Result<AndroidGuestResponse, AndroidGuestBoundaryError> {
        let workload_id = request.workload_id().to_owned();
        let unconfigured = UnconfiguredAndroidGuestProvider;
        let provider: &dyn AndroidGuestProvider =
            self.provider(&workload_id).unwrap_or(&unconfigured);
        dispatch_guest_request(provider, request)
    }

    pub(crate) fn vdi_source(
        &self,
        workload_id: &str,
        generation: u64,
    ) -> Option<AndroidVdiSource> {
        self.provider(workload_id)
            .ok()
            .and_then(|provider| provider.vdi_source(generation))
    }
}

fn validate_provider_workload_id(
    workload_id: &str,
) -> Result<(), AndroidGuestProviderRegistryError> {
    AndroidGuestInventoryRequest::new("provider-registry", workload_id).map_or_else(
        |_| {
            Err(AndroidGuestProviderRegistryError::InvalidWorkloadId {
                workload_id: workload_id.to_owned(),
            })
        },
        |_| Ok(()),
    )
}

/// Maximum number of Android VM inventories retained by the pure provider seam.
///
/// This is deliberately a fixed bound until the future CloudState fold owns a
/// durable retention policy. Rejecting a new workload at the bound preserves
/// already-admitted evidence and makes capacity behavior deterministic.
pub(super) const MAX_RETAINED_ANDROID_INVENTORIES: usize = 32;

/// Result of admitting one validated Android inventory observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AndroidInventoryLedgerAdmission {
    /// This workload had no retained inventory and was inserted.
    Inserted,
    /// The response carried the same retained observation and changed nothing.
    Unchanged,
    /// A newer observed inventory replaced the retained evidence.
    Replaced,
}

/// Typed failure from the bounded Android inventory retention seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AndroidInventoryLedgerError {
    /// The existing closed Android request/response boundary rejected the
    /// response before it could enter retention.
    Boundary(AndroidGuestBoundaryError),
    /// The response would move a workload backwards, or conflicts with an
    /// already-retained observation at the same timestamp.
    Replay {
        /// Stable Android VM workload identity.
        workload_id: String,
        /// Timestamp of the retained observation, if one exists.
        retained_observed_at_unix_ms: Option<u64>,
        /// Timestamp of the received observation, if one exists.
        received_observed_at_unix_ms: Option<u64>,
    },
    /// A new workload cannot be admitted without exceeding the fixed bound.
    CapacityExceeded { max_workloads: usize },
    /// A retained ledger snapshot was malformed or outside its bounded schema.
    InvalidSnapshot { reason: String },
    /// The durable host-local ledger could not be read or atomically replaced.
    Persistence { path: String, reason: String },
    /// The worker's host-local ledger could not be accessed safely.
    MutexPoisoned,
}

/// Pure, bounded retention for admitted schema-v2 Android guest inventories.
///
/// The map is keyed by the existing stable `AndroidAppInventory::workload_id`
/// and stores no package, intent, command, guest-process, or transport data.
/// It is intentionally not wired to `CloudState`, a live provider, a shell,
/// adb, sockets, Cuttlefish, or any guest behavior; the future provider fold
/// can consume [`Self::snapshot`] once that integration is explicitly built.
#[derive(Debug, Clone, Default)]
pub(crate) struct AndroidInventoryLedger {
    inventories: BTreeMap<String, AndroidAppInventory>,
}

const ANDROID_INVENTORY_LEDGER_SCHEMA_VERSION: u16 = 1;
const MAX_ANDROID_INVENTORY_LEDGER_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AndroidInventoryLedgerFile {
    schema_version: u16,
    inventories: Vec<AndroidAppInventory>,
}

impl AndroidInventoryLedger {
    /// Construct an empty bounded inventory ledger.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Load a bounded host-local observation snapshot.
    ///
    /// A missing file is an ordinary first-boot state. Existing bytes are
    /// validated as complete Android inventories before entering the ledger;
    /// malformed, duplicated, stale-schema, or oversized snapshots fail closed
    /// instead of becoming user-visible evidence.
    pub(crate) fn load_from(path: &Path) -> Result<Self, AndroidInventoryLedgerError> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Self::new()),
            Err(error) => {
                return Err(persistence_error(path, error));
            }
        };
        if !metadata.file_type().is_file() {
            return Err(snapshot_error(format!(
                "{} is not a regular file",
                path.display()
            )));
        }
        if metadata.len() > MAX_ANDROID_INVENTORY_LEDGER_BYTES as u64 {
            return Err(snapshot_error(format!(
                "{} exceeds the {MAX_ANDROID_INVENTORY_LEDGER_BYTES}-byte bound",
                path.display()
            )));
        }

        let mut options = OpenOptions::new();
        options.read(true).custom_flags(libc::O_NOFOLLOW);
        let file = options
            .open(path)
            .map_err(|error| persistence_error(path, error))?;
        let opened = file
            .metadata()
            .map_err(|error| persistence_error(path, error))?;
        if opened.dev() != metadata.dev()
            || opened.ino() != metadata.ino()
            || opened.len() != metadata.len()
        {
            return Err(snapshot_error(format!(
                "{} changed before validation",
                path.display()
            )));
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        (&file)
            .take((MAX_ANDROID_INVENTORY_LEDGER_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| persistence_error(path, error))?;
        if bytes.len() > MAX_ANDROID_INVENTORY_LEDGER_BYTES {
            return Err(snapshot_error(format!(
                "{} exceeds the {MAX_ANDROID_INVENTORY_LEDGER_BYTES}-byte bound",
                path.display()
            )));
        }
        let closed = file
            .metadata()
            .map_err(|error| persistence_error(path, error))?;
        if closed.dev() != opened.dev()
            || closed.ino() != opened.ino()
            || closed.len() != opened.len()
        {
            return Err(snapshot_error(format!(
                "{} changed during validation",
                path.display()
            )));
        }

        let snapshot: AndroidInventoryLedgerFile = serde_json::from_slice(&bytes)
            .map_err(|error| snapshot_error(format!("{}: {error}", path.display())))?;
        if snapshot.schema_version != ANDROID_INVENTORY_LEDGER_SCHEMA_VERSION {
            return Err(snapshot_error(format!(
                "unsupported schema version {}",
                snapshot.schema_version
            )));
        }
        if snapshot.inventories.len() > MAX_RETAINED_ANDROID_INVENTORIES {
            return Err(snapshot_error(format!(
                "{} inventories exceed the {}-record bound",
                snapshot.inventories.len(),
                MAX_RETAINED_ANDROID_INVENTORIES
            )));
        }

        let mut ledger = Self::new();
        for inventory in snapshot.inventories {
            inventory
                .validate()
                .map_err(|error| snapshot_error(format!("invalid inventory: {error:?}")))?;
            if ledger
                .inventories
                .insert(inventory.workload_id.clone(), inventory)
                .is_some()
            {
                return Err(snapshot_error(
                    "duplicate workload identity in snapshot".to_owned(),
                ));
            }
        }
        Ok(ledger)
    }

    /// Persist the deterministic ledger snapshot with a bounded atomic replace.
    pub(crate) fn persist_to(&self, path: &Path) -> Result<(), AndroidInventoryLedgerError> {
        let snapshot = AndroidInventoryLedgerFile {
            schema_version: ANDROID_INVENTORY_LEDGER_SCHEMA_VERSION,
            inventories: self.snapshot(),
        };
        let bytes =
            serde_json::to_vec(&snapshot).map_err(|error| persistence_error(path, error))?;
        if bytes.len() > MAX_ANDROID_INVENTORY_LEDGER_BYTES {
            return Err(snapshot_error(format!(
                "{} exceeds the {MAX_ANDROID_INVENTORY_LEDGER_BYTES}-byte bound",
                path.display()
            )));
        }
        let parent = path.parent().ok_or_else(|| {
            persistence_error(path, "ledger path has no parent directory".to_owned())
        })?;
        fs::create_dir_all(parent).map_err(|error| persistence_error(path, error))?;

        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                persistence_error(path, "ledger path has no valid filename".to_owned())
            })?;
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let temporary = parent.join(format!(
            ".{file_name}.{}.{}.tmp",
            std::process::id(),
            timestamp
        ));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|error| persistence_error(path, error))?;
            file.write_all(&bytes)
                .map_err(|error| persistence_error(path, error))?;
            file.sync_all()
                .map_err(|error| persistence_error(path, error))?;
            drop(file);
            fs::rename(&temporary, path).map_err(|error| persistence_error(path, error))?;
            let directory = File::open(parent).map_err(|error| persistence_error(path, error))?;
            directory
                .sync_all()
                .map_err(|error| persistence_error(path, error))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    /// Admit one correlated inventory response from a typed request.
    ///
    /// Boundary validation happens before capacity or ordering decisions. The
    /// schema-v2 observation timestamp is the only monotonic evidence key
    /// available in the existing contract: equal timestamps are idempotent
    /// only when the complete inventory is equal, newer observations replace,
    /// and older/conflicting observations are rejected as replay.
    pub(crate) fn admit_response(
        &mut self,
        request: &AndroidGuestInventoryRequest,
        response: AndroidGuestInventoryResponse,
    ) -> Result<AndroidInventoryLedgerAdmission, AndroidInventoryLedgerError> {
        let response = AndroidGuestResponse::Inventory(response)
            .admitted_against(&AndroidGuestRequest::Inventory(request.clone()))
            .map_err(AndroidInventoryLedgerError::Boundary)?;
        let AndroidGuestResponse::Inventory(response) = response else {
            unreachable!("inventory response was admitted against an inventory request")
        };
        let workload_id = response.workload_id;
        let inventory = response.inventory;

        if !self.inventories.contains_key(&workload_id) {
            if self.inventories.len() >= MAX_RETAINED_ANDROID_INVENTORIES {
                return Err(AndroidInventoryLedgerError::CapacityExceeded {
                    max_workloads: MAX_RETAINED_ANDROID_INVENTORIES,
                });
            }
            self.inventories.insert(workload_id, inventory);
            return Ok(AndroidInventoryLedgerAdmission::Inserted);
        }

        let (retained_observed_at_unix_ms, received_observed_at_unix_ms, decision) = {
            let retained = self
                .inventories
                .get(&workload_id)
                .expect("workload presence checked above");
            let retained_observed_at_unix_ms = retained.observed_at_unix_ms;
            let received_observed_at_unix_ms = inventory.observed_at_unix_ms;
            let decision = match (retained_observed_at_unix_ms, received_observed_at_unix_ms) {
                // A pending response carries no observation key. It cannot
                // roll back or improve another pending response.
                (None, None) => AndroidInventoryLedgerAdmission::Unchanged,
                // The first actual observation improves a pending placeholder.
                (None, Some(_)) => AndroidInventoryLedgerAdmission::Replaced,
                // A pending response cannot erase admitted observed evidence.
                (Some(_), None) => {
                    return Err(AndroidInventoryLedgerError::Replay {
                        workload_id: workload_id.clone(),
                        retained_observed_at_unix_ms,
                        received_observed_at_unix_ms,
                    });
                }
                (Some(retained_at), Some(received_at)) if received_at > retained_at => {
                    AndroidInventoryLedgerAdmission::Replaced
                }
                (Some(retained_at), Some(received_at))
                    if received_at == retained_at && retained == &inventory =>
                {
                    AndroidInventoryLedgerAdmission::Unchanged
                }
                // Equal-timestamp conflicts and older observations are both
                // replay/rollback attempts; neither may replace evidence.
                _ => {
                    return Err(AndroidInventoryLedgerError::Replay {
                        workload_id: workload_id.clone(),
                        retained_observed_at_unix_ms,
                        received_observed_at_unix_ms,
                    });
                }
            };
            (
                retained_observed_at_unix_ms,
                received_observed_at_unix_ms,
                decision,
            )
        };

        match decision {
            AndroidInventoryLedgerAdmission::Replaced => {
                self.inventories.insert(workload_id, inventory);
                Ok(AndroidInventoryLedgerAdmission::Replaced)
            }
            AndroidInventoryLedgerAdmission::Unchanged => {
                debug_assert!(
                    retained_observed_at_unix_ms == received_observed_at_unix_ms
                        || (retained_observed_at_unix_ms.is_none()
                            && received_observed_at_unix_ms.is_none())
                );
                Ok(AndroidInventoryLedgerAdmission::Unchanged)
            }
            AndroidInventoryLedgerAdmission::Inserted => {
                unreachable!("existing workload cannot produce an inserted admission")
            }
        }
    }

    /// Return a deterministic workload-id-sorted snapshot for a future fold.
    #[must_use]
    pub(crate) fn snapshot(&self) -> Vec<AndroidAppInventory> {
        self.inventories.values().cloned().collect()
    }

    /// Number of retained workload inventories.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.inventories.len()
    }
}

fn snapshot_error(reason: String) -> AndroidInventoryLedgerError {
    AndroidInventoryLedgerError::InvalidSnapshot { reason }
}

fn persistence_error(path: &Path, error: impl std::fmt::Display) -> AndroidInventoryLedgerError {
    AndroidInventoryLedgerError::Persistence {
        path: path.display().to_string(),
        reason: error.to_string(),
    }
}

/// Admit a supplied Android image manifest and bind its immutable provenance to
/// the outer Android workload declaration.
///
/// This is deliberately pure: it validates the closed image manifest, then
/// copies only its immutable identity and digest into the existing
/// [`WorkloadSpec`] fields. The production `android-provision` path supplies the
/// manifest from the re-verified durable signed catalog; request fields never
/// select or override it.
pub(super) fn android_spec_from_manifest(
    node: &str,
    name: &str,
    manifest: AndroidImageManifest,
) -> Result<WorkloadSpec, AndroidAppContractError> {
    let manifest = manifest.admitted()?;
    let mut spec = android_spec(node, name);
    spec.image = Some(manifest.image_id);
    spec.image_digest = Some(manifest.image_digest);
    Ok(spec)
}

/// Construct and persist a Cuttlefish desired definition only after the signed
/// release catalog, immutable artifact bytes, package manifest, host capacity,
/// and provider have formed one ready admission.
#[allow(clippy::too_many_arguments)]
fn build_reply(
    state_root: &Path,
    verb_name: &str,
    body: &CloudActionBody,
    catalog: &AndroidSignedCatalog,
    artifact: Option<&Path>,
    host_probe: &dyn AndroidHostProbe,
    provider_healthy: bool,
    now_ms: u64,
) -> CloudReply {
    let node = body.node.trim();
    if node.is_empty() {
        return refusal(
            verb_name,
            format!("`{verb_name}` requires a placement `node` for the Cuttlefish Android VM"),
        );
    }
    let name = workload_name(body, node);
    let mut spec =
        match android_spec_from_manifest(node, &name, catalog.payload.image_manifest.clone()) {
            Ok(spec) => spec,
            Err(error) => {
                return refusal(
                    verb_name,
                    format!("signed Android image manifest is invalid: {error:?}"),
                )
            }
        };
    for policy in &catalog.payload.app_policies {
        spec.vcpu = spec.vcpu.max(u16::from(policy.resources.vcpus));
        spec.memory_mb = spec.memory_mb.max(policy.resources.memory_mib);
        spec.disk_gb = spec
            .disk_gb
            .max(policy.resources.disk_mib.saturating_add(1023) / 1024);
    }
    let package_manifest = &catalog.payload.package_manifest;
    let admission = preflight(
        AndroidPreflightInput {
            workload: &spec,
            catalog: Some(catalog),
            package_manifest: Some(package_manifest),
            artifact,
            provider_healthy,
            now_unix_ms: now_ms,
        },
        host_probe,
    );
    if !admission.is_ready() {
        return refusal(
            verb_name,
            format!(
                "Android desired-state admission refused: {:?}",
                admission.refusal
            ),
        );
    }

    let existing = match reconcile::read_desired_doc_strict(state_root, node, &name) {
        Ok(existing) => existing,
        Err(error) => {
            return refusal(
                verb_name,
                format!("could not inspect existing Android desired state: {error}"),
            )
        }
    };
    if existing.as_ref().is_some_and(|existing| existing != &spec) {
        return refusal(
            verb_name,
            format!(
                "Android workload `{name}` already has a different desired-state definition; provenance replacement requires an explicit lifecycle transition"
            ),
        );
    }

    // Stage the exact package manifest first. If the desired write then fails,
    // the orphaned manifest authorizes nothing; the inverse ordering could leave
    // a startable desired row without its signed release provenance.
    if let Err(error) = persist_package_manifest(state_root, &name, package_manifest) {
        return refusal(
            verb_name,
            format!("could not persist admitted Android package provenance: {error}"),
        );
    }

    match reconcile::write_desired_doc(state_root, &spec) {
        Ok(()) => CloudReply {
            ok: true,
            verb: verb_name.to_string(),
            desired: Some(vec![spec]),
            ..Default::default()
        },
        Err(e) => CloudReply {
            ok: false,
            verb: verb_name.to_string(),
            error: Some(format!(
                "android-provision built the Cuttlefish Android VM `{name}` on `{node}` \
                 but could not persist its desired slice: {e}"
            )),
            desired: Some(vec![spec]),
            ..Default::default()
        },
    }
}

const ANDROID_PACKAGE_MANIFEST_MAX_BYTES: usize = 64 * 1024;

fn persist_package_manifest(
    state_root: &Path,
    workload_id: &str,
    manifest: &AndroidImagePackageManifest,
) -> Result<(), String> {
    manifest
        .validate()
        .map_err(|error| format!("invalid package manifest: {error:?}"))?;
    let stem = super::super::path_key::file_stem("workload", workload_id, ".json")?;
    let parent = state_root.join("mcnf/cloud/android-manifests");
    ensure_directory_chain_nofollow(&parent)?;
    fs::create_dir_all(&parent).map_err(|error| format!("create {}: {error}", parent.display()))?;
    ensure_directory_chain_nofollow(&parent)?;

    let body = serde_json::to_vec(manifest)
        .map_err(|error| format!("encode Android package manifest: {error}"))?;
    if body.is_empty() || body.len() > ANDROID_PACKAGE_MANIFEST_MAX_BYTES {
        return Err(format!(
            "Android package manifest exceeds the {ANDROID_PACKAGE_MANIFEST_MAX_BYTES}-byte bound"
        ));
    }
    let destination = parent.join(format!("{stem}.json"));
    if fs::symlink_metadata(&destination).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(format!(
            "Android package manifest path {} is a symlink",
            destination.display()
        ));
    }
    let temporary = parent.join(format!(
        ".{stem}.{}.{}.tmp",
        std::process::id(),
        rand::random::<u64>()
    ));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // Linux O_NOFOLLOW; keep this crate free of a direct libc edge.
            options.mode(0o600).custom_flags(0o400_000);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|error| format!("create {}: {error}", temporary.display()))?;
        file.write_all(&body)
            .map_err(|error| format!("write {}: {error}", temporary.display()))?;
        file.sync_all()
            .map_err(|error| format!("sync {}: {error}", temporary.display()))?;
        drop(file);
        fs::rename(&temporary, &destination).map_err(|error| {
            format!(
                "replace Android package manifest {}: {error}",
                destination.display()
            )
        })?;
        File::open(&parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("sync {}: {error}", parent.display()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn ensure_directory_chain_nofollow(path: &Path) -> Result<(), String> {
    let mut current = std::path::PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        let Ok(metadata) = fs::symlink_metadata(&current) else {
            continue;
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(format!(
                "Android package manifest parent {} is not a real directory",
                current.display()
            ));
        }
    }
    Ok(())
}

fn refusal(verb_name: &str, error: impl Into<String>) -> CloudReply {
    CloudReply {
        ok: false,
        verb: verb_name.to_owned(),
        error: Some(error.into()),
        ..Default::default()
    }
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// The workload name — the request's `name`, else a stable `android-<node>` default.
fn workload_name(body: &CloudActionBody, node: &str) -> String {
    body.name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(|| format!("android-{node}"), ToString::to_string)
}

/// Build the [`DeliveryType::AndroidVm`] L1-VM spec, sized for Cuttlefish. Pure +
/// directly tested — the load-bearing deliverable of this unit.
#[must_use]
pub(super) fn android_spec(node: &str, name: &str) -> WorkloadSpec {
    WorkloadSpec {
        name: name.to_string(),
        delivery_type: DeliveryType::AndroidVm,
        node: node.to_string(),
        vcpu: CUTTLEFISH_MIN_VCPU,
        memory_mb: CUTTLEFISH_MIN_MEMORY_MB,
        disk_gb: CUTTLEFISH_MIN_DISK_GB,
        storage_pool: mackes_mesh_types::cloud::StoragePool::default(),
        // The `modules/android` golden Debian base (or Android-x86 on the fallback
        // path) — the delivery type's default, not an operator override here.
        image: None,
        image_digest: None,
        network_isolation: false,
        raw_hcl: None,
        app: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::cloud::android_provider::AndroidHostFacts;
    use ed25519_dalek::SigningKey;
    use mackes_mesh_types::android_apps::{
        AndroidAppAvailability, AndroidAppCapability, AndroidAppPermission, AndroidAppReadiness,
        AndroidCatalogAppPolicy, AndroidCatalogGuestReadiness, AndroidCatalogPayload,
        AndroidGuestBootState, AndroidGuestInventoryResponse, AndroidImagePackage,
        AndroidImageProvenance, AndroidLaunchReadiness, AndroidLauncherResolvability,
        AndroidPackageVersion, AndroidResourceClass, AndroidResourceProfile, AospStarterApp,
        AospStarterCatalog, ANDROID_SIGNED_CATALOG_SCHEMA_VERSION,
    };
    use std::io;
    use std::sync::Arc;
    use tempfile::tempdir;

    const NOW: u64 = 1_800_000_000_000;
    const ANDROID_IMAGE_DIGEST: &str =
        "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn valid_android_image_manifest() -> AndroidImageManifest {
        AndroidImageManifest::new(
            "android_vm-golden",
            ANDROID_IMAGE_DIGEST,
            "aosp-source-2026-08",
            "starter-catalog-v1",
            1_786_000_000_000,
            1_786_000_000_100,
            AospStarterCatalog::v1(),
        )
        .expect("valid Android image manifest")
    }

    fn admitted_catalog() -> AndroidSignedCatalog {
        let image_manifest = valid_android_image_manifest();
        let image_provenance =
            AndroidImageProvenance::from_manifest(&image_manifest).expect("valid image provenance");
        let package_manifest = AndroidImagePackageManifest::new(
            image_provenance,
            AospStarterApp::ALL
                .into_iter()
                .map(|app| {
                    AndroidImagePackage::for_app(
                        app,
                        AndroidPackageVersion::new("2026.08.11", 1).expect("valid package version"),
                    )
                })
                .collect(),
        )
        .expect("valid package manifest");
        let app_policies = AospStarterApp::ALL
            .into_iter()
            .map(|app| AndroidCatalogAppPolicy {
                app,
                permissions: vec![AndroidAppPermission::Network],
                capabilities: vec![AndroidAppCapability::VdiDisplay],
                resources: AndroidResourceProfile {
                    class: AndroidResourceClass::Standard,
                    vcpus: 4,
                    memory_mib: 8_192,
                    disk_mib: 80 * 1_024,
                },
                guest_readiness: AndroidCatalogGuestReadiness::BootedInventoryAndLauncherReady,
            })
            .collect();
        let key = SigningKey::from_bytes(&[19; 32]);
        AndroidSignedCatalog::sign(
            "android-release-v1",
            AndroidCatalogPayload {
                schema_version: ANDROID_SIGNED_CATALOG_SCHEMA_VERSION,
                catalog_id: "android-production".into(),
                revision: 7,
                issued_at_unix_ms: NOW - 1_000,
                expires_at_unix_ms: NOW + 60_000,
                image_manifest,
                package_manifest,
                app_policies,
            },
            &key,
        )
        .expect("signed catalog")
        .admit("android-release-v1", &key.verifying_key(), NOW)
        .expect("admitted signed catalog")
    }

    struct ReadyProbe {
        digest: String,
    }

    impl AndroidHostProbe for ReadyProbe {
        fn facts(&self, _artifact: Option<&Path>) -> AndroidHostFacts {
            AndroidHostFacts {
                kvm_available: true,
                nested_virtualization: true,
                available_vcpus: 16,
                available_memory_mib: 32 * 1_024,
                available_disk_mib: 256 * 1_024,
            }
        }

        fn image_digest(&self, _artifact: &Path) -> io::Result<String> {
            Ok(self.digest.clone())
        }
    }

    fn ready_probe() -> ReadyProbe {
        ReadyProbe {
            digest: ANDROID_IMAGE_DIGEST.into(),
        }
    }

    fn inventory_request(request_id: &str, workload_id: &str) -> AndroidGuestInventoryRequest {
        AndroidGuestInventoryRequest::new(request_id, workload_id)
            .expect("canonical inventory request")
    }

    fn observed_inventory(workload_id: &str, observed_at_unix_ms: u64) -> AndroidAppInventory {
        let mut inventory = AndroidAppInventory::pending(workload_id.to_owned());
        inventory.guest_boot_state = AndroidGuestBootState::Ready;
        inventory.image_provenance = Some(
            AndroidImageProvenance::from_manifest(&valid_android_image_manifest())
                .expect("valid Android image provenance"),
        );
        inventory.observed_at_unix_ms = Some(observed_at_unix_ms);
        inventory.observation_age_ms = Some(0);
        for entry in &mut inventory.entries {
            entry.availability = AndroidAppAvailability::Installed;
            entry.package_version =
                Some(AndroidPackageVersion::new("1.0.0", 1).expect("valid package version"));
            entry.readiness = AndroidAppReadiness::Ready;
            entry.launcher_resolvability = AndroidLauncherResolvability::Resolved;
            entry.launch_readiness = AndroidLaunchReadiness::IntegrationPending;
        }
        inventory.entries[0].launch_readiness = AndroidLaunchReadiness::Ready;
        inventory
    }

    fn inventory_response(
        request: &AndroidGuestInventoryRequest,
        inventory: AndroidAppInventory,
    ) -> AndroidGuestInventoryResponse {
        AndroidGuestInventoryResponse::new(request, inventory).expect("valid inventory response")
    }

    fn body(node: &str, name: Option<&str>) -> CloudActionBody {
        CloudActionBody {
            node: node.to_string(),
            name: name.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn android_spec_is_an_androidvm_sized_for_cuttlefish() {
        let spec = android_spec("eagle", "droid-1");
        assert_eq!(spec.delivery_type, DeliveryType::AndroidVm);
        assert_eq!(spec.name, "droid-1");
        assert_eq!(spec.node, "eagle");
        // Cuttlefish nested-KVM minimums (≥4 vcpu / ≥8 GiB / ≥80 GiB).
        assert!(spec.vcpu >= 4, "vcpu {}", spec.vcpu);
        assert!(spec.memory_mb >= 8192, "mem {}", spec.memory_mb);
        assert!(spec.disk_gb >= 80, "disk {}", spec.disk_gb);
        assert!(spec.image.is_none());
        assert!(!spec.network_isolation);
    }

    #[test]
    fn admitted_android_manifest_binds_image_identity_and_digest_to_the_workload() {
        let spec = android_spec_from_manifest("eagle", "droid-1", valid_android_image_manifest())
            .expect("valid Android provenance");

        assert_eq!(spec.delivery_type, DeliveryType::AndroidVm);
        assert_eq!(spec.image.as_deref(), Some("android_vm-golden"));
        assert_eq!(spec.image_digest.as_deref(), Some(ANDROID_IMAGE_DIGEST));
        assert_eq!(spec.name, "droid-1");
        assert_eq!(spec.node, "eagle");
    }

    #[test]
    fn android_manifest_admission_rejects_hostile_digest_identity_and_catalog() {
        let mut malformed_digest = valid_android_image_manifest();
        malformed_digest.image_digest = "sha256:not-a-digest".to_owned();
        assert_eq!(
            android_spec_from_manifest("eagle", "droid-1", malformed_digest),
            Err(AndroidAppContractError::InvalidImageDigest)
        );

        let mut zero_digest = valid_android_image_manifest();
        zero_digest.image_digest = format!("sha256:{}", "0".repeat(64));
        assert_eq!(
            android_spec_from_manifest("eagle", "droid-1", zero_digest),
            Err(AndroidAppContractError::InvalidImageDigest)
        );

        let mut unsafe_digest = valid_android_image_manifest();
        unsafe_digest.image_digest = format!("sha256:{}{}", "a".repeat(63), "/");
        assert_eq!(
            android_spec_from_manifest("eagle", "droid-1", unsafe_digest),
            Err(AndroidAppContractError::InvalidImageDigest)
        );

        let mut unsafe_identity = valid_android_image_manifest();
        unsafe_identity.image_id = "../android_vm-golden".to_owned();
        assert_eq!(
            android_spec_from_manifest("eagle", "droid-1", unsafe_identity),
            Err(AndroidAppContractError::InvalidImageIdentity)
        );

        let mut mismatched_catalog = valid_android_image_manifest();
        mismatched_catalog.catalog.entries[1] = mismatched_catalog.catalog.entries[0];
        assert_eq!(
            android_spec_from_manifest("eagle", "droid-1", mismatched_catalog),
            Err(AndroidAppContractError::DuplicateApp(
                AospStarterApp::Browser
            ))
        );
    }

    #[test]
    fn a_request_without_a_placement_node_is_honestly_rejected() {
        let tmp = tempdir().unwrap();
        let catalog = admitted_catalog();
        let reply = build_reply(
            tmp.path(),
            "android-provision",
            &body("", None),
            &catalog,
            Some(Path::new("/android.img")),
            &ready_probe(),
            true,
            NOW,
        );
        assert!(!reply.ok);
        assert!(reply.desired.is_none());
        assert!(reply.error.unwrap().contains("placement `node`"));
    }

    #[test]
    fn signed_release_provenance_gates_the_persisted_android_definition() {
        let tmp = tempdir().unwrap();
        let catalog = admitted_catalog();
        let reply = build_reply(
            tmp.path(),
            "android-provision",
            &body("eagle", Some("droid")),
            &catalog,
            Some(Path::new("/android.img")),
            &ready_probe(),
            true,
            NOW,
        );
        assert!(reply.ok, "err: {:?}", reply.error);
        let desired = reply.desired.expect("echoed spec");
        assert_eq!(desired.len(), 1);
        assert_eq!(desired[0].name, "droid");
        assert_eq!(desired[0].delivery_type, DeliveryType::AndroidVm);
        assert_eq!(
            desired[0].image.as_deref(),
            Some(catalog.payload.image_manifest.image_id.as_str())
        );
        assert_eq!(
            desired[0].image_digest.as_deref(),
            Some(catalog.payload.image_manifest.image_digest.as_str())
        );
        let slice = reconcile::read_desired_slice(tmp.path(), "eagle");
        assert_eq!(slice.len(), 1);
        assert_eq!(slice[0], desired[0]);
        let manifest: AndroidImagePackageManifest = serde_json::from_slice(
            &fs::read(tmp.path().join("mcnf/cloud/android-manifests/droid.json"))
                .expect("persisted package manifest"),
        )
        .expect("valid persisted package manifest");
        assert_eq!(manifest, catalog.payload.package_manifest);

        let wrong_artifact = ReadyProbe {
            digest: format!("sha256:{}", "f".repeat(64)),
        };
        let refused = build_reply(
            tmp.path(),
            "android-provision",
            &body("eagle", Some("untrusted")),
            &catalog,
            Some(Path::new("/substituted.img")),
            &wrong_artifact,
            true,
            NOW,
        );
        assert!(!refused.ok);
        assert!(
            reconcile::read_desired_doc_strict(tmp.path(), "eagle", "untrusted")
                .expect("strict desired read")
                .is_none()
        );
    }

    #[test]
    fn a_default_named_request_uses_the_stable_android_node_name() {
        let tmp = tempdir().unwrap();
        let catalog = admitted_catalog();
        let reply = build_reply(
            tmp.path(),
            "android-provision",
            &body("eagle", None),
            &catalog,
            Some(Path::new("/android.img")),
            &ready_probe(),
            true,
            NOW,
        );
        assert!(reply.ok, "err: {:?}", reply.error);
        let desired = reply.desired.expect("echoed spec");
        assert_eq!(desired[0].name, "android-eagle", "default name");
    }

    #[test]
    fn inventory_ledger_admits_pending_then_observed_inventory() {
        let mut ledger = AndroidInventoryLedger::new();
        let pending_request = inventory_request("inventory-pending", "android-eagle");
        assert_eq!(
            ledger.admit_response(
                &pending_request,
                inventory_response(
                    &pending_request,
                    AndroidAppInventory::pending("android-eagle"),
                ),
            ),
            Ok(AndroidInventoryLedgerAdmission::Inserted)
        );

        let observed_request = inventory_request("inventory-observed", "android-eagle");
        assert_eq!(
            ledger.admit_response(
                &observed_request,
                inventory_response(
                    &observed_request,
                    observed_inventory("android-eagle", 1_786_000_000_000),
                ),
            ),
            Ok(AndroidInventoryLedgerAdmission::Replaced)
        );
        let snapshot = ledger.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].workload_id, "android-eagle");
        assert_eq!(snapshot[0].observed_at_unix_ms, Some(1_786_000_000_000));
    }

    #[test]
    fn inventory_ledger_treats_the_same_observation_as_idempotent() {
        let mut ledger = AndroidInventoryLedger::new();
        let first_request = inventory_request("inventory-first", "android-eagle");
        let second_request = inventory_request("inventory-retry", "android-eagle");
        let observed_at_unix_ms = 1_786_000_000_000;

        assert_eq!(
            ledger.admit_response(
                &first_request,
                inventory_response(
                    &first_request,
                    observed_inventory("android-eagle", observed_at_unix_ms),
                ),
            ),
            Ok(AndroidInventoryLedgerAdmission::Inserted)
        );
        assert_eq!(
            ledger.admit_response(
                &second_request,
                inventory_response(
                    &second_request,
                    observed_inventory("android-eagle", observed_at_unix_ms),
                ),
            ),
            Ok(AndroidInventoryLedgerAdmission::Unchanged)
        );
        assert_eq!(ledger.len(), 1);
        assert_eq!(
            ledger.snapshot()[0].observed_at_unix_ms,
            Some(observed_at_unix_ms)
        );
    }

    #[test]
    fn inventory_ledger_replaces_only_with_a_newer_observation() {
        let mut ledger = AndroidInventoryLedger::new();
        let old_request = inventory_request("inventory-old", "android-eagle");
        let new_request = inventory_request("inventory-new", "android-eagle");
        assert_eq!(
            ledger.admit_response(
                &old_request,
                inventory_response(&old_request, observed_inventory("android-eagle", 1_000),),
            ),
            Ok(AndroidInventoryLedgerAdmission::Inserted)
        );
        assert_eq!(
            ledger.admit_response(
                &new_request,
                inventory_response(&new_request, observed_inventory("android-eagle", 2_000),),
            ),
            Ok(AndroidInventoryLedgerAdmission::Replaced)
        );
        assert_eq!(ledger.snapshot()[0].observed_at_unix_ms, Some(2_000));
    }

    #[test]
    fn inventory_ledger_rejects_rollback_and_pending_replay() {
        let mut ledger = AndroidInventoryLedger::new();
        let current_request = inventory_request("inventory-current", "android-eagle");
        ledger
            .admit_response(
                &current_request,
                inventory_response(&current_request, observed_inventory("android-eagle", 2_000)),
            )
            .expect("current observation admitted");

        let older_request = inventory_request("inventory-older", "android-eagle");
        assert_eq!(
            ledger.admit_response(
                &older_request,
                inventory_response(&older_request, observed_inventory("android-eagle", 1_000),),
            ),
            Err(AndroidInventoryLedgerError::Replay {
                workload_id: "android-eagle".to_owned(),
                retained_observed_at_unix_ms: Some(2_000),
                received_observed_at_unix_ms: Some(1_000),
            })
        );

        let pending_request = inventory_request("inventory-pending-replay", "android-eagle");
        assert_eq!(
            ledger.admit_response(
                &pending_request,
                inventory_response(
                    &pending_request,
                    AndroidAppInventory::pending("android-eagle"),
                ),
            ),
            Err(AndroidInventoryLedgerError::Replay {
                workload_id: "android-eagle".to_owned(),
                retained_observed_at_unix_ms: Some(2_000),
                received_observed_at_unix_ms: None,
            })
        );
        assert_eq!(ledger.snapshot()[0].observed_at_unix_ms, Some(2_000));
    }

    #[test]
    fn inventory_ledger_rejects_correlation_workload_and_inventory_failures() {
        let mut ledger = AndroidInventoryLedger::new();
        let request = inventory_request("inventory-request", "android-eagle");
        let wrong_request = inventory_request("inventory-other-request", "android-eagle");
        assert_eq!(
            ledger.admit_response(
                &wrong_request,
                inventory_response(&request, AndroidAppInventory::pending("android-eagle"),),
            ),
            Err(AndroidInventoryLedgerError::Boundary(
                AndroidGuestBoundaryError::RequestResponseMismatch
            ))
        );

        let mut wrong_workload =
            inventory_response(&request, AndroidAppInventory::pending("android-eagle"));
        wrong_workload.inventory.workload_id = "android-other".to_owned();
        assert_eq!(
            ledger.admit_response(&request, wrong_workload),
            Err(AndroidInventoryLedgerError::Boundary(
                AndroidGuestBoundaryError::InventoryWorkloadMismatch
            ))
        );

        let mut invalid_inventory =
            inventory_response(&request, AndroidAppInventory::pending("android-eagle"));
        invalid_inventory.inventory.schema_version = 1;
        assert_eq!(
            ledger.admit_response(&request, invalid_inventory),
            Err(AndroidInventoryLedgerError::Boundary(
                AndroidGuestBoundaryError::InvalidInventory(
                    AndroidAppContractError::UnsupportedSchema(1)
                )
            ))
        );
        assert!(ledger.snapshot().is_empty());
    }

    #[test]
    fn inventory_ledger_snapshot_is_sorted_and_capacity_is_bounded() {
        let mut ledger = AndroidInventoryLedger::new();
        for (index, workload_id) in ["android-zeta", "android-alpha", "android-middle"]
            .into_iter()
            .enumerate()
        {
            let request = inventory_request(&format!("inventory-sort-{index}"), workload_id);
            assert_eq!(
                ledger.admit_response(
                    &request,
                    inventory_response(&request, AndroidAppInventory::pending(workload_id)),
                ),
                Ok(AndroidInventoryLedgerAdmission::Inserted)
            );
        }
        let sorted_ids: Vec<_> = ledger
            .snapshot()
            .into_iter()
            .map(|inventory| inventory.workload_id)
            .collect();
        assert_eq!(
            sorted_ids,
            vec![
                "android-alpha".to_owned(),
                "android-middle".to_owned(),
                "android-zeta".to_owned()
            ]
        );

        for index in 0..(MAX_RETAINED_ANDROID_INVENTORIES - ledger.len()) {
            let workload_id = format!("android-capacity-{index:02}");
            let request = inventory_request(&format!("inventory-capacity-{index}"), &workload_id);
            ledger
                .admit_response(
                    &request,
                    inventory_response(&request, AndroidAppInventory::pending(workload_id)),
                )
                .expect("capacity slot admitted");
        }
        assert_eq!(ledger.len(), MAX_RETAINED_ANDROID_INVENTORIES);

        let over_capacity_request = inventory_request("inventory-over-capacity", "android-over");
        assert_eq!(
            ledger.admit_response(
                &over_capacity_request,
                inventory_response(
                    &over_capacity_request,
                    AndroidAppInventory::pending("android-over"),
                ),
            ),
            Err(AndroidInventoryLedgerError::CapacityExceeded {
                max_workloads: MAX_RETAINED_ANDROID_INVENTORIES,
            })
        );
        assert_eq!(ledger.len(), MAX_RETAINED_ANDROID_INVENTORIES);
    }

    #[test]
    fn unconfigured_provider_returns_a_complete_pending_inventory() {
        let request = AndroidGuestRequest::inventory("inventory-01", "android-eagle")
            .expect("canonical inventory request");
        let response = handle_guest_request(request.clone()).expect("typed response");
        assert!(response.validate_against(&request).is_ok());
        match response {
            AndroidGuestResponse::Inventory(response) => {
                assert_eq!(response.workload_id, "android-eagle");
                assert!(response.inventory.validate().is_ok());
                assert_eq!(response.inventory.entries.len(), 9);
                assert!(response
                    .inventory
                    .entries
                    .iter()
                    .all(|entry| !entry.is_launchable()));
            }
            AndroidGuestResponse::Launch(_) => panic!("inventory request returned launch response"),
        }
    }

    #[test]
    fn unconfigured_provider_returns_an_explicitly_unavailable_launch() {
        let request =
            AndroidGuestRequest::launch("launch-01", "android-eagle", AospStarterApp::Browser)
                .expect("canonical launch request");
        let response = handle_guest_request(request.clone()).expect("typed response");
        assert!(response.validate_against(&request).is_ok());
        match response {
            AndroidGuestResponse::Launch(response) => {
                assert_eq!(response.app, AospStarterApp::Browser);
                assert_eq!(
                    response.outcome,
                    AndroidGuestLaunchOutcome::Unavailable,
                    "the seam must not claim a live guest launch"
                );
            }
            AndroidGuestResponse::Inventory(_) => {
                panic!("launch request returned inventory response")
            }
        }
    }

    struct ReadyTestProvider;

    impl AndroidGuestProvider for ReadyTestProvider {
        fn inventory(&self, request: &AndroidGuestInventoryRequest) -> AndroidAppInventory {
            observed_inventory(&request.workload_id, 1_786_000_000_000)
        }

        fn launch(&self, _request: &AndroidGuestLaunchRequest) -> AndroidGuestLaunchOutcome {
            AndroidGuestLaunchOutcome::Started
        }
    }

    #[test]
    fn dispatcher_can_admit_a_real_provider_response_without_widening_the_contract() {
        let provider = ReadyTestProvider;
        let inventory_request = AndroidGuestRequest::inventory("inventory-02", "android-eagle")
            .expect("canonical inventory request");
        let inventory_response =
            dispatch_guest_request(&provider, inventory_request.clone()).expect("inventory");
        assert!(inventory_response
            .validate_against(&inventory_request)
            .is_ok());
        match inventory_response {
            AndroidGuestResponse::Inventory(response) => {
                assert!(response.inventory.entries[0].is_launchable());
            }
            AndroidGuestResponse::Launch(_) => panic!("inventory request returned launch response"),
        }

        let launch_request =
            AndroidGuestRequest::launch("launch-02", "android-eagle", AospStarterApp::Browser)
                .expect("canonical launch request");
        let launch_response =
            dispatch_guest_request(&provider, launch_request.clone()).expect("launch");
        assert!(launch_response.validate_against(&launch_request).is_ok());
        match launch_response {
            AndroidGuestResponse::Launch(response) => {
                assert_eq!(response.outcome, AndroidGuestLaunchOutcome::Started);
            }
            AndroidGuestResponse::Inventory(_) => {
                panic!("launch request returned inventory response")
            }
        }
    }

    struct WrongWorkloadProvider;

    impl AndroidGuestProvider for WrongWorkloadProvider {
        fn inventory(&self, _request: &AndroidGuestInventoryRequest) -> AndroidAppInventory {
            AndroidAppInventory::pending("android-other")
        }

        fn launch(&self, _request: &AndroidGuestLaunchRequest) -> AndroidGuestLaunchOutcome {
            AndroidGuestLaunchOutcome::Unavailable
        }
    }

    #[test]
    fn dispatcher_rejects_a_provider_inventory_for_the_wrong_workload() {
        let request = AndroidGuestRequest::inventory("inventory-03", "android-eagle")
            .expect("canonical inventory request");
        assert_eq!(
            dispatch_guest_request(&WrongWorkloadProvider, request),
            Err(AndroidGuestBoundaryError::InventoryWorkloadMismatch)
        );
    }

    #[test]
    fn dispatcher_admits_requests_before_provider_dispatch() {
        let mut request =
            AndroidGuestRequest::launch("launch-03", "android-eagle", AospStarterApp::Browser)
                .expect("canonical launch request");
        if let AndroidGuestRequest::Launch(request) = &mut request {
            request.intent.package_id = mackes_mesh_types::android_apps::AospPackageId::Calendar;
        }
        assert_eq!(
            dispatch_guest_request(&ReadyTestProvider, request),
            Err(AndroidGuestBoundaryError::LaunchIdentityMismatch(
                AospStarterApp::Browser
            ))
        );
    }

    #[test]
    fn provider_registry_registers_and_dispatches_by_workload_identity() {
        let mut registry = AndroidGuestProviderRegistry::default();
        registry
            .register("android-eagle", Arc::new(ReadyTestProvider))
            .expect("provider registration");

        let request = AndroidGuestRequest::inventory("registry-inventory-01", "android-eagle")
            .expect("canonical inventory request");
        let response = registry
            .dispatch(request.clone())
            .expect("registered provider response");

        assert!(response.validate_against(&request).is_ok());
        match response {
            AndroidGuestResponse::Inventory(response) => {
                assert!(response.inventory.entries[0].is_launchable());
            }
            AndroidGuestResponse::Launch(_) => panic!("inventory request returned launch response"),
        }
    }

    #[test]
    fn provider_registry_rejects_duplicate_workload_identity() {
        let mut registry = AndroidGuestProviderRegistry::new();
        registry
            .register("android-eagle", Arc::new(ReadyTestProvider))
            .expect("first provider registration");

        assert_eq!(
            registry.register("android-eagle", Arc::new(ReadyTestProvider)),
            Err(AndroidGuestProviderRegistryError::DuplicateWorkloadId {
                workload_id: "android-eagle".to_owned(),
            })
        );
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn provider_registry_reports_missing_identity_but_dispatch_falls_back_closed() {
        let registry = AndroidGuestProviderRegistry::new();
        assert!(matches!(
            registry.provider("android-missing"),
            Err(AndroidGuestProviderRegistryError::MissingWorkloadId { workload_id })
                if workload_id == "android-missing"
        ));

        let request = AndroidGuestRequest::launch(
            "registry-launch-01",
            "android-missing",
            AospStarterApp::Browser,
        )
        .expect("canonical launch request");
        let response = registry
            .dispatch(request.clone())
            .expect("unconfigured fallback response");
        assert!(response.validate_against(&request).is_ok());
        match response {
            AndroidGuestResponse::Launch(response) => {
                assert_eq!(response.outcome, AndroidGuestLaunchOutcome::Unavailable);
            }
            AndroidGuestResponse::Inventory(_) => {
                panic!("launch request returned inventory response")
            }
        }
    }

    #[test]
    fn provider_registry_rejects_invalid_identity_and_capacity_overflow() {
        let mut registry = AndroidGuestProviderRegistry::new();
        assert_eq!(
            registry.register("../android-eagle", Arc::new(ReadyTestProvider)),
            Err(AndroidGuestProviderRegistryError::InvalidWorkloadId {
                workload_id: "../android-eagle".to_owned(),
            })
        );

        for index in 0..MAX_RETAINED_ANDROID_GUEST_PROVIDERS {
            registry
                .register(
                    format!("android-provider-{index:02}"),
                    Arc::new(ReadyTestProvider),
                )
                .expect("provider capacity slot");
        }
        assert_eq!(registry.len(), MAX_RETAINED_ANDROID_GUEST_PROVIDERS);
        assert_eq!(
            registry.register("android-over-capacity", Arc::new(ReadyTestProvider)),
            Err(AndroidGuestProviderRegistryError::CapacityExceeded {
                max_workloads: MAX_RETAINED_ANDROID_GUEST_PROVIDERS,
            })
        );
    }

    #[test]
    fn provider_registry_preserves_request_correlation_when_workloads_differ() {
        let mut registry = AndroidGuestProviderRegistry::new();
        registry
            .register("android-eagle", Arc::new(ReadyTestProvider))
            .expect("provider registration");

        let registered_request =
            AndroidGuestRequest::inventory("registry-correlation-01", "android-eagle")
                .expect("registered request");
        let registered_response = registry
            .dispatch(registered_request.clone())
            .expect("registered response");
        assert!(registered_response
            .validate_against(&registered_request)
            .is_ok());

        let absent_request =
            AndroidGuestRequest::inventory("registry-correlation-02", "android-other")
                .expect("absent request");
        let absent_response = registry
            .dispatch(absent_request.clone())
            .expect("unconfigured response");
        assert!(absent_response.validate_against(&absent_request).is_ok());
        match absent_response {
            AndroidGuestResponse::Inventory(response) => {
                assert_eq!(response.workload_id, "android-other");
                assert!(response
                    .inventory
                    .entries
                    .iter()
                    .all(|entry| !entry.is_launchable()));
            }
            AndroidGuestResponse::Launch(_) => panic!("inventory request returned launch response"),
        }
    }
}
