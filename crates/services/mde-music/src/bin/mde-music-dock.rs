//! `mde-music-dock` — the layer-shell bottom dock for MDE Music (MUSIC-DOCK-1..3).
//!
//! A wlr-layer-shell **Overlay** surface anchored to the **bottom** edge with
//! **no exclusive zone**, so the now-playing dock floats over other windows
//! instead of reserving a strut (MUSIC-DOCK-1). On show it **slides in** from
//! below using the shipped `mde_theme::animation` helpers via
//! [`mde_music::motion::DockMotion`] — reduce-motion aware (MUSIC-DOCK-2). It
//! can **minimize to a handle** pill and restore on click (MUSIC-DOCK-3).
//!
//! This is glue: the live now-playing state + the play/pause/skip transport are
//! the existing `mde_music::nowplaying` Bus client talking to the `mde-musicd`
//! daemon (the Q96 Bus-canonical path); the dock just renders that state and
//! issues the same transport verbs the maxi player uses. The surface plumbing
//! mirrors `mde-voice-hud` (the workspace's reference layer-shell HUD).
//!
//! Built on libcosmic's vendored iced fork (the EFF-35 pinned rev) and its
//! native wlr-layer-shell commands — the same toolkit the windowed `mde-music`
//! binary uses.

#![forbid(unsafe_code)]

use std::time::Instant;

use cosmic::iced::platform_specific::runtime::wayland::layer_surface::{
    IcedMargin, SctkLayerSurfaceSettings,
};
use cosmic::iced::platform_specific::shell::commands::layer_surface::{
    get_layer_surface, Anchor, KeyboardInteractivity, Layer,
};
use cosmic::iced::widget::{button, column, container, row, text, Space};
use cosmic::iced::{window, Background, Border, Color, Length, Padding, Subscription, Task};
use cosmic::{Element, Theme};

use mde_music::motion::{DockMode, DockMotion, DockTrack, TICK};
use mde_music::nowplaying;

/// MUSIC-DOCK-1 §size — the expanded dock is a slim bottom bar; the handle is a
/// small restore pill. These are surface dimensions (Wayland geometry), distinct
/// from the in-content metrics, which come from the `mde_theme` spacing tokens.
const DOCK_HEIGHT: u32 = 72;
/// Clearance above the screen's bottom edge so the dock floats just over the
/// system panel/dock instead of hugging the very edge.
const MARGIN_BOTTOM: i32 = 8;

fn main() -> Result<(), cosmic::iced::Error> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("MDE_MUSIC_DOCK_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("mde_music=info,warn")),
        )
        .json()
        .init();

    cosmic::iced::daemon(
        || {
            let now = Instant::now();
            // MOTION-CORE-2 — honour the user's reduce-motion preference (file +
            // the MDE_REDUCE_MOTION env override) for the slide-in.
            let reduce_motion = mde_theme::prefs::Preferences::load().a11y.reduce_motion;
            (
                Dock {
                    motion: DockMotion::show(now, reduce_motion),
                    track: DockTrack::default(),
                },
                Task::batch([boot_surface(), Task::done(Message::Poll)]),
            )
        },
        update,
        view,
    )
    .title(namespace)
    .subscription(subscription)
    .theme(theme)
    .run()
}

/// MUSIC-DOCK-1 — create the bottom-anchored Overlay surface. **No exclusive
/// zone** (`exclusive_zone: 0`) so the dock floats over content rather than
/// reserving a strut; anchored BOTTOM and stretched the full width.
fn boot_surface() -> Task<Message> {
    get_layer_surface(SctkLayerSurfaceSettings {
        id: window::Id::unique(),
        namespace: "mde-music-dock".to_string(),
        // Width None → stretch to the output width; fixed dock height.
        size: Some((None, Some(DOCK_HEIGHT))),
        exclusive_zone: 0,
        margin: IcedMargin {
            top: 0,
            right: 0,
            bottom: MARGIN_BOTTOM,
            left: 0,
        },
        anchor: Anchor::BOTTOM.union(Anchor::LEFT).union(Anchor::RIGHT),
        layer: Layer::Overlay,
        // On-demand so the transport buttons take click focus, but the dock
        // never grabs the keyboard from the focused app.
        keyboard_interactivity: KeyboardInteractivity::OnDemand,
        ..Default::default()
    })
}

