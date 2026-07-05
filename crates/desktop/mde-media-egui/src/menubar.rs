//! MENUBAR-ALL-media — the **shared top menu bar** across the Media surface
//! (design: `docs/design/menubar-all.md`), hosted on the shared
//! [`mde_egui::menubar::MenuBar`] (MENUBAR-ALL-1).
//!
//! One slim strip under the UPPERCASE `MEDIA` title (accent-tinted with the dock's
//! Media hue, [`Style::ACCENT_MEDIA`]) that surfaces **every control the surface can
//! actually perform** — the governing principle (design §7): the menu bar is the
//! operator's complete, discoverable control surface, incl. the advanced transport
//! (frame-step / chapters / A-B loop / speed), the audio-processing modes, and track
//! selection. Every item is the **menu twin of an existing seam** (§6): a click maps
//! to one [`TransportAction`] the core [`MediaController::dispatch`] already handles,
//! or one UI-state / cast / party method the surface already owns — never a new
//! behaviour and never a stub. Per §7 (no dead entries) an item whose context is
//! missing renders **disabled** (Play with no media, Leave with no party) and a
//! genuinely-absent feature is **omitted**.
//!
//! The menus and the seam each drives:
//!
//! * **File** — Open Media Source… (jump to the Sources view, which hosts the real
//!   index-folder / Open-URL / Jellyfin / capture add fields), Save Snapshot
//!   ([`TransportAction::Snapshot`], gated on loaded media).
//! * **View** — the four sub-views ([`MediaTab`], checkmarked) + Fullscreen
//!   ([`UiState::fullscreen`]) + Mini Player ([`UiState::pip`]).
//! * **Playback** — Play/Pause · Stop · Prev/Next · Skip ±10s · Frame step · Chapter
//!   nav · a Speed submenu · A-B loop — every [`TransportAction`] transport verb, each
//!   context-gated (loaded / seekable / chaptered / queue-neighbours).
//! * **Audio** — the enumerated audio Track submenu ([`TransportAction::SelectTrack`]),
//!   Loudness + `ReplayGain` submenus, Gapless, and Reset Equalizer (MEDIA-3/5).
//! * **Subtitles** — the enumerated subtitle tracks ([`TransportAction::SelectTrack`]).
//! * **Cast** — Find Renderers ([`MediaController::discover_cast_targets`]) + the
//!   discovered targets ([`MediaController::cast_current`]) + Leave Watch-Together
//!   ([`MediaController::leave_party`], gated on a joined party) — the MEDIA-17 mesh
//!   cast + sync-play seams.
//!
//! **Honestly omitted** (no landed surface seam, so no dead entry): an **Edit** menu
//! (the surface has no clipboard/undo ops), a **Help** menu (no keymap / shortcuts
//! system — the transport is button-driven, so there are no live chords to show), a
//! **track-off / auto** entry (the surface's [`TransportAction::SelectTrack`] carries
//! only an explicit track id), and an **aspect-ratio** control (the core `VideoConfig`
//! seam is not wired into this surface's dispatch — a menu entry would be new
//! behaviour, not glue). The party **host/join** stays in the Player view's panel: it
//! needs a text field a menu can't hold, so a menu item would be a fake.
//!
//! §4: the shared [`MenuBar`] renders through the Carbon [`Style`] install — no forced
//! colours, so egui's disabled dimming reads correctly; the surface builds the menu
//! **model** each frame ([`build_menus`]) and dispatches the activated [`Picked`] id
//! ([`apply`]), so the render host is the only thing that changed.

use mde_egui::egui::Ui;
use mde_egui::menubar::{Entry, Item as BarItem, Menu, MenuBar as SharedMenuBar, MenuBarModel};
use mde_egui::{ChipTone, StatusChip, Style};

use mde_media_core::{
    AbLoop, AudioDriver, LoudnessNorm, MediaEngine, PlayerState, ReplayGainMode, ScreenshotMode,
    Track, TrackKind, TrackSelect,
};

use crate::model::{
    format_time, now_playing_title, play_pause_label, track_label, MediaController, MediaTab,
    TransportAction, EBU_R128_DEFAULT,
};

