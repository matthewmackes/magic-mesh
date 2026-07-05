//! MENUBAR-ALL (Voice) — the **shared top menu bar** for the Voice/SIP surface
//! (design: `docs/design/menubar-all.md`).
//!
//! The slim top strip the standalone binary and the embedded shell both render
//! above [`crate::voice_panel`], hosted on the shared
//! [`mde_egui::menubar::MenuBar`] (MENUBAR-ALL-1) under the UPPERCASE **VOICE**
//! title tinted with the dock's Comms accent ([`Style::ACCENT_COMMS`], the group
//! Voice shares with Chat). It **replaces** the old registration header: the
//! account identity + live registration status move into the right-side status
//! cluster, and the header's Retry becomes the honestly-gated **File → Re-register**
//! item.
//!
//! Every menu item is the *discoverable twin of an existing seam* (§6, one
//! dispatch path — never new behaviour, never a stub): each [`Picked`] id maps in
//! [`apply`] to a real [`Command`] the SIP worker already services, an `app.tab`
//! switch, an `app.dial` edit, or a `ViewportCommand`. Per the governing principle
//! (design §7) the bar surfaces **all** of the surface's controls, but only ones
//! whose seam exists — so a context-gated item (Answer with no ringing call)
//! renders **disabled**, and a genuinely-absent feature is **omitted**:
//!
//! * **File** — Re-register ([`Command::Reregister`], present only for a
//!   registrar-backed account — a registrar-less P2P node has no registrar to
//!   re-register against, exactly the header's old Retry rule), Quit
//!   (`ViewportCommand::Close`).
//! * **Call** — Place Call ([`Command::Dial`] of the current buffer, the dialer
//!   Call button's twin, gated on a callable buffer while the dialer is showing),
//!   Answer / Decline ([`Command::Answer`]/[`Command::Decline`], gated on a ringing
//!   inbound call), Hang Up ([`Command::HangUp`], gated on an active call).
//! * **Edit** — Clear Dialed Number (empties the `app.dial` buffer, gated on a
//!   non-blank buffer).
//! * **View** — Dialer / Fleet Board (the `app.tab` toggle the panel also shows,
//!   with a live check-mark).
//! * **Help** — Keyboard Shortcuts… (a bar-owned reference window).
//!
//! **Honestly omitted** (no landed seam, so no dead entry): **Hold / Transfer**
//! (the SIP worker has no hold/transfer command — a menu item would be a stub),
//! **Contacts** (the surface has no dialer contact list; the fleet roster is
//! reachable through **View → Fleet Board**), and a **Devices / Mute** menu (the
//! [`mde_voice_hud::media`] engine picks the default cpal in/out and exposes no
//! device-select or mute seam to this surface — wiring one would be new behaviour,
//! not glue).
//!
//! The live status cluster (lock 6) reads the real [`crate::model::VoiceState`]
//! each frame: the account identity, the shipped [`RegistrationState`] label +
//! tone, the active call's [`CallState`] label + tone, and the media codec while a
//! call is connected. §4: it renders through the shared [`Style`]/[`ChipTone`]
//! tokens alone — no raw colour, no literal metric.

use mde_egui::egui::{self, Context, RichText, Ui};
use mde_egui::menubar::{Entry, Item as BarItem, Menu, MenuBar as SharedMenuBar, MenuBarModel};
use mde_egui::{ChipTone, StatusChip, Style};

use mde_voice_hud::sip::{CallState, RegistrationState};

use crate::model::{call_tone, dial_ready, registration_tone, Command, Tab, Tone};
use crate::VoiceApp;

/// The status-chip status dot (●) — a leading glyph the shared chip tints with the
/// state's tone, mirroring the old header's `status_dot`.
const STATUS_DOT: &str = "\u{25CF}";

/// The media codec the [`mde_voice_hud::media`] engine carries on a connected call
/// (G.711 µ-law/A-law, 8 kHz) — shown only while `InCall`, when media is attached.
/// A read-out of the engine's real capability, not a placeholder (§7).
const CODEC: &str = "G.711";

// ─────────────────────────────── action vocabulary ──────────────────────────

