//! WL-FUNC-023 S4 — one bounded lifecycle view model for terminal and GUI clients.
//!
//! The view contains no mutation logic. Both renderers consume this projection
//! so a session cannot acquire a client-specific lifecycle interpretation.

use crate::lifecycle::{
    FleetLifecycleReportV1, LifecycleArtifactSelectionV1, LifecycleConfirmationAction,
    LifecycleConfirmationV1, LifecycleCorrectionPlanV1, LifecycleIntentKind, LifecyclePhase,
    LifecyclePlanV1, LifecycleProgressV1, LifecycleRequirementCheckV1, OffboardingReceiptV1,
    OnboardOffboardSessionV1, SeatReadinessV1,
};

/// Warnings that begin with this prefix name a withdrawn capability (S13).
/// The projection derives the capability name from the *typed* warning so a
/// renderer never re-parses the raw readiness envelope.
const CAPABILITY_UNAVAILABLE_PREFIX: &str = "capability unavailable: ";
/// WL-REL-007 S4: Android/Cuttlefish is deferred for 13.0.0 and must stay
/// visible without becoming a readiness gate or unavailable capability.
const ANDROID_DEFERRED_SUMMARY: &str = "android: Deferred";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleSessionView {
    pub session_id: String,
    pub intent: LifecycleIntentKind,
    pub phase: LifecyclePhase,
    pub targets: Vec<String>,
    pub ready: bool,
    pub missing_requirements: Vec<String>,
    pub warnings: Vec<String>,
    pub readiness: ReadinessState,
    pub capabilities: Vec<String>,
    /// Byte digest for an unsigned artifact. Empty unless the authority
    /// already admitted unverified bytes; the seat-list digest is not this.
    pub unsigned_artifact_digest_hex: Option<String>,
    /// Durable fleet coordinator from the authority checkpoint. Empty until
    /// the first handoff claim; renderers display it and do not invent one.
    pub coordinator_id: Option<String>,
    /// First still-blocking correction from the persisted VAC DAG.
    /// Empty until the authority admits a plan; renderers do not invent one.
    pub next_correction: Option<String>,
    /// Last persisted step error from the authority. Empty until a
    /// correction attempt fails; renderers do not invent one.
    pub last_error: Option<String>,
    /// Durable offboard receipt already written next to the checkpoint.
    /// Empty until persist; renderers do not invent dest wipe.
    pub offboard_receipt_completed: bool,
    /// Staged package identity (`staged:{digest}:{shape}`). Empty until
    /// first-boot observed a pin; renderers do not treat this as installed.
    pub staged_package: Option<String>,
    /// Pending commissioning capsule id. Empty until admit wrote bytes;
    /// renderers do not treat this as confirmed enrollment.
    pub staged_capsule: Option<String>,
    /// Typed nag when join dests are missing. Dest write is not implied.
    pub onboard_nag: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadinessState {
    Blocked,
    ReadyWithWarnings,
    Ready,
}

impl ReadinessState {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Blocked => "blocked",
            Self::ReadyWithWarnings => "ready with warnings",
            Self::Ready => "ready",
        }
    }
}

impl LifecycleSessionView {
    /// Derive withdrawn-capability names from the typed readiness warnings.
    ///
    /// Only a warning whose text *starts with* the capability prefix names a
    /// capability; a warning that merely mentions the phrase mid-sentence is
    /// left as an ordinary warning. The trailing name is trimmed and empties
    /// are dropped so a renderer receives a clean capability list.
    fn is_deferred_13_0_0_guest(name: &str) -> bool {
        let lowered = name.to_ascii_lowercase();
        lowered.contains("android") || lowered.contains("cuttlefish")
    }

    fn is_deferred_guest_warning(warning: &str) -> bool {
        warning
            .strip_prefix(CAPABILITY_UNAVAILABLE_PREFIX)
            .map(str::trim)
            .is_some_and(Self::is_deferred_13_0_0_guest)
    }

    fn capabilities_from_warnings(warnings: &[String]) -> Vec<String> {
        warnings
            .iter()
            .filter_map(|warning| warning.strip_prefix(CAPABILITY_UNAVAILABLE_PREFIX))
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .filter(|name| !Self::is_deferred_13_0_0_guest(name))
            .map(str::to_owned)
            .collect()
    }

