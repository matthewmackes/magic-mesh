//! The eframe app (E12-5): the egui music surface. It loads the shared Airsonic
//! credentials, spawns the [`worker`] thread, and renders the [`MusicState`]
//! entirely through the shared [`Style`] — a library listing, an album's track
//! list, and a transport strip. Clicks become [`Command`]s for the worker; the
//! worker's [`Update`]s are drained each frame. Nothing here fakes data: with no
//! creds yet it shows the daemon's own first-run hint, and load/playback failures
//! render as honest error lines (§7).

use std::sync::mpsc::{self, Receiver, Sender};

use mde_egui::eframe::{self, App, CreationContext};
use mde_egui::egui::{
    self, Align, Context, CursorIcon, Layout, Response, RichText, ScrollArea, Sense,
};
use mde_egui::{Motion, Style};

use mde_musicd::airsonic::{Album, Client, Song};
use mde_musicd::creds;

use crate::menubar::{self, MenuAction, MenuContext, NowPlaying};
use crate::model::{album_subtitle, format_duration, Command, Fetch, MusicState, Update};
use crate::worker;

/// The music surface: the view-model plus the channels to its worker thread.
pub struct MusicApp {
    /// The render-agnostic state the view draws.
    state: MusicState,
    /// Outbound intents to the worker — `None` when no creds are configured yet
    /// (no worker is spawned in that case).
    commands: Option<Sender<Command>>,
    /// Inbound results from the worker, drained at the top of each frame.
    updates: Receiver<Update>,
    /// The configured server host, shown in the header (empty when unconfigured).
    server: String,
    /// The first-run / setup error (missing or malformed creds), if any.
    setup_error: Option<String>,
}

impl MusicApp {
    /// Build the surface: load the shared Airsonic credentials and, when present,
    /// spawn the worker and kick off the library load. With no creds yet the
    /// surface opens to an honest "connect a server" state instead of faking a
    /// library.
    #[must_use]
    pub fn new(cc: &CreationContext<'_>) -> Self {
        Self::new_with_ctx(&cc.egui_ctx)
    }

    /// Build over an egui [`egui::Context`] directly — the DRM-seat shell path
    /// (`mde-shell-egui --features drm`) has no eframe `CreationContext`, only the
    /// bare `Context` the DRM runner drives. Both entry points converge here so the
    /// worker still gets a repaint handle in either runner.
    #[must_use]
    pub fn new_with_ctx(ctx: &egui::Context) -> Self {
        let (update_tx, update_rx) = mpsc::channel::<Update>();
        let mut state = MusicState::new();
        match creds::load() {
            Ok(c) => {
                let client = Client::new(c.server_url, c.username, &c.password);
                let server = client.base_url().to_string();
                let commands = worker::spawn(client, ctx.clone(), update_tx);
                let _ = commands.send(Command::LoadLibrary);
                state.albums = Fetch::Loading;
                Self {
                    state,
                    commands: Some(commands),
                    updates: update_rx,
                    server,
                    setup_error: None,
                }
            }
            Err(e) => Self {
                state,
                commands: None,
                updates: update_rx,
                server: String::new(),
                setup_error: Some(e.to_string()),
            },
        }
    }

    /// Send an intent to the worker (a no-op when no worker is running).
    fn send(&self, cmd: Command) {
        if let Some(tx) = &self.commands {
            let _ = tx.send(cmd);
        }
    }

    /// WIN7-4 — the currently loaded track, the SAME `self.state.now_playing`
    /// field [`Self::menu_context`] already reads for its own transport
    /// status cluster (no second read, §7). `mde-shell-egui`'s embedding
    /// shell holds this `MusicApp` directly (the `mde-media-egui`
    /// `MediaController::player` precedent — a thin, already-established
    /// read-only accessor shape, not a new one invented here) and reuses it
    /// for the Start Menu Music tile's live facts.
    #[must_use]
    pub fn now_playing(&self) -> Option<&Song> {
        self.state.now_playing.as_ref()
    }

