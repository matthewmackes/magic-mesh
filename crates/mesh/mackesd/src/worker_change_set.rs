//! Generation-bound admission for the global Worker Action Console.
//!
//! This module deliberately has no provider fallback. A preview is staged only
//! when the canonical registry declares every exact action. Commit is refused
//! until a concrete authenticated handler is registered for every item.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use mackes_mesh_types::worker_runtime::{
    worker_change_set_action_topic, worker_change_set_result_topic, WorkerChangeSetItemOutcome,
    WorkerChangeSetItemResult, WorkerChangeSetOperation, WorkerChangeSetOutcome,
    WorkerChangeSetRequest, WorkerChangeSetResult, WorkerContract, WORKER_CHANGE_SET_AUTH_VERB,
    WORKER_RUNTIME_SCHEMA_VERSION,
};
use mde_bus::persist::Persist;

use crate::ipc::action_auth::{ActionAuthorizer, MutationContext};
use crate::workers::worker_runtime_status::{read_runtime_status_file, WorkerRuntimeNodeStatus};

const MAX_STAGED: usize = 64;
const MAX_REPLAYS: usize = 256;
const POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone)]
struct Stage {
    target: mackes_mesh_types::worker_runtime::WorkerChangeSetTarget,
    generation: u64,
    digest: String,
    items: Vec<mackes_mesh_types::worker_runtime::WorkerChangeSetItem>,
    expires_at_ms: u64,
}

/// Production Bus adapter for the typed Action Console protocol.
///
/// The adapter decodes only enough untrusted input to derive the immutable
/// request identity, then admits the exact retained body through the shared
/// root-only action authority. Only an admitted body reaches the executor.
/// Results are validated by their shared constructor and retained on the
/// canonical node result lane; an absent mutation handler therefore remains a
/// durable, typed refusal rather than an implicit success.
pub struct WorkerChangeSetConsumer {
    node_id: String,
    status_path: PathBuf,
    authorizer: Arc<ActionAuthorizer>,
    executor: Mutex<WorkerChangeSetExecutor>,
    cursor: Option<String>,
}

impl WorkerChangeSetConsumer {
    #[must_use]
    pub fn production(node_id: String, status_path: PathBuf) -> Self {
        Self {
            node_id,
            status_path,
            authorizer: Arc::new(ActionAuthorizer::production()),
            executor: Mutex::new(WorkerChangeSetExecutor::default()),
            cursor: None,
        }
    }

    #[cfg(test)]
    fn for_test(node_id: String, status_path: PathBuf, authorizer: Arc<ActionAuthorizer>) -> Self {
        Self {
            node_id,
            status_path,
            authorizer,
            executor: Mutex::new(WorkerChangeSetExecutor::default()),
            cursor: None,
        }
    }

    /// Drain one bounded Bus page. Malformed bodies have no trustworthy
    /// request identity and produce no result. Well-formed but unauthorized
    /// bodies produce a typed refusal without entering the executor.
    pub fn poll_once(&mut self, persist: &mut Persist, now_ms: u64) {
        let Ok(topic) = worker_change_set_action_topic(&self.node_id) else {
            return;
        };
        let Ok(messages) = persist.list_since_limit(&topic, self.cursor.as_deref(), 64) else {
            return;
        };
        for message in messages {
            self.cursor = Some(message.ulid.clone());
            let Some(body) = message.body.as_deref() else {
                continue;
            };
            let Ok(request) = WorkerChangeSetRequest::from_json(body) else {
                continue;
            };
            let result = self.admit_and_consume(body, request, now_ms);
            let Ok(result_topic) = worker_change_set_result_topic(&self.node_id) else {
                continue;
            };
            if let Ok(result_body) = result.to_json() {
                let _ = crate::bus_publish::publish_body(persist, &result_topic, &result_body);
            }
        }
    }

