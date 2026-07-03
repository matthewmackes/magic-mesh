//! The eframe app + the egui views (MEDIA-8): the full media surface rendered
//! entirely through the shared Carbon [`Style`] (§4 — no raw hex). Every view drives
//! the [`MediaController`] (and through it [`mde_media_core`]); nothing here holds
//! playback, index, or queue state of its own.
//!
//! The views are free functions over `&mut MediaController<E>` so the standalone
//! [`MediaApp`] and a future shell embed render the *same* bodies, and so each one is
//! headless-mount-tested (a real `Context::run` → `tessellate`, no GPU) at the bottom
//! of this file. Clicks become [`TransportAction`]s that flow to the core, exactly as
//! the sibling surfaces drive their daemons.

use mde_egui::eframe::{self, App, CreationContext};
use mde_egui::egui::{
    self, Align, Align2, Context, CursorIcon, FontId, Layout, Response, RichText, ScrollArea,
    Sense, Slider,
};
use mde_egui::{muted_note, status_dot, Style};

use mde_jellyfin::{
    BaseItemDto, ClientInfo, ItemsQuery, JellyfinClient, ReqwestTransport, ServerConfig,
};
use mde_media_core::{
    EqBand, LoudnessNorm, MediaEngine, MediaKind, PlayerState, ReplayGainMode, ScreenshotMode,
    SortKey, TrackKind, V4l2Cli, YtDlpCli,
};

use crate::model::{
    capture_detail, format_time, item_title, jellyfin_item_title, library_row_texts,
    now_playing_title, osd_should_show, play_pause_label, progress_fraction, repeat_label,
    state_word, track_label, MediaController, MediaTab, TransportAction, EBU_R128_DEFAULT,
    EQ_GAIN_DB_LIMIT,
};
use crate::{build_engine, Engine};

/// The alpha/darken factor applied to [`Style::BG`] for the translucent dark media OSD
/// scrim drawn over the video (design Q34). Derived from the palette token — the same
/// translucency-by-factor idiom [`Style`] itself uses for its selection fill — so no
/// raw colour is introduced (§4).
const OSD_SCRIM: f32 = 0.72;

/// The height of the video stage, on the 8px grid (a token multiple, not a magic px).
const STAGE_HEIGHT: f32 = Style::SP_XL * 6.0;

/// The playback-speed presets the Player view offers.
const SPEED_PRESETS: [f64; 5] = [0.5, 1.0, 1.25, 1.5, 2.0];

/// The seek step (seconds) of the skip-back / skip-forward transport buttons.
const SKIP_SECS: f64 = 10.0;

/// How many frames between playback-roaming convergence polls (MEDIA-16). The poll
/// reads the shared session plane, so it runs on a coarse cadence (~1 s at 60 fps),
/// not every frame — the same human-paced convergence the mesh workers use.
const ROAM_POLL_INTERVAL_FRAMES: u32 = 60;

/// The media surface: the controller plus the applied-fullscreen mirror so the app
/// only issues a viewport command when the immersive state actually flips.
pub struct MediaApp {
    controller: MediaController<Engine>,
    applied_fullscreen: bool,
    /// Frames since start, gating the MEDIA-16 roaming poll to a coarse cadence.
    roam_poll_frames: u32,
}

impl MediaApp {
    /// Build the surface over the default engine (airgap-safe `FakeMpv`, or the real
    /// mpv engine under `--features mpv`). It opens to the honest first-run Sources
    /// view — no media indexed yet, never faked (§7).
    #[must_use]
    pub fn new(_cc: &CreationContext<'_>) -> Self {
        Self::with_engine()
    }

    /// Build over an egui [`Context`] directly — the DRM-seat shell path has only the
    /// bare context, no eframe `CreationContext`. Kept for parity with the sibling
    /// surfaces' two entry points.
    #[must_use]
    pub fn new_with_ctx(_ctx: &egui::Context) -> Self {
        Self::with_engine()
    }

    fn with_engine() -> Self {
        let mut controller = MediaController::new(mde_media_core::Player::new(build_engine()));
        // MEDIA-16: pick up a roaming playback session — resume where another seat
        // left off + take the single owned lease. Best-effort: a seat with no mesh
        // workgroup root is a silent honest no-op (never a fabricated resume).
        controller.enable_roaming_default();
        Self {
            controller,
            applied_fullscreen: false,
            roam_poll_frames: 0,
        }
    }
}

impl App for MediaApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        accumulate_osd_idle(ctx, &mut self.controller);
        media_pump(&mut self.controller);

        // MEDIA-16: converge the roaming lease on a coarse cadence — checkpoint this
        // seat's position while it owns the session, or release (pause) when another
        // seat has claimed it, so only one seat ever plays.
        self.roam_poll_frames = self.roam_poll_frames.wrapping_add(1);
        if self.roam_poll_frames % ROAM_POLL_INTERVAL_FRAMES == 0 {
            self.controller.poll_roaming();
        }

        // Immersive fullscreen (design Q32) — sync only on a flip so we don't spam the
        // viewport each frame.
        let want_fullscreen = self.controller.ui().fullscreen;
        if want_fullscreen != self.applied_fullscreen {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(want_fullscreen));
            self.applied_fullscreen = want_fullscreen;
        }

        egui::TopBottomPanel::top("media-header")
            .show(ctx, |ui| media_header(ui, &mut self.controller));
        egui::CentralPanel::default().show(ctx, |ui| media_panel(ui, &mut self.controller));

        // The PiP mini-player (design Q31/Q32) floats above whatever view is active.
        pip_window(ctx, &mut self.controller);

        // Keep the frame loop ticking while playing so the core's live clock advances.
        if self.controller.is_playing() {
            ctx.request_repaint();
        }
    }
}

/// Advance the core one tick — the per-frame state pump (mirrors the sibling surfaces'
/// `*_pump`). The standalone app calls it each frame; a shell embed would too.
pub fn media_pump<E: MediaEngine>(controller: &mut MediaController<E>) {
    controller.pump();
}

/// Accumulate pointer-idle time for the OSD auto-hide: reset on any pointer motion or
/// press, else add the frame's delta.
fn accumulate_osd_idle<E: MediaEngine>(ctx: &Context, controller: &mut MediaController<E>) {
    let (dt, active) = ctx.input(|i| {
        let moving = i.pointer.velocity() != egui::Vec2::ZERO;
        (f64::from(i.stable_dt), moving || i.pointer.any_down())
    });
    let ui = controller.ui_mut();
    if active {
        ui.osd_idle_secs = 0.0;
    } else {
        ui.osd_idle_secs += dt;
    }
}

// ── header ───────────────────────────────────────────────────────────────────────

