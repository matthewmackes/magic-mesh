//! The render-agnostic onboarding **entry** state machine (ONBOARD-WIZARD OW-1).
//!
//! The role-chooser is the first-run *entry* to onboarding: an explicit four-step
//! gate the operator walks through before a single mesh action runs —
//!
//! 1. **Disclaimer** — acknowledge `mde_disclaimer::TEXT` before anything else is
//!    reachable (design lock §43).
//! 2. **Role** — pin Lighthouse or Workstation (the 2-role model, governance §5).
//! 3. **Intent** — **Create New Mesh** (a Workstation only — only the founding
//!    Workstation mints the CA, §5) or **Join Existing Mesh** (any role).
//! 4. **Confirm** — review, then hand the captured `{role, intent}` off to the
//!    (separate, future) wizard.
//!
//! This module is the logic with **no egui in it** — pure data + transitions, so
//! the whole gate (including the Workstation-only Create rule) is unit-tested
//! without a GPU. The egui surface in `main.rs` is a thin renderer over it; the
//! mesh-create / mesh-join work itself is the later wizard (OW-3 / OW-4).

use mde_role::Role;

/// What the operator wants to do with a mesh once their role is pinned.
///
/// [`Intent::as_str`] is the **stable slug** written into the hand-off file and
/// read back by the wizard — treat it as a wire contract (OW-3 / OW-4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    /// Found a brand-new mesh. **Workstation only** (§5): the founding box mints
    /// the CA + mesh identity.
    CreateNewMesh,
    /// Join an existing mesh via an invite (QR / short code). Valid for any role.
    JoinExistingMesh,
}

impl Intent {
    /// The stable lowercase slug recorded in the hand-off (`create` / `join`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CreateNewMesh => "create",
            Self::JoinExistingMesh => "join",
        }
    }
}

/// Which screen of the entry flow is live. The only legal progression is forward
/// `Disclaimer → Role → Intent → Confirm` (via [`Onboard::ack`] /
/// [`Onboard::choose_role`] / [`Onboard::choose_intent`]) plus one step back from
/// `Intent` / `Confirm` (via [`Onboard::back`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Step {
    /// Step 1 — the disclaimer acknowledgement gate.
    #[default]
    Disclaimer,
    /// Step 2 — pick a deployment role.
    Role,
    /// Step 3 — pick create-vs-join.
    Intent,
    /// Step 4 — review the captured role + intent and confirm.
    Confirm,
}

/// Why [`Onboard::choose_intent`] refused — the Workstation-only Create rule (§5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentError {
    /// `CreateNewMesh` was chosen on a non-Workstation role. Only a Workstation
    /// founds a mesh; a Lighthouse can only Join.
    CreateRequiresWorkstation,
}

impl std::fmt::Display for IntentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CreateRequiresWorkstation => f.write_str(
                "Only a Workstation can found a mesh — a Lighthouse joins an existing one.",
            ),
        }
    }
}

/// The captured hand-off: the picked role + intent, ready for the wizard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The role to pin.
    pub role: Role,
    /// What to do once pinned.
    pub intent: Intent,
}

impl Outcome {
    /// The hand-off record the wizard reads back: a tiny JSON object
    /// `{"role":"…","intent":"…"}`. Both values come from a fixed vocabulary
    /// ([`Role::as_str`] / [`Intent::as_str`]) — no quotes, backslashes, or
    /// control chars — so the hand-rolled encoding never needs escaping (and the
    /// crate stays serde-free, matching `mde-role` / `mde-disclaimer`).
    #[must_use]
    pub fn to_json(&self) -> String {
        format!(
            "{{\"role\":\"{}\",\"intent\":\"{}\"}}",
            self.role.as_str(),
            self.intent.as_str()
        )
    }
}

/// The onboarding entry state machine. Construct with [`Onboard::new`], drive it
/// with [`ack`](Self::ack) → [`choose_role`](Self::choose_role) →
/// [`choose_intent`](Self::choose_intent), then read [`outcome`](Self::outcome)
/// once it reaches [`Step::Confirm`].
#[derive(Debug, Clone, Default)]
pub struct Onboard {
    step: Step,
    acked: bool,
    role: Option<Role>,
    intent: Option<Intent>,
}