    fn admit_and_consume(
        &self,
        body: &str,
        request: WorkerChangeSetRequest,
        now_ms: u64,
    ) -> WorkerChangeSetResult {
        let generation = self.actual_generation(&request, now_ms).unwrap_or(1);
        let capability_target = format!("change-set:{}", request.request_id);
        let context = MutationContext {
            verb: WORKER_CHANGE_SET_AUTH_VERB,
            node: &self.node_id,
            target: &capability_target,
        };
        if request.target.node_id != self.node_id {
            return refused_result(request, generation, now_ms, "request targets another node");
        }
        if let Err(reason) = self.authorizer.authorize(body, context) {
            return refused_result(request, generation, now_ms, &reason);
        }
        let Some((actual_generation, contracts)) = self.actual_authority(&request, now_ms) else {
            return refused_result(
                request,
                generation,
                now_ms,
                "current supervisor authority is unavailable",
            );
        };
        self.executor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .consume(request, now_ms, actual_generation, &contracts)
    }

    fn actual_generation(&self, request: &WorkerChangeSetRequest, now_ms: u64) -> Option<u64> {
        self.actual_authority(request, now_ms)
            .map(|(generation, _)| generation)
    }

    fn actual_authority(
        &self,
        request: &WorkerChangeSetRequest,
        now_ms: u64,
    ) -> Option<(u64, Vec<WorkerContract>)> {
        let status = read_runtime_status_file(&self.status_path, now_ms).ok()?;
        authority_for_request(&status, request)
    }
}

fn authority_for_request(
    status: &WorkerRuntimeNodeStatus,
    request: &WorkerChangeSetRequest,
) -> Option<(u64, Vec<WorkerContract>)> {
    if status.node_id != request.target.node_id {
        return None;
    }
    let requested = request
        .items
        .iter()
        .map(|item| item.worker_id.as_str())
        .collect::<BTreeSet<_>>();
    let rows = status
        .workers
        .iter()
        .filter(|row| requested.contains(row.contract.worker_id.as_str()))
        .collect::<Vec<_>>();
    if rows.len() != requested.len() {
        return None;
    }
    let generation = rows.first()?.snapshot.generation;
    if generation == 0 || rows.iter().any(|row| row.snapshot.generation != generation) {
        return None;
    }
    Some((
        generation,
        rows.iter().map(|row| row.contract.clone()).collect(),
    ))
}

fn refused_result(
    request: WorkerChangeSetRequest,
    actual_generation: u64,
    now_ms: u64,
    detail: &str,
) -> WorkerChangeSetResult {
    WorkerChangeSetResult {
        schema_version: WORKER_RUNTIME_SCHEMA_VERSION,
        request_id: request.request_id,
        operation: request.operation,
        outcome: WorkerChangeSetOutcome::Refused,
        target: request.target,
        expected_generation: request.expected_generation,
        actual_generation: actual_generation.max(1),
        items: request
            .items
            .into_iter()
            .map(|item| WorkerChangeSetItemResult {
                item_id: item.item_id,
                outcome: WorkerChangeSetItemOutcome::Refused,
                detail: None,
            })
            .collect(),
        audit_id: None,
        completed_at_ms: now_ms.max(1),
        detail: Some(detail.to_owned()),
    }
}

/// Start the sole Actions-process consumer generation.
#[must_use]
pub fn spawn_consumer(
    node_id: String,
    bus_root: PathBuf,
    status_path: PathBuf,
    shutdown: Arc<AtomicBool>,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("worker-change-set".into())
        .spawn(move || {
            let mut consumer = WorkerChangeSetConsumer::production(node_id, status_path);
            let retry_root = bus_root.clone();
            let mut persist = Persist::open(bus_root).ok();
            while !shutdown.load(Ordering::Relaxed) {
                if persist.is_none() {
                    persist = Persist::open(retry_root.clone()).ok();
                }
                if let Some(bus) = persist.as_mut() {
                    consumer.poll_once(bus, wall_now_ms());
                }
                std::thread::sleep(POLL_INTERVAL);
            }
        })
}

fn wall_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// In-memory protocol authority. The Bus consumer owns one instance and must
/// supply the current supervisor generation for the exact target on every call.
#[derive(Debug, Default)]
pub struct WorkerChangeSetExecutor {
    staged: BTreeMap<String, Stage>,
    seen_requests: BTreeSet<String>,
}