/// The header strip: the app title, the tab bar, the now-playing summary, and the
/// fullscreen / mini-player toggles.
pub fn media_header<E: MediaEngine>(ui: &mut egui::Ui, controller: &mut MediaController<E>) {
    ui.add_space(Style::SP_XS);
    let mut chosen_tab = controller.ui().tab;
    let mut toggle_pip = false;
    let mut toggle_fullscreen = false;

    ui.horizontal(|ui| {
        ui.add_space(Style::SP_S);
        ui.heading(
            RichText::new("Media")
                .size(Style::HEADING)
                .color(Style::TEXT),
        );
        ui.add_space(Style::SP_M);
        for tab in MediaTab::all() {
            if ui
                .selectable_label(controller.ui().tab == tab, tab.label())
                .clicked()
            {
                chosen_tab = tab;
            }
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(Style::SP_S);
            let pip_label = if controller.ui().pip {
                "Mini-player ✓"
            } else {
                "Mini-player"
            };
            if ui.button(pip_label).clicked() {
                toggle_pip = true;
            }
            let fs_label = if controller.ui().fullscreen {
                "Exit fullscreen"
            } else {
                "Fullscreen"
            };
            if ui.button(fs_label).clicked() {
                toggle_fullscreen = true;
            }
            ui.add_space(Style::SP_M);
            // A live state dot + the now-playing title.
            let dot = match controller.player().state() {
                PlayerState::Playing => Style::OK,
                PlayerState::Paused | PlayerState::Loading => Style::WARN,
                PlayerState::Idle | PlayerState::Stopped | PlayerState::Ended => Style::TEXT_DIM,
            };
            status_dot(ui, dot);
            ui.label(
                RichText::new(now_playing_title(controller.player()))
                    .size(Style::BODY)
                    .color(Style::ACCENT),
            );
        });
    });
    ui.add_space(Style::SP_XS);

    controller.ui_mut().tab = chosen_tab;
    if toggle_pip {
        let now = controller.ui().pip;
        controller.ui_mut().pip = !now;
    }
    if toggle_fullscreen {
        let now = controller.ui().fullscreen;
        controller.ui_mut().fullscreen = !now;
    }
}

// ── central panel router ───────────────────────────────────────────────────────────

/// Render the active view's body into `ui`, then the transient status line.
pub fn media_panel<E: MediaEngine>(ui: &mut egui::Ui, controller: &mut MediaController<E>) {
    match controller.ui().tab {
        MediaTab::Sources => sources_view(ui, controller),
        MediaTab::Library => library_view(ui, controller),
        MediaTab::Player => player_view(ui, controller),
        MediaTab::Queue => queue_view(ui, controller),
    }
    status_line(ui, controller);
}

/// The transient status / error line at the foot of the panel (honest, never
/// swallowed — §7). A dim caption when there is nothing to say.
fn status_line<E: MediaEngine>(ui: &mut egui::Ui, controller: &MediaController<E>) {
    ui.add_space(Style::SP_S);
    ui.separator();
    match controller.ui().status.as_deref() {
        Some(msg) => {
            ui.colored_label(Style::TEXT_DIM, RichText::new(msg).size(Style::SMALL));
        }
        None => {
            muted_note(
                ui,
                format!("{} · ready", state_word(controller.player().state())),
            );
        }
    }
}

// ── Sources view ───────────────────────────────────────────────────────────────────

/// The Sources view: the "index a folder" field, the indexed local roots, the
/// configured Jellyfin servers (MEDIA-10 — browse + play), and an honest note
/// about where mesh sources land (MEDIA-14/15, not yet wired).
fn sources_view<E: MediaEngine>(ui: &mut egui::Ui, controller: &mut MediaController<E>) {
    section_title(ui, "Sources");

    // The add-a-folder row.
    let mut folder = controller.ui().folder_input.clone();
    let mut do_index = false;
    ui.horizontal(|ui| {
        let field = egui::TextEdit::singleline(&mut folder)
            .hint_text("/path/to/media")
            .desired_width(Style::SP_XL * 8.0);
        ui.add(field);
        if ui.button("Index folder").clicked() {
            do_index = true;
        }
    });
    controller.ui_mut().folder_input = folder;
    if do_index {
        controller.index_current_folder();
    }

    // The open-a-URL row (MEDIA-12): a direct stream, or a web link resolved by yt-dlp.
    open_url_row(ui, controller);

    ui.add_space(Style::SP_S);

    let mut open_source: Option<String> = None;
    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // ── local roots ──
            let rows = controller.sources();
            if rows.is_empty() {
                muted_note(
                    ui,
                    "No local sources yet — index a folder above to build the library.",
                );
            } else {
                for row in &rows {
                    let resp = ui.group(|ui| {
                        ui.set_min_width(ui.available_width());
                        ui.horizontal(|ui| {
                            status_dot(ui, Style::OK);
                            ui.label(
                                RichText::new(&row.label)
                                    .size(Style::BODY)
                                    .strong()
                                    .color(Style::TEXT),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                muted_note(ui, format!("{} item(s)", row.item_count));
                            });
                        });
                        muted_note(ui, &row.path);
                    });
                    if resp
                        .response
                        .interact(Sense::click())
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .clicked()
                    {
                        open_source = Some(row.path.clone());
                    }
                    ui.add_space(Style::SP_XS);
                }
            }

            // ── capture devices (MEDIA-13) ──
            capture_section(ui, controller);

            // ── Jellyfin servers (MEDIA-10) ──
            jellyfin_section(ui, controller);

            ui.add_space(Style::SP_S);
            muted_note(
                ui,
                "Mesh sources appear here once discovered (MEDIA-14/15).",
            );
        });

    // Clicking a local source jumps to the Library filtered to that root's path.
    if let Some(path) = open_source {
        controller.set_search(path);
        controller.ui_mut().tab = MediaTab::Library;
    }
}

/// The "Open URL" row (MEDIA-12): a field for a direct stream URL (`http(s)`/`hls`/
/// `rtsp`/`mms`/`rtmp`/`srt`) or a web-page link, and an Open button that routes it
/// through [`MediaController::open_url`] — direct streams to the core Player, web
/// links resolved by the bundled `yt-dlp`. All chrome is drawn from Carbon [`Style`]
/// tokens (§4). The live resolve is honest-gated on `yt-dlp` being present.
fn open_url_row<E: MediaEngine>(ui: &mut egui::Ui, controller: &mut MediaController<E>) {
    let mut url = controller.ui().url_input.clone();
    let mut do_open = false;
    ui.horizontal(|ui| {
        let field = egui::TextEdit::singleline(&mut url)
            .hint_text("https://…  ·  rtsp://…  ·  a web video link")
            .desired_width(Style::SP_XL * 8.0);
        let resp = ui.add(field);
        // Enter in the field, or the button, opens it.
        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            do_open = true;
        }
        if ui.button("Open URL").clicked() {
            do_open = true;
        }
    });
    controller.ui_mut().url_input = url;
    if do_open {
        // The real subprocess resolver; runtime-gated (honest "not installed" when
        // absent — §7). Only invoked on an explicit Open, so headless renders never
        // spawn it. The controller sets the status line for every outcome.
        let target = controller.ui().url_input.clone();
        let _ = controller.open_url(&target, &YtDlpCli);
    }
}