    /// Snapshot the surface into the [`MenuContext`] the shared menu bar renders
    /// from (the read half of a frame) — its connection health, the transport
    /// state, whether an album is open, and the now-playing readout. The elapsed
    /// playhead is clamped to the tagged length so a slightly-ahead poll never
    /// reads past the end; a `0` duration (a stream the server gave no length for)
    /// leaves the total off. The bar never reaches into the surface mid-render, so
    /// its gating + status cluster stay unit-testable without egui.
    fn menu_context(&self) -> MenuContext {
        let now_playing = self.state.now_playing.as_ref().map(|song| {
            let duration_secs = u64::from(song.duration);
            let elapsed_secs = self.state.position_ms / 1000;
            NowPlaying {
                title: song.title.clone(),
                artist: song.artist.clone(),
                elapsed_secs: if duration_secs > 0 {
                    elapsed_secs.min(duration_secs)
                } else {
                    elapsed_secs
                },
                duration_secs,
            }
        });
        MenuContext {
            connected: self.commands.is_some(),
            library_failed: matches!(self.state.albums, Fetch::Failed(_)),
            has_track: self.state.now_playing.is_some(),
            playing: self.state.playing,
            album_open: self.state.open_album.is_some(),
            server: self.server.clone(),
            now_playing,
        }
    }

    /// Dispatch a menu-bar [`MenuAction`] to its real seam (§6, one dispatch path).
    /// The transport + library-reload actions become the worker [`Command`] they
    /// map to; Reload Album resolves the open album's id from live state; Back to
    /// Library is a local navigation seam ([`MusicState::close`]) with no worker
    /// round-trip. No new behaviour — every arm drives an existing seam.
    fn run_menu_action(&mut self, action: MenuAction) {
        match action {
            MenuAction::ReloadAlbum => {
                if let Some(open) = &self.state.open_album {
                    let id = open.album.id.clone();
                    self.send(Command::LoadAlbum(id));
                }
            }
            MenuAction::BackToLibrary => self.state.close(),
            other => {
                if let Some(cmd) = other.command() {
                    self.send(cmd);
                }
            }
        }
    }

    /// The album library listing (or its loading/empty/error state).
    fn render_library(&mut self, ui: &mut egui::Ui) {
        // The listing's view title, on the same HEADING rung the open-album view
        // gives its album name — so the two top-level views read as parallel.
        ui.label(
            RichText::new("Library")
                .size(Style::HEADING)
                .color(Style::TEXT),
        );
        // The heading→rule→content rhythm matches the open-album view exactly
        // (SP_S on both sides of the separator), so the two top-level views read
        // as parallel rather than each pacing its own header a different way.
        ui.add_space(Style::SP_S);
        ui.separator();
        ui.add_space(Style::SP_S);

        let mut to_open: Option<Album> = None;
        match &self.state.albums {
            Fetch::Idle | Fetch::Loading => {
                centered_state(ui, true, "Loading library…");
            }
            Fetch::Failed(e) => {
                ui.colored_label(Style::DANGER, format!("Couldn't load the library: {e}"));
            }
            Fetch::Ready(albums) if albums.is_empty() => {
                centered_state(ui, false, "This server has no albums yet.");
            }
            Fetch::Ready(albums) => {
                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for album in albums {
                            if album_row(ui, album).clicked() {
                                to_open = Some(album.clone());
                            }
                            ui.add_space(Style::SP_XS);
                        }
                    });
            }
        }

        if let Some(album) = to_open {
            let id = album.id.clone();
            self.state.open(album);
            self.send(Command::LoadAlbum(id));
        }
    }

    /// The open album's header + track list (or its loading/empty/error state).
    fn render_album(&mut self, ui: &mut egui::Ui) {
        let mut go_back = false;
        let mut to_play: Option<Song> = None;

        if let Some(open) = &self.state.open_album {
            if ui.button("Back to library").clicked() {
                go_back = true;
            }
            ui.add_space(Style::SP_S);
            ui.label(
                RichText::new(&open.album.name)
                    .size(Style::HEADING)
                    .color(Style::TEXT),
            );
            let subtitle = album_subtitle(&open.album);
            if !subtitle.is_empty() {
                ui.label(
                    RichText::new(subtitle)
                        .size(Style::BODY)
                        .color(Style::TEXT_DIM),
                );
            }
            ui.add_space(Style::SP_S);
            ui.separator();
            ui.add_space(Style::SP_S);

            match &open.tracks {
                Fetch::Idle | Fetch::Loading => {
                    centered_state(ui, true, "Loading tracks…");
                }
                Fetch::Failed(e) => {
                    ui.colored_label(Style::DANGER, format!("Couldn't load tracks: {e}"));
                }
                Fetch::Ready(songs) if songs.is_empty() => {
                    centered_state(ui, false, "This album has no tracks.");
                }
                Fetch::Ready(songs) => {
                    ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for (i, song) in songs.iter().enumerate() {
                                if track_row(ui, i, song).clicked() {
                                    to_play = Some(song.clone());
                                }
                                // Same inter-row breathing room the album listing
                                // gives its rows, so both lists share one rhythm.
                                ui.add_space(Style::SP_XS);
                            }
                        });
                }
            }
        }

        if go_back {
            self.state.close();
        }
        if let Some(song) = to_play {
            self.send(Command::Play(song));
        }
    }
}