impl WorkerChangeSetExecutor {
    /// Admit one already exact-body-authorized request and return one bound
    /// result. `contracts` must come from the canonical worker registry.
    pub fn consume(
        &mut self,
        request: WorkerChangeSetRequest,
        now_ms: u64,
        actual_generation: u64,
        contracts: &[WorkerContract],
    ) -> WorkerChangeSetResult {
        let operation = request.operation;
        let mut outcome = WorkerChangeSetOutcome::Refused;
        let detail;
        let mut item_outcome = WorkerChangeSetItemOutcome::Refused;

        let valid = request.validate_at(now_ms).is_ok()
            && request.armed_token.is_some()
            && !self.seen_requests.contains(&request.request_id)
            && actual_generation != 0;

        if !valid {
            detail = if now_ms > request.expires_at_ms {
                outcome = WorkerChangeSetOutcome::Expired;
                "change set expired"
            } else {
                "invalid, unauthorized, or replayed request"
            };
        } else if request.expected_generation != actual_generation {
            if operation != WorkerChangeSetOperation::Preview {
                outcome = WorkerChangeSetOutcome::StaleGeneration;
            }
            detail = "supervisor generation changed";
        } else {
            let declared = request.items.iter().all(|item| {
                contracts.iter().any(|contract| {
                    contract.worker_id == item.worker_id
                        && contract.actions.iter().any(|action| {
                            action.action == item.action && action.arming == request.arming
                        })
                })
            });
            match operation {
                WorkerChangeSetOperation::Preview if declared => {
                    if self.staged.len() < MAX_STAGED {
                        self.staged.insert(
                            request.digest.clone(),
                            Stage {
                                target: request.target.clone(),
                                generation: request.expected_generation,
                                digest: request.digest.clone(),
                                items: request.items.clone(),
                                expires_at_ms: request.expires_at_ms,
                            },
                        );
                        outcome = WorkerChangeSetOutcome::Previewed;
                        item_outcome = WorkerChangeSetItemOutcome::NotApplicable;
                        detail = "exact change set staged; no mutation executed";
                    } else {
                        detail = "staging capacity exhausted";
                    }
                }
                WorkerChangeSetOperation::Preview => detail = "action is not declared",
                WorkerChangeSetOperation::Commit => {
                    let exact = self.staged.get(&request.digest).is_some_and(|stage| {
                        stage.target == request.target
                            && stage.generation == request.expected_generation
                            && stage.digest == request.digest
                            && stage.items == request.items
                            && now_ms <= stage.expires_at_ms
                    });
                    detail = if exact {
                        "no authenticated mutation handler is registered"
                    } else {
                        "commit does not match an unexpired preview"
                    };
                }
                WorkerChangeSetOperation::Cancel => {
                    let exact = self.staged.get(&request.digest).is_some_and(|stage| {
                        stage.target == request.target
                            && stage.generation == request.expected_generation
                            && stage.items == request.items
                    });
                    if exact {
                        self.staged.remove(&request.digest);
                        outcome = WorkerChangeSetOutcome::Cancelled;
                        item_outcome = WorkerChangeSetItemOutcome::Cancelled;
                        detail = "staged change set cancelled";
                    } else {
                        detail = "cancel does not match a staged preview";
                    }
                }
            }
            self.remember(request.request_id.clone());
        }

        let items = request
            .items
            .iter()
            .map(|item| WorkerChangeSetItemResult {
                item_id: item.item_id.clone(),
                outcome: item_outcome,
                detail: None,
            })
            .collect();
        WorkerChangeSetResult {
            schema_version: WORKER_RUNTIME_SCHEMA_VERSION,
            request_id: request.request_id,
            operation,
            outcome,
            target: request.target,
            expected_generation: request.expected_generation,
            actual_generation: actual_generation.max(1),
            items,
            audit_id: None,
            completed_at_ms: now_ms.max(1),
            detail: Some(detail.to_string()),
        }
    }