/// The "Capture devices" sub-section of Sources (MEDIA-13): a Scan button that
/// enumerates the local v4l2 capture inputs (webcams / TV tuners / capture cards)
/// through the real [`V4l2Cli`], then a row per playable device with a Watch action
/// that opens it in the core Player over `av://v4l2:/dev/videoN`. All chrome is drawn
/// from Carbon [`Style`] tokens (§4). Honest-gated (§7): before a scan it shows a plain
/// hint; a host with no device — or no v4l2 tooling — shows a plain "no capture devices
/// found" note, never a fake device.
fn capture_section<E: MediaEngine>(ui: &mut egui::Ui, controller: &mut MediaController<E>) {
    ui.add_space(Style::SP_S);
    let mut do_scan = false;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Capture devices")
                .size(Style::SMALL)
                .strong()
                .color(Style::TEXT_DIM),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.button("Scan").clicked() {
                do_scan = true;
            }
        });
    });
    ui.add_space(Style::SP_XS);

    if do_scan {
        // The real v4l2 enumerator; runtime-gated (honest "no devices" when absent —
        // §7). Only invoked on an explicit Scan, so headless renders never spawn it.
        // The controller sets the status line for every outcome.
        controller.refresh_capture_devices(&V4l2Cli);
    }

    // Before any scan, invite one; after, distinguish "none found" from a device list.
    if !controller.capture().probed() {
        muted_note(
            ui,
            "Scan for local capture inputs (webcams, TV tuners, capture cards).",
        );
        return;
    }
    // Collect owned row data so the immutable borrow of `controller` is released
    // before the (mutable) Watch open below.
    let rows: Vec<(String, String, String)> = controller
        .capture()
        .playable()
        .iter()
        .filter_map(|device| {
            device
                .path()
                .map(|path| (device.name.clone(), path.to_owned(), capture_detail(device)))
        })
        .collect();
    if rows.is_empty() {
        muted_note(ui, "No capture devices found.");
        return;
    }

    let mut open: Option<String> = None;
    for (name, path, detail) in &rows {
        ui.horizontal(|ui| {
            status_dot(ui, Style::OK);
            ui.label(RichText::new(name).size(Style::BODY).color(Style::TEXT));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Watch").clicked() {
                    open = Some(path.clone());
                }
                muted_note(ui, detail);
            });
        });
        ui.add_space(Style::SP_XS);
    }
    if let Some(path) = open {
        let _ = controller.open_capture_device(&path);
    }
}

/// The Jellyfin sub-section of the Sources view: add a server, list the configured
/// servers (Connect browses their libraries) with a per-server user-profile
/// switcher (MEDIA-11), the browsed titles — a click negotiates a
/// [`PlaybackDecision`](mde_jellyfin::PlaybackDecision) and drives the core Player
/// (MEDIA-10), a Download for offline — and the downloaded (offline) list a click
/// plays with no server (MEDIA-11). The live browse/play/download legs need a real
/// server (honest-gated); the negotiation + cache folds are tested in `model`.
#[allow(clippy::too_many_lines)]
fn jellyfin_section<E: MediaEngine>(ui: &mut egui::Ui, controller: &mut MediaController<E>) {
    ui.add_space(Style::SP_S);
    ui.label(
        RichText::new("Jellyfin servers")
            .size(Style::SMALL)
            .strong()
            .color(Style::TEXT_DIM),
    );
    ui.add_space(Style::SP_XS);

    // Add-a-server row (name + base URL).
    let mut name = controller.ui().jellyfin_name_input.clone();
    let mut url = controller.ui().jellyfin_url_input.clone();
    let mut add = false;
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut name)
                .hint_text("Name")
                .desired_width(Style::SP_XL * 3.0),
        );
        ui.add(
            egui::TextEdit::singleline(&mut url)
                .hint_text("https://jelly.mesh:8096")
                .desired_width(Style::SP_XL * 5.0),
        );
        if ui.button("Add server").clicked() {
            add = true;
        }
    });
    controller.ui_mut().jellyfin_name_input = name;
    controller.ui_mut().jellyfin_url_input = url;
    if add {
        add_jellyfin_from_inputs(controller);
    }

    // Configured servers, each with its user-profile switcher (MEDIA-11).
    let rows = controller.jellyfin_sources();
    let mut connect_id: Option<String> = None;
    let mut select_id: Option<String> = None;
    let mut switch_profile: Option<(String, String)> = None;
    if rows.is_empty() {
        muted_note(
            ui,
            "No Jellyfin servers yet — add one above, then Connect to browse.",
        );
    } else {
        for row in &rows {
            ui.horizontal(|ui| {
                status_dot(
                    ui,
                    if row.signed_in {
                        Style::OK
                    } else {
                        Style::TEXT_DIM
                    },
                );
                let color = if row.selected {
                    Style::ACCENT
                } else {
                    Style::TEXT
                };
                let clicked = ui
                    .label(RichText::new(&row.label).size(Style::BODY).color(color))
                    .interact(Sense::click())
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .clicked();
                if clicked {
                    select_id = Some(row.id.clone());
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Connect").clicked() {
                        connect_id = Some(row.id.clone());
                    }
                });
            });
            muted_note(ui, &row.base_url);
            // The per-server profile switcher: a chip per user, the active one lit.
            if !row.profiles.is_empty() {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Profile")
                            .size(Style::SMALL)
                            .color(Style::TEXT_DIM),
                    );
                    for profile in &row.profiles {
                        if ui
                            .selectable_label(profile.active, &profile.label)
                            .clicked()
                        {
                            switch_profile = Some((row.id.clone(), profile.user_id.clone()));
                        }
                    }
                });
            }
            ui.add_space(Style::SP_XS);
        }
    }
    if let Some(id) = select_id {
        controller.select_jellyfin_server(&id);
    }
    if let Some((server_id, user_id)) = switch_profile {
        controller.switch_jellyfin_profile(&server_id, &user_id);
    }
    if let Some(id) = connect_id {
        connect_jellyfin(controller, &id);
    }

    // Browsed titles — a click plays through the core Player; Download caches for
    // offline (MEDIA-11), and a cached title shows an offline badge.
    let items = controller.jellyfin_items().to_vec();
    let mut play_index: Option<usize> = None;
    let mut download_index: Option<usize> = None;
    if !items.is_empty() {
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new("Titles")
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
        for (index, item) in items.iter().enumerate() {
            let cached = controller.is_offline_available(&item.id);
            ui.horizontal(|ui| {
                let clicked = ui
                    .label(
                        RichText::new(jellyfin_item_title(item))
                            .size(Style::BODY)
                            .color(Style::TEXT),
                    )
                    .interact(Sense::click())
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .clicked();
                if clicked {
                    play_index = Some(index);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if cached {
                        ui.colored_label(Style::OK, RichText::new("Offline ✓").size(Style::SMALL));
                    } else if ui.button("Download").clicked() {
                        download_index = Some(index);
                    }
                });
            });
            ui.add_space(Style::SP_XS);
        }
    }
    if let Some(index) = play_index {
        if let Some(item) = controller.jellyfin_items().get(index).cloned() {
            if controller.play_jellyfin_item(&item).is_ok() {
                controller.ui_mut().tab = MediaTab::Player;
            }
        }
    }
    if let Some(index) = download_index {
        if let Some(item) = controller.jellyfin_items().get(index).cloned() {
            download_jellyfin(controller, &item);
        }
    }

    // The downloaded (offline) list — a click plays with no server (MEDIA-11).
    offline_section(ui, controller);
}

