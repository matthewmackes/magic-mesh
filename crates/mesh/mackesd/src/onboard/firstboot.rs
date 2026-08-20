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
    canonical_lifecycle_baseline, LifecycleCheckStatus, LifecycleRequirementCheckV1,
    SeatReadinessV1, LIFECYCLE_CONTRACT_SCHEMA_VERSION,
};
use sha2::{Digest, Sha256};

use crate::lifecycle_authority::{LifecycleAuthority, LifecycleAuthorityError};
use crate::onboard::role_provision::units_for_role;
use mde_role::Role;

/// Marker written only after the canonical baseline has no blocking checks.
pub const FIRSTBOOT_CONVERGED: &str = "firstboot-converged";
/// Marker written when first-boot queued corrected-forward convergence.
pub const FIRSTBOOT_PENDING: &str = "pending-convergence";
/// Production marker directory. Tests inject another path.
pub const DEFAULT_MARKER_DIR: &str = "/var/lib/mackesd/lifecycle";

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
                if facts.package_present {
                    facts.package_identity.as_str()
                } else {
                    "missing"
                },
                if facts.package_present {
                    LifecycleCheckStatus::Pass
                } else {
                    LifecycleCheckStatus::Fail
                },
                true,
                None,
            ),
            "units" => {
                let missing: Vec<&str> = facts
                    .expected_units
                    .iter()
                    .filter(|unit| !facts.active_units.iter().any(|active| active == *unit))
                    .map(String::as_str)
                    .collect();
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
            "mesh_identity" => check(
                facts,
                "mesh_identity",
                "enrolled mesh identity",
                if facts.mesh_identity_present {
                    "present"
                } else {
                    "missing"
                },
                if facts.mesh_identity_present {
                    LifecycleCheckStatus::Pass
                } else {
                    LifecycleCheckStatus::Fail
                },
                true,
                None,
            ),
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
                (!facts.compute_usable).then_some("virtualization unavailable".to_owned()),
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
                (!facts.hardware_usable).then_some("hardware capability withdrawn".to_owned()),
            ),
            "verification" => {
                let units_missing = facts
                    .expected_units
                    .iter()
                    .any(|unit| !facts.active_units.iter().any(|active| active == unit));
                let others_block = !facts.package_present
                    || units_missing
                    || !facts.configuration_present
                    || !facts.mesh_identity_present
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
/// rows and warning-level for capability rows.
#[must_use]
pub fn gather_live(target_id: &str, generation: u64, role: Role) -> FirstbootFacts {
    let expected_units: Vec<String> = units_for_role(role)
        .into_iter()
        .map(str::to_owned)
        .collect();
    let active_units: Vec<String> = expected_units
        .iter()
        .filter(|unit| unit_is_active(unit))
        .cloned()
        .collect();
    let ui_applicable = role == Role::Workstation;
    FirstbootFacts {
        target_id: target_id.to_owned(),
        generation,
        package_present: Path::new("/usr/bin/mackesd").is_file(),
        package_identity: if Path::new("/usr/bin/mackesd").is_file() {
            "/usr/bin/mackesd".to_owned()
        } else {
            "missing".to_owned()
        },
        expected_units,
        active_units,
        configuration_present: Path::new("/var/lib/mde/role.toml").is_file(),
        mesh_identity_present: Path::new("/etc/nebula/host.crt").is_file(),
        compute_usable: Path::new("/dev/kvm").exists(),
        ui_applicable,
        ui_ready: !ui_applicable || unit_is_active("mde-shell-egui.service"),
        hardware_usable: Path::new("/dev/dri").exists(),
        pending_enrollment_tokens: 0,
    }
}

fn unit_is_active(unit: &str) -> bool {
    std::process::Command::new("systemctl")
        .args(["is-active", "--quiet", unit])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Production marker directory as a [`PathBuf`].
#[must_use]
pub fn default_marker_dir() -> PathBuf {
    PathBuf::from(DEFAULT_MARKER_DIR)
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
            compute_usable: true,
            ui_applicable: false,
            ui_ready: false,
            hardware_usable: true,
            pending_enrollment_tokens: 1,
        }
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
    }
}
