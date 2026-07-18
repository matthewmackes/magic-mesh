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
use mde_egui::{muted_note, status_dot, Motion, Style};

use mde_jellyfin::{
    BaseItemDto, ClientInfo, ItemsQuery, JellyfinClient, ReqwestTransport, ServerConfig,
};
use mde_media_core::{
    EqBand, LibraryItem, LoudnessNorm, MediaEngine, MediaKind, PlayerState, ReplayGainMode,
    ScreenshotMode, SortKey, TrackKind, V4l2Cli, YtDlpCli,
};

use crate::model::{
    capture_detail, format_time, item_title, jellyfin_item_title, library_row_texts,
    now_playing_title, osd_should_show, play_pause_label, progress_fraction, repeat_label,
    state_word, track_label, MediaController, MediaTab, TransportAction, EBU_R128_DEFAULT,
    EQ_GAIN_DB_LIMIT,
};
use crate::Engine;

/// The alpha/darken factor applied to [`Style::BG`] for the translucent dark media OSD
/// scrim drawn over the video (design Q34). Derived from the palette token — the same
/// translucency-by-factor idiom [`Style`] itself uses for its selection fill — so no
/// raw colour is introduced (§4).
const OSD_SCRIM: f32 = 0.72;

/// The height of the video stage, on the 8px grid (a token multiple, not a magic px).
const STAGE_HEIGHT: f32 = Style::SP_XL * 6.0;

/// The minimum width of one Library grid card: wide enough for title + Play/Queue
/// controls, still allowing a useful multi-column browse on a laptop panel.
const LIBRARY_CARD_MIN_W: f32 = Style::SP_XL * 6.0;

/// The browse grid gap, held on the 8px grid so card columns line up with the rest of
/// the Carbon media chrome.
const LIBRARY_GRID_GAP: f32 = Style::SP_S;

/// The playback-speed presets the Player view offers.
const SPEED_PRESETS: [f64; 5] = [0.5, 1.0, 1.25, 1.5, 2.0];

/// The seek step (seconds) of the skip-back / skip-forward transport buttons.
const SKIP_SECS: f64 = 10.0;

/// How many frames between playback-roaming convergence polls (MEDIA-16). The poll
/// reads the shared session plane, so it runs on a coarse cadence (~1 s at 60 fps),
/// not every frame — the same human-paced convergence the mesh workers use.
const ROAM_POLL_INTERVAL_FRAMES: u32 = 60;

/// Compact icon-only queue action button: a 24px click target matching the sibling
/// YAMIS button rows, with a smaller painted mark centered inside.
const QUEUE_CONTROL_BUTTON: f32 = Style::SP_L;
const QUEUE_CONTROL_ICON: f32 = Style::SP_M - Style::SP_XS;

