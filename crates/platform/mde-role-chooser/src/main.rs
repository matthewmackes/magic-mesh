//! MCNF first-run deployment-role chooser (PKG-5 / AUD-6).
//!
//! A libcosmic window shown once, at first boot, to pin this box's deployment
//! role — `Lighthouse ⊂ Server ⊂ Workstation` (§5). Picking a role runs
//! `mackesd role-pin <role>` (the ENT-2 upgrade-only path) and exits. If a role
//! is already pinned it exits immediately, so an `/etc/xdg/autostart` entry can
//! launch it every login as a no-op after the first run.
//!
//! Native `cosmic::widget` styling (auto-themed) — no per-widget Carbon closures
//! needed here.

use cosmic::app::{Core, Settings, Task};
use cosmic::iced::widget::svg;
use cosmic::iced::window::Id;
use cosmic::iced::Length;
use cosmic::widget;
use cosmic::{Application, Element};

use mde_role::Role;
use mde_theme::{Brand, BrandSlot};

const APP_ID: &str = "com.mackes.MagicMeshRoleChooser";

/// Wordmark display width. 4:1 aspect (per the BrandSlot contract), so
/// this renders ~50 px tall above the title.
const WORDMARK_WIDTH: f32 = 200.0;

struct RoleChooser {
    core: Core,
    /// Inline status line (e.g. a role-pin failure). Empty on launch.
    status: String,
    /// AUD2-4 — MCNF wordmark bytes, resolved once at init via
    /// the [`Brand`] loader ($MDE_BRAND_DIR → /usr/share/mde/brand →
    /// baked fallback; the baked SVG guarantees this is never empty).
    wordmark: svg::Handle,
}

#[derive(Clone, Debug)]
enum Message {
    /// Operator picked a role (carried as its canonical slug).
    Pick(&'static str),
}

/// One role's display copy.
fn role_blurb(role: Role) -> (&'static str, &'static str) {
    match role {
        Role::Lighthouse => (
            "Lighthouse",
            "Relay-only mesh node — Nebula overlay + control plane. No storage \
             brick, no desktop. VPS-friendly. (Rank 0)",
        ),
        Role::LighthouseMedia => (
            "Lighthouse (Media)",
            "A Lighthouse that also hosts the mesh music service — runs the capped \
             Navidrome container behind music.mesh. Pick this only on a node with \
             enough RAM/disk for the container. (Rank 0 + media)",
        ),
        Role::Server => (
            "Server",
            "Headless mesh peer — Lighthouse + a replicated storage brick + \
             fleet/monitoring workers. No desktop. (Rank 1)",
        ),
        Role::Workstation => (
            "Workstation",
            "Full workstation — Server + the Cosmic desktop and all the GUIs. \
             (Rank 2)",
        ),
    }
}

/// Pin `slug` via `mackesd role-pin` (upgrade-only, fail-closed). `Ok(())` on a
/// successful pin; `Err(msg)` otherwise (mackesd missing / refused).
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

impl Application for RoleChooser {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = APP_ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: ()) -> (Self, Task<Message>) {
        (
            RoleChooser {
                core,
                status: String::new(),
                wordmark: svg::Handle::from_memory(Brand::new().bytes(BrandSlot::Wordmark)),
            },
            Task::none(),
        )
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Pick(slug) => match pin_role(slug) {
                // Pinned — the chooser's job is done; exit so first-boot moves on.
                Ok(()) => std::process::exit(0),
                Err(e) => {
                    self.status = e;
                    Task::none()
                }
            },
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let mut col = widget::Column::new()
            .spacing(16)
            // AUD2-4 — brand header: the wordmark resolved through the
            // Brand loader (system layer / env override / baked SVG).
            .push(svg::Svg::new(self.wordmark.clone()).width(Length::Fixed(WORDMARK_WIDTH)))
            .push(widget::text::title2("Choose this machine's role"))
            .push(widget::text::body(
                "MCNF pins one deployment role per machine at install. You \
                 can upgrade later (Lighthouse → Server → Workstation), never \
                 downgrade.",
            ));

        for role in Role::all() {
            let (name, blurb) = role_blurb(role);
            let card = widget::Column::new()
                .spacing(4)
                .push(widget::text::heading(name))
                .push(widget::text::caption(blurb));
            col = col.push(
                widget::button::custom(card)
                    .width(Length::Fill)
                    .on_press(Message::Pick(role.as_str())),
            );
        }

        if !self.status.is_empty() {
            col = col.push(widget::text::body(format!("⚠ {}", self.status)));
        }

        widget::container(col)
            .padding(32)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn view_window(&self, _id: Id) -> Element<'_, Message> {
        self.view()
    }
}

fn main() -> cosmic::iced::Result {
    // First-run gate: if a role is already pinned, this is a no-op (so a
    // first-boot autostart that fires every login does nothing after run one).
    if mde_role::load().is_ok() {
        return Ok(());
    }
    cosmic::app::run::<RoleChooser>(Settings::default(), ())
}
