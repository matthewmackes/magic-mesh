//! MENUBAR-ALL (Music) — the **shared top menu bar** across the music surface
//! (design: `docs/design/menubar-all.md`).
//!
//! The music surface is a narrow one: it connects to a single Airsonic server,
//! lists a library, opens an album for its tracks, and drives the native engine's
//! transport. This is the *discoverable* face over those seams — hosted on the
//! shared [`mde_egui::menubar::MenuBar`] (MENUBAR-ALL-1) under one UPPERCASE
//! `MUSIC` title, tinted with the dock's **Media** group accent
//! ([`Style::ACCENT_MEDIA`]) so the bar reads as the same platform surface the
//! dock icon promises. Each item is the **mouse twin of an existing seam** (§6,
//! one dispatch path in [`crate::app`]), never a new behaviour and never a stub.
//!
//! Per the governing lock (**no dead entries** — an item ships only when its seam
//! exists, §7), an item whose feature needs context (Pause with nothing playing,
//! Back to Library with no album open) renders **disabled**, never a silent
//! no-op; a feature the surface genuinely lacks is **omitted**.
//!
//! The menus and the seam each drives:
//!
//! * **Playback** — Pause ([`Command::Pause`], greyed when nothing plays), Resume
//!   ([`Command::Resume`], greyed unless paused), Stop ([`Command::Stop`], greyed
//!   with no loaded track).
//! * **Library** — Refresh Library ([`Command::LoadLibrary`], the album-list
//!   reload the surface issues on start), Reload Album ([`Command::LoadAlbum`] for
//!   the open album — the advanced track re-fetch, greyed with no album open).
//! * **View** — Back to Library ([`crate::model::MusicState::close`], the album
//!   view's own "Back to library" seam, greyed already at the listing).
//!
//! **Honestly omitted** (no landed seam, so no dead entry): the **File/Edit/Help**
//! spine — the surface has no file, clipboard, or about seam; **Play/Next/Prev/
//! Shuffle/Repeat** — a track is chosen by clicking a library/album row, and there
//! is no queue, next/previous, shuffle, or repeat seam to drive (adding one would
//! be new behaviour, not glue). The now-playing transport carries no keyboard
//! chord, so no menu item shows a misleading shortcut hint (§9 — a hint reflects a
//! real binding or is absent).
//!
//! §4: the shared [`MenuBar`] renders through the Carbon [`Style`] install — no
//! forced colours, so egui's disabled dimming reads correctly; the surface builds
//! the menu **model** each frame ([`build_menus`]) from a [`MenuContext`] snapshot
//! and dispatches the activated [`MenuAction`] through
//! [`crate::app::MusicApp::run_menu_action`], so every seam + gate is preserved and
//! the whole thing is unit-testable without a GPU or a sound device.

use mde_egui::egui::Ui;
use mde_egui::menubar::{Entry, Item as BarItem, Menu, MenuBar, MenuBarModel};
use mde_egui::{ChipTone, StatusChip, Style};

use crate::model::{format_duration, Command};

// ─────────────────────────────── actions ────────────────────────────────────

/// One action a menu item dispatches — each routes to a real seam in
/// [`crate::app::MusicApp::run_menu_action`] (§7, no dead entries). `Copy` so the
/// static item tables can hold it and the shared bar can return it by value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    /// Pause the engine ([`Command::Pause`]).
    Pause,
    /// Resume the paused engine ([`Command::Resume`]).
    Resume,
    /// Stop playback and clear the now-playing track ([`Command::Stop`]).
    Stop,
    /// Re-fetch the album library ([`Command::LoadLibrary`]).
    RefreshLibrary,
    /// Re-fetch the open album's track list ([`Command::LoadAlbum`]).
    ReloadAlbum,
    /// Return from an album's track list to the library listing
    /// ([`crate::model::MusicState::close`]).
    BackToLibrary,
}

impl MenuAction {
    /// Map this action to the worker [`Command`] it sends, or `None` for an action
    /// handled by a local state seam ([`Self::BackToLibrary`] closes the open album
    /// with no worker round-trip). [`Self::ReloadAlbum`] carries no id here — the
    /// album id is resolved from live state at dispatch — so it also maps to `None`
    /// and is handled by the caller. Pure, so the transport mapping is testable
    /// without a worker.
    pub const fn command(self) -> Option<Command> {
        match self {
            Self::Pause => Some(Command::Pause),
            Self::Resume => Some(Command::Resume),
            Self::Stop => Some(Command::Stop),
            Self::RefreshLibrary => Some(Command::LoadLibrary),
            Self::ReloadAlbum | Self::BackToLibrary => None,
        }
    }
}

