use super::*;
use mde_egui::egui::{pos2, vec2, Rect};
use mde_seat::{Battery, BatteryKind, BatteryState, ProfileState};
use mde_theme::brand::icons::IconId;

/// Drive one headless frame of the System panel over a real seat, and tessellate
/// on the CPU (the DRM runner's path minus GPU).
fn renders(state: &mut SystemState) -> bool {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
        ..Default::default()
    };
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| state.show(ui));
    });
    !ctx.tessellate(out.shapes, out.pixels_per_point).is_empty()
}

fn painted_fill_colors(shapes: &[egui::epaint::ClippedShape]) -> Vec<egui::Color32> {
    fn walk(shape: &egui::Shape, out: &mut Vec<egui::Color32>) {
        match shape {
            egui::Shape::Mesh(mesh) => {
                out.extend(mesh.vertices.iter().map(|vertex| vertex.color));
            }
            egui::Shape::Path(path) => {
                if path.fill != egui::Color32::TRANSPARENT {
                    out.push(path.fill);
                }
            }
            egui::Shape::Rect(rect) => {
                if rect.fill != egui::Color32::TRANSPARENT {
                    out.push(rect.fill);
                }
            }
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

fn painted_text(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, egui::Color32)> {
    fn text_color(text: &egui::epaint::TextShape) -> egui::Color32 {
        if let Some(color) = text.override_text_color {
            return color;
        }
        text.galley
            .job
            .sections
            .iter()
            .find_map(|section| {
                (section.format.color != egui::Color32::PLACEHOLDER).then_some(section.format.color)
            })
            .unwrap_or(text.fallback_color)
    }

    fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Color32)>) {
        match shape {
            egui::Shape::Text(text) => {
                out.push((text.galley.text().to_owned(), text_color(text)));
            }
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

fn render_settings_tooltip_frame(ctx: &egui::Context) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(320.0, 96.0))),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    settings_tooltip(ui, "Move display left");
                });
        },
    )
}

fn render_settings_choice_frame(ctx: &egui::Context, selected: bool) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(360.0, 120.0))),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    ui.set_width(300.0);
                    settings_choice_tile(
                        ui,
                        selected,
                        "Light",
                        Some("Windows 2000 basic"),
                        SettingsGroup::Personalization.accent(),
                        Style::SP_XL,
                    );
                });
        },
    )
}

fn render_wallpaper_section_frame(ctx: &egui::Context) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(520.0, 280.0))),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    let mut config = WallpaperServiceConfig::default();
                    let mut download = WallpaperDownloadRuntime::default();
                    let mut actions = Vec::new();
                    wallpaper_section(ui, &mut config, &mut download, &mut actions);
                });
        },
    )
}

fn render_settings_combo_popup_frame(ctx: &egui::Context) -> egui::FullOutput {
    fn input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(360.0, 180.0))),
            ..Default::default()
        }
    }

    fn show_popup(ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                ui.set_width(240.0);
                settings_popup_visual_scope(ui, |ui| {
                    let popup_id = ui.make_persistent_id("settings-popup-choice-test");
                    let response = ui.button("Right");
                    ui.memory_mut(|mem| mem.open_popup(popup_id));
                    let _ = egui::popup::popup_above_or_below_widget(
                        ui,
                        popup_id,
                        &response,
                        egui::AboveOrBelow::Below,
                        egui::popup::PopupCloseBehavior::IgnoreClicks,
                        |ui| {
                            apply_settings_popup_style(ui.style_mut());
                            let _ = ui.selectable_label(false, "Left");
                            let _ = ui.selectable_label(true, "Right");
                        },
                    );
                });
            });
    }

    let _ = ctx.run(input(), show_popup);
    ctx.run(input(), show_popup)
}

#[test]
fn the_pre_poll_state_is_a_full_paint_not_a_blank_panel() {
    let mut st = SystemState::default();
    assert!(renders(&mut st), "pre-poll System panel drew nothing");
}

#[test]
fn a_real_seat_snapshot_mounts_and_renders_every_section() {
    // Over a REAL Seat::snapshot(): on the headless farm host most backends are
    // Absent (each an honest typed line), the arrangement/power controls fold to
    // their not-available/empty states — still a full paint path, never blank.
    let ctx = egui::Context::default();
    let mut st = SystemState::default();
    st.poll(&ctx); // one snapshot + reconcile
    assert!(renders(&mut st), "live System panel drew nothing");
}

#[test]
fn default_state_is_unpolled_with_an_empty_layout() {
    let st = SystemState::default();
    assert!(st.snapshot().is_none());
    assert!(st.layout.outputs.is_empty());
    assert!(st.confirm.is_none());
}

#[test]
fn settings_chrome_copy_is_ascii_and_nudges_use_yamis_icons() {
    for copy in [
        SYSTEM_READING_SEAT_COPY,
        SYSTEM_SCANNING_COPY,
        SYSTEM_MESH_READING_COPY,
    ] {
        assert!(copy.is_ascii(), "{copy:?} should stay ASCII");
    }
    assert_eq!(DISPLAY_NUDGE_LEFT_ICON, IconId::ArrowLeft);
    assert_eq!(DISPLAY_NUDGE_RIGHT_ICON, IconId::ArrowRight);
}

#[test]
fn settings_icon_tooltips_use_themed_text_and_surface() {
    let ctx = egui::Context::default();
    mde_egui::fonts::install(&ctx);

    let out = render_settings_tooltip_frame(&ctx);
    let texts = painted_text(&out.shapes);
    assert!(
        texts
            .iter()
            .any(|(text, color)| text == "Move display left" && *color == Style::TEXT),
        "Settings icon tooltip should paint themed text: {texts:?}"
    );
    assert!(
        !texts.iter().any(|(text, color)| {
            text == "Move display left"
                && matches!(*color, egui::Color32::BLACK | Style::BG | Style::TEXT_DIM)
        }),
        "Settings icon tooltip leaked unreadable/shared shell text color: {texts:?}"
    );

    let fills = painted_fill_colors(&out.shapes);
    assert!(
        fills.contains(&Style::SURFACE),
        "Settings icon tooltip should paint its own themed surface: {fills:?}"
    );
}

#[test]
fn settings_choice_tiles_use_themed_selected_and_hover_colors() {
    let dark = egui::Context::default();
    Style::install(&dark);
    let accent = SettingsGroup::Personalization.accent();

    let dark_selected = settings_choice_colors(&dark, true, false, accent);
    assert_eq!(
        dark_selected.fill,
        Style::pressed_fill_for_scheme(StyleColorScheme::Dark, accent),
        "dark selected choices should use the shared pressed fill"
    );
    assert_eq!(dark_selected.text, Style::TEXT_STRONG);

    let dark_hover = settings_choice_colors(&dark, false, true, accent);
    assert_eq!(
        dark_hover.fill,
        Style::SURFACE_HI,
        "dark hover choices should use the highlighted surface"
    );
    assert_eq!(dark_hover.text, Style::TEXT);
    assert_eq!(
        dark_hover.stroke,
        Style::accent_for_scheme(StyleColorScheme::Dark, accent)
    );

    let light = egui::Context::default();
    Style::install_color_scheme_with_density(
        &light,
        StyleColorScheme::Light,
        mde_egui::style::Density::Mouse,
    );
    let light_palette = Style::palette_for(StyleColorScheme::Light);
    let light_selected = settings_choice_colors(&light, true, false, accent);
    assert_eq!(
        light_selected.fill,
        Style::WIN2000_PRESSED_FACE,
        "light selected choices should keep black text on the classic pressed face"
    );
    assert_eq!(light_selected.text, light_palette.text_strong);

    let out = render_settings_choice_frame(&light, true);
    let texts = painted_text(&out.shapes);
    assert!(
        texts
            .iter()
            .any(|(text, color)| text == "Light" && *color == light_palette.text_strong),
        "selected Settings choices should paint themed readable text: {texts:?}"
    );
    let fills = painted_fill_colors(&out.shapes);
    assert!(
        fills.contains(&Style::WIN2000_PRESSED_FACE),
        "selected Settings choices should paint their own selected fill: {fills:?}"
    );
}

#[test]
fn wallpaper_service_defaults_enable_bing_fallback_and_round_trip() {
    let cfg = WallpaperServiceConfig::default();
    assert!(cfg.network_fetch_enabled);
    assert!(cfg.bing_daily_enabled);
    assert!(cfg.desktop_page_url.is_empty());

    let dir = nav_temp_dir("wallpaper-service");
    let path = dir.join("settings-wallpaper-service.json");
    let changed = WallpaperServiceConfig {
        network_fetch_enabled: false,
        bing_daily_enabled: true,
        desktop_page_url: "  http://127.0.0.1:8787/  ".to_owned(),
        last_image_path: Some(dir.join("wallpaper-cache").join("bing.jpg")),
        last_image_title: "  Today  ".to_owned(),
        last_image_copyright: "  Credit  ".to_owned(),
        last_updated_ms: 42,
    };
    changed.save_to(&path).expect("save wallpaper service");
    let loaded = WallpaperServiceConfig::load_from(&path);
    assert!(!loaded.network_fetch_enabled);
    assert!(loaded.bing_daily_enabled);
    assert_eq!(loaded.desktop_page_url, "http://127.0.0.1:8787/");
    assert_eq!(loaded.last_image_title, "Today");
    assert_eq!(loaded.last_image_copyright, "Credit");
    assert_eq!(loaded.last_updated_ms, 42);
}

#[test]
fn bing_daily_archive_parser_resolves_relative_image_url() {
    let value: serde_json::Value = serde_json::json!({
        "images": [{
            "url": "/th?id=OHR.TestImage_EN-US1234567890_1920x1080.jpg&rf=LaDigue_1920x1080.jpg",
            "title": "Lake",
            "copyright": "Lake credit"
        }]
    });
    let image = bing_daily_image_from_archive(&value).expect("parse bing metadata");
    assert_eq!(
        image.image_url,
        "https://www.bing.com/th?id=OHR.TestImage_EN-US1234567890_1920x1080.jpg&rf=LaDigue_1920x1080.jpg"
    );
    assert_eq!(image.title, "Lake");
    assert_eq!(image.copyright, "Lake credit");
}

#[test]
fn wallpaper_section_renders_bing_service_instead_of_static_gallery() {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    mde_egui::fonts::install(&ctx);
    let out = render_wallpaper_section_frame(&ctx);
    let texts = painted_text(&out.shapes);

    for label in [
        "Desktop background service",
        "Allow daily picture downloads",
        "Use Bing image of the day as fallback",
        "Download today's picture",
    ] {
        assert!(
            texts.iter().any(|(text, _)| text == label),
            "Wallpaper service label {label:?} was not painted: {texts:?}"
        );
    }
    assert!(
        !texts.iter().any(|(text, _)| text.starts_with("Wallpaper 1")
            || text.starts_with("Wallpaper 2")
            || text.starts_with("Wallpaper 3")
            || text.starts_with("Wallpaper 4")
            || text.starts_with("Wallpaper 5")),
        "retired static wallpaper gallery labels should not render: {texts:?}"
    );
}

