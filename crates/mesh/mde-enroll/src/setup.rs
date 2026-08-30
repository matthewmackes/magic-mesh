//! SETUP-1 — the `magic-setup` full-lifecycle wizard state machine.
//!
//! Pure, I/O-free model (design: docs/design/magic-setup-wizard.md). The
//! crossterm event loop + ratatui render in `bin/magic-setup.rs` drive this;
//! the actual work (found/join/setup-etcd/setup-syncthing/systemctl) is shelled by the
//! action layer ([`crate::setup_action`]) — keeping the model terminal- and
//! subprocess-free makes the whole flow unit-testable.

//!
//! Lock 1 (one binary grown from `mde-enroll`): the Join screen reuses the
//! ONBOARD-5 enroll [`crate::app::App`]; `mde-enroll` stays the join-only shim.

use std::path::Path;

use crate::commissioning_view::{CapsuleView, JoinTokenView};
use crate::lifecycle_controller::LifecycleController;
use crate::lifecycle_view::LifecycleSessionView;
use mackesd_core::lifecycle_authority::{
    peek_fleet_session, peek_matching_fleet_targets, LifecycleAuthority,
};

/// Which top-level screen the wizard is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// First-run welcome + disclaimer gate (design §43). Shown once on an
    /// unconfigured node before the menu is reachable; a configured node skips
    /// straight to [`Screen::Menu`]. Acknowledging it ([`Wizard::acknowledge_welcome`])
    /// opens the menu.
    Welcome,
    /// The top menu (entries depend on configured-state).
    Menu,
    /// Create a new mesh — found LH1 (SETUP-2).
    Create,
    /// Join an existing mesh by lighthouse IP + token (SETUP-3).
    Join,
    /// Manage peers / add lighthouse (lighthouse only; SETUP-4/5).
    Manage,
    /// Status + services (SETUP-5).
    Status,
    /// Shared ONBOARD/OFFBOARD session projection (WL-FUNC-023 S4).
    /// Read-only; both TUI and GUI consume [`LifecycleSessionView`].
    Lifecycle,
}

/// A selectable top-menu entry. The set shown depends on whether the node
/// is already configured (a role is pinned).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuItem {
    /// Found a new mesh (unconfigured only).
    CreateMesh,
    /// Join an existing mesh (unconfigured only).
    JoinMesh,
    /// Manage peers / lighthouses (configured only).
    ManagePeers,
    /// Show mesh + service status (configured only).
    Status,
    Lifecycle,
    /// Leave the wizard.
    Quit,
}

impl MenuItem {
    /// Human label for the menu row.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            MenuItem::CreateMesh => "Create a new mesh",
            MenuItem::JoinMesh => "Join an existing mesh",
            MenuItem::ManagePeers => "Manage peers & lighthouses",
            MenuItem::Status => "Status & services",
            MenuItem::Lifecycle => "Lifecycle session",
            MenuItem::Quit => "Quit",
        }
    }

    /// A one-line, plain-language description shown under the menu label so a
    /// first-time operator can tell the entries apart at a glance.
    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            MenuItem::CreateMesh => {
                "Found a brand-new private mesh — this node mints the CA and becomes the founder."
            }
            MenuItem::JoinMesh => {
                "Enroll this node into an existing mesh with a join token from another node."
            }
            MenuItem::ManagePeers => {
                "Invite peers, add lighthouses, and remove nodes from the mesh."
            }
            MenuItem::Status => "Check the overlay, role daemons, and mesh services.",
            MenuItem::Lifecycle => "Review readiness, warnings, offboard, reset, and fleet plans.",
            MenuItem::Quit => "Leave the wizard.",
        }
    }

    /// The screen this entry opens (Quit has none).
    #[must_use]
    pub fn screen(self) -> Option<Screen> {
        match self {
            MenuItem::CreateMesh => Some(Screen::Create),
            MenuItem::JoinMesh => Some(Screen::Join),
            MenuItem::ManagePeers => Some(Screen::Manage),
            MenuItem::Status => Some(Screen::Status),
            MenuItem::Lifecycle => Some(Screen::Lifecycle),
            MenuItem::Quit => None,
        }
    }
}

/// The full wizard model.
#[derive(Debug, Clone)]
pub struct Wizard {
    /// True when a deployment role is already pinned (configured node).
    pub configured: bool,
    /// Current screen.
    pub screen: Screen,
    /// Menu entries for the current configured-state.
    pub menu_items: Vec<MenuItem>,
    /// Highlighted menu index.
    pub menu_index: usize,
    /// Verbose live-log pane (newest last).
    pub log: Vec<String>,
    /// Read-only authority projection rendered by lifecycle-aware clients.
    pub lifecycle_view: Option<LifecycleSessionView>,
    /// Peeked fleet controller. Absent for a single local seat.
    pub lifecycle_controller: Option<LifecycleController>,
    /// Read-only join-token projection (bearer withheld).
    pub token_view: Option<JoinTokenView>,
    /// Read-only commissioning-capsule projection (signature withheld).
    pub capsule_view: Option<CapsuleView>,
    /// Set when the operator chooses Quit.
    pub should_quit: bool,
}

impl Wizard {
    /// Build the wizard for a node, detecting configured-state.
    ///
    /// `configured` is whether a role is pinned (`mde_role::load().is_ok()`);
    /// the caller passes it so the model stays I/O-free. Unconfigured nodes
    /// see Create/Join; configured nodes see Manage/Status.
    ///
    /// A fresh (unconfigured) node opens on the [`Screen::Welcome`] disclaimer
    /// gate (§43); an already-configured node — which has necessarily passed the
    /// gate once — opens straight on the [`Screen::Menu`].
    #[must_use]
    pub fn new(configured: bool) -> Self {
        let menu_items = Self::menu_for(configured);
        let screen = if configured {
            Screen::Menu
        } else {
            Screen::Welcome
        };
        Self {
            configured,
            screen,
            menu_items,
            menu_index: 0,
            log: Vec::new(),
            lifecycle_view: None,
            lifecycle_controller: None,
            token_view: None,
            capsule_view: None,
            should_quit: false,
        }
    }