// ──────────────────────────────── gating ────────────────────────────────────

/// What must hold for an item to be **enabled** — context gating over seams that
/// all exist (§7 disable, not phasing). The presence of an item is fixed (the
/// static tables); `Gate` only greys it when its live precondition is unmet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// A configured server client exists (Refresh Library).
    Connected,
    /// A track is loaded and playing (Pause).
    Playing,
    /// A track is loaded and paused (Resume).
    Paused,
    /// A track is loaded, playing or paused (Stop).
    HasTrack,
    /// An album is open for browsing (Reload Album / Back to Library).
    AlbumOpen,
}

impl Gate {
    /// Whether the gate passes under `cx`.
    pub const fn enabled(self, cx: &MenuContext) -> bool {
        match self {
            Self::Connected => cx.connected,
            Self::Playing => cx.has_track && cx.playing,
            Self::Paused => cx.has_track && !cx.playing,
            Self::HasTrack => cx.has_track,
            Self::AlbumOpen => cx.album_open,
        }
    }
}

// ─────────────────────────────── the context ────────────────────────────────

/// The now-playing readout for the status cluster — the live track identity plus
/// the engine's playhead (in whole seconds) and the track's tagged length
/// (`0` = the server gave none, e.g. a stream). Owned so the bar renders from a
/// snapshot without borrowing the surface mid-frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NowPlaying {
    /// The track title.
    pub title: String,
    /// The track artist.
    pub artist: String,
    /// The engine's live playhead position, in whole seconds.
    pub elapsed_secs: u64,
    /// The track's tagged length in whole seconds (`0` = unknown).
    pub duration_secs: u64,
}

/// The per-frame surface-state snapshot the bar renders from (built by
/// [`crate::app::MusicApp::menu_context`]) — the bar never reaches into the
/// surface mid-render, so its gating + status cluster are unit-testable without
/// egui.
// `struct_excessive_bools`: these are independent per-frame facts (four unrelated
// enablement/health read-backs), not a disguised state machine.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuContext {
    /// A configured server client exists (a worker is running) — Refresh's gate,
    /// and the "connected" half of the server status chip.
    pub connected: bool,
    /// The album-list fetch failed (the server is unreachable / errored) — turns
    /// the server chip to the danger tone.
    pub library_failed: bool,
    /// A track is loaded in the engine (playing or paused).
    pub has_track: bool,
    /// The engine is in the playing (not paused) state.
    pub playing: bool,
    /// An album is open for its track list (View → Back to Library's gate).
    pub album_open: bool,
    /// The configured server host, empty when unconfigured (the status chip text).
    pub server: String,
    /// The now-playing readout, or `None` when nothing is loaded.
    pub now_playing: Option<NowPlaying>,
}

// ─────────────────────────── the static menu data ───────────────────────────

/// One menu item: its label, the action it dispatches, its enablement gate, and
/// whether a group separator precedes it. Music items carry no shortcut hint (the
/// surface has no keyboard chords, §9) and no check-mark (no toggle seams).
struct MenuItem {
    /// The visible label.
    label: &'static str,
    /// The dispatched action.
    action: MenuAction,
    /// The enablement gate (§7 grey-out context).
    gate: Gate,
    /// Draw a group separator above this item.
    sep_before: bool,
}

impl MenuItem {
    /// Shorthand constructor for the static tables below.
    const fn new(label: &'static str, action: MenuAction, gate: Gate, sep_before: bool) -> Self {
        Self {
            label,
            action,
            gate,
            sep_before,
        }
    }
}

/// The Playback menu — the engine transport. Pause / Resume are honestly gated
/// (only one is live at a time), Stop needs a loaded track.
const PLAYBACK_ITEMS: [MenuItem; 3] = [
    MenuItem::new("Pause", MenuAction::Pause, Gate::Playing, false),
    MenuItem::new("Resume", MenuAction::Resume, Gate::Paused, false),
    MenuItem::new("Stop", MenuAction::Stop, Gate::HasTrack, true),
];

/// The Library menu — the album-list reload (the same `getAlbumList2` the surface
/// runs on start) and the open album's advanced track re-fetch.
const LIBRARY_ITEMS: [MenuItem; 2] = [
    MenuItem::new(
        "Refresh Library",
        MenuAction::RefreshLibrary,
        Gate::Connected,
        false,
    ),
    MenuItem::new(
        "Reload Album",
        MenuAction::ReloadAlbum,
        Gate::AlbumOpen,
        true,
    ),
];

