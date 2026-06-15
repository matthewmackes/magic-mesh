//! NOTIFY-4 — the MDE-Notification-Hub **transient toast layer**: a Top-layer,
//! non-interactive layer-shell surface that slides fresh mesh alerts in as
//! auto-expiring toasts (design: `docs/design/mde-notification-hub.md`).
//!
//! It shares the [`mde_notify::AlertTail`] bus tail with the center
//! (`mde-notify-center`); where the center is the full persistent history, this
//! surface shows ONLY brand-new alerts for a few seconds, then drops them. It is
//! DND-aware: when `mde_bus::dnd` is active, ordinary alerts are suppressed
//! (the center still records them) — only Critical alerts bypass DND, so genuine
//! emergencies still reach the operator. Desktop-app (`fdo/*`) notifications are
//! shown in the center, never double-toasted.
//!
//! Adaptive motion budget: the fast fade tick only runs while toasts are on
//! screen; an idle mesh runs just the 2 s bus poll.

use cosmic::iced::platform_specific::runtime::wayland::layer_surface::SctkLayerSurfaceSettings;
use cosmic::iced::platform_specific::shell::commands::layer_surface::{
    get_layer_surface, Anchor, KeyboardInteractivity, Layer,
};
use cosmic::iced::widget::{column, container, row, text, Space};
use cosmic::iced::{
    window, Background, Border, Color, Element, Length, Padding, Subscription, Task,
};
use mde_notify::{
    severity_token, sound_for_alert, AlertItem, AlertTail, Severity, SoundSettings, Source,
};
use mde_theme::Palette;
use mde_workbench::cosmic_compat::IntoIcedColor;

/// Toast column width (px).
const TOAST_WIDTH: f32 = 380.0;
/// Bus poll cadence — a new alert toasts within this window of a publish.
const POLL_SECS: u64 = 2;
/// Fade/animation tick — only registered while toasts are live (adaptive budget).
const ANIM_MS: u64 = 80;
/// How long a toast stays on screen before it expires.
const TOAST_TTL_MS: i64 = 6_000;
/// Fade-in / fade-out ramp durations (ms) within the TTL.
const FADE_IN_MS: i64 = 220;
const FADE_OUT_MS: i64 = 500;
/// Max toasts stacked at once (newest kept; older dropped early).
const MAX_TOASTS: usize = 4;

fn main() -> Result<(), cosmic::iced::Error> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    cosmic::iced::daemon(|| (Toaster::new(), boot_task()), update, view)
        .title(namespace)
        .subscription(subscription)
        .theme(theme)
        .run()
}

fn namespace(_s: &Toaster, _id: window::Id) -> String {
    "mde-notify-toast".to_string()
}

fn theme(_s: &Toaster, _id: window::Id) -> cosmic::iced::Theme {
    let p = Palette::dark();
    cosmic::iced::Theme::custom(
        "MDE Notification Toast".to_string(),
        cosmic::iced::theme::Palette {
            background: p.background.into_cosmic_color(),
            text: p.text.into_cosmic_color(),
            primary: p.accent.into_cosmic_color(),
            success: p.success.into_cosmic_color(),
            warning: p.warning.into_cosmic_color(),
            danger: p.danger.into_cosmic_color(),
        },
    )
}

/// One on-screen toast: the alert + the wall-clock instant it appeared.
#[derive(Debug, Clone)]
struct Toast {
    item: AlertItem,
    shown_at_ms: i64,
}

struct Toaster {
    tail: AlertTail,
    toasts: Vec<Toast>,
}