impl Onboard {
    /// A fresh machine parked on the disclaimer gate.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The live step.
    #[must_use]
    pub fn step(&self) -> Step {
        self.step
    }

    /// Whether the disclaimer has been acknowledged.
    #[must_use]
    pub fn acked(&self) -> bool {
        self.acked
    }

    /// The picked role, once chosen.
    #[must_use]
    pub fn role(&self) -> Option<Role> {
        self.role
    }

    /// The picked intent, once chosen.
    #[must_use]
    pub fn intent(&self) -> Option<Intent> {
        self.intent
    }

    /// **Step 1 → 2.** Acknowledge the disclaimer and advance to role selection.
    /// A no-op unless the machine is on [`Step::Disclaimer`], so nothing past the
    /// gate is reachable without an explicit ack (§43).
    pub fn ack(&mut self) {
        if self.step == Step::Disclaimer {
            self.acked = true;
            self.step = Step::Role;
        }
    }

    /// **Step 2 → 3.** Pick a role and advance to intent. A no-op off
    /// [`Step::Role`].
    pub fn choose_role(&mut self, role: Role) {
        if self.step == Step::Role {
            self.role = Some(role);
            self.step = Step::Intent;
        }
    }

    /// Whether **Create New Mesh** is allowed for the currently-picked role: only
    /// a [`Role::Workstation`] founds a mesh (§5). The UI disables + explains
    /// Create when this is `false`.
    #[must_use]
    pub fn can_create(&self) -> bool {
        self.role == Some(Role::Workstation)
    }

    /// **Step 3 → 4.** Pick create-vs-join and advance to confirm. Enforces the
    /// Workstation-only Create rule: `CreateNewMesh` on a non-Workstation is
    /// refused with [`IntentError::CreateRequiresWorkstation`] and the machine
    /// stays on [`Step::Intent`]. A no-op off [`Step::Intent`].
    ///
    /// # Errors
    /// [`IntentError::CreateRequiresWorkstation`] when `CreateNewMesh` is chosen
    /// on a non-Workstation role.
    pub fn choose_intent(&mut self, intent: Intent) -> Result<(), IntentError> {
        if self.step != Step::Intent {
            return Ok(());
        }
        if intent == Intent::CreateNewMesh && !self.can_create() {
            return Err(IntentError::CreateRequiresWorkstation);
        }
        self.intent = Some(intent);
        self.step = Step::Confirm;
        Ok(())
    }

    /// Step one screen back: `Confirm → Intent` (clears the intent) or
    /// `Intent → Role` (clears the role). A no-op on [`Step::Role`] /
    /// [`Step::Disclaimer`] — the acknowledged disclaimer gate is the floor.
    pub fn back(&mut self) {
        match self.step {
            Step::Confirm => {
                self.intent = None;
                self.step = Step::Intent;
            }
            Step::Intent => {
                self.role = None;
                self.step = Step::Role;
            }
            Step::Role | Step::Disclaimer => {}
        }
    }

