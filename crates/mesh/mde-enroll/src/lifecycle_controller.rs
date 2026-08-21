//! Renderer-neutral lifecycle routing for S4/S14-S16.
//!
//! This module deliberately creates plans, never performs mutations. The GUI
//! and TUI can therefore present the same offboard/reset/fleet handoff intent
//! and submit it through the authority-owned Bus path.

use mackes_mesh_types::lifecycle::{
    LifecycleIntentKind, LifecycleIntentV1, LifecyclePhase, LifecyclePlanV1, LifecycleProgressV1,
};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleController {
    pub session_id: String,
    pub generation: u64,
    pub targets: Vec<String>,
    last_progress: BTreeMap<String, LifecycleProgressV1>,
}

impl LifecycleController {
    pub fn new(session_id: impl Into<String>, generation: u64, mut targets: Vec<String>) -> Self {
        targets.sort();
        targets.dedup();
        Self {
            session_id: session_id.into(),
            generation,
            targets,
            last_progress: BTreeMap::new(),
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

    /// Accept one authority progress acknowledgement for a target.
    ///
    /// Progress is a checkpoint acknowledgement, not a renderer command. The
    /// controller only accepts monotonic, target- and generation-bound
    /// checkpoints, so a stale/replayed event cannot move a renderer backward
    /// or make an interrupted session appear complete.
    pub fn acknowledge_progress(
        &mut self,
        progress: LifecycleProgressV1,
    ) -> Result<(), ProgressError> {
        progress
            .validate()
            .map_err(|error| ProgressError::Invalid(format!("{error:?}")))?;
        if progress.request_id != self.session_id {
            return Err(ProgressError::WrongSession);
        }
        if progress.generation != self.generation {
            return Err(ProgressError::StaleGeneration);
        }
        if !self
            .targets
            .iter()
            .any(|target| target == &progress.target_id)
        {
            return Err(ProgressError::OutOfScope);
        }

        if let Some(previous) = self.last_progress.get(&progress.target_id) {
            if progress.completed_steps < previous.completed_steps {
                return Err(ProgressError::Regression);
            }
            if progress.completed_steps == previous.completed_steps
                && progress.phase == previous.phase
            {
                return Err(ProgressError::Replay);
            }
            if is_terminal(previous.phase) {
                return Err(ProgressError::TerminalCheckpoint);
            }
            if progress.total_steps != previous.total_steps {
                return Err(ProgressError::TotalChanged);
            }
        }
        self.last_progress
            .insert(progress.target_id.clone(), progress);
        Ok(())
    }

    /// Return the last durable checkpoint that can be used after interruption.
    #[must_use]
    pub fn resume_checkpoint(&self, target_id: &str) -> Option<&LifecycleProgressV1> {
        self.last_progress
            .get(target_id)
            .filter(|progress| !is_terminal(progress.phase))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProgressError {
    Invalid(String),
    WrongSession,
    StaleGeneration,
    OutOfScope,
    Regression,
    Replay,
    TerminalCheckpoint,
    TotalChanged,
}

fn is_terminal(phase: LifecyclePhase) -> bool {
    matches!(
        phase,
        LifecyclePhase::Succeeded | LifecyclePhase::Failed | LifecyclePhase::Cancelled
    )
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

    fn progress(
        phase: LifecyclePhase,
        completed_steps: u32,
        target_id: &str,
    ) -> LifecycleProgressV1 {
        LifecycleProgressV1 {
            schema_version: 1,
            request_id: "fleet-1".into(),
            target_id: target_id.into(),
            generation: 7,
            phase,
            completed_steps,
            total_steps: 2,
        }
    }

    #[test]
    fn accepts_typed_ack_and_returns_it_for_interruption_resume() {
        let mut controller = LifecycleController::new("fleet-1", 7, vec!["seat-15".into()]);
        controller
            .acknowledge_progress(progress(LifecyclePhase::Running, 1, "seat-15"))
            .unwrap();
        assert_eq!(
            controller
                .resume_checkpoint("seat-15")
                .map(|item| item.completed_steps),
            Some(1)
        );
    }

    #[test]
    fn rejects_replayed_stale_and_out_of_scope_acknowledgements() {
        let mut controller = LifecycleController::new("fleet-1", 7, vec!["seat-15".into()]);
        let running = progress(LifecyclePhase::Running, 1, "seat-15");
        controller.acknowledge_progress(running.clone()).unwrap();
        assert_eq!(
            controller.acknowledge_progress(running),
            Err(ProgressError::Replay)
        );
        assert_eq!(
            controller.acknowledge_progress(LifecycleProgressV1 {
                generation: 6,
                ..progress(LifecyclePhase::Running, 2, "seat-15")
            }),
            Err(ProgressError::StaleGeneration)
        );
        assert_eq!(
            controller.acknowledge_progress(progress(LifecyclePhase::Running, 2, "other")),
            Err(ProgressError::OutOfScope)
        );
    }

    #[test]
    fn rejects_regression_and_does_not_resume_after_terminal_ack() {
        let mut controller = LifecycleController::new("fleet-1", 7, vec!["seat-15".into()]);
        controller
            .acknowledge_progress(progress(LifecyclePhase::Running, 1, "seat-15"))
            .unwrap();
        assert_eq!(
            controller.acknowledge_progress(progress(LifecyclePhase::Running, 0, "seat-15")),
            Err(ProgressError::Regression)
        );
        controller
            .acknowledge_progress(progress(LifecyclePhase::Succeeded, 2, "seat-15"))
            .unwrap();
        assert!(controller.resume_checkpoint("seat-15").is_none());
        assert_eq!(
            controller.acknowledge_progress(progress(LifecyclePhase::Running, 2, "seat-15")),
            Err(ProgressError::TerminalCheckpoint)
        );
    }

    #[test]
    fn tracks_interrupted_fleet_targets_independently() {
        let mut controller =
            LifecycleController::new("fleet-1", 7, vec!["seat-15".into(), "seat-16".into()]);
        controller
            .acknowledge_progress(progress(LifecyclePhase::Running, 2, "seat-15"))
            .unwrap();
        controller
            .acknowledge_progress(progress(LifecyclePhase::Running, 0, "seat-16"))
            .unwrap();
        assert_eq!(
            controller
                .resume_checkpoint("seat-15")
                .map(|item| item.completed_steps),
            Some(2)
        );
        assert_eq!(
            controller
                .resume_checkpoint("seat-16")
                .map(|item| item.completed_steps),
            Some(0)
        );
    }
}
