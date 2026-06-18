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
use mde_theme::{Palette, Rgba};

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
    /// NEB-CRYPTO-LABEL — the live Nebula overlay cipher strength shown beside
    /// the bell (`None` = overlay down → no label).
    cipher: Option<String>,
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

/// Convert an `mde-theme` Carbon `Rgba` token into an iced color. Single
/// converter shared by the bell glyph + the cipher label (§4 — colors come
/// from the palette tokens, never raw literals here).
fn to_color(rgba: Rgba) -> cosmic::iced::Color {
    cosmic::iced::Color {
        r: f32::from(rgba.r) / 255.0,
        g: f32::from(rgba.g) / 255.0,
        b: f32::from(rgba.b) / 255.0,
        a: rgba.a,
    }
}

/// The bell's color for a severity (idle → primary text). Shared Carbon tokens.
fn bell_color(severity: Option<Severity>) -> cosmic::iced::Color {
    let p = Palette::dark();
    let rgba = match severity {
        Some(s) => severity_token(s, &p),
        // GLYPH-FIX — idle bell in primary text (Carbon Gray-10 ≈ white on the
        // dark panel), not muted gray, so it reads clearly (operator request).
        None => p.text,
    };
    to_color(rgba)
}

/// NEB-CRYPTO-LABEL — the live Nebula tunnel cipher as a short strength label
/// ("AES-256-GCM" / "ChaCha20-Poly1305"), or `None` when the overlay is down.
///
/// Primary source is the **world-readable** mesh-status snapshot
/// (`/run/mde/mesh-status.json` → `network.cipher`), written by the root
/// snapshot timer — the applet runs as the user and cannot read the root-only
/// `/etc/nebula/config.yml`, and a sandboxed panel may not see `pgrep`. Falls
/// back to a direct config read where the snapshot isn't present yet.
fn nebula_cipher() -> Option<String> {
    if let Some(c) = cipher_from_snapshot() {
        return Some(c);
    }
    // Fallback: a running tunnel + a readable config (older nodes / no snapshot).
    let running = std::process::Command::new("pgrep")
        .args(["-x", "nebula"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !running {
        return None;
    }
    let cfg = ["/etc/nebula/config.yml", "/etc/nebula/config.yaml"]
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok());
    // Nebula's default (unset) cipher is AES-256-GCM; `chachapoly` selects
    // ChaCha20-Poly1305. Map the config token to its strength label.
    let token = cfg.as_deref().and_then(parse_cipher).unwrap_or("aes");
    Some(cipher_label(token).to_string())
}

/// Read the friendly cipher label from the world-readable mesh-status snapshot
/// (`network.cipher`). `None` when the file/field is absent or empty (overlay
/// down → the snapshot writes an empty cipher).
fn cipher_from_snapshot() -> Option<String> {
    let body = std::fs::read_to_string("/run/mde/mesh-status.json").ok()?;
    let v: serde_json::Value = serde_json::from_str(&body).ok()?;
    let c = v.get("network")?.get("cipher")?.as_str()?.trim();
    if c.is_empty() {
        None
    } else {
        Some(c.to_string())
    }
}

/// Extract the top-level `cipher:` value from a Nebula config, ignoring
/// commented lines. Returns `None` when absent/empty (→ caller uses the
/// Nebula default).
fn parse_cipher(cfg: &str) -> Option<&str> {
    cfg.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .find_map(|l| l.strip_prefix("cipher:"))
        .map(|v| v.trim().trim_matches(['"', '\'']))
        .filter(|v| !v.is_empty())
}

/// Map a Nebula cipher token to its human-readable strength label.
fn cipher_label(token: &str) -> &'static str {
    match token.to_ascii_lowercase().as_str() {
        "chachapoly" => "ChaCha20-Poly1305",
        // "aes" (and the unset default) is AES-256-GCM.
        _ => "AES-256-GCM",
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
                cipher: nebula_cipher(),
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
                // Refresh the overlay cipher label (cheap file + pgrep read).
                self.cipher = nebula_cipher();
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
        let glyph_el = cosmic::widget::text(glyph)
            .size(14)
            .class(cosmic::theme::Text::Color(color));
        // NEB-CRYPTO-LABEL — the live overlay cipher strength as text next to
        // the bell (muted Carbon token; omitted when the overlay is down).
        let mut children: Vec<Element<'_, Message>> = vec![glyph_el.into()];
        if let Some(c) = &self.cipher {
            let muted = to_color(Palette::dark().text_muted);
            children.push(
                cosmic::widget::text(c.clone())
                    .size(10)
                    .class(cosmic::theme::Text::Color(muted))
                    .into(),
            );
        }
        let content = cosmic::widget::row(children)
            .spacing(6)
            .align_y(cosmic::iced::Alignment::Center);
        let btn = cosmic::widget::button::custom(content)
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

#[cfg(test)]
mod tests {
    use super::{cipher_label, parse_cipher};

    #[test]
    fn parses_explicit_cipher_value() {
        assert_eq!(parse_cipher("cipher: chachapoly\n"), Some("chachapoly"));
        assert_eq!(parse_cipher("  cipher: \"aes\"\n"), Some("aes"));
    }

    #[test]
    fn ignores_commented_and_absent_cipher() {
        assert_eq!(parse_cipher("# cipher: chachapoly\nlisten:\n"), None);
        assert_eq!(parse_cipher("pki:\n  ca: /x\n"), None);
        assert_eq!(parse_cipher("cipher:   \n"), None); // empty value
    }

    #[test]
    fn labels_map_to_strength() {
        assert_eq!(cipher_label("chachapoly"), "ChaCha20-Poly1305");
        assert_eq!(cipher_label("AES"), "AES-256-GCM");
        // Unknown/unset tokens fall back to the Nebula default (AES-256-GCM).
        assert_eq!(cipher_label("aes"), "AES-256-GCM");
        assert_eq!(cipher_label(""), "AES-256-GCM");
    }
}