/// The offline (downloaded) titles sub-section: the cache usage, then a row per
/// downloaded title with Play (offline, no network) + Remove (MEDIA-11).
fn offline_section<E: MediaEngine>(ui: &mut egui::Ui, controller: &mut MediaController<E>) {
    let rows = controller.offline_rows();
    if rows.is_empty() {
        return;
    }
    ui.add_space(Style::SP_S);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Downloaded (offline)")
                .size(Style::SMALL)
                .strong()
                .color(Style::TEXT_DIM),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            muted_note(ui, controller.offline_usage());
        });
    });
    ui.add_space(Style::SP_XS);

    let mut play_offline: Option<String> = None;
    let mut evict: Option<String> = None;
    for row in &rows {
        ui.horizontal(|ui| {
            status_dot(ui, Style::OK);
            let clicked = ui
                .label(
                    RichText::new(&row.label)
                        .size(Style::BODY)
                        .color(Style::TEXT),
                )
                .interact(Sense::click())
                .on_hover_cursor(CursorIcon::PointingHand)
                .clicked();
            if clicked {
                play_offline = Some(row.item_id.clone());
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Remove").clicked() {
                    evict = Some(row.item_id.clone());
                }
                muted_note(ui, &row.size);
            });
        });
        ui.add_space(Style::SP_XS);
    }
    if let Some(item_id) = play_offline {
        if controller
            .play_offline_item(&item_id, mde_jellyfin::cache::unix_now())
            .is_ok()
        {
            controller.ui_mut().tab = MediaTab::Player;
        }
    }
    if let Some(item_id) = evict {
        controller.evict_offline_item(&item_id);
    }
}

/// The client identity the surface presents to a Jellyfin server (the token is
/// bound to the `DeviceId`).
fn jellyfin_device() -> ClientInfo {
    ClientInfo::new(
        "mde-media",
        "mde-media-egui",
        "mde-media-egui-seat",
        "12.0.0",
    )
}

/// Add / update a Jellyfin server from the Sources input fields (no network).
/// The base URL is its stable id; an empty URL is reported honestly.
fn add_jellyfin_from_inputs<E: MediaEngine>(controller: &mut MediaController<E>) {
    let url = controller.ui().jellyfin_url_input.trim().to_owned();
    if url.is_empty() {
        controller.ui_mut().status = Some("Enter a Jellyfin server URL.".to_owned());
        return;
    }
    let name = controller.ui().jellyfin_name_input.trim().to_owned();
    let label = if name.is_empty() { url.clone() } else { name };
    controller.add_jellyfin_server(url.clone(), label, url.clone());
    controller.select_jellyfin_server(&url);
    controller.ui_mut().jellyfin_name_input.clear();
    controller.ui_mut().jellyfin_url_input.clear();
    controller.ui_mut().status = Some(format!("Added Jellyfin server {url}."));
}

/// Build a real [`ReqwestTransport`] client for `server`, carrying its active
/// profile's saved token when signed in. The shared factory behind Connect +
/// Download; a live server is honest-gated (only the wire leg needs egress).
fn jellyfin_client(server: &ServerConfig) -> Result<JellyfinClient<ReqwestTransport>, String> {
    let transport =
        ReqwestTransport::new().map_err(|e| format!("Jellyfin transport error: {e}"))?;
    let mut client = JellyfinClient::new(server.base_url.clone(), jellyfin_device(), transport);
    if let Some(auth) = server.active_auth() {
        client = client.with_auth(auth.access_token.clone(), auth.user_id.clone());
    }
    Ok(client)
}

/// Connect to a configured Jellyfin server: build a real client and browse its
/// playable titles into the Sources list (hydrating `MediaSources` so a click can
/// negotiate). A live server is honest-gated — the blocking fetch is deliberately
/// simple; only the wire leg needs egress.
fn connect_jellyfin<E: MediaEngine>(controller: &mut MediaController<E>, server_id: &str) {
    let Some(server) = controller.jellyfin().server(server_id).cloned() else {
        controller.ui_mut().status = Some("Unknown Jellyfin server.".to_owned());
        return;
    };
    let client = match jellyfin_client(&server) {
        Ok(client) => client,
        Err(message) => {
            controller.ui_mut().status = Some(message);
            return;
        }
    };
    // Browse the server's playable leaves, hydrating MediaSources for negotiation.
    let query = ItemsQuery::default()
        .recursive()
        .include_item_types(["Movie", "Episode", "Audio"])
        .sort_by(["SortName"])
        .fields(["Overview", "Genres", "MediaSources"]);
    controller.select_jellyfin_server(server_id);
    match controller.browse_jellyfin(&client, &query) {
        Ok(count) => {
            controller.ui_mut().status =
                Some(format!("Loaded {count} title(s) from {}.", server.name));
        }
        Err(message) => controller.ui_mut().status = Some(message),
    }
}

/// Download a browsed title's bytes into the offline cache (MEDIA-11) — build a
/// real client for the selected server and store the untouched direct-play bytes.
/// A live server is honest-gated; the cache write + lifecycle are tested in
/// `model`. The controller sets the success status; a failure is surfaced here.
fn download_jellyfin<E: MediaEngine>(controller: &mut MediaController<E>, item: &BaseItemDto) {
    let Some(server) = controller.jellyfin().selected_server().cloned() else {
        controller.ui_mut().status = Some("Select a Jellyfin server first.".to_owned());
        return;
    };
    let client = match jellyfin_client(&server) {
        Ok(client) => client,
        Err(message) => {
            controller.ui_mut().status = Some(message);
            return;
        }
    };
    if let Err(message) =
        controller.download_jellyfin_item(&client, item, mde_jellyfin::cache::unix_now())
    {
        controller.ui_mut().status = Some(message);
    }
}

// ── Library view ───────────────────────────────────────────────────────────────────