    /// The menu entries shown for a given configured-state.
    #[must_use]
    pub fn menu_for(configured: bool) -> Vec<MenuItem> {
        if configured {
            vec![
                MenuItem::ManagePeers,
                MenuItem::Status,
                MenuItem::Lifecycle,
                MenuItem::Quit,
            ]
        } else {
            vec![MenuItem::CreateMesh, MenuItem::JoinMesh, MenuItem::Quit]
        }
    }

    /// Move the menu highlight up (wraps).
    pub fn menu_up(&mut self) {
        if self.menu_items.is_empty() {
            return;
        }
        self.menu_index = if self.menu_index == 0 {
            self.menu_items.len() - 1
        } else {
            self.menu_index - 1
        };
    }

    /// Move the menu highlight down (wraps).
    pub fn menu_down(&mut self) {
        if self.menu_items.is_empty() {
            return;
        }
        self.menu_index = (self.menu_index + 1) % self.menu_items.len();
    }

    /// The currently-highlighted menu entry.
    #[must_use]
    pub fn selected(&self) -> MenuItem {
        self.menu_items
            .get(self.menu_index)
            .copied()
            .unwrap_or(MenuItem::Quit)
    }

    /// Activate the highlighted entry: open its screen, or quit.
    pub fn activate(&mut self) {
        match self.selected().screen() {
            Some(screen) => {
                self.screen = screen;
                self.push_log(format!("→ {}", self.selected().label()));
                if screen == Screen::Lifecycle {
                    for line in self.lifecycle_lines() {
                        self.push_log(line);
                    }
                }
            }
            None => self.should_quit = true,
        }
    }

    /// Acknowledge the first-run welcome + disclaimer (§43) and open the menu.
    /// A no-op off [`Screen::Welcome`], so nothing behind the gate is reachable
    /// without an explicit acknowledgement.
    pub fn acknowledge_welcome(&mut self) {
        if self.screen == Screen::Welcome {
            self.screen = Screen::Menu;
        }
    }

    /// Return from a sub-screen to the top menu.
    pub fn back_to_menu(&mut self) {
        self.screen = Screen::Menu;
    }

    /// Append a verbose log line (the live-log pane).
    pub fn push_log(&mut self, line: impl Into<String>) {
        self.log.push(line.into());
    }

    /// Attach the latest authority projection without giving the wizard any
    /// lifecycle mutation capability.
    pub fn set_lifecycle_view(&mut self, view: LifecycleSessionView) {
        self.lifecycle_view = Some(view);
    }

    pub fn set_lifecycle_controller(&mut self, controller: Option<LifecycleController>) {
        self.lifecycle_controller = controller;
    }

    /// Drop a stale projection so the screen cannot keep showing a session
    /// after the authority tree is gone.
    pub fn clear_lifecycle_view(&mut self) {
        self.lifecycle_view = None;
        self.lifecycle_controller = None;
    }

    /// Honest lifecycle lines for GUI/TUI consumers. Empty session is
    /// named, never implied ready. No mutation verbs.
    #[must_use]
    pub fn lifecycle_lines(&self) -> Vec<String> {
        match &self.lifecycle_view {
            Some(view) => {
                let mut lines = vec![view.status_line(), view.capability_summary()];
                if !view.missing_requirements.is_empty() {
                    lines.push(format!("missing: {}", view.missing_requirements.join(", ")));
                }
                if let Some(fleet) = view.fleet_line() {
                    lines.push(fleet);
                }
                if let Some(coordinator) = view.coordinator_line() {
                    lines.push(coordinator);
                }
                if let Some(nag) = view.onboard_nag_line() {
                    lines.push(nag.to_owned());
                }
                if let Some(correction) = view.correction_line() {
                    lines.push(correction.to_owned());
                }
                if let Some(error) = view.last_error_line() {
                    lines.push(error);
                }
                if let Some(receipt) = view.receipt_line() {
                    lines.push(receipt.to_owned());
                }
                if let Some(package) = view.package_line() {
                    lines.push(package);
                }
                if let Some(capsule) = view.capsule_line() {
                    lines.push(capsule);
                }
                lines.extend(view.confirmation_lines());
                lines
            }
            None => vec!["no lifecycle session published".to_owned()],
        }
    }

    /// Attach a renderer-safe join-token projection. The wizard never stores
    /// the bearer — only the withheld view.
    pub fn set_token_view(&mut self, view: JoinTokenView) {
        self.token_view = Some(view);
    }

    /// Parse a pasted or issued join-token wire form and attach the withheld
    /// view. The bearer never enters wizard state. Template, empty, or garbage
    /// input refuses and leaves any existing view unchanged.
    pub fn present_join_token(&mut self, raw: &str) -> Result<(), String> {
        let view = JoinTokenView::from_wire(raw)?;
        self.set_token_view(view);
        Ok(())
    }

    /// Attach a renderer-safe commissioning-capsule projection.
    pub fn set_capsule_view(&mut self, view: CapsuleView) {
        self.capsule_view = Some(view);
    }

    /// Parse an issued or staged commissioning capsule and attach the withheld
    /// view. The signature never enters wizard state. Expired, replayable, or
    /// unsigned-looking envelopes refuse and leave any existing view unchanged.
    pub fn present_capsule(&mut self, capsule_json: &str, now_ms: i64) -> Result<(), String> {
        let view = CapsuleView::from_wire(capsule_json, now_ms)?;
        self.set_capsule_view(view);
        Ok(())
    }

    /// Honest commissioning lines for GUI/TUI consumers. Empty when no
    /// token or capsule has been attached.
    #[must_use]
    pub fn commissioning_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(view) = &self.token_view {
            lines.push(view.status_line());
        }
        if let Some(view) = &self.capsule_view {
            lines.push(view.status_line());
        }
        lines
    }
}

