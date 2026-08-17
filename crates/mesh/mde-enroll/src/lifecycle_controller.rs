//! Renderer-neutral lifecycle routing for S4/S14-S16.
//!
//! This module deliberately creates plans, never performs mutations. The GUI
//! and TUI can therefore present the same offboard/reset/fleet handoff intent
//! and submit it through the authority-owned Bus path.

use mackes_mesh_types::lifecycle::{LifecycleIntentKind, LifecycleIntentV1, LifecyclePlanV1};

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
        Self {
            session_id: session_id.into(),
            generation,
            targets,
        }
    }

    pub fn plan(&self, intent: LifecycleIntentKind, target_id: &str) -> Option<LifecyclePlanV1> {
        if !self.targets.iter().any(|target| target == target_id) {
            return None;
        }
        // Renderers select one closed intent; the public contract owns the
        // corresponding ordered step vocabulary.  Keeping a separate GUI/TUI
        // list here previously produced terms such as `cordon` and `erase`
        // that `LifecyclePlanV1` quite correctly refused.
        let intent_request = LifecycleIntentV1 {
            schema_version: 1,
            request_id: self.session_id.clone(),
            target_id: target_id.to_owned(),
            intent,
            generation: self.generation,
        };
        Some(LifecyclePlanV1 {
            schema_version: 1,
            request_id: self.session_id.clone(),
            target_id: target_id.to_owned(),
            intent,
            generation: self.generation,
            steps: intent_request.default_steps(),
        })
    }

    pub fn fleet_targets(&self) -> &[String] {
        &self.targets
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_are_identical_for_gui_and_tui_consumers() {
        let controller =
            LifecycleController::new("fleet-1", 7, vec!["b".into(), "a".into(), "a".into()]);
        let gui = controller.plan(LifecycleIntentKind::Offboard, "a").unwrap();
        let tui = controller.plan(LifecycleIntentKind::Offboard, "a").unwrap();
        assert_eq!(gui, tui);
        assert_eq!(gui.steps, vec!["offboard", "verify"]);
        assert!(gui.validate().is_ok());
        let reset = controller
            .plan(LifecycleIntentKind::ResetAndOnboard, "a")
            .unwrap();
        assert_eq!(reset.steps.first(), Some(&"offboard".to_owned()));
        assert!(reset.validate().is_ok());
        assert!(controller
            .plan(LifecycleIntentKind::ResetAndOnboard, "missing")
            .is_none());
    }
}
