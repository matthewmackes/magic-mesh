//! WL-FUNC-020 — typed Cuttlefish provider boundary for Android Workloads.
//!
//! This module is deliberately a provider adapter, not a Cuttlefish guest
//! implementation. The adapter accepts only the stable Android VM identity,
//! the already-admitted image/package provenance, and closed lifecycle,
//! readiness, or starter-app operations. A backend implementation can be
//! supplied through [`CuttlefishProviderClient`] without widening the boundary
//! to commands, paths, URLs, ADB data, sockets, or arbitrary launcher intents.

use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mackes_mesh_types::android_apps::{
    pending_starter_entries, AndroidAppContractError, AndroidAppInventory, AndroidGuestBootState,
    AndroidGuestInventoryRequest, AndroidGuestLaunchOutcome, AndroidGuestLaunchRequest,
    AndroidImagePackageManifest, AndroidImageProvenance, AndroidUnavailableReason,
};
use mackes_mesh_types::android_provider::{
    CuttlefishContractError, CuttlefishGuestBootState, CuttlefishGuestReadiness,
    CuttlefishGuestReadinessEvidence, CuttlefishLifecycleOperation, CuttlefishLifecycleRequest,
    CuttlefishUnavailableReason, CuttlefishVmLifecycleState, CuttlefishVmObservation,
    CuttlefishVmTarget,
};

use super::super::super::runner::CloudRunner;
use super::cuttlefish_guest::{
    CuttlefishGuestTransport, GuestSnapshot, UnixCuttlefishGuestTransport,
};
use super::AndroidGuestProvider;
use mackes_mesh_types::android_provider::AndroidVdiSource;
use mackes_mesh_types::cloud::{CloudInstance, LifecycleAction};

/// Closed failures returned by the Cuttlefish adapter or its backend client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CuttlefishProviderError {
    /// The stable Android workload identity was not admitted or did not match
    /// the Cuttlefish target identity.
    InvalidWorkloadIdentity,
    /// The immutable Android package manifest was not bound to the target image.
    ImagePackageProvenanceMismatch,
    /// A typed Cuttlefish contract failed local admission.
    Contract(CuttlefishContractError),
    /// The retained Android package manifest failed its own contract.
    InventoryContract(AndroidAppContractError),
    /// The configured provider could not currently answer.
    ProviderUnavailable,
    /// The configured provider rejected an admitted typed operation.
    ProviderRejected,
    /// The adapter state lock could not be acquired safely.
    StatePoisoned,
}

/// The only backend surface a Cuttlefish adapter may call.
///
/// Implementations are responsible for translating these closed values to a
/// real provider, but cannot receive a shell command, host path, endpoint, raw
/// package name, or arbitrary intent through this interface. The adapter runs
/// all admission and generation checks before either method is called.
pub(crate) trait CuttlefishProviderClient: Send + Sync {
    /// Read the provider's typed lifecycle and guest boot/readiness observation.
    fn observe(
        &self,
        target: &CuttlefishVmTarget,
    ) -> Result<CuttlefishVmObservation, CuttlefishProviderError>;

    /// Apply one already-admitted closed lifecycle operation.
    fn lifecycle(
        &self,
        request: &CuttlefishLifecycleRequest,
    ) -> Result<CuttlefishVmObservation, CuttlefishProviderError>;

    /// Dispatch one already-admitted starter-app launcher request against the
    /// exact target bound to this adapter.
    ///
    /// The adapter calls this only after the retained provider observation is
    /// `Running` with guest-owned ready evidence. Implementations must return a
    /// closed outcome and must not claim success unless the guest/session layer
    /// accepted the request.
    fn launch(
        &self,
        target: &CuttlefishVmTarget,
        request: &AndroidGuestLaunchRequest,
    ) -> Result<AndroidGuestLaunchOutcome, CuttlefishProviderError>;

    fn inventory_at(
        &self,
        _target: &CuttlefishVmTarget,
        _package_manifest: &AndroidImagePackageManifest,
        _generation: u64,
    ) -> Result<AndroidAppInventory, CuttlefishProviderError> {
        Err(CuttlefishProviderError::ProviderUnavailable)
    }

    fn launch_at(
        &self,
        target: &CuttlefishVmTarget,
        request: &AndroidGuestLaunchRequest,
        _generation: u64,
    ) -> Result<AndroidGuestLaunchOutcome, CuttlefishProviderError> {
        self.launch(target, request)
    }

    fn vdi_source(&self, _generation: u64) -> Option<AndroidVdiSource> {
        None
    }

    fn cleanup(
        &self,
        _request_id: &str,
        _target: &CuttlefishVmTarget,
        _package_manifest: &AndroidImagePackageManifest,
        _generation: u64,
    ) -> Result<(), CuttlefishProviderError> {
        Err(CuttlefishProviderError::ProviderUnavailable)
    }
}

struct GuestContract {
    package_manifest: AndroidImagePackageManifest,
    catalog_digest: String,
    transport: Arc<dyn CuttlefishGuestTransport>,
}

/// Production outer-VM client for a Cuttlefish-backed Android workload.
///
/// This client owns only the L1 libvirt lifecycle. `virsh` is reached through
/// the existing typed [`CloudRunner`] authority, while the inner Android guest
/// remains explicitly unready until a future guest-owned provider supplies the
/// package-manager/session evidence required by the Cuttlefish contract. The
/// client never turns an active libvirt domain into a false Android-ready claim.
pub(crate) struct LibvirtCuttlefishProviderClient {
    runner: Arc<dyn CloudRunner>,
    generation: Mutex<u64>,
    guest: Option<GuestContract>,
    guest_snapshot: Mutex<Option<(u64, GuestSnapshot)>>,
}

impl LibvirtCuttlefishProviderClient {
    /// Bind one workload-scoped provider client to the canonical cloud runner.
    #[must_use]
    pub(crate) fn new(runner: Arc<dyn CloudRunner>) -> Self {
        Self {
            runner,
            generation: Mutex::new(0),
            guest: None,
            guest_snapshot: Mutex::new(None),
        }
    }

