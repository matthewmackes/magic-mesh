//! MCNF first-run deployment-role chooser (PKG-5 / AUD-6).
//!
//! A libcosmic window shown once, at first boot, to pin this box's deployment
//! role — `Lighthouse ⊂ Server ⊂ Workstation` (§5). Picking a role runs
//! `mackesd role-pin <role>` (the ENT-2 upgrade-only path) and exits. If a role
//! is already pinned it exits immediately, so an `/etc/xdg/autostart` entry can
//! launch it every login as a no-op after the first run.
//!
//! The role cards + header ride `cosmic::widget`'s auto-themed styling; the only
//! explicit Carbon styling is the error state, which reads `mde-theme` tone +
//! icon tokens (never a raw glyph/colour, §4).

use cosmic::app::{Core, Settings, Task};
use cosmic::iced::widget::svg;
use cosmic::iced::window::Id;
use cosmic::iced::Length;
use cosmic::widget;
use cosmic::{Application, Element};

use mde_role::Role;
use mde_theme::{mde_icon, Brand, BrandSlot, Icon, IconSize, IconState, Palette, Rgba, StateTone};

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

// ── Error-state presentation (POLISH-rolechooser-error) ──────────────────────
//
// The inline status line only ever surfaces a role-pin *failure* (mackesd
// missing, or refusing the upgrade-only pin) — a terminal error. It renders as
// the typed Carbon error icon in the danger tone: icon **and** colour together,
// never colour alone (the a11y contract the music / files surfaces hold). The
// (icon, tone) pairing and the tone→token lookup are pulled out as pure,
// testable mappings; the glyph + hue are sourced from `mde-theme`, never minted
// inline (§4 single-source, §6 reuse-don't-reimplement).

/// The typed icon for the terminal role-pin failure.
const ERROR_ICON: Icon = Icon::StatusError;
/// The Carbon severity tone for the terminal role-pin failure.
const ERROR_TONE: StateTone = StateTone::Danger;

/// The Carbon support-colour token for a [`StateTone`] — the secondary cue under
/// the icon + label. Every arm reads a [`Palette`] token (§4); mirrors the
/// `tone_token` idiom the other surfaces use (mde-theme stays GUI-toolkit-free,
/// so each surface resolves the tone against its own `Color` locally).
const fn tone_token(p: &Palette, tone: StateTone) -> Rgba {
    match tone {
        StateTone::Neutral => p.text_muted,
        StateTone::Info => p.accent,
        StateTone::Warning => p.warning,
        StateTone::Danger => p.danger,
        StateTone::Success => p.success,
    }
}

/// Convert an mde-theme Carbon token (`Rgba`, u8 channels) to libcosmic's
/// `iced::Color` at alpha `a`. mde-theme is deliberately toolkit-free (CUT-3), so
/// the channel math lives here — the single sanctioned raw-channel spot, keeping
/// the call site on a token rather than a literal (§4).
fn carbon(rgba: Rgba, a: f32) -> cosmic::iced::Color {
    cosmic::iced::Color {
        r: f32::from(rgba.r) / 255.0,
        g: f32::from(rgba.g) / 255.0,
        b: f32::from(rgba.b) / 255.0,
        a,
    }
}

/// Render a typed [`Icon`] as a `color`-tinted inline SVG, through the same
/// `svg(Handle::from_memory(..))` path the wordmark uses. Routing the glyph
/// through the canonical Material [`Icon`] set keeps the surface free of inline
/// codepoint literals (§4); the [`IconSize::Inline`] optical tier matches the
/// neighbouring body text.
fn typed_icon(icon: Icon, color: cosmic::iced::Color) -> Element<'static, Message> {
    let resolved = mde_icon(icon, IconSize::Inline);
    let bytes = resolved.svg_bytes_for_state(IconState::Idle);
    svg::Svg::new(svg::Handle::from_memory(bytes))
        .width(Length::Fixed(resolved.size_px()))
        .height(Length::Fixed(resolved.size_px()))
        .class(cosmic::theme::iced::Svg::custom(
            move |_theme: &cosmic::Theme| svg::Style { color: Some(color) },
        ))
        .into()
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
            // Terminal role-pin failure: typed error icon + message, both in the
            // Carbon danger tone (icon + colour, never colour alone).
            let tone = carbon(tone_token(&Palette::dark(), ERROR_TONE), 1.0);
            col = col.push(
                widget::row(vec![
                    typed_icon(ERROR_ICON, tone),
                    widget::text::body(self.status.clone())
                        .class(cosmic::theme::Text::Color(tone))
                        .into(),
                ])
                .spacing(8)
                .align_y(cosmic::iced::Alignment::Center),
            );
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

#[cfg(test)]
mod tests {
    use super::{carbon, tone_token, ERROR_ICON, ERROR_TONE};
    use mde_theme::{Icon, Palette, StateTone};

    /// The status line is only ever a role-pin failure — a terminal error — so it
    /// must read as the typed error icon in the danger tone (never a softer cue).
    #[test]
    fn error_state_is_status_error_in_danger() {
        assert_eq!(ERROR_ICON, Icon::StatusError);
        assert_eq!(ERROR_TONE, StateTone::Danger);
        // StatusError is an always-filled status glyph, so it resolves to real
        // SVG bytes (the typed icon renders, never a blank box).
        let bytes = mde_theme::mde_icon(ERROR_ICON, mde_theme::IconSize::Inline)
            .svg_bytes_for_state(mde_theme::IconState::Idle);
        assert!(!bytes.is_empty());
    }

    /// Every tone resolves to a `Palette` support-colour token, never a literal
    /// (§4). `Rgba` has no `Debug`, so compare with `==`.
    #[test]
    fn tone_token_maps_to_palette_support_colours() {
        let p = Palette::dark();
        assert!(tone_token(&p, StateTone::Neutral) == p.text_muted);
        assert!(tone_token(&p, StateTone::Info) == p.accent);
        assert!(tone_token(&p, StateTone::Warning) == p.warning);
        assert!(tone_token(&p, StateTone::Danger) == p.danger);
        assert!(tone_token(&p, StateTone::Success) == p.success);
        // The error path specifically lands on the danger support colour.
        assert!(tone_token(&p, ERROR_TONE) == p.danger);
    }

    /// The Rgba→Color channel math is exact at the rail values (the one
    /// sanctioned raw-channel spot stays correct).
    #[test]
    fn carbon_converts_channels_and_alpha() {
        let c = carbon(mde_theme::Rgba::rgb(255, 0, 0), 1.0);
        assert_eq!(c.r, 1.0);
        assert_eq!(c.g, 0.0);
        assert_eq!(c.b, 0.0);
        assert_eq!(c.a, 1.0);
    }
}