/// The media surface: the controller plus the applied-fullscreen mirror so the app
/// only issues a viewport command when the immersive state actually flips.
pub struct MediaApp {
    controller: MediaController<Engine>,
    applied_fullscreen: bool,
    /// Frames since start, gating the MEDIA-16 roaming poll to a coarse cadence.
    roam_poll_frames: u32,
    /// The MEDIA-2 phase-1 frame-sink texture (`docs/gpu_encoder.md`) the Player
    /// tab's stage paints — owned here so it persists across frames instead of
    /// re-uploading a GPU texture every call.
    video: VideoTextureCache,
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
        // The one production construction path — the same `real_media()` seam the E12
        // shell holds directly (MEDIA-18). It wires the default engine + enables
        // roaming playback (MEDIA-16: a silent no-op with no mesh workgroup root).
        Self {
            controller: crate::real_media(),
            applied_fullscreen: false,
            roam_poll_frames: 0,
            video: VideoTextureCache::default(),
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
            // MEDIA-17: on the same cadence, converge the party plane — apply any
            // play/pause/seek another seat issued so a shared session stays in sync.
            self.controller.poll_party();
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
        egui::CentralPanel::default().show(ctx, |ui| {
            media_panel(ui, &mut self.controller, &mut self.video);
        });

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

/// The header strip: the shared MENUBAR-ALL top bar over the sub-view tab bar.
///
/// The bar (MENUBAR-ALL-media) renders the UPPERCASE `MEDIA` title, the
/// File/View/Playback/Audio/Subtitles/Cast menus, and the live now-playing / time /
/// output status cluster; it surfaces **all** of the surface's controls incl. the
/// advanced transport (the governing principle), every item a real seam (§7). The
/// fullscreen / mini-player toggles now live in its View menu (checkmarked) and the
/// now-playing read-out in its status cluster, so the header carries only the primary
/// Sources/Library/Player/Queue nav below the bar.
pub fn media_header<E: MediaEngine>(ui: &mut egui::Ui, controller: &mut MediaController<E>) {
    // The shared top bar (MENUBAR-ALL-1): title · menus · status cluster.
    crate::menubar::menu_bar(ui, controller);
    ui.add_space(Style::SP_XS);

    // The primary in-surface nav — also reachable from the bar's View menu.
    let mut chosen_tab = controller.ui().tab;
    ui.horizontal(|ui| {
        ui.add_space(Style::SP_S);
        for tab in MediaTab::all() {
            if ui
                .selectable_label(controller.ui().tab == tab, tab.label())
                .clicked()
            {
                chosen_tab = tab;
            }
        }
    });
    ui.add_space(Style::SP_XS);
    controller.ui_mut().tab = chosen_tab;
}

// ── central panel router ───────────────────────────────────────────────────────────

/// Render the active view's body into `ui`, then the transient status line.
///
/// `video` is the MEDIA-2 phase-1 frame-sink texture cache (`docs/gpu_encoder.md`)
/// the Player tab's stage paints through — owned by the caller (the standalone
/// [`MediaApp`], or the E12 shell for the embedded surface) so it persists across
/// frames instead of re-uploading a GPU texture every call.
pub fn media_panel<E: MediaEngine>(
    ui: &mut egui::Ui,
    controller: &mut MediaController<E>,
    video: &mut VideoTextureCache,
) {
    match controller.ui().tab {
        MediaTab::Sources => sources_view(ui, controller),
        MediaTab::Library => library_view(ui, controller),
        MediaTab::Player => player_view(ui, controller, video),
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
/// the [`MediaController::visible_items`] fold (MEDIA-7) as a compact card grid.
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

    // The browse fold as Carbon grid cards. Clicking a card plays it; the queue
    // button enqueues. The actions remain the same TransportAction path as the old
    // rows, so the view owns no playback or queue behavior.
    let mut action: Option<TransportAction> = None;
    let cards: Vec<LibraryCard> = controller
        .visible_items()
        .into_iter()
        .map(LibraryCard::from_item)
        .collect();
    if cards.is_empty() {
        muted_note(
            ui,
            "No media matches — clear the search or index a folder in Sources.",
        );
    } else {
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let width = ui.available_width();
                let columns = library_grid_columns(width);
                let card_w = library_grid_card_width(width, columns);
                egui::Grid::new("media-library-grid")
                    .num_columns(columns)
                    .spacing(egui::vec2(LIBRARY_GRID_GAP, Style::SP_M))
                    .show(ui, |ui| {
                        for (index, card) in cards.iter().enumerate() {
                            if let Some(next) = library_card(ui, card, card_w) {
                                action = Some(next);
                            }
                            if (index + 1) % columns == 0 {
                                ui.end_row();
                            }
                        }
                    });
            });
    }
    if let Some(action) = action {
        // Playing from the library also switches to the Player view.
        let to_player = matches!(action, TransportAction::PlayPath(_));
        controller.dispatch(action);
        if to_player {
            controller.ui_mut().tab = MediaTab::Player;
        }
    }
}

/// The display data one Library card needs. Built before rendering so egui can draw
/// the grid without holding borrowed library rows across potential dispatch.
struct LibraryCard {
    path: String,
    title: String,
    subtitle: String,
    kind: MediaKind,
}

impl LibraryCard {
    fn from_item(item: &LibraryItem) -> Self {
        let (title, subtitle) = library_row_texts(item);
        Self {
            path: item.path.clone(),
            title,
            subtitle,
            kind: item.metadata.kind,
        }
    }
}

/// Number of grid columns that fit a given browse width while preserving the minimum
/// playable card width. Always returns at least one column, even during first-frame
/// zero-width layout probes.
#[must_use]
fn library_grid_columns(width: f32) -> usize {
    if width <= LIBRARY_CARD_MIN_W {
        return 1;
    }
    ((width + LIBRARY_GRID_GAP) / (LIBRARY_CARD_MIN_W + LIBRARY_GRID_GAP))
        .floor()
        .max(1.0) as usize
}

/// The exact card width for `columns`, including equal-width expansion when the grid
/// has extra space.
#[must_use]
fn library_grid_card_width(width: f32, columns: usize) -> f32 {
    let columns = columns.max(1);
    let gaps = LIBRARY_GRID_GAP * (columns.saturating_sub(1) as f32);
    ((width - gaps) / columns as f32).max(LIBRARY_CARD_MIN_W)
}

/// One Netflix-style Library card: a 16:9 media plate, title/subtitle, and the same
/// Play/Queue actions the old list rows used.
fn library_card(ui: &mut egui::Ui, card: &LibraryCard, width: f32) -> Option<TransportAction> {
    let band = ui.painter().add(egui::Shape::Noop);
    let mut action = None;
    let content_w = (width - Style::SP_M).max(Style::SP_XL * 4.0);
    let group = library_card_frame().show(ui, |ui| {
        ui.set_min_width(content_w);
        ui.set_max_width(content_w);

        paint_library_art(ui, card.kind, content_w);

        ui.add_space(Style::SP_S);
        ui.add(
            egui::Label::new(
                RichText::new(&card.title)
                    .size(Style::BODY)
                    .strong()
                    .color(Style::TEXT),
            )
            .wrap(),
        );
        if !card.subtitle.is_empty() {
            ui.add(
                egui::Label::new(
                    RichText::new(&card.subtitle)
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                )
                .wrap(),
            );
        }

        ui.add_space(Style::SP_S);
        ui.horizontal(|ui| {
            if ui
                .add_sized(
                    egui::vec2(content_w * 0.52, Style::SP_L),
                    egui::Button::new(
                        RichText::new("▶  Play")
                            .size(Style::SMALL)
                            .color(Style::TEXT_STRONG),
                    )
                    .fill(Style::ACCENT),
                )
                .on_hover_text("Play")
                .on_hover_cursor(CursorIcon::PointingHand)
                .clicked()
            {
                action = Some(card_play_action(card));
            }
            if ui
                .add_sized(
                    egui::vec2(content_w * 0.38, Style::SP_L),
                    egui::Button::new(
                        RichText::new("+  Queue")
                            .size(Style::SMALL)
                            .color(Style::TEXT),
                    ),
                )
                .on_hover_text("Add to queue")
                .on_hover_cursor(CursorIcon::PointingHand)
                .clicked()
            {
                action = Some(card_queue_action(card));
            }
        });
    });
    let response = group
        .response
        .interact(Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    let hover = Motion::animate(
        ui.ctx(),
        ("library-card-hover", card.path.as_str()),
        response.hovered(),
        Motion::FAST,
    );
    if hover > 0.0 {
        ui.painter().set(
            band,
            egui::Shape::rect_filled(
                response.rect,
                Style::RADIUS,
                Style::SURFACE_HI.gamma_multiply(hover),
            ),
        );
    }
    if response.clicked() && action.is_none() {
        action = Some(card_play_action(card));
    }
    action
}

fn library_card_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(Style::LAYER_02)
        .stroke(egui::Stroke::new(1.0, Style::BORDER))
        .corner_radius(Style::RADIUS)
        .inner_margin(Style::SP_S)
}

fn paint_library_art(ui: &mut egui::Ui, kind: MediaKind, width: f32) {
    let height = (width * 9.0 / 16.0).clamp(Style::SP_XL * 2.0, Style::SP_XL * 3.5);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, Style::RADIUS, Style::LAYER_01);
    painter.rect_stroke(
        rect,
        Style::RADIUS,
        egui::Stroke::new(1.0, Style::BORDER),
        egui::StrokeKind::Inside,
    );