/// The Library browse view: the search field, the kind filter, the sort controls, and
/// the [`MediaController::visible_items`] fold (MEDIA-7) as clickable rows.
#[allow(clippy::too_many_lines)]
fn library_view<E: MediaEngine>(ui: &mut egui::Ui, controller: &mut MediaController<E>) {
    section_title(ui, "Library");

    // Search.
    let mut search = controller.ui().search_input.clone();
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Search")
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
        ui.add_space(Style::SP_S);
        let field = egui::TextEdit::singleline(&mut search)
            .hint_text("title / artist / album / path")
            .desired_width(Style::SP_XL * 8.0);
        if ui.add(field).changed() {
            controller.set_search(search.clone());
        }
    });

    // Kind filter + sort direction.
    let mut set_kind: Option<Option<MediaKind>> = None;
    let mut set_sort: Option<SortKey> = None;
    let mut set_desc: Option<bool> = None;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Kind")
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
        let current = controller.ui().query.kind;
        if ui.selectable_label(current.is_none(), "All").clicked() {
            set_kind = Some(None);
        }
        if ui
            .selectable_label(current == Some(MediaKind::Audio), "Audio")
            .clicked()
        {
            set_kind = Some(Some(MediaKind::Audio));
        }
        if ui
            .selectable_label(current == Some(MediaKind::Video), "Video")
            .clicked()
        {
            set_kind = Some(Some(MediaKind::Video));
        }
        ui.add_space(Style::SP_M);
        let desc = controller.ui().query.descending;
        if ui.selectable_label(desc, "Descending").clicked() {
            set_desc = Some(!desc);
        }
    });
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Sort")
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
        for (key, label) in [
            (SortKey::Title, "Title"),
            (SortKey::Artist, "Artist"),
            (SortKey::Album, "Album"),
            (SortKey::Duration, "Duration"),
            (SortKey::DateAdded, "Added"),
        ] {
            if ui
                .selectable_label(controller.ui().query.sort == key, label)
                .clicked()
            {
                set_sort = Some(key);
            }
        }
    });
    if let Some(kind) = set_kind {
        controller.set_kind_filter(kind);
    }
    if let Some(sort) = set_sort {
        controller.set_sort(sort);
    }
    if let Some(desc) = set_desc {
        controller.set_descending(desc);
    }

    ui.add_space(Style::SP_S);
    ui.separator();
    ui.add_space(Style::SP_S);

    // The browse fold as rows. Clicking a title plays it; the queue button enqueues.
    let mut action: Option<TransportAction> = None;
    let items = controller.visible_items();
    if items.is_empty() {
        muted_note(
            ui,
            "No media matches — clear the search or index a folder in Sources.",
        );
    } else {
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for item in &items {
                    let (title, subtitle) = library_row_texts(item);
                    let path = item.path.clone();
                    ui.group(|ui| {
                        ui.set_min_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let kind_dot = match item.metadata.kind {
                                MediaKind::Audio => Style::ACCENT,
                                MediaKind::Video => Style::OK,
                            };
                            status_dot(ui, kind_dot);
                            ui.add_space(Style::SP_XS);
                            let clicked = ui
                                .label(
                                    RichText::new(&title)
                                        .size(Style::BODY)
                                        .strong()
                                        .color(Style::TEXT),
                                )
                                .interact(Sense::click())
                                .on_hover_cursor(CursorIcon::PointingHand)
                                .clicked();
                            if clicked {
                                action = Some(TransportAction::PlayPath(path.clone()));
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("Queue").clicked() {
                                    action = Some(TransportAction::Enqueue(
                                        path.clone(),
                                        Some(title.clone()),
                                    ));
                                }
                            });
                        });
                        if !subtitle.is_empty() {
                            muted_note(ui, subtitle);
                        }
                    });
                    ui.add_space(Style::SP_XS);
                }
            });
    }
    drop(items);
    if let Some(action) = action {
        // Playing from the library also switches to the Player view.
        let to_player = matches!(action, TransportAction::PlayPath(_));
        controller.dispatch(action);
        if to_player {
            controller.ui_mut().tab = MediaTab::Player;
        }
    }
}

// ── Player view ────────────────────────────────────────────────────────────────────

/// The Player view: the video stage (with the auto-hide OSD over it), the scrubber,
/// the transport row, the MEDIA-6 advanced controls, and the track menus.
#[allow(clippy::too_many_lines)]
fn player_view<E: MediaEngine>(ui: &mut egui::Ui, controller: &mut MediaController<E>) {
    // The stage + OSD. Clicking it toggles play/pause; the controls below override.
    let mut action: Option<TransportAction> = player_stage(ui, controller)
        .clicked()
        .then_some(TransportAction::TogglePlay);

    ui.add_space(Style::SP_S);

    // The scrubber (only meaningful with a known duration).
    let position = controller.player().position();
    let duration = controller.player().duration();
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format_time(position))
                .monospace()
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
        if let Some(dur) = duration {
            let mut seek = position.min(dur);
            let resp = ui.add(
                Slider::new(&mut seek, 0.0..=dur)
                    .show_value(false)
                    .trailing_fill(true),
            );
            if resp.changed() {
                action = Some(TransportAction::SeekTo(seek));
            }
            ui.label(
                RichText::new(format_time(dur))
                    .monospace()
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
        } else {
            muted_note(ui, "live / unknown length");
        }
    });

    ui.add_space(Style::SP_S);

    // The transport row.
    ui.horizontal(|ui| {
        if ui.button("⏮ Prev").clicked() {
            action = Some(TransportAction::Prev);
        }
        if ui.button("⏪ 10s").clicked() {
            action = Some(TransportAction::SeekBy(-SKIP_SECS));
        }
        if ui
            .button(play_pause_label(controller.player().state()))
            .clicked()
        {
            action = Some(TransportAction::TogglePlay);
        }
        if ui.button("10s ⏩").clicked() {
            action = Some(TransportAction::SeekBy(SKIP_SECS));
        }
        if ui.button("Next ⏭").clicked() {
            action = Some(TransportAction::Next);
        }
        if ui.button("Stop").clicked() {
            action = Some(TransportAction::Stop);
        }
    });

    ui.add_space(Style::SP_XS);

    // Frame-step + snapshot (design Q12/Q15).
    ui.horizontal(|ui| {
        if ui.button("◁ Frame").clicked() {
            action = Some(TransportAction::FrameBack);
        }
        if ui.button("Frame ▷").clicked() {
            action = Some(TransportAction::FrameForward);
        }
        if ui.button("Snapshot").clicked() {
            action = Some(TransportAction::Snapshot(ScreenshotMode::Subtitles));
        }
        // Chapter nav, only when the media is chaptered.
        if controller.player().chapter_count().is_some() {
            ui.add_space(Style::SP_M);
            if ui.button("Chapter ◀").clicked() {
                action = Some(TransportAction::ChapterPrev);
            }
            if ui.button("Chapter ▶").clicked() {
                action = Some(TransportAction::ChapterNext);
            }
        }
    });

    ui.add_space(Style::SP_XS);

    // Speed presets + the A-B loop (design Q12).
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Speed")
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
        let speed = controller.player().controls().speed;
        for preset in SPEED_PRESETS {
            let selected = (speed - preset).abs() < f64::EPSILON;
            if ui
                .selectable_label(selected, format!("{preset}×"))
                .clicked()
            {
                action = Some(TransportAction::SetSpeed(preset));
            }
        }
        ui.add_space(Style::SP_M);
        let ab_pending = controller.ui().ab_pending.is_some();
        let ab_label = if ab_pending { "A-B: set B" } else { "A-B loop" };
        if ui.button(ab_label).clicked() {
            action = Some(TransportAction::MarkAbLoop);
        }
        if ui.button("Clear A-B").clicked() {
            action = Some(TransportAction::ClearAbLoop);
        }
    });

    // Track menus (audio / subtitle), from the enumerated tracks (MEDIA-5).
    track_menus(ui, controller, &mut action);

    // Audio processing (MEDIA-3): the graphic EQ, loudness, ReplayGain, gapless.
    ui.add_space(Style::SP_XS);
    audio_controls(ui, controller, &mut action);

    if let Some(action) = action {
        controller.dispatch(action);
    }
}

/// The MEDIA-3 audio-processing controls, tucked in a collapsing "Audio & EQ" section
/// under the transport: a ten-band graphic EQ, loudness normalization, `ReplayGain`,
/// and gapless. Every change raises a [`TransportAction`] that folds the core
/// [`AudioConfig`](mde_media_core::AudioConfig) back to mpv's `af` graph + properties
/// (§6 — the surface reimplements no DSP). All chrome is Carbon [`Style`] tokens (§4).
fn audio_controls<E: MediaEngine>(
    ui: &mut egui::Ui,
    controller: &MediaController<E>,
    action: &mut Option<TransportAction>,
) {
    egui::CollapsingHeader::new(
        RichText::new("Audio & EQ")
            .size(Style::SMALL)
            .color(Style::TEXT_DIM),
    )
    .show(ui, |ui| audio_processing_body(ui, controller, action));
}