    pub fn from_wire(session_json: &str, readiness_json: &str) -> Result<Self, String> {
        let session: OnboardOffboardSessionV1 = serde_json::from_str(session_json)
            .map_err(|_| "invalid lifecycle session".to_owned())?;
        session
            .validate()
            .map_err(|error| format!("invalid lifecycle session: {error:?}"))?;
        let readiness_wire: SeatReadinessV1 = serde_json::from_str(readiness_json)
            .map_err(|_| "invalid lifecycle readiness".to_owned())?;
        readiness_wire
            .validate()
            .map_err(|error| format!("invalid lifecycle readiness: {error:?}"))?;
        if !session
            .target_ids
            .iter()
            .any(|target| target == &readiness_wire.target_id)
        {
            return Err("readiness target is outside session scope".to_owned());
        }
        if readiness_wire.generation != session.generation {
            return Err("readiness generation does not match session".to_owned());
        }
        let production_warnings = readiness_wire
            .warnings
            .iter()
            .any(|warning| !Self::is_deferred_guest_warning(warning));
        let readiness = if !readiness_wire.missing_requirements.is_empty() {
            ReadinessState::Blocked
        } else if production_warnings {
            ReadinessState::ReadyWithWarnings
        } else {
            ReadinessState::Ready
        };
        let capabilities = Self::capabilities_from_warnings(&readiness_wire.warnings);
        Ok(Self {
            session_id: session.session_id,
            intent: session.intent,
            phase: session.phase,
            targets: session.target_ids,
            ready: readiness_wire.ready,
            missing_requirements: readiness_wire.missing_requirements,
            warnings: readiness_wire.warnings,
            readiness,
            capabilities,
            unsigned_artifact_digest_hex: None,
            coordinator_id: None,
            next_correction: None,
            last_error: None,
            offboard_receipt_completed: false,
            staged_package: None,
            staged_capsule: None,
            onboard_nag: None,
        })
    }

    /// Same phrase doctor and first-boot print when join dests are missing.
    #[must_use]
    pub fn onboard_nag_from_checks(checks: &[LifecycleRequirementCheckV1]) -> Option<String> {
        checks.iter().find_map(|check| {
            if !matches!(check.check_id.as_str(), "mesh" | "mesh_identity") {
                return None;
            }
            check
                .observed
                .strip_prefix("missing:")
                .map(str::trim)
                .filter(|dests| !dests.is_empty())
                .map(|dests| format!("open ONBOARD: missing {dests}"))
        })
    }

    /// Bind the ONBOARD nag from observed join dests. Dest write is not implied.
    #[must_use]
    pub fn with_onboard_nag(mut self, checks: &[LifecycleRequirementCheckV1]) -> Self {
        if self.onboard_nag.is_none() {
            self.onboard_nag = Self::onboard_nag_from_checks(checks);
        }
        self
    }

    /// Visible ONBOARD nag. Absent when join dests are present or unobserved.
    #[must_use]
    pub fn onboard_nag_line(&self) -> Option<&str> {
        self.onboard_nag.as_deref()
    }

    /// Bind an admitted unsigned artifact so Upgrade shows the same phrase
    /// the authority requires. Signed selections add no confirmation lines.
    #[must_use]
    pub fn with_artifact_selection(
        mut self,
        selection: Option<&LifecycleArtifactSelectionV1>,
    ) -> Self {
        self.unsigned_artifact_digest_hex = selection
            .filter(|selection| selection.unverified_build)
            .map(|selection| selection.artifact_digest_hex.clone());
        self
    }

    /// Bind the coordinator already recorded on durable checkpoints.
    #[must_use]
    pub fn with_coordinator(mut self, coordinator_id: Option<&str>) -> Self {
        self.coordinator_id = coordinator_id
            .filter(|id| !id.is_empty())
            .map(str::to_owned);
        self
    }

    /// Bind the durable fleet seat list. Confirmation phrases use this
    /// count; a single peeked seat cannot shrink a fleet job.
    #[must_use]
    pub fn with_fleet_targets(
        mut self,
        target_ids: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        let mut targets: Vec<String> = target_ids
            .into_iter()
            .map(Into::into)
            .filter(|target| !target.is_empty())
            .collect();
        targets.sort();
        targets.dedup();
        if !targets.is_empty() {
            self.targets = targets;
        }
        self
    }

    /// Bind the signed/printed fleet report. Empty coordinator stays unset.
    #[must_use]
    pub fn with_fleet_report(self, report: &FleetLifecycleReportV1) -> Self {
        self.with_coordinator(
            (!report.coordinator_id.is_empty()).then_some(report.coordinator_id.as_str()),
        )
    }

    /// Bind the next still-blocking correction from a persisted VAC DAG.
    /// Passed or absent checks are skipped; a renderer cannot reorder repairs.
    #[must_use]
    pub fn with_correction_plan(
        mut self,
        plan: Option<&LifecycleCorrectionPlanV1>,
        checks: &[LifecycleRequirementCheckV1],
    ) -> Self {
        self.next_correction = plan.and_then(|plan| {
            plan.corrections
                .iter()
                .find(|correction| {
                    checks.iter().any(|check| {
                        check.check_id == correction.check_id && check.blocks_progress()
                    })
                })
                .map(|correction| {
                    format!(
                        "correct {}: {} ({})",
                        correction.step, correction.check_id, correction.reason
                    )
                })
        });
        self
    }

