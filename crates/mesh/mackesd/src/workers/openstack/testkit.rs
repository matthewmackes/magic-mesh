//! QC-2 — shared in-memory fakes for the `openstack` worker's two seams,
//! used by the `reconcile` and `mod` unit tests (one fake per seam, no
//! duplication across test modules).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use crate::workers::container::{Container, ContainerState};

use super::fleet::{CloudDesired, FleetStateError, FleetStateSource};
use super::podman::{KollaServiceSpec, PodmanRunner, RunnerError};

/// An in-memory [`FleetStateSource`]: a fixed doctrine view or a typed gate.
pub struct FakeFleet {
    result: Result<CloudDesired, FleetStateError>,
}

impl FakeFleet {
    /// Always answer `view`.
    pub const fn fixed(view: CloudDesired) -> Self {
        Self { result: Ok(view) }
    }

    /// Always answer a typed `IntegrationGated` with `reason`.
    pub fn gated(reason: &str) -> Self {
        Self {
            result: Err(FleetStateError::IntegrationGated {
                reason: reason.to_string(),
            }),
        }
    }
}

impl FleetStateSource for FakeFleet {
    fn read(&self) -> Result<CloudDesired, FleetStateError> {
        self.result.clone()
    }
}

/// An in-memory [`PodmanRunner`] recording calls + container/image state, so
/// the whole converge pipeline runs without Podman.
pub struct FakeRunner {
    containers: Mutex<BTreeMap<String, ContainerState>>,
    images: Mutex<BTreeSet<String>>,
    calls: Mutex<Vec<String>>,
    /// Container names whose next `run_service` fails with the mapped reason.
    fail_runs: Mutex<BTreeMap<String, String>>,
    /// When set, every call answers [`RunnerError::PodmanAbsent`].
    absent: bool,
}

impl FakeRunner {
    /// A live fake with nothing running and no images mirrored.
    pub fn new() -> Self {
        Self {
            containers: Mutex::new(BTreeMap::new()),
            images: Mutex::new(BTreeSet::new()),
            calls: Mutex::new(Vec::new()),
            fail_runs: Mutex::new(BTreeMap::new()),
            absent: false,
        }
    }

    /// A podman-less host: every call answers the typed absence.
    pub fn absent() -> Self {
        Self {
            absent: true,
            ..Self::new()
        }
    }

    /// Seed a container in `state`.
    pub fn seed_container(&self, name: &str, state: ContainerState) {
        self.containers
            .lock()
            .unwrap()
            .insert(name.to_string(), state);
    }

    /// Mirror an image into the local store.
    pub fn seed_image(&self, image: &str) {
        self.images.lock().unwrap().insert(image.to_string());
    }

    /// Make the next `run_service` for `name` fail with `reason`.
    pub fn fail_next_run(&self, name: &str, reason: &str) {
        self.fail_runs
            .lock()
            .unwrap()
            .insert(name.to_string(), reason.to_string());
    }

    /// The recorded call log (`list` / `run:<name>` / `start:<name>` / …).
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn record(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }

    fn gate(&self) -> Result<(), RunnerError> {
        if self.absent {
            Err(RunnerError::PodmanAbsent {
                reason: "test fake: podman-less host".to_string(),
            })
        } else {
            Ok(())
        }
    }
}

impl PodmanRunner for FakeRunner {
    fn list(&self) -> Result<Vec<Container>, RunnerError> {
        self.record("list".to_string());
        self.gate()?;
        Ok(self
            .containers
            .lock()
            .unwrap()
            .iter()
            .map(|(name, state)| Container {
                id: "-".into(),
                name: name.clone(),
                image: "img".into(),
                state: state.as_podman_str().into(),
            })
            .collect())
    }

    fn image_present(&self, image: &str) -> Result<bool, RunnerError> {
        self.record(format!("image_present:{image}"));
        self.gate()?;
        Ok(self.images.lock().unwrap().contains(image))
    }

    fn run_service(&self, spec: &KollaServiceSpec) -> Result<(), RunnerError> {
        let name = spec.kind.container_name();
        self.record(format!("run:{name}"));
        self.gate()?;
        let planned_failure = self.fail_runs.lock().unwrap().remove(name);
        if let Some(reason) = planned_failure {
            return Err(RunnerError::Command {
                cmd: "run".into(),
                code: 125,
                stderr: reason,
            });
        }
        self.containers
            .lock()
            .unwrap()
            .insert(name.to_string(), ContainerState::Running);
        Ok(())
    }

    fn start_existing(&self, name: &str) -> Result<(), RunnerError> {
        self.record(format!("start:{name}"));
        self.gate()?;
        self.containers
            .lock()
            .unwrap()
            .insert(name.to_string(), ContainerState::Running);
        Ok(())
    }

    fn stop(&self, name: &str) -> Result<(), RunnerError> {
        self.record(format!("stop:{name}"));
        self.gate()?;
        self.containers
            .lock()
            .unwrap()
            .insert(name.to_string(), ContainerState::Exited);
        Ok(())
    }

    fn remove(&self, name: &str) -> Result<(), RunnerError> {
        self.record(format!("remove:{name}"));
        self.gate()?;
        self.containers.lock().unwrap().remove(name);
        Ok(())
    }
}
