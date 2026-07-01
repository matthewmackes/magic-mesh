//! MCNF first-run onboarding **entry** — the role-chooser (ONBOARD-WIZARD OW-1).
//!
//! The first thing a bare box shows at first boot: a four-step gate that pins the
//! deployment role and captures what to do next, then hands off to the (separate)
//! onboarding wizard.
//!
//! 1. **Disclaimer** — show `mde_disclaimer::TEXT`; an explicit acknowledgement is
//!    required before anything else is reachable (design lock §43).
//! 2. **Role** — Lighthouse or Workstation (the 2-role model, governance §5).
//! 3. **Intent** — **Create New Mesh** (a Workstation only — only the founding
//!    Workstation mints the CA, §5) or **Join Existing Mesh** (any role).
//! 4. **Confirm** — review, then pin the role via `mackesd role-pin <role>` (the
//!    upgrade-only ENT-2 path) and record the hand-off
//!    (`~/.config/mde/onboard.json` + the disclaimer acceptance marker) for the
//!    wizard to pick up.
//!
//! If a role is already pinned this exits immediately, so an `/etc/xdg/autostart`
//! entry can fire every login as a no-op after the first run. OW-1 only captures
//! role + intent + ack and hands off; the mesh-create / mesh-join work is the
//! separate wizard (OW-3 / OW-4).
//!
//! The flow logic is the render-agnostic [`flow::Onboard`] state machine (unit
//! tested); this file is its thin egui renderer. All look comes from
//! `mde_egui::Style` — no hand-rolled colour or metric (governance §4).

mod flow;

use std::io;
use std::path::PathBuf;

use flow::{Intent, Onboard, Outcome, Step};
use mde_egui::{eframe, egui, run_client, Style};
use mde_role::Role;

/// One role's display copy: `(name, blurb)`.
fn role_blurb(role: Role) -> (&'static str, &'static str) {
    match role {
        Role::Lighthouse => (
            "Lighthouse",
            "Always-on relay + control plane — Nebula overlay, the mackesd \
             control plane, the media server, and the CA/signer. No desktop. \
             VPS-friendly. (Rank 0)",
        ),
        Role::Workstation => (
            "Workstation",
            "The full Quasar stack — the egui-DRM shell + VDI + local \
             KVM/cloud-hypervisor + Podman. A headless machine is just a \
             Workstation without a local display. (Rank 1)",
        ),
    }
}

/// Human-readable label for an intent (the machine slug is [`Intent::as_str`]).
fn intent_label(intent: Intent) -> &'static str {
    match intent {
        Intent::CreateNewMesh => "Create New Mesh",
        Intent::JoinExistingMesh => "Join Existing Mesh",
    }
}

/// Pin `slug` via `mackesd role-pin` (upgrade-only, fail-closed). `Ok(())` on a
/// successful pin; `Err(msg)` otherwise (mackesd missing / refused the downgrade).
fn pin_role(slug: &str) -> Result<(), String> {
    match std::process::Command::new("mackesd")
        .args(["role-pin", slug])
        .status()
    {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("mackesd role-pin exited with status {s}")),
        Err(e) => Err(format!("could not run mackesd role-pin: {e}")),
    }
}

/// Where the onboarding hand-off is recorded: a sibling of the disclaimer
/// acceptance marker (`…/mde/onboard.json`). Reusing the disclaimer crate's XDG
/// resolver keeps the `$XDG_CONFIG_HOME` → `$HOME/.config` fallback in one place
/// and guarantees the wizard finds the intent next to the ack. `None` when neither
/// env var is set.
fn onboard_path() -> Option<PathBuf> {
    mde_disclaimer::acceptance_path().map(|p| p.with_file_name("onboard.json"))
}

/// Record the captured `{role, intent}` for the (separate) wizard to read.
///
/// # Errors
/// I/O errors creating the config dir or writing the hand-off file, or the
/// absence of `$XDG_CONFIG_HOME` / `$HOME` to anchor it.
fn write_onboard(outcome: &Outcome) -> io::Result<()> {
    let path = onboard_path().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "no XDG_CONFIG_HOME / HOME to record onboard intent",
        )
    })?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, format!("{}\n", outcome.to_json()))
}

/// A dimmed secondary label — the one place this surface reaches for `TEXT_DIM`.
fn dim(ui: &mut egui::Ui, text: impl Into<egui::RichText>) {
    ui.colored_label(Style::TEXT_DIM, text);
}

/// A section heading in the shared heading style.
fn heading(ui: &mut egui::Ui, text: &str) {
    ui.heading(
        egui::RichText::new(text)
            .color(Style::TEXT)
            .size(Style::HEADING),
    );
}

/// The chooser surface — a thin egui renderer over the [`Onboard`] state machine.
struct RoleChooser {
    /// The render-agnostic onboarding state machine.
    flow: Onboard,
    /// Local tick for the disclaimer checkbox; the state-machine ack only fires
    /// once the operator then clicks Continue.
    disclaimer_checked: bool,
    /// An inline failure line (a refused intent, or a role-pin / hand-off write
    /// failure). Empty when there is nothing to report.
    status: String,
}