/// The body of the audio-processing section (factored out so the whole surface — the
/// EQ sliders + the mode rows — is tessellated by a headless mount test, §7, without a
/// pointer to expand the collapsing header).
fn audio_processing_body<E: MediaEngine>(
    ui: &mut egui::Ui,
    controller: &MediaController<E>,
    action: &mut Option<TransportAction>,
) {
    // The ten-band graphic EQ, one vertical slider per ISO octave centre (MEDIA-3).
    let gains = controller.eq_gains();
    ui.horizontal(|ui| {
        for (band, (&freq_hz, mut gain)) in EqBand::ISO_10_BAND_HZ.iter().zip(gains).enumerate() {
            ui.vertical(|ui| {
                let resp = ui.add(
                    Slider::new(&mut gain, -EQ_GAIN_DB_LIMIT..=EQ_GAIN_DB_LIMIT)
                        .vertical()
                        .show_value(false),
                );
                if resp.changed() {
                    *action = Some(TransportAction::SetEqGain(band, gain));
                }
                ui.label(
                    RichText::new(fmt_hz(freq_hz))
                        .monospace()
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
            });
        }
        ui.add_space(Style::SP_M);
        if ui.button("Flat").clicked() {
            *action = Some(TransportAction::ResetEq);
        }
    });

    ui.add_space(Style::SP_S);

    // Loudness normalization — Off / EBU R128 (loudnorm) / Dynamic (dynaudnorm).
    let loudness = controller.audio_config().loudness;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Loudness")
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
        if ui
            .selectable_label(loudness == LoudnessNorm::Off, "Off")
            .clicked()
        {
            *action = Some(TransportAction::SetLoudness(LoudnessNorm::Off));
        }
        if ui
            .selectable_label(matches!(loudness, LoudnessNorm::Ebu { .. }), "EBU R128")
            .clicked()
        {
            *action = Some(TransportAction::SetLoudness(EBU_R128_DEFAULT));
        }
        if ui
            .selectable_label(loudness == LoudnessNorm::Dynamic, "Dynamic")
            .clicked()
        {
            *action = Some(TransportAction::SetLoudness(LoudnessNorm::Dynamic));
        }
    });

    // ReplayGain — tag-based volume levelling: Off / Track / Album.
    let replaygain = controller.audio_config().replaygain;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("ReplayGain")
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
        for (mode, label) in [
            (ReplayGainMode::Off, "Off"),
            (ReplayGainMode::Track, "Track"),
            (ReplayGainMode::Album, "Album"),
        ] {
            if ui.selectable_label(replaygain == mode, label).clicked() {
                *action = Some(TransportAction::SetReplayGain(mode));
            }
        }
    });

    // Gapless across the queue (mpv's gapless-audio).
    let gapless = controller.audio_config().gapless;
    if ui.selectable_label(gapless, "Gapless").clicked() {
        *action = Some(TransportAction::ToggleGapless);
    }
}

/// A compact axis label for an EQ band centre frequency (`1000.0` → `"1k"`,
/// `16000.0` → `"16k"`, `250.0` → `"250"`).
fn fmt_hz(hz: f64) -> String {
    if hz >= 1000.0 {
        format!("{:.0}k", hz / 1000.0)
    } else {
        format!("{:.0}", hz.round())
    }
}

/// Draw the video stage — a dark rounded panel with the title centred — and, over it,
/// the translucent auto-hiding OSD (design Q34). Returns the stage's click response.
fn player_stage<E: MediaEngine>(ui: &mut egui::Ui, controller: &MediaController<E>) -> Response {
    let size = egui::vec2(ui.available_width(), STAGE_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let painter = ui.painter();
    painter.rect_filled(rect, Style::RADIUS, Style::BG);

    let title = now_playing_title(controller.player());
    let loaded = controller.player().media().is_some();
    let center_text = if loaded {
        title.clone()
    } else {
        "No media loaded — pick a title in Library".to_owned()
    };
    let center_color = if loaded { Style::TEXT } else { Style::TEXT_DIM };
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        center_text,
        FontId::proportional(Style::BODY),
        center_color,
    );

    // The OSD scrim + line, over the video, auto-hidden after the dwell.
    let paused = controller.player().state() != PlayerState::Playing;
    if loaded && osd_should_show(controller.ui().osd_idle_secs, paused) {
        let osd_h = Style::SP_XL;
        let osd_rect =
            egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - osd_h), rect.max);
        painter.rect_filled(osd_rect, Style::RADIUS, Style::BG.gamma_multiply(OSD_SCRIM));
        let position = controller.player().position();
        let osd = format!(
            "{}  ·  {}  ·  {}",
            state_word(controller.player().state()),
            title,
            format_time(position),
        );
        painter.text(
            egui::pos2(osd_rect.left() + Style::SP_S, osd_rect.center().y),
            Align2::LEFT_CENTER,
            osd,
            FontId::proportional(Style::SMALL),
            Style::TEXT,
        );
    }
    response
}

/// The audio + subtitle track menus, appending any selection to `action`.
fn track_menus<E: MediaEngine>(
    ui: &mut egui::Ui,
    controller: &MediaController<E>,
    action: &mut Option<TransportAction>,
) {
    let tracks = controller.tracks();
    if tracks.is_empty() {
        return;
    }
    ui.add_space(Style::SP_XS);
    for kind in [TrackKind::Audio, TrackKind::Subtitle] {
        let label = match kind {
            TrackKind::Audio => "Audio",
            TrackKind::Subtitle => "Subtitles",
            TrackKind::Video => "Video",
        };
        let matching: Vec<_> = tracks.iter().filter(|t| t.kind == kind).collect();
        if matching.is_empty() {
            continue;
        }
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(label)
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
            for track in matching {
                if ui.button(track_label(track)).clicked() {
                    *action = Some(TransportAction::SelectTrack(kind, track.id));
                }
            }
        });
    }
}

// ── Queue view ─────────────────────────────────────────────────────────────────────

/// The Queue view: the [`mde_media_core::Playlist`] rows (current highlighted) with
/// per-row play / reorder / remove, plus the repeat / shuffle / clear controls.
fn queue_view<E: MediaEngine>(ui: &mut egui::Ui, controller: &mut MediaController<E>) {
    section_title(ui, "Queue");

    let mut action: Option<TransportAction> = None;
    ui.horizontal(|ui| {
        if ui
            .button(repeat_label(controller.player().playlist().repeat()))
            .clicked()
        {
            action = Some(TransportAction::ToggleRepeat);
        }
        let shuffled = controller.player().playlist().is_shuffled();
        if ui
            .selectable_label(shuffled, if shuffled { "Shuffle ✓" } else { "Shuffle" })
            .clicked()
        {
            action = Some(TransportAction::ToggleShuffle);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.button("Clear").clicked() {
                action = Some(TransportAction::ClearQueue);
            }
        });
    });

    ui.add_space(Style::SP_S);
    ui.separator();
    ui.add_space(Style::SP_S);

    let items = controller.player().playlist().items().to_vec();
    let current = controller.player().playlist().current_index();
    let len = items.len();
    if items.is_empty() {
        muted_note(ui, "The queue is empty — add titles from Library.");
    } else {
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (index, item) in items.iter().enumerate() {
                    ui.group(|ui| {
                        ui.set_min_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let is_current = current == Some(index);
                            if is_current {
                                status_dot(ui, Style::ACCENT);
                            }
                            let color = if is_current {
                                Style::ACCENT
                            } else {
                                Style::TEXT
                            };
                            let clicked = ui
                                .label(
                                    RichText::new(format!("{}. {}", index + 1, item_title(item)))
                                        .size(Style::BODY)
                                        .color(color),
                                )
                                .interact(Sense::click())
                                .on_hover_cursor(CursorIcon::PointingHand)
                                .clicked();
                            if clicked {
                                action = Some(TransportAction::SelectQueueIndex(index));
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("✕").clicked() {
                                    action = Some(TransportAction::RemoveQueueIndex(index));
                                }
                                if index + 1 < len && ui.button("▼").clicked() {
                                    action = Some(TransportAction::MoveQueueItem(index, index + 1));
                                }
                                if index > 0 && ui.button("▲").clicked() {
                                    action = Some(TransportAction::MoveQueueItem(index, index - 1));
                                }
                            });
                        });
                    });
                    ui.add_space(Style::SP_XS);
                }
            });
    }

    if let Some(action) = action {
        controller.dispatch(action);
    }
}