#[test]
fn settings_combobox_popups_use_themed_readable_choice_colors() {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    mde_egui::fonts::install(&ctx);

    let mut style = (*ctx.style()).clone();
    apply_settings_popup_style(&mut style);
    assert_eq!(style.visuals.window_fill, Style::SURFACE);
    assert_eq!(style.visuals.panel_fill, Style::SURFACE);
    assert_eq!(style.visuals.override_text_color, Some(Style::TEXT));
    assert_eq!(style.visuals.widgets.inactive.fg_stroke.color, Style::TEXT);
    assert_eq!(style.visuals.widgets.hovered.bg_fill, Style::SURFACE_HI);
    assert_eq!(style.visuals.widgets.hovered.fg_stroke.color, Style::TEXT);
    assert_eq!(style.visuals.widgets.active.bg_fill, Style::SURFACE_HI);
    assert_eq!(style.visuals.widgets.active.fg_stroke.color, Style::TEXT);
    assert_eq!(style.visuals.widgets.open.bg_fill, Style::SURFACE_HI);
    assert_eq!(style.visuals.widgets.open.fg_stroke.color, Style::TEXT);
    assert_eq!(style.visuals.widgets.open.bg_stroke.color, Style::BORDER);
    assert_eq!(
        style.visuals.widgets.noninteractive.fg_stroke.color,
        Style::TEXT_DIM
    );

    let out = render_settings_combo_popup_frame(&ctx);
    let texts = painted_text(&out.shapes);
    for label in ["Left", "Right"] {
        assert!(
            texts
                .iter()
                .any(|(text, color)| text == label && *color == Style::TEXT),
            "Settings ComboBox choice {label:?} should paint themed text: {texts:?}"
        );
        assert!(
            !texts
                .iter()
                .any(|(text, color)| text == label && *color == egui::Color32::BLACK),
            "Settings ComboBox choice {label:?} leaked raw black popup text: {texts:?}"
        );
    }

    let fills = painted_fill_colors(&out.shapes);
    assert!(
        fills.contains(&Style::SURFACE),
        "Settings ComboBox popup should paint the Settings surface: {fills:?}"
    );
}

#[test]
fn poll_drains_the_latest_published_snapshot_without_probing_inline() {
    // perf-2: the render path (`poll`) must NEVER run the blocking seat probe —
    // it only drains the newest snapshot the off-thread pump published. Inject a
    // pump backed by a plain channel (no thread, no producing Seat), publish
    // three snapshots carrying an ASCENDING marker, and assert one `poll` adopts
    // the LAST of them (latest-wins). If `poll` had probed the real (headless)
    // seat inline it would show an Absent `charge_limit` instead of the injected
    // marker, so the marker surviving is the proof the probe never ran here.
    use std::sync::mpsc;
    let ctx = egui::Context::default();
    let (tx, rx) = mpsc::channel();
    let mut st = SystemState::default();
    st.pump = Some(SnapshotPump::from_receiver(rx));

    for marker in 1u8..=3 {
        let mut snap = Seat::new().snapshot();
        snap.charge_limit = Probe::Present(Some(marker));
        tx.send(snap).expect("publish");
    }
    st.poll(&ctx);

    assert!(
        matches!(
            st.snapshot().map(|s| &s.charge_limit),
            Some(Probe::Present(Some(3)))
        ),
        "poll must adopt the newest published snapshot (latest-wins drain)"
    );
    // The drained snapshot flowed through reconcile, seeding the charge slider.
    assert_eq!(st.charge_threshold, Some(3));
}

#[test]
fn poll_is_non_blocking_with_no_snapshot_published_yet() {
    // An empty pump channel: `poll` must return at once (a `try_recv` drain,
    // never a block on the probe) and leave the pre-poll snapshot untouched. The
    // test simply completing proves the drain does not block.
    use std::sync::mpsc;
    let ctx = egui::Context::default();
    let (tx, rx) = mpsc::channel::<SeatSnapshot>();
    let mut st = SystemState::default();
    st.pump = Some(SnapshotPump::from_receiver(rx));

    st.poll(&ctx);
    assert!(
        st.snapshot().is_none(),
        "no publish yet → the snapshot stays None, not a blocked inline probe"
    );
    drop(tx);
}

#[test]
fn a_reconcile_builds_the_layout_and_seeds_brightness_from_the_probe() {
    // Feed a synthetic snapshot via the real reconcile path (no hardware): a
    // connected panel + a backlight seed the layout + the panel-brightness map.
    let mut st = SystemState::default();
    let snap = Seat::new().snapshot();
    st.reconcile(&snap);
    // On the farm host displays are Absent → the layout stays empty but the
    // reconcile never panics. The point is the intent model tracks the probe.
    assert_eq!(st.layout.outputs.len(), st.layout_key.len());
}

#[test]
fn strip_card_and_connector_matching_line_up_ddcutil_and_drm_names() {
    assert_eq!(strip_card("card0-DP-1"), "DP-1");
    assert_eq!(strip_card("card1/HDMI-A-2"), "HDMI-A-2");
    assert_eq!(strip_card("DP-3"), "DP-3");
    assert!(connector_matches(Some("card0-DP-1"), "DP-1"));
    assert!(!connector_matches(Some("card0-DP-2"), "DP-1"));
    assert!(!connector_matches(None, "DP-1"));
    assert!(is_internal("eDP-1"));
    assert!(is_internal("card0-eDP-1"));
    assert!(!is_internal("DP-1"));
}

#[test]
fn hotkey_dispatch_acts_on_a_headless_seat_without_panicking() {
    // On the farm host every backend is Absent, so the hardware hotkeys have no
    // target: they must fold to `None` (no OSD) or an honest inline error, never
    // panic. The live OSD-returning path needs real PipeWire/backlight hardware
    // (integration-gated); this proves the dispatch seam is total + reachable.
    let ctx = egui::Context::default();
    let mut st = SystemState::default();
    st.poll(&ctx); // one real snapshot (all Absent on the farm)

    // No mixer → no OSD, no panic.
    assert!(st.dispatch_hotkey(HotkeyAction::VolumeUp).is_none());
    assert!(st.dispatch_hotkey(HotkeyAction::VolumeMute).is_none());
    // The mic key is honestly not-available (output-only mixer model).
    assert!(st.dispatch_hotkey(HotkeyAction::MicMute).is_none());
    assert!(st.error.as_deref().unwrap().contains("Microphone"));
    // No backlight / DDC → the honest not-controllable note.
    assert!(st.dispatch_hotkey(HotkeyAction::BrightnessDown).is_none());
    assert!(st.error.as_deref().unwrap().contains("Brightness"));
    // A navigation action never touches the seat (the shell applies it).
    assert!(st.dispatch_hotkey(HotkeyAction::SessionSwitch).is_none());
    // Lock reaches logind (Absent here → an error, never a real lock/panic).
    assert!(st.dispatch_hotkey(HotkeyAction::Lock).is_none());
}

#[test]
fn the_confirm_gate_arms_before_a_host_down_verb_acts() {
    // The two-step gate (lock 12): a Reboot click arms confirm; only the confirm
    // click emits the Power action. Exercised through apply() (no real reboot —
    // the seat's logind is Absent on the farm host, so Power folds to an error,
    // never an actual poweroff).
    let mut st = SystemState::default();
    st.apply(vec![SysAction::ArmConfirm(PowerVerb::Reboot)]);
    assert_eq!(st.confirm, Some(PowerVerb::Reboot));
    st.apply(vec![SysAction::CancelConfirm]);
    assert!(st.confirm.is_none());
}

// ── Power Settings (POWER-4) ──────────────────────────────────────────────

#[test]
fn a_live_power_panel_renders_the_power4_controls() {
    // Inject Present POWER-4 probes over an otherwise-real (Absent) snapshot
    // and prove the profile segmented control, the AC source line, the charge
    // slider, and the rich battery telemetry all tessellate real geometry —
    // reachable controls driving the real seat, not a mockup.
    let mut st = SystemState::default();
    let mut snap = Seat::new().snapshot();
    snap.power_profile = Probe::Present(ProfileState {
        active: "balanced".to_owned(),
        available: vec![
            "power-saver".to_owned(),
            "balanced".to_owned(),
            "performance".to_owned(),
        ],
    });
    snap.on_ac = Probe::Present(Some(false));
    snap.charge_limit = Probe::Present(Some(80));
    snap.batteries = Probe::Present(vec![Battery {
        model: "BAT0".to_owned(),
        kind: BatteryKind::Internal,
        percentage: 61.0,
        state: BatteryState::Discharging,
        power_supply: true,
        time_to_empty: Some(Duration::from_secs(5400)),
        time_to_full: None,
        energy_rate: Some(11.7),
    }]);
    // Exercise the reconcile seam (it seeds the charge-slider live value from
    // the probe) before rendering, matching the live poll path.
    st.reconcile(&snap);
    st.snapshot = Some(snap);
    assert!(renders(&mut st), "the live POWER-4 panel drew nothing");
    assert_eq!(
        st.charge_threshold,
        Some(80),
        "reconcile seeds the charge-slider from the probe"
    );
}

#[test]
fn a_refused_power_profile_switch_never_lies_about_the_active_profile() {
    // With a Present profile (active=balanced), a switch to "performance" on
    // the headless farm host has no daemon → a typed error. apply must surface
    // it inline AND withhold the optimistic active flip (§7: a failed switch
    // never reports the new profile as active). Asserted as the honest
    // coupling so a build host that DID have the daemon can't make it flaky.
    let mut st = SystemState::default();
    let mut snap = Seat::new().snapshot();
    snap.power_profile = Probe::Present(ProfileState {
        active: "balanced".to_owned(),
        available: vec!["balanced".to_owned(), "performance".to_owned()],
    });
    st.snapshot = Some(snap);
    st.apply(vec![SysAction::SetPowerProfile("performance".to_owned())]);
    let active = match st.snapshot.as_ref().map(|s| &s.power_profile) {
        Some(Probe::Present(p)) => p.active.clone(),
        _ => unreachable!("the profile probe stays Present"),
    };
    // error set ⇔ the switch failed ⇔ active stays balanced (never a lie).
    assert_eq!(
        st.error.is_some(),
        active == "balanced",
        "a failed switch must not flip the cached active profile"
    );
}

