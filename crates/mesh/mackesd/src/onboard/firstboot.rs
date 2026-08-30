//! WL-FUNC-023 S17 — first-boot convergence over the canonical lifecycle baseline.
//!
//! Package, installer, and `meshctl doctor` paths must not treat `mackesd status`
//! or an unconditional marker file as readiness. This module audits every
//! [`canonical_lifecycle_baseline`] entry through injectable facts, records the
//! checks on the lifecycle authority, and stamps `firstboot-converged` only when
//! no required check blocks. Failed enrollment tokens stay pending; a blocking
//! result writes `pending-convergence` instead.

use std::io;
use std::path::{Path, PathBuf};

use mackes_mesh_types::lifecycle::{
    canonical_lifecycle_baseline, FleetLifecycleReportV1, LifecycleCheckStatus,
    LifecycleRequirementCheckV1, SeatReadinessV1, LIFECYCLE_CONTRACT_SCHEMA_VERSION,
};
use sha2::{Digest, Sha256};

use crate::lifecycle_authority::{
    peek_fleet_session, peek_matching_fleet_targets, LifecycleAuthority, LifecycleAuthorityError,
};
use crate::onboard::role_provision::{
    units_for_role, GROUPED_MACKESD_CONTROL_UNIT_FILE, GROUPED_MACKESD_UNITS,
};
use mde_role::Role;

/// First-boot must not require its own oneshot: the unit is `activating`
/// while `gather_live` runs, so `systemctl is-active --quiet` can never pass.
const FIRSTBOOT_SELF_UNIT: &str = "mcnf-lifecycle-firstboot.service";

/// Dest-gated Open Onboarding unit. Health/node-grade may warn; first-boot
/// must not treat it as a required plane unit or invent a dest receipt.
const COLLAB_IDENTITY_UNIT: &str = "mcnf-collaboration-identity.service";

/// Marker written only after the canonical baseline has no blocking checks.
pub const FIRSTBOOT_CONVERGED: &str = "firstboot-converged";
/// Marker written when first-boot queued corrected-forward convergence.
pub const FIRSTBOOT_PENDING: &str = "pending-convergence";
/// Production marker directory. Tests inject another path.
pub const DEFAULT_MARKER_DIR: &str = "/var/lib/mackesd/lifecycle";

/// Production Nebula config root. Dest-cut seats store the host cert under
/// `identity/current/`; older seats keep a flat `host.crt`. First-boot must
/// see the same two paths telemetry and nebula_supervisor already use.
const NEBULA_CONFIG_ROOT: &str = "/etc/nebula";
/// Dest-cut identity layout relative to [`NEBULA_CONFIG_ROOT`].
const ACTIVE_NEBULA_HOST_CERT_REL: &str = "identity/current/host.crt";
/// Legacy flat layout relative to [`NEBULA_CONFIG_ROOT`].
const LEGACY_NEBULA_HOST_CERT_REL: &str = "host.crt";

/// Observed first-boot facts. Production gathers these from the seat; tests
/// plant missing units, packages, and identity without touching systemd.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirstbootFacts {
    /// Lifecycle target id recorded on every check.
    pub target_id: String,
    /// Authority generation the checks must bind to.
    pub generation: u64,
    /// Whether the shipped package/binary is present.
    pub package_present: bool,
    /// Observed package identity (NEVRA, path, or `missing`).
    pub package_identity: String,
    /// Role-catalog units this seat must run.
    pub expected_units: Vec<String>,
    /// Units observed active.
    pub active_units: Vec<String>,
    /// Whether the pinned role/configuration file is present.
    pub configuration_present: bool,
    /// Whether mesh identity material is present.
    pub mesh_identity_present: bool,
    /// Whether `/var/lib/mackesd/nebula/overlay-ip` is a non-empty regular file.
    pub overlay_ip_present: bool,
    /// Whether `/etc/mackesd/etcd-endpoints` names at least one endpoint.
    pub etcd_endpoints_present: bool,
    /// Workstations must have etcd-endpoints; lighthouse members do not.
    pub etcd_endpoints_required: bool,
    /// Whether virtualization is usable. Failure is a warning, not a block.
    pub compute_usable: bool,
    /// Whether the UI requirement applies (Workstation). Lighthouse skips it.
    pub ui_applicable: bool,
    /// Whether the DRM shell unit is ready when UI applies.
    pub ui_ready: bool,
    /// Whether required hardware (DRM, etc.) is present. Failure is a warning.
    pub hardware_usable: bool,
    /// Count of still-pending enrollment capsules/tokens. First-boot never
    /// decrements this; a failed run must leave it unchanged.
    pub pending_enrollment_tokens: usize,
}

/// Result of applying first-boot markers. Capsule/token files are never named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstbootMarker {
    /// Baseline produced no blocking checks.
    Converged,
    /// Core failures remain; convergence is queued.
    Pending,
}

/// Fold [`canonical_lifecycle_baseline`] over [`FirstbootFacts`].
#[must_use]
pub fn assemble(facts: &FirstbootFacts) -> Vec<LifecycleRequirementCheckV1> {
    canonical_lifecycle_baseline()
        .into_iter()
        .map(|entry| match entry.requirement_id.as_str() {
            "packages" => check(
                facts,
                "packages",
                "installed magic-mesh package",
                facts.package_identity.as_str(),
                if facts.package_present {
                    LifecycleCheckStatus::Pass
                } else {
                    LifecycleCheckStatus::Fail
                },
                true,
                None,
            ),
            "units" => {
                let missing = missing_required_units(facts);
                let observed = if missing.is_empty() {
                    "all role units active".to_owned()
                } else {
                    format!("inactive: {}", missing.join(","))
                };
                check(
                    facts,
                    "units",
                    "role catalog units active",
                    &observed,
                    if missing.is_empty() {
                        LifecycleCheckStatus::Pass
                    } else {
                        LifecycleCheckStatus::Fail
                    },
                    true,
                    None,
                )
            }
            "configuration" => check(
                facts,
                "configuration",
                "pinned role configuration",
                if facts.configuration_present {
                    "present"
                } else {
                    "missing"
                },
                if facts.configuration_present {
                    LifecycleCheckStatus::Pass
                } else {
                    LifecycleCheckStatus::Fail
                },
                true,
                None,
            ),
            "mesh_identity" => {
                let missing = missing_mesh_join_dests(facts);
                check(
                    facts,
                    "mesh_identity",
                    "enrolled mesh identity",
                    if missing.is_empty() {
                        "present"
                    } else {
                        missing.as_str()
                    },
                    if missing.is_empty() {
                        LifecycleCheckStatus::Pass
                    } else {
                        LifecycleCheckStatus::Fail
                    },
                    true,
                    None,
                )
            }
            "compute" => check(
                facts,
                "compute",
                "virtualization usable",
                if facts.compute_usable {
                    "kvm ready"
                } else {
                    "kvm unavailable"
                },
                if facts.compute_usable {
                    LifecycleCheckStatus::Pass
                } else {
                    LifecycleCheckStatus::Warn
                },
                true,
                (!facts.compute_usable).then_some("capability unavailable: kvm".to_owned()),
            ),
            "ui" => {
                if !facts.ui_applicable {
                    check(
                        facts,
                        "ui",
                        "DRM shell when Workstation",
                        "not-applicable",
                        LifecycleCheckStatus::Pass,
                        true,
                        None,
                    )
                } else {
                    check(
                        facts,
                        "ui",
                        "DRM shell when Workstation",
                        if facts.ui_ready { "ready" } else { "inactive" },
                        if facts.ui_ready {
                            LifecycleCheckStatus::Pass
                        } else {
                            LifecycleCheckStatus::Fail
                        },
                        true,
                        None,
                    )
                }
            }
            "hardware" => check(
                facts,
                "hardware",
                "required seat hardware",
                if facts.hardware_usable {
                    "present"
                } else {
                    "degraded"
                },
                if facts.hardware_usable {
                    LifecycleCheckStatus::Pass
                } else {
                    LifecycleCheckStatus::Warn
                },
                true,
                (!facts.hardware_usable).then_some("capability unavailable: hardware".to_owned()),
            ),
            "verification" => {
                let units_missing = !missing_required_units(facts).is_empty();
                let others_block = !facts.package_present
                    || units_missing
                    || !facts.configuration_present
                    || !missing_mesh_join_dests(facts).is_empty()
                    || (facts.ui_applicable && !facts.ui_ready);
                check(
                    facts,
                    "verification",
                    "canonical baseline has no blocking core failure",
                    if others_block { "blocked" } else { "clear" },
                    if others_block {
                        LifecycleCheckStatus::Fail
                    } else {
                        LifecycleCheckStatus::Pass
                    },
                    true,
                    None,
                )
            }
            other => check(
                facts,
                other,
                "known baseline entry",
                "unknown requirement",
                LifecycleCheckStatus::Unknown,
                true,
                Some(format!("unrecognized baseline id {other}")),
            ),
        })
        .collect()
}

fn check(
    facts: &FirstbootFacts,
    check_id: &str,
    expected: &str,
    observed: &str,
    status: LifecycleCheckStatus,
    required: bool,
    warning: Option<String>,
) -> LifecycleRequirementCheckV1 {
    let mut hasher = Sha256::new();
    hasher.update(check_id.as_bytes());
    hasher.update(expected.as_bytes());
    hasher.update(observed.as_bytes());
    hasher.update(format!("{status:?}").as_bytes());
    LifecycleRequirementCheckV1 {
        schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
        check_id: check_id.to_owned(),
        target_id: facts.target_id.clone(),
        expected: expected.to_owned(),
        observed: observed.to_owned(),
        status,
        required,
        evidence_digest_hex: format!("{:x}", hasher.finalize()),
        warning,
        generation: facts.generation,
    }
}

/// Project the persisted VAC action through the shared session view.
/// Absent when no blocking correction remains; dest repair is not implied.
pub fn next_correction_line_from_checkpoint(
    checkpoint: &crate::lifecycle_authority::LifecycleCheckpointV1,
) -> Option<String> {
    use mackes_mesh_types::lifecycle_view::LifecycleSessionView;
    LifecycleSessionView::from_authority_parts(
        &checkpoint.plan,
        &checkpoint.progress,
        &checkpoint.checks,
    )
    .ok()
    .map(|view| view.with_correction_plan(checkpoint.correction_plan.as_ref(), &checkpoint.checks))
    .and_then(|view| view.correction_line().map(str::to_owned))
}

/// Project the persisted VAC action through the shared session view.
/// Absent when no blocking correction remains; dest repair is not implied.
pub fn next_correction_line(authority: &LifecycleAuthority) -> Option<String> {
    next_correction_line_from_checkpoint(authority.checkpoint())
}

/// Name the next VAC action without persisting a plan. Report-only
/// first-boot and doctor preview use this; dest repair is not implied.
pub fn preview_correction_line(authority: &LifecycleAuthority) -> Option<String> {
    preview_correction_line_from_checkpoint(authority.checkpoint())
}

/// One typed nag when join dests are missing. Dest write is not implied.
#[must_use]
pub fn onboard_nag_line(checks: &[LifecycleRequirementCheckV1]) -> Option<String> {
    mackes_mesh_types::lifecycle_view::LifecycleSessionView::onboard_nag_from_checks(checks)
}

/// Per-seat status lines for a fleet report. Renderers already share these
/// phrases; the fleet CLI must not invent a different ready/blocked answer.
pub fn fleet_seat_status_lines(
    checkpoint: &crate::lifecycle_authority::LifecycleCheckpointV1,
) -> Vec<String> {
    let target = &checkpoint.plan.target_id;
    let mut lines = Vec::new();
    if let Some(nag) = onboard_nag_line(&checkpoint.checks) {
        lines.push(format!("{target}: {nag}"));
    }
    if let Some(line) = preview_correction_line_from_checkpoint(checkpoint) {
        lines.push(format!("{target}: {line}"));
    }
    if let Some(error) = checkpoint
        .last_error
        .as_deref()
        .filter(|error| !error.is_empty())
    {
        lines.push(format!("{target}: last error: {error}"));
    }
    lines
}

/// Peek-only receipt line. Missing or invalid is absent; dest wipe is not implied.
#[must_use]
pub fn peek_receipt_status_line(root: &Path, target_id: &str) -> Option<String> {
    LifecycleAuthority::peek_offboarding_receipt(root, target_id)
        .ok()
        .flatten()
        .map(|_| format!("{target_id}: offboard receipt completed"))
}

/// Peek-only staged pin. Dest NEVRA/path is never this line.
#[must_use]
pub fn peek_staged_package_line(root: &Path, target_id: &str) -> Option<String> {
    staged_package_identity(&root.join("lifecycle").join(target_id))
        .map(|identity| format!("{target_id}: packages {identity} (not installed)"))
}

/// Peek-only pending capsule id. Missing or symlink is absent.
#[must_use]
pub fn peek_staged_capsule_id(root: &Path, target_id: &str) -> Option<String> {
    let checkpoint = crate::lifecycle_authority::LifecycleAuthority::peek(root, target_id).ok()?;
    let capsule_id = checkpoint.pending_capsule_ids.first()?.trim();
    if capsule_id.is_empty() {
        return None;
    }
    let path = root
        .join("lifecycle")
        .join(target_id)
        .join("capsule")
        .join(capsule_id);
    let meta = std::fs::symlink_metadata(&path).ok()?;
    meta.file_type().is_file().then(|| capsule_id.to_owned())
}

/// Peek-only staged capsule. Confirm is never this line.
#[must_use]
pub fn peek_staged_capsule_line(root: &Path, target_id: &str) -> Option<String> {
    peek_staged_capsule_id(root, target_id)
        .map(|capsule_id| format!("{target_id}: capsule {capsule_id} staged (not confirmed)"))
}

