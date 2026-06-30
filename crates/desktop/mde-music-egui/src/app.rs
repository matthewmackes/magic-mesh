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
                    ui.label(
                        RichText::new(state_word)
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

impl App for MusicApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        // Drain everything the worker has sent since the last frame.
        while let Ok(update) = self.updates.try_recv() {
            self.state.apply(update);
        }

        egui::TopBottomPanel::top("music-header").show(ctx, |ui| {
            self.render_header(ui);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(detail) = &self.setup_error {
                render_setup_needed(ui, detail);
                return;
            }
            // A transient playback/engine error (e.g. no sound device on a
            // headless host) — rendered, not swallowed.
            if let Some(error) = self.state.error.clone() {
                ui.add_space(Style::SP_XS);
                ui.colored_label(Style::DANGER, error);
                ui.add_space(Style::SP_XS);
            }
            ui.add_space(Style::SP_S);
            if self.state.open_album.is_some() {
                self.render_album(ui);
            } else {
                self.render_library(ui);
            }
        });
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
                    RichText::new(format_duration(song.duration))
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