    pub(crate) fn with_guest_contract(
        runner: Arc<dyn CloudRunner>,
        package_manifest: AndroidImagePackageManifest,
        catalog_digest: String,
    ) -> Result<Self, CuttlefishProviderError> {
        package_manifest
            .validate()
            .map_err(CuttlefishProviderError::InventoryContract)?;
        if catalog_digest.len() != 71 || !catalog_digest.starts_with("sha256:") {
            return Err(CuttlefishProviderError::ProviderRejected);
        }
        Ok(Self {
            runner,
            generation: Mutex::new(0),
            guest: Some(GuestContract {
                package_manifest,
                catalog_digest,
                transport: Arc::new(UnixCuttlefishGuestTransport::production()),
            }),
            guest_snapshot: Mutex::new(None),
        })
    }

    fn stable_generation(&self, present: bool) -> Result<u64, CuttlefishProviderError> {
        let mut generation = self
            .generation
            .lock()
            .map_err(|_| CuttlefishProviderError::StatePoisoned)?;
        if present {
            *generation = (*generation).max(1);
        } else if *generation == 0 {
            return Ok(0);
        }
        Ok(*generation)
    }

    fn next_generation(&self) -> Result<u64, CuttlefishProviderError> {
        let mut generation = self
            .generation
            .lock()
            .map_err(|_| CuttlefishProviderError::StatePoisoned)?;
        *generation = generation.saturating_add(1).max(1);
        Ok(*generation)
    }

    fn reset_generation(&self) -> Result<(), CuttlefishProviderError> {
        let mut generation = self
            .generation
            .lock()
            .map_err(|_| CuttlefishProviderError::StatePoisoned)?;
        *generation = 0;
        Ok(())
    }

    fn observation(
        target: &CuttlefishVmTarget,
        lifecycle_state: CuttlefishVmLifecycleState,
        generation: u64,
        unavailable_reason: Option<CuttlefishUnavailableReason>,
    ) -> Result<CuttlefishVmObservation, CuttlefishProviderError> {
        let (boot_state, readiness) = match lifecycle_state {
            CuttlefishVmLifecycleState::Absent => (
                CuttlefishGuestBootState::Pending,
                CuttlefishGuestReadiness::Unknown,
            ),
            CuttlefishVmLifecycleState::Provisioning => (
                CuttlefishGuestBootState::Pending,
                CuttlefishGuestReadiness::NotReady,
            ),
            CuttlefishVmLifecycleState::Stopped => (
                CuttlefishGuestBootState::Stopped,
                CuttlefishGuestReadiness::NotReady,
            ),
            CuttlefishVmLifecycleState::Starting | CuttlefishVmLifecycleState::Rebooting => (
                CuttlefishGuestBootState::Booting,
                CuttlefishGuestReadiness::NotReady,
            ),
            CuttlefishVmLifecycleState::Running => (
                CuttlefishGuestBootState::Ready,
                CuttlefishGuestReadiness::Ready,
            ),
            CuttlefishVmLifecycleState::Unavailable => (
                CuttlefishGuestBootState::Unavailable,
                CuttlefishGuestReadiness::Unavailable,
            ),
            CuttlefishVmLifecycleState::Failed => (
                CuttlefishGuestBootState::Failed,
                CuttlefishGuestReadiness::Unavailable,
            ),
        };
        let evidence =
            CuttlefishGuestReadinessEvidence::new(boot_state, readiness, unavailable_reason)
                .map_err(CuttlefishProviderError::Contract)?;
        CuttlefishVmObservation::new(
            target.clone(),
            lifecycle_state,
            evidence,
            generation,
            now_unix_ms(),
        )
        .map_err(CuttlefishProviderError::Contract)
    }

    fn instance<'a>(
        instances: &'a [CloudInstance],
        target: &CuttlefishVmTarget,
    ) -> Option<&'a CloudInstance> {
        instances
            .iter()
            .find(|instance| instance.id == target.vm_id.as_str())
            .or_else(|| {
                instances
                    .iter()
                    .find(|instance| instance.name == target.vm_id.as_str())
            })
    }
}

impl CuttlefishProviderClient for LibvirtCuttlefishProviderClient {
    fn observe(
        &self,
        target: &CuttlefishVmTarget,
    ) -> Result<CuttlefishVmObservation, CuttlefishProviderError> {
        let instances = self
            .runner
            .list_instances()
            .map_err(|_| CuttlefishProviderError::ProviderUnavailable)?;
        let Some(instance) = Self::instance(&instances, target) else {
            let generation = self.stable_generation(false)?;
            return Self::observation(
                target,
                if generation == 0 {
                    CuttlefishVmLifecycleState::Absent
                } else {
                    CuttlefishVmLifecycleState::Unavailable
                },
                generation,
                (generation > 0).then_some(CuttlefishUnavailableReason::ProviderUnavailable),
            );
        };

        let status = instance.status.trim();
        let generation = self.stable_generation(true)?;
        match status.to_ascii_uppercase().as_str() {
            "ACTIVE" | "RUNNING" => Self::observation(
                target,
                if let Some(guest) = &self.guest {
                    match guest.transport.observe(
                        "provider-observe",
                        target,
                        &guest.catalog_digest,
                        &guest.package_manifest,
                        generation,
                    ) {
                        Ok(snapshot) => {
                            *self
                                .guest_snapshot
                                .lock()
                                .map_err(|_| CuttlefishProviderError::StatePoisoned)? =
                                Some((generation, snapshot));
                            CuttlefishVmLifecycleState::Running
                        }
                        Err(_) => CuttlefishVmLifecycleState::Starting,
                    }
                } else {
                    CuttlefishVmLifecycleState::Starting
                },
                generation,
                None,
            ),
            "SHUTOFF" | "SHUT OFF" | "STOPPED" => Self::observation(
                target,
                CuttlefishVmLifecycleState::Stopped,
                generation,
                None,
            ),
            "ERROR" | "FAILED" => Self::observation(
                target,
                CuttlefishVmLifecycleState::Failed,
                generation,
                Some(CuttlefishUnavailableReason::GuestBootFailed),
            ),
            _ => Self::observation(
                target,
                CuttlefishVmLifecycleState::Unavailable,
                generation,
                Some(CuttlefishUnavailableReason::ProviderUnavailable),
            ),
        }
    }