/// The View menu — navigation between the two views the surface has (the library
/// listing and an album's track list).
const VIEW_ITEMS: [MenuItem; 1] = [MenuItem::new(
    "Back to Library",
    MenuAction::BackToLibrary,
    Gate::AlbumOpen,
    false,
)];

/// The whole menu bar as data: `(title, items)` left→right. The File/Edit/Help
/// spine is omitted (no file/clipboard/about seam), so the present menus are the
/// three the surface genuinely backs — [`tests`] assert none is empty.
const MENUS: [(&str, &[MenuItem]); 3] = [
    ("Playback", &PLAYBACK_ITEMS),
    ("Library", &LIBRARY_ITEMS),
    ("View", &VIEW_ITEMS),
];

// ───────────────────────────────── render ───────────────────────────────────

/// Render the menu bar and return the action the operator picked this frame, if
/// any. The surface owns its [`MENUS`] tables + [`MenuAction`] vocabulary + [`Gate`]
/// context (§6, one dispatch path); this only builds the shared model each frame —
/// the host widget, the `MUSIC` title header, and the live status cluster are all
/// the shared component provides.
pub fn show(ui: &mut Ui, cx: &MenuContext) -> Option<MenuAction> {
    let menus = build_menus(cx);
    let status = build_status(cx);
    let model = MenuBarModel {
        // UPPERCASE'd by the shared header (lock 2/14); the Media group accent
        // matches how the dock tints the Music icon (lock 2).
        title: "Music",
        accent: Style::ACCENT_MEDIA,
        menus: &menus,
        status: &status,
    };
    MenuBar::show(ui, &model)
}

/// Convert the static [`MENUS`] tables + the live [`MenuContext`] into the shared
/// menu model, preserving every item, its §7 grey-out [`Gate`], and its group
/// separators — the render host is the only thing that changed (§6/§7).
fn build_menus(cx: &MenuContext) -> Vec<Menu<MenuAction>> {
    MENUS
        .iter()
        .map(|(title, items)| {
            let mut entries = Vec::with_capacity(items.len());
            for item in *items {
                if item.sep_before {
                    entries.push(Entry::Separator);
                }
                let bar_item = BarItem::new(item.action, item.label).enabled(item.gate.enabled(cx));
                entries.push(Entry::Item(bar_item));
            }
            Menu::new(*title, entries)
        })
        .collect()
}

/// The music surface's live status cluster (lock 6): the server the surface is
/// connected to (its health tinted by the library fetch), and the now-playing
/// readout — a ▶/⏸ play-state glyph, the elapsed / total playhead, and the track
/// identity. Every chip reflects real state (§7 — never a placeholder).
fn build_status(cx: &MenuContext) -> Vec<StatusChip> {
    let mut chips = Vec::new();

    // The server / output the surface is bound to. Unconfigured → an honest "Not
    // connected"; configured but the library fetch failed → the danger tone; else
    // a healthy green dot.
    if cx.server.is_empty() {
        chips.push(StatusChip::with_icon(
            "\u{25CF}",
            "Not connected",
            ChipTone::Warn,
        ));
    } else {
        let tone = if cx.library_failed {
            ChipTone::Danger
        } else {
            ChipTone::Ok
        };
        chips.push(StatusChip::with_icon("\u{25CF}", cx.server.clone(), tone));
    }

    // The now-playing readout, rightmost (most prominent).
    if let Some(np) = &cx.now_playing {
        let time = if np.duration_secs > 0 {
            format!(
                "{} / {}",
                format_duration(np.elapsed_secs.min(np.duration_secs)),
                format_duration(np.duration_secs),
            )
        } else {
            format_duration(np.elapsed_secs)
        };
        chips.push(StatusChip::new(time, ChipTone::Neutral));

        // ▶ while playing, ⏸ while paused — both covered by the shared fallback
        // fonts; the tone reinforces the glyph (green = playing).
        let (glyph, tone) = if cx.playing {
            ("\u{25B6}", ChipTone::Ok)
        } else {
            ("\u{23F8}", ChipTone::Neutral)
        };
        chips.push(StatusChip::with_icon(
            glyph,
            format!("{} \u{2014} {}", np.title, np.artist),
            tone,
        ));
    } else {
        chips.push(StatusChip::new("Nothing playing", ChipTone::Neutral));
    }

    chips
}