// ── PiP mini-player ────────────────────────────────────────────────────────────────

/// The floating `PiP` mini-player (design Q31/Q32): a compact now-playing + play/pause
/// window shown when [`crate::model::UiState::pip`] is on. A real, reachable window —
/// not a stub.
pub fn pip_window<E: MediaEngine>(ctx: &Context, controller: &mut MediaController<E>) {
    if !controller.ui().pip {
        return;
    }
    let mut action: Option<TransportAction> = None;
    let mut close = false;
    egui::Window::new("Mini-player")
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            ui.label(
                RichText::new(now_playing_title(controller.player()))
                    .size(Style::BODY)
                    .color(Style::TEXT),
            );
            let pos = controller.player().position();
            let frac = progress_fraction(pos, controller.player().duration());
            ui.add(egui::ProgressBar::new(frac).desired_width(Style::SP_XL * 4.0));
            ui.horizontal(|ui| {
                if ui
                    .button(play_pause_label(controller.player().state()))
                    .clicked()
                {
                    action = Some(TransportAction::TogglePlay);
                }
                if ui.button("Next ⏭").clicked() {
                    action = Some(TransportAction::Next);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });
        });
    if let Some(action) = action {
        controller.dispatch(action);
    }
    if close {
        controller.ui_mut().pip = false;
    }
}

// ── shared chrome ──────────────────────────────────────────────────────────────────