/// The one thing a rendered frame chose — the shared [`SharedMenuBar`] returns this
/// as the activated item's id, and [`apply`] routes it to the surface's real seam.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Picked {
    /// Place a call with the current dial buffer (Call → Place Call →
    /// [`Command::Dial`]).
    PlaceCall,
    /// Answer the ringing inbound call ([`Command::Answer`]).
    Answer,
    /// Decline the ringing inbound call ([`Command::Decline`]).
    Decline,
    /// Hang up the active call ([`Command::HangUp`]).
    HangUp,
    /// Re-attempt registration ([`Command::Reregister`]; registrar-backed only).
    Reregister,
    /// Empty the dialer's target buffer (Edit → Clear Dialed Number).
    ClearDial,
    /// Switch the surface's face (View → Dialer / Fleet Board).
    ShowTab(Tab),
    /// Open the keyboard-shortcuts reference (Help; bar-owned window).
    ShowShortcuts,
    /// Close the surface's viewport (File → Quit).
    Quit,
}

/// The stateful bit the bar owns: only the shortcuts-reference toggle. Every other
/// bit of state lives in the [`VoiceApp`] the bar renders over.
#[derive(Default, Debug)]
pub struct MenuBarState {
    /// Whether the keyboard-shortcuts reference window is open.
    shortcuts_open: bool,
}

// ─────────────────────────────── the render context ─────────────────────────

/// The per-frame surface snapshot the bar builds its model from — owned (small
/// clones), so the gating + status are unit-testable without egui or a live
/// [`VoiceApp`] (which needs worker channels).
#[derive(Clone, Debug)]
pub struct MenuContext {
    /// The live SIP registration mirror (label + tone + Re-register gating).
    pub registration: RegistrationState,
    /// The current call lifecycle (label + tone + the Answer/Decline/Hang-up gates).
    pub call: CallState,
    /// Whether the account is registrar-backed (gates Re-register; a P2P node has
    /// no registrar, so the item is omitted rather than a dead-end).
    pub registrar_backed: bool,
    /// The dialer's free-form target buffer (Place Call / Clear gating).
    pub dial: String,
    /// Which face is showing (the View check-marks).
    pub tab: Tab,
    /// The account identity shown as the leading status chip.
    pub identity: String,
}

impl MenuContext {
    /// Snapshot the live surface (the read half of a render frame).
    #[must_use]
    pub fn snapshot(app: &VoiceApp) -> Self {
        Self {
            registration: app.state.registration.clone(),
            call: app.state.call.clone(),
            registrar_backed: app.registrar_backed,
            dial: app.dial.clone(),
            tab: app.tab,
            identity: app.identity.clone(),
        }
    }

    /// Whether an inbound call is ringing (the Answer/Decline gate).
    const fn ringing_in(&self) -> bool {
        matches!(self.call, CallState::Incoming { .. })
    }

    /// Whether the dialer face is showing — no call set up or ringing (the Place
    /// Call gate's first half; mirrors [`crate::model::VoiceState::show_dialer`]).
    const fn dialer_face(&self) -> bool {
        matches!(
            self.call,
            CallState::Idle | CallState::Ended | CallState::Failed(_)
        )
    }

    /// Whether Place Call is enabled — the dialer is showing *and* the buffer is
    /// callable (the exact condition the dialer's Call button uses).
    fn can_place_call(&self) -> bool {
        self.dialer_face() && dial_ready(&self.dial)
    }

    /// Whether the dial buffer holds something to clear.
    fn can_clear_dial(&self) -> bool {
        !self.dial.trim().is_empty()
    }
}

// ─────────────────────────────── the menu model ─────────────────────────────

/// Map a render-agnostic [`Tone`] to a shared [`ChipTone`] (no raw colour, §4).
const fn chip_tone(tone: Tone) -> ChipTone {
    match tone {
        Tone::Ok => ChipTone::Ok,
        Tone::Busy => ChipTone::Info,
        Tone::Bad => ChipTone::Danger,
        Tone::Neutral => ChipTone::Neutral,
    }
}