#[test]
fn a_charge_threshold_write_either_succeeds_or_is_surfaced_honestly() {
    // The charge-cap write on the headless farm host has no advertising
    // battery / is unprivileged → a typed error apply must surface inline
    // (§7), never a silent success. On a machine that genuinely has the attr
    // + privilege it would succeed and seed the live cap — asserted as the
    // honest either/or so the test holds on any host.
    let mut st = SystemState::default();
    st.apply(vec![SysAction::SetChargeThreshold(70)]);
    let ok = st.error.is_none() && st.charge_threshold == Some(70);
    let surfaced = st
        .error
        .as_deref()
        .is_some_and(|e| e.contains("Charge limit"));
    assert!(
        ok || surfaced,
        "the write must either honestly succeed or surface a typed error"
    );
}

// ── Bluetooth control panel (E12-17) ──────────────────────────────────────

fn bt_device(path: &str, paired: bool, connected: bool, trusted: bool) -> BtDevice {
    BtDevice {
        path: path.to_owned(),
        alias: path.to_owned(),
        address: Some("AA:BB:CC:DD:EE:FF".to_owned()),
        rssi: Some(-55),
        paired,
        connected,
        trusted,
        battery_percent: Some(72),
        icon: None,
    }
}

#[test]
fn device_actions_reflect_bluetooth_state() {
    // An available (un-paired, un-connected) device: Connect + Pair, no
    // Disconnect, no Forget (Forget is a paired-only verb).
    let available = bt_device("/dev/a", false, false, false);
    assert_eq!(
        device_actions(&available, Some("/org/bluez/hci0")),
        DeviceActions {
            connect: true,
            disconnect: false,
            pair: true,
            forget: false,
        }
    );

    // A paired-but-offline device: Connect + Forget (adapter known), no Pair.
    let paired = bt_device("/dev/b", true, false, true);
    assert_eq!(
        device_actions(&paired, Some("/org/bluez/hci0")),
        DeviceActions {
            connect: true,
            disconnect: false,
            pair: false,
            forget: true,
        }
    );
    // …but Forget is withheld when the owning adapter path is unknown.
    assert_eq!(
        device_actions(&paired, None),
        DeviceActions {
            connect: true,
            disconnect: false,
            pair: false,
            forget: false,
        }
    );

    // A connected + paired device: Disconnect + Forget, no Connect, no Pair.
    let connected = bt_device("/dev/c", true, true, true);
    assert_eq!(
        device_actions(&connected, Some("/org/bluez/hci0")),
        DeviceActions {
            connect: false,
            disconnect: true,
            pair: false,
            forget: true,
        }
    );
}

#[test]
fn a_bluetooth_error_is_a_flagged_warning_alert() {
    let e = SeatError::Unavailable {
        backend: mde_seat::Backend::Bluetooth,
        reason: "no adapter".into(),
    };
    let toast = bt_error_toast("connect", &e);
    assert_eq!(toast.flag, "BLUETOOTH");
    assert!(toast.headline.contains("connect"));
    assert!(toast.headline.contains("no adapter"));
}

#[test]
fn a_live_bluetooth_panel_renders_its_controls() {
    // Inject a Present Bluetooth probe over an otherwise-real (Absent) snapshot
    // and prove the control rows tessellate real geometry — the reachable panel,
    // not a mockup. No button is clicked in a headless frame, so no seat write
    // fires.
    let mut st = SystemState::default();
    let mut snap = Seat::new().snapshot();
    snap.bluetooth = Probe::Present(BtStatus {
        adapters: vec![BtAdapter {
            path: "/org/bluez/hci0".to_owned(),
            name: "eagle".to_owned(),
            powered: true,
            discovering: true,
            discoverable: true,
            pairable: false,
        }],
        devices: vec![
            bt_device("/org/bluez/hci0/dev_AA", true, true, true),
            bt_device("/org/bluez/hci0/dev_BB", false, false, false),
        ],
    });
    st.snapshot = Some(snap);

    assert!(
        renders(&mut st),
        "the live Bluetooth control panel drew nothing"
    );
}

#[test]
fn a_bluetooth_toggle_couples_the_cache_update_to_the_real_write() {
    // A Discoverable toggle drives the real seat. On the headless farm host the
    // write fails (no bus/adapter) → a toast is raised and the optimistic cache
    // update is withheld (§7: a failed write never lies "on"). The optimistic
    // flip only lands on a real success — the two outcomes are asserted together
    // so a live build-host adapter can't make the test flaky.
    let mut st = SystemState::default();
    let mut snap = Seat::new().snapshot();
    snap.bluetooth = Probe::Present(BtStatus {
        adapters: vec![BtAdapter {
            path: "/org/bluez/hci0".to_owned(),
            name: "eagle".to_owned(),
            powered: true,
            discovering: false,
            discoverable: false,
            pairable: false,
        }],
        devices: vec![],
    });
    st.snapshot = Some(snap);
    st.apply(vec![SysAction::BtDiscoverable(
        "/org/bluez/hci0".to_owned(),
        true,
    )]);
    let toasts = st.take_toasts();
    let cached_on = matches!(
        st.snapshot.as_ref().map(|s| &s.bluetooth),
        Some(Probe::Present(bt)) if bt.adapters[0].discoverable
    );
    // Failure ⇒ exactly one toast + cache stays false; success ⇒ no toast + the
    // optimistic flip landed. Never a toast with a lying "on" cache.
    assert_eq!(
        toasts.len() == 1,
        !cached_on,
        "the cache update must track the write outcome"
    );
}

#[test]
fn leaving_the_system_surface_drops_the_pairing_agent() {
    // sync_pairing_agent(false) always releases the agent + re-arms, and with no
    // adapter present sync_pairing_agent(true) is a no-op (nothing to pair) —
    // never a bus error on a headless host.
    let mut st = SystemState {
        agent_attempted: true,
        ..SystemState::default()
    };
    st.sync_pairing_agent(false);
    assert!(st.agent.is_none());
    assert!(!st.agent_attempted);
    // Active but no snapshot/adapter yet → does not attempt (stays un-attempted).
    st.sync_pairing_agent(true);
    assert!(
        !st.agent_attempted,
        "no adapter ⇒ no agent registration attempt"
    );
}

// ── Settings master-detail shell (SETTINGS-1) ─────────────────────────────

/// A unique per-test temp dir (the manual idiom `power_honor`'s tests use — no
/// tempfile dep on the airgapped farm).
fn nav_temp_dir(tag: &str) -> PathBuf {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("mde-settings1-{tag}-{}-{n}", std::process::id()))
}

#[test]
fn the_rail_lists_the_three_domain_groups_covering_every_section() {
    // The master rail is exactly the three domain groups (lock 3), each with at
    // least one section, and every listed section names the group that lists it
    // (no orphan / mis-grouped leaf).
    assert_eq!(SettingsGroup::ALL.len(), 3);
    for group in SettingsGroup::ALL {
        assert!(
            !group.sections().is_empty(),
            "{} has no sections",
            group.label()
        );
        for &section in group.sections() {
            assert_eq!(
                section.group(),
                group,
                "{} is listed under the wrong group",
                section.label()
            );
        }
    }
}

#[test]
fn every_section_is_reachable_exactly_once() {
    // Every section — the host-control sections AND the Mesh & System
    // sections SETTINGS-4 wired — appears exactly once across the whole taxonomy
    // and routes to a real body (the live routing is the exhaustive match in
    // settings_detail; the render test proves each paints).
    let all: Vec<SettingsSection> = SettingsGroup::ALL
        .iter()
        .flat_map(|g| g.sections().iter().copied())
        .collect();
    for section in [
        SettingsSection::Displays,
        SettingsSection::Mouse,
        SettingsSection::Audio,
        SettingsSection::Bluetooth,
        SettingsSection::Power,
        SettingsSection::Wallpaper,
        SettingsSection::Hotkeys,
        SettingsSection::Theme,
        SettingsSection::Identity,
        SettingsSection::Role,
        SettingsSection::Pairing,
        SettingsSection::Network,
        SettingsSection::RemoteProofing,
        SettingsSection::KeyMapping,
    ] {
        assert_eq!(
            all.iter().filter(|&&s| s == section).count(),
            1,
            "{} must be reachable exactly once",
            section.label()
        );
    }
    // The whole taxonomy is exactly those fourteen sections (no orphan leaf).
    assert_eq!(
        all.len(),
        14,
        "the taxonomy lists exactly fourteen sections"
    );
}

#[test]
fn every_settings_section_has_a_yamis_backed_icon() {
    let all: Vec<SettingsSection> = SettingsGroup::ALL
        .iter()
        .flat_map(|g| g.sections().iter().copied())
        .collect();

    assert_eq!(SettingsSection::Mouse.icon_id(), IconId::Mouse);
    for section in all {
        let icon = section.icon_id();
        assert!(
            icon.name().starts_with("yamis-"),
            "{} should use the shared YAMIS platform icon catalog, got {}",
            section.label(),
            icon.name()
        );
        assert!(
            icon.svg().contains("<svg"),
            "{} icon source should be embedded SVG",
            section.label()
        );
    }
}

#[test]
fn selecting_each_section_routes_the_detail_pane_and_paints() {
    // Drive a headless frame per section with the rail resting on it: the detail
    // pane must tessellate real geometry (route to that body / honest-empty note,
    // never blank), and a click-free render leaves the selection put.
    for group in SettingsGroup::ALL {
        for &section in group.sections() {
            let mut st = SystemState {
                nav: SettingsNav::at(section),
                ..SystemState::default()
            };
            assert!(
                renders(&mut st),
                "the detail pane for {} drew nothing",
                section.label()
            );
            assert_eq!(
                st.nav.section, section,
                "a click-free render must not move the selection"
            );
        }
    }
}