#[derive(Debug, Clone)]
enum Message {
    /// Animation frame tick while the slide is in flight.
    Tick,
    /// Poll the live now-playing snapshot from the daemon.
    Poll,
    /// The now-playing snapshot resolved (state + resolved title/artist).
    Loaded(DockTrack),
    /// MUSIC-DOCK-3 — minimize to / restore from the handle.
    ToggleMinimized,
    /// Transport — drives the same `action/music/*` verbs as the maxi player.
    PlayPause,
    SkipNext,
    SkipPrev,
    /// A transport verb finished; re-poll for the truth.
    TransportDone,
    /// Close the dock surface.
    Exit,
}

struct Dock {
    /// The slide-in + minimize-to-handle state. Carries the resolved
    /// reduce-motion preference internally (so a restore re-arms correctly).
    motion: DockMotion,
    /// The live now-playing snapshot rendered in the bar.
    track: DockTrack,
}

fn namespace(_state: &Dock, _id: window::Id) -> String {
    "mde-music-dock".to_string()
}

/// Carbon Gray-100 dark theme (matches the windowed `mde-music`).
fn theme(_state: &Dock, _id: window::Id) -> Theme {
    cosmic::Theme::dark()
}

/// Convert an `mde_theme` Carbon token to the fork's `Color` at alpha `a`. The
/// single sanctioned channel-math spot so every call site stays on a token (§4),
/// mirroring the windowed binary's `carbon` helper.
fn carbon(rgba: mde_theme::Rgba, a: f32) -> Color {
    Color {
        r: f32::from(rgba.r) / 255.0,
        g: f32::from(rgba.g) / 255.0,
        b: f32::from(rgba.b) / 255.0,
        a,
    }
}

fn update(state: &mut Dock, message: Message) -> Task<Message> {
    match message {
        Message::Tick => Task::none(),
        Message::Poll => Task::perform(fetch_track(), Message::Loaded),
        Message::Loaded(track) => {
            state.track = track;
            Task::none()
        }
        Message::ToggleMinimized => {
            // The dock keeps one fixed-height surface for its lifetime; expanded
            // vs. handle is rendered *inside* it (the body fades to a centered
            // pill), so there's no live surface resize — which also avoids a
            // resize round-trip re-triggering the slide.
            let now = Instant::now();
            state.motion = state.motion.toggle_minimized(now);
            Task::none()
        }
        Message::PlayPause => {
            let was = state.track.playing;
            state.track.playing = !was; // optimistic flip
            Task::perform(nowplaying::play_pause(was), |_| Message::TransportDone)
        }
        Message::SkipNext => {
            state.track.playing = true;
            Task::perform(nowplaying::skip_next(), |_| Message::TransportDone)
        }
        Message::SkipPrev => {
            state.track.playing = true;
            Task::perform(nowplaying::skip_prev(), |_| Message::TransportDone)
        }
        Message::TransportDone => Task::perform(fetch_track(), Message::Loaded),
        Message::Exit => std::process::exit(0),
    }
}

/// MUSIC-DOCK glue — pull the live transport snapshot from the daemon and
/// resolve the current song to a title/artist, flattening both into a
/// [`DockTrack`] the view renders. All over the canonical Bus path.
async fn fetch_track() -> DockTrack {
    let state = nowplaying::fetch_state().await.unwrap_or_default();
    let (title, artist) = if state.song_id.is_empty() {
        (String::new(), String::new())
    } else {
        nowplaying::resolve_song(state.song_id.clone())
            .await
            .unwrap_or_default()
    };
    let (_cover, duration_ms) = if state.song_id.is_empty() {
        (None, 0)
    } else {
        nowplaying::resolve_now_meta(state.song_id.clone())
            .await
            .unwrap_or((None, 0))
    };
    DockTrack {
        title,
        artist,
        playing: state.playing,
        has_track: state.has_track(),
        position_ms: state.position_ms,
        duration_ms,
    }
}