fn with_durable_receipt(
    view: LifecycleSessionView,
    root: &Path,
    targets: &[String],
) -> LifecycleSessionView {
    for target in targets {
        if let Ok(Some(receipt)) = LifecycleAuthority::peek_offboarding_receipt(root, target) {
            return view.with_offboarding_receipt(Some(&receipt));
        }
    }
    view
}

fn with_staged_package(
    view: LifecycleSessionView,
    root: &Path,
    targets: &[String],
) -> LifecycleSessionView {
    for target in targets {
        if let Some(identity) = mackesd_core::onboard::firstboot::staged_package_identity(
            &root.join("lifecycle").join(target),
        ) {
            return view.with_staged_package(Some(&identity));
        }
    }
    view
}

fn with_staged_capsule(
    view: LifecycleSessionView,
    root: &Path,
    targets: &[String],
) -> LifecycleSessionView {
    for target in targets {
        if let Some(capsule_id) =
            mackesd_core::onboard::firstboot::peek_staged_capsule_id(root, target)
        {
            return view.with_staged_capsule(Some(&capsule_id));
        }
    }
    view
}

/// Hydrate the wizard from `root/lifecycle/*/checkpoint.json` without
/// taking the exclusive authority lock. Returns false when no valid
/// session is published; the caller then keeps the honest empty line.
pub fn attach_lifecycle_from_authority_root(wiz: &mut Wizard, root: &Path) -> bool {
    match LifecycleAuthority::peek_latest(root) {
        Ok(Some(checkpoint)) => {
            let request_id = checkpoint.plan.request_id.clone();
            let generation = checkpoint.plan.generation;
            if let Ok(targets) = peek_matching_fleet_targets(root, &request_id, generation) {
                if targets.len() > 1 {
                    if let Ok((report, checkpoints)) = peek_fleet_session(root, &targets) {
                        if let Ok(view) =
                            crate::lifecycle_view::view_from_fleet_session(&report, &checkpoints)
                        {
                            let controller = crate::lifecycle_controller::LifecycleController::from_fleet_report(
                                &report,
                                checkpoints.iter().map(|checkpoint| {
                                    checkpoint.plan.target_id.clone()
                                }),
                            )
                            .ok();
                            wiz.set_lifecycle_controller(controller);
                            wiz.set_lifecycle_view(with_staged_capsule(
                                with_staged_package(
                                    with_durable_receipt(view, root, &targets),
                                    root,
                                    &targets,
                                ),
                                root,
                                &targets,
                            ));
                            return true;
                        }
                    }
                }
            }
            match crate::lifecycle_view::view_from_checkpoint(&checkpoint) {
                Ok(view) => {
                    let targets = vec![checkpoint.plan.target_id.clone()];
                    wiz.set_lifecycle_controller(None);
                    wiz.set_lifecycle_view(with_staged_capsule(
                        with_staged_package(
                            with_durable_receipt(view, root, &targets),
                            root,
                            &targets,
                        ),
                        root,
                        &targets,
                    ));
                    true
                }
                Err(_) => false,
            }
        }
        Ok(None) | Err(_) => false,
    }
}

/// Known local authority roots. Workgroup first (join/found), then the
/// mackesd state tree. Neither path is a dest and neither is treated as ready
/// just because the directory exists.
pub fn default_lifecycle_authority_roots() -> [std::path::PathBuf; 2] {
    [
        mackesd_core::default_qnm_shared_root(),
        std::path::PathBuf::from("/var/lib/mackesd"),
    ]
}