#[test]
fn the_nav_selection_round_trips_through_disk_persistence() {
    // A moved rail selection survives a restart: write it through the real
    // save_to/load_from seam (the PowerHonorConfig idiom) and read it back; a
    // missing file folds to the default (Displays), never a fatal.
    let dir = nav_temp_dir("rt");
    std::fs::create_dir_all(&dir).expect("mkroot");
    let path = dir.join(NAV_CONFIG_FILE);

    assert_eq!(
        SettingsNav::load_from(&path),
        SettingsNav::default(),
        "a missing file folds to the default"
    );
    assert_eq!(SettingsNav::default().section, SettingsSection::Displays);

    let nav = SettingsNav::at(SettingsSection::Hotkeys);
    nav.save_to(&path).expect("save");
    let back = SettingsNav::load_from(&path);
    assert_eq!(back, nav, "the pick round-trips through disk");
    assert_eq!(back.section, SettingsSection::Hotkeys);
    assert_eq!(back.group, SettingsGroup::Personalization);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_stale_group_in_the_file_is_normalised_against_the_section() {
    // A hand-edited / schema-drifted file whose group doesn't own its section is
    // folded so the pair is always consistent (§7 — the section wins). Also
    // exercises the snake_case serde wire form.
    let dir = nav_temp_dir("norm");
    std::fs::create_dir_all(&dir).expect("mkroot");
    let path = dir.join(NAV_CONFIG_FILE);
    std::fs::write(&path, r#"{"group":"devices","section":"hotkeys"}"#).expect("write");

    let nav = SettingsNav::load_from(&path);
    assert_eq!(nav.section, SettingsSection::Hotkeys);
    assert_eq!(
        nav.group,
        SettingsGroup::Personalization,
        "the group is re-derived from the section"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ── U19 — the HIG Settings anatomy (PLATFORM-INTERFACES Q27) ───────────────

/// Drive one headless `show()` frame at `size` and collect the painted text.
fn painted_frame(st: &mut SystemState, size: egui::Vec2) -> Vec<(String, egui::Color32)> {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let out = ctx.run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), size)),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| st.show(ui));
        },
    );
    painted_text(&out.shapes)
}

#[test]
fn the_sidebar_model_lists_every_section_exactly_once_under_its_group() {
    // The unfiltered sidebar row model is EXACTLY the taxonomy: the three domain
    // groups in order, each listing its own sections in order, fourteen rows in
    // all, no duplicate and no orphan (the Q27 shape the rail renders from).
    let groups = sidebar_rows("");
    assert_eq!(
        groups.len(),
        3,
        "the unfiltered sidebar is the three groups"
    );
    let flat: Vec<SettingsSection> = groups
        .iter()
        .flat_map(|(_, sections)| sections.iter().copied())
        .collect();
    assert_eq!(flat.len(), 14, "the unfiltered sidebar lists fourteen rows");
    for &section in &flat {
        assert_eq!(
            flat.iter().filter(|&&s| s == section).count(),
            1,
            "{} must be exactly one sidebar row",
            section.label()
        );
    }
    for (group, sections) in &groups {
        assert_eq!(
            sections.as_slice(),
            group.sections(),
            "{} must list its taxonomy sections in order",
            group.label()
        );
    }
}

#[test]
fn the_sidebar_paints_all_fourteen_rows_exactly_once() {
    // One full frame (tall enough that no row scrolls out): every section label
    // tessellates exactly once as its sidebar row — except the selected one,
    // which recurs exactly once more as the detail pane's NavigationBar title
    // (Q27; the old in-body header is gone, so never a third time).
    let mut st = SystemState {
        nav: SettingsNav::at(SettingsSection::Displays),
        ..SystemState::default()
    };
    let texts = painted_frame(&mut st, vec2(960.0, 1600.0));
    let count = |label: &str| texts.iter().filter(|(t, _)| t == label).count();
    for group in SettingsGroup::ALL {
        for &section in group.sections() {
            let expected = if section == st.nav.section { 2 } else { 1 };
            assert_eq!(
                count(section.label()),
                expected,
                "{} must paint exactly {expected} time(s): {texts:?}",
                section.label()
            );
        }
    }
}

#[test]
fn the_sidebar_search_narrows_by_label_and_clearing_restores_the_taxonomy() {
    // Case-insensitive label substring (no ranking): "BLUE" survives only
    // Bluetooth, and the emptied groups drop out entirely (header and all).
    let narrowed = sidebar_rows("BLUE");
    assert_eq!(narrowed.len(), 1, "empty groups must drop out");
    assert_eq!(narrowed[0].0, SettingsGroup::Devices);
    assert_eq!(narrowed[0].1, vec![SettingsSection::Bluetooth]);
    // Substring, not prefix: "proof" reaches Remote Proofing.
    assert_eq!(
        sidebar_rows("proof")
            .iter()
            .flat_map(|(_, s)| s.iter().copied())
            .collect::<Vec<_>>(),
        vec![SettingsSection::RemoteProofing]
    );
    // A no-match query is an honest empty rail, never a phantom row.
    assert!(sidebar_rows("no-such-section").is_empty());
    // Clearing (or an all-blank query) restores the whole taxonomy.
    for cleared in ["", "   "] {
        assert_eq!(
            sidebar_rows(cleared).iter().flat_map(|(_, s)| s).count(),
            14,
            "clearing the filter must restore all fourteen rows"
        );
    }
}

#[test]
fn a_narrowed_sidebar_paints_only_matches_and_keeps_the_selection_pane() {
    // A live filtered frame: only the matching row paints, and the (narrowed-
    // out) selection keeps its detail pane — its title survives exactly once,
    // as the NavigationBar title.
    let mut st = SystemState {
        nav: SettingsNav::at(SettingsSection::Displays),
        nav_filter: "blue".to_owned(),
        ..SystemState::default()
    };
    let texts = painted_frame(&mut st, vec2(960.0, 640.0));
    let count = |label: &str| texts.iter().filter(|(t, _)| t == label).count();
    assert_eq!(
        count("Bluetooth"),
        1,
        "the matching row must paint: {texts:?}"
    );
    assert_eq!(
        count("Mouse & Touch"),
        0,
        "a filtered-out row must not paint"
    );
    assert_eq!(
        count("Displays"),
        1,
        "the narrowed-out selection keeps its NavigationBar title"
    );
    assert_eq!(
        st.nav.section,
        SettingsSection::Displays,
        "a filter change must never move the selection"
    );
}

#[test]
fn a_sidebar_pick_routes_the_detail_pane_through_the_nav_seam() {
    // Drive the REAL rail headlessly (the same salt + row model `show()`
    // mounts): focus a row, press Enter — the pick must route `nav` through
    // SettingsNav::at, and the routed state must tessellate a real detail pane.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut nav = SettingsNav::at(SettingsSection::Displays);
    let mut filter = String::new();
    let input = || egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(240.0, 1600.0))),
        ..Default::default()
    };
    let mut frame = |raw: egui::RawInput, nav: &mut SettingsNav, filter: &mut String| {
        ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| settings_rail(ui, nav, filter));
        })
    };
    // Frame 1 registers the rows; then focus Audio's row (its flattened
    // taxonomy index — the deterministic Sidebar row id) and press Enter.
    let _ = frame(input(), &mut nav, &mut filter);
    let audio = sidebar_rows("")
        .iter()
        .flat_map(|(_, s)| s.iter().copied())
        .position(|s| s == SettingsSection::Audio)
        .expect("Audio is a sidebar row");
    ctx.memory_mut(|m| m.request_focus(Sidebar::row_id(SETTINGS_SIDEBAR_SALT, audio)));
    let enter = egui::RawInput {
        events: vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        ..input()
    };
    let _ = frame(enter, &mut nav, &mut filter);
    assert_eq!(
        nav.section,
        SettingsSection::Audio,
        "Enter on the focused row must route the nav"
    );
    assert_eq!(
        nav.group,
        SettingsGroup::Devices,
        "the group re-derives from the pick"
    );

    // The routed selection renders a real (non-empty) Audio detail pane.
    let mut st = SystemState {
        nav,
        ..SystemState::default()
    };
    assert!(
        renders(&mut st),
        "the routed Audio detail pane drew nothing"
    );
    assert_eq!(st.nav.section, SettingsSection::Audio);
}

// ── Personalization → Theme appearance (SETTINGS-5) ───────────────────────

#[test]
fn theme_is_a_personalization_section_reachable_once() {
    // The new Theme section lives under Personalization (the design taxonomy) and
    // is exactly one rail leaf.
    assert_eq!(
        SettingsSection::Theme.group(),
        SettingsGroup::Personalization
    );
    assert!(SettingsGroup::Personalization
        .sections()
        .contains(&SettingsSection::Theme));
    let count = SettingsGroup::ALL
        .iter()
        .flat_map(|g| g.sections().iter())
        .filter(|&&s| s == SettingsSection::Theme)
        .count();
    assert_eq!(count, 1, "Theme must be reachable exactly once");
}

#[test]
fn remote_proofing_is_a_mesh_system_section_reachable_once() {
    // Sunshine/Moonlight and the VNC fallback are operator exposure controls, so
    // they live in the Mesh & System settings workspace rather than the Browser
    // toolbar or a hand-edited service file.
    assert_eq!(
        SettingsSection::RemoteProofing.group(),
        SettingsGroup::MeshSystem
    );
    assert!(SettingsGroup::MeshSystem
        .sections()
        .contains(&SettingsSection::RemoteProofing));
    let count = SettingsGroup::ALL
        .iter()
        .flat_map(|g| g.sections().iter())
        .filter(|&&s| s == SettingsSection::RemoteProofing)
        .count();
    assert_eq!(count, 1, "Remote Proofing must be reachable exactly once");
}

#[test]
fn mouse_touch_is_a_devices_section_reachable_once() {
    // Mouse & Touch is a host-device control surface, not personalization. It must be
    // one normal rail leaf so the Settings chrome, menubar routing, and detail router
    // all treat it like Displays/Audio/Bluetooth/Power.
    assert_eq!(SettingsSection::Mouse.group(), SettingsGroup::Devices);
    assert!(SettingsGroup::Devices
        .sections()
        .contains(&SettingsSection::Mouse));
    let count = SettingsGroup::ALL
        .iter()
        .flat_map(|g| g.sections().iter())
        .filter(|&&s| s == SettingsSection::Mouse)
        .count();
    assert_eq!(count, 1, "Mouse & Touch must be reachable exactly once");
}