fn subscription(state: &Dock) -> Subscription<Message> {
    let now = Instant::now();
    // Poll the now-playing snapshot on the same cadence the windowed app uses.
    let poll = cosmic::iced::time::every(nowplaying::POLL).map(|_| Message::Poll);
    // MOTION-PERF-1 — only run the 60 fps animation clock while the slide is in
    // flight; at rest there are zero idle wakeups.
    if state.motion.is_animating(now) {
        Subscription::batch([poll, cosmic::iced::time::every(TICK).map(|_| Message::Tick)])
    } else {
        poll
    }
}

fn view(state: &Dock, _id: window::Id) -> Element<'_, Message> {
    let space = mde_theme::Space::for_density(mde_theme::Density::default());
    let palette = mde_theme::Palette::dark();
    let now = Instant::now();

    if state.motion.mode() == DockMode::Handle {
        return handle_view(&space, palette);
    }

    // MUSIC-DOCK-2 — slide-in: the eased params drive the body's alpha + a
    // bottom-padding offset (the fork has no transform widget, so we offset via
    // padding, which is compositor-cheap and never thrashes layout).
    let p = state.motion.params(now);
    let alpha = p.alpha;
    let drop = p.translate_y.max(0.0);

    let title = text(state.track.primary_line().to_string())
        .size(15.0)
        .class(cosmic::theme::iced::Text::Color(carbon(
            palette.text,
            alpha,
        )));
    let artist =
        text(state.track.artist.clone())
            .size(12.0)
            .class(cosmic::theme::iced::Text::Color(carbon(
                palette.text_muted,
                alpha,
            )));
    let meta = column![title, artist].spacing(f32::from(space.xs2));

    let transport = row![
        transport_button("⏮", Message::SkipPrev, palette, alpha, &space),
        transport_button(
            if state.track.playing { "⏸" } else { "▶" },
            Message::PlayPause,
            palette,
            alpha,
            &space,
        ),
        transport_button("⏭", Message::SkipNext, palette, alpha, &space),
    ]
    .spacing(f32::from(space.xs));

    let minimize = transport_button("▾", Message::ToggleMinimized, palette, alpha, &space);
    let close = transport_button("✕", Message::Exit, palette, alpha, &space);

    let bar = row![
        meta,
        Space::new().width(Length::Fill),
        transport,
        Space::new().width(Length::Fixed(f32::from(space.md))),
        minimize,
        close,
    ]
    .align_y(cosmic::iced::Alignment::Center)
    .spacing(f32::from(space.sm))
    .width(Length::Fill);

    // MUSIC-DOCK — a slim playback progress track under the bar: the accent fill
    // spans `progress()` of the width, the rest is the muted track. Fed by the
    // live position/duration the daemon reports (the `resolve_now_meta` round
    // trip in `fetch_track`), so the dock shows where the track is.
    let progress = dock_progress(state.track.progress(), palette, alpha);

    let stacked = column![bar, progress]
        .spacing(f32::from(space.xs2))
        .width(Length::Fill);

    let body = container(stacked)
        .width(Length::Fill)
        .height(Length::Fill)
        // The slide offset: push the body down by `drop` px (eases to 0).
        .padding(Padding {
            top: drop,
            right: f32::from(space.lg),
            bottom: 0.0,
            left: f32::from(space.lg),
        })
        .align_y(cosmic::iced::alignment::Vertical::Center)
        .style(move |_| cosmic::iced::widget::container::Style {
            background: Some(Background::Color(carbon(palette.surface, alpha))),
            border: Border {
                radius: f32::from(space.sm).into(),
                width: 1.0,
                color: carbon(palette.border, alpha),
            },
            ..Default::default()
        });

    container(body)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// MUSIC-DOCK-3 — the collapsed restore handle: a small centered pill; one click