/// The playback-speed presets the Speed submenu offers — the same set the Player
/// view's inline speed row uses, so the menu twin drives the identical seam.
const SPEED_PRESETS: [f64; 5] = [0.5, 1.0, 1.25, 1.5, 2.0];

/// The relative-seek step (seconds) of the Skip-back / Skip-forward items — the menu
/// twin of the Player view's `⏪ 10s` / `10s ⏩` transport buttons.
const SKIP_SECS: f64 = 10.0;

// ─────────────────────────────── the action vocabulary ──────────────────────────────

/// One thing a Media menu item activates — every arm is exactly one existing surface
/// seam (§6 glue): a core [`TransportAction`], a [`UiState`](crate::model::UiState)
/// toggle, or a cast / party method. [`apply`] maps each to that seam; nothing here is
/// new behaviour.
#[derive(Clone, Debug, PartialEq)]
pub enum Picked {
    /// A core transport intent, dispatched through [`MediaController::dispatch`].
    Transport(TransportAction),
    /// Switch the active sub-view ([`UiState::tab`](crate::model::UiState::tab)).
    GoTab(MediaTab),
    /// Toggle immersive fullscreen ([`UiState::fullscreen`](crate::model::UiState::fullscreen)).
    ToggleFullscreen,
    /// Toggle the `PiP` mini-player ([`UiState::pip`](crate::model::UiState::pip)).
    TogglePip,
    /// Probe the network for cast renderers ([`MediaController::discover_cast_targets`]).
    DiscoverCast,
    /// Cast the current playback to the renderer with this id
    /// ([`MediaController::cast_current`]).
    Cast(String),
    /// Leave the joined watch-together party ([`MediaController::leave_party`]).
    LeaveParty,
}

/// Dispatch a [`Picked`] to its real seam (§6 — the render host changed, the seam did
/// not). The single mutable touch of the controller per activated frame.
pub fn apply<E: MediaEngine>(picked: Picked, controller: &mut MediaController<E>) {
    match picked {
        Picked::Transport(action) => controller.dispatch(action),
        Picked::GoTab(tab) => controller.ui_mut().tab = tab,
        Picked::ToggleFullscreen => {
            let now = controller.ui().fullscreen;
            controller.ui_mut().fullscreen = !now;
        }
        Picked::TogglePip => {
            let now = controller.ui().pip;
            controller.ui_mut().pip = !now;
        }
        Picked::DiscoverCast => controller.discover_cast_targets(),
        Picked::Cast(id) => controller.cast_current(&id),
        Picked::LeaveParty => controller.leave_party(),
    }
}

// ─────────────────────────────── the menu model ─────────────────────────────────────

/// A plain, always-enabled command item.
fn item(id: Picked, label: impl Into<String>) -> Entry<Picked> {
    Entry::Item(BarItem::new(id, label))
}

/// A command item behind a context gate (§7 — `enabled = false` greys it, never omits
/// its seam).
fn gated(id: Picked, label: impl Into<String>, enabled: bool) -> Entry<Picked> {
    Entry::Item(BarItem::new(id, label).enabled(enabled))
}

/// The File menu: the real "open / add media" entry point (the Sources view hosts every
/// add field) + Save Snapshot (an export = a File-menu-appropriate seam).
fn file_menu(loaded: bool) -> Menu<Picked> {
    Menu::new(
        "File",
        vec![
            item(
                Picked::GoTab(MediaTab::Sources),
                "Open Media Source\u{2026}",
            ),
            Entry::Separator,
            gated(
                Picked::Transport(TransportAction::Snapshot(ScreenshotMode::Subtitles)),
                "Save Snapshot",
                loaded,
            ),
        ],
    )
}

/// The View menu: the four sub-views (checkmarked to the live tab) + the display
/// toggles.
fn view_menu(tab: MediaTab, fullscreen: bool, pip: bool) -> Menu<Picked> {
    let mut entries: Vec<Entry<Picked>> = MediaTab::all()
        .into_iter()
        .map(|t| Entry::Item(BarItem::new(Picked::GoTab(t), t.label()).checked(t == tab)))
        .collect();
    entries.push(Entry::Separator);
    entries.push(Entry::Item(
        BarItem::new(Picked::ToggleFullscreen, "Fullscreen").checked(fullscreen),
    ));
    entries.push(Entry::Item(
        BarItem::new(Picked::TogglePip, "Mini Player").checked(pip),
    ));
    Menu::new("View", entries)
}

