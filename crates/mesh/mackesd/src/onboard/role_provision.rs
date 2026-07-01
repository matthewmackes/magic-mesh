//! OW-2 — `mackesd onboard role-provision`: apply a deployment role's systemd
//! unit set.
//!
//! A node's role decides which top-level systemd units it should run. This verb
//! makes the on-disk enable/mask state match the role: **enable** every unit the
//! role runs and **mask** every unit it does not (so a lighthouse can never
//! accidentally start the Workstation-only voice/desktop units, even via a
//! dependency pull-in).
//!
//! The role→units set is derived from the same rank model
//! [`crate::worker_role`] tiers the in-process workers by, reusing
//! [`mde_role::Role::rank`]: a unit sits at the *minimum role rank* that runs it
//! (0 = every node's control/data plane; 1 = Workstation-only). The pure mapping
//! ([`plan`]) is what the unit tests pin; [`apply`] folds that plan through an
//! injectable [`UnitManager`] so the fold is testable without a live systemd.

use mde_role::Role;

/// What [`apply`] does to a unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UnitAction {
    /// The role runs this unit — ensure it is unmasked + boot-enabled.
    Enable,
    /// The role does not run this unit — mask it so nothing can start it.
    Mask,
}

/// One unit in the role plan: the unit, its rank floor, and the action for the
/// target role.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PlannedUnit {
    /// The systemd unit name (e.g. `nebula.service`).
    pub unit: &'static str,
    /// The minimum role rank that runs it (0 lighthouse · 1 workstation).
    pub min_rank: u8,
    /// Enable (role runs it) or Mask (role does not).
    pub action: UnitAction,
}

/// The role-gated **systemd unit** catalog — the top-level units the RPM ships,
/// tiered by the minimum deployment rank that runs each, mirroring
/// [`crate::worker_role`]'s worker census for the in-process workers.
///
/// * **Rank 0 (every node)** — the control/data plane: the Nebula overlay, the
///   `mackesd` daemon, the etcd + Syncthing substrate, and the health + status
///   timers. This is [`crate::site_yml::CONVERGE_SERVICES`] plus the status
///   timer (a unit test pins that superset relationship).
/// * **Rank 1 (Workstation only)** — the desktop adds: the voice stack
///   (kamailio/rtpengine, gated to the rank-1 `voice_config` worker's tier) and
///   the one-way Carbon desktop branding.
const ROLE_UNITS: &[(&str, u8)] = &[
    // ── Rank 0 — universal control/data plane (CONVERGE_SERVICES + status timer).
    ("nebula.service", 0),
    ("mackesd.service", 0),
    ("etcd.service", 0),
    ("syncthing.service", 0),
    ("mesh-health.timer", 0),
    ("mesh-status.timer", 0),
    // ── Rank 1 — Workstation-only: the voice stack + desktop branding.
    ("kamailio-mde.service", 1),
    ("rtpengine-mde.service", 1),
    ("magic-mesh-brand.service", 1),
];

/// The pure role→unit-actions mapping.
///
/// A unit is **enabled** when the role's rank meets its floor, else **masked**.
/// Deterministic + side-effect-free — this is the tested core; [`apply`] is the
/// shell that runs it.
#[must_use]
pub fn plan(role: Role) -> Vec<PlannedUnit> {
    ROLE_UNITS
        .iter()
        .map(|&(unit, min_rank)| PlannedUnit {
            unit,
            min_rank,
            action: if role.rank() >= min_rank {
                UnitAction::Enable
            } else {
                UnitAction::Mask
            },
        })
        .collect()
}

/// Injectable seam over the two systemd operations, so [`apply`] is testable
/// without a live systemd. Production wires [`SystemctlUnits`]; tests pass a fake.
///
/// Both operations are idempotent: `enable` on an already-enabled (and unmasked)
/// unit is a no-op, `mask` on an already-masked unit is a no-op — so re-running
/// `role-provision` for the same role changes nothing.
pub trait UnitManager {
    /// Ensure `unit` is unmasked and boot-enabled.
    ///
    /// # Errors
    /// A human-readable message when the operation fails.
    fn enable(&self, unit: &str) -> Result<(), String>;

