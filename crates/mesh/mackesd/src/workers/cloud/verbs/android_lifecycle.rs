//! WL-FUNC-020 S3 — durable, generation-checked Android app lifecycle.
//!
//! The outer VM is mutated only through `CloudRunner`; the inner guest is
//! reached only through the closed Android provider boundary. No request can
//! carry an adb/qemu command, package string, intent, path, or endpoint.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use mackes_mesh_types::android_apps::{
    AndroidGuestBootState, AndroidGuestInventoryRequest, AndroidGuestLaunchOutcome,
    AndroidGuestLaunchRequest, AospStarterApp,
};
use mackes_mesh_types::cloud::{CloudReply, LifecycleAction};
use serde::{Deserialize, Serialize};

use super::super::CloudWorker;

const SCHEMA_VERSION: u16 = 1;
const MAX_STATE_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Operation {
    Start,
    Stop,
    Cancel,
    Retry,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    schema_version: u16,
    node: String,
    workload_id: String,
    request_id: String,
    expected_generation: u64,
    operation: Operation,
    #[serde(default)]
    app: Option<AospStarterApp>,
    #[serde(default)]
    armed_token: Option<String>,
    #[serde(default)]
    typed_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Stopped,
    Starting,
    CheckingGuest,
    Running,
    Stopping,
    Cancelled,
    Failed,
}

impl Phase {
    const fn transitional(self) -> bool {
        matches!(self, Self::Starting | Self::CheckingGuest | Self::Stopping)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    schema_version: u16,
    workload_id: String,
    generation: u64,
    phase: Phase,
    app: Option<AospStarterApp>,
    last_request_id: Option<String>,
    last_operation: Option<Operation>,
    last_ok: bool,
    failure: Option<String>,
}

impl State {
    fn initial(workload_id: String) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            workload_id,
            generation: 0,
            phase: Phase::Stopped,
            app: None,
            last_request_id: None,
            last_operation: None,
            last_ok: true,
            failure: None,
        }
    }
}

pub(super) fn authorization_target(raw: &str) -> Result<String, String> {
    let request = parse(raw)?;
    Ok(request.workload_id)
}

pub(super) fn handle(worker: &CloudWorker, verb: &str, raw: &str) -> CloudReply {
    let request = match parse(raw) {
        Ok(request) => request,
        Err(error) => return failure(verb, error),
    };
    let _guard = match worker.android_lifecycle_lock.lock() {
        Ok(guard) => guard,
        Err(_) => return failure(verb, "Android lifecycle authority is unavailable"),
    };
    let path = match state_path(&worker.state_root, &request.workload_id) {
        Ok(path) => path,
        Err(error) => return failure(verb, error),
    };
    let mut state = match load(&path, &request.workload_id) {
        Ok(state) => state,
        Err(error) => return failure(verb, error),
    };

    if state.last_request_id.as_deref() == Some(&request.request_id)
        && state.last_operation == Some(request.operation)
    {
        return reply(verb, &state);
    }
    if state.phase.transitional() {
        let cleanup = worker
            .runner
            .lifecycle(LifecycleAction::Stop, &request.workload_id);
        state.phase = Phase::Failed;
        state.app = None;
        state.last_ok = false;
        state.failure = Some(if cleanup.ok && cleanup.applied {
            "recovered an interrupted lifecycle and stopped the outer VM; retry explicitly"
                .to_owned()
        } else {
            "interrupted lifecycle recovery could not confirm outer-VM cleanup".to_owned()
        });
        if let Err(error) = persist(&path, &state) {
            return failure(verb, error);
        }
        return reply(verb, &state);
    }
    if request.expected_generation != state.generation {
        return failure(
            verb,
            format!(
                "stale Android lifecycle generation: expected {}, current {}",
                request.expected_generation, state.generation
            ),
        );
    }

    state.generation = state.generation.saturating_add(1);
    state.last_request_id = Some(request.request_id.clone());
    state.last_operation = Some(request.operation);
    state.failure = None;
    let result = match request.operation {
        Operation::Start => start(worker, &request, &path, &mut state, request.app),
        Operation::Retry => {
            let app = request.app.or(state.app);
            if state.phase != Phase::Failed && state.phase != Phase::Cancelled {
                Err("retry requires failed or cancelled lifecycle state".to_owned())
            } else {
                start(worker, &request, &path, &mut state, app)
            }
        }
        Operation::Stop | Operation::Cancel => stop(worker, &request, &path, &mut state),
    };
    if let Err(error) = result {
        let error = if matches!(request.operation, Operation::Start | Operation::Retry) {
            let _ = worker.android_guest_providers.cleanup(
                &request.workload_id,
                &format!("{}-failure-cleanup", request.request_id),
                state.generation,
            );
            let cleanup = worker
                .runner
                .lifecycle(LifecycleAction::Stop, &request.workload_id);
            if cleanup.ok && cleanup.applied {
                error
            } else {
                format!(
                    "{error}; outer-VM cleanup was not confirmed: {}",
                    cleanup.summary
                )
            }
        } else {
            error
        };
        state.phase = if request.operation == Operation::Cancel {
            Phase::Cancelled
        } else {
            Phase::Failed
        };
        state.app = None;
        clear_vdi_source(worker, &request.workload_id);
        state.last_ok = false;
        state.failure = Some(error);
        if let Err(error) = persist(&path, &state) {
            return failure(verb, error);
        }
    }
    reply(verb, &state)
}