/// Build the full ordered menu tree (File · Call · Edit · View · Help).
///
/// The shared model, gating each item off `cx` (§7 — present only when its seam
/// exists; disabled when context-gated). Pure — no egui.
#[must_use]
pub fn build_menus(cx: &MenuContext) -> Vec<Menu<Picked>> {
    let ringing = cx.ringing_in();

    // File — account/session maintenance. Re-register is present only for a
    // registrar-backed account (a P2P node has no registrar to retry against).
    let mut file = Vec::new();
    if cx.registrar_backed {
        file.push(Entry::Item(BarItem::new(Picked::Reregister, "Re-register")));
        file.push(Entry::Separator);
    }
    file.push(Entry::Item(BarItem::new(Picked::Quit, "Quit")));

    // Call — the surface's primary verbs, each a Command the worker services.
    let call = vec![
        Entry::Item(BarItem::new(Picked::PlaceCall, "Place Call").enabled(cx.can_place_call())),
        Entry::Separator,
        Entry::Item(BarItem::new(Picked::Answer, "Answer").enabled(ringing)),
        Entry::Item(BarItem::new(Picked::Decline, "Decline").enabled(ringing)),
        Entry::Item(BarItem::new(Picked::HangUp, "Hang Up").enabled(cx.call.is_active())),
    ];

    // Edit — the one real edit seam: clear the dialer buffer.
    let edit = vec![Entry::Item(
        BarItem::new(Picked::ClearDial, "Clear Dialed Number").enabled(cx.can_clear_dial()),
    )];

    // View — the dialer / fleet-board face toggle, with a live check-mark.
    let view = vec![
        Entry::Item(
            BarItem::new(Picked::ShowTab(Tab::Dialer), "Dialer").checked(cx.tab == Tab::Dialer),
        ),
        Entry::Item(
            BarItem::new(Picked::ShowTab(Tab::Fleet), "Fleet Board").checked(cx.tab == Tab::Fleet),
        ),
    ];

    // Help — the bar-owned shortcuts reference.
    let help = vec![Entry::Item(BarItem::new(
        Picked::ShowShortcuts,
        "Keyboard Shortcuts\u{2026}",
    ))];

    vec![
        Menu::new("File", file),
        Menu::new("Call", call),
        Menu::new("Edit", edit),
        Menu::new("View", view),
        Menu::new("Help", help),
    ]
}

/// The Voice live status cluster (lock 6).
///
/// The account identity, the shipped registration label + tone, the active call's
/// label + tone, and the media codec while a call is connected — all real state
/// read from `cx` (§7). Pure — no egui.
#[must_use]
pub fn build_status(cx: &MenuContext) -> Vec<StatusChip> {
    let mut chips = vec![
        // The account identity (which AOR / the P2P-overlay note).
        StatusChip::new(cx.identity.clone(), ChipTone::Neutral),
        // The live registration state — the shipped `RegistrationState` label,
        // never re-worded (§6).
        StatusChip::with_icon(
            STATUS_DOT,
            cx.registration.label(),
            chip_tone(registration_tone(&cx.registration)),
        ),
    ];
    // The active call's shipped label + tone, only while a call is up.
    if cx.call.is_active() {
        chips.push(StatusChip::with_icon(
            STATUS_DOT,
            cx.call.label(),
            chip_tone(call_tone(&cx.call)),
        ));
    }
    // The media codec, only while `InCall` — media is attached only on a connected
    // call, so the chip appears exactly when audio is (best-effort) flowing.
    if matches!(cx.call, CallState::InCall { .. }) {
        chips.push(StatusChip::new(CODEC, ChipTone::Info));
    }
    chips
}

// ─────────────────────────────── render + dispatch ──────────────────────────

/// Render the Voice menu bar over `app` and drive the chosen seam.
///
/// Snapshots `app` up front into an owned [`MenuContext`], builds the shared model,
/// renders it, then applies the one chosen item — so no borrow of `app` is held
/// across the render. The shell (E12-3b) and the standalone [`VoiceApp`] both call
/// this as the surface's top bar.
pub fn voice_menubar(ui: &mut Ui, app: &mut VoiceApp) {
    let picked = {
        let cx = MenuContext::snapshot(app);
        let menus = build_menus(&cx);
        let status = build_status(&cx);
        let model = MenuBarModel {
            title: "Voice",
            accent: Style::ACCENT_COMMS,
            menus: &menus,
            status: &status,
        };
        SharedMenuBar::show(ui, &model)
    };
    if let Some(picked) = picked {
        let ctx = ui.ctx().clone();
        apply(app, picked, &ctx);
    }
    let ctx = ui.ctx().clone();
    shortcuts_window(&ctx, &mut app.menu.shortcuts_open);
}

/// Dispatch a [`Picked`] id to its real seam (§6 — the render host is new, the
/// seam is the one the dialer/header already drove).
fn apply(app: &mut VoiceApp, picked: Picked, ctx: &Context) {
    match picked {
        Picked::PlaceCall => {
            let target = app.dial.clone();
            app.send(Command::Dial(target));
        }
        Picked::Answer => app.send(Command::Answer),
        Picked::Decline => app.send(Command::Decline),
        Picked::HangUp => app.send(Command::HangUp),
        Picked::Reregister => app.send(Command::Reregister),
        Picked::ClearDial => app.dial.clear(),
        Picked::ShowTab(tab) => app.tab = tab,
        Picked::ShowShortcuts => app.menu.shortcuts_open = true,
        Picked::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
    }
}