    fn lifecycle(
        &self,
        request: &CuttlefishLifecycleRequest,
    ) -> Result<CuttlefishVmObservation, CuttlefishProviderError> {
        let action = match request.operation {
            CuttlefishLifecycleOperation::Provision => {
                // Provision is owned by the armed desired-state/tofu lane; this
                // client has no authority to invent a workload document.
                return Err(CuttlefishProviderError::ProviderUnavailable);
            }
            CuttlefishLifecycleOperation::Start => LifecycleAction::Start,
            CuttlefishLifecycleOperation::Stop => LifecycleAction::Stop,
            CuttlefishLifecycleOperation::Reboot => LifecycleAction::Reboot,
            CuttlefishLifecycleOperation::Destroy => LifecycleAction::Delete,
        };
        let outcome = self.runner.lifecycle(action, request.target.vm_id.as_str());
        if !outcome.ok || !outcome.applied {
            return Err(CuttlefishProviderError::ProviderRejected);
        }

        if request.operation == CuttlefishLifecycleOperation::Destroy {
            self.reset_generation()?;
            return Self::observation(&request.target, CuttlefishVmLifecycleState::Absent, 0, None);
        }

        let generation = self.next_generation()?;
        let state = match request.operation {
            CuttlefishLifecycleOperation::Start => CuttlefishVmLifecycleState::Starting,
            CuttlefishLifecycleOperation::Stop => CuttlefishVmLifecycleState::Stopped,
            CuttlefishLifecycleOperation::Reboot => CuttlefishVmLifecycleState::Rebooting,
            CuttlefishLifecycleOperation::Provision | CuttlefishLifecycleOperation::Destroy => {
                unreachable!("handled above")
            }
        };
        Self::observation(&request.target, state, generation, None)
    }

    fn launch(
        &self,
        target: &CuttlefishVmTarget,
        request: &AndroidGuestLaunchRequest,
    ) -> Result<AndroidGuestLaunchOutcome, CuttlefishProviderError> {
        let generation = *self
            .generation
            .lock()
            .map_err(|_| CuttlefishProviderError::StatePoisoned)?;
        self.launch_at(target, request, generation)
    }

    fn inventory_at(
        &self,
        target: &CuttlefishVmTarget,
        package_manifest: &AndroidImagePackageManifest,
        generation: u64,
    ) -> Result<AndroidAppInventory, CuttlefishProviderError> {
        let guest = self
            .guest
            .as_ref()
            .ok_or(CuttlefishProviderError::ProviderUnavailable)?;
        if package_manifest != &guest.package_manifest {
            return Err(CuttlefishProviderError::ImagePackageProvenanceMismatch);
        }
        let snapshot = guest.transport.observe(
            "lifecycle-inventory",
            target,
            &guest.catalog_digest,
            package_manifest,
            generation,
        )?;
        let inventory = snapshot.inventory.clone();
        *self
            .guest_snapshot
            .lock()
            .map_err(|_| CuttlefishProviderError::StatePoisoned)? = Some((generation, snapshot));
        Ok(inventory)
    }

    fn launch_at(
        &self,
        target: &CuttlefishVmTarget,
        request: &AndroidGuestLaunchRequest,
        generation: u64,
    ) -> Result<AndroidGuestLaunchOutcome, CuttlefishProviderError> {
        let guest = self
            .guest
            .as_ref()
            .ok_or(CuttlefishProviderError::ProviderUnavailable)?;
        guest.transport.launch(
            request,
            target,
            &guest.catalog_digest,
            &guest.package_manifest,
            generation,
        )
    }

    fn vdi_source(&self, generation: u64) -> Option<AndroidVdiSource> {
        self.guest_snapshot
            .lock()
            .ok()
            .and_then(|snapshot| snapshot.as_ref().cloned())
            .and_then(|(retained_generation, snapshot)| {
                (retained_generation == generation).then_some(snapshot.vdi_source)
            })
    }

    fn cleanup(
        &self,
        request_id: &str,
        target: &CuttlefishVmTarget,
        package_manifest: &AndroidImagePackageManifest,
        generation: u64,
    ) -> Result<(), CuttlefishProviderError> {
        let guest = self
            .guest
            .as_ref()
            .ok_or(CuttlefishProviderError::ProviderUnavailable)?;
        if package_manifest != &guest.package_manifest {
            return Err(CuttlefishProviderError::ImagePackageProvenanceMismatch);
        }
        guest.transport.cleanup(
            request_id,
            target,
            &guest.catalog_digest,
            package_manifest,
            generation,
        )?;
        *self
            .guest_snapshot
            .lock()
            .map_err(|_| CuttlefishProviderError::StatePoisoned)? = None;
        Ok(())
    }
}

/// A workload-scoped Cuttlefish adapter reachable through the existing Android
/// provider registry and CloudWorker inventory poll.
///
/// The retained observation is the local compare-and-admit clock. A lifecycle
/// request must match its target image and exact generation and be allowed from
/// the retained state before the backend client is contacted. A provider error
/// or malformed observation leaves the last admitted state untouched.
pub(crate) struct CuttlefishProviderAdapter<C> {
    workload_id: String,
    target: CuttlefishVmTarget,
    package_manifest: AndroidImagePackageManifest,
    client: C,
    observation: Mutex<CuttlefishVmObservation>,
}

impl<C: CuttlefishProviderClient> CuttlefishProviderAdapter<C> {
    /// Construct an adapter after validating all stable identity and provenance
    /// bindings. No provider method is called during construction.
    pub(crate) fn new(
        workload_id: impl Into<String>,
        target: CuttlefishVmTarget,
        package_manifest: AndroidImagePackageManifest,
        observation: CuttlefishVmObservation,
        client: C,
    ) -> Result<Self, CuttlefishProviderError> {
        let workload_id = workload_id.into();
        AndroidGuestInventoryRequest::new("cuttlefish-adapter", workload_id.clone())
            .map_err(|_| CuttlefishProviderError::InvalidWorkloadIdentity)?;
        if target.vm_id.as_str() != workload_id {
            return Err(CuttlefishProviderError::InvalidWorkloadIdentity);
        }

        let target = target
            .admitted()
            .map_err(CuttlefishProviderError::Contract)?;
        let package_manifest = package_manifest
            .admitted()
            .map_err(CuttlefishProviderError::InventoryContract)?;
        let image_provenance = android_image_provenance(&target)?;
        if package_manifest.image_provenance != image_provenance {
            return Err(CuttlefishProviderError::ImagePackageProvenanceMismatch);
        }

        let observation = observation
            .admitted()
            .map_err(CuttlefishProviderError::Contract)?;
        if observation.target != target {
            return Err(CuttlefishProviderError::Contract(
                CuttlefishContractError::TargetIdentityMismatch,
            ));
        }

        Ok(Self {
            workload_id,
            target,
            package_manifest,
            client,
            observation: Mutex::new(observation),
        })
    }