fn start(
    worker: &CloudWorker,
    request: &Request,
    path: &Path,
    state: &mut State,
    app: Option<AospStarterApp>,
) -> Result<(), String> {
    if !matches!(
        state.phase,
        Phase::Stopped | Phase::Failed | Phase::Cancelled
    ) {
        return Err("start requires stopped, failed, or cancelled lifecycle state".to_owned());
    }
    let app = app.ok_or_else(|| "start/retry requires one approved starter app".to_owned())?;
    let admission_ready = worker
        .android_provider_admissions
        .lock()
        .map_err(|_| "Android provider admission authority is unavailable".to_owned())?
        .iter()
        .any(|row| row.workload_id == request.workload_id && row.is_ready());
    if !admission_ready {
        return Err("signed catalog/image/provider preflight is not ready".to_owned());
    }
    let catalog = worker
        .load_admitted_android_catalog()
        .ok_or_else(|| "admitted signed Android catalog is unavailable".to_owned())?;
    if !catalog
        .payload
        .app_policies
        .iter()
        .any(|policy| policy.app == app)
    {
        return Err("requested app is not approved by the admitted catalog".to_owned());
    }
    let manifest =
        super::super::load_android_package_manifest(&worker.state_root, &request.workload_id)
            .ok_or_else(|| "admitted Android package manifest is unavailable".to_owned())?;
    if manifest != catalog.payload.package_manifest {
        return Err("Android package manifest no longer matches the signed catalog".to_owned());
    }

    state.phase = Phase::Starting;
    state.app = Some(app);
    state.last_ok = false;
    persist(path, state)?;
    let outer = worker
        .runner
        .lifecycle(LifecycleAction::Start, &request.workload_id);
    if !outer.ok || !outer.applied {
        return Err(format!("outer VM start failed: {}", outer.summary));
    }

    state.phase = Phase::CheckingGuest;
    persist(path, state)?;
    let inventory_request = AndroidGuestInventoryRequest::new(
        format!("{}-inventory", request.request_id),
        request.workload_id.clone(),
    )
    .map_err(|error| format!("invalid guest inventory request: {error:?}"))?;
    let inventory = worker
        .android_guest_providers
        .inventory_at(&inventory_request, state.generation);
    let launchable = inventory.guest_boot_state == AndroidGuestBootState::Ready
        && inventory
            .entries
            .iter()
            .any(|entry| entry.descriptor.app == app && entry.is_launchable());
    if !launchable {
        return Err("guest did not prove approved package/install/launcher readiness".to_owned());
    }
    let launch = AndroidGuestLaunchRequest::for_app(
        format!("{}-launch", request.request_id),
        request.workload_id.clone(),
        app,
    )
    .map_err(|error| format!("invalid guest launch request: {error:?}"))?;
    let outcome = worker
        .android_guest_providers
        .launch_at(&launch, state.generation);
    if !matches!(
        outcome,
        AndroidGuestLaunchOutcome::Started | AndroidGuestLaunchOutcome::AlreadyRunning
    ) {
        return Err(format!("guest launch was not ready: {outcome:?}"));
    }
    let source = worker
        .android_guest_providers
        .vdi_source(&request.workload_id, state.generation)
        .ok_or_else(|| "guest launch produced no admitted VDI source".to_owned())?;
    let mut sources = worker
        .android_vdi_sources
        .lock()
        .map_err(|_| "Android VDI source authority is unavailable".to_owned())?;
    sources.retain(|row| row.workload_id != request.workload_id);
    sources.push(source);
    sources.sort_by(|left, right| left.workload_id.cmp(&right.workload_id));
    state.phase = Phase::Running;
    state.last_ok = true;
    persist(path, state)
}

