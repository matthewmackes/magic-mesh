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
//! Adaptive motion budget (MOTION-PERF-1): the fast animation tick only runs
//! while a toast is mid-transition (sliding/fading in or out); during the steady
//! hold — and on an idle mesh — only the 2 s bus poll runs, so a settled toast
//! costs no per-frame wakeups.
//!
//! NOTIFY-FX-1 / MOTION-FEEDBACK-3 — the enter/exit motion is glued onto the
//! shared shell vocabulary in [`mde_theme::animation`] (`slide_in` +
//! `Transition` over a reduce-motion-aware [`mde_theme::animation::Tween`]) and
//! the Carbon [`mde_theme::motion`] duration grid, so the toast reads with the
//! same idiom as the Notification Hub's NOTIFY-HUB-2 card entrance — never a
//! hand-rolled fade. Reduce-motion collapses both directions to an instant
//! crossfade (opacity only, no slide), the helpers' a11y contract.

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
use mde_theme::animation::{ease, lerp_f32, Transition, Tween};
use mde_theme::motion::Motion;
use mde_theme::Palette;
use mde_workbench::cosmic_compat::IntoIcedColor;

/// Toast column width (px).
const TOAST_WIDTH: f32 = 380.0;
/// Bus poll cadence — a new alert toasts within this window of a publish.
const POLL_SECS: u64 = 2;
/// Fast animation tick (~60 fps) — registered ONLY while a toast is mid-enter or
/// mid-exit transition (MOTION-PERF-1: no idle wakeups during the hold).
const ANIM_MS: u64 = 16;
/// How long a toast stays on screen before it expires.
const TOAST_TTL_MS: i64 = 6_000;
/// NOTIFY-FX-1 — enter (slide/fade-in) duration: the Carbon `moderate-02` beat
/// the shared `slide_in` helper + the Hub's NOTIFY-HUB-2 card entrance both use,
/// so the toast and the Hub share one entrance feel.
fn enter_ms() -> i64 {
    Motion::panel_mount().duration.as_millis() as i64
}
/// NOTIFY-FX-1 — exit (slide/fade-out) duration: the Carbon `moderate-02` beat,
/// kept symmetric with the entrance. The fade-out begins this long before the
/// TTL elapses so the toast finishes leaving exactly as it's pruned.
fn exit_ms() -> i64 {
    Motion::dialog_mount().duration.as_millis() as i64
}
/// NOTIFY-HUB-2 idiom — a fresh toast slides in this many px from the right (and
/// a leaving one slides back out the same way), echoing the Hub card's
/// horizontal entrance. Component dimension (the toast column is 380 px wide),
/// so a local constant, not a density-scaled metric. Reduce-motion drops it.
const SLIDE_PX: f32 = 36.0;
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
    /// NOTIFY-FX-1 — honor the user's reduce-motion preference (read once at
    /// boot, like the Hub): collapses every enter/exit to an instant crossfade
    /// (opacity only, no slide) via the shared helpers' a11y contract.
    reduce_motion: bool,
}