fn append_receipt_lines(
    root: &Path,
    target_ids: impl IntoIterator<Item = impl AsRef<str>>,
    mut lines: Vec<String>,
) -> Vec<String> {
    for target_id in target_ids {
        let target_id = target_id.as_ref();
        if let Some(line) = peek_receipt_status_line(root, target_id) {
            if !lines.iter().any(|existing| existing == &line) {
                lines.push(line);
            }
        }
        if let Some(line) = peek_staged_package_line(root, target_id) {
            if !lines.iter().any(|existing| existing == &line) {
                lines.push(line);
            }
        }
        if let Some(line) = peek_staged_capsule_line(root, target_id) {
            if !lines.iter().any(|existing| existing == &line) {
                lines.push(line);
            }
        }
    }
    lines
}

/// Shared fleet status for CLI, doctor, and first-boot. Peek-only.
pub fn fleet_session_status_lines(
    report: &FleetLifecycleReportV1,
    checkpoints: &[crate::lifecycle_authority::LifecycleCheckpointV1],
) -> Vec<String> {
    let mut targets: Vec<String> = checkpoints
        .iter()
        .map(|checkpoint| checkpoint.plan.target_id.clone())
        .collect();
    targets.sort();
    targets.dedup();
    let mut lines = Vec::new();
    if !report.coordinator_id.is_empty() {
        lines.push(format!("coordinator {}", report.coordinator_id));
    }
    if targets.len() > 1 {
        lines.push(format!("fleet {}", targets.join(", ")));
    }
    for checkpoint in checkpoints {
        lines.extend(fleet_seat_status_lines(checkpoint));
    }
    lines
}

/// Peek-safe preview. Does not persist and does not take the authority lock.
pub fn preview_correction_line_from_checkpoint(
    checkpoint: &crate::lifecycle_authority::LifecycleCheckpointV1,
) -> Option<String> {
    if let Some(line) = next_correction_line_from_checkpoint(checkpoint) {
        return Some(line);
    }
    use mackes_mesh_types::lifecycle_view::LifecycleSessionView;
    let proposed = checkpoint.propose_correction_plan().ok()?;
    LifecycleSessionView::from_authority_parts(
        &checkpoint.plan,
        &checkpoint.progress,
        &checkpoint.checks,
    )
    .ok()
    .map(|view| view.with_correction_plan(Some(&proposed), &checkpoint.checks))
    .and_then(|view| view.correction_line().map(str::to_owned))
}

fn firstboot_marker_is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
}

/// Doctor detail for first-boot markers. When pending-convergence is
/// queued, name the persisted VAC action instead of a bare ready bool.
pub fn doctor_lifecycle_detail(marker_dir: &Path, authority_root: &Path) -> (bool, String) {
    let pending = marker_dir.join(FIRSTBOOT_PENDING);
    let converged = marker_dir.join(FIRSTBOOT_CONVERGED);
    if let Some(detail) = planted_marker_refuse_line(marker_dir) {
        return (false, detail);
    }
    if let Some(nag) = LifecycleAuthority::peek_latest(authority_root)
        .ok()
        .flatten()
        .as_ref()
        .and_then(|checkpoint| onboard_nag_line(&checkpoint.checks))
    {
        return (false, nag);
    }
    if pending.exists() {
        let checkpoint = LifecycleAuthority::peek_latest(authority_root)
            .ok()
            .flatten();
        let detail = checkpoint
            .as_ref()
            .and_then(|checkpoint| preview_correction_line_from_checkpoint(checkpoint))
            .or_else(|| {
                checkpoint
                    .as_ref()
                    .and_then(|checkpoint| checkpoint.last_error.clone())
                    .map(|error| format!("last error: {error}"))
            })
            .unwrap_or_else(|| "pending-convergence queued; core baseline still blocked".into());
        return (false, detail);
    }
    if converged.is_file() {
        (true, "canonical baseline converged".into())
    } else {
        (false, "firstboot-converged marker missing".into())
    }
}

/// Peek-only fleet status for the newest durable generation. Empty when
/// only one local seat is published and no coordinator is held; dest
/// repair is not implied.
pub fn firstboot_fleet_status_lines(authority_root: &Path) -> Vec<String> {
    let Some(latest) = LifecycleAuthority::peek_latest(authority_root)
        .ok()
        .flatten()
    else {
        return Vec::new();
    };
    let Ok(targets) = peek_matching_fleet_targets(
        authority_root,
        &latest.plan.request_id,
        latest.plan.generation,
    ) else {
        return Vec::new();
    };
    let Ok((report, checkpoints)) = peek_fleet_session(authority_root, &targets) else {
        return Vec::new();
    };
    if targets.len() <= 1 && report.coordinator_id.is_empty() {
        return Vec::new();
    }
    append_receipt_lines(
        authority_root,
        &targets,
        fleet_session_status_lines(&report, &checkpoints),
    )
}

/// Status lines for one peeked seat. If that seat is part of a durable
/// fleet generation, name every target so readiness cannot hide them.
pub fn readiness_status_lines(
    authority_root: &Path,
    checkpoint: &crate::lifecycle_authority::LifecycleCheckpointV1,
) -> Vec<String> {
    let targets = peek_matching_fleet_targets(
        authority_root,
        &checkpoint.plan.request_id,
        checkpoint.plan.generation,
    )
    .unwrap_or_default();
    let peek_targets = if targets.is_empty() {
        vec![checkpoint.plan.target_id.clone()]
    } else {
        targets.clone()
    };
    let has_coordinator = checkpoint
        .coordinator_id
        .as_deref()
        .is_some_and(|id| !id.is_empty());
    let lines = if peek_targets.len() > 1 || has_coordinator {
        match peek_fleet_session(authority_root, &peek_targets) {
            Ok((report, checkpoints)) => fleet_session_status_lines(&report, &checkpoints),
            Err(_) => fleet_seat_status_lines(checkpoint),
        }
    } else {
        fleet_seat_status_lines(checkpoint)
    };
    let receipt_targets = if targets.is_empty() {
        vec![checkpoint.plan.target_id.clone()]
    } else {
        targets
    };
    with_planted_marker_refuse(
        authority_root,
        append_receipt_lines(authority_root, &receipt_targets, lines),
    )
}

fn with_planted_marker_refuse(authority_root: &Path, mut lines: Vec<String>) -> Vec<String> {
    if let Some(refuse) = planted_marker_refuse_line(&authority_root.join("lifecycle")) {
        if lines.first().map(String::as_str) != Some(refuse.as_str()) {
            lines.insert(0, refuse);
        }
    }
    lines
}

/// Peek-only fleet CLI lines. A planted first-boot marker cannot hide them.
#[must_use]
pub fn fleet_status_lines(
    authority_root: &Path,
    report: &FleetLifecycleReportV1,
    checkpoints: &[crate::lifecycle_authority::LifecycleCheckpointV1],
) -> Vec<String> {
    with_planted_marker_refuse(
        authority_root,
        append_receipt_lines(
            authority_root,
            checkpoints
                .iter()
                .map(|checkpoint| checkpoint.plan.target_id.as_str()),
            fleet_session_status_lines(report, checkpoints),
        ),
    )
}

/// Doctor lines for every durable seat in the newest fleet generation.
/// A single peeked seat cannot hide another target's correction.
pub fn doctor_fleet_lifecycle_lines(
    marker_dir: &Path,
    authority_root: &Path,
) -> (bool, Vec<String>) {
    let (ok, detail) = doctor_lifecycle_detail(marker_dir, authority_root);
    let fleet = firstboot_fleet_status_lines(authority_root);
    if !ok && detail.contains("must not be a symlink") {
        let mut lines = vec![detail];
        lines.extend(fleet);
        return (false, lines);
    }
    if fleet.is_empty() {
        let mut lines = vec![detail];
        if let Some(latest) = LifecycleAuthority::peek_latest(authority_root)
            .ok()
            .flatten()
        {
            lines = append_receipt_lines(
                authority_root,
                std::slice::from_ref(&latest.plan.target_id),
                lines,
            );
        }
        (ok, lines)
    } else {
        (ok, fleet)
    }
}

/// Same join `meshctl doctor` prints for the first-boot check.
#[must_use]
pub fn doctor_check_detail(lines: &[String]) -> String {
    lines.join("; ")
}

/// Peek-only first-boot report. Does not take the authority lock and does
/// not persist checks or markers.
#[must_use]
pub fn report_only_firstboot(
    authority_root: &Path,
    marker_dir: &Path,
    target_id: &str,
    role: Role,
) -> (SeatReadinessV1, Vec<String>) {
    let peeked = LifecycleAuthority::peek(authority_root, target_id).ok();
    let generation = peeked
        .as_ref()
        .map(|checkpoint| checkpoint.plan.generation)
        .unwrap_or(1);
    let facts = gather_live_in(
        target_id,
        generation,
        role,
        Some(authority_root),
        Some(marker_dir),
    );
    let checks = assemble(&facts);
    let readiness = SeatReadinessV1::from_requirement_checks(
        LIFECYCLE_CONTRACT_SCHEMA_VERSION,
        target_id,
        generation,
        &checks,
    )
    .unwrap_or(SeatReadinessV1 {
        schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
        target_id: target_id.to_owned(),
        generation,
        ready: false,
        missing_requirements: vec!["baseline".into()],
        warnings: Vec::new(),
    });
    let mut lines = Vec::new();
    if let Some(line) = onboard_nag_line(&checks) {
        lines.push(format!("first-boot nag: {line}"));
    }
    if let Some(checkpoint) = peeked.as_ref() {
        if let Some(line) = preview_correction_line_from_checkpoint(checkpoint) {
            lines.push(format!("first-boot correction: {line}"));
        }
    }
    if let Some(line) = planted_marker_refuse_line(marker_dir) {
        lines.push(format!("first-boot doctor: {line}"));
    }
    for line in firstboot_fleet_status_lines(authority_root) {
        lines.push(format!("first-boot fleet: {line}"));
    }
    if let Some(line) = peek_staged_package_line(authority_root, target_id) {
        lines.push(format!("first-boot {line}"));
    }
    if let Some(line) = peek_staged_capsule_line(authority_root, target_id) {
        lines.push(format!("first-boot {line}"));
    }
    if let Some(line) = peek_receipt_status_line(authority_root, target_id) {
        lines.push(format!("first-boot {line}"));
    }
    (readiness, lines)
}

/// Peek-safe planted-marker refuse for report-only first-boot and doctor.
/// Absent when both markers are regular files or missing.
#[must_use]
pub fn planted_marker_refuse_line(marker_dir: &Path) -> Option<String> {
    let pending = marker_dir.join(FIRSTBOOT_PENDING);
    let converged = marker_dir.join(FIRSTBOOT_CONVERGED);
    if firstboot_marker_is_symlink(&pending) || firstboot_marker_is_symlink(&converged) {
        Some("first-boot marker must not be a symlink; dest repair is not implied".into())
    } else {
        None
    }
}

/// Record assembled checks on the authority without touching capsules.
pub fn record_on_authority(
    authority: &mut LifecycleAuthority,
    checks: Vec<LifecycleRequirementCheckV1>,
) -> Result<SeatReadinessV1, LifecycleAuthorityError> {
    authority.replace_checks(checks)?;
    authority.readiness()
}

/// Stamp or refuse the first-boot markers. Never deletes enrollment material.
pub fn apply_markers(dir: &Path, ready: bool) -> io::Result<FirstbootMarker> {
    std::fs::create_dir_all(dir)?;
    let converged = dir.join(FIRSTBOOT_CONVERGED);
    let pending = dir.join(FIRSTBOOT_PENDING);
    if ready && staged_package_identity(dir).is_some() {
        write_marker(&pending, b"queued\n")?;
        let _ = std::fs::remove_file(&converged);
        return Ok(FirstbootMarker::Pending);
    }
    if ready {
        write_marker(&converged, b"canonical-baseline\n")?;
        let _ = std::fs::remove_file(&pending);
        Ok(FirstbootMarker::Converged)
    } else {
        write_marker(&pending, b"queued\n")?;
        let _ = std::fs::remove_file(&converged);
        Ok(FirstbootMarker::Pending)
    }
}

/// True when any assembled baseline check is a required Fail or Unknown.
#[must_use]
pub fn has_blocking_checks(checks: &[LifecycleRequirementCheckV1]) -> bool {
    checks
        .iter()
        .any(LifecycleRequirementCheckV1::blocks_progress)
}

/// Stamp markers from the assembled baseline. A planted unit Fail cannot be
/// ignored by passing `ready: true` to [`apply_markers`].
pub fn apply_markers_from_checks(
    dir: &Path,
    checks: &[LifecycleRequirementCheckV1],
) -> io::Result<FirstbootMarker> {
    apply_markers(dir, !has_blocking_checks(checks))
}

/// After a failed invite enrollment, retain the token and queue
/// `pending-convergence`. Never stamps Ready: a failed enrollment is a
/// critical activation failure even if a hostile caller planted healthy facts.
///
/// # Errors
/// Propagates marker or ledger IO failures.
pub fn queue_after_failed_enrollment(
    marker_dir: &Path,
    workgroup_root: &Path,
    presented: &str,
) -> io::Result<FirstbootMarker> {
    crate::onboard::invite::retain_failed_enrollment(workgroup_root, presented)?;
    apply_markers(marker_dir, false)
}

fn write_marker(path: &Path, body: &[u8]) -> io::Result<()> {
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "first-boot marker must not be a symlink",
            ));
        }
    }
    std::fs::write(path, body)
}

/// Live seat facts for the CLI. Unknown probes stay fail-closed for core
/// rows and warning-level for capability rows. Pending enrollment tokens
/// stay `0` unless a workgroup root is supplied via [`gather_live_in`].
#[must_use]
pub fn gather_live(target_id: &str, generation: u64, role: Role) -> FirstbootFacts {
    gather_live_in(target_id, generation, role, None, None)
}

