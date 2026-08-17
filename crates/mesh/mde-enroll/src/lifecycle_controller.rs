//! Renderer-neutral lifecycle routing for S4/S14-S16.
//!
//! This module deliberately creates plans, never performs mutations. The GUI
//! and TUI can therefore present the same offboard/reset/fleet handoff intent
//! and submit it through the authority-owned Bus path.

use mackes_mesh_types::lifecycle::{LifecycleIntentKind, LifecyclePlanV1};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleController {
    pub session_id: String,
    pub generation: u64,
    pub targets: Vec<String>,
}

impl LifecycleController {
    pub fn new(session_id: impl Into<String>, generation: u64, mut targets: Vec<String>) -> Self {
        targets.sort();
        targets.dedup();
        Self { session_id: session_id.into(), generation, targets }
    }

    pub fn plan(&self, intent: LifecycleIntentKind, target_id: &str) -> Option<LifecyclePlanV1> {
        if !self.targets.iter().any(|target| target == target_id) {
            return None;
        }
        let steps = match intent {
            LifecycleIntentKind::Offboard => vec!["cordon", "drain", "revoke", "erase", "verify"],
            LifecycleIntentKind::ResetAndOnboard => vec!["offboard", "erase", "install", "identity", "enroll", "verify"],
            LifecycleIntentKind::Onboard => vec!["identity", "install", "configure", "enroll", "verify"],
            LifecycleIntentKind::Upgrade => vec!["preflight", "stage", "migrate", "activate", "verify"],
            LifecycleIntentKind::VerifyAndCorrect => vec!["audit", "correct", "re-audit"],
        }.into_iter().map(str::to_owned).collect();
        Some(LifecyclePlanV1 {
            schema_version: 1,
            request_id: self.session_id.clone(),
            target_id: target_id.to_owned(),
            intent,
            generation: self.generation,
            steps,
        })
    }

    pub fn fleet_targets(&self) -> &[String] { &self.targets }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_are_identical_for_gui_and_tui_consumers() {
        let controller = LifecycleController::new("fleet-1", 7, vec!["b".into(), "a".into(), "a".into()]);
        let gui = controller.plan(LifecycleIntentKind::Offboard, "a").unwrap();
        let tui = controller.plan(LifecycleIntentKind::Offboard, "a").unwrap();
        assert_eq!(gui, tui);
        assert_eq!(gui.steps, vec!["cordon", "drain", "revoke", "erase", "verify"]);
        assert!(controller.plan(LifecycleIntentKind::ResetAndOnboard, "missing").is_none());
    }
}
