//! mde-cosmic-applet — the MCNF **notification bell** (NOTIFY-7 / Action
//! Center, operator direction 2026-06-15).
//!
//! A Cosmic panel applet that:
//!   * **toggles the Action Center** (`mde-notify-center`) on a single click —
//!     open if closed, close if open. No popover (the old quick-action popover
//!     took seconds to build on click); the click is now an instant
//!     spawn-or-kill, so it feels immediate.
//!   * **tints by the highest-severity unread alert** tailed off the live bus
//!     (`mde_notify::AlertTail`), using the same severity→Carbon-token map as
//!     the Action Center. Opening the center clears the indicator (marks seen).
//!
//! The render-agnostic logic (pip/quick-action tables) still lives in the lib;
//! this bin is the libcosmic panel shell.

use std::time::Duration;

use cosmic::app::{Core, Task};
use cosmic::iced::window::Id;
use cosmic::iced::{time, Subscription};
use cosmic::{Application, Element};

use mde_notify::{severity_token, AlertTail, Severity};
use mde_theme::Palette;

const ID: &str = "com.mackes.MagicMeshApplet";
/// Bell refresh cadence — a new alert tints the bell within this window.
const REFRESH: Duration = Duration::from_secs(5);

struct Applet {
    core: Core,
    /// Shared bus tail — each tick drains fresh alerts.
    tail: AlertTail,
    /// Highest-severity unread alert since the center was last opened
    /// (`None` = nothing unread → idle bell).
    severity: Option<Severity>,
}

#[derive(Clone, Debug)]
enum Message {
    /// Periodic bus poll for fresh alerts.
    Tick,
    /// Click — toggle the Action Center open/closed + clear the indicator.
    Toggle,
}

/// Drain fresh alerts off the bus; return the most-severe new alert's severity
/// (`Severity` is ordered most-severe-first, so `min` is the most severe).
fn poll_new_severity(tail: &mut AlertTail) -> Option<Severity> {
    let dir = mde_bus::client_data_dir()?;
    let persist = mde_bus::persist::Persist::open(dir).ok()?;
    tail.poll(&persist).into_iter().map(|a| a.severity).min()
}

/// Merge a newly-observed severity into the held one, keeping the most severe.
fn merge_severity(held: Option<Severity>, fresh: Option<Severity>) -> Option<Severity> {
    match (held, fresh) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, None) => a,
        (None, b) => b,
    }
}

/// Toggle the Action Center: kill it if running (close), else spawn it (open).
/// Fast (a `pgrep`/`pkill`/spawn), so the click feels instant.
fn toggle_center() {
    let running = std::process::Command::new("pgrep")
        .args(["-f", "/usr/bin/mde-notify-center"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if running {
        let _ = std::process::Command::new("pkill")
            .args(["-f", "mde-notify-center"])
            .status();
    } else {
        // Launch detached via `setsid --fork` so the Action Center reparents to
        // init (which reaps it on exit). A bare `spawn()` leaves the child as the
        // applet's own un-waited child → a zombie accumulates on every toggle
        // (NOTIFY-UI-4: an operator clicking the bell repeatedly left ~18 defunct
        // `mde-notify-center` entries). `.status()` reaps the short-lived forking
        // parent immediately; the real process detaches.
        let _ = std::process::Command::new("setsid")
            .args(["--fork", "mde-notify-center"])
            .status();
    }
}

/// The bell's color for a severity (idle → muted text). Shared Carbon tokens.
fn bell_color(severity: Option<Severity>) -> cosmic::iced::Color {
    let p = Palette::dark();
    let rgba = match severity {
        Some(s) => severity_token(s, &p),
        // GLYPH-FIX — idle bell in primary text (Carbon Gray-10 ≈ white on the
        // dark panel), not muted gray, so it reads clearly (operator request).
        None => p.text,
    };
    cosmic::iced::Color {
        r: f32::from(rgba.r) / 255.0,
        g: f32::from(rgba.g) / 255.0,
        b: f32::from(rgba.b) / 255.0,
        a: rgba.a,
    }
}

fn main() -> cosmic::iced::Result {
    cosmic::applet::run::<Applet>(())
}

impl Application for Applet {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: Self::Flags) -> (Self, Task<Message>) {
        (
            Applet {
                core,
                tail: AlertTail::default(),
                severity: None,
            },
            // Prime immediately so the bell reflects existing alerts at launch.
            Task::done(cosmic::Action::App(Message::Tick)),
        )
    }

    fn subscription(&self) -> Subscription<Message> {
        time::every(REFRESH).map(|_| Message::Tick)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => {
                let fresh = poll_new_severity(&mut self.tail);
                self.severity = merge_severity(self.severity, fresh);
            }
            Message::Toggle => {
                toggle_center();
                // Opening (or closing) the center marks the current alerts seen.
                self.severity = None;
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        // A bell glyph tinted by the highest unread severity (filled ● when
        // unread, hollow ○ when idle). The whole button toggles the center.
        let unread = self.severity.is_some();
        // GLYPH-FIX (2026-06-18) — use a PURE-SYMBOL BMP glyph (●/○), not an
        // emoji. The 🔔/🔕 emoji (even with U+FE0E) render via the color-emoji
        // font: they ignore our Carbon `Text::Color` tint (→ black/invisible on
        // the dark panel) AND loading the emoji font on first paint stalls the
        // draw for seconds. ● filled = unread, ○ hollow = idle; the tint applies.
        let glyph = if unread { "\u{25CF}" } else { "\u{25CB}" }; // ● / ○
        let color = bell_color(self.severity);
        let label = cosmic::widget::text(glyph)
            .size(14)
            .class(cosmic::theme::Text::Color(color));
        let btn = cosmic::widget::button::custom(label)
            .on_press(Message::Toggle)
            .class(cosmic::theme::Button::AppletIcon);
        Element::from(self.core.applet.applet_tooltip::<Message>(
            btn,
            "Notifications — click to open the Action Center".to_string(),
            false,
            |_| Message::Toggle,
            None,
        ))
    }

    fn view_window(&self, _id: Id) -> Element<'_, Message> {
        cosmic::widget::text("").into()
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }
}