/// Render the music surface's central content into the given `ui`.
///
/// Draws the honest "connect a server" state when no credentials are configured,
/// then any transient engine-error line, and finally either the open album's track
/// list or the library listing. Clicks still flow to the worker through `app`'s
/// command channel, exactly as the standalone binary drives them.
///
/// This is the one body shared by the standalone binary's `CentralPanel` and the
/// embedded shell panel (E12-3b), so the surface renders identically whether it
/// owns a window or is a panel inside the one shell — the EMBED model of E12
/// "Construct" §5 (surfaces are panels in the shell, not separate clients). It draws
/// only through the shared [`Style`], reusing `app`'s existing state (no parallel
/// state is introduced).
pub fn music_panel(ui: &mut egui::Ui, app: &mut MusicApp) {
    if let Some(detail) = &app.setup_error {
        render_setup_needed(ui, detail);
        return;
    }
    // A transient playback/engine error (e.g. no sound device on a headless
    // host) — rendered, not swallowed.
    if let Some(error) = app.state.error.clone() {
        ui.add_space(Style::SP_XS);
        ui.colored_label(Style::DANGER, error);
        ui.add_space(Style::SP_XS);
    }
    ui.add_space(Style::SP_S);
    if app.state.open_album.is_some() {
        app.render_album(ui);
    } else {
        app.render_library(ui);
    }
}

/// Drain the worker's updates into the surface state — the per-frame **state
/// pump**.
///
/// The standalone [`MusicApp`]'s `update` calls this at the top of every frame;
/// the E12 shell (E12-3b) calls it for the mounted surface each frame too, because
/// the shell owns the one frame loop and never calls the surface's `App::update`.
/// Non-blocking (`try_recv`) and a no-op when the worker has sent nothing — or when
/// no worker is running (the unconfigured, no-creds surface).
pub fn music_pump(app: &mut MusicApp) {
    while let Ok(update) = app.updates.try_recv() {
        app.state.apply(update);
    }
}

/// Render the surface's **shared top menu bar** (MENUBAR-ALL) into `ui`, then
/// dispatch the action the operator picked to its real seam.
///
/// The bar carries the UPPERCASE `MUSIC` title, the Playback / Library / View
/// menus, and the live status cluster (server health + now-playing). The standalone
/// app frames it in the window's top panel; the E12 shell renders it above the
/// mounted [`music_panel`] so the embedded surface keeps the same discoverable
/// chrome + transport the standalone binary shows. The bar stays out of
/// [`music_panel`] because the shell supplies its own surrounding chrome. Takes
/// `&mut` because Back to Library mutates the surface's own view state (§6 glue: the
/// menu is the mouse twin of an existing seam).
pub fn music_header(ui: &mut egui::Ui, app: &mut MusicApp) {
    let cx = app.menu_context();
    if let Some(action) = menubar::show(ui, &cx) {
        app.run_menu_action(action);
    }
}

impl App for MusicApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        // Drain everything the worker has sent since the last frame.
        music_pump(self);

        egui::TopBottomPanel::top("music-header").show(ctx, |ui| music_header(ui, self));

        // The central content is the shared `music_panel` body, so the standalone
        // window and the embedded shell panel (E12-3b) render identically.
        egui::CentralPanel::default().show(ctx, |ui| music_panel(ui, self));
    }
}