    fn remember(&mut self, request_id: String) {
        if self.seen_requests.len() == MAX_REPLAYS {
            if let Some(oldest) = self.seen_requests.iter().next().cloned() {
                self.seen_requests.remove(&oldest);
            }
        }
        self.seen_requests.insert(request_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use crate::ipc::action_auth::authorize_test_body;
    use crate::workers::worker_runtime_status::{
        project_status, write_runtime_status_file, WorkerRuntimeNodeStatus,
    };
    use mackes_mesh_types::worker_runtime::{
        worker_change_set_digest, WorkerAction, WorkerActionDescriptor, WorkerArmingRequirement,
        WorkerChangeSetItem, WorkerChangeSetTarget, WorkerGroup, WorkerRuntimeSnapshot,
        WorkerRuntimeState,
    };

    const KEY: &[u8] = b"worker-change-set-action-authority-test-key";
    const NOW: u64 = 1_700_000_000_000;

    fn contract() -> WorkerContract {
        let mut contract =
            WorkerContract::new("safe-worker", WorkerGroup::Actions, "safe").expect("contract");
        contract.actions.push(WorkerActionDescriptor {
            action: WorkerAction::Refresh,
            label: "Refresh".into(),
            arming: WorkerArmingRequirement::Confirmation,
        });
        contract.admitted().expect("admitted contract")
    }

    fn request(
        id: &str,
        operation: WorkerChangeSetOperation,
        generation: u64,
    ) -> WorkerChangeSetRequest {
        let target = WorkerChangeSetTarget {
            node_id: "node-1".into(),
            worker_id: Some("safe-worker".into()),
        };
        let items = vec![WorkerChangeSetItem {
            item_id: "item-1".into(),
            worker_id: "safe-worker".into(),
            action: WorkerAction::Refresh,
        }];
        let digest = worker_change_set_digest(
            &target,
            generation,
            &items,
            "refresh",
            "retry",
            WorkerArmingRequirement::Confirmation,
        )
        .expect("digest");
        let mut request = WorkerChangeSetRequest::new(
            id,
            operation,
            target,
            generation,
            items,
            "refresh",
            "retry",
            WorkerArmingRequirement::Confirmation,
            digest,
            1000,
            2000,
        )
        .expect("request");
        request.armed_token = Some(format!("token-{id}"));
        request
    }

    fn unsigned_request(
        id: &str,
        operation: WorkerChangeSetOperation,
        generation: u64,
    ) -> WorkerChangeSetRequest {
        let mut request = request(id, operation, generation);
        request.armed_token = None;
        request.requested_at_ms = NOW;
        request.expires_at_ms = NOW + 20_000;
        request
    }

    fn status_file(root: &Path, generation: u64) -> PathBuf {
        let contract = contract();
        let snapshot = WorkerRuntimeSnapshot::new(
            format!("safe-worker-{generation}"),
            "node-1",
            "safe-worker",
            WorkerGroup::Actions,
            generation,
            WorkerRuntimeState::Running,
            NOW - 1_000,
            NOW,
            NOW,
            NOW + 15_000,
        )
        .expect("runtime snapshot");
        let row = project_status(&contract, snapshot, NOW).expect("runtime status");
        let node = WorkerRuntimeNodeStatus {
            schema_version: WORKER_RUNTIME_SCHEMA_VERSION,
            node_id: "node-1".into(),
            observed_at_ms: NOW,
            workers: vec![row],
        };
        let path = root.join("status.json");
        write_runtime_status_file(&path, &node).expect("write status");
        path
    }

    fn signed_body(request: &WorkerChangeSetRequest, node: &str, nonce: &str) -> String {
        let unsigned = request.to_json().expect("unsigned request");
        let target = format!("change-set:{}", request.request_id);
        authorize_test_body(
            KEY,
            &unsigned,
            MutationContext {
                verb: WORKER_CHANGE_SET_AUTH_VERB,
                node,
                target: &target,
            },
            nonce,
            i64::try_from(NOW + 20_000).unwrap(),
        )
    }

    #[test]
    fn hostile_change_set_lifecycle_is_exact_generation_bound_and_fail_closed() {
        let mut executor = WorkerChangeSetExecutor::default();
        let contracts = vec![contract()];
        let preview = request("preview-1", WorkerChangeSetOperation::Preview, 7);
        assert_eq!(
            executor
                .consume(preview.clone(), 1100, 7, &contracts)
                .outcome,
            WorkerChangeSetOutcome::Previewed
        );
        assert_eq!(
            executor.consume(preview, 1101, 7, &contracts).outcome,
            WorkerChangeSetOutcome::Refused
        );
        assert_eq!(
            executor
                .consume(
                    request("stale", WorkerChangeSetOperation::Commit, 7),
                    1200,
                    8,
                    &contracts
                )
                .outcome,
            WorkerChangeSetOutcome::StaleGeneration
        );
        let substituted = request("substitute", WorkerChangeSetOperation::Commit, 8);
        assert_eq!(
            executor.consume(substituted, 1200, 8, &contracts).outcome,
            WorkerChangeSetOutcome::Refused
        );
        let commit = request("commit-1", WorkerChangeSetOperation::Commit, 7);
        assert_eq!(
            executor.consume(commit, 1200, 7, &contracts).outcome,
            WorkerChangeSetOutcome::Refused
        );
        let cancel = request("cancel-1", WorkerChangeSetOperation::Cancel, 7);
        assert_eq!(
            executor.consume(cancel, 1300, 7, &contracts).outcome,
            WorkerChangeSetOutcome::Cancelled
        );
        let second_cancel = request("cancel-2", WorkerChangeSetOperation::Cancel, 7);
        assert_eq!(
            executor.consume(second_cancel, 1301, 7, &contracts).outcome,
            WorkerChangeSetOutcome::Refused
        );
    }

    #[test]
    fn undeclared_action_never_stages() {
        let mut executor = WorkerChangeSetExecutor::default();
        let result = executor.consume(
            request("preview-2", WorkerChangeSetOperation::Preview, 3),
            1100,
            3,
            &[],
        );
        assert_eq!(result.outcome, WorkerChangeSetOutcome::Refused);
    }

    #[test]
    fn bus_admission_rejects_wrong_body_identity_and_replay_before_dispatch() {
        let temp = tempfile::tempdir().expect("temp root");
        let status_path = status_file(temp.path(), 7);
        let authorizer = Arc::new(ActionAuthorizer::for_test(
            KEY,
            temp.path().join("auth"),
            i64::try_from(NOW).unwrap(),
        ));
        let consumer = WorkerChangeSetConsumer::for_test("node-1".into(), status_path, authorizer);

        let request = unsigned_request("preview-auth", WorkerChangeSetOperation::Preview, 7);
        let body = signed_body(&request, "node-1", "nonce-body");
        let substituted_body = body.replacen("preview-auth", "preview-tampered", 1);
        let substituted = WorkerChangeSetRequest::from_json(&substituted_body)
            .expect("request id is outside staged digest");
        assert_eq!(
            consumer
                .admit_and_consume(&substituted_body, substituted, NOW)
                .outcome,
            WorkerChangeSetOutcome::Refused
        );

        let wrong_identity =
            unsigned_request("preview-identity", WorkerChangeSetOperation::Preview, 7);
        let wrong_identity_body = signed_body(&wrong_identity, "other-node", "nonce-identity");
        let wrong_identity =
            WorkerChangeSetRequest::from_json(&wrong_identity_body).expect("signed request");
        assert_eq!(
            consumer
                .admit_and_consume(&wrong_identity_body, wrong_identity, NOW)
                .outcome,
            WorkerChangeSetOutcome::Refused
        );

        let replay = unsigned_request("preview-replay", WorkerChangeSetOperation::Preview, 7);
        let replay_body = signed_body(&replay, "node-1", "nonce-replay");
        let admitted = WorkerChangeSetRequest::from_json(&replay_body).expect("signed request");
        assert_eq!(
            consumer
                .admit_and_consume(&replay_body, admitted.clone(), NOW)
                .outcome,
            WorkerChangeSetOutcome::Previewed
        );
        assert_eq!(
            consumer
                .admit_and_consume(&replay_body, admitted, NOW)
                .outcome,
            WorkerChangeSetOutcome::Refused
        );
    }
}
