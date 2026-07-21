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
            should_quit: false,
        }
    }

    /// The menu entries shown for a given configured-state.
    #[must_use]
    pub fn menu_for(configured: bool) -> Vec<MenuItem> {
        if configured {
            vec![MenuItem::ManagePeers, MenuItem::Status, MenuItem::Quit]
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
            vec![MenuItem::ManagePeers, MenuItem::Status, MenuItem::Quit]
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
    fn create_screen_only_reachable_when_unconfigured() {
        // A configured node has no CreateMesh entry — you can't re-found.
        let w = Wizard::new(true);
        assert!(!w.menu_items.contains(&MenuItem::CreateMesh));
        assert!(!w.menu_items.contains(&MenuItem::JoinMesh));
    }
}