/// Units first-boot requires to be active.
///
/// This is not a copy of [`units_for_role`]: that catalog still lists the
/// enable/mask set (including this oneshot and, on every rank, `etcd.service`
/// plus monolithic `mackesd.service`). First-boot facts must match the live
/// plane: grouped `mackesd-*.service` when shipped, no self-check, and no
/// workstation etcd member. Dest-gated collaboration-identity is Open
/// Onboarding, not a first-boot unit block; workstation grouped plane also
/// drops timer units leaked from the enable/mask catalog.
#[must_use]
pub fn runtime_expected_units(role: Role, grouped_control_unit_file_present: bool) -> Vec<String> {
    let mut units: Vec<String> = units_for_role(role)
        .into_iter()
        .filter(|unit| *unit != FIRSTBOOT_SELF_UNIT)
        .filter(|unit| !is_open_onboarding_unit(unit))
        .map(str::to_owned)
        .collect();
    if grouped_control_unit_file_present {
        units.retain(|unit| unit != "mackesd.service");
        for grouped in GROUPED_MACKESD_UNITS {
            if !units.iter().any(|unit| unit == grouped) {
                units.push((*grouped).to_owned());
            }
        }
    }
    if role == Role::Workstation {
        units.retain(|unit| unit != "etcd.service");
        if grouped_control_unit_file_present {
            units.retain(|unit| !is_timer_unit(unit));
        }
    }
    units.retain(|unit| !is_open_onboarding_unit(unit));
    units
}

/// Dest-gated collaboration identity is Open Onboarding, not a first-boot
/// plane requirement. Matches the shipped unit and any `units_for_role` leak
/// that carries the same identity name.
fn is_open_onboarding_unit(unit: &str) -> bool {
    unit == COLLAB_IDENTITY_UNIT || unit.contains("collaboration-identity")
}

fn is_timer_unit(unit: &str) -> bool {
    unit.ends_with(".timer")
}

/// Missing units that actually block the first-boot `units` row. Open
/// Onboarding collab-identity is skipped even if a catalog leak planted it
/// on [`FirstbootFacts::expected_units`].
fn missing_required_units(facts: &FirstbootFacts) -> Vec<&str> {
    facts
        .expected_units
        .iter()
        .filter(|unit| !is_open_onboarding_unit(unit))
        .filter(|unit| !facts.active_units.iter().any(|active| active == *unit))
        .map(String::as_str)
        .collect()
}

/// Join dests S11 must observe. Missing overlay-ip or workstation
/// etcd-endpoints fail mesh and nag into ONBOARD; this helper never writes them.
fn missing_mesh_join_dests(facts: &FirstbootFacts) -> String {
    let mut missing = Vec::new();
    if !facts.mesh_identity_present {
        missing.push("host-cert");
    }
    if !facts.overlay_ip_present {
        missing.push("overlay-ip");
    }
    if facts.etcd_endpoints_required && !facts.etcd_endpoints_present {
        missing.push("etcd-endpoints");
    }
    if missing.is_empty() {
        String::new()
    } else {
        format!("missing: {}", missing.join(","))
    }
}

/// True when overlay-ip is a non-empty regular file. Tests inject a temp path
/// so coverage does not write dest `/var/lib/mackesd/nebula/overlay-ip`.
#[must_use]
pub fn overlay_ip_present_at(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .is_some_and(|body| !body.trim().is_empty())
}

/// True when etcd-endpoints names at least one endpoint. Tests inject a temp
/// path so coverage does not write dest `/etc/mackesd/etcd-endpoints`.
#[must_use]
pub fn etcd_endpoints_present_at(path: &Path) -> bool {
    std::fs::read_to_string(path).ok().is_some_and(|body| {
        body.split(|character| character == ',' || character == '\n')
            .any(|endpoint| !endpoint.trim().is_empty())
    })
}

/// Caller-supplied overlay pin. Join stages from this; dest write is not implied.
pub const JOIN_OVERLAY_IP_PIN: &str = "join-overlay-ip";
/// Caller-supplied workstation etcd pin. Absent on lighthouse.
pub const JOIN_ETCD_ENDPOINTS_PIN: &str = "join-etcd-endpoints";
/// Staged overlay-ip under the supplied root, never the live nebula dest.
pub const STAGED_OVERLAY_IP: &str = "overlay-ip";
/// Staged etcd-endpoints under the supplied root, never `/etc/mackesd`.
pub const STAGED_ETCD_ENDPOINTS: &str = "etcd-endpoints";
/// Staged grouped-plane unit list. Dest systemd enable is not implied.
pub const STAGED_GROUPED_PLANE: &str = "grouped-plane";

const LIVE_OVERLAY_IP: &str = "/var/lib/mackesd/nebula/overlay-ip";

/// Stage join dests from pins under `dest_root`. Returns true when an overlay
/// pin was present and files were written. Never writes live
/// `/var/lib/mackesd/nebula/overlay-ip` or `/etc/mackesd/etcd-endpoints`.
pub fn stage_mesh_join_dests(dest_root: &Path) -> io::Result<bool> {
    refuse_live_join_dest_files(dest_root)?;
    let overlay_pin = dest_root.join(JOIN_OVERLAY_IP_PIN);
    if !overlay_pin.exists() {
        return Ok(false);
    }
    refuse_join_symlink(&overlay_pin)?;
    if !overlay_pin.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "join overlay-ip pin must be a regular file; dest write is not implied",
        ));
    }
    let overlay = std::fs::read_to_string(&overlay_pin)?;
    let overlay = overlay.trim();
    if overlay.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "join overlay-ip pin is empty; dest write is not implied",
        ));
    }
    std::fs::create_dir_all(dest_root)?;
    let overlay_dest = dest_root.join(STAGED_OVERLAY_IP);
    refuse_join_symlink(&overlay_dest)?;
    std::fs::write(&overlay_dest, format!("{overlay}\n"))?;

    let etcd_pin = dest_root.join(JOIN_ETCD_ENDPOINTS_PIN);
    if etcd_pin.exists() {
        refuse_join_symlink(&etcd_pin)?;
        if !etcd_pin.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "join etcd-endpoints pin must be a regular file; dest write is not implied",
            ));
        }
        let endpoints = std::fs::read_to_string(&etcd_pin)?;
        if !etcd_endpoints_present_at(&etcd_pin) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "join etcd-endpoints pin is empty; dest write is not implied",
            ));
        }
        let etcd_dest = dest_root.join(STAGED_ETCD_ENDPOINTS);
        refuse_join_symlink(&etcd_dest)?;
        std::fs::write(&etcd_dest, endpoints)?;
    }

    let plane = dest_root.join(STAGED_GROUPED_PLANE);
    refuse_join_symlink(&plane)?;
    let mut body = GROUPED_MACKESD_UNITS.join("\n");
    body.push('\n');
    std::fs::write(&plane, body)?;
    Ok(true)
}

/// Write join pins under `dest_root`. Never writes live overlay-ip or
/// etcd-endpoints dests. Credential env uses [`pin_mesh_join_from_env`].
pub fn pin_mesh_join_dests(
    dest_root: &Path,
    overlay_ip: &str,
    etcd_endpoints: Option<&str>,
) -> io::Result<()> {
    refuse_live_join_dest_files(dest_root)?;
    let overlay = overlay_ip.trim();
    if overlay.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "join overlay-ip pin is empty; dest write is not implied",
        ));
    }
    std::fs::create_dir_all(dest_root)?;
    let overlay_pin = dest_root.join(JOIN_OVERLAY_IP_PIN);
    refuse_join_symlink(&overlay_pin)?;
    std::fs::write(&overlay_pin, format!("{overlay}\n"))?;
    if let Some(endpoints) = etcd_endpoints {
        let etcd_pin = dest_root.join(JOIN_ETCD_ENDPOINTS_PIN);
        refuse_join_symlink(&etcd_pin)?;
        std::fs::write(&etcd_pin, endpoints)?;
        if !etcd_endpoints_present_at(&etcd_pin) {
            let _ = std::fs::remove_file(&etcd_pin);
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "join etcd-endpoints pin is empty; dest write is not implied",
            ));
        }
    }
    Ok(())
}

/// Credential env, never argv. Missing overlay env is a no-op.
pub fn pin_mesh_join_from_env(dest_root: &Path) -> io::Result<bool> {
    let overlay = match std::env::var("MCNF_JOIN_OVERLAY_IP") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => return Ok(false),
    };
    let endpoints = std::env::var("MCNF_JOIN_ETCD_ENDPOINTS")
        .ok()
        .filter(|value| {
            value
                .split(|character| character == ',' || character == '\n')
                .any(|endpoint| !endpoint.trim().is_empty())
        });
    pin_mesh_join_dests(dest_root, &overlay, endpoints.as_deref())?;
    Ok(true)
}

/// Pin then stage join dests under `workgroup_root/lifecycle`. Never writes
/// live `/var/lib/mackesd/nebula/overlay-ip` or `/etc/mackesd/etcd-endpoints`.
pub fn pin_and_stage_mesh_join(
    workgroup_root: &Path,
    overlay_ip: &str,
    etcd_endpoints: Option<&str>,
) -> io::Result<bool> {
    let marker_dir = workgroup_root.join("lifecycle");
    pin_mesh_join_dests(&marker_dir, overlay_ip, etcd_endpoints)?;
    stage_mesh_join_dests(&marker_dir)
}

fn refuse_join_symlink(path: &Path) -> io::Result<()> {
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("join dest");
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{name} must not be a symlink; dest join write is not implied"),
            ));
        }
    }
    Ok(())
}

fn refuse_live_join_dest_files(dest_root: &Path) -> io::Result<()> {
    if dest_root == Path::new("/") {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "live dest root is not implied",
        ));
    }
    let overlay = dest_root.join(STAGED_OVERLAY_IP);
    let endpoints = dest_root.join(STAGED_ETCD_ENDPOINTS);
    if overlay == Path::new(LIVE_OVERLAY_IP)
        || endpoints == Path::new(crate::substrate::etcd::ENDPOINTS_FILE)
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "live dest join paths are not implied",
        ));
    }
    Ok(())
}

fn overlay_ip_file_present() -> bool {
    overlay_ip_present_at(Path::new("/var/lib/mackesd/nebula/overlay-ip"))
}

fn etcd_endpoints_file_present() -> bool {
    etcd_endpoints_present_at(Path::new(crate::substrate::etcd::ENDPOINTS_FILE))
}

/// Live seat facts, counting pending invite/enrollment bearers from the
/// workgroup ledger when `workgroup_root` is present.
#[must_use]
pub fn gather_live_in(
    target_id: &str,
    generation: u64,
    role: Role,
    workgroup_root: Option<&Path>,
    marker_dir: Option<&Path>,
) -> FirstbootFacts {
    let expected_units =
        runtime_expected_units(role, Path::new(GROUPED_MACKESD_CONTROL_UNIT_FILE).is_file());
    let active_units: Vec<String> = expected_units
        .iter()
        .filter(|unit| unit_is_active(unit))
        .cloned()
        .collect();
    let ui_applicable = role == Role::Workstation;
    let (package_present, package_identity) = package_identity_or_staged(
        Path::new("/usr/bin/mackesd")
            .is_file()
            .then_some("/usr/bin/mackesd"),
        marker_dir.unwrap_or_else(|| Path::new(DEFAULT_MARKER_DIR)),
    );
    FirstbootFacts {
        target_id: target_id.to_owned(),
        generation,
        package_present,
        package_identity,
        expected_units,
        active_units,
        configuration_present: Path::new("/var/lib/mde/role.toml").is_file(),
        mesh_identity_present: mesh_identity_present_under(Path::new(NEBULA_CONFIG_ROOT)),
        overlay_ip_present: overlay_ip_file_present(),
        etcd_endpoints_present: etcd_endpoints_file_present(),
        etcd_endpoints_required: role == Role::Workstation,
        compute_usable: Path::new("/dev/kvm").exists(),
        ui_applicable,
        ui_ready: !ui_applicable || unit_is_active("mde-shell-egui.service"),
        hardware_usable: Path::new("/dev/dri").exists(),
        pending_enrollment_tokens: workgroup_root
            .map(crate::onboard::invite::count_pending)
            .unwrap_or(0),
    }
}

/// True when either dest-cut `identity/current/host.crt` or legacy `host.crt`
/// is a regular file under `nebula_root`. Tests inject a temp root so coverage
/// does not require a live `/etc/nebula`.
#[must_use]
pub fn mesh_identity_present_under(nebula_root: &Path) -> bool {
    nebula_root.join(ACTIVE_NEBULA_HOST_CERT_REL).is_file()
        || nebula_root.join(LEGACY_NEBULA_HOST_CERT_REL).is_file()
}