    /// The captured hand-off, available only on [`Step::Confirm`] with both a role
    /// and an intent picked; `None` at every earlier step.
    #[must_use]
    pub fn outcome(&self) -> Option<Outcome> {
        match (self.step, self.role, self.intent) {
            (Step::Confirm, Some(role), Some(intent)) => Some(Outcome { role, intent }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_parked_on_the_disclaimer_gate() {
        let m = Onboard::new();
        assert_eq!(m.step(), Step::Disclaimer);
        assert!(!m.acked());
        assert_eq!(m.role(), None);
        assert_eq!(m.intent(), None);
        assert_eq!(m.outcome(), None);
    }

    #[test]
    fn nothing_past_the_gate_is_reachable_without_an_ack() {
        // Trying to pick a role before acking is a no-op: still on the disclaimer.
        let mut m = Onboard::new();
        m.choose_role(Role::Workstation);
        assert_eq!(m.step(), Step::Disclaimer);
        assert_eq!(m.role(), None);
        // Acking opens role selection.
        m.ack();
        assert!(m.acked());
        assert_eq!(m.step(), Step::Role);
    }

    #[test]
    fn workstation_create_path_captures_the_outcome() {
        let mut m = Onboard::new();
        m.ack();
        m.choose_role(Role::Workstation);
        assert_eq!(m.step(), Step::Intent);
        assert!(m.can_create());
        m.choose_intent(Intent::CreateNewMesh)
            .expect("a workstation may create");
        assert_eq!(m.step(), Step::Confirm);
        assert_eq!(
            m.outcome(),
            Some(Outcome {
                role: Role::Workstation,
                intent: Intent::CreateNewMesh,
            })
        );
    }

    #[test]
    fn lighthouse_join_path_captures_the_outcome() {
        let mut m = Onboard::new();
        m.ack();
        m.choose_role(Role::Lighthouse);
        assert!(!m.can_create());
        m.choose_intent(Intent::JoinExistingMesh)
            .expect("any role may join");
        assert_eq!(
            m.outcome(),
            Some(Outcome {
                role: Role::Lighthouse,
                intent: Intent::JoinExistingMesh,
            })
        );
    }

    #[test]
    fn lighthouse_cannot_create_a_mesh() {
        // §5: only a Workstation founds a mesh.
        let mut m = Onboard::new();
        m.ack();
        m.choose_role(Role::Lighthouse);
        let err = m
            .choose_intent(Intent::CreateNewMesh)
            .expect_err("a lighthouse may not create");
        assert_eq!(err, IntentError::CreateRequiresWorkstation);
        // Refused: still on Intent, no intent captured, no outcome.
        assert_eq!(m.step(), Step::Intent);
        assert_eq!(m.intent(), None);
        assert_eq!(m.outcome(), None);
        // Join is still open from here.
        m.choose_intent(Intent::JoinExistingMesh)
            .expect("join after a refused create");
        assert_eq!(m.step(), Step::Confirm);
    }

    #[test]
    fn outcome_is_none_until_confirm() {
        let mut m = Onboard::new();
        assert_eq!(m.outcome(), None);
        m.ack();
        assert_eq!(m.outcome(), None);
        m.choose_role(Role::Workstation);
        assert_eq!(m.outcome(), None);
        m.choose_intent(Intent::JoinExistingMesh).unwrap();
        assert!(m.outcome().is_some());
    }

    #[test]
    fn back_steps_and_clears_the_later_choice() {
        let mut m = Onboard::new();
        m.ack();
        m.choose_role(Role::Workstation);
        m.choose_intent(Intent::CreateNewMesh).unwrap();
        assert_eq!(m.step(), Step::Confirm);
        // Confirm → Intent clears the intent, keeps the role.
        m.back();
        assert_eq!(m.step(), Step::Intent);
        assert_eq!(m.intent(), None);
        assert_eq!(m.role(), Some(Role::Workstation));
        // Intent → Role clears the role.
        m.back();
        assert_eq!(m.step(), Step::Role);
        assert_eq!(m.role(), None);
        // Role is the floor for Back — the acknowledged disclaimer stands.
        m.back();
        assert_eq!(m.step(), Step::Role);
        assert!(m.acked());
    }

    #[test]
    fn intent_slugs_are_stable() {
        assert_eq!(Intent::CreateNewMesh.as_str(), "create");
        assert_eq!(Intent::JoinExistingMesh.as_str(), "join");
    }

    #[test]
    fn outcome_serialises_to_the_handoff_json() {
        let ws_create = Outcome {
            role: Role::Workstation,
            intent: Intent::CreateNewMesh,
        };
        assert_eq!(
            ws_create.to_json(),
            r#"{"role":"workstation","intent":"create"}"#
        );
        let lh_join = Outcome {
            role: Role::Lighthouse,
            intent: Intent::JoinExistingMesh,
        };
        assert_eq!(
            lh_join.to_json(),
            r#"{"role":"lighthouse","intent":"join"}"#
        );
    }
}