    /// Borrow the immutable package provenance retained by this adapter.
    #[must_use]
    pub(crate) fn package_manifest(&self) -> &AndroidImagePackageManifest {
        &self.package_manifest
    }

    /// Return the last locally admitted provider observation.
    pub(crate) fn current_observation(
        &self,
    ) -> Result<CuttlefishVmObservation, CuttlefishProviderError> {
        self.observation
            .lock()
            .map(|observation| observation.clone())
            .map_err(|_| CuttlefishProviderError::StatePoisoned)
    }

    /// Contact the backend for a typed readiness observation.
    ///
    /// The client is contacted only after the adapter has retained a valid
    /// target. Any invalid provider result is rejected before it replaces the
    /// current observation.
    pub(crate) fn observe_readiness(
        &self,
    ) -> Result<CuttlefishVmObservation, CuttlefishProviderError> {
        let mut current = self
            .observation
            .lock()
            .map_err(|_| CuttlefishProviderError::StatePoisoned)?;
        let observed = self.client.observe(&self.target)?;
        commit_observation(&self.target, &mut current, observed)
    }

    /// Apply one typed lifecycle request after local admission.
    ///
    /// `admitted_against` is intentionally evaluated while the retained state
    /// lock is held and before the client call. This makes stale generations,
    /// image drift, identity drift, and invalid state transitions fail closed
    /// without provider contact.
    pub(crate) fn lifecycle(
        &self,
        request: CuttlefishLifecycleRequest,
    ) -> Result<CuttlefishVmObservation, CuttlefishProviderError> {
        let request = request
            .admitted()
            .map_err(CuttlefishProviderError::Contract)?;
        let mut current = self
            .observation
            .lock()
            .map_err(|_| CuttlefishProviderError::StatePoisoned)?;
        request
            .admitted_against(&current)
            .map_err(CuttlefishProviderError::Contract)?;

        let observed = self.client.lifecycle(&request)?;
        if request.operation == CuttlefishLifecycleOperation::Destroy {
            commit_destroy_observation(&self.target, &mut current, observed)
        } else {
            commit_observation(&self.target, &mut current, observed)
        }
    }

    fn inventory_from_observation(
        &self,
        observation: &CuttlefishVmObservation,
    ) -> Result<AndroidAppInventory, CuttlefishProviderError> {
        if observation.target != self.target {
            return Err(CuttlefishProviderError::Contract(
                CuttlefishContractError::TargetIdentityMismatch,
            ));
        }
        match observation.lifecycle_state {
            CuttlefishVmLifecycleState::Absent => {
                let mut inventory = AndroidAppInventory::pending(self.workload_id.clone());
                inventory.image_provenance = Some(android_image_provenance(&self.target)?);
                inventory
                    .validate()
                    .map_err(CuttlefishProviderError::InventoryContract)?;
                Ok(inventory)
            }
            CuttlefishVmLifecycleState::Provisioning
            | CuttlefishVmLifecycleState::Starting
            | CuttlefishVmLifecycleState::Rebooting => self.booting_inventory(observation),
            CuttlefishVmLifecycleState::Stopped => {
                self.unavailable_inventory(observation, AndroidUnavailableReason::GuestUnavailable)
            }
            CuttlefishVmLifecycleState::Running => self
                .client
                .inventory_at(&self.target, &self.package_manifest, observation.generation)
                .and_then(|inventory| self.admit_guest_inventory(inventory)),
            CuttlefishVmLifecycleState::Unavailable => self.unavailable_inventory(
                observation,
                observation
                    .guest
                    .unavailable_reason
                    .map(map_unavailable_reason)
                    .unwrap_or(AndroidUnavailableReason::ProviderUnavailable),
            ),
            CuttlefishVmLifecycleState::Failed => self.unavailable_inventory(
                observation,
                observation
                    .guest
                    .unavailable_reason
                    .map(map_unavailable_reason)
                    .unwrap_or(AndroidUnavailableReason::GuestBootFailed),
            ),
        }
    }

    fn booting_inventory(
        &self,
        observation: &CuttlefishVmObservation,
    ) -> Result<AndroidAppInventory, CuttlefishProviderError> {
        AndroidAppInventory::observed(
            self.workload_id.clone(),
            android_image_provenance(&self.target)?,
            AndroidGuestBootState::Booting,
            observation.observed_at_unix_ms,
            observation_age_ms(observation.observed_at_unix_ms),
            pending_starter_entries(),
        )
        .map_err(CuttlefishProviderError::InventoryContract)
    }

    fn unavailable_inventory(
        &self,
        observation: &CuttlefishVmObservation,
        reason: AndroidUnavailableReason,
    ) -> Result<AndroidAppInventory, CuttlefishProviderError> {
        let mut inventory = AndroidAppInventory::pending(self.workload_id.clone());
        inventory.image_provenance = Some(android_image_provenance(&self.target)?);
        inventory.guest_boot_state = AndroidGuestBootState::Unavailable;
        inventory.observed_at_unix_ms = Some(observation.observed_at_unix_ms);
        inventory.observation_age_ms = Some(observation_age_ms(observation.observed_at_unix_ms));
        inventory.unavailable_reason = Some(reason);
        inventory
            .validate()
            .map_err(CuttlefishProviderError::InventoryContract)?;
        Ok(inventory)
    }