fn unit_is_active(unit: &str) -> bool {
    let mut command = std::process::Command::new("systemctl");
    command.args(["is-active", "--quiet", unit]);
    crate::lifecycle_child_env::strip_lifecycle_child_env(&mut command);
    command
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Production marker directory as a [`PathBuf`].
#[must_use]
pub fn default_marker_dir() -> PathBuf {
    PathBuf::from(DEFAULT_MARKER_DIR)
}

/// Observed package identity when bytes were staged but dest install did not run.
/// Staging is not `package_present`; first-boot must still Fail packages.
#[must_use]
pub fn staged_package_identity(marker_dir: &Path) -> Option<String> {
    let digest_path = marker_dir.join("staged-artifact.digest");
    let staged = marker_dir.join("staged-artifact");
    let shape_path = marker_dir.join("staged-artifact.shape");
    if firstboot_marker_is_symlink(&digest_path)
        || firstboot_marker_is_symlink(&staged)
        || firstboot_marker_is_symlink(&shape_path)
    {
        return None;
    }
    let digest = std::fs::read_to_string(digest_path).ok()?;
    let digest = digest.trim();
    if digest.len() != 64 || !digest.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let shape = std::fs::read_to_string(shape_path).ok()?;
    let shape = shape.trim();
    if !matches!(shape, "rpm" | "bootc" | "kickstart" | "nocloud" | "usb") {
        return None;
    }
    let bytes = std::fs::read(staged).ok()?;
    let observed = format!("{:x}", Sha256::digest(&bytes));
    (observed == digest).then(|| format!("staged:{digest}:{shape}"))
}

/// Prefer dest NEVRA/path; otherwise name a verified staged digest. Never
/// treat stage as installed.
#[must_use]
pub fn package_identity_or_staged(installed: Option<&str>, marker_dir: &Path) -> (bool, String) {
    if let Some(identity) = installed.filter(|value| !value.is_empty()) {
        return (true, identity.to_owned());
    }
    match staged_package_identity(marker_dir) {
        Some(staged) => (false, staged),
        None => (false, "missing".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifecycle_authority::LifecycleAuthority;
    use ed25519_dalek::SigningKey;
    use mackes_mesh_types::lifecycle::{
        CommissioningCapsuleV1, LifecycleIntentKind, LifecyclePlanV1, LifecycleStepKind,
    };

    fn healthy(target: &str) -> FirstbootFacts {
        FirstbootFacts {
            target_id: target.to_owned(),
            generation: 1,
            package_present: true,
            package_identity: "magic-mesh-13.0.0-1.fc44.x86_64".to_owned(),
            expected_units: vec!["mackesd.service".into(), "nebula.service".into()],
            active_units: vec!["mackesd.service".into(), "nebula.service".into()],
            configuration_present: true,
            mesh_identity_present: true,
            overlay_ip_present: true,
            etcd_endpoints_present: true,
            etcd_endpoints_required: false,
            compute_usable: true,
            ui_applicable: false,
            ui_ready: false,
            hardware_usable: true,
            pending_enrollment_tokens: 1,
        }
    }

    #[test]
    fn staged_artifact_is_observed_and_is_not_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let bytes = b"rpm-bytes";
        let digest = format!("{:x}", Sha256::digest(bytes));
        std::fs::write(tmp.path().join("staged-artifact"), bytes).unwrap();
        std::fs::write(
            tmp.path().join("staged-artifact.digest"),
            format!("{digest}\n"),
        )
        .unwrap();
        std::fs::write(tmp.path().join("staged-artifact.shape"), "rpm\n").unwrap();
        let (present, identity) = package_identity_or_staged(None, tmp.path());
        assert!(!present, "stage must not imply dest RPM/bootc install");
        assert_eq!(identity, format!("staged:{digest}:rpm"));
        let mut facts = healthy("seat-15");
        facts.package_present = present;
        facts.package_identity = identity;
        let checks = assemble(&facts);
        assert!(
            checks
                .iter()
                .any(|check| check.check_id == "packages" && check.blocks_progress()),
            "staged-but-not-installed must still Fail packages"
        );
        let observed = checks
            .iter()
            .find(|check| check.check_id == "packages")
            .map(|check| check.observed.clone())
            .unwrap();
        assert_eq!(observed, format!("staged:{digest}:rpm"));
    }

    #[test]
    fn apply_markers_cannot_converge_while_a_pin_is_only_staged() {
        let tmp = tempfile::tempdir().unwrap();
        let bytes = b"rpm-bytes";
        let digest = format!("{:x}", Sha256::digest(bytes));
        std::fs::write(tmp.path().join("staged-artifact"), bytes).unwrap();
        std::fs::write(
            tmp.path().join("staged-artifact.digest"),
            format!("{digest}\n"),
        )
        .unwrap();
        std::fs::write(tmp.path().join("staged-artifact.shape"), "rpm\n").unwrap();
        let marker = apply_markers(tmp.path(), true).unwrap();
        assert_eq!(marker, FirstbootMarker::Pending);
        assert!(tmp.path().join(FIRSTBOOT_PENDING).is_file());
        assert!(!tmp.path().join(FIRSTBOOT_CONVERGED).exists());
    }

    #[test]
    fn staged_package_identity_requires_a_known_shape() {
        let tmp = tempfile::tempdir().unwrap();
        let bytes = b"rpm-bytes";
        let digest = format!("{:x}", Sha256::digest(bytes));
        std::fs::write(tmp.path().join("staged-artifact"), bytes).unwrap();
        std::fs::write(
            tmp.path().join("staged-artifact.digest"),
            format!("{digest}\n"),
        )
        .unwrap();
        assert!(
            staged_package_identity(tmp.path()).is_none(),
            "digest without a pinned shape is not dest install"
        );
        std::fs::write(tmp.path().join("staged-artifact.shape"), "tarball\n").unwrap();
        assert!(
            staged_package_identity(tmp.path()).is_none(),
            "an unsupported shape is not dest RPM/bootc/USB"
        );
    }

    #[test]
    fn staged_package_identity_refuses_a_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("dest-rpm");
        std::fs::write(&dest, b"rpm-bytes").unwrap();
        let digest = format!("{:x}", Sha256::digest(b"rpm-bytes"));
        let markers = tmp.path().join("markers");
        std::fs::create_dir_all(&markers).unwrap();
        std::fs::write(
            markers.join("staged-artifact.digest"),
            format!("{digest}\n"),
        )
        .unwrap();
        std::os::unix::fs::symlink(&dest, markers.join("staged-artifact")).unwrap();
        assert_eq!(
            staged_package_identity(&markers),
            None,
            "a planted staged-artifact symlink is not dest install"
        );
        let (present, identity) = package_identity_or_staged(None, &markers);
        assert!(!present);
        assert_eq!(identity, "missing");
        assert_eq!(std::fs::read(&dest).unwrap(), b"rpm-bytes");
    }

    #[test]
    fn staged_package_identity_refuses_a_digest_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("dest-digest");
        let digest = format!("{:x}", Sha256::digest(b"rpm-bytes"));
        std::fs::write(&dest, format!("{digest}\n")).unwrap();
        let markers = tmp.path().join("markers");
        std::fs::create_dir_all(&markers).unwrap();
        std::fs::write(markers.join("staged-artifact"), b"rpm-bytes").unwrap();
        std::os::unix::fs::symlink(&dest, markers.join("staged-artifact.digest")).unwrap();
        assert_eq!(
            staged_package_identity(&markers),
            None,
            "a planted digest symlink is not dest install"
        );
        let (present, identity) = package_identity_or_staged(None, &markers);
        assert!(!present);
        assert_eq!(identity, "missing");
        assert_eq!(
            std::fs::read_to_string(&dest).unwrap().trim(),
            digest.as_str()
        );
    }

    #[test]
    fn missing_overlay_ip_or_etcd_endpoints_fail_mesh_and_queue() {
        let mut facts = healthy("seat-15");
        facts.etcd_endpoints_required = true;
        facts.overlay_ip_present = false;
        facts.etcd_endpoints_present = false;
        let checks = assemble(&facts);
        let mesh = checks
            .iter()
            .find(|check| check.check_id == "mesh_identity")
            .expect("mesh_identity check");
        assert!(mesh.blocks_progress(), "missing join dests must fail mesh");
        assert_eq!(mesh.observed, "missing: overlay-ip,etcd-endpoints");
        assert_eq!(
            onboard_nag_line(&checks).as_deref(),
            Some("open ONBOARD: missing overlay-ip,etcd-endpoints")
        );
        assert!(
            checks
                .iter()
                .any(|check| check.check_id == "verification" && check.blocks_progress()),
            "verification must not ignore missing overlay-ip or etcd-endpoints"
        );
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            apply_markers_from_checks(tmp.path(), &checks).unwrap(),
            FirstbootMarker::Pending
        );
        assert!(tmp.path().join(FIRSTBOOT_PENDING).exists());
        assert!(!tmp.path().join(FIRSTBOOT_CONVERGED).exists());
    }

    #[test]
    fn join_dest_files_are_observed_without_writing_production_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let overlay = tmp.path().join("overlay-ip");
        let endpoints = tmp.path().join("etcd-endpoints");
        assert!(!overlay_ip_present_at(&overlay));
        assert!(!etcd_endpoints_present_at(&endpoints));
        std::fs::write(&overlay, b"10.42.0.15\n").unwrap();
        std::fs::write(&endpoints, b"https://10.42.0.1:2379\n").unwrap();
        assert!(overlay_ip_present_at(&overlay));
        assert!(etcd_endpoints_present_at(&endpoints));
        std::fs::write(&endpoints, b"\n,\n").unwrap();
        assert!(
            !etcd_endpoints_present_at(&endpoints),
            "empty endpoint list is not a dest join"
        );
    }

    #[test]
    fn stage_mesh_join_dests_writes_overlay_etcd_and_grouped_plane() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(JOIN_OVERLAY_IP_PIN), "10.42.0.15\n").unwrap();
        std::fs::write(
            tmp.path().join(JOIN_ETCD_ENDPOINTS_PIN),
            "https://10.42.0.1:2379\n",
        )
        .unwrap();
        assert!(stage_mesh_join_dests(tmp.path()).unwrap());
        assert_eq!(
            std::fs::read_to_string(tmp.path().join(STAGED_OVERLAY_IP))
                .unwrap()
                .trim(),
            "10.42.0.15"
        );
        assert_eq!(
            std::fs::read_to_string(tmp.path().join(STAGED_ETCD_ENDPOINTS))
                .unwrap()
                .trim(),
            "https://10.42.0.1:2379"
        );
        let plane = std::fs::read_to_string(tmp.path().join(STAGED_GROUPED_PLANE)).unwrap();
        for unit in GROUPED_MACKESD_UNITS {
            assert!(
                plane.lines().any(|line| line == unit),
                "grouped plane must name {unit}: {plane}"
            );
        }
    }

    #[test]
    fn stage_mesh_join_dests_skips_etcd_when_the_pin_is_absent() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(JOIN_OVERLAY_IP_PIN), "10.42.0.16\n").unwrap();
        assert!(stage_mesh_join_dests(tmp.path()).unwrap());
        assert!(tmp.path().join(STAGED_OVERLAY_IP).is_file());
        assert!(
            !tmp.path().join(STAGED_ETCD_ENDPOINTS).exists(),
            "lighthouse join must not invent workstation etcd-endpoints"
        );
        assert!(tmp.path().join(STAGED_GROUPED_PLANE).is_file());
    }

    #[test]
    fn stage_mesh_join_dests_is_a_no_op_without_an_overlay_pin() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!stage_mesh_join_dests(tmp.path()).unwrap());
        assert!(!tmp.path().join(STAGED_OVERLAY_IP).exists());
        assert!(!tmp.path().join(STAGED_GROUPED_PLANE).exists());
    }

    #[test]
    fn stage_mesh_join_dests_refuses_empty_or_symlink_pins() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(JOIN_OVERLAY_IP_PIN), "\n").unwrap();
        let error =
            stage_mesh_join_dests(tmp.path()).expect_err("empty overlay is not a dest join");
        assert!(error.to_string().contains("empty"));

        let dest = tmp.path().join("keep-overlay");
        std::fs::write(&dest, b"10.42.0.99\n").unwrap();
        std::fs::remove_file(tmp.path().join(JOIN_OVERLAY_IP_PIN)).unwrap();
        std::os::unix::fs::symlink(&dest, tmp.path().join(JOIN_OVERLAY_IP_PIN)).unwrap();
        let error = stage_mesh_join_dests(tmp.path()).expect_err("symlink pin is not dest join");
        assert!(error.to_string().contains("symlink"));
        assert_eq!(std::fs::read(&dest).unwrap(), b"10.42.0.99\n");
    }

    #[test]
    fn stage_mesh_join_dests_refuses_the_live_nebula_dest_root() {
        let live = Path::new("/var/lib/mackesd/nebula");
        let error = stage_mesh_join_dests(live).expect_err("live overlay dest is not implied");
        assert!(error.to_string().contains("live dest"));
    }

    #[test]
    fn pin_mesh_join_dests_writes_pins_not_live_dests() {
        let tmp = tempfile::tempdir().unwrap();
        pin_mesh_join_dests(tmp.path(), "10.42.0.21", Some("https://10.42.0.1:2379")).unwrap();
        assert_eq!(
            std::fs::read_to_string(tmp.path().join(JOIN_OVERLAY_IP_PIN))
                .unwrap()
                .trim(),
            "10.42.0.21"
        );
        assert_eq!(
            std::fs::read_to_string(tmp.path().join(JOIN_ETCD_ENDPOINTS_PIN))
                .unwrap()
                .trim(),
            "https://10.42.0.1:2379"
        );
        assert!(
            !tmp.path().join(STAGED_OVERLAY_IP).exists(),
            "pin must not imply dest overlay write"
        );
        assert!(stage_mesh_join_dests(tmp.path()).unwrap());
        assert_eq!(
            std::fs::read_to_string(tmp.path().join(STAGED_OVERLAY_IP))
                .unwrap()
                .trim(),
            "10.42.0.21"
        );
    }

    #[test]
    fn pin_mesh_join_dests_refuses_empty_overlay() {
        let tmp = tempfile::tempdir().unwrap();
        let error = pin_mesh_join_dests(tmp.path(), " \n", None)
            .expect_err("empty overlay is not a dest pin");
        assert!(error.to_string().contains("empty"));
        assert!(!tmp.path().join(JOIN_OVERLAY_IP_PIN).exists());
    }

    #[test]
    fn pin_and_stage_mesh_join_uses_the_workgroup_lifecycle_dir() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(
            pin_and_stage_mesh_join(tmp.path(), "10.42.0.31", Some("https://10.42.0.1:2379"),)
                .unwrap()
        );
        let dir = tmp.path().join("lifecycle");
        assert_eq!(
            std::fs::read_to_string(dir.join(STAGED_OVERLAY_IP))
                .unwrap()
                .trim(),
            "10.42.0.31"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join(STAGED_ETCD_ENDPOINTS))
                .unwrap()
                .trim(),
            "https://10.42.0.1:2379"
        );
        assert!(dir.join(STAGED_GROUPED_PLANE).is_file());
        assert!(!tmp.path().join(STAGED_OVERLAY_IP).exists());
    }

    #[test]
    fn pin_mesh_join_dests_from_env_is_noop_without_overlay() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::remove_var("MCNF_JOIN_OVERLAY_IP");
        std::env::remove_var("MCNF_JOIN_ETCD_ENDPOINTS");
        assert!(!pin_mesh_join_from_env(tmp.path()).unwrap());
        assert!(!tmp.path().join(JOIN_OVERLAY_IP_PIN).exists());
    }

    #[test]
    fn planted_missing_unit_cannot_produce_ready() {
        let mut facts = healthy("seat-15");
        facts.active_units.retain(|unit| unit != "mackesd.service");
        let checks = assemble(&facts);
        assert!(
            checks
                .iter()
                .any(|check| check.check_id == "units" && check.blocks_progress()),
            "missing mackesd.service must block"
        );
        assert!(
            checks
                .iter()
                .any(|check| check.check_id == "verification" && check.blocks_progress()),
            "verification must not ignore a blocking core failure"
        );
        let tmp = tempfile::tempdir().unwrap();
        let marker = apply_markers(
            tmp.path(),
            !checks
                .iter()
                .any(LifecycleRequirementCheckV1::blocks_progress),
        )
        .unwrap();
        assert_eq!(marker, FirstbootMarker::Pending);
        assert!(!tmp.path().join(FIRSTBOOT_CONVERGED).exists());
        assert!(tmp.path().join(FIRSTBOOT_PENDING).exists());
    }

    #[test]
    fn healthy_baseline_stamps_converged_and_keeps_tokens() {
        let facts = healthy("seat-15");
        let checks = assemble(&facts);
        assert_eq!(checks.len(), canonical_lifecycle_baseline().len());
        assert!(!checks
            .iter()
            .any(LifecycleRequirementCheckV1::blocks_progress));
        let tmp = tempfile::tempdir().unwrap();
        let token = tmp.path().join("enrollment.token");
        std::fs::write(&token, b"keep-me").unwrap();
        assert_eq!(
            apply_markers(tmp.path(), true).unwrap(),
            FirstbootMarker::Converged
        );
        assert!(tmp.path().join(FIRSTBOOT_CONVERGED).exists());
        assert!(!tmp.path().join(FIRSTBOOT_PENDING).exists());
        assert_eq!(std::fs::read(&token).unwrap(), b"keep-me");
        assert_eq!(facts.pending_enrollment_tokens, 1);
    }

    #[test]
    fn compute_and_hardware_failures_are_warnings() {
        let mut facts = healthy("dell");
        facts.compute_usable = false;
        facts.hardware_usable = false;
        let checks = assemble(&facts);
        assert!(!checks
            .iter()
            .any(LifecycleRequirementCheckV1::blocks_progress));
        assert!(checks
            .iter()
            .any(|c| c.check_id == "compute" && c.status == LifecycleCheckStatus::Warn));
        assert!(checks
            .iter()
            .any(|c| c.check_id == "hardware" && c.status == LifecycleCheckStatus::Warn));
        let plan = LifecyclePlanV1 {
            schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
            request_id: "warn-1".into(),
            target_id: "dell".into(),
            intent: LifecycleIntentKind::VerifyAndCorrect,
            generation: 1,
            steps: vec!["verify".into(), "verify".into()],
        };
        let progress = mackes_mesh_types::lifecycle::LifecycleProgressV1 {
            schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
            request_id: "warn-1".into(),
            target_id: "dell".into(),
            generation: 1,
            phase: mackes_mesh_types::lifecycle::LifecyclePhase::Succeeded,
            completed_steps: 2,
            total_steps: 2,
        };
        let view = mackes_mesh_types::lifecycle_view::LifecycleSessionView::from_authority_parts(
            &plan, &progress, &checks,
        )
        .unwrap();
        assert_eq!(
            view.readiness,
            mackes_mesh_types::lifecycle_view::ReadinessState::ReadyWithWarnings
        );
        assert_eq!(view.capabilities, vec!["kvm", "hardware"]);
    }

    #[test]
    fn doctor_names_last_error_when_no_blocking_correction() {
        let root = tempfile::tempdir().unwrap();
        let plan = LifecyclePlanV1 {
            schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
            request_id: "firstboot-retry".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::VerifyAndCorrect,
            generation: 1,
            steps: vec!["verify".into(), "configuration".into(), "verify".into()],
        };
        let mut authority = LifecycleAuthority::begin(root.path(), plan).unwrap();
        assert!(authority
            .run_next(|_| Err("provider timeout".into()))
            .is_err());
        apply_markers(&root.path().join("lifecycle"), false).unwrap();
        let (ok, detail) = doctor_lifecycle_detail(&root.path().join("lifecycle"), root.path());
        assert!(!ok);
        assert_eq!(detail, "last error: provider timeout");
        authority.finish().unwrap();
    }

    #[test]
    fn doctor_onboard_nag_when_join_dests_are_missing() {
        let root = tempfile::tempdir().unwrap();
        let plan = LifecyclePlanV1 {
            schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
            request_id: "firstboot-nag".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::Onboard,
            generation: 1,
            steps: vec!["mesh".into(), "verify".into()],
        };
        let mut authority = LifecycleAuthority::begin(root.path(), plan).unwrap();
        authority
            .record_check(LifecycleRequirementCheckV1 {
                schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
                check_id: "mesh_identity".into(),
                target_id: "seat-15".into(),
                expected: "enrolled mesh identity".into(),
                observed: "missing: overlay-ip,etcd-endpoints".into(),
                status: LifecycleCheckStatus::Fail,
                required: true,
                evidence_digest_hex: "d".repeat(64),
                warning: None,
                generation: 1,
            })
            .unwrap();
        apply_markers(&root.path().join("lifecycle"), false).unwrap();
        let (ok, detail) = doctor_lifecycle_detail(&root.path().join("lifecycle"), root.path());
        assert!(!ok);
        assert_eq!(detail, "open ONBOARD: missing overlay-ip,etcd-endpoints");
        let lines = fleet_seat_status_lines(authority.checkpoint());
        assert!(
            lines
                .iter()
                .any(|line| line == "seat-15: open ONBOARD: missing overlay-ip,etcd-endpoints"),
            "fleet CLI must nag into ONBOARD: {lines:?}"
        );
        authority.finish().unwrap();
    }

    #[test]
    fn doctor_refuses_a_pending_convergence_symlink() {
        let root = tempfile::tempdir().unwrap();
        let dest = root.path().join("dest-apply");
        std::fs::write(&dest, b"keep").unwrap();
        let markers = root.path().join("lifecycle");
        std::fs::create_dir_all(&markers).unwrap();
        std::os::unix::fs::symlink(&dest, markers.join(FIRSTBOOT_PENDING)).unwrap();
        let (ok, detail) = doctor_lifecycle_detail(&markers, root.path());
        assert!(!ok);
        assert!(
            detail.contains("symlink"),
            "doctor must not follow a planted marker into dest: {detail}"
        );
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            b"keep",
            "doctor must not treat dest material as a first-boot marker"
        );
        assert_eq!(
            planted_marker_refuse_line(&markers).as_deref(),
            Some("first-boot marker must not be a symlink; dest repair is not implied")
        );
    }

    #[test]
    fn report_only_firstboot_does_not_take_the_lock() {
        let root = tempfile::tempdir().unwrap();
        let plan = LifecyclePlanV1 {
            schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
            request_id: "report-only".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::VerifyAndCorrect,
            generation: 1,
            steps: vec!["verify".into(), "configuration".into(), "verify".into()],
        };
        let held = LifecycleAuthority::begin(root.path(), plan).unwrap();
        let before = held.checkpoint().checks.clone();
        let dest = root.path().join("dest-apply");
        std::fs::write(&dest, b"keep").unwrap();
        let markers = root.path().join("lifecycle");
        std::os::unix::fs::symlink(&dest, markers.join(FIRSTBOOT_PENDING)).unwrap();
        let (readiness, lines) =
            report_only_firstboot(root.path(), &markers, "seat-15", Role::Workstation);
        assert!(!readiness.ready);
        let peeked = LifecycleAuthority::peek(root.path(), "seat-15").unwrap();
        assert_eq!(
            peeked.checks, before,
            "report-only must not persist assembled checks onto a held seat"
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("must not be a symlink")),
            "report-only must name a planted marker without locking: {lines:?}"
        );
        assert!(
            LifecycleAuthority::resume(root.path(), "seat-15").is_err(),
            "report-only must not steal the authority lock"
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"keep");
        held.finish().unwrap();
    }

    #[test]
    fn report_only_firstboot_names_the_staged_package_without_the_lock() {
        let root = tempfile::tempdir().unwrap();
        let plan = LifecyclePlanV1 {
            schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
            request_id: "report-staged".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::Onboard,
            generation: 1,
            steps: vec!["packages".into(), "verify".into()],
        };
        let held = LifecycleAuthority::begin(root.path(), plan).unwrap();
        let before = held.checkpoint().checks.clone();
        let digest = "e262f1de2c38fd96cb1a8a8410f58222f0e0b5681b84217b877e78c114eb9a31";
        let dir = root.path().join("lifecycle").join("seat-15");
        std::fs::write(dir.join("staged-artifact"), b"rpm-bytes").unwrap();
        std::fs::write(dir.join("staged-artifact.digest"), format!("{digest}\n")).unwrap();
        std::fs::write(dir.join("staged-artifact.shape"), "rpm\n").unwrap();
        let markers = root.path().join("markers");
        std::fs::create_dir_all(&markers).unwrap();
        let (_readiness, lines) =
            report_only_firstboot(root.path(), &markers, "seat-15", Role::Workstation);
        assert!(
            lines.iter().any(|line| {
                line == "first-boot seat-15: packages staged:e262f1de2c38fd96cb1a8a8410f58222f0e0b5681b84217b877e78c114eb9a31:rpm (not installed)"
            }),
            "report-only must name the staged pin without locking: {lines:?}"
        );
        let peeked = LifecycleAuthority::peek(root.path(), "seat-15").unwrap();
        assert_eq!(
            peeked.checks, before,
            "report-only must not persist assembled checks onto a held seat"
        );
        assert!(
            LifecycleAuthority::resume(root.path(), "seat-15").is_err(),
            "report-only must not steal the authority lock"
        );
        held.finish().unwrap();
    }

    #[test]
    fn report_only_firstboot_names_the_staged_capsule_without_the_lock() {
        let root = tempfile::tempdir().unwrap();
        let plan = LifecyclePlanV1 {
            schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
            request_id: "report-capsule".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::Onboard,
            generation: 1,
            steps: vec!["identity".into(), "verify".into()],
        };
        let mut held = LifecycleAuthority::begin(root.path(), plan).unwrap();
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let capsule = CommissioningCapsuleV1 {
            schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
            capsule_id: "cap-report".into(),
            target_id: "seat-15".into(),
            expires_at_ms: 2_000,
            bootstrap_digest_hex: "ab".repeat(32),
            one_time: true,
            key_id: "commissioning-v1".into(),
            signature_hex: String::new(),
        }
        .sign("commissioning-v1", &signing);
        held.admit_commissioning_capsule(capsule, 1_000, &signing.verifying_key())
            .unwrap();
        let markers = root.path().join("markers");
        std::fs::create_dir_all(&markers).unwrap();
        let (_readiness, lines) =
            report_only_firstboot(root.path(), &markers, "seat-15", Role::Workstation);
        assert!(
            lines.iter().any(|line| {
                line == "first-boot seat-15: capsule cap-report staged (not confirmed)"
            }),
            "report-only must name the staged capsule without locking: {lines:?}"
        );
        assert!(
            LifecycleAuthority::resume(root.path(), "seat-15").is_err(),
            "report-only must not steal the authority lock"
        );
        held.finish().unwrap();
    }

    #[test]
    fn report_only_firstboot_names_the_receipt_without_the_lock() {
        let root = tempfile::tempdir().unwrap();
        let plan = LifecyclePlanV1 {
            schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
            request_id: "report-receipt".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::Offboard,
            generation: 1,
            steps: vec!["offboard".into(), "verify".into()],
        };
        let held = LifecycleAuthority::begin(root.path(), plan).unwrap();
        let before = held.checkpoint().checks.clone();
        let receipt = mackes_mesh_types::lifecycle::OffboardingReceiptV1 {
            schema_version: 1,
            request_id: "report-receipt".into(),
            target_id: "seat-15".into(),
            generation: 1,
            completed: true,
            retained_resources: Vec::new(),
            signature_hex: String::new(),
        };
        std::fs::write(
            root.path()
                .join("lifecycle")
                .join("seat-15")
                .join("receipt.json"),
            serde_json::to_vec_pretty(&receipt).unwrap(),
        )
        .unwrap();
        let markers = root.path().join("markers");
        std::fs::create_dir_all(&markers).unwrap();
        let (_readiness, lines) =
            report_only_firstboot(root.path(), &markers, "seat-15", Role::Workstation);
        assert!(
            lines
                .iter()
                .any(|line| line == "first-boot seat-15: offboard receipt completed"),
            "report-only must name the durable receipt without locking: {lines:?}"
        );
        let peeked = LifecycleAuthority::peek(root.path(), "seat-15").unwrap();
        assert_eq!(
            peeked.checks, before,
            "report-only must not persist assembled checks onto a held seat"
        );
        assert!(
            LifecycleAuthority::resume(root.path(), "seat-15").is_err(),
            "report-only must not steal the authority lock"
        );
        held.finish().unwrap();
    }

    #[test]
    fn report_only_firstboot_names_the_fleet_without_the_lock() {
        let root = tempfile::tempdir().unwrap();
        let vac_plan = |target: &str| LifecyclePlanV1 {
            schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
            request_id: "report-fleet".into(),
            target_id: target.into(),
            intent: LifecycleIntentKind::VerifyAndCorrect,
            generation: 1,
            steps: vec!["verify".into(), "configuration".into(), "verify".into()],
        };
        let held = LifecycleAuthority::begin(root.path(), vac_plan("seat-15")).unwrap();
        let second = LifecycleAuthority::begin(root.path(), vac_plan("seat-16")).unwrap();
        second.finish().unwrap();
        let markers = root.path().join("lifecycle");
        let (_, lines) = report_only_firstboot(root.path(), &markers, "seat-15", Role::Workstation);
        assert!(
            lines
                .iter()
                .any(|line| line == "first-boot fleet: fleet seat-15, seat-16"),
            "report-only must name every durable seat without locking: {lines:?}"
        );
        assert!(
            LifecycleAuthority::resume(root.path(), "seat-15").is_err(),
            "report-only must not steal a held fleet lock"
        );
        held.finish().unwrap();
    }

    #[test]
    fn report_only_firstboot_names_the_coordinator_without_the_lock() {
        let root = tempfile::tempdir().unwrap();
        let vac_plan = |target: &str| LifecyclePlanV1 {
            schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
            request_id: "report-coord".into(),
            target_id: target.into(),
            intent: LifecycleIntentKind::VerifyAndCorrect,
            generation: 1,
            steps: vec!["verify".into(), "configuration".into(), "verify".into()],
        };
        let first = LifecycleAuthority::begin(root.path(), vac_plan("seat-15")).unwrap();
        let second = LifecycleAuthority::begin(root.path(), vac_plan("seat-16")).unwrap();
        let mut authorities = [first, second];
        crate::lifecycle_authority::execute_fleet_handoff(&mut authorities, "coord-a", "coord-b")
            .unwrap();
        for authority in authorities {
            authority.finish().unwrap();
        }
        let held = LifecycleAuthority::resume(root.path(), "seat-15").unwrap();
        let markers = root.path().join("lifecycle");
        let (_, lines) = report_only_firstboot(root.path(), &markers, "seat-16", Role::Workstation);
        assert!(
            lines
                .iter()
                .any(|line| line == "first-boot fleet: coordinator coord-b"),
            "report-only must name the durable coordinator without locking: {lines:?}"
        );
        assert!(
            LifecycleAuthority::resume(root.path(), "seat-15").is_err(),
            "report-only must not steal the coordinator seat lock"
        );
        held.finish().unwrap();
    }

    #[test]
    fn report_only_firstboot_names_a_sibling_last_error_without_the_lock() {
        let root = tempfile::tempdir().unwrap();
        let vac_plan = |target: &str| LifecyclePlanV1 {
            schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
            request_id: "report-err".into(),
            target_id: target.into(),
            intent: LifecycleIntentKind::VerifyAndCorrect,
            generation: 1,
            steps: vec!["verify".into(), "configuration".into(), "verify".into()],
        };
        let held = LifecycleAuthority::begin(root.path(), vac_plan("seat-15")).unwrap();
        let mut failed = LifecycleAuthority::begin(root.path(), vac_plan("seat-16")).unwrap();
        assert!(failed.run_next(|_| Err("wave-2 timeout".into())).is_err());
        failed.finish().unwrap();
        let markers = root.path().join("lifecycle");
        let (_, lines) = report_only_firstboot(root.path(), &markers, "seat-15", Role::Workstation);
        assert!(
            lines
                .iter()
                .any(|line| { line == "first-boot fleet: seat-16: last error: wave-2 timeout" }),
            "report-only must surface a sibling last error without locking: {lines:?}"
        );
        assert!(
            LifecycleAuthority::resume(root.path(), "seat-15").is_err(),
            "report-only must not steal a held fleet lock"
        );
        held.finish().unwrap();
    }

    #[test]
    fn report_only_firstboot_names_a_sibling_correction_without_the_lock() {
        let root = tempfile::tempdir().unwrap();
        let vac_plan = |target: &str| LifecyclePlanV1 {
            schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
            request_id: "report-vac".into(),
            target_id: target.into(),
            intent: LifecycleIntentKind::VerifyAndCorrect,
            generation: 1,
            steps: vec!["verify".into(), "configuration".into(), "verify".into()],
        };
        let held = LifecycleAuthority::begin(root.path(), vac_plan("seat-15")).unwrap();
        let mut sibling = LifecycleAuthority::begin(root.path(), vac_plan("seat-16")).unwrap();
        sibling
            .record_check(LifecycleRequirementCheckV1 {
                schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
                check_id: "mesh".into(),
                target_id: "seat-16".into(),
                expected: "joined".into(),
                observed: "absent".into(),
                status: LifecycleCheckStatus::Fail,
                required: true,
                evidence_digest_hex: "c".repeat(64),
                warning: None,
                generation: 1,
            })
            .unwrap();
        let correction = sibling.propose_correction_plan().unwrap();
        sibling.admit_correction_plan(correction).unwrap();
        sibling.finish().unwrap();
        let markers = root.path().join("lifecycle");
        let (_, lines) = report_only_firstboot(root.path(), &markers, "seat-15", Role::Workstation);
        assert!(
            lines
                .iter()
                .any(|line| { line == "first-boot fleet: seat-16: correct mesh: mesh (absent)" }),
            "report-only must surface a sibling correction without locking: {lines:?}"
        );
        assert!(
            LifecycleAuthority::resume(root.path(), "seat-15").is_err(),
            "report-only must not steal a held fleet lock"
        );
        held.finish().unwrap();
    }

    #[test]
    fn doctor_refuses_a_firstboot_converged_symlink() {
        let root = tempfile::tempdir().unwrap();
        let dest = root.path().join("dest-ready");
        std::fs::write(&dest, b"keep").unwrap();
        let markers = root.path().join("lifecycle");
        std::fs::create_dir_all(&markers).unwrap();
        std::os::unix::fs::symlink(&dest, markers.join(FIRSTBOOT_CONVERGED)).unwrap();
        let (ok, detail) = doctor_lifecycle_detail(&markers, root.path());
        assert!(!ok);
        assert!(
            detail.contains("symlink"),
            "doctor must not follow a planted converged marker into dest: {detail}"
        );
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            b"keep",
            "a converged symlink is not dest-ready"
        );
    }

    #[test]
    fn fleet_seat_status_lines_name_correction_and_last_error() {
        let root = tempfile::tempdir().unwrap();
        let plan = LifecyclePlanV1 {
            schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
            request_id: "fleet-status".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::VerifyAndCorrect,
            generation: 1,
            steps: vec!["verify".into(), "configuration".into(), "verify".into()],
        };
        let mut authority = LifecycleAuthority::begin(root.path(), plan).unwrap();
        authority
            .record_check(LifecycleRequirementCheckV1 {
                schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
                check_id: "mesh".into(),
                target_id: "seat-15".into(),
                expected: "joined".into(),
                observed: "absent".into(),
                status: LifecycleCheckStatus::Fail,
                required: true,
                evidence_digest_hex: "c".repeat(64),
                warning: None,
                generation: 1,
            })
            .unwrap();
        assert!(authority
            .run_next(|_| Err("provider timeout".into()))
            .is_err());
        let lines = fleet_seat_status_lines(authority.checkpoint());
        assert!(
            lines
                .iter()
                .any(|line| line == "seat-15: correct mesh: mesh (absent)"),
            "fleet CLI must name the previewed VAC action: {lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|line| line == "seat-15: last error: provider timeout"),
            "fleet CLI must name the persisted last error: {lines:?}"
        );
        let peeked = LifecycleAuthority::peek(root.path(), "seat-15").unwrap();
        assert_eq!(
            fleet_seat_status_lines(&peeked),
            lines,
            "lifecycle-readiness peek must match the locked fleet lines"
        );
        let readiness = readiness_status_lines(root.path(), &peeked);
        assert_eq!(readiness, lines);
        assert!(
            !readiness.iter().any(|line| line.starts_with("fleet ")),
            "a single seat is not a fleet: {readiness:?}"
        );
        assert!(
            LifecycleAuthority::resume(root.path(), "seat-15").is_err(),
            "status peek must not steal the fleet authority lock"
        );
        authority.finish().unwrap();
    }

    #[test]
    fn readiness_status_lines_name_the_durable_receipt() {
        let root = tempfile::tempdir().unwrap();
        let authority = LifecycleAuthority::begin(
            root.path(),
            LifecyclePlanV1 {
                schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
                request_id: "request-receipt".into(),
                target_id: "seat-15".into(),
                intent: LifecycleIntentKind::Offboard,
                generation: 1,
                steps: vec!["offboard".into(), "verify".into()],
            },
        )
        .unwrap();
        authority.finish().unwrap();
        let receipt = mackes_mesh_types::lifecycle::OffboardingReceiptV1 {
            schema_version: 1,
            request_id: "request-receipt".into(),
            target_id: "seat-15".into(),
            generation: 1,
            completed: true,
            retained_resources: Vec::new(),
            signature_hex: String::new(),
        };
        std::fs::write(
            root.path()
                .join("lifecycle")
                .join("seat-15")
                .join("receipt.json"),
            serde_json::to_vec_pretty(&receipt).unwrap(),
        )
        .unwrap();
        let held = LifecycleAuthority::resume(root.path(), "seat-15").unwrap();
        let peeked = LifecycleAuthority::peek(root.path(), "seat-15").unwrap();
        let lines = readiness_status_lines(root.path(), &peeked);
        assert!(
            lines
                .iter()
                .any(|line| line == "seat-15: offboard receipt completed"),
            "doctor/CLI readiness must name the durable receipt: {lines:?}"
        );
        assert!(
            LifecycleAuthority::resume(root.path(), "seat-15").is_err(),
            "receipt peek must not steal the authority lock"
        );
        held.finish().unwrap();
    }

    #[test]
    fn readiness_status_lines_name_the_staged_package() {
        let root = tempfile::tempdir().unwrap();
        let authority = LifecycleAuthority::begin(
            root.path(),
            LifecyclePlanV1 {
                schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
                request_id: "request-staged".into(),
                target_id: "seat-15".into(),
                intent: LifecycleIntentKind::Onboard,
                generation: 1,
                steps: vec!["packages".into(), "verify".into()],
            },
        )
        .unwrap();
        authority.finish().unwrap();
        let digest = "e262f1de2c38fd96cb1a8a8410f58222f0e0b5681b84217b877e78c114eb9a31";
        let dir = root.path().join("lifecycle").join("seat-15");
        std::fs::write(dir.join("staged-artifact"), b"rpm-bytes").unwrap();
        std::fs::write(dir.join("staged-artifact.digest"), format!("{digest}\n")).unwrap();
        std::fs::write(dir.join("staged-artifact.shape"), "rpm\n").unwrap();
        let held = LifecycleAuthority::resume(root.path(), "seat-15").unwrap();
        let peeked = LifecycleAuthority::peek(root.path(), "seat-15").unwrap();
        let lines = readiness_status_lines(root.path(), &peeked);
        assert!(
            lines.iter().any(|line| {
                line == "seat-15: packages staged:e262f1de2c38fd96cb1a8a8410f58222f0e0b5681b84217b877e78c114eb9a31:rpm (not installed)"
            }),
            "doctor/CLI readiness must name the staged pin: {lines:?}"
        );
        assert!(
            LifecycleAuthority::resume(root.path(), "seat-15").is_err(),
            "staged-package peek must not steal the authority lock"
        );
        held.finish().unwrap();
    }

    #[test]
    fn readiness_status_lines_name_the_staged_capsule() {
        let root = tempfile::tempdir().unwrap();
        let mut authority = LifecycleAuthority::begin(
            root.path(),
            LifecyclePlanV1 {
                schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
                request_id: "request-capsule".into(),
                target_id: "seat-15".into(),
                intent: LifecycleIntentKind::Onboard,
                generation: 1,
                steps: vec!["identity".into(), "verify".into()],
            },
        )
        .unwrap();
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let capsule = CommissioningCapsuleV1 {
            schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
            capsule_id: "cap-doctor".into(),
            target_id: "seat-15".into(),
            expires_at_ms: 2_000,
            bootstrap_digest_hex: "ab".repeat(32),
            one_time: true,
            key_id: "commissioning-v1".into(),
            signature_hex: String::new(),
        }
        .sign("commissioning-v1", &signing);
        authority
            .admit_commissioning_capsule(capsule, 1_000, &signing.verifying_key())
            .unwrap();
        authority.finish().unwrap();
        let held = LifecycleAuthority::resume(root.path(), "seat-15").unwrap();
        let peeked = LifecycleAuthority::peek(root.path(), "seat-15").unwrap();
        let lines = readiness_status_lines(root.path(), &peeked);
        assert!(
            lines
                .iter()
                .any(|line| line == "seat-15: capsule cap-doctor staged (not confirmed)"),
            "doctor/CLI readiness must name the staged capsule: {lines:?}"
        );
        assert!(
            LifecycleAuthority::resume(root.path(), "seat-15").is_err(),
            "staged-capsule peek must not steal the authority lock"
        );
        held.finish().unwrap();
    }

    #[test]
    fn readiness_status_lines_drop_the_capsule_after_confirm() {
        let root = tempfile::tempdir().unwrap();
        let mut authority = LifecycleAuthority::begin(
            root.path(),
            LifecyclePlanV1 {
                schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
                request_id: "request-capsule-confirm".into(),
                target_id: "seat-15".into(),
                intent: LifecycleIntentKind::Onboard,
                generation: 1,
                steps: vec!["identity".into(), "verify".into()],
            },
        )
        .unwrap();
        let signing = SigningKey::from_bytes(&[8u8; 32]);
        let capsule = CommissioningCapsuleV1 {
            schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
            capsule_id: "cap-gone".into(),
            target_id: "seat-15".into(),
            expires_at_ms: 2_000,
            bootstrap_digest_hex: "cd".repeat(32),
            one_time: true,
            key_id: "commissioning-v1".into(),
            signature_hex: String::new(),
        }
        .sign("commissioning-v1", &signing);
        authority
            .admit_commissioning_capsule(capsule, 1_000, &signing.verifying_key())
            .unwrap();
        authority.confirm_commissioning_capsule("cap-gone").unwrap();
        authority.finish().unwrap();
        let held = LifecycleAuthority::resume(root.path(), "seat-15").unwrap();
        let peeked = LifecycleAuthority::peek(root.path(), "seat-15").unwrap();
        let lines = readiness_status_lines(root.path(), &peeked);
        assert!(
            lines.iter().all(|line| !line.contains("capsule cap-gone")),
            "confirm must erase the staged capsule line: {lines:?}"
        );
        assert!(
            peek_staged_capsule_id(root.path(), "seat-15").is_none(),
            "confirmed capsule bytes must not remain staged"
        );
        held.finish().unwrap();
    }

    #[test]
    fn doctor_fleet_lifecycle_lines_name_the_staged_package_on_a_single_seat() {
        let root = tempfile::tempdir().unwrap();
        let authority = LifecycleAuthority::begin(
            root.path(),
            LifecyclePlanV1 {
                schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
                request_id: "doctor-staged".into(),
                target_id: "seat-15".into(),
                intent: LifecycleIntentKind::Onboard,
                generation: 1,
                steps: vec!["packages".into(), "verify".into()],
            },
        )
        .unwrap();
        authority.finish().unwrap();
        let digest = "e262f1de2c38fd96cb1a8a8410f58222f0e0b5681b84217b877e78c114eb9a31";
        let dir = root.path().join("lifecycle").join("seat-15");
        std::fs::write(dir.join("staged-artifact"), b"rpm-bytes").unwrap();
        std::fs::write(dir.join("staged-artifact.digest"), format!("{digest}\n")).unwrap();
        std::fs::write(dir.join("staged-artifact.shape"), "rpm\n").unwrap();
        let markers = root.path().join("markers");
        std::fs::create_dir_all(&markers).unwrap();
        let held = LifecycleAuthority::resume(root.path(), "seat-15").unwrap();
        let (ok, lines) = doctor_fleet_lifecycle_lines(&markers, root.path());
        assert!(!ok);
        assert!(
            lines.iter().any(|line| {
                line == "seat-15: packages staged:e262f1de2c38fd96cb1a8a8410f58222f0e0b5681b84217b877e78c114eb9a31:rpm (not installed)"
            }),
            "single-seat doctor must name the staged pin: {lines:?}"
        );
        assert!(
            doctor_check_detail(&lines).contains("not installed"),
            "meshctl doctor must print the staged pin"
        );
        held.finish().unwrap();
    }

    #[test]
    fn doctor_fleet_lifecycle_lines_name_the_receipt_on_a_single_seat() {
        let root = tempfile::tempdir().unwrap();
        let authority = LifecycleAuthority::begin(
            root.path(),
            LifecyclePlanV1 {
                schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
                request_id: "doctor-receipt".into(),
                target_id: "seat-15".into(),
                intent: LifecycleIntentKind::Offboard,
                generation: 1,
                steps: vec!["offboard".into(), "verify".into()],
            },
        )
        .unwrap();
        authority.finish().unwrap();
        let receipt = mackes_mesh_types::lifecycle::OffboardingReceiptV1 {
            schema_version: 1,
            request_id: "doctor-receipt".into(),
            target_id: "seat-15".into(),
            generation: 1,
            completed: true,
            retained_resources: Vec::new(),
            signature_hex: String::new(),
        };
        std::fs::write(
            root.path()
                .join("lifecycle")
                .join("seat-15")
                .join("receipt.json"),
            serde_json::to_vec_pretty(&receipt).unwrap(),
        )
        .unwrap();
        let markers = root.path().join("markers");
        std::fs::create_dir_all(&markers).unwrap();
        let held = LifecycleAuthority::resume(root.path(), "seat-15").unwrap();
        let (ok, lines) = doctor_fleet_lifecycle_lines(&markers, root.path());
        assert!(!ok);
        assert!(
            lines
                .iter()
                .any(|line| line == "seat-15: offboard receipt completed"),
            "single-seat doctor must name the durable receipt: {lines:?}"
        );
        assert!(
            doctor_check_detail(&lines).contains("offboard receipt completed"),
            "meshctl doctor must print the durable receipt"
        );
        held.finish().unwrap();
    }

    #[test]
    fn doctor_fleet_lifecycle_lines_name_every_durable_seat() {
        let root = tempfile::tempdir().unwrap();
        let vac_plan = |target: &str| LifecyclePlanV1 {
            schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
            request_id: "fleet-doctor".into(),
            target_id: target.into(),
            intent: LifecycleIntentKind::VerifyAndCorrect,
            generation: 1,
            steps: vec!["verify".into(), "configuration".into(), "verify".into()],
        };
        let mut first = LifecycleAuthority::begin(root.path(), vac_plan("seat-15")).unwrap();
        let mut second = LifecycleAuthority::begin(root.path(), vac_plan("seat-16")).unwrap();
        for (authority, target) in [(&mut first, "seat-15"), (&mut second, "seat-16")] {
            authority
                .record_check(LifecycleRequirementCheckV1 {
                    schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
                    check_id: "mesh".into(),
                    target_id: target.into(),
                    expected: "joined".into(),
                    observed: "absent".into(),
                    status: LifecycleCheckStatus::Fail,
                    required: true,
                    evidence_digest_hex: "c".repeat(64),
                    warning: None,
                    generation: 1,
                })
                .unwrap();
            assert!(authority
                .run_next(|_| Err("provider timeout".into()))
                .is_err());
        }
        let mut authorities = [first, second];
        crate::lifecycle_authority::execute_fleet_handoff(&mut authorities, "coord-a", "coord-b")
            .unwrap();
        for authority in authorities {
            authority.finish().unwrap();
        }
        let fleet = firstboot_fleet_status_lines(root.path());
        assert!(
            fleet.iter().any(|line| line == "coordinator coord-b"),
            "report-only must name the durable coordinator without markers: {fleet:?}"
        );
        apply_markers(&root.path().join("lifecycle"), false).unwrap();
        let (ok, lines) = doctor_fleet_lifecycle_lines(&root.path().join("lifecycle"), root.path());
        assert_eq!(lines, fleet, "doctor must reuse the peek-only fleet lines");
        assert!(!ok);
        assert!(
            lines.iter().any(|line| line == "coordinator coord-b"),
            "doctor must name the durable coordinator: {lines:?}"
        );
        assert!(
            lines.iter().any(|line| line == "fleet seat-15, seat-16"),
            "doctor must list every durable fleet seat: {lines:?}"
        );
        let peeked = LifecycleAuthority::peek(root.path(), "seat-15").unwrap();
        let readiness = readiness_status_lines(root.path(), &peeked);
        assert_eq!(
            readiness, lines,
            "lifecycle-readiness on one seat must name the whole fleet"
        );
        assert!(
            lines
                .iter()
                .any(|line| line == "seat-15: last error: provider timeout"),
            "doctor must not hide seat-15: {lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|line| line == "seat-16: last error: provider timeout"),
            "doctor must not hide seat-16: {lines:?}"
        );
    }

    #[test]
    fn doctor_and_readiness_keep_the_coordinator_after_a_wiped_sibling() {
        let root = tempfile::tempdir().unwrap();
        let plan = |target: &str| LifecyclePlanV1 {
            schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
            request_id: "fleet-wipe".into(),
            target_id: target.into(),
            intent: LifecycleIntentKind::Offboard,
            generation: 1,
            steps: vec!["offboard".into(), "verify".into()],
        };
        let first = LifecycleAuthority::begin(root.path(), plan("seat-15")).unwrap();
        let second = LifecycleAuthority::begin(root.path(), plan("seat-16")).unwrap();
        let mut authorities = [first, second];
        crate::lifecycle_authority::execute_fleet_handoff(&mut authorities, "coord-a", "coord-b")
            .unwrap();
        for authority in authorities {
            authority.finish().unwrap();
        }
        std::fs::remove_dir_all(root.path().join("lifecycle").join("seat-16")).unwrap();
        let fleet = firstboot_fleet_status_lines(root.path());
        assert!(
            fleet.iter().any(|line| line == "coordinator coord-b"),
            "a wiped sibling cannot hide the durable coordinator: {fleet:?}"
        );
        assert!(
            !fleet.iter().any(|line| line.contains("seat-16")),
            "a wiped sibling cannot remain on the fleet line: {fleet:?}"
        );
        apply_markers(&root.path().join("lifecycle"), false).unwrap();
        let (ok, lines) = doctor_fleet_lifecycle_lines(&root.path().join("lifecycle"), root.path());
        assert!(!ok);
        assert_eq!(lines, fleet);
        let peeked = LifecycleAuthority::peek(root.path(), "seat-15").unwrap();
        assert_eq!(readiness_status_lines(root.path(), &peeked), fleet);
    }

    #[test]
    fn doctor_fleet_lines_lead_with_a_pending_convergence_symlink() {
        let root = tempfile::tempdir().unwrap();
        let vac_plan = |target: &str| LifecyclePlanV1 {
            schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
            request_id: "fleet-symlink".into(),
            target_id: target.into(),
            intent: LifecycleIntentKind::VerifyAndCorrect,
            generation: 1,
            steps: vec!["verify".into(), "configuration".into(), "verify".into()],
        };
        let first = LifecycleAuthority::begin(root.path(), vac_plan("seat-15")).unwrap();
        let second = LifecycleAuthority::begin(root.path(), vac_plan("seat-16")).unwrap();
        first.finish().unwrap();
        second.finish().unwrap();
        let dest = root.path().join("dest-apply");
        std::fs::write(&dest, b"keep").unwrap();
        let markers = root.path().join("lifecycle");
        std::os::unix::fs::symlink(&dest, markers.join(FIRSTBOOT_PENDING)).unwrap();
        let (ok, lines) = doctor_fleet_lifecycle_lines(&markers, root.path());
        assert!(!ok);
        assert_eq!(
            lines.first().map(String::as_str),
            Some("first-boot marker must not be a symlink; dest repair is not implied")
        );
        assert!(
            lines.iter().any(|line| line == "fleet seat-15, seat-16"),
            "a planted marker must not hide the durable fleet: {lines:?}"
        );
        assert!(
            doctor_check_detail(&lines)
                .starts_with("first-boot marker must not be a symlink; dest repair is not implied"),
            "meshctl doctor must lead with the planted-marker refuse"
        );
        let peeked = LifecycleAuthority::peek(root.path(), "seat-15").unwrap();
        let readiness = readiness_status_lines(root.path(), &peeked);
        assert_eq!(
            readiness.first().map(String::as_str),
            Some("first-boot marker must not be a symlink; dest repair is not implied"),
            "lifecycle-readiness must not hide a planted marker: {readiness:?}"
        );
        let targets = crate::lifecycle_authority::peek_matching_fleet_targets(
            root.path(),
            "fleet-symlink",
            1,
        )
        .unwrap();
        let (report, checkpoints) =
            crate::lifecycle_authority::peek_fleet_session(root.path(), &targets).unwrap();
        assert_eq!(
            fleet_status_lines(root.path(), &report, &checkpoints)
                .first()
                .map(String::as_str),
            Some("first-boot marker must not be a symlink; dest repair is not implied"),
            "lifecycle-fleet-status must not hide a planted marker"
        );
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            b"keep",
            "doctor must not follow a fleet marker into dest"
        );
    }

    #[test]
    fn doctor_fleet_lines_lead_with_a_firstboot_converged_symlink() {
        let root = tempfile::tempdir().unwrap();
        let vac_plan = |target: &str| LifecyclePlanV1 {
            schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
            request_id: "fleet-converged-symlink".into(),
            target_id: target.into(),
            intent: LifecycleIntentKind::VerifyAndCorrect,
            generation: 1,
            steps: vec!["verify".into(), "configuration".into(), "verify".into()],
        };
        let first = LifecycleAuthority::begin(root.path(), vac_plan("seat-15")).unwrap();
        let second = LifecycleAuthority::begin(root.path(), vac_plan("seat-16")).unwrap();
        first.finish().unwrap();
        second.finish().unwrap();
        let dest = root.path().join("dest-ready");
        std::fs::write(&dest, b"keep").unwrap();
        let markers = root.path().join("lifecycle");
        std::os::unix::fs::symlink(&dest, markers.join(FIRSTBOOT_CONVERGED)).unwrap();
        let (ok, lines) = doctor_fleet_lifecycle_lines(&markers, root.path());
        assert!(!ok);
        assert_eq!(
            lines.first().map(String::as_str),
            Some("first-boot marker must not be a symlink; dest repair is not implied")
        );
        assert!(
            lines.iter().any(|line| line == "fleet seat-15, seat-16"),
            "a planted converged marker must not hide the durable fleet: {lines:?}"
        );
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            b"keep",
            "doctor must not follow a fleet converged marker into dest"
        );
    }

    #[test]
    fn firstboot_resume_keeps_pending_capsules() {
        let root = tempfile::tempdir().unwrap();
        let plan = LifecyclePlanV1 {
            schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
            request_id: "firstboot-1".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::Onboard,
            generation: 1,
            steps: vec![LifecycleStepKind::Verify.as_str().to_owned()],
        };
        let mut authority = LifecycleAuthority::begin(root.path(), plan).unwrap();
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let capsule = CommissioningCapsuleV1 {
            schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
            capsule_id: "cap-1".into(),
            target_id: "seat-15".into(),
            expires_at_ms: 2_000,
            bootstrap_digest_hex: "ab".repeat(32),
            one_time: true,
            key_id: "commissioning-v1".into(),
            signature_hex: String::new(),
        }
        .sign("commissioning-v1", &signing);
        authority
            .admit_commissioning_capsule(capsule, 1_000, &signing.verifying_key())
            .unwrap();
        let pending_before = authority.checkpoint().pending_capsule_ids.clone();
        let facts = healthy("seat-15");
        let mut blocked = facts.clone();
        blocked.package_present = false;
        blocked.package_identity = "missing".into();
        let readiness = record_on_authority(&mut authority, assemble(&blocked)).unwrap();
        assert!(!readiness.ready);
        assert_eq!(authority.checkpoint().pending_capsule_ids, pending_before);
        authority.finish().unwrap();
    }

    #[test]
    fn firstboot_verify_and_correct_walks_canonical_check_ids() {
        let root = tempfile::tempdir().unwrap();
        let intent = mackes_mesh_types::lifecycle::LifecycleIntentV1 {
            schema_version: LIFECYCLE_CONTRACT_SCHEMA_VERSION,
            request_id: "firstboot-vac".into(),
            target_id: "seat-15".into(),
            intent: LifecycleIntentKind::VerifyAndCorrect,
            generation: 1,
        };
        let steps = intent.default_steps();
        let plan = LifecyclePlanV1 {
            schema_version: intent.schema_version,
            request_id: intent.request_id,
            target_id: intent.target_id,
            intent: intent.intent,
            generation: intent.generation,
            steps,
        };
        let mut authority = LifecycleAuthority::begin(root.path(), plan).unwrap();
        let mut facts = healthy("seat-15");
        facts.active_units.retain(|unit| unit != "mackesd.service");
        record_on_authority(&mut authority, assemble(&facts)).unwrap();
        let preview = preview_correction_line(&authority).expect("report-only names the action");
        assert!(
            authority.checkpoint().correction_plan.is_none(),
            "preview must not persist a correction plan"
        );
        apply_markers(&root.path().join("lifecycle"), false).unwrap();
        let (ok_before, detail_before) =
            doctor_lifecycle_detail(&root.path().join("lifecycle"), root.path());
        assert!(!ok_before);
        assert_eq!(
            detail_before, preview,
            "doctor must name the previewed action before the walker persists a plan"
        );
        authority.run_declared_until_blocked(None).unwrap();
        assert!(
            root.path()
                .join("lifecycle")
                .join(FIRSTBOOT_PENDING)
                .exists(),
            "first-boot VAC must queue pending-convergence from units via configuration"
        );
        assert!(
            authority.checkpoint().progress.completed_steps >= 3,
            "walker must consume verify/configuration/mesh before the final gate"
        );
        assert!(
            authority.checkpoint().correction_plan.is_some(),
            "VAC walker must persist the proposed correction DAG before walking"
        );
        let line = next_correction_line(&authority).expect("VAC names one exact action");
        assert_eq!(
            line, preview,
            "walker must persist the same previewed action"
        );
        let (ok, detail) = doctor_lifecycle_detail(&root.path().join("lifecycle"), root.path());
        assert!(!ok);
        assert_eq!(
            detail, line,
            "doctor must name the same persisted VAC action"
        );
        authority.finish().unwrap();
    }

    #[test]
    fn failed_invite_enrollment_cannot_ignore_unit_fail_or_burn_token() {
        use crate::onboard::invite::{self, EnrollEndpoint};
        use std::time::Duration;

        let tmp = tempfile::tempdir().unwrap();
        let workgroup = tmp.path().join("workgroup");
        let markers = tmp.path().join("markers");
        std::fs::create_dir_all(&workgroup).unwrap();

        let issued = invite::issue(&workgroup, "home-mesh", Duration::from_secs(600)).unwrap();
        assert_eq!(invite::count_pending(&workgroup), 1);
        let token = invite::redeem_once(
            &workgroup,
            &issued.code,
            issued.invite.exp_ms - 1,
            "home-mesh",
            &EnrollEndpoint {
                lighthouse: "10.0.0.5".into(),
                port: 4242,
                fp: None,
            },
        )
        .expect("consume before the failed activation");
        assert_eq!(token.mesh_id, "home-mesh");
        assert!(
            !invite::is_recorded(&workgroup, &issued.code),
            "redeem_once consumed the bearer before transport"
        );

        let mut facts = healthy("seat-15");
        facts.active_units.retain(|unit| unit != "mackesd.service");
        facts.mesh_identity_present = false;
        facts.pending_enrollment_tokens = invite::count_pending(&workgroup);
        let checks = assemble(&facts);
        assert!(
            checks
                .iter()
                .any(|check| check.check_id == "units" && check.blocks_progress()),
            "inactive mackesd.service is a critical activation failure"
        );
        assert!(
            apply_markers(&markers, true).unwrap() == FirstbootMarker::Converged,
            "sanity: the raw marker helper still accepts a hostile ready=true"
        );
        assert_eq!(
            apply_markers_from_checks(&markers, &checks).unwrap(),
            FirstbootMarker::Pending,
            "baseline checks, not a caller bool, decide the marker"
        );

        assert_eq!(
            queue_after_failed_enrollment(&markers, &workgroup, &issued.code).unwrap(),
            FirstbootMarker::Pending
        );
        assert!(
            invite::is_recorded(&workgroup, &issued.code),
            "failed enrollment must re-record the consumed invite"
        );
        assert_eq!(invite::count_pending(&workgroup), 1);
        assert!(!markers.join(FIRSTBOOT_CONVERGED).exists());
        assert!(markers.join(FIRSTBOOT_PENDING).exists());
        assert_eq!(
            std::fs::read(markers.join(FIRSTBOOT_PENDING)).unwrap(),
            b"queued\n"
        );
    }

    #[test]
    fn runtime_expected_units_never_require_the_firstboot_oneshot() {
        for role in [Role::Lighthouse, Role::Workstation] {
            for grouped in [false, true] {
                let units = runtime_expected_units(role, grouped);
                assert!(
                    !units.iter().any(|unit| unit == FIRSTBOOT_SELF_UNIT),
                    "{role:?} grouped={grouped} must not require the activating oneshot"
                );
                assert!(
                    !units.iter().any(|unit| is_open_onboarding_unit(unit)),
                    "{role:?} grouped={grouped} must not require dest-gated collab-identity"
                );
            }
        }
        let workstation_grouped = runtime_expected_units(Role::Workstation, true);
        assert!(
            !workstation_grouped.iter().any(|unit| is_timer_unit(unit)),
            "workstation grouped first-boot must drop timer leaks: {workstation_grouped:?}"
        );
        assert!(!workstation_grouped
            .iter()
            .any(|unit| unit == "mackesd.service"));
        assert!(!workstation_grouped
            .iter()
            .any(|unit| unit == "etcd.service"));
    }

    #[test]
    fn runtime_expected_units_use_grouped_plane_and_drop_workstation_etcd() {
        let lighthouse = runtime_expected_units(Role::Lighthouse, false);
        assert!(lighthouse.iter().any(|unit| unit == "mackesd.service"));
        assert!(lighthouse.iter().any(|unit| unit == "etcd.service"));
        assert!(!lighthouse
            .iter()
            .any(|unit| unit == "mackesd-control.service"));

        let grouped_lh = runtime_expected_units(Role::Lighthouse, true);
        assert!(!grouped_lh.iter().any(|unit| unit == "mackesd.service"));
        assert!(grouped_lh.iter().any(|unit| unit == "etcd.service"));
        assert!(
            grouped_lh.iter().any(|unit| unit == "mesh-health.timer"),
            "lighthouse grouped plane still enable-masks timers; first-boot only drops them on workstation grouped"
        );
        for unit in GROUPED_MACKESD_UNITS {
            assert!(
                grouped_lh.iter().any(|active| active == unit),
                "grouped lighthouse first-boot must require {unit}"
            );
        }

        let workstation = runtime_expected_units(Role::Workstation, true);
        assert!(!workstation.iter().any(|unit| unit == "mackesd.service"));
        assert!(!workstation.iter().any(|unit| unit == "etcd.service"));
        assert!(
            !workstation.iter().any(|unit| is_timer_unit(unit)),
            "workstation grouped plane must drop timer units leaked from units_for_role: {workstation:?}"
        );
        assert!(
            !workstation
                .iter()
                .any(|unit| is_open_onboarding_unit(unit)),
            "workstation grouped plane must not require dest-gated collab-identity: {workstation:?}"
        );
        for unit in GROUPED_MACKESD_UNITS {
            assert!(
                workstation.iter().any(|active| active == unit),
                "grouped workstation first-boot must require {unit}"
            );
        }
        assert!(workstation.iter().any(|unit| unit == "nebula.service"));
    }

    #[test]
    fn grouped_mackesd_plane_can_produce_ready_without_monolithic_unit() {
        let mut facts = healthy("seat-15");
        facts.expected_units = runtime_expected_units(Role::Workstation, true);
        facts.active_units = facts.expected_units.clone();
        facts.ui_applicable = true;
        facts.ui_ready = true;
        let checks = assemble(&facts);
        assert!(
            checks.iter().all(|check| !check.blocks_progress()),
            "active grouped plane must not block first-boot: {checks:?}"
        );
    }

    #[test]
    fn firstboot_grouped_workstation_units_pass_when_collab_identity_is_inactive() {
        let mut facts = healthy("seat-15");
        facts.expected_units = runtime_expected_units(Role::Workstation, true);
        assert!(
            !facts
                .expected_units
                .iter()
                .any(|unit| unit == "mackesd.service" || unit == "etcd.service"),
            "workstation grouped expected units leaked monolithic/etcd: {:?}",
            facts.expected_units
        );
        assert!(
            !facts.expected_units.iter().any(|unit| is_timer_unit(unit)),
            "workstation grouped expected units leaked timers: {:?}",
            facts.expected_units
        );
        for unit in GROUPED_MACKESD_UNITS {
            assert!(
                facts.expected_units.iter().any(|expected| expected == unit),
                "expected units must include grouped {unit}"
            );
        }
        assert!(facts
            .expected_units
            .iter()
            .any(|unit| unit == "nebula.service"));

        // Hostile leak: dest-gated collab-identity appears in expected_units
        // as if units_for_role started shipping it. assemble must still pass
        // the units row while the unit stays inactive/failed.
        facts.expected_units.push(COLLAB_IDENTITY_UNIT.to_owned());
        facts.active_units = facts
            .expected_units
            .iter()
            .filter(|unit| !is_open_onboarding_unit(unit))
            .cloned()
            .collect();
        assert!(facts
            .active_units
            .iter()
            .any(|unit| unit == "nebula.service"));
        for unit in GROUPED_MACKESD_UNITS {
            assert!(
                facts.active_units.iter().any(|active| active == unit),
                "hostile seat must have grouped {unit} active"
            );
        }
        assert!(!facts
            .active_units
            .iter()
            .any(|unit| unit == "mackesd.service"));
        assert!(!facts.active_units.iter().any(|unit| unit == "etcd.service"));
        assert!(!facts
            .active_units
            .iter()
            .any(|unit| is_open_onboarding_unit(unit)));

        facts.ui_applicable = true;
        facts.ui_ready = false;
        let checks = assemble(&facts);
        let units = checks
            .iter()
            .find(|check| check.check_id == "units")
            .expect("units row");
        assert_eq!(
            units.status,
            LifecycleCheckStatus::Pass,
            "grouped plane with inactive collab-identity must not fail units: {}",
            units.observed
        );
        assert!(
            !units.blocks_progress(),
            "units must not block first-boot when only dest-gated collab-identity is inactive"
        );
        assert!(
            checks
                .iter()
                .any(|check| check.check_id == "verification" && check.blocks_progress()),
            "verification may still fail for other missing core facts (ui)"
        );
    }

    #[test]
    fn mesh_identity_present_under_accepts_either_host_cert_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert!(
            !mesh_identity_present_under(root),
            "neither dest-cut nor legacy host.crt must report absent"
        );

        std::fs::write(root.join(LEGACY_NEBULA_HOST_CERT_REL), b"legacy").unwrap();
        assert!(
            mesh_identity_present_under(root),
            "legacy /etc/nebula/host.crt must count as enrolled"
        );

        std::fs::remove_file(root.join(LEGACY_NEBULA_HOST_CERT_REL)).unwrap();
        assert!(
            !mesh_identity_present_under(root),
            "removing the legacy cert must return to absent"
        );

        let current = root.join("identity/current");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::write(current.join("host.crt"), b"dest-cut").unwrap();
        assert!(
            mesh_identity_present_under(root),
            "dest-cut identity/current/host.crt must count as enrolled"
        );
        assert!(
            !root.join(LEGACY_NEBULA_HOST_CERT_REL).exists(),
            "identity/current-only case must not plant a legacy host.crt"
        );
    }

    #[test]
    fn unit_file_does_not_use_status_or_unconditional_touch() {
        let unit =
            include_str!("../../../../../packaging/systemd/mcnf-lifecycle-firstboot.service");
        assert!(unit.contains("mackesd onboard lifecycle-firstboot"));
        assert!(!unit.contains("mackesd status"));
        assert!(!unit.contains("touch /var/lib/mackesd/lifecycle/firstboot-converged"));
        let cargo = include_str!("../../Cargo.toml");
        assert!(
            cargo.contains("packaging/systemd/mcnf-lifecycle-firstboot.service"),
            "RPM must ship the first-boot unit"
        );
        assert!(
            cargo.contains("systemctl enable")
                && cargo.contains("mcnf-lifecycle-firstboot.service"),
            "post-install must enable the first-boot unit"
        );
        assert!(
            cargo.contains("packaging/systemd/mcnf-node-virt.service")
                && cargo.contains("install-helpers/install-mm-nopasswd.sh")
                && cargo.contains("install-helpers/prepare-node-virt.sh"),
            "RPM must ship the Eagle/T480 sudo+virt helpers"
        );
    }
}