#[cfg(test)]
mod tests {
    use super::{
        build_menus, build_status, Gate, MenuAction, MenuContext, NowPlaying, LIBRARY_ITEMS, MENUS,
        VIEW_ITEMS,
    };
    use crate::model::Command;
    use mde_egui::menubar::Entry;
    use mde_egui::ChipTone;

    /// A live, playing surface: connected, an album open, a track playing.
    fn playing_context() -> MenuContext {
        MenuContext {
            connected: true,
            library_failed: false,
            has_track: true,
            playing: true,
            album_open: true,
            server: "airsonic.mesh:4040".to_owned(),
            now_playing: Some(NowPlaying {
                title: "Aja".to_owned(),
                artist: "Steely Dan".to_owned(),
                elapsed_secs: 83,
                duration_secs: 480,
            }),
        }
    }

    /// A fresh, unconfigured surface: no server, nothing loaded, at the listing.
    fn idle_context() -> MenuContext {
        MenuContext {
            connected: false,
            library_failed: false,
            has_track: false,
            playing: false,
            album_open: false,
            server: String::new(),
            now_playing: None,
        }
    }

    // ── structure (§7 no dead entries) ───────────────────────────────────────

    #[test]
    fn every_menu_is_nonempty_and_labeled() {
        for (title, items) in MENUS {
            assert!(!items.is_empty(), "menu {title} shipped empty");
            for item in items {
                assert!(!item.label.is_empty(), "{title} has an unlabeled item");
            }
        }
    }

    #[test]
    fn menu_order_is_stable_and_spine_is_omitted() {
        let titles: Vec<&str> = MENUS.iter().map(|(t, _)| *t).collect();
        assert_eq!(titles, vec!["Playback", "Library", "View"]);
        // File/Edit/Help have no music seam, so they are honestly absent (not
        // present-but-empty); a Play/Next/Shuffle/Repeat item would be a dead
        // entry (no queue/next/shuffle/repeat seam exists).
        for banned in ["File", "Edit", "Help", "Queue"] {
            assert!(!titles.contains(&banned), "{banned} shipped without a seam");
        }
        let labels: Vec<&str> = MENUS
            .iter()
            .flat_map(|(_, items)| items.iter())
            .map(|i| i.label)
            .collect();
        for banned in ["Play", "Next", "Previous", "Shuffle", "Repeat"] {
            assert!(
                !labels.contains(&banned),
                "{banned} shipped without a landed seam"
            );
        }
    }

    // ── every item maps to its real seam ─────────────────────────────────────

    #[test]
    fn build_menus_maps_items_to_their_actions() {
        let menus = build_menus(&playing_context());
        // Playback carries the three transport actions in order, with a separator
        // before Stop.
        let playback = &menus[0];
        assert_eq!(playback.title, "Playback");
        let actions: Vec<MenuAction> = playback
            .entries
            .iter()
            .filter_map(|e| match e {
                Entry::Item(item) => Some(item.id),
                _ => None,
            })
            .collect();
        assert_eq!(
            actions,
            vec![MenuAction::Pause, MenuAction::Resume, MenuAction::Stop]
        );
        assert!(
            playback
                .entries
                .iter()
                .any(|e| matches!(e, Entry::Separator)),
            "Stop is grouped under a separator"
        );
        // Library + View carry their reload / navigation seams.
        assert!(LIBRARY_ITEMS
            .iter()
            .any(|i| i.action == MenuAction::RefreshLibrary));
        assert!(LIBRARY_ITEMS
            .iter()
            .any(|i| i.action == MenuAction::ReloadAlbum));
        assert_eq!(VIEW_ITEMS[0].action, MenuAction::BackToLibrary);
    }

    #[test]
    fn transport_actions_map_to_their_commands() {
        assert!(matches!(MenuAction::Pause.command(), Some(Command::Pause)));
        assert!(matches!(
            MenuAction::Resume.command(),
            Some(Command::Resume)
        ));
        assert!(matches!(MenuAction::Stop.command(), Some(Command::Stop)));
        assert!(matches!(
            MenuAction::RefreshLibrary.command(),
            Some(Command::LoadLibrary)
        ));
        // Album re-fetch (id resolved at dispatch) + local navigation carry no
        // fixed command here.
        assert!(MenuAction::ReloadAlbum.command().is_none());
        assert!(MenuAction::BackToLibrary.command().is_none());
    }

    // ── honest gating (§7): context-gated items disable ──────────────────────