    fn admit_guest_inventory(
        &self,
        inventory: AndroidAppInventory,
    ) -> Result<AndroidAppInventory, CuttlefishProviderError> {
        inventory
            .validate()
            .map_err(CuttlefishProviderError::InventoryContract)?;
        if inventory.workload_id != self.workload_id
            || inventory.image_provenance.as_ref() != Some(&self.package_manifest.image_provenance)
            || inventory.guest_boot_state != AndroidGuestBootState::Ready
        {
            return Err(CuttlefishProviderError::ProviderRejected);
        }
        Ok(inventory)
    }
}

impl<C: CuttlefishProviderClient> AndroidGuestProvider for CuttlefishProviderAdapter<C> {
    fn inventory(&self, request: &AndroidGuestInventoryRequest) -> AndroidAppInventory {
        if request.workload_id != self.workload_id {
            // The outer dispatcher normally prevents this path. Keeping the
            // adapter defensive makes direct trait use fail closed too.
            return AndroidAppInventory::pending(request.workload_id.clone());
        }
        match self
            .observe_readiness()
            .and_then(|observation| self.inventory_from_observation(&observation))
        {
            Ok(inventory) => inventory,
            Err(_) => self
                .current_observation()
                .ok()
                .and_then(|observation| {
                    self.unavailable_inventory(
                        &observation,
                        AndroidUnavailableReason::ProviderUnavailable,
                    )
                    .ok()
                })
                .unwrap_or_else(|| AndroidAppInventory::pending(self.workload_id.clone())),
        }
    }

    fn launch(&self, request: &AndroidGuestLaunchRequest) -> AndroidGuestLaunchOutcome {
        if request.workload_id != self.workload_id || request.validate().is_err() {
            AndroidGuestLaunchOutcome::Rejected
        } else {
            let ready = self
                .current_observation()
                .map(|observation| observation.is_guest_ready())
                .unwrap_or(false);
            if !ready {
                // A lifecycle state or image manifest is not package/session
                // evidence. Do not contact the guest launcher until the
                // provider has supplied a current, guest-owned ready pair.
                return AndroidGuestLaunchOutcome::Unavailable;
            }
            match self.client.launch(&self.target, request) {
                Ok(outcome) => outcome,
                Err(CuttlefishProviderError::ProviderRejected) => {
                    AndroidGuestLaunchOutcome::Rejected
                }
                Err(_) => AndroidGuestLaunchOutcome::Unavailable,
            }
        }
    }

    fn inventory_at(
        &self,
        request: &AndroidGuestInventoryRequest,
        generation: u64,
    ) -> AndroidAppInventory {
        if request.workload_id != self.workload_id || generation == 0 {
            return AndroidAppInventory::pending(request.workload_id.clone());
        }
        self.client
            .inventory_at(&self.target, &self.package_manifest, generation)
            .and_then(|inventory| self.admit_guest_inventory(inventory))
            .unwrap_or_else(|_| AndroidAppInventory::pending(self.workload_id.clone()))
    }

    fn launch_at(
        &self,
        request: &AndroidGuestLaunchRequest,
        generation: u64,
    ) -> AndroidGuestLaunchOutcome {
        if request.workload_id != self.workload_id || request.validate().is_err() || generation == 0
        {
            return AndroidGuestLaunchOutcome::Rejected;
        }
        match self.client.launch_at(&self.target, request, generation) {
            Ok(outcome) => outcome,
            Err(CuttlefishProviderError::ProviderRejected) => AndroidGuestLaunchOutcome::Rejected,
            Err(_) => AndroidGuestLaunchOutcome::Unavailable,
        }
    }

    fn vdi_source(&self, generation: u64) -> Option<AndroidVdiSource> {
        self.client.vdi_source(generation)
    }

    fn cleanup(&self, request_id: &str, generation: u64) -> bool {
        self.client
            .cleanup(request_id, &self.target, &self.package_manifest, generation)
            .is_ok()
    }
}

fn commit_observation(
    target: &CuttlefishVmTarget,
    current: &mut CuttlefishVmObservation,
    observed: CuttlefishVmObservation,
) -> Result<CuttlefishVmObservation, CuttlefishProviderError> {
    observed
        .validate()
        .map_err(CuttlefishProviderError::Contract)?;
    if observed.target.vm_id != target.vm_id {
        return Err(CuttlefishProviderError::Contract(
            CuttlefishContractError::TargetIdentityMismatch,
        ));
    }
    if observed.target.image_provenance != target.image_provenance {
        return Err(CuttlefishProviderError::Contract(
            CuttlefishContractError::ImageProvenanceMismatch,
        ));
    }
    if observed.generation < current.generation {
        return Err(CuttlefishProviderError::Contract(
            CuttlefishContractError::GenerationMismatch {
                expected: current.generation,
                actual: observed.generation,
            },
        ));
    }
    if observed.observed_at_unix_ms < current.observed_at_unix_ms {
        return Err(CuttlefishProviderError::Contract(
            CuttlefishContractError::InvalidTimestamp,
        ));
    }
    *current = observed.clone();
    Ok(observed)
}

/// Destroy is the one admitted lifecycle operation that intentionally resets a
/// VM generation to the contract's `Absent/generation=0` state. Keep its target
/// and timestamp checks, but do not apply the normal monotonic-generation rule.
fn commit_destroy_observation(
    target: &CuttlefishVmTarget,
    current: &mut CuttlefishVmObservation,
    observed: CuttlefishVmObservation,
) -> Result<CuttlefishVmObservation, CuttlefishProviderError> {
    observed
        .validate()
        .map_err(CuttlefishProviderError::Contract)?;
    if observed.target != *target {
        return Err(CuttlefishProviderError::Contract(
            CuttlefishContractError::TargetIdentityMismatch,
        ));
    }
    if observed.observed_at_unix_ms < current.observed_at_unix_ms {
        return Err(CuttlefishProviderError::Contract(
            CuttlefishContractError::InvalidTimestamp,
        ));
    }
    if observed.lifecycle_state != CuttlefishVmLifecycleState::Absent {
        return Err(CuttlefishProviderError::Contract(
            CuttlefishContractError::InvalidLifecycleState,
        ));
    }
    *current = observed.clone();
    Ok(observed)
}