    /// Ensure `unit` is masked (cannot be started).
    ///
    /// # Errors
    /// A human-readable message when the operation fails.
    fn mask(&self, unit: &str) -> Result<(), String>;
}

/// Production [`UnitManager`]: drives `systemctl`.
///
/// `enable` first unmasks (best-effort — so a lighthouse→workstation upgrade can
/// enable a unit the earlier lighthouse pass masked) then boot-enables; `mask`
/// masks. No `--now`: this sets boot-durable state, it does not start/stop
/// services mid-provision.
pub struct SystemctlUnits;

impl UnitManager for SystemctlUnits {
    fn enable(&self, unit: &str) -> Result<(), String> {
        // Best-effort unmask: a first-ever enable has nothing to unmask, and we
        // don't want that to look like a failure — so the result is ignored and
        // only the enable is load-bearing.
        let _ = systemctl(&["unmask", unit]);
        systemctl(&["enable", unit])
    }

    fn mask(&self, unit: &str) -> Result<(), String> {
        systemctl(&["mask", unit])
    }
}

/// Run `systemctl <args…>`; `Ok` on exit 0, else an error naming the command. A
/// missing `systemctl` (a dev box) surfaces as an error the caller records.
fn systemctl(args: &[&str]) -> Result<(), String> {
    let status = std::process::Command::new("systemctl")
        .args(args)
        .status()
        .map_err(|e| format!("spawn `systemctl {}`: {e}", args.join(" ")))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`systemctl {}` exited {status}", args.join(" ")))
    }
}

/// The result of applying one [`PlannedUnit`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct UnitOutcome {
    /// The unit acted on.
    pub unit: &'static str,
    /// The action taken.
    pub action: UnitAction,
    /// Whether the action succeeded.
    pub ok: bool,
    /// The failure message when `!ok`.
    pub error: Option<String>,
}

/// Apply a `plan` through `mgr`, recording each unit's outcome.
///
/// Best-effort: a failed unit is recorded and the rest still run (a partial
/// systemd state should not abort the whole provision). Idempotent when the
/// manager's ops are (the production [`SystemctlUnits`] is).
#[must_use]
pub fn apply(plan: &[PlannedUnit], mgr: &dyn UnitManager) -> Vec<UnitOutcome> {
    plan.iter()
        .map(|p| {
            let res = match p.action {
                UnitAction::Enable => mgr.enable(p.unit),
                UnitAction::Mask => mgr.mask(p.unit),
            };
            UnitOutcome {
                unit: p.unit,
                action: p.action,
                ok: res.is_ok(),
                error: res.err(),
            }
        })
        .collect()
}