impl Toaster {
    fn new() -> Self {
        Self {
            tail: AlertTail::default(),
            toasts: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
enum Message {
    /// Periodic bus poll for fresh alerts.
    Poll,
    /// Fast animation/expiry tick (only while toasts are live).
    Anim,
}

fn subscription(state: &Toaster) -> Subscription<Message> {
    let poll =
        cosmic::iced::time::every(std::time::Duration::from_secs(POLL_SECS)).map(|_| Message::Poll);
    // Adaptive budget: the fade tick runs ONLY while toasts are on screen.
    if state.toasts.is_empty() {
        poll
    } else {
        Subscription::batch([
            poll,
            cosmic::iced::time::every(std::time::Duration::from_millis(ANIM_MS))
                .map(|_| Message::Anim),
        ])
    }
}

/// Boot: spawn a Top-layer, non-interactive, non-exclusive surface anchored
/// top-right (overlays content; never steals focus or blocks clicks).
fn boot_task() -> Task<Message> {
    let id = window::Id::unique();
    Task::batch([
        get_layer_surface(SctkLayerSurfaceSettings {
            id,
            namespace: "mde-notify-toast".to_string(),
            size: Some((Some(TOAST_WIDTH as u32), None)),
            // Not exclusive — toasts float over content, they don't reserve a strut.
            exclusive_zone: 0,
            anchor: Anchor::TOP.union(Anchor::RIGHT),
            layer: Layer::Top,
            // Non-interactive: a toast must never take focus or eat clicks.
            keyboard_interactivity: KeyboardInteractivity::None,
            ..Default::default()
        }),
        Task::done(Message::Poll),
    ])
}

/// True when an alert should pop a toast. Mesh alerts toast; desktop-app
/// (`fdo/*`) alerts are shown in the center only (never double-toasted). When
/// DND is active, ordinary alerts are suppressed — only Critical bypasses (a
/// genuine emergency still reaches the operator).
#[must_use]
pub fn should_toast(item: &AlertItem, dnd_active: bool) -> bool {
    if item.source == Source::DesktopApp {
        return false;
    }
    if dnd_active && item.severity != Severity::Critical {
        return false;
    }
    true
}

/// Drop toasts whose TTL has elapsed.
#[must_use]
fn prune_expired(toasts: Vec<Toast>, now_ms: i64) -> Vec<Toast> {
    toasts
        .into_iter()
        .filter(|t| now_ms - t.shown_at_ms < TOAST_TTL_MS)
        .collect()
}

/// Opacity (0.0..=1.0) for a toast at `age_ms` into its TTL: ramp up over
/// `FADE_IN_MS`, hold, ramp down over the final `FADE_OUT_MS`. Pure + testable.
#[must_use]
pub fn toast_alpha(age_ms: i64, ttl_ms: i64) -> f32 {
    if age_ms <= 0 {
        return 0.0;
    }
    if age_ms < FADE_IN_MS {
        return age_ms as f32 / FADE_IN_MS as f32;
    }
    let fade_out_start = ttl_ms - FADE_OUT_MS;
    if age_ms >= fade_out_start && fade_out_start > 0 {
        let remaining = (ttl_ms - age_ms).max(0) as f32;
        return (remaining / FADE_OUT_MS as f32).clamp(0.0, 1.0);
    }
    1.0
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn update(state: &mut Toaster, message: Message) -> Task<Message> {
    let now = now_ms();
    match message {
        Message::Poll => {
            // DND state gates ordinary alerts (Critical bypasses).
            let dir = mde_bus::client_data_dir();
            let dnd_active = dir
                .as_ref()
                .map(|d| mde_bus::dnd::load_default(d).active)
                .unwrap_or(false);
            // NOTIFY-5 — per-group sound preferences (shared YAML next to the bus).
            let sound = dir
                .as_ref()
                .map(|d| SoundSettings::load(d))
                .unwrap_or_default();
            // Pull fresh alerts off the shared bus tail (Persist is !Send — open
            // + drop within this call, never across an await).
            if let Some(dir) = dir {
                if let Ok(persist) = mde_bus::persist::Persist::open(dir) {
                    let fresh = state.tail.poll(&persist);
                    for item in fresh {
                        if should_toast(&item, dnd_active) {
                            // NOTIFY-5 — play the severity sound (DND/mute-gated).
                            if let Some(name) = sound_for_alert(&sound, &item, dnd_active) {
                                play_sound(name);
                            }
                            state.toasts.push(Toast {
                                item,
                                shown_at_ms: now,
                            });
                        }
                    }
                }
            }
            // Keep only the newest MAX_TOASTS, and drop expired.
            state.toasts = prune_expired(std::mem::take(&mut state.toasts), now);
            if state.toasts.len() > MAX_TOASTS {
                let drop = state.toasts.len() - MAX_TOASTS;
                state.toasts.drain(0..drop);
            }
        }
        Message::Anim => {
            state.toasts = prune_expired(std::mem::take(&mut state.toasts), now);
        }
    }
    Task::none()
}

/// NOTIFY-5 — play a freedesktop XDG sound-theme sound by name. Prefers
/// `canberra-gtk-play` (theme-aware); falls back to `paplay` of the matching
/// `/usr/share/sounds/freedesktop/stereo/<name>.oga`. Fire-and-forget +
/// best-effort: a missing player or sound is a silent no-op (never blocks the
/// UI thread — the child is detached and not awaited).
fn play_sound(name: &str) {
    use std::process::{Command, Stdio};
    let quiet = |c: &mut Command| {
        c.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
    };
    let mut canberra = Command::new("canberra-gtk-play");
    canberra.args(["-i", name]);
    quiet(&mut canberra);
    if canberra.spawn().is_ok() {
        return;
    }
    let oga = format!("/usr/share/sounds/freedesktop/stereo/{name}.oga");
    if std::path::Path::new(&oga).exists() {
        let mut paplay = Command::new("paplay");
        paplay.arg(&oga);
        quiet(&mut paplay);
        let _ = paplay.spawn();
    }
}

/// One-glyph severity marker (matches the center).
fn severity_glyph(s: Severity) -> &'static str {
    match s {
        Severity::Critical => "●",
        Severity::Warning => "◐",
        Severity::Info => "○",
        Severity::Success => "✓",
    }
}

fn view(state: &Toaster, _id: window::Id) -> Element<'_, Message> {
    let p = Palette::dark();
    let now = now_ms();
    let mut col = column![].spacing(8).width(Length::Fixed(TOAST_WIDTH));
    // Newest at the top.
    for t in state.toasts.iter().rev() {
        let age = now - t.shown_at_ms;
        let alpha = toast_alpha(age, TOAST_TTL_MS);
        col = col.push(toast_card(&t.item, alpha, p));
    }
    container(col)
        .padding(Padding::from([12u16, 12u16]))
        .width(Length::Fill)
        .into()
}

/// Render one toast card at the given opacity. Severity accents the left border
/// + glyph; the whole card fades via `alpha`.
fn toast_card(item: &AlertItem, alpha: f32, p: Palette) -> Element<'static, Message> {
    let fade = |c: Color| Color {
        a: c.a * alpha,
        ..c
    };
    let sev = severity_token(item.severity, &p).into_cosmic_color();
    let host = item.host.clone().unwrap_or_default();
    let title = if host.is_empty() {
        item.title.clone()
    } else {
        format!("{}  ·  {host}", item.title)
    };
    let head = row![
        text(severity_glyph(item.severity))
            .size(13)
            .color(fade(sev)),
        Space::new().width(Length::Fixed(8.0)),
        text(title).size(13).color(fade(p.text.into_cosmic_color())),
    ]
    .align_y(cosmic::iced::Alignment::Center);
    let mut inner = column![head].spacing(2);
    if !item.body.is_empty() {
        let body: String = item.body.chars().take(140).collect();
        inner = inner.push(
            text(body)
                .size(11)
                .color(fade(p.text_muted.into_cosmic_color())),
        );
    }
    let surface = fade(p.surface.into_cosmic_color());
    let border = fade(sev);
    container(inner)
        .padding(Padding::from([10u16, 12u16]))
        .width(Length::Fill)
        .style(move |_| container::Style {
            snap: false,
            background: Some(Background::Color(surface)),
            text_color: None,
            border: Border {
                color: border,
                width: 2.0,
                radius: 0.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(src: Source, sev: Severity) -> AlertItem {
        AlertItem {
            id: "x".into(),
            ts_unix_ms: 0,
            severity: sev,
            source: src,
            topic: "t".into(),
            host: None,
            title: "t".into(),
            body: String::new(),
            read: false,
        }
    }

    #[test]
    fn mesh_alert_toasts_when_dnd_off() {
        assert!(should_toast(
            &item(Source::Security, Severity::Warning),
            false
        ));
        assert!(should_toast(&item(Source::Firewall, Severity::Info), false));
    }

    #[test]
    fn desktop_app_never_toasts() {
        // fdo/* desktop notifications live in the center, not double-toasted.
        assert!(!should_toast(
            &item(Source::DesktopApp, Severity::Critical),
            false
        ));
    }

    #[test]
    fn dnd_suppresses_ordinary_but_not_critical() {
        assert!(!should_toast(
            &item(Source::Security, Severity::Warning),
            true
        ));
        assert!(!should_toast(&item(Source::System, Severity::Info), true));
        // Critical bypasses DND — a genuine emergency reaches the operator.
        assert!(should_toast(
            &item(Source::Security, Severity::Critical),
            true
        ));
    }

    #[test]
    fn prune_drops_expired_keeps_live() {
        let toasts = vec![
            Toast {
                item: item(Source::System, Severity::Info),
                shown_at_ms: 0,
            },
            Toast {
                item: item(Source::System, Severity::Info),
                shown_at_ms: 5_000,
            },
        ];
        let live = prune_expired(toasts, 6_500);
        // The first (age 6500 >= TTL 6000) is dropped; the second (age 1500) stays.
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].shown_at_ms, 5_000);
    }

    #[test]
    fn alpha_fades_in_holds_and_out() {
        assert_eq!(toast_alpha(0, TOAST_TTL_MS), 0.0);
        assert!(toast_alpha(FADE_IN_MS / 2, TOAST_TTL_MS) > 0.0);
        assert!(toast_alpha(FADE_IN_MS / 2, TOAST_TTL_MS) < 1.0);
        assert_eq!(toast_alpha(2_000, TOAST_TTL_MS), 1.0); // hold
                                                           // Near the end it fades back toward 0.
        assert!(toast_alpha(TOAST_TTL_MS - 100, TOAST_TTL_MS) < 1.0);
        assert!(toast_alpha(TOAST_TTL_MS, TOAST_TTL_MS) <= 0.01);
    }
}