fn android_image_provenance(
    target: &CuttlefishVmTarget,
) -> Result<AndroidImageProvenance, CuttlefishProviderError> {
    AndroidImageProvenance::new(
        target.image_provenance.image_id.clone(),
        target.image_provenance.image_digest.clone(),
        target.image_provenance.source_revision.clone(),
        target.image_provenance.catalog_revision.clone(),
    )
    .map_err(CuttlefishProviderError::InventoryContract)
}

fn observation_age_ms(observed_at_unix_ms: u64) -> u64 {
    now_unix_ms().saturating_sub(observed_at_unix_ms)
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

fn map_unavailable_reason(reason: CuttlefishUnavailableReason) -> AndroidUnavailableReason {
    match reason {
        CuttlefishUnavailableReason::ProviderUnavailable => {
            AndroidUnavailableReason::ProviderUnavailable
        }
        CuttlefishUnavailableReason::ImageUnavailable => AndroidUnavailableReason::ImageUnavailable,
        CuttlefishUnavailableReason::CapacityUnavailable => {
            AndroidUnavailableReason::CapacityUnavailable
        }
        CuttlefishUnavailableReason::GuestBootFailed => AndroidUnavailableReason::GuestBootFailed,
        CuttlefishUnavailableReason::GuestNotReady => AndroidUnavailableReason::GuestUnavailable,
        CuttlefishUnavailableReason::TransportUnavailable => {
            AndroidUnavailableReason::TransportUnavailable
        }
        CuttlefishUnavailableReason::ObservationStale => AndroidUnavailableReason::ObservationStale,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use super::*;
    use mackes_mesh_types::android_apps::{
        AndroidImagePackage, AndroidPackageVersion, AospStarterApp,
    };
    use mackes_mesh_types::android_provider::{
        CuttlefishGuestBootState, CuttlefishGuestReadiness, CuttlefishGuestReadinessEvidence,
        CuttlefishImageProvenanceRef, CuttlefishLifecycleOperation, CuttlefishVmId,
    };

    const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[derive(Clone)]
    struct FakeClient {
        observe_calls: Arc<AtomicUsize>,
        lifecycle_calls: Arc<AtomicUsize>,
        launch_calls: Arc<AtomicUsize>,
        observe_result:
            Arc<Mutex<Option<Result<CuttlefishVmObservation, CuttlefishProviderError>>>>,
        lifecycle_result:
            Arc<Mutex<Option<Result<CuttlefishVmObservation, CuttlefishProviderError>>>>,
        launch_result:
            Arc<Mutex<Option<Result<AndroidGuestLaunchOutcome, CuttlefishProviderError>>>>,
    }

    impl FakeClient {
        fn new(
            observe_result: Result<CuttlefishVmObservation, CuttlefishProviderError>,
            lifecycle_result: Result<CuttlefishVmObservation, CuttlefishProviderError>,
        ) -> Self {
            Self {
                observe_calls: Arc::new(AtomicUsize::new(0)),
                lifecycle_calls: Arc::new(AtomicUsize::new(0)),
                launch_calls: Arc::new(AtomicUsize::new(0)),
                observe_result: Arc::new(Mutex::new(Some(observe_result))),
                lifecycle_result: Arc::new(Mutex::new(Some(lifecycle_result))),
                launch_result: Arc::new(Mutex::new(Some(Ok(AndroidGuestLaunchOutcome::Started)))),
            }
        }
    }

    impl CuttlefishProviderClient for FakeClient {
        fn observe(
            &self,
            _target: &CuttlefishVmTarget,
        ) -> Result<CuttlefishVmObservation, CuttlefishProviderError> {
            self.observe_calls.fetch_add(1, Ordering::Relaxed);
            self.observe_result
                .lock()
                .expect("observe result lock")
                .take()
                .expect("one observe call")
        }

        fn lifecycle(
            &self,
            _request: &CuttlefishLifecycleRequest,
        ) -> Result<CuttlefishVmObservation, CuttlefishProviderError> {
            self.lifecycle_calls.fetch_add(1, Ordering::Relaxed);
            self.lifecycle_result
                .lock()
                .expect("lifecycle result lock")
                .take()
                .expect("one lifecycle call")
        }

        fn launch(
            &self,
            _target: &CuttlefishVmTarget,
            _request: &AndroidGuestLaunchRequest,
        ) -> Result<AndroidGuestLaunchOutcome, CuttlefishProviderError> {
            self.launch_calls.fetch_add(1, Ordering::Relaxed);
            self.launch_result
                .lock()
                .expect("launch result lock")
                .take()
                .expect("one launch call")
        }
    }

    fn target() -> CuttlefishVmTarget {
        CuttlefishVmTarget::new(
            CuttlefishVmId::new("android-t480").expect("VM id"),
            CuttlefishImageProvenanceRef::new(
                "android-cuttlefish-v1",
                DIGEST,
                "aosp-source-r1",
                "starter-catalog-v1",
            )
            .expect("image provenance"),
        )
        .expect("target")
    }

    fn package_manifest() -> AndroidImagePackageManifest {
        let provenance = AndroidImageProvenance::new(
            "android-cuttlefish-v1",
            DIGEST,
            "aosp-source-r1",
            "starter-catalog-v1",
        )
        .expect("package provenance");
        let version = AndroidPackageVersion::new("1.0.0", 1).expect("package version");
        AndroidImagePackageManifest::new(
            provenance,
            AospStarterApp::ALL
                .into_iter()
                .map(|app| AndroidImagePackage::for_app(app, version.clone()))
                .collect(),
        )
        .expect("package manifest")
    }

    fn evidence(
        boot_state: CuttlefishGuestBootState,
        readiness: CuttlefishGuestReadiness,
        unavailable_reason: Option<CuttlefishUnavailableReason>,
    ) -> CuttlefishGuestReadinessEvidence {
        CuttlefishGuestReadinessEvidence::new(boot_state, readiness, unavailable_reason)
            .expect("guest evidence")
    }

    fn observation(
        lifecycle_state: CuttlefishVmLifecycleState,
        generation: u64,
        observed_at_unix_ms: u64,
    ) -> CuttlefishVmObservation {
        let (boot_state, readiness, unavailable_reason) = match lifecycle_state {
            CuttlefishVmLifecycleState::Absent => (
                CuttlefishGuestBootState::Pending,
                CuttlefishGuestReadiness::Unknown,
                None,
            ),
            CuttlefishVmLifecycleState::Provisioning => (
                CuttlefishGuestBootState::Pending,
                CuttlefishGuestReadiness::NotReady,
                None,
            ),
            CuttlefishVmLifecycleState::Stopped => (
                CuttlefishGuestBootState::Stopped,
                CuttlefishGuestReadiness::NotReady,
                None,
            ),
            CuttlefishVmLifecycleState::Starting | CuttlefishVmLifecycleState::Rebooting => (
                CuttlefishGuestBootState::Booting,
                CuttlefishGuestReadiness::NotReady,
                None,
            ),
            CuttlefishVmLifecycleState::Running => (
                CuttlefishGuestBootState::Ready,
                CuttlefishGuestReadiness::Ready,
                None,
            ),
            CuttlefishVmLifecycleState::Unavailable => (
                CuttlefishGuestBootState::Unavailable,
                CuttlefishGuestReadiness::Unavailable,
                Some(CuttlefishUnavailableReason::ProviderUnavailable),
            ),
            CuttlefishVmLifecycleState::Failed => (
                CuttlefishGuestBootState::Failed,
                CuttlefishGuestReadiness::Unavailable,
                Some(CuttlefishUnavailableReason::GuestBootFailed),
            ),
        };
        CuttlefishVmObservation::new(
            target(),
            lifecycle_state,
            evidence(boot_state, readiness, unavailable_reason),
            generation,
            observed_at_unix_ms,
        )
        .expect("observation")
    }

    fn adapter(
        initial: CuttlefishVmObservation,
        lifecycle_result: Result<CuttlefishVmObservation, CuttlefishProviderError>,
    ) -> (CuttlefishProviderAdapter<FakeClient>, FakeClient) {
        let client = FakeClient::new(Ok(initial.clone()), lifecycle_result);
        let client_handle = client.clone();
        let adapter = CuttlefishProviderAdapter::new(
            "android-t480",
            target(),
            package_manifest(),
            initial,
            client,
        )
        .expect("adapter");
        (adapter, client_handle)
    }

    #[test]
    fn valid_lifecycle_call_reaches_client_after_preserving_provenance() {
        let initial = observation(CuttlefishVmLifecycleState::Stopped, 7, 100);
        let next = observation(CuttlefishVmLifecycleState::Starting, 8, 200);
        let (adapter, client) = adapter(initial, Ok(next));
        let request = CuttlefishLifecycleRequest::new(
            "start-t480-01",
            target(),
            CuttlefishLifecycleOperation::Start,
            7,
        )
        .expect("valid lifecycle request");

        let result = adapter.lifecycle(request).expect("provider response");
        assert_eq!(result.lifecycle_state, CuttlefishVmLifecycleState::Starting);
        assert_eq!(result.generation, 8);
        assert_eq!(client.lifecycle_calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            adapter.package_manifest().packages.len(),
            AospStarterApp::ALL.len()
        );
        assert_eq!(
            adapter.package_manifest().image_provenance.image_digest,
            DIGEST
        );
    }

    #[test]
    fn invalid_transition_is_rejected_before_provider_contact() {
        let initial = observation(CuttlefishVmLifecycleState::Stopped, 7, 100);
        let (adapter, client) = adapter(
            initial,
            Ok(observation(CuttlefishVmLifecycleState::Starting, 8, 200)),
        );
        let request = CuttlefishLifecycleRequest::new(
            "stop-t480-01",
            target(),
            CuttlefishLifecycleOperation::Stop,
            7,
        )
        .expect("shape-valid lifecycle request");

        assert_eq!(
            adapter.lifecycle(request),
            Err(CuttlefishProviderError::Contract(
                CuttlefishContractError::OperationNotAllowed {
                    operation: CuttlefishLifecycleOperation::Stop,
                    state: CuttlefishVmLifecycleState::Stopped,
                }
            ))
        );
        assert_eq!(client.lifecycle_calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn stale_generation_is_rejected_before_provider_contact() {
        let initial = observation(CuttlefishVmLifecycleState::Stopped, 7, 100);
        let (adapter, client) = adapter(
            initial,
            Ok(observation(CuttlefishVmLifecycleState::Starting, 8, 200)),
        );
        let request = CuttlefishLifecycleRequest::new(
            "start-t480-stale",
            target(),
            CuttlefishLifecycleOperation::Start,
            6,
        )
        .expect("shape-valid stale request");

        assert_eq!(
            adapter.lifecycle(request),
            Err(CuttlefishProviderError::Contract(
                CuttlefishContractError::GenerationMismatch {
                    expected: 6,
                    actual: 7,
                }
            ))
        );
        assert_eq!(client.lifecycle_calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn provider_failure_leaves_the_last_admitted_observation_intact() {
        let initial = observation(CuttlefishVmLifecycleState::Stopped, 7, 100);
        let (adapter, client) = adapter(
            initial.clone(),
            Err(CuttlefishProviderError::ProviderUnavailable),
        );
        let request = CuttlefishLifecycleRequest::new(
            "start-t480-failure",
            target(),
            CuttlefishLifecycleOperation::Start,
            7,
        )
        .expect("valid lifecycle request");

        assert_eq!(
            adapter.lifecycle(request),
            Err(CuttlefishProviderError::ProviderUnavailable)
        );
        assert_eq!(client.lifecycle_calls.load(Ordering::Relaxed), 1);
        assert_eq!(adapter.current_observation().expect("state").generation, 7);
        assert_eq!(
            adapter
                .current_observation()
                .expect("state")
                .lifecycle_state,
            CuttlefishVmLifecycleState::Stopped
        );
    }

    #[test]
    fn registry_reaches_typed_readiness_and_keeps_package_inventory_pending() {
        let initial = observation(CuttlefishVmLifecycleState::Running, 7, now_unix_ms());
        let client = FakeClient::new(
            Ok(initial.clone()),
            Err(CuttlefishProviderError::ProviderRejected),
        );
        let client_handle = client.clone();
        let mut registry = super::super::AndroidGuestProviderRegistry::new();
        registry
            .register_cuttlefish_provider(
                "android-t480",
                target(),
                package_manifest(),
                initial,
                client,
            )
            .expect("register Cuttlefish adapter");

        let request = mackes_mesh_types::android_apps::AndroidGuestRequest::inventory(
            "inventory-t480-01",
            "android-t480",
        )
        .expect("inventory request");
        let response = registry.dispatch(request.clone()).expect("typed response");
        assert!(response.validate_against(&request).is_ok());
        match response {
            mackes_mesh_types::android_apps::AndroidGuestResponse::Inventory(response) => {
                assert_eq!(
                    response
                        .inventory
                        .image_provenance
                        .as_ref()
                        .map(|p| p.image_id.as_str()),
                    Some("android-cuttlefish-v1")
                );
                assert_eq!(
                    response.inventory.unavailable_reason,
                    Some(AndroidUnavailableReason::ProviderUnavailable)
                );
                assert!(response
                    .inventory
                    .entries
                    .iter()
                    .all(|entry| !entry.is_launchable()));
            }
            mackes_mesh_types::android_apps::AndroidGuestResponse::Launch(_) => {
                panic!("inventory request returned launch response")
            }
        }
        assert_eq!(client_handle.observe_calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn launch_waits_for_guest_owned_ready_evidence() {
        let initial = observation(CuttlefishVmLifecycleState::Starting, 7, now_unix_ms());
        let client = FakeClient::new(
            Ok(initial.clone()),
            Err(CuttlefishProviderError::ProviderUnavailable),
        );
        let calls = client.launch_calls.clone();
        let adapter = CuttlefishProviderAdapter::new(
            "android-t480",
            target(),
            package_manifest(),
            initial,
            client,
        )
        .expect("adapter");
        let request = AndroidGuestLaunchRequest::for_app(
            "launch-t480-not-ready",
            "android-t480",
            AospStarterApp::Browser,
        )
        .expect("launch request");

        // A booting guest is not a package/session target. A launch must
        // therefore remain unavailable and must not contact the guest session
        // backend.
        assert_eq!(
            adapter.launch(&request),
            AndroidGuestLaunchOutcome::Unavailable
        );
        assert_eq!(calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn launch_reaches_backend_only_after_guest_ready_observation() {
        let initial = observation(CuttlefishVmLifecycleState::Running, 7, now_unix_ms());
        let client = FakeClient::new(
            Ok(initial.clone()),
            Err(CuttlefishProviderError::ProviderUnavailable),
        );
        let calls = client.launch_calls.clone();
        // This fixture's Running observation carries guest-ready evidence, so
        // a typed client launch is permitted. The inventory layer still stays
        // conservative until it receives package-manager evidence separately.
        let adapter = CuttlefishProviderAdapter::new(
            "android-t480",
            target(),
            package_manifest(),
            initial,
            client,
        )
        .expect("adapter");
        let request = AndroidGuestLaunchRequest::for_app(
            "launch-t480-ready",
            "android-t480",
            AospStarterApp::Browser,
        )
        .expect("launch request");

        assert_eq!(adapter.launch(&request), AndroidGuestLaunchOutcome::Started);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn package_manifest_drift_is_rejected_without_contact() {
        let mut manifest = package_manifest();
        manifest.image_provenance.image_id = "different-image".to_owned();
        let client = FakeClient::new(
            Ok(observation(CuttlefishVmLifecycleState::Absent, 0, 100)),
            Err(CuttlefishProviderError::ProviderUnavailable),
        );
        let calls = client.lifecycle_calls.clone();
        assert!(matches!(
            CuttlefishProviderAdapter::new(
                "android-t480",
                target(),
                manifest,
                observation(CuttlefishVmLifecycleState::Absent, 0, 100),
                client,
            ),
            Err(CuttlefishProviderError::ImagePackageProvenanceMismatch)
        ));
        assert_eq!(calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn libvirt_client_projects_active_outer_vm_as_booting_not_guest_ready() {
        let runner = crate::workers::cloud::runner::fake::FakeRunner {
            roster: vec![CloudInstance {
                id: "android-t480".to_owned(),
                name: "android-t480".to_owned(),
                status: "ACTIVE".to_owned(),
                flavor: None,
                image: None,
                networks: None,
            }],
            ..Default::default()
        };
        let client = LibvirtCuttlefishProviderClient::new(Arc::new(runner));

        let observed = client.observe(&target()).expect("outer VM observation");
        assert_eq!(
            observed.lifecycle_state,
            CuttlefishVmLifecycleState::Starting
        );
        assert_eq!(observed.guest.readiness, CuttlefishGuestReadiness::NotReady);
        assert!(!observed.is_guest_ready());
    }

    #[test]
    fn libvirt_client_drives_typed_lifecycle_and_destroy_resets_generation() {
        let runner = Arc::new(crate::workers::cloud::runner::fake::FakeRunner {
            roster: vec![CloudInstance {
                id: "android-t480".to_owned(),
                name: "android-t480".to_owned(),
                status: "SHUTOFF".to_owned(),
                flavor: None,
                image: None,
                networks: None,
            }],
            ..Default::default()
        });
        let client = LibvirtCuttlefishProviderClient::new(runner.clone());
        let stopped = client.observe(&target()).expect("stopped observation");
        assert_eq!(stopped.generation, 1);

        let start = CuttlefishLifecycleRequest::new(
            "start-t480-production",
            target(),
            CuttlefishLifecycleOperation::Start,
            stopped.generation,
        )
        .expect("start request");
        let starting = client.lifecycle(&start).expect("start lifecycle");
        assert_eq!(
            starting.lifecycle_state,
            CuttlefishVmLifecycleState::Starting
        );
        assert_eq!(starting.generation, 2);

        let destroy = CuttlefishLifecycleRequest::new(
            "destroy-t480-production",
            target(),
            CuttlefishLifecycleOperation::Destroy,
            starting.generation,
        )
        .expect("destroy request");
        let absent = client.lifecycle(&destroy).expect("destroy lifecycle");
        assert_eq!(absent.lifecycle_state, CuttlefishVmLifecycleState::Absent);
        assert_eq!(absent.generation, 0);
    }
}