    let accent = library_kind_color(kind);
    let strip = egui::Rect::from_min_max(
        rect.left_top(),
        egui::pos2(rect.left() + Style::SP_XS, rect.bottom()),
    );
    painter.rect_filled(strip, Style::RADIUS_S, accent);

    let icon_rect =
        egui::Rect::from_center_size(rect.center(), egui::vec2(Style::SP_L, Style::SP_L));
    match kind {
        MediaKind::Audio => paint_audio_glyph(painter, icon_rect, accent),
        MediaKind::Video => paint_video_glyph(painter, icon_rect, accent),
    }

    let label_rect = egui::Rect::from_min_size(
        rect.left_top() + egui::vec2(Style::SP_S, Style::SP_S),
        egui::vec2(Style::SP_XL * 2.0, Style::SP_L),
    );
    painter.rect_filled(label_rect, Style::RADIUS_S, Style::BG.gamma_multiply(0.86));
    painter.text(
        label_rect.center(),
        Align2::CENTER_CENTER,
        library_kind_label(kind),
        FontId::proportional(Style::SMALL),
        Style::TEXT_STRONG,
    );
}

fn paint_audio_glyph(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let stroke = egui::Stroke::new(2.0, color);
    let stem_x = rect.center().x + Style::SP_XS;
    painter.line_segment(
        [
            egui::pos2(stem_x, rect.top()),
            egui::pos2(stem_x, rect.bottom() - Style::SP_XS),
        ],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(stem_x, rect.top()),
            egui::pos2(rect.right(), rect.top() + Style::SP_XS),
        ],
        stroke,
    );
    painter.circle_stroke(
        egui::pos2(rect.left() + Style::SP_S, rect.bottom() - Style::SP_XS),
        Style::SP_S,
        stroke,
    );
}

fn paint_video_glyph(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let stroke = egui::Stroke::new(2.0, color);
    let body = rect.shrink(Style::SP_XS);
    painter.rect_stroke(body, Style::RADIUS_S, stroke, egui::StrokeKind::Inside);
    let lens = [
        body.left_center(),
        egui::pos2(body.right() - Style::SP_XS, body.top() + Style::SP_S),
        egui::pos2(body.right() - Style::SP_XS, body.bottom() - Style::SP_S),
    ];
    painter.add(egui::Shape::convex_polygon(
        lens.to_vec(),
        color.gamma_multiply(0.36),
        stroke,
    ));
}

const fn library_kind_color(kind: MediaKind) -> egui::Color32 {
    match kind {
        MediaKind::Audio => Style::ACCENT_MEDIA,
        MediaKind::Video => Style::OK,
    }
}

const fn library_kind_label(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Audio => "Audio",
        MediaKind::Video => "Video",
    }
}

fn card_play_action(card: &LibraryCard) -> TransportAction {
    TransportAction::PlayPath(card.path.clone())
}

fn card_queue_action(card: &LibraryCard) -> TransportAction {
    TransportAction::Enqueue(card.path.clone(), Some(card.title.clone()))
}

// ── Player view ────────────────────────────────────────────────────────────────────