impl Toaster {
    fn new() -> Self {
        Self {
            tail: AlertTail::default(),
            toasts: Vec::new(),
            reduce_motion: mde_theme::Preferences::load().a11y.reduce_motion,
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
    // MOTION-PERF-1: the fast animation tick runs ONLY while at least one toast
    // is actually mid-transition (entering or leaving). A toast sitting in its
    // steady hold has no per-frame visual change, so it needs no wakeups — the
    // 2 s poll re-arms the tick the moment a toast crosses into its fade-out.
    let now = now_ms();
    let in_flight = state
        .toasts
        .iter()
        .any(|t| toast_in_transition(now - t.shown_at_ms, state.reduce_motion));
    if in_flight {
        Subscription::batch([
            poll,
            cosmic::iced::time::every(std::time::Duration::from_millis(ANIM_MS))
                .map(|_| Message::Anim),
        ])
    } else {
        poll
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
/// (`fdo/*`) alerts are shown in the center only (never double-toasted). SELinux
/// AVC denials (`fleet/sec/selinux/*`) are recorded in the Security group but
/// never toast below Critical — they are noisy, repeat across restarts, and are
/// explicitly not latency-critical (the selinux_monitor worker), so a benign
/// permissive/warning denial must not pop a toast on every reboot. When DND is
/// active, ordinary alerts are suppressed — only Critical bypasses (a genuine
/// emergency still reaches the operator).
#[must_use]
pub fn should_toast(item: &AlertItem, dnd_active: bool) -> bool {
    if item.source == Source::DesktopApp {
        return false;
    }
    if is_selinux_denial(&item.topic) && item.severity != Severity::Critical {
        return false;
    }
    if dnd_active && item.severity != Severity::Critical {
        return false;
    }
    true
}

/// SELinux AVC denials publish to `fleet/sec/selinux/<host>`; classified as the
/// Security group but kept out of the toast stream (center-only) below Critical.
#[must_use]
fn is_selinux_denial(topic: &str) -> bool {
    topic.trim().starts_with("fleet/sec/selinux/")
}

/// Drop toasts whose TTL has elapsed.
#[must_use]
fn prune_expired(toasts: Vec<Toast>, now_ms: i64) -> Vec<Toast> {
    toasts
        .into_iter()
        .filter(|t| now_ms - t.shown_at_ms < TOAST_TTL_MS)
        .collect()
}

/// NOTIFY-FX-1 — the render motion for a toast at `age_ms` into its TTL: a
/// fade+slide-in entrance, a steady hold, then a fade+slide-out exit. Positive
/// `translate_x` = still offset to the right of rest (slides to 0 on enter, back
/// out on exit), matching the Hub's NOTIFY-HUB-2 "from the right" idiom.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToastMotion {
    /// Opacity multiplier `0.0..=1.0`.
    pub alpha: f32,
    /// Horizontal offset in px (positive = right of rest).
    pub translate_x: f32,
}

/// `true` while a toast at `age_ms` needs the fast animation tick armed — i.e.
/// it is entering, exiting, or about to exit. The fast tick is only re-evaluated
/// when the 2 s poll fires, so it must be armed at least one poll interval BEFORE
/// the exit window opens; otherwise a narrow exit window that opens and closes
/// entirely between two polls would never be ticked and the toast would vanish
/// abruptly instead of fading/sliding out. The steady mid-hold (well clear of
/// both the entrance and the imminent exit) returns `false` so a settled toast
/// costs no wakeups (MOTION-PERF-1).
#[must_use]
fn toast_in_transition(age_ms: i64, reduce_motion: bool) -> bool {
    let enter = if reduce_motion {
        mde_theme::motion::REDUCE_MOTION_CAP_MS as i64
    } else {
        enter_ms()
    };
    let exit = if reduce_motion {
        mde_theme::motion::REDUCE_MOTION_CAP_MS as i64
    } else {
        exit_ms()
    };
    // Arm the tick one poll interval ahead of the exit window so the poll that
    // re-evaluates the subscription always lands BEFORE the fade-out begins — the
    // exit is never skipped, however the polls align with the TTL. A negative age
    // (clock skew) counts as entering so the toast still animates in.
    let exit_arm = TOAST_TTL_MS - exit - (POLL_SECS as i64 * 1_000);
    age_ms < enter || age_ms >= exit_arm
}

/// NOTIFY-FX-1 — the toast's enter/exit motion at `age_ms`, glued onto the
/// shared shell vocabulary: the entrance reuses [`mde_theme::animation::slide_in`]
/// (fade 0→1 + slide in, reduce-motion ⇒ crossfade) and the exit drives
/// [`Transition::FadeOut`] over a reduce-motion-aware [`Tween`] (the symmetric
/// slide back out, reduce-motion ⇒ opacity-only). Both use the Carbon
/// `moderate-02` beat. Pure + testable.
#[must_use]
pub fn toast_motion(age_ms: i64, reduce_motion: bool) -> ToastMotion {
    // Synthetic shared clock: the helpers are `(start, now)`-relative, so a fixed
    // epoch + an offset of `age_ms` samples them at the toast's current age.
    let epoch = std::time::Instant::now();
    let at = |ms: i64| epoch + std::time::Duration::from_millis(ms.max(0) as u64);

    // Exit window first: once a toast is within `exit` of its TTL it fades + slides
    // back out (right). This takes precedence so a very short TTL still exits.
    let exit = if reduce_motion {
        mde_theme::motion::REDUCE_MOTION_CAP_MS as i64
    } else {
        exit_ms()
    };
    let exit_start = TOAST_TTL_MS - exit;
    if age_ms >= exit_start {
        // Progress 0→1 across the exit window, eased like the entrance.
        let tw = Tween::resolved(
            at(exit_start),
            std::time::Duration::from_millis(exit.max(1) as u64),
            reduce_motion,
        );
        let t = ease(tw.progress(at(age_ms)), Motion::dialog_mount().resolved(reduce_motion).easing);
        let p = Transition::FadeOut.params(t);
        // Reduce-motion: crossfade only (no slide). Full motion: slide back out
        // to +SLIDE_PX as it fades.
        let translate_x = if reduce_motion {
            0.0
        } else {
            lerp_f32(0.0, SLIDE_PX, t)
        };
        return ToastMotion {
            alpha: p.alpha,
            translate_x,
        };
    }

    // Entrance: fade 0→1 + slide in from +SLIDE_PX → 0 over `moderate-02`. The
    // shared `slide_in` helper carries the reduce-motion contract (it collapses
    // to a pure crossfade, zero translate).
    let p = mde_theme::animation::slide_in(at(0), at(age_ms), SLIDE_PX, reduce_motion);
    ToastMotion {
        alpha: p.alpha,
        translate_x: p.translate_y, // `slide_in` yields the offset in translate_y; map to x.
    }
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
        let motion = toast_motion(age, state.reduce_motion);
        col = col.push(toast_card(&t.item, motion, p));
    }
    container(col)
        .padding(Padding::from([12u16, 12u16]))
        .width(Length::Fill)
        .into()
}

/// Render one toast card with the given enter/exit [`ToastMotion`]. Severity
/// accents the left border + glyph; the whole card fades via `motion.alpha`, and
/// `motion.translate_x` slides it horizontally (applied as left padding — the
/// iced 0.13 fork has no transform widget, so offset via padding, the same
/// convention the shared motion helpers document).
fn toast_card(item: &AlertItem, motion: ToastMotion, p: Palette) -> Element<'static, Message> {
    let alpha = motion.alpha;
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
    let card = container(inner)
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
        });
    // Horizontal slide: leading spacer of `translate_x` px pushes the card right
    // of rest while it enters/exits, settling to 0 at the hold (reduce-motion ⇒
    // always 0). The card shrinks to fill the remainder so the column width is
    // stable (no layout thrash — the helpers' compositor-friendly contract).
    let offset = motion.translate_x.max(0.0);
    if offset > f32::EPSILON {
        row![Space::new().width(Length::Fixed(offset)), card].into()
    } else {
        card.into()
    }
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
    fn selinux_denials_record_but_never_toast_below_critical() {
        // A benign AVC denial (info/warning) lands in the Security group but must
        // not pop a toast — it repeats across reboots and is not latency-critical.
        let mut warn = item(Source::Security, Severity::Warning);
        warn.topic = "fleet/sec/selinux/UNIT-EAGLE".into();
        assert!(!should_toast(&warn, false));
        let mut info = item(Source::Security, Severity::Info);
        info.topic = "fleet/sec/selinux/fedora".into();
        assert!(!should_toast(&info, false));
        // A real (non-SELinux) security alert still toasts.
        let mut enrol = item(Source::Security, Severity::Warning);
        enrol.topic = "fleet/sec".into();
        assert!(should_toast(&enrol, false));
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
    fn motion_slides_and_fades_in_holds_then_exits() {
        // NOTIFY-FX-1 acceptance: a fresh toast starts transparent + offset to the
        // right, slides/fades in, holds opaque + at rest, then fades/slides back
        // out as it nears its TTL.
        // Entrance start: invisible, fully offset right.
        let m0 = toast_motion(0, false);
        assert!(m0.alpha < 1e-3, "starts transparent, got {}", m0.alpha);
        assert!(
            (m0.translate_x - SLIDE_PX).abs() < 1e-3,
            "starts at full right offset, got {}",
            m0.translate_x
        );
        // Mid-entrance: interpolating in.
        let mm = toast_motion(enter_ms() / 2, false);
        assert!(mm.alpha > 0.0 && mm.alpha < 1.0, "fading in, got {}", mm.alpha);
        assert!(
            mm.translate_x > 0.0 && mm.translate_x < SLIDE_PX,
            "sliding in, got {}",
            mm.translate_x
        );
        // Hold: fully opaque + at rest (no offset).
        let hold = toast_motion(2_000, false);
        assert!((hold.alpha - 1.0).abs() < 1e-3, "holds opaque, got {}", hold.alpha);
        assert!(hold.translate_x.abs() < 1e-3, "holds at rest, got {}", hold.translate_x);
        // Exit window: fading + sliding back out toward 0 alpha / +SLIDE_PX.
        let near_end = toast_motion(TOAST_TTL_MS - 100, false);
        assert!(near_end.alpha < 1.0, "exit fades out, got {}", near_end.alpha);
        assert!(near_end.translate_x > 0.0, "exit slides out, got {}", near_end.translate_x);
        // At the TTL the toast is fully gone.
        let gone = toast_motion(TOAST_TTL_MS, false);
        assert!(gone.alpha <= 0.01, "fully faded at TTL, got {}", gone.alpha);
    }

    #[test]
    fn reduce_motion_is_crossfade_only_no_slide() {
        // The a11y contract: reduce-motion keeps the fade (the state cue) but drops
        // every horizontal slide — translate_x is 0 across the whole lifecycle.
        for age in [0, 20, 40, 80, 2_000, TOAST_TTL_MS - 40, TOAST_TTL_MS] {
            let m = toast_motion(age, true);
            assert_eq!(m.translate_x, 0.0, "no slide under reduce-motion @{age}ms");
            assert!((0.0..=1.0).contains(&m.alpha), "alpha in range @{age}ms");
        }
        // It still fades: invisible at the start of the (capped) entrance, opaque
        // once past the cap.
        assert!(toast_motion(0, true).alpha < 1e-3);
        let cap = mde_theme::motion::REDUCE_MOTION_CAP_MS as i64;
        assert!((toast_motion(cap, true).alpha - 1.0).abs() < 1e-3);
    }

    #[test]
    fn transition_flag_arms_tick_only_while_moving() {
        // MOTION-PERF-1: the fast tick is armed while entering, exiting, or within
        // one poll interval of the exit window; the steady mid-hold reports no
        // transition in flight (so the tick can stop).
        assert!(toast_in_transition(0, false), "entering");
        assert!(toast_in_transition(enter_ms() - 1, false), "still entering");
        assert!(!toast_in_transition(2_000, false), "mid-hold has no transition");
        assert!(
            toast_in_transition(TOAST_TTL_MS - 1, false),
            "exiting near TTL"
        );
        // Regression: the tick must be armed at least one poll interval BEFORE the
        // exit window opens, so a fade-out that falls between two polls is never
        // skipped. Sample just before the exit window opens.
        let exit_open = TOAST_TTL_MS - exit_ms();
        assert!(
            toast_in_transition(exit_open - 1, false),
            "armed ahead of the exit window so the poll catches it"
        );
        assert!(
            toast_in_transition(exit_open - (POLL_SECS as i64 * 1_000), false),
            "armed a full poll interval before the exit window opens"
        );
    }
}
