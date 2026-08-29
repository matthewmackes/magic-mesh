//! WL-FUNC-023 S4 — Construct reads the same lifecycle session the TUI does.
//!
//! The shell never takes the authority lock and never mutates a checkpoint.
//! Extra checkpoint fields are ignored so the desktop tier stays off `mackesd`.

use std::path::{Path, PathBuf};

use mackes_mesh_types::lifecycle::{
    LifecyclePlanV1, LifecycleProgressV1, LifecycleRequirementCheckV1,
};
use mackes_mesh_types::lifecycle_view::LifecycleSessionView;
use mackes_mesh_types::peers::default_workgroup_root;
use mde_egui::egui::{self, RichText};
use mde_egui::Style;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct CheckpointProjection {
    plan: LifecyclePlanV1,
    progress: LifecycleProgressV1,
    #[serde(default)]
    checks: Vec<LifecycleRequirementCheckV1>,
}

/// Known local authority roots. Workgroup first (join/found), then the
/// mackesd state tree. Neither path is a dest and neither is treated as ready
/// just because the directory exists.
pub fn default_lifecycle_authority_roots() -> [PathBuf; 2] {
    [default_workgroup_root(), PathBuf::from("/var/lib/mackesd")]
}

pub fn load_lifecycle_session_from_root(root: &Path) -> Option<LifecycleSessionView> {
    let dir = root.join("lifecycle");
    let entries = std::fs::read_dir(&dir).ok()?;
    let mut best: Option<CheckpointProjection> = None;
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let path = entry.path().join("checkpoint.json");
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(checkpoint) = serde_json::from_slice::<CheckpointProjection>(&bytes) else {
            continue;
        };
        if checkpoint.plan.validate().is_err() || checkpoint.progress.validate().is_err() {
            continue;
        }
        let take = match &best {
            None => true,
            Some(prev) => {
                checkpoint.progress.generation > prev.progress.generation
                    || (checkpoint.progress.generation == prev.progress.generation
                        && checkpoint.plan.request_id > prev.plan.request_id)
            }
        };
        if take {
            best = Some(checkpoint);
        }
    }
    let checkpoint = best?;
    LifecycleSessionView::from_authority_parts(
        &checkpoint.plan,
        &checkpoint.progress,
        &checkpoint.checks,
    )
    .ok()
}

pub fn load_lifecycle_session() -> Option<LifecycleSessionView> {
    for root in default_lifecycle_authority_roots() {
        if let Some(view) = load_lifecycle_session_from_root(&root) {
            return Some(view);
        }
    }
    None
}

/// Read-only Construct card. Same status/capability lines as `magic-setup`.
pub fn show_lifecycle_session(ui: &mut egui::Ui, view: Option<&LifecycleSessionView>) {
    ui.label(RichText::new("ONBOARD & OFFBOARDING session").strong());
    match view {
        Some(view) => {
            mde_egui::field(ui, "Session", &view.status_line(), Style::TEXT);
            mde_egui::field(ui, "Capabilities", &view.capability_summary(), Style::TEXT);
            if !view.missing_requirements.is_empty() {
                ui.colored_label(
                    Style::WARN,
                    format!("missing: {}", view.missing_requirements.join(", ")),
                );
            }
        }
        None => {
            ui.colored_label(Style::TEXT_DIM, "no lifecycle session published");
        }
    }
    ui.label(
        RichText::new(
            "This projection is read-only. Mutation stays with the mackesd lifecycle authority.",
        )
        .color(Style::TEXT_DIM)
        .size(Style::SMALL),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_root_is_unpublished_not_ready() {
        let root = std::env::temp_dir().join(format!(
            "mcnf-shell-lifecycle-missing-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        assert!(load_lifecycle_session_from_root(&root).is_none());
    }

    #[test]
    fn hydrates_the_same_status_line_as_the_tui() {
        let root = std::env::temp_dir().join(format!(
            "mcnf-shell-lifecycle-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let dir = root.join("lifecycle").join("seat-15");
        std::fs::create_dir_all(&dir).expect("temp checkpoint dir");
        let checkpoint = serde_json::json!({
            "plan": {
                "schema_version": 1,
                "request_id": "request-1",
                "target_id": "seat-15",
                "intent": "onboard",
                "generation": 1,
                "steps": ["identity"]
            },
            "progress": {
                "schema_version": 1,
                "request_id": "request-1",
                "target_id": "seat-15",
                "generation": 1,
                "phase": "waiting_for_operator",
                "completed_steps": 0,
                "total_steps": 1
            },
            "checks": [{
                "schema_version": 1,
                "check_id": "identity",
                "target_id": "seat-15",
                "expected": "present",
                "observed": "missing",
                "status": "fail",
                "required": true,
                "evidence_digest_hex": "2".repeat(64),
                "generation": 1
            }]
        });
        std::fs::write(
            dir.join("checkpoint.json"),
            serde_json::to_vec(&checkpoint).unwrap(),
        )
        .unwrap();
        let view = load_lifecycle_session_from_root(&root).expect("published session");
        assert_eq!(view.status_line(), "request-1: onboard (blocked)");
        assert_eq!(view.missing_requirements, vec!["identity"]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