/// The Speed submenu — each preset checkmarked when it is the live rate.
fn speed_entries(speed: f64, loaded: bool) -> Vec<Entry<Picked>> {
    SPEED_PRESETS
        .into_iter()
        .map(|preset| {
            let selected = (speed - preset).abs() < f64::EPSILON;
            Entry::Item(
                BarItem::new(
                    Picked::Transport(TransportAction::SetSpeed(preset)),
                    format!("{preset}\u{00D7}"),
                )
                .enabled(loaded)
                .checked(selected),
            )
        })
        .collect()
}

/// The per-frame transport snapshot the Playback menu gates + labels from — the read
/// half of a render frame, so [`playback_menu`] is a pure fold over live state.
///
/// The transport is genuinely a cluster of independent boolean gates (each a distinct
/// §7 disable condition), so the many-bools shape is the honest model, not a flags
/// smell.
#[allow(clippy::struct_excessive_bools)]
struct PlaybackCtx {
    /// The live player state (drives the Play/Pause verb label).
    state: PlayerState,
    /// Media is loaded (Play/Pause/Stop/frame-step/snapshot gate).
    loaded: bool,
    /// The duration is known, so relative seeks are meaningful (skip gate).
    seekable: bool,
    /// The media is chaptered (chapter-nav gate).
    chaptered: bool,
    /// The queue has more than one item, so Prev/Next have somewhere to go.
    has_neighbours: bool,
    /// The live playback speed (the Speed submenu checkmark).
    speed: f64,
    /// An A-B loop range is applied (Clear gate).
    ab_active: bool,
    /// An A-loop mark is pending its B (Clear gate + the mark-A/B label).
    ab_pending: bool,
}

/// The Playback menu — the full transport verb set, each context-gated over a seam
/// that exists (§7 disable, not phasing).
fn playback_menu(cx: &PlaybackCtx) -> Menu<Picked> {
    let ab_label = if cx.ab_pending {
        "A-B Loop: mark B"
    } else {
        "A-B Loop: mark A"
    };
    Menu::new(
        "Playback",
        vec![
            gated(
                Picked::Transport(TransportAction::TogglePlay),
                play_pause_label(cx.state),
                cx.loaded,
            ),
            gated(Picked::Transport(TransportAction::Stop), "Stop", cx.loaded),
            Entry::Separator,
            gated(
                Picked::Transport(TransportAction::Prev),
                "Previous",
                cx.has_neighbours,
            ),
            gated(
                Picked::Transport(TransportAction::Next),
                "Next",
                cx.has_neighbours,
            ),
            Entry::Separator,
            gated(
                Picked::Transport(TransportAction::SeekBy(-SKIP_SECS)),
                "Skip Back 10s",
                cx.seekable,
            ),
            gated(
                Picked::Transport(TransportAction::SeekBy(SKIP_SECS)),
                "Skip Forward 10s",
                cx.seekable,
            ),
            Entry::Separator,
            gated(
                Picked::Transport(TransportAction::FrameBack),
                "Step Back Frame",
                cx.loaded,
            ),
            gated(
                Picked::Transport(TransportAction::FrameForward),
                "Step Forward Frame",
                cx.loaded,
            ),
            Entry::Separator,
            gated(
                Picked::Transport(TransportAction::ChapterPrev),
                "Previous Chapter",
                cx.chaptered,
            ),
            gated(
                Picked::Transport(TransportAction::ChapterNext),
                "Next Chapter",
                cx.chaptered,
            ),
            Entry::Separator,
            Entry::Submenu {
                label: "Speed".to_owned(),
                mnemonic: None,
                entries: speed_entries(cx.speed, cx.loaded),
            },
            Entry::Separator,
            gated(
                Picked::Transport(TransportAction::MarkAbLoop),
                ab_label,
                cx.loaded,
            ),
            gated(
                Picked::Transport(TransportAction::ClearAbLoop),
                "Clear A-B Loop",
                cx.ab_active || cx.ab_pending,
            ),
        ],
    )
}