fn stop(
    worker: &CloudWorker,
    request: &Request,
    path: &Path,
    state: &mut State,
) -> Result<(), String> {
    state.phase = Phase::Stopping;
    persist(path, state)?;
    let _ = worker.android_guest_providers.cleanup(
        &request.workload_id,
        &format!("{}-cleanup", request.request_id),
        state.generation,
    );
    let outcome = worker
        .runner
        .lifecycle(LifecycleAction::Stop, &request.workload_id);
    if !outcome.ok || !outcome.applied {
        return Err(format!("outer VM cleanup failed: {}", outcome.summary));
    }
    state.phase = if request.operation == Operation::Cancel {
        Phase::Cancelled
    } else {
        Phase::Stopped
    };
    state.app = None;
    clear_vdi_source(worker, &request.workload_id);
    state.last_ok = true;
    persist(path, state)
}

fn clear_vdi_source(worker: &CloudWorker, workload_id: &str) {
    if let Ok(mut sources) = worker.android_vdi_sources.lock() {
        sources.retain(|row| row.workload_id != workload_id);
    }
}

fn parse(raw: &str) -> Result<Request, String> {
    if raw.len() > super::MAX_CLOUD_ACTION_BODY_BYTES {
        return Err("Android lifecycle request exceeds the action bound".to_owned());
    }
    let request: Request = serde_json::from_str(raw)
        .map_err(|error| format!("invalid Android lifecycle request: {error}"))?;
    if request.schema_version != SCHEMA_VERSION {
        return Err("unsupported Android lifecycle schema".to_owned());
    }
    super::super::path_key::segment("node", &request.node)?;
    super::super::path_key::segment("workload_id", &request.workload_id)?;
    super::super::path_key::segment("request_id", &request.request_id)?;
    if request.operation == Operation::Start && request.app.is_none() {
        return Err("start requires an approved `app`".to_owned());
    }
    if matches!(request.operation, Operation::Stop | Operation::Cancel) && request.app.is_some() {
        return Err("stop/cancel must not carry an app".to_owned());
    }
    Ok(request)
}

fn state_path(root: &Path, workload_id: &str) -> Result<PathBuf, String> {
    let key = super::super::path_key::file_stem("workload_id", workload_id, ".json")?;
    Ok(root
        .join("mcnf/cloud/android-lifecycle")
        .join(format!("{key}.json")))
}

fn load(path: &Path, workload_id: &str) -> Result<State, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(State::initial(workload_id.to_owned()))
        }
        Err(error) => return Err(format!("reading Android lifecycle state: {error}")),
    };
    if !metadata.file_type().is_file() || metadata.len() > MAX_STATE_BYTES as u64 {
        return Err("Android lifecycle state is not an admitted regular file".to_owned());
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|error| format!("opening Android lifecycle state: {error}"))?
        .take((MAX_STATE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("reading Android lifecycle state: {error}"))?;
    if bytes.len() > MAX_STATE_BYTES {
        return Err("Android lifecycle state exceeds its bound".to_owned());
    }
    let state: State = serde_json::from_slice(&bytes)
        .map_err(|error| format!("decoding Android lifecycle state: {error}"))?;
    if state.schema_version != SCHEMA_VERSION || state.workload_id != workload_id {
        return Err("Android lifecycle state identity/schema mismatch".to_owned());
    }
    Ok(state)
}