impl RoleChooser {
    fn new() -> Self {
        Self {
            flow: Onboard::new(),
            disclaimer_checked: false,
            status: String::new(),
        }
    }

    /// Step 1 — show the disclaimer behind an explicit acknowledgement gate.
    fn view_disclaimer(&mut self, ui: &mut egui::Ui) {
        let (title, body) = mde_disclaimer::split();
        heading(ui, title);
        ui.add_space(Style::SP_S);
        dim(
            ui,
            "Read and accept this before the machine joins or founds a mesh.",
        );
        ui.add_space(Style::SP_M);

        egui::ScrollArea::vertical()
            .max_height(Style::SP_XL * 8.0)
            .show(ui, |ui| {
                ui.colored_label(Style::TEXT_DIM, body);
            });
        ui.add_space(Style::SP_M);

        ui.checkbox(
            &mut self.disclaimer_checked,
            egui::RichText::new("I have read and understand this disclaimer.").color(Style::TEXT),
        );
        ui.add_space(Style::SP_S);

        let ready = self.disclaimer_checked;
        let label = egui::RichText::new("I understand — continue").color(if ready {
            Style::BG
        } else {
            Style::TEXT_DIM
        });
        let button =
            egui::Button::new(label).fill(if ready { Style::ACCENT } else { Style::SURFACE });
        if ui.add_enabled(ready, button).clicked() {
            self.flow.ack();
        }
    }

    /// Step 2 — pick a deployment role (the two role cards).
    fn view_role(&mut self, ui: &mut egui::Ui) {
        heading(ui, "Choose this machine's role");
        ui.add_space(Style::SP_S);
        dim(
            ui,
            "One deployment role per machine — upgrade-only (Lighthouse → \
             Workstation), never a downgrade. A headless box is a Workstation \
             with no local display.",
        );
        ui.add_space(Style::SP_M);

        for role in Role::all() {
            let (name, blurb) = role_blurb(role);
            let button = egui::Button::new(
                egui::RichText::new(format!("{name}\n{blurb}")).color(Style::TEXT),
            )
            .fill(Style::SURFACE);
            if ui.add_sized([ui.available_width(), 0.0], button).clicked() {
                self.flow.choose_role(role);
            }
            ui.add_space(Style::SP_S);
        }
    }

    /// Step 3 — create-vs-join, with Create disabled + explained off a Workstation.
    fn view_intent(&mut self, ui: &mut egui::Ui) {
        let Some(role) = self.flow.role() else {
            return;
        };
        heading(ui, "Create a new mesh, or join one?");
        ui.add_space(Style::SP_S);
        dim(ui, format!("Role: {}.", role_blurb(role).0));
        ui.add_space(Style::SP_M);

        // Create New Mesh — a Workstation founds the mesh (§5); disabled + explained
        // on a Lighthouse (the honest "Workstation founds the mesh" lock).
        let can_create = self.flow.can_create();
        let create = egui::Button::new(
            egui::RichText::new(
                "Create New Mesh\nFound a brand-new mesh — this machine mints the \
                 CA and mesh identity.",
            )
            .color(if can_create {
                Style::TEXT
            } else {
                Style::TEXT_DIM
            }),
        )
        .fill(Style::SURFACE)
        .min_size(egui::vec2(ui.available_width(), 0.0));
        if ui.add_enabled(can_create, create).clicked() {
            match self.flow.choose_intent(Intent::CreateNewMesh) {
                Ok(()) => self.status.clear(),
                Err(e) => self.status = e.to_string(),
            }
        }
        if !can_create {
            ui.add_space(Style::SP_XS);
            ui.colored_label(
                Style::WARN,
                format!(
                    "Only a Workstation can found a mesh — a {} joins an existing one.",
                    role_blurb(role).0
                ),
            );
        }
        ui.add_space(Style::SP_S);

        // Join Existing Mesh — valid for any role.
        let join = egui::Button::new(
            egui::RichText::new(
                "Join Existing Mesh\nScan a QR code / type a short code from a node \
                 already in the mesh.",
            )
            .color(Style::TEXT),
        )
        .fill(Style::SURFACE)
        .min_size(egui::vec2(ui.available_width(), 0.0));
        if ui.add(join).clicked() {
            self.status.clear();
            // Join is valid for every role; the state machine never refuses it.
            let _ = self.flow.choose_intent(Intent::JoinExistingMesh);
        }
        ui.add_space(Style::SP_M);

        if ui.button("← Back").clicked() {
            self.status.clear();
            self.flow.back();
        }
    }