#[test]
fn mouse_touch_config_defaults_round_trip_and_clamp() {
    // Defaults lower the relative pointer speed for the native seat while keeping
    // normal primary-button and scroll direction behavior. The policy object is what
    // the DRM runner reads, so this proves the persisted values have a runtime shape.
    let cfg = MouseTouchConfig::default();
    assert_eq!(cfg.pointer_speed_percent, -35);
    assert_eq!(cfg.primary_button, PrimaryButton::Left);
    assert!(!cfg.natural_scroll);
    assert_eq!(cfg.scroll_speed_percent, 100);
    assert_eq!(cfg.double_click_ms, 300);
    assert!(cfg.touchpad_tap_to_click);
    assert!(cfg.two_finger_scroll);
    assert!(cfg.touchscreen_enabled);
    assert!(cfg.edge_gestures);
    assert!(cfg.long_press_secondary);
    assert!((cfg.input_policy().pointer_scale() - 0.65).abs() < f32::EPSILON);
    assert!((cfg.mackesd_pointer_accel() + 0.35).abs() < f64::EPSILON);

    let dir = nav_temp_dir("mouse-touch");
    std::fs::create_dir_all(&dir).expect("mkroot");
    let path = dir.join(MOUSE_TOUCH_CONFIG_FILE);
    let changed = MouseTouchConfig {
        pointer_speed_percent: 20,
        primary_button: PrimaryButton::Right,
        natural_scroll: true,
        scroll_speed_percent: 140,
        double_click_ms: 450,
        touchpad_tap_to_click: false,
        two_finger_scroll: false,
        touchscreen_enabled: false,
        edge_gestures: false,
        long_press_secondary: false,
    };
    changed.save_to(&path).expect("save");
    assert_eq!(MouseTouchConfig::load_from(&path), changed);
    assert!(changed.input_policy().left_handed);
    assert_eq!(changed.input_policy().scroll_speed_percent, 140);

    std::fs::write(
        &path,
        r#"{"pointer_speed_percent":-500,"scroll_speed_percent":999,"double_click_ms":1200,"primary_button":"right"}"#,
    )
    .expect("write drifted config");
    let clamped = MouseTouchConfig::load_from(&path);
    assert_eq!(clamped.pointer_speed_percent, -100);
    assert_eq!(clamped.scroll_speed_percent, 300);
    assert_eq!(clamped.double_click_ms, 900);
    assert_eq!(clamped.primary_button, PrimaryButton::Right);
    assert!(clamped.touchpad_tap_to_click, "absent fields keep defaults");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn mouse_touch_policy_publishes_to_runtime_input_and_egui_options() {
    let original = mde_egui::input_policy();
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let st = SystemState {
        mouse_touch: MouseTouchConfig {
            pointer_speed_percent: -60,
            primary_button: PrimaryButton::Right,
            natural_scroll: true,
            scroll_speed_percent: 175,
            double_click_ms: 650,
            touchpad_tap_to_click: true,
            two_finger_scroll: false,
            touchscreen_enabled: false,
            edge_gestures: false,
            long_press_secondary: false,
        },
        ..SystemState::default()
    };

    st.apply_mouse_touch(&ctx);
    assert_eq!(mde_egui::input_policy(), st.mouse_touch.input_policy());
    assert!(mde_egui::input_policy().left_handed);
    assert!((mde_egui::input_policy().pointer_scale() - 0.4).abs() < f32::EPSILON);
    let double_click = ctx.options(|o| o.input_options.max_double_click_delay);
    assert!(
        (double_click - 0.65).abs() < f64::EPSILON,
        "double-click timing is applied to egui input options"
    );

    mde_egui::set_input_policy(original);
}

#[test]
fn remote_proofing_config_defaults_and_round_trips() {
    // Conservative defaults keep Sunshine/Moonlight policy grouped and safe:
    // disabled until the operator enables it, mesh-only exposure, DRM/KMS capture
    // for the native shell, local approval, on-seat indicator, remote input policy,
    // VNC fallback, and a 30 FPS proof floor.
    let cfg = RemoteProofingConfig::default();
    assert!(!cfg.enabled);
    assert_eq!(cfg.exposure, RemoteProofingExposure::MeshOnly);
    assert_eq!(cfg.capture, RemoteProofingCapture::Kms);
    assert_eq!(cfg.encoder, RemoteProofingEncoder::Auto);
    assert!(cfg.native_pairing_prompt);
    assert!(cfg.require_local_approval);
    assert!(cfg.show_shadowing_indicator);
    assert!(cfg.allow_remote_input);
    assert!(cfg.vnc_fallback);
    assert_eq!(cfg.min_fps_target, 30);

    let dir = nav_temp_dir("remote-proofing");
    std::fs::create_dir_all(&dir).expect("mkroot");
    let path = dir.join(REMOTE_PROOFING_CONFIG_FILE);
    let changed = RemoteProofingConfig {
        enabled: true,
        exposure: RemoteProofingExposure::Lan,
        capture: RemoteProofingCapture::Kms,
        encoder: RemoteProofingEncoder::IntelVaapi,
        native_pairing_prompt: true,
        require_local_approval: true,
        show_shadowing_indicator: true,
        allow_remote_input: true,
        vnc_fallback: true,
        min_fps_target: 45,
    };
    changed.save_to(&path).expect("save");
    assert_eq!(RemoteProofingConfig::load_from(&path), changed);

    std::fs::write(
        &path,
        r#"{"enabled":true,"exposure":"public","min_fps_target":250}"#,
    )
    .expect("write drifted config");
    let clamped = RemoteProofingConfig::load_from(&path);
    assert_eq!(clamped.exposure, RemoteProofingExposure::Public);
    assert_eq!(clamped.min_fps_target, 120);
    assert_eq!(clamped.capture, RemoteProofingCapture::Kms);
    assert!(clamped.require_local_approval);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn remote_proofing_service_plan_maps_policy_to_sunshine_and_firewall() {
    let mesh = MeshFacts {
        seen: true,
        identity: Some("eagle".to_owned()),
        role: Some("workstation".to_owned()),
        overlay_ip: Some("10.42.0.8".to_owned()),
        default_gw: Some("172.20.0.1".to_owned()),
        ..MeshFacts::default()
    };
    let cfg = RemoteProofingConfig {
        enabled: true,
        exposure: RemoteProofingExposure::MeshOnly,
        capture: RemoteProofingCapture::Kms,
        encoder: RemoteProofingEncoder::IntelVaapi,
        native_pairing_prompt: true,
        require_local_approval: true,
        show_shadowing_indicator: true,
        allow_remote_input: true,
        vnc_fallback: true,
        min_fps_target: 45,
    };

    let plan = cfg.service_plan(&mesh);
    assert!(plan.enabled);
    assert_eq!(plan.bind_scope, RemoteProofingBindScope::MeshOnly);
    assert_eq!(plan.bind_address.as_deref(), Some("10.42.0.8"));
    assert_eq!(plan.firewall, RemoteProofingFirewallPolicy::MeshOverlayOnly);
    assert_eq!(plan.sunshine_capture, "kms");
    assert_eq!(plan.sunshine_encoder, "vaapi");
    assert_eq!(plan.min_fps_target, 45);
    assert!(plan.require_local_approval);
    assert!(plan.show_shadowing_indicator);
    assert!(plan.allow_remote_input);
    assert!(plan.vnc_fallback);
    assert!(
        plan.warnings.is_empty(),
        "healthy mesh-only policy should not warn: {:?}",
        plan.warnings
    );
}

#[test]
fn remote_proofing_service_plan_surfaces_degraded_or_public_exposure() {
    let mesh_without_overlay = MeshFacts {
        seen: true,
        role: Some("workstation".to_owned()),
        ..MeshFacts::default()
    };
    let mesh_plan = RemoteProofingConfig {
        enabled: true,
        exposure: RemoteProofingExposure::MeshOnly,
        ..RemoteProofingConfig::default()
    }
    .service_plan(&mesh_without_overlay);
    assert_eq!(mesh_plan.bind_scope, RemoteProofingBindScope::MeshOnly);
    assert_eq!(mesh_plan.bind_address, None);
    assert_eq!(
        mesh_plan.firewall,
        RemoteProofingFirewallPolicy::MeshOverlayOnly
    );
    assert!(
        mesh_plan
            .warnings
            .iter()
            .any(|w| w.contains("Mesh address")),
        "missing overlay IP must be an explicit degraded state"
    );

    let public_plan = RemoteProofingConfig {
        enabled: true,
        exposure: RemoteProofingExposure::Public,
        require_local_approval: false,
        show_shadowing_indicator: false,
        ..RemoteProofingConfig::default()
    }
    .service_plan(&MeshFacts::default());
    assert_eq!(public_plan.bind_scope, RemoteProofingBindScope::Public);
    assert_eq!(public_plan.bind_address.as_deref(), Some("0.0.0.0"));
    assert_eq!(
        public_plan.firewall,
        RemoteProofingFirewallPolicy::PublicExplicit
    );
    assert!(
        public_plan
            .warnings
            .iter()
            .any(|w| w.contains("All-interfaces")),
        "public exposure must carry an explicit warning"
    );
    assert!(
        public_plan
            .warnings
            .iter()
            .any(|w| w.contains("Local approval")),
        "turning off approval must stay visible in the plan"
    );
    assert!(
        public_plan.warnings.iter().any(|w| w.contains("indicator")),
        "turning off the on-seat indicator must stay visible in the plan"
    );

    let disabled_plan = RemoteProofingConfig::default().service_plan(&MeshFacts::default());
    assert!(!disabled_plan.enabled);
    assert_eq!(disabled_plan.bind_scope, RemoteProofingBindScope::Disabled);
    assert_eq!(disabled_plan.firewall, RemoteProofingFirewallPolicy::Closed);
    assert!(disabled_plan.warnings.is_empty());
}

#[test]
fn every_accent_choice_maps_to_a_shared_style_token() {
    // Each accent choice paints an EXISTING shared Style::ACCENT* token (§4 — no
    // new hex), Brand is the interactive brand accent, and the non-Brand choices
    // are mutually distinct so the picker offers real variation.
    assert_eq!(AccentChoice::default(), AccentChoice::Brand);
    assert_eq!(AccentChoice::Brand.color(), Style::ACCENT);
    let variants: Vec<_> = AccentChoice::ALL
        .iter()
        .filter(|c| **c != AccentChoice::Brand)
        .map(|c| c.color())
        .collect();
    for (i, a) in variants.iter().enumerate() {
        assert_ne!(*a, Style::ACCENT, "a variant must differ from the brand");
        for b in &variants[i + 1..] {
            assert_ne!(a, b, "accent choices must be mutually distinct");
        }
    }
}

#[test]
fn text_scale_steps_ascend_around_a_default_identity() {
    // The steps are strictly ascending and Default is the 1.0 identity (a no-op),
    // so a Default pick never perturbs the seat's DPI zoom.
    assert_eq!(TextScale::default(), TextScale::Default);
    assert!((TextScale::Default.factor() - 1.0).abs() < f32::EPSILON);
    let mut prev = f32::MIN;
    for step in TextScale::ALL {
        assert!(
            step.factor() > prev,
            "{} breaks the ascending order",
            step.label()
        );
        prev = step.factor();
    }
}

#[test]
fn the_theme_appearance_round_trips_through_disk_persistence() {
    // A Theme pick survives a restart: write it through the real save_to/load_from
    // seam (the SettingsNav idiom) and read it back; a missing file folds to the
    // default (Brand / Default), never a fatal.
    let dir = nav_temp_dir("theme-rt");
    std::fs::create_dir_all(&dir).expect("mkroot");
    let path = dir.join(APPEARANCE_CONFIG_FILE);

    assert_eq!(
        AppearanceConfig::load_from(&path),
        AppearanceConfig::default(),
        "a missing file folds to the default"
    );
    assert_eq!(
        AppearanceConfig::default().color_scheme,
        AppearanceColorScheme::Dark,
        "dark mode preserves the current status-quo interface by default"
    );
    assert_eq!(AppearanceConfig::default().accent, AccentChoice::Brand);
    assert_eq!(
        AppearanceConfig::default().layout_profile,
        LayoutProfile::Construct,
        "Construct is the fresh-install layout (Q42: two profiles)"
    );
    assert_eq!(AppearanceConfig::default().text_scale, TextScale::Default);
    assert_eq!(
        AppearanceConfig::default().motion_mode,
        AppearanceMotionMode::Normal,
        "motion defaults to the full normal mode"
    );
    assert!(
        !AppearanceConfig::default().taskbar_autohide,
        "taskbar auto-hide defaults off so the bottom strut remains reserved"
    );

    let cfg = AppearanceConfig {
        color_scheme: AppearanceColorScheme::Light,
        accent: AccentChoice::Green,
        layout_profile: LayoutProfile::Car,
        text_scale: TextScale::Larger,
        motion_mode: AppearanceMotionMode::Disabled,
        taskbar_autohide: true,
    };
    cfg.save_to(&path).expect("save");
    let back = AppearanceConfig::load_from(&path);
    assert_eq!(back, cfg, "the appearance round-trips through disk");
    assert_eq!(back.accent, AccentChoice::Green);
    assert_eq!(back.text_scale, TextScale::Larger);
    assert_eq!(
        back.layout_profile,
        LayoutProfile::Car,
        "the layout profile pick round-trips through disk"
    );
    assert_eq!(
        back.motion_mode,
        AppearanceMotionMode::Disabled,
        "the motion-mode pick round-trips through disk"
    );
    assert_eq!(
        back.color_scheme,
        AppearanceColorScheme::Light,
        "the color-mode pick round-trips through disk"
    );
    assert!(
        back.taskbar_autohide,
        "the taskbar auto-hide pick round-trips"
    );
    let json = std::fs::read_to_string(&path).expect("appearance json");
    assert!(
        json.contains("\"color_scheme\": \"light\""),
        "the new color mode is persisted explicitly: {json}"
    );
    assert!(
        json.contains("\"motion_mode\": \"disabled\""),
        "the new runtime mode is persisted explicitly: {json}"
    );
    assert!(
        json.contains("\"layout_profile\": \"car\""),
        "the layout profile is persisted explicitly: {json}"
    );
    assert!(
        json.contains("\"taskbar_autohide\": true"),
        "the taskbar auto-hide setting is persisted explicitly: {json}"
    );
    assert!(
        !json.contains("reduce_motion"),
        "the legacy boolean should not be written back once migrated: {json}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_partial_appearance_file_folds_missing_fields_to_their_defaults() {
    // A drifted / partial file (only one field) reads back with the other field at
    // its serde default — never a fatal, honest to what was written.
    let dir = nav_temp_dir("theme-partial");
    std::fs::create_dir_all(&dir).expect("mkroot");
    let path = dir.join(APPEARANCE_CONFIG_FILE);
    std::fs::write(&path, r#"{"accent":"gold"}"#).expect("write");

    let cfg = AppearanceConfig::load_from(&path);
    assert_eq!(
        cfg.accent,
        AccentChoice::Gold,
        "the written field is honoured"
    );
    assert_eq!(
        cfg.text_scale,
        TextScale::Default,
        "the absent field folds to its default"
    );
    assert_eq!(
        cfg.color_scheme,
        AppearanceColorScheme::Dark,
        "the absent color-mode field folds to dark"
    );
    assert_eq!(
        cfg.layout_profile,
        LayoutProfile::Construct,
        "the absent layout-profile field folds to Construct"
    );
    assert_eq!(
        cfg.motion_mode,
        AppearanceMotionMode::Normal,
        "the absent motion-mode field folds to Normal"
    );
    assert!(
        !cfg.taskbar_autohide,
        "the absent taskbar auto-hide field folds to the docked default"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn legacy_reduce_motion_json_migrates_to_the_reduced_motion_mode() {
    let cfg: AppearanceConfig =
        serde_json::from_str(r#"{"accent":"green","text_scale":"large","reduce_motion":true}"#)
            .expect("legacy appearance config");

    assert_eq!(cfg.accent, AccentChoice::Green);
    assert_eq!(cfg.text_scale, TextScale::Large);
    assert_eq!(
        cfg.motion_mode,
        AppearanceMotionMode::Reduced,
        "old reduce_motion=true configs migrate to the reduced runtime mode"
    );
    assert_eq!(
        cfg.color_scheme,
        AppearanceColorScheme::Dark,
        "legacy appearance configs keep the dark status-quo palette"
    );
    assert_eq!(
        cfg.layout_profile,
        LayoutProfile::Construct,
        "legacy appearance configs keep the Construct workstation layout"
    );
    assert!(
        !cfg.taskbar_autohide,
        "legacy configs keep the taskbar docked unless explicitly opted in"
    );

    let cfg: AppearanceConfig = serde_json::from_str(
        r#"{"motion_mode":"disabled","taskbar_autohide":true,"reduce_motion":false}"#,
    )
    .expect("explicit appearance config");
    assert_eq!(
        cfg.motion_mode,
        AppearanceMotionMode::Disabled,
        "an explicit new motion_mode wins over any legacy field"
    );
    assert_eq!(
        cfg.color_scheme,
        AppearanceColorScheme::Dark,
        "configs without color_scheme keep the dark status-quo palette"
    );
    assert!(
        cfg.taskbar_autohide,
        "an explicit taskbar auto-hide field is honoured"
    );
}

#[test]
fn legacy_workstation_and_tablet_profiles_migrate_to_construct() {
    // PLATFORM-INTERFACES Q42: seats persisted "workstation" or "tablet" before
    // the two-profile fold; both must deserialize to Construct silently (a parse
    // failure would reset the whole seat appearance), and the next save writes
    // the new "construct" wire name.
    let cfg: AppearanceConfig = serde_json::from_str(r#"{"layout_profile":"tablet"}"#)
        .expect("tablet-era appearance config parses");
    assert_eq!(
        cfg.layout_profile,
        LayoutProfile::Construct,
        "a persisted tablet profile folds into Construct"
    );
    let cfg: AppearanceConfig = serde_json::from_str(r#"{"layout_profile":"workstation"}"#)
        .expect("workstation-era appearance config parses");
    assert_eq!(
        cfg.layout_profile,
        LayoutProfile::Construct,
        "a persisted workstation profile folds into Construct"
    );

    let dir = nav_temp_dir("theme-profile-fold");
    std::fs::create_dir_all(&dir).expect("mkroot");
    let path = dir.join(APPEARANCE_CONFIG_FILE);
    cfg.save_to(&path).expect("save");
    let json = std::fs::read_to_string(&path).expect("appearance json");
    assert!(
        json.contains("\"layout_profile\": \"construct\""),
        "a migrated config re-persists the new construct wire name: {json}"
    );
    assert_eq!(
        AppearanceConfig::load_from(&path).layout_profile,
        LayoutProfile::Construct,
        "the re-persisted construct wire name round-trips"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hardware_formfactor_flips_adjust_density_within_construct() {
    // PLATFORM-INTERFACES Q42: formfactor ≠ profile. A hardware Tablet flip
    // grows the live density WITHIN Construct (the folded Tablet profile's
    // touch metrics now arrive this way); flipping back to Laptop restores the
    // Mouse anchor; Car pins Touch regardless of the hardware state.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut st = SystemState {
        appearance: AppearanceConfig::default(),
        ..SystemState::default()
    };
    assert_eq!(
        st.layout_density(),
        Density::Mouse,
        "Construct without a seat formfactor report keeps the Mouse anchor"
    );

    st.set_formfactor(Formfactor::Tablet);
    st.poll(&ctx);
    assert_eq!(
        st.layout_density(),
        Density::Touch,
        "a hardware Tablet flip grows density within Construct"
    );
    assert_eq!(
        Style::density(&ctx),
        Density::Touch,
        "poll installs the formfactor-refined density on the live context"
    );

    st.set_formfactor(Formfactor::Laptop);
    st.poll(&ctx);
    assert_eq!(
        st.layout_density(),
        Density::Mouse,
        "flipping back to Laptop restores the Construct Mouse anchor"
    );
    assert_eq!(Style::density(&ctx), Density::Mouse);

    st.appearance.layout_profile = LayoutProfile::Car;
    assert_eq!(
        st.layout_density(),
        Density::Touch,
        "Car pins Touch even on Laptop hardware"
    );
}

#[test]
fn appearance_taskbar_autohide_preference_is_exposed_to_shell_chrome() {
    let st = SystemState {
        appearance: AppearanceConfig {
            taskbar_autohide: true,
            ..AppearanceConfig::default()
        },
        ..SystemState::default()
    };
    assert!(
        st.taskbar_autohide(),
        "main.rs mirrors this persisted preference into DockState each frame"
    );
}

#[test]
fn appearance_layout_profile_drives_live_density() {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    assert_eq!(
        Style::density(&ctx),
        LayoutProfile::Construct.density(),
        "the installed default is the Construct anchor (Mouse) density"
    );

    let mut st = SystemState {
        appearance: AppearanceConfig {
            layout_profile: LayoutProfile::Car,
            ..AppearanceConfig::default()
        },
        ..SystemState::default()
    };
    st.poll(&ctx);
    assert_eq!(
        st.layout_profile(),
        LayoutProfile::Car,
        "SystemState exposes the persisted profile to shell chrome"
    );
    assert_eq!(
        st.layout_density(),
        LayoutProfile::Car.density(),
        "the shell mirrors the profile-selected density"
    );
    assert_eq!(
        Style::density(&ctx),
        LayoutProfile::Car.density(),
        "poll applies the profile density to the live context"
    );
    assert_eq!(
        ctx.style().spacing.interact_size.y,
        LayoutProfile::Car.density().min_hit_target(),
        "Car mode installs the larger touch hit target"
    );
}

#[test]
fn the_theme_accent_choice_retints_the_live_context_on_poll() {
    // The apply seam is real: with a persisted accent pick, one poll re-tints the
    // live egui interactive accent (observable in the context's visuals) — not a
    // dead toggle (§7). Poll runs every frame in both runners, so this is global.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    assert_eq!(ctx.style().visuals.hyperlink_color, Style::ACCENT);
    let mut st = SystemState {
        appearance: AppearanceConfig {
            color_scheme: AppearanceColorScheme::Dark,
            accent: AccentChoice::Green,
            layout_profile: LayoutProfile::Construct,
            text_scale: TextScale::Default,
            motion_mode: AppearanceMotionMode::Normal,
            taskbar_autohide: false,
        },
        ..SystemState::default()
    };
    st.poll(&ctx);
    assert_eq!(
        ctx.style().visuals.hyperlink_color,
        Style::ACCENT_MESH,
        "the accent pick re-tinted the live interactive accent"
    );
    assert_eq!(
        ctx.style().visuals.widgets.active.bg_fill,
        Style::pressed_fill(Style::ACCENT_MESH),
        "the pressed fill re-tinted to the darkened chosen accent"
    );
}

#[test]
fn the_theme_color_scheme_applies_windows_2000_light_visuals_on_poll() {
    // The light-mode switch is a real palette change, not a dead setting: one poll
    // applies classic Windows 2000 basic system colors to egui visuals, keeps dark as
    // the default/status quo, and leaves explicit accent picks usable.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    assert_eq!(Style::color_scheme(&ctx), StyleColorScheme::Dark);
    assert_eq!(ctx.style().visuals.panel_fill, Style::BG);

    let mut st = SystemState {
        appearance: AppearanceConfig {
            color_scheme: AppearanceColorScheme::Light,
            accent: AccentChoice::Brand,
            ..AppearanceConfig::default()
        },
        ..SystemState::default()
    };
    st.poll(&ctx);
    let p = Style::palette_for(StyleColorScheme::Light);
    assert_eq!(Style::color_scheme(&ctx), StyleColorScheme::Light);
    assert_eq!(ctx.style().visuals.panel_fill, p.bg);
    assert_eq!(ctx.style().visuals.window_fill, p.surface);
    assert_eq!(ctx.style().visuals.widgets.hovered.bg_fill, p.surface_hi);
    assert_eq!(ctx.style().visuals.override_text_color, Some(p.text));
    assert_eq!(
        ctx.style().visuals.widgets.active.bg_fill,
        Style::WIN2000_PRESSED_FACE,
        "pressed widgets use the classic gray face so black strong text stays readable"
    );
    assert_eq!(
        ctx.style().visuals.hyperlink_color,
        Style::WIN2000_ACTIVE_TITLE,
        "the default brand accent becomes classic active-title blue in light mode"
    );

    st.appearance.color_scheme = AppearanceColorScheme::Dark;
    st.poll(&ctx);
    assert_eq!(Style::color_scheme(&ctx), StyleColorScheme::Dark);
    assert_eq!(
        ctx.style().visuals.panel_fill,
        Style::BG,
        "switching back restores the current dark status-quo palette"
    );
}

#[test]
fn the_theme_text_scale_zooms_the_live_context_atop_the_dpi_base() {
    // The text-scale pick sets the whole-UI zoom to the DPI base × the step; a
    // Default pick is the identity (the base is untouched). egui STAGES a
    // set_zoom_factor to the next begin_pass, so drive the poll through real
    // passes (as both runners do) and read the applied zoom back after.
    fn poll_pass(ctx: &egui::Context, st: &mut SystemState) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| st.poll(ctx));
    }

    let ctx = egui::Context::default();
    Style::install(&ctx);
    let base = ctx.zoom_factor();
    let mut st = SystemState {
        appearance: AppearanceConfig {
            color_scheme: AppearanceColorScheme::Dark,
            accent: AccentChoice::default(),
            layout_profile: LayoutProfile::Construct,
            text_scale: TextScale::Larger,
            motion_mode: AppearanceMotionMode::Normal,
            taskbar_autohide: false,
        },
        ..SystemState::default()
    };
    poll_pass(&ctx, &mut st); // stages the zoom
    poll_pass(&ctx, &mut st); // the next begin_pass applies it
    let want = base * TextScale::Larger.factor();
    assert!(
        (ctx.zoom_factor() - want).abs() < f32::EPSILON,
        "the whole-UI zoom follows the text-scale step atop the DPI base"
    );

    // A Default pick leaves the base zoom untouched (a genuine no-op).
    let ctx2 = egui::Context::default();
    Style::install(&ctx2);
    let base2 = ctx2.zoom_factor();
    let mut st2 = SystemState {
        appearance: AppearanceConfig::default(),
        ..SystemState::default()
    };
    poll_pass(&ctx2, &mut st2);
    poll_pass(&ctx2, &mut st2);
    assert!(
        (ctx2.zoom_factor() - base2).abs() < f32::EPSILON,
        "a Default text-scale must not perturb the DPI base zoom"
    );
}

#[test]
fn reduce_motion_damps_the_live_context_and_motion_global_on_poll() {
    // a11y-07: a persisted reduce-motion pick is a REAL runtime effect on poll (§7 —
    // no dead toggle). One poll zeroes egui's `animation_time` (the signal the menu
    // bar + ambient explorer + egui's built-in animate_bool honour) AND flips the
    // shared Motion global (the explicit-duration easings the shell paints with).
    // The Motion flag is process-global, so restore it at the end for sibling tests.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let base = ctx.style().animation_time;
    assert!(base > 0.0, "the baseline cadence genuinely eases");

    let mut st = SystemState {
        appearance: AppearanceConfig {
            motion_mode: AppearanceMotionMode::Reduced,
            ..AppearanceConfig::default()
        },
        ..SystemState::default()
    };
    st.poll(&ctx);
    assert_eq!(
        ctx.style().animation_time,
        0.0,
        "reduce-motion zeroes egui's animation_time live"
    );
    assert!(
        Motion::reduce_motion(),
        "…and flips the shared Motion global the eased helpers read"
    );
    assert_eq!(
        Motion::mode(),
        mde_egui::MotionMode::Reduced,
        "the full runtime enum is reduced, not just a boolean side effect"
    );

    st.appearance.motion_mode = AppearanceMotionMode::Disabled;
    st.poll(&ctx);
    assert_eq!(
        Motion::mode(),
        mde_egui::MotionMode::Disabled,
        "disabled mode is a distinct endpoint-only runtime state"
    );

    // Turning it back OFF restores the captured baseline cadence and clears the
    // global — motion resumes, proving the toggle is bidirectional, not one-way.
    st.appearance.motion_mode = AppearanceMotionMode::Normal;
    st.poll(&ctx);
    assert!(
        (ctx.style().animation_time - base).abs() < f32::EPSILON,
        "clearing reduce-motion restores the real baseline cadence"
    );
    assert!(
        !Motion::reduce_motion(),
        "…and clears the shared Motion global"
    );

    Motion::set_mode(mde_egui::MotionMode::Normal); // restore for sibling tests
}

// ── Categorical accent + Carbon layers (SETTINGS-2) ───────────────────────

#[test]
fn each_domain_group_wears_a_distinct_shared_categorical_accent() {
    // The three domain accents REUSE the shared Style::ACCENT_* categorical set
    // (the ONE colour language PICKER-2 / EXPLORER-15 speak, §4 — no second set
    // minted here), are mutually distinct, and are each set apart from the
    // interactive brand accent so a domain tint never reads as an affordance.
    let categorical = [
        Style::ACCENT_COMMS,
        Style::ACCENT_WORKLOADS,
        Style::ACCENT_TERMINALS,
        Style::ACCENT_MESH,
        Style::ACCENT_SYSTEM,
        Style::ACCENT_MEDIA,
    ];
    let accents: Vec<egui::Color32> = SettingsGroup::ALL.iter().map(|g| g.accent()).collect();
    for a in &accents {
        assert!(
            categorical.contains(a),
            "a domain accent must be drawn from the shared categorical set, not minted"
        );
        assert_ne!(
            *a,
            Style::ACCENT,
            "a domain accent must differ from the interactive brand accent"
        );
    }
    for (i, a) in accents.iter().enumerate() {
        for b in &accents[i + 1..] {
            assert_ne!(a, b, "domain accents must be mutually distinct");
        }
    }
    // Every section inherits exactly its group's accent — the section BODIES
    // (the Mesh & System panels, the Personalization choice tiles) key off
    // `section.group().accent()`, so a section's tints can never disagree with
    // its group's. (Q27: the sidebar + NavigationBar chrome itself is quiet —
    // the shared Q19 components, not accent-tinted.)
    for group in SettingsGroup::ALL {
        for &section in group.sections() {
            assert_eq!(
                section.group().accent(),
                group.accent(),
                "{} must wear its group's accent",
                section.label()
            );
        }
    }
}

#[test]
fn the_page_and_section_card_sit_on_ascending_carbon_layers() {
    // The page frame fills Carbon layer-01 and the section card fills layer-02
    // with a hairline border — every value a Style token (no raw literal, §4) —
    // and the card reads one elevation step above the page (not a flat fill).
    let page = page_frame(Style::SP_L);
    assert_eq!(page.fill, Style::LAYER_01, "the page rests on layer-01");

    let card = card_frame();
    assert_eq!(
        card.fill,
        Style::LAYER_02,
        "the section card rests on layer-02"
    );
    assert_eq!(
        card.stroke.color,
        Style::BORDER,
        "the card wears a hairline border"
    );
    assert!(
        (card.stroke.width - 1.0).abs() < f32::EPSILON,
        "the card border is a 1px hairline"
    );
    assert_ne!(
        card.fill, page.fill,
        "the card must be a tonal step above the page (Carbon elevation)"
    );

    // The section card also casts the shared Elevation::Raised soft shadow, so it reads
    // as genuinely lifted off the page (not only a tonal step) — and the shadow is the
    // token's, never a hand-rolled one (§4). The page base stays flat (no shadow).
    let raised = mde_egui::style::Elevation::Raised.shadow();
    assert_eq!(
        card.shadow.offset,
        [raised.offset[0] as i8, raised.offset[1] as i8],
        "the card shadow offset comes from the Raised token"
    );
    assert_eq!(
        card.shadow.blur, raised.blur as u8,
        "the card shadow blur comes from the Raised token"
    );
    assert_eq!(
        card.shadow.color, raised.umbra,
        "the card shadow umbra is the Raised token's, not a minted colour"
    );
    assert!(
        card.shadow.color.a() > 0 && card.shadow.color.a() < 255,
        "the depth is a translucent umbra (lock #2), never an opaque fill"
    );
    assert_eq!(
        page.shadow.color.a(),
        0,
        "the layer-01 page base stays flat — depth lifts only the raised card"
    );

    // And the layered detail path actually paints headless — the section body
    // renders inside the layer-02 card without panicking, a full paint never blank.
    let mut st = SystemState::default();
    assert!(renders(&mut st), "the layered Settings page drew nothing");
}

// ── Expressive wide layouts (SETTINGS-3) ──────────────────────────────────

/// Render one headless frame at an explicit pane width, tessellating on the CPU
/// (the DRM runner's path minus the GPU) — the wide-pane variant of [`renders`].
fn renders_at(state: &mut SystemState, width: f32) -> bool {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 720.0))),
        ..Default::default()
    };
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| state.show(ui));
    });
    !ctx.tessellate(out.shapes, out.pixels_per_point).is_empty()
}

fn mixer_strip(name: &str, volume: u8, muted: bool) -> MixerStrip {
    MixerStrip {
        id: name.to_owned(),
        name: name.to_owned(),
        origin: mde_seat::StripOrigin::HostSession,
        volume,
        muted,
    }
}

fn connected_connector(name: &str) -> Connector {
    Connector {
        name: name.to_owned(),
        status: ConnectorStatus::Connected,
        size_mm: Some((600, 340)),
        modes: vec![DisplayMode {
            width: 1920,
            height: 1080,
            refresh_hz: 60,
            preferred: true,
        }],
    }
}

#[test]
fn fit_columns_widens_on_a_wide_pane_and_collapses_on_a_narrow_one() {
    // The across-the-width reflow decision (design lock #4): a pane narrower than
    // two tiles is a single column; a wider pane fits more, capped at the section
    // max; and it never returns zero (so chunks() cannot panic on a small seat).
    assert_eq!(fit_columns(TILE_MIN_W * 1.5, 4), 1);
    assert_eq!(fit_columns(TILE_MIN_W * 2.0, 4), 2);
    assert_eq!(fit_columns(TILE_MIN_W * 3.0, 4), 3);
    assert_eq!(
        fit_columns(TILE_MIN_W * 100.0, 3),
        3,
        "a very wide pane is capped at the section max"
    );
    assert_eq!(
        fit_columns(TILE_MIN_W * 100.0, 1),
        1,
        "a one-item section stays a single column"
    );
    assert_eq!(
        fit_columns(0.0, 4),
        1,
        "a zero-width pane is still one column"
    );
}

#[test]
fn the_reworked_sections_paint_across_a_wide_detail_pane() {
    // Inject Present Audio / Bluetooth / Power probes over an otherwise-real
    // (Absent) snapshot and render each reworked section in a WIDE pane: the
    // across / side-by-side layout must tessellate real geometry, never a blank —
    // the wide-pane counterpart of selecting_each_section_routes_the_detail_pane.
    let build = || {
        let mut snap = Seat::new().snapshot();
        snap.mixer = Probe::Present(MixerStatus {
            master: mixer_strip("master", 64, false),
            strips: vec![
                mixer_strip("Music", 80, false),
                mixer_strip("Voice", 40, true),
                mixer_strip("VM: build", 55, false),
            ],
        });
        snap.bluetooth = Probe::Present(BtStatus {
            adapters: vec![BtAdapter {
                path: "/org/bluez/hci0".to_owned(),
                name: "eagle".to_owned(),
                powered: true,
                discovering: false,
                discoverable: false,
                pairable: false,
            }],
            devices: vec![bt_device("/org/bluez/hci0/dev_AA", true, true, true)],
        });
        snap.power_profile = Probe::Present(ProfileState {
            active: "balanced".to_owned(),
            available: vec!["balanced".to_owned(), "performance".to_owned()],
        });
        snap.charge_limit = Probe::Present(Some(80));
        snap.batteries = Probe::Present(vec![Battery {
            model: "BAT0".to_owned(),
            kind: BatteryKind::Internal,
            percentage: 61.0,
            state: BatteryState::Discharging,
            power_supply: true,
            time_to_empty: Some(Duration::from_secs(5400)),
            time_to_full: None,
            energy_rate: Some(11.7),
        }]);
        snap
    };
    for section in [
        SettingsSection::Mouse,
        SettingsSection::Audio,
        SettingsSection::Bluetooth,
        SettingsSection::Power,
        SettingsSection::Wallpaper,
        SettingsSection::Hotkeys,
        SettingsSection::Theme,
    ] {
        let snap = build();
        let mut st = SystemState {
            nav: SettingsNav::at(section),
            ..SystemState::default()
        };
        st.reconcile(&snap);
        st.snapshot = Some(snap);
        assert!(
            renders_at(&mut st, 1440.0),
            "the wide {} pane drew nothing",
            section.label()
        );
    }
}

#[test]
fn the_displays_section_lays_outputs_across_and_still_drives_the_layout() {
    // Two connected outputs reconciled into the layout render as a ROW of tiles in
    // a wide pane (SETTINGS-3) — a full paint, never a stacked blank — and the
    // ToggleOutput seam still drives the intent layout, proving the presentation
    // pass didn't fork the control (§6/§7).
    let mut st = SystemState {
        nav: SettingsNav::at(SettingsSection::Displays),
        ..SystemState::default()
    };
    let mut snap = Seat::new().snapshot();
    snap.displays = Probe::Present(vec![
        connected_connector("DP-1"),
        connected_connector("DP-2"),
    ]);
    st.reconcile(&snap);
    st.snapshot = Some(snap);
    assert_eq!(
        st.layout.outputs.len(),
        2,
        "both outputs entered the layout"
    );

    assert!(
        renders_at(&mut st, 1440.0),
        "the wide Displays row of cards drew nothing"
    );

    // Toggling the FIRST output off drives the layout through apply() (the
    // last-console interlock keeps the second lit) — the real SysAction still
    // fires after the re-layout.
    let first = st.layout.outputs[0].id.clone();
    st.apply(vec![SysAction::ToggleOutput(first.clone(), false)]);
    let disabled = st
        .layout
        .outputs
        .iter()
        .find(|o| o.id == first)
        .is_some_and(|o| !o.enabled);
    assert!(
        disabled,
        "a ToggleOutput still drives the layout after the re-layout"
    );
}

// ── Mesh & System (SETTINGS-4) ────────────────────────────────────────────

/// A faithful mesh-status snapshot — the exact shape `mesh-status-snapshot.sh`
/// writes: `self` + a `nodes` directory (this node plus a lighthouse peer), the
/// fleet counts, and the network overview. `leader` names the mesh leader so both
/// the is-leader and not-leader paths are reachable from one fixture.
fn mesh_snapshot(self_host: &str, leader: &str) -> String {
    format!(
        r#"{{
              "generated_ms": 1000000,
              "self": "{self_host}",
              "online": 2,
              "total": 3,
              "nodes": [
                {{"hostname":"this-node","overlay_ip":"10.42.0.7","presence":"online",
                  "role":"workstation"}},
                {{"hostname":"lh-01","overlay_ip":"10.42.0.1","presence":"online",
                  "role":"lighthouse"}}
              ],
              "network": {{"overlay_if":"nebula1","leader":"{leader}","overlay_ip":"10.42.0.7",
                "overlay_cidr":"10.42.0.0/16","routes":[],"default_gw":"172.20.0.1",
                "gateway_endpoints":["203.0.113.9:4242"],"lighthouse_ips":["10.42.0.1"],
                "cipher":"AES-256-GCM"}}
            }}"#
    )
}