/// The Player view: the video stage (with the auto-hide OSD over it), the scrubber,
/// the transport row, the MEDIA-6 advanced controls, and the track menus.
#[allow(clippy::too_many_lines)]
fn player_view<E: MediaEngine>(
    ui: &mut egui::Ui,
    controller: &mut MediaController<E>,
    video: &mut VideoTextureCache,
) {
    // The stage + OSD. Clicking it toggles play/pause; the controls below override.
    let mut action: Option<TransportAction> = player_stage(ui, controller, video)
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

    // The transport row — the primary play/pause is the one accent-filled action; the
    // rest are icon buttons that name themselves on hover (CRAFT §6).
    ui.horizontal(|ui| {
        if transport_button(ui, "⏮", "Previous track").clicked() {
            action = Some(TransportAction::Prev);
        }
        if transport_button(ui, "⏪ 10s", "Back 10 seconds").clicked() {
            action = Some(TransportAction::SeekBy(-SKIP_SECS));
        }
        let state = controller.player().state();
        let glyph = if state == PlayerState::Playing {
            "⏸"
        } else {
            "▶"
        };
        if primary_transport_button(
            ui,
            &format!("{glyph}  {}", play_pause_label(state)),
            "Play / pause",
        )
        .clicked()
        {
            action = Some(TransportAction::TogglePlay);
        }
        if transport_button(ui, "10s ⏩", "Forward 10 seconds").clicked() {
            action = Some(TransportAction::SeekBy(SKIP_SECS));
        }
        if transport_button(ui, "⏭", "Next track").clicked() {
            action = Some(TransportAction::Next);
        }
        if transport_button(ui, "Stop", "Stop playback").clicked() {
            action = Some(TransportAction::Stop);
        }
    });

    ui.add_space(Style::SP_XS);

    // Frame-step + snapshot (design Q12/Q15).
    ui.horizontal(|ui| {
        if transport_button(ui, "◁ Frame", "Step back one frame").clicked() {
            action = Some(TransportAction::FrameBack);
        }
        if transport_button(ui, "Frame ▷", "Step forward one frame").clicked() {
            action = Some(TransportAction::FrameForward);
        }
        if transport_button(ui, "Snapshot", "Save a frame snapshot").clicked() {
            action = Some(TransportAction::Snapshot(ScreenshotMode::Subtitles));
        }
        // Chapter nav, only when the media is chaptered.
        if controller.player().chapter_count().is_some() {
            ui.add_space(Style::SP_M);
            if transport_button(ui, "Chapter ◀", "Previous chapter").clicked() {
                action = Some(TransportAction::ChapterPrev);
            }
            if transport_button(ui, "Chapter ▶", "Next chapter").clicked() {
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

    // Watch-together party + cast (MEDIA-17). Rendered after the transport dispatch so a
    // just-broadcast control is reflected; it drives the controller directly.
    ui.add_space(Style::SP_XS);
    party_cast_controls(ui, controller);
}

/// A click intent from the [`party_cast_controls`] section — collected while rendering
/// (immutable controller reads), applied after, to keep egui's borrows clean.
enum PartyCastIntent {
    /// Host / join the named party.
    JoinParty(String),
    /// Leave the joined party.
    LeaveParty,
    /// Probe the network for cast renderers.
    DiscoverCast,
    /// Cast the current playback to the target with this id.
    Cast(String),
}

/// The MEDIA-17 "Party & Cast" section: host/join a watch-together party (play/pause/seek
/// propagate to every joined seat) and throw the current playback at a discovered
/// renderer. All chrome is Carbon [`Style`] tokens (§4); the cast list honest-gates on an
/// empty probe ("no renderer found"), never a fabricated device (§7).
fn party_cast_controls<E: MediaEngine>(ui: &mut egui::Ui, controller: &mut MediaController<E>) {
    let mut intent: Option<PartyCastIntent> = None;
    egui::CollapsingHeader::new(
        RichText::new("Party & Cast")
            .size(Style::BODY)
            .color(Style::TEXT),
    )
    .id_salt("media-party-cast")
    .show(ui, |ui| {
        // ── Watch-together party ──
        ui.label(
            RichText::new("Watch together")
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
        if let Some(name) = controller.party_name() {
            let members = controller.party_members();
            ui.horizontal(|ui| {
                status_dot(ui, Style::OK);
                ui.label(
                    RichText::new(format!("In \"{name}\" · {} seat(s)", members.len().max(1)))
                        .size(Style::BODY)
                        .color(Style::TEXT),
                );
                if ui.button("Leave").clicked() {
                    intent = Some(PartyCastIntent::LeaveParty);
                }
            });
            if !members.is_empty() {
                muted_note(ui, members.join(", "));
            }
        } else {
            let mut party = controller.ui().party_input.clone();
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut party)
                        .hint_text("party name")
                        .desired_width(Style::SP_XL * 5.0),
                );
                let trimmed = party.trim().to_owned();
                if ui
                    .add_enabled(!trimmed.is_empty(), egui::Button::new("Host / Join"))
                    .clicked()
                {
                    intent = Some(PartyCastIntent::JoinParty(trimmed));
                }
            });
            controller.ui_mut().party_input = party;
            muted_note(
                ui,
                "Several seats join one session; play/pause/seek stay in sync.",
            );
        }

        ui.add_space(Style::SP_S);
        ui.separator();

        // ── Cast ──
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Cast to")
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
            if ui.button("Find renderers").clicked() {
                intent = Some(PartyCastIntent::DiscoverCast);
            }
        });
        let targets = controller.cast().targets();
        if targets.is_empty() {
            // The honest gate: nothing found (or not yet probed).
            let note = if controller.cast().probed() {
                "No cast renderer found on this network."
            } else {
                "No renderers discovered yet — Find renderers to look."
            };
            muted_note(ui, note);
        } else {
            for target in targets {
                ui.horizontal(|ui| {
                    status_dot(ui, Style::ACCENT);
                    ui.label(
                        RichText::new(&target.name)
                            .size(Style::BODY)
                            .color(Style::TEXT),
                    );
                    muted_note(ui, target.kind.label());
                    if ui.button("Cast").clicked() {
                        intent = Some(PartyCastIntent::Cast(target.id.clone()));
                    }
                });
            }
        }
    });

    match intent {
        Some(PartyCastIntent::JoinParty(name)) => {
            controller.join_party(name);
        }
        Some(PartyCastIntent::LeaveParty) => controller.leave_party(),
        Some(PartyCastIntent::DiscoverCast) => controller.discover_cast_targets(),
        Some(PartyCastIntent::Cast(id)) => controller.cast_current(&id),
        None => {}
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

/// The MEDIA-2 phase-1 frame sink (`docs/gpu_encoder.md` "Render API to egui
/// texture first"): the video stage's texture.
///
/// Uploaded from [`MediaEngine::latest_frame`]. Mirrors the shell's
/// `VdiState.texture` pattern (`mde-shell-egui/src/vdi.rs` — allocate on the
/// first frame, `TextureHandle::set` in place after) so the same "decode →
/// `ColorImage` → `TextureHandle` → paint" shape is used everywhere an
/// external/engine frame source lands in the shell.
///
/// Owned by whoever holds the [`MediaController`] — the standalone
/// [`MediaApp`] here, the E12 shell for the embedded surface (MEDIA-18) — and
/// threaded into [`media_panel`]/[`player_view`]/`player_stage`. Kept out of
/// the render-agnostic [`crate::model`] on purpose: that module "touches no
/// egui" so the core state stays GPU-free and unit-testable with no context.
#[derive(Default)]
pub struct VideoTextureCache {
    texture: Option<egui::TextureHandle>,
    /// The checksum of the uploaded frame, so a throttled capture that hasn't
    /// changed yet (`MpvEngine::latest_frame`'s ~150 ms cadence) skips a
    /// redundant GPU upload.
    last_checksum: Option<u64>,
}

/// Linear filtering for the video texture — the same choice
/// `mde-shell-egui/src/vdi.rs`'s `DESKTOP_TEX` makes for its decoded desktop
/// framebuffer.
const VIDEO_TEX: egui::TextureOptions = egui::TextureOptions::LINEAR;

impl VideoTextureCache {
    /// Upload `frame` to the GPU if its content differs from what is already
    /// there; a no-op on an unchanged repeat capture.
    fn upload(&mut self, ctx: &Context, frame: &mde_media_core::VideoFrame) {
        let checksum = frame.checksum();
        if self.last_checksum == Some(checksum) {
            return;
        }
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [frame.width as usize, frame.height as usize],
            &frame.rgba,
        );
        match self.texture.as_mut() {
            Some(handle) => handle.set(image, VIDEO_TEX),
            None => self.texture = Some(ctx.load_texture("mde-media-video", image, VIDEO_TEX)),
        }
        self.last_checksum = Some(checksum);
    }

    /// The uploaded texture, if any frame has landed yet.
    const fn texture(&self) -> Option<&egui::TextureHandle> {
        self.texture.as_ref()
    }

    /// Drop the cached texture (media unloaded/stopped) so the stage falls
    /// back to the placeholder instead of freezing on the last frame shown.
    fn clear(&mut self) {
        self.texture = None;
        self.last_checksum = None;
    }
}