/// The honest first-run state when no Airsonic credentials are configured yet: it
/// shows the daemon's own missing-creds message (which names the `--first-run`
/// flow + the path it looked at), never fake library data (§7).
fn render_setup_needed(ui: &mut egui::Ui, detail: &str) {
    ui.add_space(Style::SP_XL);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new("No music server connected")
                .size(Style::HEADING)
                .color(Style::TEXT),
        );
        ui.add_space(Style::SP_S);
        ui.label(
            RichText::new(detail)
                .size(Style::BODY)
                .color(Style::TEXT_DIM),
        );
    });
}

/// A **designed transient state** — a centred message, with the brand-accent
/// spinner above it while `busy` — for the library / album loading and empty
/// branches, so an in-flight or empty surface reads as a deliberate state rather
/// than a lone dim line pinned to the top-left corner (§7 — an honest "nothing
/// yet", never a mockup). Draws only through the shared `Style`: the spinner
/// takes [`Style::ACCENT`] (the one progress token, CRAFT §7) and the message the
/// dim secondary tone — no raw colour, no literal size.
fn centered_state(ui: &mut egui::Ui, busy: bool, message: &str) {
    ui.add_space(Style::SP_XL);
    ui.vertical_centered(|ui| {
        if busy {
            ui.add(egui::Spinner::new().color(Style::ACCENT).size(Style::SP_L));
            ui.add_space(Style::SP_S);
        }
        ui.label(
            RichText::new(message)
                .size(Style::BODY)
                .color(Style::TEXT_DIM),
        );
    });
}

/// Ease a **hover treatment** onto a just-built list row through the shared FAST
/// motion so the row responds to the pointer instead of snapping (CRAFT §4 — hover
/// changes state, so it animates). One hover progress `t` drives two eased layers:
///
/// * a **wash** behind the row content — `band` is the painter slot reserved
///   *before* the row (the repo's reserved-shape idiom, so it renders underneath)
///   — the hovered-surface fill ([`Style::SURFACE_HI`]) faded by the 0→1 progress
///   at the shared card radius ([`Style::RADIUS_M`], matching [`mde_egui::card`]);
/// * a slim **leading accent tab** over the row's left gutter in the surface's own
///   Media group accent ([`Style::ACCENT_MEDIA`] — the same tint the menu bar
///   wears), so the hovered row reads as the live one. Its presence eases in with
///   [`Motion::hover_lift`] over the same progress, sitting in the card's padding
///   gutter so it never crosses the row text.
///
/// `id` keys the per-row animation. Consumes only shared tokens — no raw colour,
/// size, or literal duration — and, because `t` comes from [`Motion::animate`],
/// reduce-motion collapses both layers to their endpoint (a snap, no glide;
/// a11y-07).
fn hover_indicator(
    ui: &egui::Ui,
    band: egui::layers::ShapeIdx,
    id: impl std::hash::Hash,
    response: &Response,
) {
    let t = Motion::animate(ui.ctx(), id, response.hovered(), Motion::FAST);
    if t <= 0.0 {
        return;
    }
    let row = response.rect;
    ui.painter().set(
        band,
        egui::Shape::rect_filled(row, Style::RADIUS_M, Style::SURFACE_HI.gamma_multiply(t)),
    );
    // The accent tab paints after the card (on top), eased in over the same hover
    // progress; the card's SP_M padding keeps it clear of the row content.
    ui.painter().rect_filled(
        hover_tab_rect(row),
        Style::RADIUS_S,
        Style::ACCENT_MEDIA.gamma_multiply(Motion::hover_lift(t)),
    );
}

/// The slim leading **accent-tab** rect for a hovered row: [`Style::SP_XS`] wide,
/// pinned to the row's left edge and inset top and bottom by [`Style::SP_S`] so it
/// reads as a tab rather than a full-height rule. Pure, so the geometry stays on
/// the 8px grid and is unit-tested without a GPU.
fn hover_tab_rect(row: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(row.left(), row.top() + Style::SP_S),
        egui::pos2(row.left() + Style::SP_XS, row.bottom() - Style::SP_S),
    )
}