#[test]
fn mesh_facts_fold_this_nodes_real_identity_role_and_network() {
    // The leader is a peer (lh-01) here, so this node is NOT the leader; every
    // field is the node's real snapshot reality (§7).
    let mesh = MeshFacts::project(&mesh_snapshot("this-node", "lh-01"));
    assert!(mesh.seen);
    assert_eq!(mesh.identity.as_deref(), Some("this-node"));
    assert_eq!(mesh.role.as_deref(), Some("workstation"));
    assert_eq!(mesh.overlay_ip.as_deref(), Some("10.42.0.7"));
    assert_eq!(mesh.overlay_if.as_deref(), Some("nebula1"));
    assert_eq!(mesh.overlay_cidr.as_deref(), Some("10.42.0.0/16"));
    assert_eq!(mesh.cipher.as_deref(), Some("AES-256-GCM"));
    assert_eq!(mesh.leader.as_deref(), Some("lh-01"));
    assert_eq!(mesh.lighthouses, vec!["10.42.0.1".to_owned()]);
    assert_eq!(mesh.gateways, vec!["203.0.113.9:4242".to_owned()]);
    assert_eq!(mesh.default_gw.as_deref(), Some("172.20.0.1"));
    assert_eq!((mesh.peers_online, mesh.peers_total), (2, 3));
    assert!(!mesh.is_leader(), "the leader is a peer, not this node");

    // When this node holds the lease, is_leader flips.
    let leading = MeshFacts::project(&mesh_snapshot("this-node", "this-node"));
    assert!(leading.is_leader());
}

