//! Generation-bound admission for the global Worker Action Console.
//!
//! This module deliberately has no provider fallback. A preview is staged only
//! when the canonical registry declares every exact action. Commit is refused
//! until a concrete authenticated handler is registered for every item.

use std::collections::{BTreeMap, BTreeSet};

use mackes_mesh_types::worker_runtime::{
    WorkerChangeSetItemOutcome,
    WorkerChangeSetItemResult, WorkerChangeSetOperation, WorkerChangeSetOutcome,
    WorkerChangeSetRequest, WorkerChangeSetResult, WorkerContract,
    WORKER_RUNTIME_SCHEMA_VERSION,
};

const MAX_STAGED: usize = 64;
const MAX_REPLAYS: usize = 256;

#[derive(Debug, Clone)]
struct Stage {
    target: mackes_mesh_types::worker_runtime::WorkerChangeSetTarget,
    generation: u64,
    digest: String,
    items: Vec<mackes_mesh_types::worker_runtime::WorkerChangeSetItem>,
    expires_at_ms: u64,
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
    use mackes_mesh_types::worker_runtime::{
        worker_change_set_digest, WorkerAction, WorkerActionDescriptor,
        WorkerArmingRequirement, WorkerChangeSetItem, WorkerChangeSetTarget, WorkerGroup,
    };

    fn contract() -> WorkerContract {
        let mut contract = WorkerContract::new("safe-worker", WorkerGroup::Actions, "safe")
            .expect("contract");
        contract.actions.push(WorkerActionDescriptor {
            action: WorkerAction::Refresh,
            label: "Refresh".into(),
            arming: WorkerArmingRequirement::Confirmation,
        });
        contract.admitted().expect("admitted contract")
    }

    fn request(id: &str, operation: WorkerChangeSetOperation, generation: u64) -> WorkerChangeSetRequest {
        let target = WorkerChangeSetTarget { node_id: "node-1".into(), worker_id: Some("safe-worker".into()) };
        let items = vec![WorkerChangeSetItem { item_id: "item-1".into(), worker_id: "safe-worker".into(), action: WorkerAction::Refresh }];
        let digest = worker_change_set_digest(&target, generation, &items, "refresh", "retry", WorkerArmingRequirement::Confirmation).expect("digest");
        let mut request = WorkerChangeSetRequest::new(id, operation, target, generation, items, "refresh", "retry", WorkerArmingRequirement::Confirmation, digest, 1000, 2000).expect("request");
        request.armed_token = Some(format!("token-{id}"));
        request
    }

    #[test]
    fn hostile_change_set_lifecycle_is_exact_generation_bound_and_fail_closed() {
        let mut executor = WorkerChangeSetExecutor::default();
        let contracts = vec![contract()];
        let preview = request("preview-1", WorkerChangeSetOperation::Preview, 7);
        assert_eq!(executor.consume(preview.clone(), 1100, 7, &contracts).outcome, WorkerChangeSetOutcome::Previewed);
        assert_eq!(executor.consume(preview, 1101, 7, &contracts).outcome, WorkerChangeSetOutcome::Refused);
        assert_eq!(executor.consume(request("stale", WorkerChangeSetOperation::Commit, 7), 1200, 8, &contracts).outcome, WorkerChangeSetOutcome::StaleGeneration);
        let substituted = request("substitute", WorkerChangeSetOperation::Commit, 8);
        assert_eq!(executor.consume(substituted, 1200, 8, &contracts).outcome, WorkerChangeSetOutcome::Refused);
        let commit = request("commit-1", WorkerChangeSetOperation::Commit, 7);
        assert_eq!(executor.consume(commit, 1200, 7, &contracts).outcome, WorkerChangeSetOutcome::Refused);
        let cancel = request("cancel-1", WorkerChangeSetOperation::Cancel, 7);
        assert_eq!(executor.consume(cancel, 1300, 7, &contracts).outcome, WorkerChangeSetOutcome::Cancelled);
        let second_cancel = request("cancel-2", WorkerChangeSetOperation::Cancel, 7);
        assert_eq!(executor.consume(second_cancel, 1301, 7, &contracts).outcome, WorkerChangeSetOutcome::Refused);
    }

    #[test]
    fn undeclared_action_never_stages() {
        let mut executor = WorkerChangeSetExecutor::default();
        let result = executor.consume(request("preview-2", WorkerChangeSetOperation::Preview, 3), 1100, 3, &[]);
        assert_eq!(result.outcome, WorkerChangeSetOutcome::Refused);
    }
}