/// The enumerated tracks of `kind` as check items (checkmarked to the live selection),
/// or an honest dim caption when none are enumerated yet (§7 — a caption, never a dead
/// item). The `empty` note names why: the surface has no tracks until media loads.
fn track_entries(
    tracks: &[Track],
    kind: TrackKind,
    selection: TrackSelect,
    empty: &str,
) -> Vec<Entry<Picked>> {
    let matching: Vec<&Track> = tracks.iter().filter(|t| t.kind == kind).collect();
    if matching.is_empty() {
        return vec![Entry::Caption(empty.to_owned())];
    }
    matching
        .into_iter()
        .map(|track| {
            let checked = selection == TrackSelect::Id(track.id);
            Entry::Item(
                BarItem::new(
                    Picked::Transport(TransportAction::SelectTrack(kind, track.id)),
                    track_label(track),
                )
                .checked(checked),
            )
        })
        .collect()
}

/// The Loudness submenu — Off / EBU R128 / Dynamic, checkmarked to the live mode.
fn loudness_entries(loudness: LoudnessNorm) -> Vec<Entry<Picked>> {
    vec![
        Entry::Item(
            BarItem::new(
                Picked::Transport(TransportAction::SetLoudness(LoudnessNorm::Off)),
                "Off",
            )
            .checked(loudness == LoudnessNorm::Off),
        ),
        Entry::Item(
            BarItem::new(
                Picked::Transport(TransportAction::SetLoudness(EBU_R128_DEFAULT)),
                "EBU R128",
            )
            .checked(matches!(loudness, LoudnessNorm::Ebu { .. })),
        ),
        Entry::Item(
            BarItem::new(
                Picked::Transport(TransportAction::SetLoudness(LoudnessNorm::Dynamic)),
                "Dynamic",
            )
            .checked(loudness == LoudnessNorm::Dynamic),
        ),
    ]
}

/// The `ReplayGain` submenu — Off / Track / Album, checkmarked to the live mode.
fn replaygain_entries(replaygain: ReplayGainMode) -> Vec<Entry<Picked>> {
    [
        (ReplayGainMode::Off, "Off"),
        (ReplayGainMode::Track, "Track"),
        (ReplayGainMode::Album, "Album"),
    ]
    .into_iter()
    .map(|(mode, label)| {
        Entry::Item(
            BarItem::new(
                Picked::Transport(TransportAction::SetReplayGain(mode)),
                label,
            )
            .checked(replaygain == mode),
        )
    })
    .collect()
}

/// The Audio menu (MEDIA-3/5): the enumerated audio Track submenu, the loudness /
/// `ReplayGain` submenus, the Gapless toggle, and Reset Equalizer. The continuous
/// 10-band EQ sliders stay in the Player view's Audio panel (a menu can't hold a
/// slider); the menu surfaces the discrete modes + the flatten seam.
fn audio_menu(
    tracks: &[Track],
    audio_selection: TrackSelect,
    loudness: LoudnessNorm,
    replaygain: ReplayGainMode,
    gapless: bool,
) -> Menu<Picked> {
    Menu::new(
        "Audio",
        vec![
            Entry::Submenu {
                label: "Track".to_owned(),
                mnemonic: None,
                entries: track_entries(
                    tracks,
                    TrackKind::Audio,
                    audio_selection,
                    "No audio tracks (load media)",
                ),
            },
            Entry::Separator,
            Entry::Submenu {
                label: "Loudness".to_owned(),
                mnemonic: None,
                entries: loudness_entries(loudness),
            },
            Entry::Submenu {
                label: "ReplayGain".to_owned(),
                mnemonic: None,
                entries: replaygain_entries(replaygain),
            },
            Entry::Item(
                BarItem::new(Picked::Transport(TransportAction::ToggleGapless), "Gapless")
                    .checked(gapless),
            ),
            Entry::Separator,
            item(
                Picked::Transport(TransportAction::ResetEq),
                "Reset Equalizer",
            ),
        ],
    )
}

/// The Subtitles menu — the enumerated subtitle tracks, checkmarked to the live
/// selection (or an honest caption when none).
fn subtitles_menu(tracks: &[Track], subtitle_selection: TrackSelect) -> Menu<Picked> {
    Menu::new(
        "Subtitles",
        track_entries(
            tracks,
            TrackKind::Subtitle,
            subtitle_selection,
            "No subtitle tracks",
        ),
    )
}