/// One clickable album row: title over the `artist · tracks · year` subtitle, in
/// a bordered surface that turns the cursor to a pointing hand on hover. Rendered
/// through the shared `Style` visuals (no raw colours).
fn album_row(ui: &mut egui::Ui, album: &Album) -> Response {
    // Reserve the wash slot so it paints BEHIND the row content (the repo idiom).
    let band = ui.painter().add(egui::Shape::Noop);
    let group = mde_egui::card().show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.vertical(|ui| {
            ui.label(
                RichText::new(&album.name)
                    .size(Style::BODY)
                    .strong()
                    .color(Style::TEXT),
            );
            let subtitle = album_subtitle(album);
            if !subtitle.is_empty() {
                mde_egui::muted_note(ui, subtitle);
            }
        });
    });
    let response = group
        .response
        .interact(Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    hover_indicator(ui, band, ("album-row", album.id.as_str()), &response);
    response
}

/// One clickable track row: track number, title, and right-aligned duration.
/// Clicking the row plays the track. `index` provides a 1-based fallback number
/// when the server didn't tag the track.
fn track_row(ui: &mut egui::Ui, index: usize, song: &Song) -> Response {
    let number = song
        .track
        .map_or_else(|| (index + 1).to_string(), |t| t.to_string());
    // Reserve the wash slot so it paints BEHIND the row content (the repo idiom).
    let band = ui.painter().add(egui::Shape::Noop);
    let group = mde_egui::card().show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{number:>2}"))
                    .monospace()
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
            ui.add_space(Style::SP_S);
            ui.label(
                RichText::new(&song.title)
                    .size(Style::BODY)
                    .color(Style::TEXT),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(format_duration(u64::from(song.duration)))
                        .monospace()
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
            });
        });
    });
    let response = group
        .response
        .interact(Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    hover_indicator(ui, band, ("track-row", song.id.as_str()), &response);
    response
}

#[cfg(test)]
mod tests {
    use super::{hover_tab_rect, music_panel, MusicApp};
    use crate::menubar::MenuAction;
    use crate::model::{Fetch, MusicState, Update};
    use mde_egui::egui::{self, pos2, vec2, Rect};
    use mde_egui::Style;
    use mde_musicd::airsonic::{Album, Song};
    use std::sync::mpsc;

    fn album(id: &str) -> Album {
        Album {
            id: id.to_string(),
            name: format!("Album {id}"),
            artist: "Artist".to_string(),
            artist_id: String::new(),
            song_count: 2,
            cover_art: String::new(),
            year: Some(2021),
        }
    }

    fn song(id: &str) -> Song {
        Song {
            id: id.to_string(),
            title: format!("Track {id}"),
            album: "Album".to_string(),
            artist: "Artist".to_string(),
            duration: 180,
            track: None,
            suffix: "flac".to_string(),
            cover_art: String::new(),
        }
    }

    /// Build a `MusicApp` around a given `state` with no worker and no credentials
    /// — the embedded case a shell would drive, minus the daemon. `music_panel`
    /// never touches the update channel, so a disconnected receiver is fine.
    fn app_with(state: MusicState, setup_error: Option<String>) -> MusicApp {
        let (_tx, rx) = mpsc::channel::<Update>();
        MusicApp {
            state,
            commands: None,
            updates: rx,
            server: "airsonic.mesh:4040".to_string(),
            setup_error,
        }
    }

