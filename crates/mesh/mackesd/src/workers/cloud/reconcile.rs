//! Workloads U2 — the placement / tfvars-render reconcile SKELETON.
//!
//! This is the seam U4 (desired-state + set-desired) and U5 (plan + tfvars render)
//! fill in. U2 lands ONLY the trait + entry points + an honest `not-yet-wired`
//! implementation ([`NotYetPlanner`]); it computes no plan and renders no tfvars.
//! Every entry point returns a truthful [`ReconcileNotYet`] rather than a fabricated
//! empty plan / an all-zero [`PlanCounts`] that would read as "in sync" (§7 — a
//! skeleton never fakes success).
//!
//! The shape is deliberately disjoint from the verb handlers so U4/U5 can own it
//! without touching the drain, the gate, or the runner.

use mackes_mesh_types::cloud::{PlanCounts, WorkloadSpec};

/// The honest "this reconcile leg is not yet wired" outcome — carries which unit
/// owns it, so a caller surfaces a truthful "coming in U#" rather than a fake result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReconcileNotYet(pub String);

impl ReconcileNotYet {
    /// Build a not-yet outcome attributing the leg to the unit that will land it.
    #[must_use]
    pub fn owned_by(leg: &str, unit: &str) -> Self {
        Self(format!("{leg} not yet wired ({unit})"))
    }
}

impl std::fmt::Display for ReconcileNotYet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The placement / tfvars-render reconcile seam.
///
/// U4 wires [`PlacementPlanner::render_tfvars`] (write a node's desired-state slice
/// to `terraform.tfvars.json`); U5 wires [`PlacementPlanner::plan`] (the pending
/// change counts a `tofu plan` of that slice yields). U2's implementation returns
/// `not-yet-wired` from both.
pub(crate) trait PlacementPlanner: Send + Sync {
    /// Render node `node`'s desired-state `specs` into a `terraform.tfvars.json`
    /// document (returned as a string for the caller to persist under the node's
    /// tfvars root). U4/U5.
    ///
    /// # Errors
    /// [`ReconcileNotYet`] until the render leg is wired.
    fn render_tfvars(&self, node: &str, specs: &[WorkloadSpec]) -> Result<String, ReconcileNotYet>;

    /// The pending change a `tofu plan` of node `node`'s rendered slice would apply
    /// — the lean counts the surface previews before an armed apply. U5.
    ///
    /// # Errors
    /// [`ReconcileNotYet`] until the plan leg is wired.
    fn plan(&self, node: &str) -> Result<PlanCounts, ReconcileNotYet>;
}

/// The U2 skeleton planner — honest `not-yet-wired` on every leg.
pub(crate) struct NotYetPlanner;

impl PlacementPlanner for NotYetPlanner {
    fn render_tfvars(
        &self,
        _node: &str,
        _specs: &[WorkloadSpec],
    ) -> Result<String, ReconcileNotYet> {
        Err(ReconcileNotYet::owned_by("tfvars render", "U4/U5"))
    }

    fn plan(&self, _node: &str) -> Result<PlanCounts, ReconcileNotYet> {
        Err(ReconcileNotYet::owned_by("placement plan", "U5"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_skeleton_planner_is_honestly_not_yet_wired_never_a_fake_in_sync_plan() {
        let p = NotYetPlanner;
        // A fabricated all-zero PlanCounts would read as "in sync" — the skeleton
        // must instead surface a truthful not-yet.
        let plan = p.plan("eagle");
        assert!(plan.is_err());
        assert!(plan.unwrap_err().to_string().contains("not yet wired"));

        let render = p.render_tfvars("eagle", &[]);
        assert!(render.is_err());
        assert!(render.unwrap_err().0.contains("U4"));
    }
}