/// The Cast menu (MEDIA-17): the renderer probe + the discovered targets + Leave
/// Watch-Together. The cast list honest-gates on an empty probe (a caption, never a
/// fabricated device, §7); Leave is disabled with no joined party.
fn cast_menu<E: MediaEngine>(controller: &MediaController<E>, loaded: bool) -> Menu<Picked> {
    let mut entries = vec![item(Picked::DiscoverCast, "Find Renderers")];
    let targets = controller.cast().targets();
    if targets.is_empty() {
        let note = if controller.cast().probed() {
            "No cast renderer found on this network"
        } else {
            "Find Renderers to look"
        };
        entries.push(Entry::Caption(note.to_owned()));
    } else {
        entries.push(Entry::Caption(
            "Cast current playback to\u{2026}".to_owned(),
        ));
        for target in targets {
            entries.push(gated(
                Picked::Cast(target.id.clone()),
                format!("{} \u{00B7} {}", target.name, target.kind.label()),
                loaded,
            ));
        }
    }
    entries.push(Entry::Separator);
    entries.push(gated(
        Picked::LeaveParty,
        "Leave Watch-Together",
        controller.party_enabled(),
    ));
    Menu::new("Cast", entries)
}

/// Build the full ordered Media menu tree as the shared model, from an immutable read
/// of the controller (the read half of a render frame — pure + unit-testable without
/// egui).
fn build_menus<E: MediaEngine>(controller: &MediaController<E>) -> Vec<Menu<Picked>> {
    let player = controller.player();
    let loaded = player.media().is_some();
    let transport = PlaybackCtx {
        state: player.state(),
        loaded,
        seekable: player.duration().is_some(),
        chaptered: player.chapter_count().is_some(),
        has_neighbours: player.playlist().len() > 1,
        speed: player.controls().speed,
        ab_active: !matches!(player.controls().ab_loop, AbLoop::Off),
        ab_pending: controller.ui().ab_pending.is_some(),
    };
    let selection = player.track_selection();
    let audio_selection = selection.audio;
    let subtitle_selection = selection.subtitle;
    let tracks = controller.tracks();
    let audio = controller.audio_config();

    vec![
        file_menu(loaded),
        view_menu(
            controller.ui().tab,
            controller.ui().fullscreen,
            controller.ui().pip,
        ),
        playback_menu(&transport),
        audio_menu(
            tracks,
            audio_selection,
            audio.loudness,
            audio.replaygain,
            audio.gapless,
        ),
        subtitles_menu(tracks, subtitle_selection),
        cast_menu(controller, loaded),
    ]
}

// ─────────────────────────────── the status cluster ─────────────────────────────────

/// The audio-output label for the status cluster — the live [`AudioDriver`] (never a
/// raw ao string; reads the enum, mints no behaviour).
fn output_label(driver: &AudioDriver) -> String {
    match driver {
        AudioDriver::PipeWire => "PipeWire".to_owned(),
        AudioDriver::Auto => "Auto".to_owned(),
        AudioDriver::Custom(name) => name.clone(),
    }
}

/// The Media surface's live status cluster (lock 6): the play state + now-playing
/// title, the position / duration, the audio output, and — when joined — the
/// watch-together party. Every chip reads real state (§7), tone-mapped to a [`Style`]
/// token via [`ChipTone`].
fn build_status<E: MediaEngine>(controller: &MediaController<E>) -> Vec<StatusChip> {
    let player = controller.player();
    let (tone, dot) = match player.state() {
        PlayerState::Playing => (ChipTone::Ok, "\u{25CF}"),
        PlayerState::Paused | PlayerState::Loading => (ChipTone::Warn, "\u{25CF}"),
        PlayerState::Idle | PlayerState::Stopped | PlayerState::Ended => {
            (ChipTone::Neutral, "\u{25CB}")
        }
    };
    let mut chips = vec![StatusChip::with_icon(dot, now_playing_title(player), tone)];
    if player.media().is_some() {
        let pos = format_time(player.position());
        let time = player.duration().map_or_else(
            || format!("{pos} \u{00B7} live"),
            |dur| format!("{pos} / {}", format_time(dur)),
        );
        chips.push(StatusChip::new(time, ChipTone::Neutral));
    }
    chips.push(StatusChip::with_icon(
        "\u{266A}",
        output_label(&controller.audio_config().output.driver),
        ChipTone::Neutral,
    ));
    if let Some(name) = controller.party_name() {
        chips.push(StatusChip::with_icon(
            "\u{25C8}",
            format!("Party: {name}"),
            ChipTone::Info,
        ));
    }
    chips
}