/// The real keyboard/behaviour affordances the surface carries, for the Help
/// reference — documented, never invented.
const SHORTCUTS: [(&str, &str); 1] = [("Place the call (dialer field focused)", "Enter")];

/// The keyboard-shortcuts reference window (Help → Keyboard Shortcuts). Lists the
/// surface's one real chord plus the honest peer-vs-number routing note.
fn shortcuts_window(ctx: &Context, open: &mut bool) {
    if !*open {
        return;
    }
    egui::Window::new("Voice Shortcuts")
        .open(open)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            egui::Grid::new("voice-shortcuts")
                .num_columns(2)
                .spacing([Style::SP_L, Style::SP_XS])
                .show(ui, |ui| {
                    for (label, chord) in SHORTCUTS {
                        ui.label(label);
                        ui.label(RichText::new(chord).color(Style::ACCENT));
                        ui.end_row();
                    }
                });
            ui.add_space(Style::SP_S);
            mde_egui::muted_note(
                ui,
                "A mesh peer name dials directly over the overlay; a number dials via the registrar.",
            );
        });
}

#[cfg(test)]
mod tests {
    use super::{apply, build_menus, build_status, MenuBarState, MenuContext, Picked};
    use crate::model::{Command, Tab, Update, VoiceState};
    use crate::VoiceApp;
    use mde_egui::egui;
    use mde_egui::menubar::{Entry, Menu};
    use mde_voice_hud::sip::{CallState, RegistrationState};
    use std::sync::mpsc::{self, Receiver};

    /// A fixture context (no channels, no egui) for the pure builder tests.
    fn fixture(
        call: CallState,
        reg: RegistrationState,
        dial: &str,
        registrar_backed: bool,
    ) -> MenuContext {
        MenuContext {
            registration: reg,
            call,
            registrar_backed,
            dial: dial.to_owned(),
            tab: Tab::Dialer,
            identity: "alice@sip.example.com".to_owned(),
        }
    }

    /// A live `VoiceApp` with a readable command channel — the dispatch tests
    /// assert an item drives the right `Command` / state mutation. No worker is
    /// spawned; `send` on the live sender just queues into `cmd_rx`.
    fn app_with_channel() -> (VoiceApp, Receiver<Command>) {
        let (commands, cmd_rx) = mpsc::channel::<Command>();
        let (_upd_tx, updates) = mpsc::channel::<Update>();
        let app = VoiceApp {
            state: VoiceState::new(),
            dial: String::new(),
            commands,
            updates,
            identity: "alice@sip.example.com".to_owned(),
            registrar_backed: true,
            tab: Tab::default(),
            fleet: crate::fleet::FleetState::new(),
            menu: MenuBarState::default(),
        };
        (app, cmd_rx)
    }