    /// Step 4 — review the captured role + intent, then commit + hand off.
    fn view_confirm(&mut self, ui: &mut egui::Ui) {
        let Some(outcome) = self.flow.outcome() else {
            return;
        };
        heading(ui, "Confirm and continue");
        ui.add_space(Style::SP_S);
        dim(
            ui,
            "Pinning the role is one-way (upgrade-only). The onboarding wizard \
             takes it from here.",
        );
        ui.add_space(Style::SP_M);

        ui.group(|ui| {
            ui.colored_label(
                Style::TEXT,
                format!("Role:  {}", role_blurb(outcome.role).0),
            );
            ui.add_space(Style::SP_XS);
            ui.colored_label(
                Style::TEXT,
                format!("Next:  {}", intent_label(outcome.intent)),
            );
        });
        ui.add_space(Style::SP_M);

        ui.horizontal(|ui| {
            if ui.button("← Back").clicked() {
                self.status.clear();
                self.flow.back();
            }
            ui.add_space(Style::SP_S);
            let confirm = egui::Button::new(
                egui::RichText::new("Confirm — pin role & continue").color(Style::BG),
            )
            .fill(Style::ACCENT);
            if ui.add(confirm).clicked() {
                self.commit(&outcome);
            }
        });
    }

    /// Commit the captured outcome: persist the hand-off, then pin the role.
    ///
    /// Persist BEFORE the one-way role pin: once a role is pinned the first-run
    /// gate stops the chooser ever running again, so the disclaimer acceptance +
    /// intent must already be on disk by then. A failure before the pin leaves no
    /// role pinned, so the autostart simply re-runs the chooser and rewrites the
    /// hand-off — never a half-onboarded box.
    fn commit(&mut self, outcome: &Outcome) {
        if let Err(e) = mde_disclaimer::record_acceptance() {
            self.status = format!("could not record disclaimer acceptance: {e}");
            return;
        }
        if let Err(e) = write_onboard(outcome) {
            self.status = format!("could not record onboarding intent: {e}");
            return;
        }
        // Pin last (the gated, fail-closed step). Also print the hand-off so a
        // headless / logged run captures it, then exit cleanly.
        match pin_role(outcome.role.as_str()) {
            Ok(()) => {
                println!("{}", outcome.to_json());
                std::process::exit(0);
            }
            Err(e) => self.status = e,
        }
    }
}

impl eframe::App for RoleChooser {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(Style::SP_L);
            match self.flow.step() {
                Step::Disclaimer => self.view_disclaimer(ui),
                Step::Role => self.view_role(ui),
                Step::Intent => self.view_intent(ui),
                Step::Confirm => self.view_confirm(ui),
            }
            if !self.status.is_empty() {
                ui.add_space(Style::SP_S);
                ui.colored_label(Style::DANGER, format!("⚠  {}", self.status));
            }
        });
    }
}

fn main() -> eframe::Result<()> {
    // First-run gate: if a role is already pinned, this is a no-op (so a first-boot
    // autostart that fires every login does nothing after run one).
    if mde_role::load().is_ok() {
        return Ok(());
    }
    run_client("org.magicmesh.RoleChooser", "MCNF — Welcome", |_cc| {
        RoleChooser::new()
    })
}

#[cfg(test)]
mod tests {
    use super::{intent_label, onboard_path, role_blurb};
    use crate::flow::Intent;
    use mde_role::Role;

    #[test]
    fn every_role_has_display_copy() {
        for r in Role::all() {
            let (name, blurb) = role_blurb(r);
            assert!(!name.is_empty(), "role {r:?} has no name");
            assert!(blurb.len() > 20, "role {r:?} blurb too short");
        }
    }

    #[test]
    fn the_two_roles_are_lighthouse_and_workstation() {
        // The 2-role model: only Lighthouse and Workstation, no middle role.
        assert_eq!(Role::all().len(), 2);
        assert_eq!(role_blurb(Role::Lighthouse).0, "Lighthouse");
        assert_eq!(role_blurb(Role::Workstation).0, "Workstation");
    }

    #[test]
    fn intent_labels_are_human_readable_and_distinct() {
        let create = intent_label(Intent::CreateNewMesh);
        let join = intent_label(Intent::JoinExistingMesh);
        assert_eq!(create, "Create New Mesh");
        assert_eq!(join, "Join Existing Mesh");
        assert_ne!(create, join);
    }

    #[test]
    fn onboard_path_sits_next_to_the_acceptance_marker() {
        // The wizard reads the intent next to the disclaimer ack — both under
        // `…/mde/`. Anchor a temp XDG dir so the assertion is hermetic.
        let tmp = std::env::temp_dir().join(format!("mde-ow1-test-{}", std::process::id()));
        let prev = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", &tmp);

        let onboard = onboard_path().expect("onboard path");
        let accept = mde_disclaimer::acceptance_path().expect("acceptance path");
        assert_eq!(onboard.file_name().unwrap(), "onboard.json");
        assert_eq!(
            onboard.parent(),
            accept.parent(),
            "hand-off lives beside the ack marker"
        );
        assert!(onboard.starts_with(&tmp));

        match prev {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
    }
}