// ─────────────────────────────── render ─────────────────────────────────────────────

/// Render the shared MENUBAR-ALL top bar over the Media surface and apply the chosen
/// item. Reads the controller immutably to build the menu model + status cluster,
/// renders through [`SharedMenuBar::show`], then applies the one activated [`Picked`]
/// mutably — so no borrow of the controller is held across the render.
pub fn menu_bar<E: MediaEngine>(ui: &mut Ui, controller: &mut MediaController<E>) {
    let menus = build_menus(controller);
    let status = build_status(controller);
    let model = MenuBarModel {
        title: "Media",
        accent: Style::ACCENT_MEDIA,
        menus: &menus,
        status: &status,
    };
    let picked = SharedMenuBar::show(ui, &model);
    if let Some(picked) = picked {
        apply(picked, controller);
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    // `Track` / `TrackKind` / `ReplayGainMode` / `TransportAction` / `MediaTab` /
    // `PlayerState` reach here through `use super::*`; only the fixture-construction
    // types are new.
    use mde_media_core::{FakeMpv, Player, PlaylistItem};

    /// The bar's menu titles, left to right — the stable spine + Media's own menus.
    const TITLES: [&str; 6] = ["File", "View", "Playback", "Audio", "Subtitles", "Cast"];

    fn tracks() -> Vec<Track> {
        vec![
            Track {
                id: 1,
                kind: TrackKind::Audio,
                title: None,
                lang: Some("eng".into()),
                codec: Some("aac".into()),
                default: true,
                selected: true,
            },
            Track {
                id: 2,
                kind: TrackKind::Audio,
                title: Some("Commentary".into()),
                lang: Some("eng".into()),
                codec: Some("aac".into()),
                default: false,
                selected: false,
            },
            Track {
                id: 1,
                kind: TrackKind::Subtitle,
                title: None,
                lang: Some("eng".into()),
                codec: Some("ass".into()),
                default: false,
                selected: false,
            },
        ]
    }

    /// A fresh idle controller (no media loaded).
    fn idle() -> MediaController<FakeMpv> {
        MediaController::new(Player::new(FakeMpv::new()))
    }

    /// A controller with a duration + enumerated tracks, loaded + pumped to Playing.
    fn playing() -> MediaController<FakeMpv> {
        let mut c = MediaController::new(Player::new(
            FakeMpv::new().with_duration(120.0).with_tracks(tracks()),
        ));
        c.dispatch(TransportAction::PlayPath("clip.mkv".to_owned()));
        c.pump();
        c
    }

    /// Find a top-level menu by title.
    fn menu<'a>(menus: &'a [Menu<Picked>], title: &str) -> &'a Menu<Picked> {
        menus
            .iter()
            .find(|m| m.title == title)
            .unwrap_or_else(|| panic!("menu {title} missing"))
    }

    /// Find an item by its activation id, descending into submenus.
    fn by_id<'a>(entries: &'a [Entry<Picked>], id: &Picked) -> Option<&'a BarItem<Picked>> {
        for entry in entries {
            match entry {
                Entry::Item(item) if &item.id == id => return Some(item),
                Entry::Submenu { entries, .. } => {
                    if let Some(found) = by_id(entries, id) {
                        return Some(found);
                    }
                }
                _ => {}
            }
        }
        None
    }

    // ── structure (§7 no dead entries) ───────────────────────────────────────────

    #[test]
    fn menu_order_is_stable_and_every_menu_is_nonempty() {
        let c = idle();
        let menus = build_menus(&c);
        let titles: Vec<&str> = menus.iter().map(|m| m.title.as_str()).collect();
        assert_eq!(titles, TITLES);
        for m in &menus {
            assert!(!m.entries.is_empty(), "menu {} shipped empty", m.title);
        }
    }

    #[test]
    fn omitted_menus_have_no_dead_entry() {
        // The surface has no clipboard/undo ops and no keymap/shortcuts — so no Edit
        // and no Help menu (honest omission, not a greyed stub).
        let c = idle();
        let titles: Vec<String> = build_menus(&c).iter().map(|m| m.title.clone()).collect();
        assert!(!titles.iter().any(|t| t == "Edit"));
        assert!(!titles.iter().any(|t| t == "Help"));
    }

    // ── context gating (§7 disable, not phasing) ─────────────────────────────────

    #[test]
    fn transport_items_disable_without_media_and_enable_once_loaded() {
        let idle = idle();
        let menus = build_menus(&idle);
        let playback = &menu(&menus, "Playback").entries;
        // Idle: Play/Pause + Stop + frame-step are disabled (no media), and the
        // seek items are disabled (no known duration).
        let play = by_id(playback, &Picked::Transport(TransportAction::TogglePlay)).unwrap();
        assert!(!play.enabled, "Play greys out with no media");
        assert!(
            !by_id(playback, &Picked::Transport(TransportAction::Stop))
                .unwrap()
                .enabled
        );
        assert!(
            !by_id(
                playback,
                &Picked::Transport(TransportAction::SeekBy(SKIP_SECS))
            )
            .unwrap()
            .enabled
        );
        // Loaded + playing: the same items enable.
        let live = playing();
        let live_menus = build_menus(&live);
        let live_playback = &menu(&live_menus, "Playback").entries;
        assert!(
            by_id(
                live_playback,
                &Picked::Transport(TransportAction::TogglePlay)
            )
            .unwrap()
            .enabled
        );
        assert!(
            by_id(
                live_playback,
                &Picked::Transport(TransportAction::SeekBy(SKIP_SECS))
            )
            .unwrap()
            .enabled
        );
        assert!(
            by_id(
                live_playback,
                &Picked::Transport(TransportAction::FrameForward)
            )
            .unwrap()
            .enabled
        );
    }

    #[test]
    fn clear_ab_loop_gates_on_an_active_or_pending_loop() {
        let mut c = playing();
        let clear = Picked::Transport(TransportAction::ClearAbLoop);
        // No loop yet → disabled.
        assert!(
            !by_id(&menu(&build_menus(&c), "Playback").entries, &clear)
                .unwrap()
                .enabled
        );
        // Mark A (pending) → Clear enables.
        c.dispatch(TransportAction::MarkAbLoop);
        assert!(
            by_id(&menu(&build_menus(&c), "Playback").entries, &clear)
                .unwrap()
                .enabled
        );
    }

    #[test]
    fn prev_next_gate_on_queue_neighbours() {
        let mut c = idle();
        let next = Picked::Transport(TransportAction::Next);
        assert!(
            !by_id(&menu(&build_menus(&c), "Playback").entries, &next)
                .unwrap()
                .enabled
        );
        c.player_mut().playlist_mut().push(PlaylistItem::new("a"));
        c.player_mut().playlist_mut().push(PlaylistItem::new("b"));
        assert!(
            by_id(&menu(&build_menus(&c), "Playback").entries, &next)
                .unwrap()
                .enabled
        );
    }

    #[test]
    fn leave_party_disabled_without_a_party() {
        let c = playing();
        let menus = build_menus(&c);
        let leave = by_id(&menu(&menus, "Cast").entries, &Picked::LeaveParty).unwrap();
        assert!(!leave.enabled, "Leave greys out with no joined party");
    }

    // ── checkmarks read back the live state ──────────────────────────────────────

    #[test]
    fn view_tab_items_checkmark_the_active_tab() {
        let mut c = idle();
        c.ui_mut().tab = MediaTab::Queue;
        let menus = build_menus(&c);
        let view = &menu(&menus, "View").entries;
        let queue = by_id(view, &Picked::GoTab(MediaTab::Queue)).unwrap();
        assert_eq!(queue.checked, Some(true));
        let sources = by_id(view, &Picked::GoTab(MediaTab::Sources)).unwrap();
        assert_eq!(sources.checked, Some(false));
    }

    #[test]
    fn speed_and_audio_mode_items_reflect_the_live_config() {
        let mut c = playing();
        c.dispatch(TransportAction::SetSpeed(1.5));
        c.dispatch(TransportAction::SetReplayGain(ReplayGainMode::Album));
        let menus = build_menus(&c);
        let speed = by_id(
            &menu(&menus, "Playback").entries,
            &Picked::Transport(TransportAction::SetSpeed(1.5)),
        )
        .unwrap();
        assert_eq!(speed.checked, Some(true), "the live 1.5× rate is checked");
        let album = by_id(
            &menu(&menus, "Audio").entries,
            &Picked::Transport(TransportAction::SetReplayGain(ReplayGainMode::Album)),
        )
        .unwrap();
        assert_eq!(album.checked, Some(true));
    }

    #[test]
    fn audio_and_subtitle_tracks_list_the_enumerated_tracks() {
        let mut c = playing();
        // Select the second audio track — the menu checkmarks it.
        c.dispatch(TransportAction::SelectTrack(TrackKind::Audio, 2));
        let menus = build_menus(&c);
        let sel = by_id(
            &menu(&menus, "Audio").entries,
            &Picked::Transport(TransportAction::SelectTrack(TrackKind::Audio, 2)),
        )
        .unwrap();
        assert_eq!(sel.checked, Some(true));
        // The Subtitles menu lists the one enumerated subtitle track.
        let sub = by_id(
            &menu(&menus, "Subtitles").entries,
            &Picked::Transport(TransportAction::SelectTrack(TrackKind::Subtitle, 1)),
        );
        assert!(sub.is_some(), "the enumerated subtitle track is offered");
    }

    #[test]
    fn track_menus_show_an_honest_caption_when_empty() {
        // No media → no enumerated tracks → the Subtitles menu is a dim caption, not a
        // dead item.
        let c = idle();
        let menus = build_menus(&c);
        let subs = &menu(&menus, "Subtitles").entries;
        assert!(matches!(subs.as_slice(), [Entry::Caption(_)]));
    }

    // ── apply drives the real seam (§6) ──────────────────────────────────────────

    #[test]
    fn apply_transport_toggles_play() {
        let mut c = playing();
        assert_eq!(c.player().state(), PlayerState::Playing);
        apply(Picked::Transport(TransportAction::TogglePlay), &mut c);
        assert_eq!(
            c.player().state(),
            PlayerState::Paused,
            "menu Pause reached mpv"
        );
    }

    #[test]
    fn apply_gotab_switches_the_view() {
        let mut c = idle();
        apply(Picked::GoTab(MediaTab::Library), &mut c);
        assert_eq!(c.ui().tab, MediaTab::Library);
    }

    #[test]
    fn apply_toggles_drive_ui_state() {
        let mut c = idle();
        assert!(!c.ui().fullscreen && !c.ui().pip);
        apply(Picked::ToggleFullscreen, &mut c);
        apply(Picked::TogglePip, &mut c);
        assert!(c.ui().fullscreen, "View → Fullscreen flipped the UI state");
        assert!(c.ui().pip, "View → Mini Player flipped the UI state");
    }

    // ── the status cluster reflects live state (lock 6) ──────────────────────────

    #[test]
    fn status_cluster_reflects_state_and_output() {
        // Idle: a now-playing chip ("Nothing playing") + the audio output chip, but no
        // time chip (nothing loaded).
        let idle = idle();
        let idle_status = build_status(&idle);
        assert!(idle_status.iter().any(|c| c.text == "Nothing playing"));
        assert!(idle_status.iter().any(|c| c.text == "PipeWire"));
        assert_eq!(idle_status.len(), 2, "no time chip with nothing loaded");
        // Playing: a time chip appears (position / duration).
        let live = playing();
        let live_status = build_status(&live);
        assert!(
            live_status.iter().any(|c| c.text.contains('/')),
            "a position / duration chip appears once loaded"
        );
    }

    // ── the bar renders headless (title + all menus + status) ────────────────────

    #[test]
    fn menu_bar_renders_headless() {
        use mde_egui::egui::{self, pos2, vec2, Rect};
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut c = playing();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1100.0, 680.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::TopBottomPanel::top("media-menu-bar").show(ctx, |ui| menu_bar(ui, &mut c));
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(
            !prims.is_empty(),
            "the media menu bar produced no draw primitives"
        );
    }
}