/// Draw the video stage — the real decoded picture once the real mpv engine
/// (`--features mpv`) produces one, else the dark rounded placeholder panel
/// with the title centred — and, over it, the translucent auto-hiding OSD
/// (design Q34). Returns the stage's click response.
fn player_stage<E: MediaEngine>(
    ui: &mut egui::Ui,
    controller: &mut MediaController<E>,
    video: &mut VideoTextureCache,
) -> Response {
    let size = egui::vec2(ui.available_width(), STAGE_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());

    let title = now_playing_title(controller.player());
    let loaded = controller.player().media().is_some();

    // MEDIA-2 phase 1 (docs/gpu_encoder.md): pull the newest decoded frame off
    // the engine and upload it to the stage texture. `FakeMpv` (the
    // airgap-safe default) never produces one, so the placeholder paints
    // exactly as before; the real mpv engine (`--features mpv`) does, closing
    // the FakeMpv/placeholder gap BUG-VIDEO-1 records.
    if loaded {
        if let Some(frame) = controller.player_mut().engine_mut().latest_frame() {
            video.upload(ui.ctx(), &frame);
        }
    } else {
        video.clear();
    }

    match video.texture() {
        Some(texture) if loaded => {
            let tex_id = texture.id();
            egui::Image::new(egui::load::SizedTexture::new(tex_id, rect.size())).paint_at(ui, rect);
        }
        _ => {
            // The dark stage panel, then a *designed* transient state centred on it: an
            // accent spinner while the engine buffers the pick, the title once it is
            // loaded, or the honest "no media" prompt (§7 — never a faked frame).
            ui.painter().rect_filled(rect, Style::RADIUS, Style::BG);
            if controller.player().state() == PlayerState::Loading {
                // A genuine motion cue for the buffering wait (the Spinner drives its
                // own repaint), keyed on the live Loading state — not a frozen still.
                let spinner_rect = egui::Rect::from_center_size(
                    rect.center() - egui::vec2(0.0, Style::SP_M),
                    egui::vec2(Style::SP_L, Style::SP_L),
                );
                ui.put(
                    spinner_rect,
                    egui::Spinner::new().color(Style::ACCENT).size(Style::SP_L),
                );
                ui.painter().text(
                    rect.center() + egui::vec2(0.0, Style::SP_M),
                    Align2::CENTER_CENTER,
                    format!("Buffering {title}…"),
                    FontId::proportional(Style::BODY),
                    Style::TEXT_DIM,
                );
            } else {
                let (center_text, center_color) = if loaded {
                    (title.clone(), Style::TEXT)
                } else {
                    (
                        "No media loaded — pick a title in Library".to_owned(),
                        Style::TEXT_DIM,
                    )
                };
                ui.painter().text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    center_text,
                    FontId::proportional(Style::BODY),
                    center_color,
                );
            }
        }
    }

    // The OSD scrim + readout, over the video, **cross-fading** in/out on the dwell
    // (CRAFT §8.2 — it eases rather than popping) with the timecode in the mono metric
    // face (mono-first lock). Both the scrim and the text scale by the eased progress.
    let paused = controller.player().state() != PlayerState::Playing;
    let osd_t = Motion::animate(
        ui.ctx(),
        "media-osd-fade",
        loaded && osd_should_show(controller.ui().osd_idle_secs, paused),
        Motion::BASE,
    );
    if osd_t > 0.0 {
        let osd_h = Style::SP_XL;
        let osd_rect =
            egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - osd_h), rect.max);
        ui.painter().rect_filled(
            osd_rect,
            Style::RADIUS,
            Style::BG.gamma_multiply(OSD_SCRIM * osd_t),
        );
        let position = controller.player().position();
        let osd = format!(
            "{}  ·  {}  ·  {}",
            state_word(controller.player().state()),
            title,
            format_time(position),
        );
        ui.painter().text(
            egui::pos2(osd_rect.left() + Style::SP_S, osd_rect.center().y),
            Align2::LEFT_CENTER,
            osd,
            FontId::monospace(Style::SMALL),
            Style::TEXT.gamma_multiply(osd_t),
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
                                if queue_icon_button(
                                    ui,
                                    QueueControlIcon::Remove,
                                    "Remove from queue",
                                )
                                .clicked()
                                {
                                    action = Some(TransportAction::RemoveQueueIndex(index));
                                }
                                if index + 1 < len
                                    && queue_icon_button(
                                        ui,
                                        QueueControlIcon::MoveDown,
                                        "Move down",
                                    )
                                    .clicked()
                                {
                                    action = Some(TransportAction::MoveQueueItem(index, index + 1));
                                }
                                if index > 0
                                    && queue_icon_button(ui, QueueControlIcon::MoveUp, "Move up")
                                        .clicked()
                                {
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

#[derive(Clone, Copy)]
enum QueueControlIcon {
    Remove,
    MoveDown,
    MoveUp,
}

/// A compact icon-only queue action. It follows the shared YAMIS button idiom used
/// by sibling egui surfaces: allocate an empty button, install a real labelled
/// widget for accessibility, then paint the action mark as geometry rather than
/// text. The queue stays inside this crate's dependency boundary while avoiding
/// raw glyph text in the render output.
fn queue_icon_button(ui: &mut egui::Ui, icon: QueueControlIcon, label: &'static str) -> Response {
    let enabled = ui.is_enabled();
    let response = ui.add(
        egui::Button::new("")
            .fill(Style::LAYER_02)
            .stroke(egui::Stroke::new(1.0, Style::BORDER))
            .corner_radius(Style::RADIUS_S)
            .min_size(egui::vec2(QUEUE_CONTROL_BUTTON, QUEUE_CONTROL_BUTTON)),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));

    let tint = if enabled {
        Style::TEXT
    } else {
        Style::TEXT_DIM
    };
    let icon_rect = egui::Rect::from_center_size(
        response.rect.center(),
        egui::vec2(QUEUE_CONTROL_ICON, QUEUE_CONTROL_ICON),
    );
    paint_queue_control_icon(ui.painter(), icon_rect, icon, tint);
    mde_egui::focus::paint_focus_ring(ui.painter(), response.rect, response.has_focus());

    response
        .on_hover_text(label)
        .on_hover_cursor(CursorIcon::PointingHand)
}

fn paint_queue_control_icon(
    painter: &egui::Painter,
    rect: egui::Rect,
    icon: QueueControlIcon,
    tint: egui::Color32,
) {
    match icon {
        QueueControlIcon::Remove => {
            let mark = egui::Rect::from_center_size(
                rect.center(),
                egui::vec2(rect.width() * 0.82, Style::SP_XS * 0.5),
            );
            painter.rect_filled(mark, Style::RADIUS_S * 0.25, tint);
        }
        QueueControlIcon::MoveDown => paint_queue_arrow(painter, rect, false, tint),
        QueueControlIcon::MoveUp => paint_queue_arrow(painter, rect, true, tint),
    }
}

fn paint_queue_arrow(painter: &egui::Painter, rect: egui::Rect, up: bool, tint: egui::Color32) {
    let center = rect.center();
    let half_w = rect.width() * 0.38;
    let half_h = rect.height() * 0.30;
    let points = if up {
        vec![
            egui::pos2(center.x, center.y - half_h),
            egui::pos2(center.x + half_w, center.y + half_h),
            egui::pos2(center.x - half_w, center.y + half_h),
        ]
    } else {
        vec![
            egui::pos2(center.x, center.y + half_h),
            egui::pos2(center.x - half_w, center.y - half_h),
            egui::pos2(center.x + half_w, center.y - half_h),
        ]
    };
    painter.add(egui::Shape::convex_polygon(
        points,
        tint,
        egui::Stroke::NONE,
    ));
}

// ── PiP mini-player ────────────────────────────────────────────────────────────────

/// The floating `PiP` mini-player's window shadow — the surface-side conversion of the
/// shared [`Elevation::Overlay`](mde_egui::style::Elevation::Overlay) depth token into
/// an [`egui::Shadow`] (the token module stays free of egui's shadow type). Every field
/// comes straight from the token: offset/blur/spread cast onto epaint's small integer
/// fields, and the umbra colour verbatim — no minted `Color32` (§4) — so the mini-player
/// reads as a genuine floating overlay lifted off the surface behind it, and the depth
/// is a translucent umbra (design lock #2), not egui's stock window shadow.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // token px values are small +ve.
fn pip_overlay_shadow() -> egui::Shadow {
    let token = mde_egui::style::Elevation::Overlay.shadow();
    egui::Shadow {
        offset: [token.offset[0] as i8, token.offset[1] as i8],
        blur: token.blur as u8,
        spread: token.spread as u8,
        color: token.umbra,
    }
}

/// The floating `PiP` mini-player (design Q31/Q32): a compact now-playing + play/pause
/// window shown when [`crate::model::UiState::pip`] is on. A real, reachable window —
/// not a stub.
pub fn pip_window<E: MediaEngine>(ctx: &Context, controller: &mut MediaController<E>) {
    if !controller.ui().pip {
        return;
    }
    let mut action: Option<TransportAction> = None;
    let mut close = false;
    // The stock window frame, its depth re-sourced from the shared Overlay token so the
    // floating mini-player casts the same soft overlay shadow as every other popover
    // (same fill/stroke/margin — only the shadow changes, no layout change).
    let window_frame = egui::Frame::window(&ctx.style()).shadow(pip_overlay_shadow());
    egui::Window::new("Mini-player")
        .resizable(false)
        .collapsible(false)
        .frame(window_frame)
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

/// A **transport-control button** — an icon/label button that names itself in a
/// tooltip and shows the pointing-hand cursor (CRAFT §6: every icon-only control is
/// keyboard/hover discoverable, and buttons show a hand). Consolidates the transport
/// rows' repeated `button + on_hover_text + on_hover_cursor` idiom so every transport
/// affordance reads identically. Draws through the shared `Style` (§4).
fn transport_button(ui: &mut egui::Ui, label: &str, tip: &str) -> Response {
    ui.button(label)
        .on_hover_text(tip)
        .on_hover_cursor(CursorIcon::PointingHand)
}

/// The **primary** transport action — the play/pause — rendered as the one
/// accent-filled control (design lock: a single accent, reserved for the primary
/// action, so the eye lands on it first in the transport row). Built from `Style`
/// tokens only (§4): a [`Style::ACCENT`] fill under [`Style::TEXT_STRONG`] text, plus
/// the shared transport tooltip + pointing-hand cursor.
fn primary_transport_button(ui: &mut egui::Ui, label: &str, tip: &str) -> Response {
    ui.add(egui::Button::new(RichText::new(label).color(Style::TEXT_STRONG)).fill(Style::ACCENT))
        .on_hover_text(tip)
        .on_hover_cursor(CursorIcon::PointingHand)
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
    fn render_shapes(
        controller: &mut MediaController<FakeMpv>,
        body: impl Fn(&mut egui::Ui, &mut MediaController<FakeMpv>),
    ) -> Vec<egui::epaint::ClippedShape> {
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
        let prims = ctx.tessellate(out.shapes.clone(), out.pixels_per_point);
        assert!(!prims.is_empty(), "frame produced no draw primitives");
        out.shapes
    }

    fn render(
        controller: &mut MediaController<FakeMpv>,
        body: impl Fn(&mut egui::Ui, &mut MediaController<FakeMpv>),
    ) {
        let _ = render_shapes(controller, body);
    }

    /// Like [`render`], but also threads a [`VideoTextureCache`] — for bodies
    /// that reach `player_stage` (`player_view`/`media_panel`), which now owns
    /// the MEDIA-2 phase-1 video frame-sink texture (`docs/gpu_encoder.md`).
    fn render_with_video(
        controller: &mut MediaController<FakeMpv>,
        video: &mut VideoTextureCache,
        body: impl Fn(&mut egui::Ui, &mut MediaController<FakeMpv>, &mut VideoTextureCache),
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
            egui::CentralPanel::default().show(ctx, |ui| body(ui, controller, video));
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

    fn painted_text(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
        fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => out.push(text.galley.text().to_owned()),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }

        let mut out = Vec::new();
        for clipped in shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    fn queue_icon_shape_count(shapes: &[egui::epaint::ClippedShape]) -> usize {
        fn walk(shape: &egui::Shape) -> usize {
            match shape {
                egui::Shape::Rect(rect)
                    if rect.fill == Style::TEXT
                        && rect.rect.width() <= QUEUE_CONTROL_ICON
                        && rect.rect.height() <= Style::SP_XS =>
                {
                    1
                }
                egui::Shape::Path(path)
                    if path.fill == Style::TEXT && path.closed && path.points.len() == 3 =>
                {
                    1
                }
                egui::Shape::Mesh(mesh) if !mesh.vertices.is_empty() => 1,
                egui::Shape::Vec(shapes) => shapes.iter().map(walk).sum(),
                _ => 0,
            }
        }

        shapes.iter().map(|clipped| walk(&clipped.shape)).sum()
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
    fn library_grid_columns_preserve_bounded_card_widths() {
        assert_eq!(library_grid_columns(0.0), 1);
        assert_eq!(library_grid_columns(LIBRARY_CARD_MIN_W), 1);
        assert_eq!(
            library_grid_columns(LIBRARY_CARD_MIN_W * 2.0 + LIBRARY_GRID_GAP - 1.0),
            1
        );
        assert_eq!(
            library_grid_columns(LIBRARY_CARD_MIN_W * 2.0 + LIBRARY_GRID_GAP),
            2
        );
        assert_eq!(
            library_grid_columns(LIBRARY_CARD_MIN_W * 3.0 + LIBRARY_GRID_GAP * 2.0),
            3
        );

        let width = LIBRARY_CARD_MIN_W * 3.0 + LIBRARY_GRID_GAP * 2.0 + Style::SP_XL;
        let columns = library_grid_columns(width);
        let card_w = library_grid_card_width(width, columns);
        assert_eq!(columns, 3);
        assert!(card_w > LIBRARY_CARD_MIN_W);
        let occupied = card_w * columns as f32 + LIBRARY_GRID_GAP * (columns - 1) as f32;
        assert!((occupied - width).abs() < f32::EPSILON);
    }

    #[test]
    fn library_cards_dispatch_the_existing_transport_actions() {
        let item = LibraryItem {
            path: "/m/song.flac".to_owned(),
            metadata: MediaMetadata::from_path("/m/song.flac")
                .expect("audio")
                .with_artist("Artist")
                .with_duration(210.0),
            added_seq: 0,
        };
        let card = LibraryCard::from_item(&item);

        assert_eq!(card.title, "song");
        assert_eq!(card.subtitle, "Audio · 3:30 · Artist");
        assert_eq!(card.kind, MediaKind::Audio);
        assert_eq!(
            card_play_action(&card),
            TransportAction::PlayPath("/m/song.flac".to_owned())
        );
        assert_eq!(
            card_queue_action(&card),
            TransportAction::Enqueue("/m/song.flac".to_owned(), Some("song".to_owned()))
        );
    }

    #[test]
    fn player_view_renders_idle_and_playing_with_osd() {
        let mut c = controller();
        let mut video = VideoTextureCache::default();
        // Idle: the stage shows the "no media" prompt.
        render_with_video(&mut c, &mut video, player_view);
        // Load (before the pump) → Loading: the designed buffering state (the accent
        // Spinner over the stage) is proven runtime-reachable (§7).
        c.dispatch(TransportAction::PlayPath("clip.mkv".to_owned()));
        assert_eq!(c.player().state(), PlayerState::Loading);
        render_with_video(&mut c, &mut video, player_view);
        // Pump → Playing; the scrubber, transport, speed, A-B, tracks all draw.
        c.pump();
        c.ui_mut().osd_idle_secs = 0.0; // OSD visible
        render_with_video(&mut c, &mut video, player_view);
        // Idle-hidden OSD branch.
        c.ui_mut().osd_idle_secs = crate::model::OSD_HIDE_SECS + 1.0;
        render_with_video(&mut c, &mut video, player_view);
    }

    /// A canned discovery so the cast list renders with no real network probe.
    struct FakeDiscovery(Vec<mde_media_core::CastTarget>);
    impl mde_media_core::RendererDiscovery for FakeDiscovery {
        fn discover(&self) -> Vec<mde_media_core::CastTarget> {
            self.0.clone()
        }
    }

    #[test]
    fn party_and_cast_section_renders_all_branches() {
        let mut c = controller();
        c.dispatch(TransportAction::PlayPath("clip.mkv".to_owned()));
        c.pump();

        // Not joined + un-probed: the party name field + the honest "look" hint.
        render(&mut c, party_cast_controls);

        // A discovered renderer draws a Cast row (§7 — the whole cast picker is reachable).
        c.refresh_cast_targets(&FakeDiscovery(vec![mde_media_core::CastTarget {
            kind: mde_media_core::CastKind::DlnaUpnp,
            id: "tv-1".to_owned(),
            name: "Living Room TV".to_owned(),
            location: "http://192.168.1.50:8200/desc.xml".to_owned(),
        }]));
        render(&mut c, party_cast_controls);

        // The empty-probe honest gate renders its "no renderer found" note.
        c.refresh_cast_targets(&FakeDiscovery(vec![]));
        render(&mut c, party_cast_controls);

        // Joined over a tempdir root: the in-party branch (members + Leave) renders.
        let dir = tempfile::tempdir().expect("tempdir");
        c.enable_party(mde_media_core::PartySession::new(
            mde_media_core::PartyStore::new(dir.path().to_path_buf()),
            "movie-night",
            "seat-a",
        ));
        render(&mut c, party_cast_controls);
        assert!(c.party_enabled());
        assert_eq!(
            c.player().state(),
            PlayerState::Playing,
            "render never stops"
        );
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
        render_with_video(&mut c, &mut VideoTextureCache::default(), player_view);
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
        let shapes = render_shapes(&mut c, queue_view);
        let texts = painted_text(&shapes);
        assert!(
            !texts
                .iter()
                .any(|text| text.contains('✕') || text.contains('▼') || text.contains('▲')),
            "queue controls must not paint raw glyph text: {texts:?}"
        );
        let icons = queue_icon_shape_count(&shapes);
        assert!(
            icons >= 7,
            "queue controls must paint icon geometry/textures for remove and move controls; got {icons}"
        );
    }

    #[test]
    fn pip_and_fullscreen_states_render() {
        let mut c = controller();
        c.dispatch(TransportAction::PlayPath("clip.mkv".to_owned()));
        c.pump();
        c.ui_mut().pip = true;
        c.ui_mut().fullscreen = true;
        // The PiP window + header (with the toggles lit) render.
        render_with_video(&mut c, &mut VideoTextureCache::default(), player_view);
    }

    /// The floating PiP mini-player casts the shared `Elevation::Overlay` soft shadow
    /// (Phase-C depth adoption): every field of [`pip_overlay_shadow`] comes straight
    /// from the token — offset/blur/spread and, critically, the umbra colour (no minted
    /// `Color32`, §4) — and the umbra stays translucent (design lock #2), so the window
    /// reads as a genuine overlay, not egui's stock window shadow.
    #[test]
    fn pip_window_casts_the_overlay_depth_token() {
        let overlay = mde_egui::style::Elevation::Overlay.shadow();
        let shadow = pip_overlay_shadow();
        assert_eq!(
            shadow.offset,
            [overlay.offset[0] as i8, overlay.offset[1] as i8],
            "the PiP window shadow offset comes from the Overlay token"
        );
        assert_eq!(
            shadow.blur, overlay.blur as u8,
            "the PiP window shadow blur comes from the Overlay token"
        );
        assert_eq!(
            shadow.spread, overlay.spread as u8,
            "the PiP window shadow spread comes from the Overlay token"
        );
        assert_eq!(
            shadow.color, overlay.umbra,
            "the PiP window shadow umbra is the Overlay token's, not a minted colour"
        );
        assert!(
            shadow.color.a() > 0 && shadow.color.a() < 255,
            "the depth is a translucent umbra (lock #2), never an opaque fill"
        );
    }

    #[test]
    fn tabs_switch_via_the_header() {
        // The header writes the chosen tab back onto the controller — proving the tab
        // bar is live wiring, not decoration. We can't click headlessly, so drive the
        // state the header binds to and confirm a full frame renders on each tab.
        let mut c = controller();
        populate(&mut c);
        let mut video = VideoTextureCache::default();
        for tab in MediaTab::all() {
            c.ui_mut().tab = tab;
            render_with_video(&mut c, &mut video, media_panel);
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
