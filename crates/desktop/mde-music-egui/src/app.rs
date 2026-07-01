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
use mde_egui::Style;

use mde_musicd::airsonic::{Album, Client, Song};
use mde_musicd::creds;

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
        let (update_tx, update_rx) = mpsc::channel::<Update>();
        let mut state = MusicState::new();
        match creds::load() {
            Ok(c) => {
                let client = Client::new(c.server_url, c.username, &c.password);
                let server = client.base_url().to_string();
                let commands = worker::spawn(client, cc.egui_ctx.clone(), update_tx);
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

    /// The header strip: the surface title, the server host, and the transport
    /// controls for the now-playing track.
    fn render_header(&self, ui: &mut egui::Ui) {
        let mut cmd: Option<Command> = None;
        ui.add_space(Style::SP_XS);
        ui.horizontal(|ui| {
            ui.add_space(Style::SP_S);
            ui.heading(
                RichText::new("Music")
                    .size(Style::HEADING)
                    .color(Style::TEXT),
            );
            if !self.server.is_empty() {
                ui.add_space(Style::SP_M);
                ui.label(
                    RichText::new(&self.server)
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
            }
            // Transport, pinned to the right edge.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(Style::SP_S);
                if let Some(song) = &self.state.now_playing {
                    if ui.button("Stop").clicked() {
                        cmd = Some(Command::Stop);
                    }
                    let (label, intent) = if self.state.playing {
                        ("Pause", Command::Pause)
                    } else {
                        ("Resume", Command::Resume)
                    };
                    if ui.button(label).clicked() {
                        cmd = Some(intent);
                    }
                    ui.add_space(Style::SP_M);
                    ui.label(
                        RichText::new(format!("{} — {}", song.title, song.artist))
                            .size(Style::BODY)
                            .color(Style::ACCENT),
                    );
                    let state_word = if self.state.playing {
                        "Now playing"
                    } else {
                        "Paused"
                    };
                    // Live elapsed / total from the worker's playhead poll. Clamp
                    // the elapsed value to the tagged length so a slightly-ahead
                    // playhead never reads past the end; when the server gave no
                    // length (e.g. a stream), show the elapsed time on its own.
                    let elapsed = self.state.position_ms / 1000;
                    let status = if song.duration > 0 {
                        format!(
                            "{state_word} · {} / {}",
                            format_duration(elapsed.min(u64::from(song.duration))),
                            format_duration(u64::from(song.duration)),
                        )
                    } else {
                        format!("{state_word} · {}", format_duration(elapsed))
                    };
                    ui.label(
                        RichText::new(status)
                            .size(Style::SMALL)
                            .color(Style::TEXT_DIM),
                    );
                } else {
                    ui.label(
                        RichText::new("Nothing playing")
                            .size(Style::SMALL)
                            .color(Style::TEXT_DIM),
                    );
                }
            });
        });
        ui.add_space(Style::SP_XS);
        if let Some(c) = cmd {
            self.send(c);
        }
    }

    /// The album library listing (or its loading/empty/error state).
    fn render_library(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("Library")
                .size(Style::BODY)
                .strong()
                .color(Style::TEXT),
        );
        ui.add_space(Style::SP_XS);
        ui.separator();
        ui.add_space(Style::SP_S);

        let mut to_open: Option<Album> = None;
        match &self.state.albums {
            Fetch::Idle | Fetch::Loading => {
                ui.colored_label(Style::TEXT_DIM, "Loading library…");
            }
            Fetch::Failed(e) => {
                ui.colored_label(Style::DANGER, format!("Couldn't load the library: {e}"));
            }
            Fetch::Ready(albums) if albums.is_empty() => {
                ui.colored_label(Style::TEXT_DIM, "This server has no albums yet.");
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
                    ui.colored_label(Style::TEXT_DIM, "Loading tracks…");
                }
                Fetch::Failed(e) => {
                    ui.colored_label(Style::DANGER, format!("Couldn't load tracks: {e}"));
                }
                Fetch::Ready(songs) if songs.is_empty() => {
                    ui.colored_label(Style::TEXT_DIM, "This album has no tracks.");
                }
                Fetch::Ready(songs) => {
                    ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for (i, song) in songs.iter().enumerate() {
                                if track_row(ui, i, song).clicked() {
                                    to_play = Some(song.clone());
                                }
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
/// "Quasar" §5 (surfaces are panels in the shell, not separate clients). It draws
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
/// pump**. The standalone [`MusicApp`]'s `update` calls this at the top of every
/// frame; the E12 shell (E12-3b) calls it for the mounted surface each frame too,
/// because the shell owns the one frame loop and never calls the surface's
/// `App::update`. Non-blocking (`try_recv`) and a no-op when the worker has sent
/// nothing — or when no worker is running (the unconfigured, no-creds surface).
pub fn music_pump(app: &mut MusicApp) {
    while let Ok(update) = app.updates.try_recv() {
        app.state.apply(update);
    }
}

/// Render the surface **header** strip — title · server host · transport controls
/// for the now-playing track — into `ui`. The standalone app frames it in the
/// window's top panel; the E12 shell renders it above the mounted [`music_panel`]
/// so the embedded surface keeps its transport + server identity, the same chrome
/// the standalone binary shows. The header stays out of [`music_panel`] because
/// the shell supplies its own surrounding chrome.
pub fn music_header(ui: &mut egui::Ui, app: &MusicApp) {
    app.render_header(ui);
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

/// One clickable album row: title over the `artist · tracks · year` subtitle, in
/// a bordered surface that turns the cursor to a pointing hand on hover. Rendered
/// through the shared `Style` visuals (no raw colours).
fn album_row(ui: &mut egui::Ui, album: &Album) -> Response {
    let group = ui.group(|ui| {
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
                ui.label(
                    RichText::new(subtitle)
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
            }
        });
    });
    group
        .response
        .interact(Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand)
}

/// One clickable track row: track number, title, and right-aligned duration.
/// Clicking the row plays the track. `index` provides a 1-based fallback number
/// when the server didn't tag the track.
fn track_row(ui: &mut egui::Ui, index: usize, song: &Song) -> Response {
    let number = song
        .track
        .map_or_else(|| (index + 1).to_string(), |t| t.to_string());
    let group = ui.group(|ui| {
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
    group
        .response
        .interact(Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand)
}

#[cfg(test)]
mod tests {
    use super::{music_panel, MusicApp};
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
}