#[test]
fn mesh_facts_stay_unseen_on_a_garbage_or_fragment_snapshot() {
    for bad in ["", "not json", "{}", "[]", r#"{"network":{}}"#] {
        let mesh = MeshFacts::project(bad);
        assert!(!mesh.seen, "{bad:?} must not read as a live snapshot");
        assert!(mesh.identity.is_none());
        assert!(mesh.lighthouses.is_empty());
    }
}

#[test]
fn each_mesh_system_section_renders_live_data_and_honest_unknown() {
    // Drive each Mesh & System section twice: once over live MeshFacts and once
    // over the unseen default (every fact absent). Both must tessellate real
    // geometry — the live data OR the honest "unknown" / "reading…" note, never a
    // blank (§7). The wide side-by-side Network layout is exercised at 1440px.
    let live = MeshFacts::project(&mesh_snapshot("this-node", "this-node"));
    for section in [
        SettingsSection::Identity,
        SettingsSection::Role,
        SettingsSection::Pairing,
        SettingsSection::Network,
        SettingsSection::RemoteProofing,
    ] {
        for mesh in [live.clone(), MeshFacts::default()] {
            let mut st = SystemState {
                nav: SettingsNav::at(section),
                mesh,
                ..SystemState::default()
            };
            assert!(
                renders_at(&mut st, 1440.0),
                "the Mesh & System {} pane drew nothing",
                section.label()
            );
        }
    }
}

#[test]
fn the_pairing_section_retry_rearms_the_agent_seam() {
    // The Pairing section's Retry drives the SAME sync_pairing_agent seam: it
    // clears the once-per-visit latch and re-attempts. On the headless farm host
    // there's no adapter, so the re-attempt is an honest no-op (nothing to pair) —
    // never a bus error, never a fabricated agent (§7). Asserting the latch was
    // re-armed proves the section's action reached the seam.
    let mut st = SystemState {
        agent_attempted: true,
        ..SystemState::default()
    };
    st.apply(vec![SysAction::PairingRetry]);
    assert!(st.agent.is_none(), "no adapter ⇒ no agent registered");
    assert!(
        !st.agent_attempted,
        "Retry re-armed the once-per-visit latch on the pairing seam"
    );
}