/// Attach from the first published root, or clear so a vanished checkpoint
/// cannot keep a stale ready line on screen.
pub fn refresh_lifecycle_view(wiz: &mut Wizard) -> bool {
    let roots = default_lifecycle_authority_roots();
    for root in &roots {
        if attach_lifecycle_from_authority_root(wiz, root) {
            return true;
        }
    }
    wiz.clear_lifecycle_view();
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unconfigured_node_offers_create_and_join() {
        let w = Wizard::new(false);
        assert_eq!(
            w.menu_items,
            vec![MenuItem::CreateMesh, MenuItem::JoinMesh, MenuItem::Quit]
        );
    }

    #[test]
    fn unconfigured_node_opens_on_the_welcome_gate() {
        // §43: a fresh box lands on the welcome/disclaimer gate, not the menu.
        let w = Wizard::new(false);
        assert_eq!(w.screen, Screen::Welcome);
    }

    #[test]
    fn configured_node_skips_the_welcome_gate() {
        // An already-configured node has passed the gate once — straight to menu.
        let w = Wizard::new(true);
        assert_eq!(w.screen, Screen::Menu);
    }

    #[test]
    fn acknowledging_welcome_opens_the_menu() {
        let mut w = Wizard::new(false);
        assert_eq!(w.screen, Screen::Welcome);
        w.acknowledge_welcome();
        assert_eq!(w.screen, Screen::Menu);
        // Idempotent / no-op off the gate: acking again from the menu is inert.
        w.acknowledge_welcome();
        assert_eq!(w.screen, Screen::Menu);
    }

    #[test]
    fn every_menu_item_has_a_nonempty_description() {
        for item in [
            MenuItem::CreateMesh,
            MenuItem::JoinMesh,
            MenuItem::ManagePeers,
            MenuItem::Status,
            MenuItem::Lifecycle,
            MenuItem::Quit,
        ] {
            assert!(
                !item.description().is_empty(),
                "{item:?} has no description"
            );
        }
    }

    #[test]
    fn configured_node_offers_manage_and_status() {
        let w = Wizard::new(true);
        assert_eq!(
            w.menu_items,
            vec![
                MenuItem::ManagePeers,
                MenuItem::Status,
                MenuItem::Lifecycle,
                MenuItem::Quit
            ]
        );
    }

    #[test]
    fn menu_navigation_wraps_both_ways() {
        let mut w = Wizard::new(false); // 3 items
        assert_eq!(w.menu_index, 0);
        w.menu_up(); // wrap to last
        assert_eq!(w.menu_index, 2);
        w.menu_down(); // wrap to first
        assert_eq!(w.menu_index, 0);
        w.menu_down();
        assert_eq!(w.selected(), MenuItem::JoinMesh);
    }

    #[test]
    fn activate_opens_the_selected_screen() {
        let mut w = Wizard::new(false);
        w.menu_down(); // JoinMesh
        w.activate();
        assert_eq!(w.screen, Screen::Join);
        assert!(w.log.iter().any(|l| l.contains("Join an existing mesh")));
        w.back_to_menu();
        assert_eq!(w.screen, Screen::Menu);
    }

    #[test]
    fn quit_sets_should_quit_not_a_screen() {
        let mut w = Wizard::new(true);
        // Quit is the last entry for a configured node.
        w.menu_index = w.menu_items.len() - 1;
        assert_eq!(w.selected(), MenuItem::Quit);
        w.activate();
        assert!(w.should_quit);
        assert_eq!(w.screen, Screen::Menu, "quit doesn't change the screen");
    }

    #[test]
    fn lifecycle_menu_opens_the_shared_session_screen_not_status() {
        let mut w = Wizard::new(true);
        w.menu_index = w
            .menu_items
            .iter()
            .position(|item| *item == MenuItem::Lifecycle)
            .expect("configured menu includes Lifecycle");
        w.activate();
        assert_eq!(w.screen, Screen::Lifecycle);
        assert_ne!(w.screen, Screen::Status);
        assert_eq!(
            w.lifecycle_lines(),
            vec!["no lifecycle session published".to_owned()]
        );

        let session = serde_json::json!({
            "schema_version": 1, "session_id": "session-1", "operator_id": "operator-1",
            "intent": "onboard", "target_ids": ["seat-15"], "generation": 1, "phase": "succeeded"
        });
        let readiness = serde_json::json!({
            "schema_version": 1, "target_id": "seat-15", "generation": 1,
            "ready": true, "missing_requirements": [], "warnings": []
        });
        w.set_lifecycle_view(
            LifecycleSessionView::from_wire(&session.to_string(), &readiness.to_string()).unwrap(),
        );
        let lines = w.lifecycle_lines();
        assert_eq!(lines[0], "session-1: onboard (ready)");
        assert!(lines.iter().any(|line| line.contains("capabilities")));
        assert!(!lines.iter().any(|line| line.contains("FORCE OFFBOARD")));

        let offboard = serde_json::json!({
            "schema_version": 1, "session_id": "session-2", "operator_id": "operator-1",
            "intent": "offboard", "target_ids": ["seat-15", "seat-16"], "generation": 1, "phase": "planned"
        });
        let blocked = serde_json::json!({
            "schema_version": 1, "target_id": "seat-15", "generation": 1,
            "ready": false, "missing_requirements": ["identity"], "warnings": []
        });
        w.set_lifecycle_view(
            LifecycleSessionView::from_wire(&offboard.to_string(), &blocked.to_string()).unwrap(),
        );
        let lines = w.lifecycle_lines();
        assert!(
            lines.iter().any(|line| line == "FORCE OFFBOARD 2 SYSTEMS"),
            "TUI must name the same fleet phrase as the authority"
        );

        let reset = serde_json::json!({
            "schema_version": 1, "session_id": "session-3", "operator_id": "operator-1",
            "intent": "reset_and_onboard", "target_ids": ["seat-15", "seat-16"], "generation": 1, "phase": "planned"
        });
        w.set_lifecycle_view(
            LifecycleSessionView::from_wire(&reset.to_string(), &blocked.to_string()).unwrap(),
        );
        let lines = w.lifecycle_lines();
        assert!(
            lines.iter().any(|line| line == "WIPE 2 SYSTEMS"),
            "TUI must name the same reset phrase as the authority"
        );
    }

    #[test]
    fn lifecycle_screen_hydrates_from_an_authority_checkpoint() {
        let root = std::env::temp_dir().join(format!(
            "mcnf-enroll-lifecycle-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("temp root");
        let authority = LifecycleAuthority::begin(
            &root,
            mackes_mesh_types::lifecycle::LifecyclePlanV1 {
                schema_version: 1,
                request_id: "request-1".into(),
                target_id: "seat-15".into(),
                intent: mackes_mesh_types::lifecycle::LifecycleIntentKind::Onboard,
                generation: 1,
                steps: vec!["identity".into(), "verify".into()],
            },
        )
        .expect("begin lifecycle");
        authority.finish().expect("release lock");

        let mut w = Wizard::new(true);
        assert!(attach_lifecycle_from_authority_root(&mut w, &root));
        assert_eq!(w.lifecycle_lines()[0], "request-1: onboard (in progress)");
        assert!(!attach_lifecycle_from_authority_root(
            &mut w,
            &root.join("missing")
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn lifecycle_screen_hydrates_reset_wipe_phrase_from_authority() {
        let root = std::env::temp_dir().join(format!(
            "mcnf-enroll-lifecycle-reset-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("temp root");
        let authority = LifecycleAuthority::begin(
            &root,
            mackes_mesh_types::lifecycle::LifecyclePlanV1 {
                schema_version: 1,
                request_id: "request-reset".into(),
                target_id: "seat-15".into(),
                intent: mackes_mesh_types::lifecycle::LifecycleIntentKind::ResetAndOnboard,
                generation: 1,
                steps: vec!["offboard".into(), "identity".into(), "verify".into()],
            },
        )
        .expect("begin reset lifecycle");
        authority.finish().expect("release lock");

        let mut w = Wizard::new(true);
        assert!(attach_lifecycle_from_authority_root(&mut w, &root));
        assert!(
            w.lifecycle_lines()
                .iter()
                .any(|line| line == "WIPE 1 SYSTEMS"),
            "TUI must project the authority reset phrase from the checkpoint"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn lifecycle_screen_hydrates_offboard_phrase_from_authority() {
        let root = std::env::temp_dir().join(format!(
            "mcnf-enroll-lifecycle-offboard-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("temp root");
        let authority = LifecycleAuthority::begin(
            &root,
            mackes_mesh_types::lifecycle::LifecyclePlanV1 {
                schema_version: 1,
                request_id: "request-offboard".into(),
                target_id: "seat-15".into(),
                intent: mackes_mesh_types::lifecycle::LifecycleIntentKind::Offboard,
                generation: 1,
                steps: vec!["offboard".into(), "verify".into()],
            },
        )
        .expect("begin offboard lifecycle");
        authority.finish().expect("release lock");

        let mut w = Wizard::new(true);
        assert!(attach_lifecycle_from_authority_root(&mut w, &root));
        assert!(
            w.lifecycle_lines()
                .iter()
                .any(|line| line == "FORCE OFFBOARD 1 SYSTEMS"),
            "TUI must project the authority offboard phrase from the checkpoint"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn lifecycle_screen_hydrates_offboard_receipt_without_taking_the_lock() {
        let root = std::env::temp_dir().join(format!(
            "mcnf-enroll-lifecycle-receipt-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("temp root");
        let authority = LifecycleAuthority::begin(
            &root,
            mackes_mesh_types::lifecycle::LifecyclePlanV1 {
                schema_version: 1,
                request_id: "request-receipt".into(),
                target_id: "seat-15".into(),
                intent: mackes_mesh_types::lifecycle::LifecycleIntentKind::Offboard,
                generation: 1,
                steps: vec!["offboard".into(), "verify".into()],
            },
        )
        .expect("begin offboard lifecycle");
        authority.finish().expect("release after begin");
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
            root.join("lifecycle").join("seat-15").join("receipt.json"),
            serde_json::to_vec_pretty(&receipt).unwrap(),
        )
        .unwrap();
        let held = LifecycleAuthority::resume(&root, "seat-15").expect("hold lock after persist");

        let mut w = Wizard::new(true);
        assert!(attach_lifecycle_from_authority_root(&mut w, &root));
        assert!(
            w.lifecycle_lines()
                .iter()
                .any(|line| line == "offboard receipt completed"),
            "TUI must name the durable receipt without taking the lock: {:?}",
            w.lifecycle_lines()
        );
        held.finish().expect("release held lock");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn lifecycle_screen_hydrates_staged_package_without_taking_the_lock() {
        let root = std::env::temp_dir().join(format!(
            "mcnf-enroll-lifecycle-staged-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("temp root");
        let authority = LifecycleAuthority::begin(
            &root,
            mackes_mesh_types::lifecycle::LifecyclePlanV1 {
                schema_version: 1,
                request_id: "request-staged".into(),
                target_id: "seat-15".into(),
                intent: mackes_mesh_types::lifecycle::LifecycleIntentKind::Onboard,
                generation: 1,
                steps: vec!["packages".into(), "verify".into()],
            },
        )
        .expect("begin onboard lifecycle");
        authority.finish().expect("release after begin");
        let digest = "e262f1de2c38fd96cb1a8a8410f58222f0e0b5681b84217b877e78c114eb9a31";
        let dir = root.join("lifecycle").join("seat-15");
        std::fs::write(dir.join("staged-artifact"), b"rpm-bytes").unwrap();
        std::fs::write(dir.join("staged-artifact.digest"), format!("{digest}\n")).unwrap();
        std::fs::write(dir.join("staged-artifact.shape"), "rpm\n").unwrap();
        let held = LifecycleAuthority::resume(&root, "seat-15").expect("hold lock after stage");

        let mut w = Wizard::new(true);
        assert!(attach_lifecycle_from_authority_root(&mut w, &root));
        assert!(
            w.lifecycle_lines().iter().any(|line| {
                line == "packages staged:e262f1de2c38fd96cb1a8a8410f58222f0e0b5681b84217b877e78c114eb9a31:rpm (not installed)"
            }),
            "TUI must name the staged pin without claiming dest install: {:?}",
            w.lifecycle_lines()
        );
        held.finish().expect("release held lock");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn lifecycle_screen_hydrates_staged_capsule_without_taking_the_lock() {
        let root = std::env::temp_dir().join(format!(
            "mcnf-enroll-lifecycle-capsule-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("temp root");
        let authority = LifecycleAuthority::begin(
            &root,
            mackes_mesh_types::lifecycle::LifecyclePlanV1 {
                schema_version: 1,
                request_id: "request-capsule".into(),
                target_id: "seat-15".into(),
                intent: mackes_mesh_types::lifecycle::LifecycleIntentKind::Onboard,
                generation: 1,
                steps: vec!["identity".into(), "verify".into()],
            },
        )
        .expect("begin onboard lifecycle");
        authority.finish().expect("release after begin");
        let dir = root.join("lifecycle").join("seat-15");
        let checkpoint_path = dir.join("checkpoint.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&checkpoint_path).unwrap()).unwrap();
        value["pending_capsule_ids"] = serde_json::json!(["cap-tui"]);
        std::fs::write(&checkpoint_path, serde_json::to_vec(&value).unwrap()).unwrap();
        std::fs::create_dir_all(dir.join("capsule")).unwrap();
        std::fs::write(
            dir.join("capsule").join("cap-tui"),
            b"{\"capsule_id\":\"cap-tui\"}",
        )
        .unwrap();
        let held = LifecycleAuthority::resume(&root, "seat-15").expect("hold lock after stage");

        let mut w = Wizard::new(true);
        assert!(attach_lifecycle_from_authority_root(&mut w, &root));
        assert!(
            w.lifecycle_lines()
                .iter()
                .any(|line| line == "capsule cap-tui staged (not confirmed)"),
            "TUI must name the staged capsule without claiming confirm: {:?}",
            w.lifecycle_lines()
        );
        held.finish().expect("release held lock");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn lifecycle_screen_hydrates_fleet_offboard_phrase_from_durable_seats() {
        let root = std::env::temp_dir().join(format!(
            "mcnf-enroll-lifecycle-fleet-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("temp root");
        let first = LifecycleAuthority::begin(
            &root,
            mackes_mesh_types::lifecycle::LifecyclePlanV1 {
                schema_version: 1,
                request_id: "request-1".into(),
                target_id: "seat-15".into(),
                intent: mackes_mesh_types::lifecycle::LifecycleIntentKind::Offboard,
                generation: 1,
                steps: vec!["offboard".into(), "verify".into()],
            },
        )
        .expect("begin first fleet seat");
        let second = LifecycleAuthority::begin(
            &root,
            mackes_mesh_types::lifecycle::LifecyclePlanV1 {
                schema_version: 1,
                request_id: "request-1".into(),
                target_id: "seat-16".into(),
                intent: mackes_mesh_types::lifecycle::LifecycleIntentKind::Offboard,
                generation: 1,
                steps: vec!["offboard".into(), "verify".into()],
            },
        )
        .expect("begin second fleet seat");
        let mut authorities = [first, second];
        mackesd_core::lifecycle_authority::execute_fleet_handoff(
            &mut authorities,
            "coord-a",
            "coord-b",
        )
        .expect("persist coordinator");
        for authority in authorities {
            authority.finish().expect("release lock");
        }
        let mut failed = LifecycleAuthority::resume(&root, "seat-16").expect("resume sibling");
        failed
            .record_check(mackes_mesh_types::lifecycle::LifecycleRequirementCheckV1 {
                schema_version: 1,
                check_id: "mesh".into(),
                target_id: "seat-16".into(),
                expected: "joined".into(),
                observed: "absent".into(),
                status: mackes_mesh_types::lifecycle::LifecycleCheckStatus::Fail,
                required: true,
                evidence_digest_hex: "a".repeat(64),
                warning: None,
                generation: 1,
            })
            .expect("persist sibling check");
        let correction = failed
            .propose_correction_plan()
            .expect("propose sibling correction");
        failed
            .admit_correction_plan(correction)
            .expect("persist sibling correction");
        assert!(
            failed
                .run_next_with_retry(0, "offboard".into(), |_| Err("wave-2 timeout".into()))
                .is_err(),
            "sibling last error must persist before TUI attach"
        );
        failed.finish().expect("release sibling");

        let mut w = Wizard::new(true);
        assert!(attach_lifecycle_from_authority_root(&mut w, &root));
        let lines = w.lifecycle_lines();
        assert!(
            lines.iter().any(|line| line == "FORCE OFFBOARD 2 SYSTEMS"),
            "TUI must not shrink a durable fleet to one seat: {lines:?}"
        );
        assert!(
            lines.iter().any(|line| line == "fleet seat-15, seat-16"),
            "TUI must list every durable fleet seat: {lines:?}"
        );
        assert!(
            lines.iter().any(|line| line == "coordinator coord-b"),
            "TUI must name the durable coordinator after disconnect: {lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|line| line == "last error: wave-2 timeout"),
            "TUI must surface a sibling durable last error: {lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|line| line == "correct mesh: mesh (absent)"),
            "TUI must surface a sibling durable correction: {lines:?}"
        );
        let controller = w
            .lifecycle_controller
            .as_ref()
            .expect("fleet attach must bind the peeked report");
        let view = w
            .lifecycle_view
            .as_ref()
            .expect("fleet attach must bind the shared view");
        assert_eq!(controller.fleet_line(), view.fleet_line());
        assert_eq!(controller.coordinator_line(), view.coordinator_line());
        assert_eq!(
            controller.admit_fleet_handoff("coord-forged", "coord-c"),
            Err(crate::lifecycle_controller::ProgressError::Invalid(
                "coordinator mismatch".into()
            ))
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn lifecycle_lines_keep_the_coordinator_after_a_wiped_sibling() {
        let root = std::env::temp_dir().join(format!(
            "mcnf-enroll-lifecycle-wiped-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("temp root");
        let first = LifecycleAuthority::begin(
            &root,
            mackes_mesh_types::lifecycle::LifecyclePlanV1 {
                schema_version: 1,
                request_id: "request-wipe".into(),
                target_id: "seat-15".into(),
                intent: mackes_mesh_types::lifecycle::LifecycleIntentKind::Offboard,
                generation: 1,
                steps: vec!["offboard".into(), "verify".into()],
            },
        )
        .expect("begin first fleet seat");
        let second = LifecycleAuthority::begin(
            &root,
            mackes_mesh_types::lifecycle::LifecyclePlanV1 {
                schema_version: 1,
                request_id: "request-wipe".into(),
                target_id: "seat-16".into(),
                intent: mackes_mesh_types::lifecycle::LifecycleIntentKind::Offboard,
                generation: 1,
                steps: vec!["offboard".into(), "verify".into()],
            },
        )
        .expect("begin second fleet seat");
        let mut authorities = [first, second];
        mackesd_core::lifecycle_authority::execute_fleet_handoff(
            &mut authorities,
            "coord-a",
            "coord-b",
        )
        .expect("persist coordinator");
        for authority in authorities {
            authority.finish().expect("release lock");
        }
        std::fs::remove_dir_all(root.join("lifecycle").join("seat-16")).expect("wipe sibling");
        let mut w = Wizard::new(true);
        assert!(attach_lifecycle_from_authority_root(&mut w, &root));
        let lines = w.lifecycle_lines();
        assert!(
            lines.iter().any(|line| line == "coordinator coord-b"),
            "TUI must keep the durable coordinator after a wiped sibling: {lines:?}"
        );
        assert!(
            !lines.iter().any(|line| line.contains("seat-16")),
            "a wiped sibling cannot remain in the TUI fleet: {lines:?}"
        );
        assert!(
            lines.iter().any(|line| line == "FORCE OFFBOARD 1 SYSTEMS"),
            "remaining durable scope is one seat: {lines:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn lifecycle_screen_hydrates_fleet_without_taking_the_lock() {
        let root = std::env::temp_dir().join(format!(
            "mcnf-enroll-lifecycle-lock-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("temp root");
        let held = LifecycleAuthority::begin(
            &root,
            mackes_mesh_types::lifecycle::LifecyclePlanV1 {
                schema_version: 1,
                request_id: "request-lock".into(),
                target_id: "seat-15".into(),
                intent: mackes_mesh_types::lifecycle::LifecycleIntentKind::Offboard,
                generation: 1,
                steps: vec!["offboard".into(), "verify".into()],
            },
        )
        .expect("hold first fleet seat");
        let second = LifecycleAuthority::begin(
            &root,
            mackes_mesh_types::lifecycle::LifecyclePlanV1 {
                schema_version: 1,
                request_id: "request-lock".into(),
                target_id: "seat-16".into(),
                intent: mackes_mesh_types::lifecycle::LifecycleIntentKind::Offboard,
                generation: 1,
                steps: vec!["offboard".into(), "verify".into()],
            },
        )
        .expect("begin second fleet seat");
        second.finish().expect("release sibling");
        let mut w = Wizard::new(true);
        assert!(attach_lifecycle_from_authority_root(&mut w, &root));
        let lines = w.lifecycle_lines();
        assert!(
            lines.iter().any(|line| line == "fleet seat-15, seat-16"),
            "TUI attach must peek the fleet without the lock: {lines:?}"
        );
        assert!(
            LifecycleAuthority::resume(&root, "seat-15").is_err(),
            "TUI attach must not steal a held fleet lock"
        );
        held.finish().expect("release held seat");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn lifecycle_screen_hydrates_unsigned_phrase_from_authority() {
        let root = std::env::temp_dir().join(format!(
            "mcnf-enroll-lifecycle-unsigned-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let dir = root.join("lifecycle").join("seat-15");
        std::fs::create_dir_all(&dir).expect("temp checkpoint dir");
        let digest = "e".repeat(64);
        let checkpoint = serde_json::json!({
            "plan": {
                "schema_version": 1,
                "request_id": "request-unsigned",
                "target_id": "seat-15",
                "intent": "upgrade",
                "generation": 1,
                "steps": ["packages", "verify"]
            },
            "progress": {
                "schema_version": 1,
                "request_id": "request-unsigned",
                "target_id": "seat-15",
                "generation": 1,
                "phase": "planned",
                "completed_steps": 0,
                "total_steps": 2
            },
            "artifact_selection": {
                "schema_version": 1,
                "selection_id": "sel-1",
                "target_id": "seat-15",
                "channel": "dev",
                "artifact_digest_hex": digest,
                "source_revision": "rev-1",
                "signed": false,
                "unverified_build": true,
                "generation": 1
            }
        });
        std::fs::write(
            dir.join("checkpoint.json"),
            serde_json::to_vec(&checkpoint).unwrap(),
        )
        .unwrap();
        let mut w = Wizard::new(true);
        assert!(attach_lifecycle_from_authority_root(&mut w, &root));
        let lines = w.lifecycle_lines();
        assert!(
            lines
                .iter()
                .any(|line| line == "INSTALL UNSIGNED 1 SYSTEMS"),
            "TUI must project the unsigned phrase from the checkpoint"
        );
        assert!(
            lines
                .iter()
                .any(|line| line.as_str() == format!("scope {digest}")),
            "TUI must pin the artifact digest, not the seat list"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn hydrates_the_durable_coordinator_from_the_checkpoint() {
        let root = std::env::temp_dir().join(format!(
            "mcnf-enroll-lifecycle-coord-{}-{}",
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
                "request_id": "request-handoff",
                "target_id": "seat-15",
                "intent": "onboard",
                "generation": 1,
                "steps": ["identity", "verify"]
            },
            "progress": {
                "schema_version": 1,
                "request_id": "request-handoff",
                "target_id": "seat-15",
                "generation": 1,
                "phase": "running",
                "completed_steps": 0,
                "total_steps": 2
            },
            "coordinator_id": "coord-b"
        });
        std::fs::write(
            dir.join("checkpoint.json"),
            serde_json::to_vec(&checkpoint).unwrap(),
        )
        .unwrap();
        let mut w = Wizard::new(true);
        assert!(attach_lifecycle_from_authority_root(&mut w, &root));
        assert!(
            w.lifecycle_lines()
                .iter()
                .any(|line| line == "coordinator coord-b"),
            "TUI must name the durable coordinator"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn lifecycle_lines_name_the_first_still_blocking_correction() {
        let root = std::env::temp_dir().join(format!(
            "mcnf-enroll-lifecycle-vac-{}-{}",
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
                "request_id": "request-vac",
                "target_id": "seat-15",
                "intent": "verify_and_correct",
                "generation": 1,
                "steps": ["mesh", "verify"]
            },
            "progress": {
                "schema_version": 1,
                "request_id": "request-vac",
                "target_id": "seat-15",
                "generation": 1,
                "phase": "running",
                "completed_steps": 0,
                "total_steps": 2
            },
            "checks": [{
                "schema_version": 1,
                "check_id": "mesh",
                "target_id": "seat-15",
                "expected": "joined",
                "observed": "absent",
                "status": "fail",
                "required": true,
                "evidence_digest_hex": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "generation": 1
            }],
            "correction_plan": {
                "schema_version": 1,
                "request_id": "request-vac",
                "target_id": "seat-15",
                "generation": 1,
                "corrections": [{
                    "check_id": "mesh",
                    "step": "mesh",
                    "reason": "absent",
                    "prerequisites": []
                }],
                "edges": [],
                "rollback_forbidden": true
            }
        });
        std::fs::write(
            dir.join("checkpoint.json"),
            serde_json::to_vec(&checkpoint).unwrap(),
        )
        .unwrap();
        let mut w = Wizard::new(true);
        assert!(attach_lifecycle_from_authority_root(&mut w, &root));
        assert!(
            w.lifecycle_lines()
                .iter()
                .any(|line| line == "correct mesh: mesh (absent)"),
            "TUI must name the persisted VAC action"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn lifecycle_lines_name_the_last_error() {
        let root = std::env::temp_dir().join(format!(
            "mcnf-enroll-lifecycle-err-{}-{}",
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
                "request_id": "request-err",
                "target_id": "seat-15",
                "intent": "verify_and_correct",
                "generation": 1,
                "steps": ["verify", "verify"]
            },
            "progress": {
                "schema_version": 1,
                "request_id": "request-err",
                "target_id": "seat-15",
                "generation": 1,
                "phase": "running",
                "completed_steps": 0,
                "total_steps": 2
            },
            "last_error": "provider timeout"
        });
        std::fs::write(
            dir.join("checkpoint.json"),
            serde_json::to_vec(&checkpoint).unwrap(),
        )
        .unwrap();
        let mut w = Wizard::new(true);
        assert!(attach_lifecycle_from_authority_root(&mut w, &root));
        assert!(
            w.lifecycle_lines()
                .iter()
                .any(|line| line == "last error: provider timeout"),
            "TUI must name the persisted last error"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn lifecycle_lines_name_the_onboard_nag() {
        let root = std::env::temp_dir().join(format!(
            "mcnf-enroll-lifecycle-nag-{}-{}",
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
                "request_id": "request-nag",
                "target_id": "seat-15",
                "intent": "onboard",
                "generation": 1,
                "steps": ["mesh", "verify"]
            },
            "progress": {
                "schema_version": 1,
                "request_id": "request-nag",
                "target_id": "seat-15",
                "generation": 1,
                "phase": "running",
                "completed_steps": 0,
                "total_steps": 2
            },
            "checks": [{
                "schema_version": 1,
                "check_id": "mesh_identity",
                "target_id": "seat-15",
                "expected": "enrolled mesh identity",
                "observed": "missing: overlay-ip,etcd-endpoints",
                "status": "fail",
                "required": true,
                "evidence_digest_hex": "3333333333333333333333333333333333333333333333333333333333333333",
                "generation": 1
            }]
        });
        std::fs::write(
            dir.join("checkpoint.json"),
            serde_json::to_vec(&checkpoint).unwrap(),
        )
        .unwrap();
        let mut w = Wizard::new(true);
        assert!(attach_lifecycle_from_authority_root(&mut w, &root));
        assert!(
            w.lifecycle_lines()
                .iter()
                .any(|line| line == "open ONBOARD: missing overlay-ip,etcd-endpoints"),
            "TUI must nag into ONBOARD: {:?}",
            w.lifecycle_lines()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    fn create_screen_only_reachable_when_unconfigured() {
        // A configured node has no CreateMesh entry — you can't re-found.
        let w = Wizard::new(true);
        assert!(!w.menu_items.contains(&MenuItem::CreateMesh));
        assert!(!w.menu_items.contains(&MenuItem::JoinMesh));
    }

    #[test]
    fn commissioning_lines_show_token_identity_without_the_bearer() {
        let mut w = Wizard::new(false);
        let bearer = "single-use-bearer";
        let token = format!("mesh:home@10.0.0.5:4243#{bearer}?fp={}", "a".repeat(64));
        w.set_token_view(JoinTokenView::from_wire(&token).unwrap());
        let lines = w.commissioning_lines();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("bearer withheld"));
        assert!(
            !lines.iter().any(|line| line.contains(bearer)),
            "wizard commissioning lines leaked the bearer: {lines:?}"
        );
    }

    #[test]
    fn presenting_a_token_or_capsule_fills_commissioning_lines_without_secrets() {
        let mut w = Wizard::new(false);
        assert!(w.commissioning_lines().is_empty());

        let bearer = "single-use-bearer";
        let token = format!("mesh:home@10.0.0.5:4243#{bearer}?fp={}", "a".repeat(64));
        w.present_join_token(&token).unwrap();

        let signature = "c".repeat(128);
        let capsule = serde_json::json!({
            "schema_version": 1,
            "capsule_id": "capsule-1",
            "target_id": "seat-15",
            "expires_at_ms": 2_000,
            "bootstrap_digest_hex": "b".repeat(64),
            "one_time": true,
            "key_id": "commissioning-v1",
            "signature_hex": signature,
        })
        .to_string();
        w.present_capsule(&capsule, 1_000).unwrap();

        let lines = w.commissioning_lines();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("bearer withheld"));
        assert!(lines[1].contains("signature withheld"));
        assert!(
            !lines.iter().any(|line| line.contains(bearer)),
            "present_join_token leaked the bearer into commissioning lines: {lines:?}"
        );
        assert!(
            !lines.iter().any(|line| line.contains(&signature)),
            "present_capsule leaked the signature into commissioning lines: {lines:?}"
        );
        let debug = format!("{w:?}");
        assert!(
            !debug.contains(bearer),
            "wizard debug leaked the bearer: {debug}"
        );
        assert!(
            !debug.contains(&signature),
            "wizard debug leaked the capsule signature: {debug}"
        );

        assert!(w.present_join_token("{{JOIN_TOKEN}}").is_err());
        assert!(w.present_join_token("garbage").is_err());
        assert!(w.present_capsule(&capsule, 2_000).is_err());
        assert_eq!(
            w.commissioning_lines(),
            lines,
            "failed present must leave attached views unchanged"
        );
    }
}