/// A section title heading in the shared type scale.
fn section_title(ui: &mut egui::Ui, title: &str) {
    ui.add_space(Style::SP_XS);
    ui.label(
        RichText::new(title)
            .size(Style::BODY)
            .strong()
            .color(Style::TEXT),
    );
    ui.add_space(Style::SP_XS);
    ui.separator();
    ui.add_space(Style::SP_S);
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_media_core::{FakeMpv, MediaMetadata, Player, PlaylistItem, Track};

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

    fn controller() -> MediaController<FakeMpv> {
        MediaController::new(Player::new(
            FakeMpv::new().with_duration(120.0).with_tracks(tracks()),
        ))
    }

    /// Drive one headless egui frame that shows `body`, then tessellate on the CPU so
    /// any paint-path fault surfaces as a failure — the same `Context::run` →
    /// `tessellate` path the DRM runner drives, minus the GPU. Proves the views are
    /// runtime-reachable in `cargo test` (§7), with no window / wgpu / seat.
    fn render(
        controller: &mut MediaController<FakeMpv>,
        body: impl Fn(&mut egui::Ui, &mut MediaController<FakeMpv>),
    ) {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(900.0, 640.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::TopBottomPanel::top("t").show(ctx, |ui| media_header(ui, controller));
            egui::CentralPanel::default().show(ctx, |ui| body(ui, controller));
            pip_window(ctx, controller);
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty(), "frame produced no draw primitives");
    }

    fn populate(controller: &mut MediaController<FakeMpv>) {
        controller.library_mut().add_root("/m");
        controller.library_mut().upsert(
            "/m/alpha.flac",
            MediaMetadata::from_path("/m/alpha.flac")
                .expect("audio")
                .with_artist("Aurora")
                .with_duration(180.0),
        );
        controller.library_mut().upsert(
            "/m/clip.mkv",
            MediaMetadata::from_path("/m/clip.mkv").expect("video"),
        );
    }

    #[test]
    fn sources_view_renders_empty_and_populated() {
        let mut c = controller();
        render(&mut c, sources_view); // honest empty first-run (capture un-probed hint)
        populate(&mut c);
        // The MEDIA-12 Open-URL field renders with text in it (never spawns yt-dlp
        // without an explicit Open click, which headless rendering cannot deliver).
        c.ui_mut().url_input = "https://youtu.be/dQw4w9WgXcQ".to_owned();
        render(&mut c, sources_view);
        assert_eq!(c.player().state(), PlayerState::Idle, "render never opens");
    }

    /// A headless v4l2 enumerator (a recorded listing) so the populated capture
    /// section renders with no real `/dev/video` and no `v4l2-ctl` subprocess.
    struct FakeCapture(String);
    impl mde_media_core::CaptureEnumerator for FakeCapture {
        fn is_available(&self) -> bool {
            true
        }
        fn enumerate(
            &self,
        ) -> Result<Vec<mde_media_core::CaptureDevice>, mde_media_core::CaptureError> {
            Ok(mde_media_core::parse_v4l2_listing(&self.0))
        }
    }

    #[test]
    fn sources_view_renders_capture_devices_and_no_device_state() {
        let mut c = controller();
        // Probed with devices → the Watch rows tessellate (MEDIA-13).
        c.refresh_capture_devices(&FakeCapture(
            "UVC Camera (usb-0000:00:14.0-1):\n\t/dev/video0\n\t/dev/media0\n".to_owned(),
        ));
        render(&mut c, sources_view);
        assert_eq!(c.capture().playable().len(), 1);
        // Probed with no hardware → the honest "no capture devices found" note renders.
        c.refresh_capture_devices(&FakeCapture(String::new()));
        render(&mut c, sources_view);
        assert!(c.capture().playable().is_empty());
        assert_eq!(c.player().state(), PlayerState::Idle, "render never opens");
    }

    #[test]
    fn library_view_renders_across_query_states() {
        let mut c = controller();
        populate(&mut c);
        render(&mut c, library_view);
        // Narrow the browse fold, filter by kind, sort descending — all render.
        c.set_search("aurora");
        render(&mut c, library_view);
        c.set_search("");
        c.set_kind_filter(Some(MediaKind::Video));
        c.set_sort(SortKey::Duration);
        c.set_descending(true);
        render(&mut c, library_view);
        // Empty match state.
        c.set_kind_filter(None);
        c.set_search("nothing-matches-this");
        render(&mut c, library_view);
    }

    #[test]
    fn player_view_renders_idle_and_playing_with_osd() {
        let mut c = controller();
        // Idle: the stage shows the "no media" prompt.
        render(&mut c, player_view);
        // Load + pump → Playing; the scrubber, transport, speed, A-B, tracks all draw.
        c.dispatch(TransportAction::PlayPath("clip.mkv".to_owned()));
        c.pump();
        c.ui_mut().osd_idle_secs = 0.0; // OSD visible
        render(&mut c, player_view);
        // Idle-hidden OSD branch.
        c.ui_mut().osd_idle_secs = crate::model::OSD_HIDE_SECS + 1.0;
        render(&mut c, player_view);
    }

    #[test]
    fn audio_processing_surface_renders_and_reflects_the_config() {
        let mut c = controller();
        // Flat default: the EQ sliders + mode rows tessellate (§7 — the whole audio
        // surface is proven reachable, not just the collapsing header).
        render(&mut c, |ui, c| {
            let mut action = None;
            audio_processing_body(ui, c, &mut action);
        });
        // With a shaped EQ + loudness + ReplayGain + gapless-off, the surface reflects
        // the live core AudioConfig (the sliders seed from it, the modes light up).
        c.dispatch(TransportAction::SetEqGain(9, 6.0));
        c.dispatch(TransportAction::SetLoudness(EBU_R128_DEFAULT));
        c.dispatch(TransportAction::SetReplayGain(ReplayGainMode::Album));
        c.dispatch(TransportAction::ToggleGapless);
        assert!((c.eq_gains()[9] - 6.0).abs() < f64::EPSILON);
        assert!(!c.audio_config().gapless);
        render(&mut c, |ui, c| {
            let mut action = None;
            audio_processing_body(ui, c, &mut action);
        });
        // The collapsing wrapper itself renders inside the full player view.
        render(&mut c, player_view);
        assert_eq!(c.player().state(), PlayerState::Idle, "render never plays");
    }

    #[test]
    fn queue_view_renders_empty_and_with_items() {
        let mut c = controller();
        render(&mut c, queue_view);
        c.player_mut()
            .playlist_mut()
            .push(PlaylistItem::titled("a", "Alpha"));
        c.player_mut().playlist_mut().push(PlaylistItem::new("b"));
        c.player_mut().playlist_mut().push(PlaylistItem::new("c"));
        render(&mut c, queue_view);
    }

    #[test]
    fn pip_and_fullscreen_states_render() {
        let mut c = controller();
        c.dispatch(TransportAction::PlayPath("clip.mkv".to_owned()));
        c.pump();
        c.ui_mut().pip = true;
        c.ui_mut().fullscreen = true;
        // The PiP window + header (with the toggles lit) render.
        render(&mut c, player_view);
    }

    #[test]
    fn tabs_switch_via_the_header() {
        // The header writes the chosen tab back onto the controller — proving the tab
        // bar is live wiring, not decoration. We can't click headlessly, so drive the
        // state the header binds to and confirm a full frame renders on each tab.
        let mut c = controller();
        populate(&mut c);
        for tab in MediaTab::all() {
            c.ui_mut().tab = tab;
            render(&mut c, media_panel);
            assert_eq!(c.ui().tab, tab);
        }
    }

    /// A fixture transport serving one direct-playable Jellyfin movie — no network.
    struct JellyStub;
    impl mde_jellyfin::HttpTransport for JellyStub {
        fn execute(
            &self,
            _request: &mde_jellyfin::HttpRequest,
        ) -> Result<mde_jellyfin::HttpResponse, mde_jellyfin::TransportError> {
            let body = r#"{"Items":[{"Id":"m1","Name":"Movie One","Type":"Movie",
                "MediaSources":[{"Id":"s1","Container":"mkv","MediaStreams":[
                {"Type":"Video","Codec":"h264","Index":0},
                {"Type":"Audio","Codec":"aac","Index":1}]}]}],
                "TotalRecordCount":1,"StartIndex":0}"#;
            Ok(mde_jellyfin::HttpResponse {
                status: 200,
                body: body.as_bytes().to_vec(),
            })
        }
    }

    #[test]
    fn sources_view_renders_jellyfin_servers_and_titles() {
        let mut c = controller();
        c.add_jellyfin_server("srv", "Home", "https://jelly.mesh:8096");
        c.select_jellyfin_server("srv");
        // A configured server (no titles yet) renders the server row + add fields.
        render(&mut c, sources_view);
        // Browse a title through a stub client, then the playable row tessellates.
        let device = mde_jellyfin::ClientInfo::new("mde-media", "test", "dev", "12.0.0");
        let client =
            mde_jellyfin::JellyfinClient::new("https://jelly.mesh:8096", device, JellyStub)
                .with_auth("T", "u");
        c.browse_jellyfin(&client, &mde_jellyfin::ItemsQuery::default().recursive())
            .expect("browse");
        assert_eq!(c.jellyfin_items().len(), 1);
        render(&mut c, sources_view);
    }

    /// A fixture transport serving synthetic media bytes for a download and one
    /// Jellyfin movie for a browse — the offline-cache seam, no network.
    struct JellyDownloadStub;
    impl mde_jellyfin::HttpTransport for JellyDownloadStub {
        fn execute(
            &self,
            request: &mde_jellyfin::HttpRequest,
        ) -> Result<mde_jellyfin::HttpResponse, mde_jellyfin::TransportError> {
            let body: Vec<u8> = if request.url.contains("/stream") {
                b"SYNTHETIC-OFFLINE-MEDIA".to_vec()
            } else {
                br#"{"Items":[{"Id":"m1","Name":"Movie One","Type":"Movie",
                    "MediaSources":[{"Id":"s1","Container":"mkv","MediaStreams":[
                    {"Type":"Video","Codec":"h264","Index":0},
                    {"Type":"Audio","Codec":"aac","Index":1}]}]}],
                    "TotalRecordCount":1,"StartIndex":0}"#
                    .to_vec()
            };
            Ok(mde_jellyfin::HttpResponse { status: 200, body })
        }
    }

    fn profile(user_id: &str, name: &str, token: &str) -> mde_jellyfin::ServerAuth {
        mde_jellyfin::ServerAuth {
            access_token: token.into(),
            user_id: user_id.into(),
            user_name: Some(name.into()),
            server_id: None,
        }
    }

    #[test]
    fn sources_view_renders_profile_switcher_and_offline_downloads() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut c = controller();
        c.add_jellyfin_server("srv", "Home", "https://jelly.mesh:8096");
        c.select_jellyfin_server("srv");
        c.set_jellyfin_offline_root(dir.path());
        // Two user profiles → the per-server profile switcher renders (MEDIA-11).
        c.add_jellyfin_profile("srv", profile("user-a", "matthew", "A"));
        c.add_jellyfin_profile("srv", profile("user-b", "guest", "B"));

        // Browse a title, then download it → the offline badge + downloaded list draw.
        let device = mde_jellyfin::ClientInfo::new("mde-media", "test", "dev", "12.0.0");
        let client =
            mde_jellyfin::JellyfinClient::new("https://jelly.mesh:8096", device, JellyDownloadStub)
                .with_auth("A", "user-a");
        c.browse_jellyfin(&client, &mde_jellyfin::ItemsQuery::default().recursive())
            .expect("browse");
        let item = c.jellyfin_items()[0].clone();
        c.download_jellyfin_item(&client, &item, 1)
            .expect("download");
        assert!(c.is_offline_available("m1"));
        render(&mut c, sources_view);
    }
}
