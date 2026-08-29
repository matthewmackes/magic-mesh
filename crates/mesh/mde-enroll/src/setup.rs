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
use crate::lifecycle_view::LifecycleSessionView;
use mackesd_core::lifecycle_authority::LifecycleAuthority;

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

    /// Drop a stale projection so the screen cannot keep showing a session
    /// after the authority tree is gone.
    pub fn clear_lifecycle_view(&mut self) {
        self.lifecycle_view = None;
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

/// Hydrate the wizard from `root/lifecycle/*/checkpoint.json` without
/// taking the exclusive authority lock. Returns false when no valid
/// session is published; the caller then keeps the honest empty line.
pub fn attach_lifecycle_from_authority_root(wiz: &mut Wizard, root: &Path) -> bool {
    match LifecycleAuthority::peek_latest(root) {
        Ok(Some(checkpoint)) => match crate::lifecycle_view::view_from_checkpoint(&checkpoint) {
            Ok(view) => {
                wiz.set_lifecycle_view(view);
                true
            }
            Err(_) => false,
        },
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