    /// Exact next VAC action. Absent until a blocking correction remains.
    #[must_use]
    pub fn correction_line(&self) -> Option<&str> {
        self.next_correction.as_deref()
    }

    /// Bind the last persisted step error. Empty strings are dropped.
    #[must_use]
    pub fn with_last_error(mut self, last_error: Option<&str>) -> Self {
        self.last_error = last_error
            .filter(|error| !error.is_empty())
            .map(str::to_owned);
        self
    }

    /// Same last-error line doctor prints when no blocking correction remains.
    #[must_use]
    pub fn last_error_line(&self) -> Option<String> {
        self.last_error
            .as_ref()
            .map(|error| format!("last error: {error}"))
    }

    /// Bind a durable offboard receipt. Foreign request, other target, or
    /// failed validate is dropped so a renderer cannot invent dest wipe.
    #[must_use]
    pub fn with_offboarding_receipt(mut self, receipt: Option<&OffboardingReceiptV1>) -> Self {
        self.offboard_receipt_completed = receipt.is_some_and(|receipt| {
            receipt.validate().is_ok()
                && receipt.request_id == self.session_id
                && self
                    .targets
                    .iter()
                    .any(|target| target == &receipt.target_id)
        });
        self
    }

    /// Visible receipt line. Absent until persist wrote a valid receipt.
    #[must_use]
    pub fn receipt_line(&self) -> Option<&'static str> {
        self.offboard_receipt_completed
            .then_some("offboard receipt completed")
    }

    /// Bind a first-boot staged identity. Dest NEVRA/path is not this line.
    #[must_use]
    pub fn with_staged_package(mut self, identity: Option<&str>) -> Self {
        self.staged_package = identity
            .filter(|identity| identity.starts_with("staged:"))
            .map(str::to_owned);
        self
    }

    /// Visible staged-package line. Absent until a pin was observed.
    #[must_use]
    pub fn package_line(&self) -> Option<String> {
        self.staged_package
            .as_ref()
            .map(|identity| format!("packages {identity} (not installed)"))
    }

    /// Bind a pending capsule id. Confirm/revoke is not this line.
    #[must_use]
    pub fn with_staged_capsule(mut self, capsule_id: Option<&str>) -> Self {
        self.staged_capsule = capsule_id
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_owned);
        self
    }

    /// Visible staged-capsule line. Absent until admit persisted bytes.
    #[must_use]
    pub fn capsule_line(&self) -> Option<String> {
        self.staged_capsule
            .as_ref()
            .map(|id| format!("capsule {id} staged (not confirmed)"))
    }

    /// Visible coordinator line. Absent until an authority handoff persists one.
    #[must_use]
    pub fn coordinator_line(&self) -> Option<String> {
        self.coordinator_id
            .as_ref()
            .map(|id| format!("coordinator {id}"))
    }

    /// Visible fleet seat list. Absent for a single local seat.
    #[must_use]
    pub fn fleet_line(&self) -> Option<String> {
        if self.targets.len() <= 1 {
            return None;
        }
        Some(format!("fleet {}", self.targets.join(", ")))
    }

    /// Project an authority checkpoint through the same session/readiness
    /// wire the GUI and TUI already share. `operator_id` names the local
    /// authority, not a dest or a signed operator.
    pub fn from_authority_parts(
        plan: &LifecyclePlanV1,
        progress: &LifecycleProgressV1,
        checks: &[LifecycleRequirementCheckV1],
    ) -> Result<Self, String> {
        let readiness = SeatReadinessV1::from_requirement_checks(
            plan.schema_version,
            plan.target_id.clone(),
            plan.generation,
            checks,
        )
        .map_err(|error| format!("invalid lifecycle readiness: {error:?}"))?;
        let session = OnboardOffboardSessionV1 {
            schema_version: plan.schema_version,
            session_id: plan.request_id.clone(),
            operator_id: "local-authority".to_owned(),
            intent: plan.intent,
            target_ids: vec![plan.target_id.clone()],
            generation: plan.generation,
            phase: progress.phase,
        };
        session
            .validate()
            .map_err(|error| format!("invalid lifecycle session: {error:?}"))?;
        let session_json =
            serde_json::to_string(&session).map_err(|_| "invalid lifecycle session".to_owned())?;
        let readiness_json = serde_json::to_string(&readiness)
            .map_err(|_| "invalid lifecycle readiness".to_owned())?;
        Ok(Self::from_wire(&session_json, &readiness_json)?.with_onboard_nag(checks))
    }

    pub fn status_line(&self) -> String {
        let state = match self.phase {
            LifecyclePhase::Failed => "failed",
            LifecyclePhase::Succeeded => self.readiness.label(),
            _ if self.readiness == ReadinessState::Blocked => "blocked",
            _ => "in progress",
        };
        format!("{}: {} ({})", self.session_id, self.intent_label(), state)
    }

    pub fn capability_summary(&self) -> String {
        let baseline = if self.capabilities.is_empty() {
            "capabilities: baseline available".to_owned()
        } else {
            format!("capabilities unavailable: {}", self.capabilities.join(", "))
        };
        format!("{baseline}; {ANDROID_DEFERRED_SUMMARY}")
    }

    /// Typed fleet phrase the authority will require. Renderers display it;
    /// they do not sign or invent dest wipe.
    #[must_use]
    pub fn confirmation_lines(&self) -> Vec<String> {
        let count = self.targets.len() as u32;
        if count == 0 {
            return Vec::new();
        }
        match self.intent {
            LifecycleIntentKind::Offboard => vec![
                LifecycleConfirmationV1::expected_phrase(
                    LifecycleConfirmationAction::Offboard,
                    count,
                ),
                format!(
                    "scope {}",
                    LifecycleConfirmationV1::fleet_scope_digest(&self.targets)
                ),
            ],
            LifecycleIntentKind::ResetAndOnboard => vec![
                LifecycleConfirmationV1::expected_phrase(LifecycleConfirmationAction::Reset, count),
                format!(
                    "scope {}",
                    LifecycleConfirmationV1::fleet_scope_digest(&self.targets)
                ),
            ],
            LifecycleIntentKind::Upgrade => {
                let Some(digest) = self.unsigned_artifact_digest_hex.as_deref() else {
                    return Vec::new();
                };
                if digest.len() != 64 || !digest.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Vec::new();
                }
                vec![
                    LifecycleConfirmationV1::expected_phrase(
                        LifecycleConfirmationAction::InstallUnsigned,
                        count,
                    ),
                    format!("scope {digest}"),
                ]
            }
            _ => Vec::new(),
        }
    }

    fn intent_label(&self) -> &'static str {
        match self.intent {
            LifecycleIntentKind::Onboard => "onboard",
            LifecycleIntentKind::Upgrade => "upgrade",
            LifecycleIntentKind::VerifyAndCorrect => "verify-and-correct",
            LifecycleIntentKind::Offboard => "offboard",
            LifecycleIntentKind::ResetAndOnboard => "reset-and-onboard",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_a_scoped_session_for_any_renderer() {
        let session = serde_json::json!({
            "schema_version": 1, "session_id": "session-1", "operator_id": "operator-1",
            "intent": "onboard", "target_ids": ["seat-15"], "generation": 1, "phase": "succeeded"
        });
        let readiness = serde_json::json!({
            "schema_version": 1, "target_id": "seat-15", "generation": 1,
            "ready": true, "missing_requirements": [], "warnings": []
        });
        let view =
            LifecycleSessionView::from_wire(&session.to_string(), &readiness.to_string()).unwrap();
        assert_eq!(view.status_line(), "session-1: onboard (ready)");
        assert_eq!(view.readiness, ReadinessState::Ready);
        assert!(view.confirmation_lines().is_empty());
    }

    #[test]
    fn offboard_view_names_the_same_fleet_phrase_for_any_renderer() {
        let session = serde_json::json!({
            "schema_version": 1, "session_id": "session-1", "operator_id": "operator-1",
            "intent": "offboard", "target_ids": ["seat-16", "seat-15"], "generation": 1, "phase": "planned"
        });
        let readiness = serde_json::json!({
            "schema_version": 1, "target_id": "seat-15", "generation": 1,
            "ready": false, "missing_requirements": ["identity"], "warnings": []
        });
        let view =
            LifecycleSessionView::from_wire(&session.to_string(), &readiness.to_string()).unwrap();
        assert_eq!(view.confirmation_lines()[0], "FORCE OFFBOARD 2 SYSTEMS");
        assert_eq!(
            view.confirmation_lines()[1],
            format!(
                "scope {}",
                LifecycleConfirmationV1::fleet_scope_digest(&["seat-15", "seat-16"])
            )
        );
    }

    #[test]
    fn upgrade_view_names_the_unsigned_phrase_bound_to_the_artifact_bytes() {
        let session = serde_json::json!({
            "schema_version": 1, "session_id": "session-1", "operator_id": "operator-1",
            "intent": "upgrade", "target_ids": ["seat-16", "seat-15"], "generation": 1, "phase": "planned"
        });
        let readiness = serde_json::json!({
            "schema_version": 1, "target_id": "seat-15", "generation": 1,
            "ready": true, "missing_requirements": [], "warnings": []
        });
        let digest = "e".repeat(64);
        let selection = LifecycleArtifactSelectionV1 {
            schema_version: 1,
            selection_id: "sel-1".into(),
            target_id: "seat-15".into(),
            channel: crate::lifecycle::LifecycleArtifactChannel::Dev,
            artifact_digest_hex: digest.clone(),
            source_revision: "rev-1".into(),
            signed: false,
            unverified_build: true,
            generation: 1,
        };
        let view = LifecycleSessionView::from_wire(&session.to_string(), &readiness.to_string())
            .unwrap()
            .with_artifact_selection(Some(&selection));
        assert_eq!(view.confirmation_lines()[0], "INSTALL UNSIGNED 2 SYSTEMS");
        assert_eq!(view.confirmation_lines()[1], format!("scope {digest}"));
        assert_ne!(
            view.confirmation_lines()[1],
            format!(
                "scope {}",
                LifecycleConfirmationV1::fleet_scope_digest(&["seat-15", "seat-16"])
            )
        );
        let signed = LifecycleArtifactSelectionV1 {
            unverified_build: false,
            signed: true,
            ..selection
        };
        let signed_view =
            LifecycleSessionView::from_wire(&session.to_string(), &readiness.to_string())
                .unwrap()
                .with_artifact_selection(Some(&signed));
        assert!(signed_view.confirmation_lines().is_empty());
    }

    #[test]
    fn projects_ready_with_warnings_without_blocking_core_readiness() {
        let session = serde_json::json!({
            "schema_version": 1, "session_id": "session-1", "operator_id": "operator-1",
            "intent": "verify_and_correct", "target_ids": ["seat-15"], "generation": 1, "phase": "succeeded"
        });
        let readiness = serde_json::json!({
            "schema_version": 1, "target_id": "seat-15", "generation": 1,
            "ready": true, "missing_requirements": [], "warnings": ["capability unavailable: kvm"]
        });
        let view =
            LifecycleSessionView::from_wire(&session.to_string(), &readiness.to_string()).unwrap();
        assert_eq!(view.readiness, ReadinessState::ReadyWithWarnings);
        assert!(view.capability_summary().contains("kvm"));
    }

    #[test]
    fn capabilities_are_projected_from_typed_warnings_not_raw_json() {
        let session = serde_json::json!({
            "schema_version": 1, "session_id": "session-1", "operator_id": "operator-1",
            "intent": "verify_and_correct", "target_ids": ["seat-15"], "generation": 1, "phase": "running"
        });
        let readiness = serde_json::json!({
            "schema_version": 1, "target_id": "seat-15", "generation": 1,
            "ready": true, "missing_requirements": [],
            "warnings": ["capability unavailable: kvm", "capability unavailable: gpu passthrough"]
        });
        let session_json = session.to_string();
        let compact = serde_json::to_string(&readiness).unwrap();
        let pretty = serde_json::to_string_pretty(&readiness).unwrap();
        assert_ne!(
            compact, pretty,
            "fixture must actually differ in wire layout"
        );
        let compact_view = LifecycleSessionView::from_wire(&session_json, &compact).unwrap();
        let pretty_view = LifecycleSessionView::from_wire(&session_json, &pretty).unwrap();
        assert_eq!(compact_view.capabilities, pretty_view.capabilities);
        assert_eq!(pretty_view.capabilities, vec!["kvm", "gpu passthrough"]);
        assert_eq!(
            pretty_view.capability_summary(),
            "capabilities unavailable: kvm, gpu passthrough; android: Deferred"
        );
    }

    #[test]
    fn does_not_harvest_a_capability_from_a_mid_sentence_mention() {
        let session = serde_json::json!({
            "schema_version": 1, "session_id": "session-1", "operator_id": "operator-1",
            "intent": "onboard", "target_ids": ["seat-15"], "generation": 1, "phase": "running"
        });
        let readiness = serde_json::json!({
            "schema_version": 1, "target_id": "seat-15", "generation": 1,
            "ready": true, "missing_requirements": [],
            "warnings": ["note: this is not a capability unavailable: report"]
        });
        let view =
            LifecycleSessionView::from_wire(&session.to_string(), &readiness.to_string()).unwrap();
        assert!(view.capabilities.is_empty());
        assert_eq!(
            view.capability_summary(),
            "capabilities: baseline available; android: Deferred"
        );
        assert_eq!(view.readiness, ReadinessState::ReadyWithWarnings);
    }

    #[test]
    fn drops_empty_capability_names_after_the_typed_prefix() {
        let session = serde_json::json!({
            "schema_version": 1, "session_id": "session-1", "operator_id": "operator-1",
            "intent": "onboard", "target_ids": ["seat-15"], "generation": 1, "phase": "running"
        });
        let readiness = serde_json::json!({
            "schema_version": 1, "target_id": "seat-15", "generation": 1,
            "ready": true, "missing_requirements": [],
            "warnings": [
                "capability unavailable: ",
                "capability unavailable:    ",
                "capability unavailable: kvm"
            ]
        });
        let view =
            LifecycleSessionView::from_wire(&session.to_string(), &readiness.to_string()).unwrap();
        assert_eq!(view.capabilities, vec!["kvm"]);
        assert_eq!(
            view.capability_summary(),
            "capabilities unavailable: kvm; android: Deferred"
        );
    }

    #[test]
    fn android_and_cuttlefish_are_deferred_and_non_gating() {
        let session = serde_json::json!({
            "schema_version": 1, "session_id": "session-1", "operator_id": "operator-1",
            "intent": "verify_and_correct", "target_ids": ["seat-15"], "generation": 1, "phase": "succeeded"
        });
        let readiness = serde_json::json!({
            "schema_version": 1, "target_id": "seat-15", "generation": 1,
            "ready": true, "missing_requirements": [],
            "warnings": [
                "capability unavailable: android",
                "capability unavailable: cuttlefish"
            ]
        });
        let view =
            LifecycleSessionView::from_wire(&session.to_string(), &readiness.to_string()).unwrap();
        assert!(view.capabilities.is_empty());
        assert_eq!(view.readiness, ReadinessState::Ready);
        assert_eq!(
            view.capability_summary(),
            "capabilities: baseline available; android: Deferred"
        );
        assert!(!view.capability_summary().contains("unavailable: android"));
    }

    #[test]
    fn rejects_readiness_outside_session_scope() {
        let session = serde_json::json!({
            "schema_version": 1, "session_id": "session-1", "operator_id": "operator-1",
            "intent": "offboard", "target_ids": ["seat-15"], "generation": 1, "phase": "running"
        });
        let readiness = serde_json::json!({
            "schema_version": 1, "target_id": "seat-16", "generation": 1,
            "ready": false, "missing_requirements": ["mesh_identity"], "warnings": []
        });
        assert!(
            LifecycleSessionView::from_wire(&session.to_string(), &readiness.to_string()).is_err()
        );
    }

    #[test]
    fn rejects_stale_readiness_generation() {
        let session = serde_json::json!({
            "schema_version": 1, "session_id": "session-1", "operator_id": "operator-1",
            "intent": "upgrade", "target_ids": ["seat-15"], "generation": 2, "phase": "running"
        });
        let readiness = serde_json::json!({
            "schema_version": 1, "target_id": "seat-15", "generation": 1,
            "ready": true, "missing_requirements": [], "warnings": []
        });
        let error = LifecycleSessionView::from_wire(&session.to_string(), &readiness.to_string())
            .expect_err("stale readiness must not project into a newer session");
        assert_eq!(error, "readiness generation does not match session");
    }

    #[test]
    fn fleet_targets_expand_a_single_seat_projection() {
        let plan = LifecyclePlanV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::Offboard,
            generation: 1,
            steps: vec!["offboard".into(), "verify".into()],
        };
        let progress = LifecycleProgressV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            target_id: "seat-15".into(),
            generation: 1,
            phase: LifecyclePhase::Planned,
            completed_steps: 0,
            total_steps: 2,
        };
        let report = FleetLifecycleReportV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            generation: 1,
            phase: LifecyclePhase::Running,
            target_count: 2,
            succeeded: 0,
            failed: 0,
            coordinator_id: "coord-b".into(),
            signature_hex: String::new(),
        };
        let view = LifecycleSessionView::from_authority_parts(&plan, &progress, &[])
            .unwrap()
            .with_fleet_targets(["seat-16", "seat-15"])
            .with_fleet_report(&report);
        assert_eq!(view.confirmation_lines()[0], "FORCE OFFBOARD 2 SYSTEMS");
        assert_eq!(
            view.confirmation_lines()[1],
            format!(
                "scope {}",
                LifecycleConfirmationV1::fleet_scope_digest(&["seat-15", "seat-16"])
            )
        );
        assert_eq!(
            view.coordinator_line().as_deref(),
            Some("coordinator coord-b")
        );
        assert_eq!(view.fleet_line().as_deref(), Some("fleet seat-15, seat-16"));
    }

    #[test]
    fn from_authority_parts_uses_shared_readiness_not_a_fabricated_ready() {
        let plan = LifecyclePlanV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::Onboard,
            generation: 1,
            steps: vec!["identity".into()],
        };
        let progress = LifecycleProgressV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            target_id: "seat-15".into(),
            generation: 1,
            phase: LifecyclePhase::WaitingForOperator,
            completed_steps: 0,
            total_steps: 1,
        };
        let checks = vec![LifecycleRequirementCheckV1 {
            schema_version: 1,
            check_id: "identity".into(),
            target_id: "seat-15".into(),
            expected: "present".into(),
            observed: "missing".into(),
            status: crate::lifecycle::LifecycleCheckStatus::Fail,
            required: true,
            evidence_digest_hex: "2".repeat(64),
            warning: None,
            generation: 1,
        }];
        let view = LifecycleSessionView::from_authority_parts(&plan, &progress, &checks).unwrap();
        assert_eq!(view.status_line(), "request-1: onboard (blocked)");
        assert_eq!(view.missing_requirements, vec!["identity"]);
        let ready = LifecycleSessionView::from_authority_parts(&plan, &progress, &[]).unwrap();
        assert!(ready.missing_requirements.is_empty());
        assert_eq!(ready.status_line(), "request-1: onboard (in progress)");
        let with_coord = ready.with_coordinator(Some("coord-b"));
        assert_eq!(
            with_coord.coordinator_line().as_deref(),
            Some("coordinator coord-b")
        );
    }

    #[test]
    fn onboard_nag_line_names_missing_join_dests() {
        let plan = LifecyclePlanV1 {
            schema_version: 1,
            request_id: "request-nag".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::Onboard,
            generation: 1,
            steps: vec!["mesh".into()],
        };
        let progress = LifecycleProgressV1 {
            schema_version: 1,
            request_id: "request-nag".into(),
            target_id: "seat-15".into(),
            generation: 1,
            phase: LifecyclePhase::Running,
            completed_steps: 0,
            total_steps: 1,
        };
        let checks = vec![LifecycleRequirementCheckV1 {
            schema_version: 1,
            check_id: "mesh_identity".into(),
            target_id: "seat-15".into(),
            expected: "enrolled mesh identity".into(),
            observed: "missing: overlay-ip,etcd-endpoints".into(),
            status: crate::lifecycle::LifecycleCheckStatus::Fail,
            required: true,
            evidence_digest_hex: "3".repeat(64),
            warning: None,
            generation: 1,
        }];
        let view = LifecycleSessionView::from_authority_parts(&plan, &progress, &checks).unwrap();
        assert_eq!(
            view.onboard_nag_line(),
            Some("open ONBOARD: missing overlay-ip,etcd-endpoints")
        );
        assert!(LifecycleSessionView::onboard_nag_from_checks(&[]).is_none());
    }

    #[test]
    fn correction_line_names_the_first_still_blocking_action() {
        let plan = LifecyclePlanV1 {
            schema_version: 1,
            request_id: "request-vac".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::VerifyAndCorrect,
            generation: 1,
            steps: vec!["mesh".into(), "verify".into()],
        };
        let progress = LifecycleProgressV1 {
            schema_version: 1,
            request_id: "request-vac".into(),
            target_id: "seat-15".into(),
            generation: 1,
            phase: LifecyclePhase::Running,
            completed_steps: 0,
            total_steps: 2,
        };
        let checks = vec![
            LifecycleRequirementCheckV1 {
                schema_version: 1,
                check_id: "mesh".into(),
                target_id: "seat-15".into(),
                expected: "joined".into(),
                observed: "absent".into(),
                status: crate::lifecycle::LifecycleCheckStatus::Fail,
                required: true,
                evidence_digest_hex: "a".repeat(64),
                warning: None,
                generation: 1,
            },
            LifecycleRequirementCheckV1 {
                schema_version: 1,
                check_id: "units".into(),
                target_id: "seat-15".into(),
                expected: "active".into(),
                observed: "inactive".into(),
                status: crate::lifecycle::LifecycleCheckStatus::Fail,
                required: true,
                evidence_digest_hex: "b".repeat(64),
                warning: None,
                generation: 1,
            },
        ];
        let correction = LifecycleCorrectionPlanV1 {
            schema_version: 1,
            request_id: "request-vac".into(),
            target_id: "seat-15".into(),
            generation: 1,
            corrections: vec![
                crate::lifecycle::LifecycleCorrectionV1 {
                    check_id: "mesh".into(),
                    step: "mesh".into(),
                    reason: "absent".into(),
                    prerequisites: Vec::new(),
                },
                crate::lifecycle::LifecycleCorrectionV1 {
                    check_id: "units".into(),
                    step: "configuration".into(),
                    reason: "inactive".into(),
                    prerequisites: Vec::new(),
                },
            ],
            edges: Vec::new(),
            rollback_forbidden: true,
        };
        let view = LifecycleSessionView::from_authority_parts(&plan, &progress, &checks)
            .unwrap()
            .with_correction_plan(Some(&correction), &checks);
        assert_eq!(view.correction_line(), Some("correct mesh: mesh (absent)"));
        let mut mesh_pass = checks.clone();
        mesh_pass[0].status = crate::lifecycle::LifecycleCheckStatus::Pass;
        mesh_pass[0].observed = "joined".into();
        let advanced = LifecycleSessionView::from_authority_parts(&plan, &progress, &mesh_pass)
            .unwrap()
            .with_correction_plan(Some(&correction), &mesh_pass);
        assert_eq!(
            advanced.correction_line(),
            Some("correct configuration: units (inactive)")
        );
    }

    #[test]
    fn last_error_line_matches_doctor() {
        let plan = LifecyclePlanV1 {
            schema_version: 1,
            request_id: "request-err".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::VerifyAndCorrect,
            generation: 1,
            steps: vec!["verify".into(), "verify".into()],
        };
        let progress = LifecycleProgressV1 {
            schema_version: 1,
            request_id: "request-err".into(),
            target_id: "seat-15".into(),
            generation: 1,
            phase: LifecyclePhase::Running,
            completed_steps: 0,
            total_steps: 2,
        };
        let view = LifecycleSessionView::from_authority_parts(&plan, &progress, &[])
            .unwrap()
            .with_last_error(Some("provider timeout"));
        assert_eq!(
            view.last_error_line().as_deref(),
            Some("last error: provider timeout")
        );
        assert!(view
            .clone()
            .with_last_error(Some(""))
            .last_error_line()
            .is_none());
    }

    #[test]
    fn receipt_line_binds_only_a_durable_completed_receipt() {
        let plan = LifecyclePlanV1 {
            schema_version: 1,
            request_id: "request-offboard".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::Offboard,
            generation: 1,
            steps: vec!["offboard".into(), "verify".into()],
        };
        let progress = LifecycleProgressV1 {
            schema_version: 1,
            request_id: "request-offboard".into(),
            target_id: "seat-15".into(),
            generation: 1,
            phase: LifecyclePhase::Succeeded,
            completed_steps: 2,
            total_steps: 2,
        };
        let receipt = OffboardingReceiptV1 {
            schema_version: 1,
            request_id: "request-offboard".into(),
            target_id: "seat-15".into(),
            generation: 1,
            completed: true,
            retained_resources: Vec::new(),
            signature_hex: String::new(),
        };
        let view = LifecycleSessionView::from_authority_parts(&plan, &progress, &[])
            .unwrap()
            .with_offboarding_receipt(Some(&receipt));
        assert_eq!(view.receipt_line(), Some("offboard receipt completed"));
        let foreign = OffboardingReceiptV1 {
            request_id: "other-request".into(),
            ..receipt.clone()
        };
        assert!(view
            .clone()
            .with_offboarding_receipt(Some(&foreign))
            .receipt_line()
            .is_none());
        let retained = OffboardingReceiptV1 {
            retained_resources: vec!["identity bundle".into()],
            ..receipt
        };
        assert!(
            LifecycleSessionView::from_authority_parts(&plan, &progress, &[])
                .unwrap()
                .with_offboarding_receipt(Some(&retained))
                .receipt_line()
                .is_none(),
            "retained resources are not a completed dest wipe"
        );
    }

    #[test]
    fn package_line_names_a_staged_pin_and_not_dest_install() {
        let plan = LifecyclePlanV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::Onboard,
            generation: 1,
            steps: vec!["packages".into(), "verify".into()],
        };
        let progress = LifecycleProgressV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            target_id: "seat-15".into(),
            generation: 1,
            phase: LifecyclePhase::Running,
            completed_steps: 1,
            total_steps: 2,
        };
        let digest = "a".repeat(64);
        let view = LifecycleSessionView::from_authority_parts(&plan, &progress, &[])
            .unwrap()
            .with_staged_package(Some(&format!("staged:{digest}:rpm")));
        assert_eq!(
            view.package_line(),
            Some(format!("packages staged:{digest}:rpm (not installed)"))
        );
        assert!(view
            .clone()
            .with_staged_package(Some("magic-mesh-13.0.0-1.fc44.x86_64"))
            .package_line()
            .is_none());
    }

    #[test]
    fn staged_capsule_line_names_a_pending_id_and_not_confirmed() {
        let plan = LifecyclePlanV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::Onboard,
            generation: 1,
            steps: vec!["identity".into(), "verify".into()],
        };
        let progress = LifecycleProgressV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            target_id: "seat-15".into(),
            generation: 1,
            phase: LifecyclePhase::Running,
            completed_steps: 0,
            total_steps: 2,
        };
        let view = LifecycleSessionView::from_authority_parts(&plan, &progress, &[])
            .unwrap()
            .with_staged_capsule(Some("capsule-1"));
        assert_eq!(
            view.capsule_line().as_deref(),
            Some("capsule capsule-1 staged (not confirmed)")
        );
        assert!(view
            .clone()
            .with_staged_capsule(Some("  "))
            .capsule_line()
            .is_none());
    }
}