    #[test]
    fn transport_gates_follow_the_engine_state() {
        let playing = playing_context();
        assert!(
            Gate::Playing.enabled(&playing),
            "Pause is live while playing"
        );
        assert!(
            !Gate::Paused.enabled(&playing),
            "Resume greys while playing"
        );
        assert!(
            Gate::HasTrack.enabled(&playing),
            "Stop is live with a track"
        );

        let paused = MenuContext {
            playing: false,
            ..playing
        };
        assert!(!Gate::Playing.enabled(&paused), "Pause greys while paused");
        assert!(Gate::Paused.enabled(&paused), "Resume is live while paused");
        assert!(
            Gate::HasTrack.enabled(&paused),
            "Stop stays live when paused"
        );
    }

    #[test]
    fn idle_surface_greys_every_context_gated_item() {
        let idle = idle_context();
        // Nothing loaded, nothing open, no server: every gate but none-such is off.
        assert!(!Gate::Playing.enabled(&idle));
        assert!(!Gate::Paused.enabled(&idle));
        assert!(!Gate::HasTrack.enabled(&idle));
        assert!(!Gate::AlbumOpen.enabled(&idle));
        assert!(!Gate::Connected.enabled(&idle));
        // The built menu greys them too (a disabled item, never omitted).
        let menus = build_menus(&idle);
        for menu in &menus {
            for entry in &menu.entries {
                if let Entry::Item(item) = entry {
                    assert!(
                        !item.enabled,
                        "{} should grey on an idle surface",
                        item.label
                    );
                }
            }
        }
    }

    #[test]
    fn connected_enables_refresh_only() {
        // A connected surface with nothing playing and no album open: Refresh is
        // live, but the transport + album items stay greyed (their context is
        // still unmet) — presence is fixed, the gate is the only variable.
        let cx = MenuContext {
            connected: true,
            ..idle_context()
        };
        assert!(Gate::Connected.enabled(&cx));
        assert!(!Gate::HasTrack.enabled(&cx));
        assert!(!Gate::AlbumOpen.enabled(&cx));
    }

    // ── the status cluster reflects live state ───────────────────────────────

    #[test]
    fn status_shows_server_and_now_playing() {
        let chips = build_status(&playing_context());
        // The server chip (green — connected + healthy) plus the elapsed/total and
        // the now-playing identity.
        assert!(chips.iter().any(|c| c.text == "airsonic.mesh:4040"));
        assert!(
            chips
                .iter()
                .any(|c| c.text.contains("1:23") && c.text.contains("8:00")),
            "the elapsed / total playhead is shown"
        );
        let np = chips
            .iter()
            .find(|c| c.text.contains("Aja"))
            .expect("the now-playing chip is present");
        assert_eq!(np.icon.as_deref(), Some("\u{25B6}"), "▶ while playing");
        assert_eq!(np.tone, ChipTone::Ok);
        assert!(np.text.contains("Steely Dan"), "carries title — artist");
    }

    #[test]
    fn status_shows_pause_glyph_and_failed_server() {
        // Paused + the library fetch failed: the play glyph flips to ⏸ and the
        // server chip goes danger.
        let cx = MenuContext {
            playing: false,
            library_failed: true,
            ..playing_context()
        };
        let chips = build_status(&cx);
        let server = chips
            .iter()
            .find(|c| c.text == "airsonic.mesh:4040")
            .expect("server chip");
        assert_eq!(server.tone, ChipTone::Danger, "a failed fetch reads danger");
        let np = chips
            .iter()
            .find(|c| c.text.contains("Aja"))
            .expect("now-playing chip");
        assert_eq!(np.icon.as_deref(), Some("\u{23F8}"), "⏸ while paused");
    }

    #[test]
    fn status_is_honest_when_idle_and_unconfigured() {
        let chips = build_status(&idle_context());
        assert!(
            chips
                .iter()
                .any(|c| c.text == "Not connected" && c.tone == ChipTone::Warn),
            "an unconfigured surface says so"
        );
        assert!(chips.iter().any(|c| c.text == "Nothing playing"));
    }

    // ── the bar renders headless (title + menus + status) ────────────────────

    #[test]
    fn menu_bar_renders_headless() {
        use mde_egui::egui::{self, pos2, vec2, Rect};
        use mde_egui::Style;
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let cx = playing_context();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1024.0, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = super::show(ui, &cx);
            });
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(
            !prims.is_empty(),
            "the menu bar produced no draw primitives"
        );
    }
}