    /// Find a top-level menu's entries by title.
    fn menu<'a>(menus: &'a [Menu<Picked>], title: &str) -> &'a Menu<Picked> {
        menus
            .iter()
            .find(|m| m.title == title)
            .expect("menu present")
    }

    /// The enabled state of the item carrying `id`, or `None` if it is absent.
    fn enabled_of(menus: &[Menu<Picked>], id: Picked) -> Option<bool> {
        menus.iter().flat_map(|m| &m.entries).find_map(|e| match e {
            Entry::Item(item) if item.id == id => Some(item.enabled),
            _ => None,
        })
    }

    // ── the spine + surface menus (§7 structure) ─────────────────────────────

    #[test]
    fn menu_spine_builds_from_a_fixture() {
        let cx = fixture(CallState::Idle, RegistrationState::NoAccount, "", false);
        let menus = build_menus(&cx);
        let titles: Vec<&str> = menus.iter().map(|m| m.title.as_str()).collect();
        assert_eq!(
            titles,
            ["File", "Call", "Edit", "View", "Help"],
            "the File/Edit/View/Help spine + the Call surface menu, in order"
        );
        for m in &menus {
            assert!(!m.entries.is_empty(), "menu {} shipped empty", m.title);
        }
    }

    #[test]
    fn omitted_features_have_no_dead_entry() {
        // Hold/Transfer/Contacts/Devices/Mute have no landed seam → no menu, no
        // item (honest omission, not a greyed "coming soon").
        let cx = fixture(
            CallState::InCall {
                peer: "pine".to_owned(),
            },
            RegistrationState::Registered {
                server: "sip.example.com:5060".to_owned(),
                expires: 3600,
            },
            "",
            true,
        );
        let menus = build_menus(&cx);
        let labels: Vec<&str> = menus
            .iter()
            .flat_map(|m| &m.entries)
            .filter_map(|e| match e {
                Entry::Item(item) => Some(item.label.as_str()),
                _ => None,
            })
            .collect();
        for banned in ["Hold", "Transfer", "Contacts", "Devices", "Mute"] {
            assert!(
                !labels.contains(&banned),
                "{banned} shipped without a landed seam"
            );
        }
    }

    // ── honest context gating (§7 disable, not phasing) ──────────────────────

    #[test]
    fn call_items_gate_on_the_live_call_state() {
        // Idle + a callable buffer: Place Call enabled; Answer/Decline/Hang Up off.
        let idle = fixture(CallState::Idle, RegistrationState::NoAccount, "pine", false);
        let m = build_menus(&idle);
        assert_eq!(enabled_of(&m, Picked::PlaceCall), Some(true));
        assert_eq!(enabled_of(&m, Picked::Answer), Some(false));
        assert_eq!(enabled_of(&m, Picked::Decline), Some(false));
        assert_eq!(enabled_of(&m, Picked::HangUp), Some(false));

        // Idle + a blank buffer: Place Call disabled (the Call button's own rule).
        let blank = fixture(CallState::Idle, RegistrationState::NoAccount, "   ", false);
        assert_eq!(
            enabled_of(&build_menus(&blank), Picked::PlaceCall),
            Some(false)
        );

        // A ringing inbound call: Answer/Decline open; Place Call + Hang Up off
        // (Incoming is not yet an active dialog).
        let ringing = fixture(
            CallState::Incoming {
                from: "Bob".to_owned(),
            },
            RegistrationState::NoAccount,
            "pine",
            false,
        );
        let m = build_menus(&ringing);
        assert_eq!(enabled_of(&m, Picked::Answer), Some(true));
        assert_eq!(enabled_of(&m, Picked::Decline), Some(true));
        assert_eq!(enabled_of(&m, Picked::HangUp), Some(false));
        assert_eq!(enabled_of(&m, Picked::PlaceCall), Some(false));

        // A connected call: Hang Up opens; Answer/Decline/Place Call off.
        let in_call = fixture(
            CallState::InCall {
                peer: "pine".to_owned(),
            },
            RegistrationState::NoAccount,
            "pine",
            false,
        );
        let m = build_menus(&in_call);
        assert_eq!(enabled_of(&m, Picked::HangUp), Some(true));
        assert_eq!(enabled_of(&m, Picked::Answer), Some(false));
        assert_eq!(enabled_of(&m, Picked::PlaceCall), Some(false));
    }

    #[test]
    fn clear_dial_gates_on_a_nonblank_buffer() {
        let empty = fixture(CallState::Idle, RegistrationState::NoAccount, "  ", false);
        assert_eq!(
            enabled_of(&build_menus(&empty), Picked::ClearDial),
            Some(false)
        );
        let typed = fixture(CallState::Idle, RegistrationState::NoAccount, "1004", false);
        assert_eq!(
            enabled_of(&build_menus(&typed), Picked::ClearDial),
            Some(true)
        );
    }

    #[test]
    fn reregister_is_present_only_for_a_registrar_backed_account() {
        // A registrar-less P2P node: no registrar to retry → the item is omitted.
        let p2p = fixture(CallState::Idle, RegistrationState::NoAccount, "", false);
        assert_eq!(
            enabled_of(&build_menus(&p2p), Picked::Reregister),
            None,
            "P2P has no registrar — Re-register is omitted, never a dead-end"
        );
        // A registrar-backed account: the item is present (and enabled).
        let backed = fixture(
            CallState::Idle,
            RegistrationState::Failed("timeout".to_owned()),
            "",
            true,
        );
        assert_eq!(
            enabled_of(&build_menus(&backed), Picked::Reregister),
            Some(true)
        );
    }

    #[test]
    fn view_toggle_checkmarks_track_the_tab() {
        let mut cx = fixture(CallState::Idle, RegistrationState::NoAccount, "", false);
        cx.tab = Tab::Fleet;
        let menus = build_menus(&cx);
        let view = menu(&menus, "View");
        let checks: Vec<(Picked, Option<bool>)> = view
            .entries
            .iter()
            .filter_map(|e| match e {
                Entry::Item(item) => Some((item.id, item.checked)),
                _ => None,
            })
            .collect();
        assert_eq!(checks[0], (Picked::ShowTab(Tab::Dialer), Some(false)));
        assert_eq!(checks[1], (Picked::ShowTab(Tab::Fleet), Some(true)));
    }

    // ── the live status cluster (lock 6) ─────────────────────────────────────

    #[test]
    fn status_reflects_registration_and_call_state() {
        // Idle + registered: identity + registration chips, no call/codec chip.
        let reg = fixture(
            CallState::Idle,
            RegistrationState::Registered {
                server: "sip.example.com:5060".to_owned(),
                expires: 3600,
            },
            "",
            true,
        );
        let chips = build_status(&reg);
        assert_eq!(chips.len(), 2, "identity + registration only when idle");
        assert_eq!(chips[0].text, "alice@sip.example.com");
        assert!(chips[1].text.starts_with("Registered"));

        // A connected call adds the call chip *and* the codec chip.
        let in_call = fixture(
            CallState::InCall {
                peer: "pine".to_owned(),
            },
            RegistrationState::Registered {
                server: "sip.example.com:5060".to_owned(),
                expires: 3600,
            },
            "",
            true,
        );
        let chips = build_status(&in_call);
        assert_eq!(chips.len(), 4, "identity + registration + call + codec");
        assert_eq!(chips[2].text, "In call · pine");
        assert_eq!(chips[3].text, super::CODEC);

        // A dialing (not-yet-connected) call adds the call chip but no codec yet.
        let dialing = fixture(
            CallState::Calling {
                peer: "pine".to_owned(),
            },
            RegistrationState::NoAccount,
            "",
            false,
        );
        assert_eq!(
            build_status(&dialing).len(),
            3,
            "call chip, no codec until InCall"
        );
    }

    // ── item → real seam (§6 glue) ───────────────────────────────────────────

    #[test]
    fn call_items_dispatch_their_worker_command() {
        let ctx = egui::Context::default();
        let (mut app, cmd_rx) = app_with_channel();
        app.dial = "pine".to_owned();

        apply(&mut app, Picked::PlaceCall, &ctx);
        assert!(
            matches!(cmd_rx.recv().unwrap(), Command::Dial(t) if t == "pine"),
            "Place Call drove Command::Dial of the buffer"
        );
        apply(&mut app, Picked::Answer, &ctx);
        assert!(matches!(cmd_rx.recv().unwrap(), Command::Answer));
        apply(&mut app, Picked::Decline, &ctx);
        assert!(matches!(cmd_rx.recv().unwrap(), Command::Decline));
        apply(&mut app, Picked::HangUp, &ctx);
        assert!(matches!(cmd_rx.recv().unwrap(), Command::HangUp));
        apply(&mut app, Picked::Reregister, &ctx);
        assert!(matches!(cmd_rx.recv().unwrap(), Command::Reregister));
    }

    #[test]
    fn edit_view_and_help_items_mutate_the_surface() {
        let ctx = egui::Context::default();
        let (mut app, _cmd_rx) = app_with_channel();
        app.dial = "1004".to_owned();

        apply(&mut app, Picked::ClearDial, &ctx);
        assert!(app.dial.is_empty(), "Edit → Clear emptied the dial buffer");

        apply(&mut app, Picked::ShowTab(Tab::Fleet), &ctx);
        assert_eq!(app.tab, Tab::Fleet, "View → Fleet Board switched the face");
        apply(&mut app, Picked::ShowTab(Tab::Dialer), &ctx);
        assert_eq!(app.tab, Tab::Dialer);

        assert!(!app.menu.shortcuts_open);
        apply(&mut app, Picked::ShowShortcuts, &ctx);
        assert!(
            app.menu.shortcuts_open,
            "Help → Shortcuts opened the window"
        );
    }

    // ── the bar renders headless (title + menus + status) ────────────────────

    #[test]
    fn menu_bar_renders_headless() {
        use mde_egui::egui::{pos2, vec2, Rect};
        use mde_egui::Style;
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let (mut app, _cmd_rx) = app_with_channel();
        app.state.call = CallState::InCall {
            peer: "pine".to_owned(),
        };
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::TopBottomPanel::top("voice-menubar").show(ctx, |ui| {
                super::voice_menubar(ui, &mut app);
            });
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(
            !prims.is_empty(),
            "the menu bar produced no draw primitives"
        );
    }
}