/// Convenience: [`plan`] then [`apply`] against the live systemd, for the CLI
/// dispatcher + a front-end that wants the one-call provision.
#[must_use]
pub fn provision(role: Role) -> Vec<UnitOutcome> {
    apply(&plan(role), &SystemctlUnits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn action_for<'a>(plan: &'a [PlannedUnit], unit: &str) -> &'a PlannedUnit {
        plan.iter().find(|p| p.unit == unit).expect("unit in plan")
    }

    #[test]
    fn lighthouse_enables_control_plane_and_masks_workstation_units() {
        let p = plan(Role::Lighthouse);
        // Rank-0 control plane → enabled.
        for u in [
            "nebula.service",
            "mackesd.service",
            "etcd.service",
            "syncthing.service",
            "mesh-health.timer",
            "mesh-status.timer",
        ] {
            assert_eq!(
                action_for(&p, u).action,
                UnitAction::Enable,
                "lighthouse must enable {u}"
            );
        }
        // Rank-1 Workstation units → masked (a lighthouse never runs them).
        for u in [
            "kamailio-mde.service",
            "rtpengine-mde.service",
            "magic-mesh-brand.service",
        ] {
            assert_eq!(
                action_for(&p, u).action,
                UnitAction::Mask,
                "lighthouse must mask {u}"
            );
        }
    }

    #[test]
    fn workstation_enables_every_unit() {
        let p = plan(Role::Workstation);
        assert!(
            p.iter().all(|u| u.action == UnitAction::Enable),
            "workstation (top rank) runs the full unit set"
        );
        // Same catalog for both roles — only the actions differ.
        assert_eq!(p.len(), plan(Role::Lighthouse).len());
    }

    #[test]
    fn plan_is_deterministic() {
        assert_eq!(plan(Role::Lighthouse), plan(Role::Lighthouse));
        assert_eq!(plan(Role::Workstation), plan(Role::Workstation));
    }

    #[test]
    fn rank_zero_units_are_a_superset_of_converge_services() {
        // The role catalog's rank-0 tier must cover the canonical boot-durable
        // service set, so a provisioned node keeps CONVERGE_SERVICES enabled.
        let rank0: Vec<&str> = ROLE_UNITS
            .iter()
            .filter(|(_, r)| *r == 0)
            .map(|(u, _)| *u)
            .collect();
        for svc in crate::site_yml::CONVERGE_SERVICES {
            assert!(
                rank0.contains(&svc),
                "{svc} (CONVERGE_SERVICES) missing from the rank-0 role units"
            );
        }
    }

    /// Fake manager: records every call and always succeeds.
    struct Recorder {
        calls: RefCell<Vec<(String, String)>>,
    }
    impl Recorder {
        fn new() -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
            }
        }
        fn calls(&self) -> Vec<(String, String)> {
            self.calls.borrow().clone()
        }
    }
    impl UnitManager for Recorder {
        fn enable(&self, unit: &str) -> Result<(), String> {
            self.calls
                .borrow_mut()
                .push(("enable".to_string(), unit.to_string()));
            Ok(())
        }
        fn mask(&self, unit: &str) -> Result<(), String> {
            self.calls
                .borrow_mut()
                .push(("mask".to_string(), unit.to_string()));
            Ok(())
        }
    }

    #[test]
    fn apply_folds_plan_through_the_manager() {
        let rec = Recorder::new();
        let plan = plan(Role::Lighthouse);
        let outcomes = apply(&plan, &rec);
        // One outcome per planned unit, all ok.
        assert_eq!(outcomes.len(), plan.len());
        assert!(outcomes.iter().all(|o| o.ok && o.error.is_none()));
        // Every planned action reached the manager as the matching call.
        let calls = rec.calls();
        assert_eq!(calls.len(), plan.len());
        for pu in &plan {
            let verb = match pu.action {
                UnitAction::Enable => "enable",
                UnitAction::Mask => "mask",
            };
            assert!(
                calls.contains(&(verb.to_string(), pu.unit.to_string())),
                "expected {verb} {}",
                pu.unit
            );
        }
        // Lighthouse masks exactly the 3 Workstation units.
        assert_eq!(
            calls.iter().filter(|(v, _)| v == "mask").count(),
            3,
            "lighthouse masks the rank-1 voice + brand units"
        );
    }

    /// Fake manager that fails one specific unit — proves a partial failure is
    /// recorded without aborting the rest.
    struct FailOne(&'static str);
    impl UnitManager for FailOne {
        fn enable(&self, unit: &str) -> Result<(), String> {
            if unit == self.0 {
                Err("boom".to_string())
            } else {
                Ok(())
            }
        }
        fn mask(&self, _unit: &str) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn apply_records_a_partial_failure_and_continues() {
        let outcomes = apply(&plan(Role::Workstation), &FailOne("mackesd.service"));
        let failed: Vec<&UnitOutcome> = outcomes.iter().filter(|o| !o.ok).collect();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].unit, "mackesd.service");
        assert_eq!(failed[0].error.as_deref(), Some("boom"));
        // Every other unit still ran and succeeded.
        assert_eq!(outcomes.iter().filter(|o| o.ok).count(), outcomes.len() - 1);
    }
}