fn persist(path: &Path, state: &State) -> Result<(), String> {
    let bytes = serde_json::to_vec(state)
        .map_err(|error| format!("encoding Android lifecycle state: {error}"))?;
    if bytes.len() > MAX_STATE_BYTES {
        return Err("Android lifecycle state exceeds its bound".to_owned());
    }
    let parent = path
        .parent()
        .ok_or("Android lifecycle state path has no parent")?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("creating Android lifecycle state directory: {error}"))?;
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| format!("creating Android lifecycle state: {error}"))?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("writing Android lifecycle state: {error}"))?;
        fs::rename(&temporary, path)
            .map_err(|error| format!("replacing Android lifecycle state: {error}"))?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("syncing Android lifecycle state directory: {error}"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn reply(verb: &str, state: &State) -> CloudReply {
    CloudReply {
        ok: state.last_ok,
        verb: verb.to_owned(),
        error: state.failure.clone(),
        raw_log: serde_json::to_string(state).ok(),
        ..Default::default()
    }
}

fn failure(verb: &str, error: impl Into<String>) -> CloudReply {
    CloudReply {
        ok: false,
        verb: verb.to_owned(),
        error: Some(error.into()),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::workers::cloud::runner::fake::FakeRunner;

    fn request(operation: &str, request_id: &str, generation: u64) -> String {
        format!(
            r#"{{"schema_version":1,"node":"node-a","workload_id":"android-one","request_id":"{request_id}","expected_generation":{generation},"operation":"{operation}"}}"#
        )
    }

    fn worker(root: &Path, runner: Arc<FakeRunner>) -> CloudWorker {
        CloudWorker::new("node-a".into(), "test".into(), root.to_path_buf()).with_runner(runner)
    }

    #[test]
    fn stop_is_durable_generation_checked_and_idempotent() {
        let temporary = tempfile::tempdir().expect("temporary state root");
        let runner = Arc::new(FakeRunner::default());
        let worker = worker(temporary.path(), runner.clone());
        let body = request("stop", "stop-1", 0);

        let first = handle(&worker, "android-lifecycle", &body);
        assert!(first.ok, "stop should clean one exact outer VM: {first:?}");
        let replay = handle(&worker, "android-lifecycle", &body);
        assert!(replay.ok, "same request is idempotent: {replay:?}");
        assert_eq!(
            runner.calls.lock().expect("runner calls").as_slice(),
            &[("lifecycle-stop".to_owned(), true)]
        );

        let state = load(
            &state_path(temporary.path(), "android-one").expect("state path"),
            "android-one",
        )
        .expect("durable state");
        assert_eq!(state.generation, 1);
        assert_eq!(state.phase, Phase::Stopped);
        assert!(state.app.is_none());
    }

    #[test]
    fn stale_generation_has_no_runtime_effect() {
        let temporary = tempfile::tempdir().expect("temporary state root");
        let runner = Arc::new(FakeRunner::default());
        let worker = worker(temporary.path(), runner.clone());
        assert!(handle(&worker, "android-lifecycle", &request("stop", "stop-1", 0)).ok);

        let stale = handle(
            &worker,
            "android-lifecycle",
            &request("cancel", "cancel-stale", 0),
        );
        assert!(!stale.ok);
        assert!(stale.error.as_deref().unwrap_or_default().contains("stale"));
        assert_eq!(runner.calls.lock().expect("runner calls").len(), 1);
    }

    #[test]
    fn restart_recovers_transitional_state_before_accepting_new_work() {
        let temporary = tempfile::tempdir().expect("temporary state root");
        let path = state_path(temporary.path(), "android-one").expect("state path");
        let mut interrupted = State::initial("android-one".to_owned());
        interrupted.generation = 7;
        interrupted.phase = Phase::CheckingGuest;
        interrupted.app = Some(AospStarterApp::Browser);
        interrupted.last_request_id = Some("start-before-crash".to_owned());
        interrupted.last_operation = Some(Operation::Start);
        persist(&path, &interrupted).expect("interrupted journal");

        let runner = Arc::new(FakeRunner::default());
        let restarted = worker(temporary.path(), runner.clone());
        let recovery = handle(
            &restarted,
            "android-lifecycle",
            &request("cancel", "cancel-after-crash", 7),
        );
        assert!(!recovery.ok, "recovery is explicit, never fake success");
        assert!(recovery
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("interrupted lifecycle"));
        assert_eq!(
            runner.calls.lock().expect("runner calls").as_slice(),
            &[("lifecycle-stop".to_owned(), true)]
        );
        let recovered = load(&path, "android-one").expect("recovered journal");
        assert_eq!(recovered.phase, Phase::Failed);
        assert_eq!(recovered.generation, 7);
        assert!(recovered.app.is_none());
    }

    #[test]
    fn failed_start_preflight_still_cleans_the_exact_outer_vm() {
        let temporary = tempfile::tempdir().expect("temporary state root");
        let runner = Arc::new(FakeRunner::default());
        let worker = worker(temporary.path(), runner.clone());
        let body = r#"{"schema_version":1,"node":"node-a","workload_id":"android-one","request_id":"start-1","expected_generation":0,"operation":"start","app":"browser"}"#;

        let reply = handle(&worker, "android-lifecycle", body);
        assert!(!reply.ok, "missing S1/S2 authority must fail closed");
        assert!(reply
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("preflight"));
        assert_eq!(
            runner.calls.lock().expect("runner calls").as_slice(),
            &[("lifecycle-stop".to_owned(), true)]
        );
        let state = load(
            &state_path(temporary.path(), "android-one").expect("state path"),
            "android-one",
        )
        .expect("failed state");
        assert_eq!(state.phase, Phase::Failed);
        assert_eq!(state.generation, 1);
        assert!(state.app.is_none());
    }
}