    /// Drive one headless egui frame that shows `music_panel`, then tessellate the
    /// result on the CPU so any paint-path fault (bad shape/text/geometry) surfaces
    /// as a test failure. This is the same `Context::run` → `tessellate` path the
    /// DRM runner drives, minus the GPU — no window, no wgpu, no sound device — so
    /// the embeddable panel is proven runtime-reachable in `cargo test`.
    fn render(app: &mut MusicApp) {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(480.0, 360.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| music_panel(ui, app));
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty(), "frame produced no draw primitives");
    }

    #[test]
    fn setup_needed_renders_without_credentials() {
        // No creds ⇒ the honest "connect a server" state (§7), the path an
        // unconfigured embed opens to — rendered end-to-end, no worker spawned.
        let mut app = app_with(
            MusicState::new(),
            Some("no music server configured (run `mde-musicd --first-run`)".to_string()),
        );
        render(&mut app);
    }

    #[test]
    fn library_listing_and_states_render() {
        // A populated library exercises album_row for every row.
        let mut ready = MusicState::new();
        ready.albums = Fetch::Ready(vec![album("1"), album("2")]);
        render(&mut app_with(ready, None));

        // The loading / failed / empty branches each paint their honest line.
        let mut loading = MusicState::new();
        loading.albums = Fetch::Loading;
        render(&mut app_with(loading, None));

        let mut failed = MusicState::new();
        failed.albums = Fetch::Failed("server down".to_string());
        render(&mut app_with(failed, None));

        let mut empty = MusicState::new();
        empty.albums = Fetch::Ready(Vec::new());
        render(&mut app_with(empty, None));
    }

    #[test]
    fn open_album_with_tracks_and_error_banner_render() {
        // Transient engine error + a now-playing track + an open album with tracks
        // exercises the error banner and track_row alongside the album header.
        let mut state = MusicState::new();
        state.error = Some("audio output unavailable".to_string());
        state.now_playing = Some(song("42"));
        state.playing = true;
        state.open(album("7"));
        state.open_album.as_mut().expect("an album is open").tracks =
            Fetch::Ready(vec![song("a"), song("b")]);
        render(&mut app_with(state, None));
    }

    #[test]
    fn menu_back_to_library_closes_the_open_album() {
        // The View → Back to Library menu action drives the same `close` seam the
        // album view's button does — a real navigation seam, not a no-op.
        let mut state = MusicState::new();
        state.open(album("7"));
        let mut app = app_with(state, None);
        assert!(app.state.open_album.is_some());
        app.run_menu_action(MenuAction::BackToLibrary);
        assert!(
            app.state.open_album.is_none(),
            "Back to Library returned to the listing"
        );
    }

    #[test]
    fn menu_context_snapshots_transport_and_connection() {
        // A worker-less fixture (no creds) with a track playing + an album open:
        // the context mirrors the live state the bar gates + renders from.
        let mut state = MusicState::new();
        state.now_playing = Some(song("42"));
        state.playing = true;
        state.position_ms = 5_000;
        state.open(album("7"));
        let app = app_with(state, None);
        let cx = app.menu_context();
        assert!(!cx.connected, "app_with spawns no worker");
        assert!(cx.has_track && cx.playing);
        assert!(cx.album_open);
        let np = cx.now_playing.expect("a track is playing");
        assert_eq!(np.title, "Track 42");
        // 5000ms → 5s, clamped to the 180s tagged length.
        assert_eq!(np.elapsed_secs, 5);
        assert_eq!(np.duration_secs, 180);
    }

    #[test]
    fn album_rows_adopt_the_shared_card_primitive() {
        // The library/album rows are the shared `mde_egui::card()` surface, so their
        // depth is the foundation's Raised elevation verbatim — no per-surface shadow
        // helper is minted here (§4). A translucent umbra keeps it a soft shadow,
        // never an opaque fill (design lock #2).
        use mde_egui::style::Elevation;
        let card = mde_egui::card();
        assert_eq!(
            card.shadow,
            Elevation::Raised.egui_shadow(),
            "the row card casts the shared Raised soft shadow"
        );
        assert_eq!(
            card.fill,
            Style::SURFACE,
            "the row card fills the base surface"
        );
        let alpha = card.shadow.color.a();
        assert!(
            alpha > 0 && alpha < 255,
            "a Raised card casts a translucent soft shadow (lock #2), got alpha {alpha}"
        );
    }

    #[test]
    fn hover_accent_tab_sits_on_the_left_edge_on_the_grid() {
        // The hovered-row leading accent tab is a pure geometry on the 8px grid:
        // pinned to the row's left edge, one half-step wide, and inset top and
        // bottom by the base unit so it reads as a tab — every extent a Style
        // token, none a raw literal (§4). A tall row keeps the insets well-formed.
        let row = Rect::from_min_max(pos2(10.0, 20.0), pos2(210.0, 80.0));
        let tab = hover_tab_rect(row);
        assert_eq!(tab.left(), row.left(), "the tab hugs the row's left edge");
        assert_eq!(tab.width(), Style::SP_XS, "one 8px-grid half-step wide");
        assert_eq!(
            tab.top(),
            row.top() + Style::SP_S,
            "inset from the top by the base grid unit"
        );
        assert_eq!(
            tab.bottom(),
            row.bottom() - Style::SP_S,
            "inset from the bottom by the base grid unit"
        );
        assert!(
            row.contains_rect(tab),
            "the tab stays within the row it marks"
        );
    }
}