/// expands the dock.
fn handle_view<'a>(space: &mde_theme::Space, palette: mde_theme::Palette) -> Element<'a, Message> {
    let radius = f32::from(space.md);
    let pill = button(
        text("♪  Music")
            .size(12.0)
            .class(cosmic::theme::iced::Text::Color(carbon(palette.text, 1.0))),
    )
    .on_press(Message::ToggleMinimized)
    .padding(Padding {
        top: f32::from(space.xs2),
        right: f32::from(space.md),
        bottom: f32::from(space.xs2),
        left: f32::from(space.md),
    })
    .class(cosmic::theme::iced::Button::Custom(Box::new(
        move |_t, _status| cosmic::iced::widget::button::Style {
            background: Some(Background::Color(carbon(palette.surface, 1.0))),
            text_color: carbon(palette.text, 1.0),
            border: Border {
                radius: radius.into(),
                width: 1.0,
                color: carbon(palette.border, 1.0),
            },
            ..Default::default()
        },
    )));

    container(pill)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(cosmic::iced::alignment::Horizontal::Center)
        .align_y(cosmic::iced::alignment::Vertical::Center)
        .into()
}

/// MUSIC-DOCK — the slim playback progress track: an accent fill spanning
/// `progress` (0.0..=1.0) of the width over a muted remainder. Built from
/// `Length::FillPortion` so it reflows with the dock width. A 2 px-tall hairline.
fn dock_progress<'a>(
    progress: f32,
    palette: mde_theme::Palette,
    alpha: f32,
) -> Element<'a, Message> {
    const BAR_HEIGHT: f32 = 2.0;
    // Integer portions for the fill/rest split (× 1000 keeps sub-percent
    // resolution); guard the zero case so an empty fill still lays out.
    let filled = (progress.clamp(0.0, 1.0) * 1000.0).round() as u16;
    let rest = 1000u16.saturating_sub(filled).max(1);

    let played = container(Space::new())
        .height(Length::Fixed(BAR_HEIGHT))
        .width(Length::FillPortion(filled.max(1)))
        .style(move |_| cosmic::iced::widget::container::Style {
            background: Some(Background::Color(carbon(palette.accent, alpha))),
            ..Default::default()
        });
    let remaining = container(Space::new())
        .height(Length::Fixed(BAR_HEIGHT))
        .width(Length::FillPortion(rest))
        .style(move |_| cosmic::iced::widget::container::Style {
            background: Some(Background::Color(carbon(palette.border, alpha))),
            ..Default::default()
        });

    // When nothing has played yet, show the whole track as the muted rail so the
    // (zero-width) accent fill doesn't render a stray sliver.
    if filled == 0 {
        remaining.width(Length::Fill).into()
    } else {
        row![played, remaining].width(Length::Fill).into()
    }
}

/// A flat transport glyph button; transparent idle, accent text. Alpha threads
/// the slide-in fade so the controls fade in with the bar.
fn transport_button<'a>(
    glyph: &'a str,
    msg: Message,
    palette: mde_theme::Palette,
    alpha: f32,
    space: &mde_theme::Space,
) -> Element<'a, Message> {
    button(
        text(glyph)
            .size(18.0)
            .class(cosmic::theme::iced::Text::Color(carbon(
                palette.text,
                alpha,
            ))),
    )
    .on_press(msg)
    .padding(Padding {
        top: f32::from(space.xs2),
        right: f32::from(space.sm),
        bottom: f32::from(space.xs2),
        left: f32::from(space.sm),
    })
    .class(cosmic::theme::iced::Button::Custom(Box::new(
        move |_t, _status| cosmic::iced::widget::button::Style {
            background: Some(Background::Color(carbon(palette.accent, 0.0))),
            text_color: carbon(palette.text, alpha),
            ..Default::default()
        },
    )))
    .into()
}
