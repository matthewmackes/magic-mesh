//! The **Browser** surface — the sandboxed Servo browser rendered egui-native.
//!
//! BOOKMARKS-6 brokers the out-of-process `mde-web-preview` helper *into* the one
//! shell (the same EMBED model as the VDI Desktop surface): the helper renders
//! offscreen into a shared-memory frame; [`mde_web_preview_client`] receives that
//! frame fd over the per-session socket, maps it read-only, and hands the shell an
//! [`egui::ColorImage`]. This panel uploads that image to a `TextureHandle` on a
//! paint-ready (never a per-frame re-upload), paints it as the body, wires the
//! navigation chrome (back / forward / reload / address bar, §4 tokens) to the
//! control socket, and forwards this frame's egui input scaled by
//! `pixels_per_point`.
//!
//! ```text
//!   session.take_frame() ─▶ ColorImage ─▶ TextureHandle ─▶ ui paints the body
//!   chrome + ui.input     ───────────────────────────────▶ session control/input
//! ```
//!
//! Each tab is an independent [`WebSession`], so one page crashing surfaces an
//! honest "page crashed" state for THAT tab only (respawn-on-reload) and never
//! touches the others (per-session isolation). Spawning the live Servo helper is
//! the client crate's `live-helper` path, honest-gated to a GPU seat; with no live
//! session attached this surface shows an honest gated `EmptyState`, never a fake
//! page (§7).

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_chat::MessageKind;
use mde_egui::egui::{self, RichText, Sense, TextureHandle, TextureOptions};
use mde_egui::{muted_note, Style};

use mde_web_preview_client::{SessionState, WebSession};

// ── live-helper: spawning the real sandboxed `mde-web-preview` helper ──────────
//
// Gated behind `mde-shell-egui`'s `live-helper` feature, which turns on the client
// crate's `live-helper` spawn API ([`WebSession::spawn`] + [`SpawnSpec`]). OFF by
// default so the shell stays portable and the Browser surface shows its honest
// gated EmptyState (§7); ON, the surface spawns the sandboxed helper on first open.
#[cfg(feature = "live-helper")]
use mde_web_preview_client::session::SpawnSpec;

/// The sandboxed-helper binary the RPM installs; overridable via [`HELPER_BIN_ENV`]
/// for the test bed / dev builds.
#[cfg(feature = "live-helper")]
const DEFAULT_HELPER_BIN: &str = "/usr/bin/mde-web-preview";

/// The env var overriding [`DEFAULT_HELPER_BIN`] (test bed / dev builds).
#[cfg(feature = "live-helper")]
const HELPER_BIN_ENV: &str = "MDE_WEB_PREVIEW_BIN";

/// The first page a freshly spawned live tab loads.
#[cfg(feature = "live-helper")]
const START_URL: &str = "about:blank";

/// The initial helper view geometry (device px); the scaled body fills the panel,
/// and the helper repaints on the address bar's first navigation.
#[cfg(feature = "live-helper")]
const INIT_W: u32 = 1280;
#[cfg(feature = "live-helper")]
const INIT_H: u32 = 800;

/// Resolve the sandboxed-helper binary path — the [`HELPER_BIN_ENV`] override, else
/// [`DEFAULT_HELPER_BIN`].
#[cfg(feature = "live-helper")]
fn helper_bin_path() -> std::path::PathBuf {
    std::env::var_os(HELPER_BIN_ENV).map_or_else(
        || std::path::PathBuf::from(DEFAULT_HELPER_BIN),
        std::path::PathBuf::from,
    )
}

/// The browser body is scaled to fill the surface, so sample it linearly.
const BROWSER_TEX: TextureOptions = TextureOptions::LINEAR;

/// One browser tab: its driven session and the GPU texture its frames upload into.
struct Tab {
    /// The IPC + shm session driving one sandboxed helper.
    session: WebSession,
    /// The body texture — allocated on the first frame, then updated in place with
    /// [`TextureHandle::set`] on each subsequent paint-ready (egui reuses the
    /// allocation, so a live page is not a per-frame upload churn).
    texture: Option<TextureHandle>,
}

/// The Browser surface's state: the open tabs, the active one, and the address-bar
/// edit buffer.
#[derive(Default)]
pub(crate) struct WebState {
    /// The open browser tabs (each an isolated session). Empty until a session
    /// attaches — spawning the live helper is the gated `live-helper` path.
    tabs: Vec<Tab>,
    /// Index of the active tab in [`Self::tabs`].
    active: usize,
    /// The address-bar edit buffer for the active tab.
    address: String,
    /// Set when Reload is pressed on a *crashed* active tab — the shell (or a test)
    /// drains it and swaps in a fresh session (respawn-on-reload).
    respawn_requested: bool,
    /// An honest gated notice shown in place of the `EmptyState` when a `live-helper`
    /// open couldn't proceed (no seat · helper binary absent · spawn failed). `None`
    /// = the default gated caption. Only ever set on the live path — a named reason,
    /// never a fake page (§7).
    #[cfg(feature = "live-helper")]
    gate_notice: Option<String>,
    /// One-shot latch so the real `Command::spawn` is attempted at most once per
    /// surface lifetime — a spawn *failure* must not respawn a process every frame.
    #[cfg(feature = "live-helper")]
    spawn_attempted: bool,
}

impl WebState {
    /// The active tab, if any.
    fn active_tab(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }

    /// Append a session as a new tab and make it active. The live helper-spawn open
    /// path (gated) and the tests both funnel through here; the default gated build
    /// opens no tabs and shows the honest `EmptyState`, so this is unused there.
    #[cfg_attr(
        not(any(test, feature = "live-helper")),
        allow(
            dead_code,
            reason = "tabs are opened by the gated live-helper spawn (client crate) \
                      and the tests; the default shell build shows the EmptyState"
        )
    )]
    pub(crate) fn push_session(&mut self, session: WebSession) {
        self.tabs.push(Tab {
            session,
            texture: None,
        });
        self.active = self.tabs.len() - 1;
    }

    /// Whether a crashed tab's Reload asked for a respawn — drained by the shell
    /// each frame (and by the tests). The live build swaps in a fresh session via
    /// [`Self::respawn_active_with`]; the gated build acknowledges it honestly.
    pub(crate) fn take_respawn_request(&mut self) -> bool {
        std::mem::take(&mut self.respawn_requested)
    }

    /// Replace the active tab's crashed session with a fresh one (respawn-on-reload),
    /// discarding its stale texture so the new page uploads cleanly.
    #[cfg_attr(
        not(any(test, feature = "live-helper")),
        allow(
            dead_code,
            reason = "the respawn target is created by the gated live-helper path (and tests)"
        )
    )]
    pub(crate) fn respawn_active_with(&mut self, session: WebSession) {
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.session = session;
            tab.texture = None;
        }
    }
}

/// The `live-helper` spawn glue: creating live [`WebSession`]s by launching the
/// sandboxed `mde-web-preview` helper (the client crate's `live-helper` API) and
/// attaching them as tabs, behind the honest deployment gates (§7). All of this is
/// compiled out of the default portable build.
#[cfg(feature = "live-helper")]
impl WebState {
    /// Ensure a live browser tab exists — spawn the sandboxed helper on first open.
    /// The shell's Browser arm calls this each frame with the live seat verdict. A
    /// no-op once a tab is open, and the real `Command::spawn` is attempted at most
    /// once (a failure surfaces an honest notice, never a per-frame spawn-storm).
    pub(crate) fn ensure_live_tab(&mut self, seat_present: bool) {
        if !self.tabs.is_empty() || self.spawn_attempted {
            return;
        }
        self.spawn_attempted = true;
        self.open_with(seat_present, helper_bin_path(), WebSession::spawn);
    }

    /// Respawn the active crashed tab with a fresh live session (respawn-on-reload),
    /// drained by the Browser arm when [`Self::take_respawn_request`] fires. Driven
    /// by an explicit user Reload, so it is not rate-limited by the one-shot latch.
    pub(crate) fn respawn_live(&mut self) {
        // A tab was already open, so the seat gate is already proven live.
        if let Some(session) = self.make_session(true, helper_bin_path(), WebSession::spawn) {
            self.respawn_active_with(session);
        }
    }

    /// Testable core of [`Self::ensure_live_tab`]: attach a session from `spawn` as
    /// the first tab, applying the honest gates. Production passes
    /// [`WebSession::spawn`]; the tests inject a `testkit` factory so no real process
    /// is spawned (hermetic CI).
    fn open_with(
        &mut self,
        seat_present: bool,
        helper_bin: std::path::PathBuf,
        spawn: impl FnOnce(&SpawnSpec) -> std::io::Result<WebSession>,
    ) {
        if let Some(session) = self.make_session(seat_present, helper_bin, spawn) {
            self.push_session(session);
        }
    }

    /// Build one live session behind the honest gates (a usable seat · the helper
    /// binary installed · the spawn succeeding), or record a NAMED notice and return
    /// `None`. Never fakes a page, never panics, never hangs — a spawn failure
    /// surfaces its reason (§7).
    fn make_session(
        &mut self,
        seat_present: bool,
        helper_bin: std::path::PathBuf,
        spawn: impl FnOnce(&SpawnSpec) -> std::io::Result<WebSession>,
    ) -> Option<WebSession> {
        if !seat_present {
            self.gate_notice =
                Some("The sandboxed browser needs a GPU seat — none is available here.".to_owned());
            return None;
        }
        if !helper_bin.exists() {
            self.gate_notice = Some(format!(
                "The sandboxed browser helper is not installed ({}).",
                helper_bin.display()
            ));
            return None;
        }
        let spec = SpawnSpec {
            helper_bin,
            url: START_URL.to_owned(),
            width: INIT_W,
            height: INIT_H,
        };
        match spawn(&spec) {
            Ok(session) => {
                self.gate_notice = None;
                Some(session)
            }
            Err(e) => {
                self.gate_notice =
                    Some(format!("The sandboxed browser helper failed to start: {e}"));
                None
            }
        }
    }
}

/// Render the Browser surface into `ui`: poll every tab, upload any fresh frame on
/// the active tab, draw the navigation chrome, and paint the body (or the honest
/// crashed / loading / gated states).
pub(crate) fn web_panel(ui: &mut egui::Ui, state: &mut WebState) {
    // 1. Poll every tab so background tabs keep receiving — and so ONE tab's crash
    //    is observed here without disturbing the others (per-session isolation).
    for tab in &mut state.tabs {
        tab.session.poll();
    }

    // 2. Upload the active tab's pending frame — ONLY when one is present, so an
    //    idle page never triggers a re-upload.
    if let Some(tab) = state.active_tab() {
        if let Some(img) = tab.session.take_frame() {
            match tab.texture.as_mut() {
                Some(handle) => handle.set(img, BROWSER_TEX),
                None => tab.texture = Some(ui.ctx().load_texture("web-preview", img, BROWSER_TEX)),
            }
        }
    }

    // 3. The shared MENUBAR-ALL top bar — the UPPERCASE BROWSER title, the real
    //    WebSession menus (Edit / View / History / Bookmarks), and the live status
    //    cluster. It COMPLEMENTS the navigation chrome below (never replaces it —
    //    the address bar + back/forward/reload buttons stay), the same model→seam
    //    pattern every other surface uses (design: `docs/design/menubar-all.md`).
    if let Some(action) = menubar::show(state, ui) {
        menubar::apply(ui.ctx(), state, action);
    }
    ui.add_space(Style::SP_XS);

    // 4. The navigation chrome (back / forward / reload / address bar), wired to
    //    the active session's control socket.
    nav_chrome(ui, state);
    ui.add_space(Style::SP_XS);

    // 5. The body. Read the active tab's status first so the crashed arm can set
    //    the respawn flag without holding a `&mut Tab` borrow of `state`.
    let active = state.active;
    let status = state.tabs.get(active).map(|t| {
        (
            t.session.is_crashed(),
            t.texture.is_some(),
            crash_reason(&t.session),
        )
    });
    match status {
        Some((true, _, reason)) => crashed_body(ui, reason, &mut state.respawn_requested),
        Some((false, true, _)) => {
            if let Some(tab) = state.tabs.get_mut(active) {
                paint_body(ui, tab);
            }
        }
        Some((false, false, _)) => {
            // Connected, no first frame yet — an honest loading note, never a blank.
            centered(ui, |ui| {
                muted_note(ui, "Loading the page\u{2026}");
            });
        }
        None => {
            // The honest gated body — a `live-helper` build shows the NAMED gate
            // notice (no seat · helper absent · spawn failed) when one is set; the
            // default build always shows the standard gated caption (§7).
            #[cfg(feature = "live-helper")]
            let notice = state.gate_notice.as_deref();
            #[cfg(not(feature = "live-helper"))]
            let notice: Option<&str> = None;
            empty_body(ui, notice);
        }
    }
}

/// The crash reason string for a session, or empty if it has not crashed.
fn crash_reason(session: &WebSession) -> String {
    match session.state() {
        SessionState::Crashed { reason } => reason.clone(),
        _ => String::new(),
    }
}

// ── BOOKMARKS-10: mesh integration (Send-in-Chat + copy-URL + add-from-page) ──
//
// The Browser surface composes the live page with two mesh services by REUSING
// their existing Bus verbs (§6 JSON boundaries — a local mirror of each topic,
// never a dep on the mackesd worker, never a re-derived store):
//
//   * add-from-page → `action/bookmarks/add` — the BOOKMARKS-2 worker (the
//                      single writer of this node's op-log segment + HLC clock,
//                      the services/desktop tier boundary) drains it, mints the
//                      real `Op::Add`, persists it, and Syncthing-syncs it across
//                      the mesh. `source` is omitted → the honest `Source::Manual`.
//   * Send-in-Chat  → `action/chat/send` — the SAME verb the shell's Chat composer
//                      and Files' `chat_bridge` publish; a link rides the
//                      NOTIFY-CHAT `Clipboard` message-kind (a `kind` wins over
//                      `text` in the worker), addressed to this node's own Chat
//                      contact (the notification hub where its clips land).
//   * copy-URL      → the shell clipboard — egui's output command (the DRM /
//                      windowed backend owns the wire), the same one-click re-copy
//                      the Chat clipboard card uses.

/// The mackesd bookmarks worker's add verb (`action/bookmarks/<verb>`, §9).
const ACTION_BOOKMARKS_ADD: &str = "action/bookmarks/add";

/// The mackesd chat worker's send verb (reused, never re-invented).
const ACTION_CHAT_SEND: &str = "action/chat/send";

/// Build the `action/bookmarks/add` body for the live page. Pure — the wire shape
/// is asserted headless. `source` is omitted, so the worker mints the default
/// `Source::Manual` (a page the user bookmarked in-app).
fn bookmark_add_body(url: &str, title: &str) -> String {
    serde_json::json!({ "url": url, "title": title }).to_string()
}

/// Build the `action/chat/send` body sharing the live page into Chat. A link is
/// carried as the NOTIFY-CHAT [`MessageKind::Clipboard`] kind — its `preview`
/// (the title, falling back to the URL) shows in the timeline and its `full` (the
/// exact URL) is what a one-click re-copy puts back. Pure: the `kind` is a real
/// `mde_chat::MessageKind`, so it round-trips straight into what the worker
/// accepts (the same shape Files' `chat_bridge` writes).
fn chat_share_body(to: &str, url: &str, title: &str) -> String {
    let preview = if title.trim().is_empty() { url } else { title };
    let kind = MessageKind::Clipboard {
        preview: preview.to_string(),
        full: url.to_string(),
    };
    let kind_val = serde_json::to_value(&kind).unwrap_or(serde_json::Value::Null);
    serde_json::json!({ "scope": "peer", "to": to, "kind": kind_val }).to_string()
}

/// Publish `body` on `topic` via the persist-first path (the same discipline as
/// the shell's Chat composer + Files' `chat_bridge`). Best-effort: no Bus on this
/// node / a transient open failure is a silent no-op — the honest solo-host
/// state, never a panic.
fn publish(topic: &str, body: &str) {
    let Some(root) = mde_bus::client_data_dir() else {
        return;
    };
    let Ok(persist) = Persist::open(root) else {
        return;
    };
    let _ = persist.write(topic, Priority::Default, None, Some(body));
}

/// The local hostname — the mesh identity a Send-in-Chat addresses (lock 2/21:
/// the hostname *is* the chat contact username). `$HOSTNAME` →
/// `/proc/sys/kernel/hostname` → `/etc/hostname` → `"localhost"`.
fn local_hostname() -> String {
    if let Ok(h) = std::env::var("HOSTNAME") {
        let h = h.trim();
        if !h.is_empty() {
            return h.to_string();
        }
    }
    for path in ["/proc/sys/kernel/hostname", "/etc/hostname"] {
        if let Ok(h) = std::fs::read_to_string(path) {
            let h = h.trim();
            if !h.is_empty() {
                return h.to_string();
            }
        }
    }
    "localhost".to_string()
}

/// The Browser page-actions menu (BOOKMARKS-10): the three mesh-integration verbs
/// on the current page. Rendered by BOTH the toolbar menu button and the address
/// bar's right-click context menu (one body, two entry points). Each item greys
/// out with no live URL to act on. §4 Carbon tokens on the chrome.
fn page_actions_menu(ui: &mut egui::Ui, url: &str, title: &str) {
    let has_page = !url.trim().is_empty();
    // Add-from-page → the mesh-synced bookmarks store (via the worker's add verb).
    if ui
        .add_enabled(
            has_page,
            egui::Button::new(RichText::new("\u{2606}  Add bookmark").color(Style::TEXT)),
        )
        .clicked()
    {
        publish(ACTION_BOOKMARKS_ADD, &bookmark_add_body(url, title));
        ui.close_menu();
    }
    // Copy-URL → the shell clipboard (egui's output command).
    if ui
        .add_enabled(
            has_page,
            egui::Button::new(RichText::new("\u{29C9}  Copy URL").color(Style::TEXT)),
        )
        .clicked()
    {
        ui.ctx().copy_text(url.to_string());
        ui.close_menu();
    }
    // Send-in-Chat → the NOTIFY-CHAT send verb.
    if ui
        .add_enabled(
            has_page,
            egui::Button::new(RichText::new("\u{1F4AC}  Send in Chat").color(Style::TEXT)),
        )
        .clicked()
    {
        publish(
            ACTION_CHAT_SEND,
            &chat_share_body(&local_hostname(), url, title),
        );
        ui.close_menu();
    }
}

/// The toolbar star that opens the BOOKMARKS-10 [`page_actions_menu`]; the glyph
/// dims with no live page (the menu items disable themselves too). Split out of
/// [`nav_chrome`] to keep that toolbar within its line budget.
fn page_actions_button(ui: &mut egui::Ui, has_page: bool, url: &str, title: &str) {
    let color = if has_page {
        Style::TEXT
    } else {
        Style::TEXT_DIM
    };
    ui.menu_button(
        RichText::new("\u{2606}").size(Style::BODY).color(color),
        |ui| {
            page_actions_menu(ui, url, title);
        },
    )
    .response
    .on_hover_text("Page actions \u{2014} bookmark, copy URL, send in Chat");
}

/// The navigation chrome bar — a §4-token toolbar. Back / forward / reload act on
/// the active session; the address bar loads on submit. On a crashed tab, Reload
/// becomes a respawn request. The page-actions menu (BOOKMARKS-10) hangs off both
/// the toolbar star button and the address bar's right-click.
fn nav_chrome(ui: &mut egui::Ui, state: &mut WebState) {
    let crashed = state
        .tabs
        .get(state.active)
        .is_some_and(|t| t.session.is_crashed());
    let nav = state
        .tabs
        .get(state.active)
        .map(|t| t.session.nav().clone())
        .unwrap_or_default();
    let has_tab = !state.tabs.is_empty();
    // BOOKMARKS-7 — the per-page ad-filter blocked count the active session tracks.
    let blocked = state
        .tabs
        .get(state.active)
        .map_or(0, |t| t.session.blocked_count());
    // BOOKMARKS-10 — the live page's URL + title, owned clones so the page-actions
    // closures never borrow `state`; empty when there is no live tab to act on.
    let page_url = state
        .tabs
        .get(state.active)
        .map(|t| t.session.nav().url.clone())
        .unwrap_or_default();
    let page_title = state
        .tabs
        .get(state.active)
        .map(|t| t.session.title().to_string())
        .unwrap_or_default();
    let has_page = has_tab && !crashed && !page_url.trim().is_empty();

    ui.horizontal(|ui| {
        // Back / forward — enabled only when the live session offers the history.
        if nav_button(ui, "\u{2039}", "Back", has_tab && !crashed && nav.can_back) {
            if let Some(tab) = state.active_tab() {
                tab.session.go_back();
            }
        }
        if nav_button(
            ui,
            "\u{203A}",
            "Forward",
            has_tab && !crashed && nav.can_forward,
        ) {
            if let Some(tab) = state.active_tab() {
                tab.session.go_forward();
            }
        }
        // Reload — respawns a crashed tab, otherwise reloads the page.
        let reload_tip = if crashed {
            "Reload (restart page)"
        } else {
            "Reload"
        };
        if nav_button(ui, "\u{21BB}", reload_tip, has_tab) {
            if crashed {
                state.respawn_requested = true;
            } else if let Some(tab) = state.active_tab() {
                tab.session.reload();
            }
        }

        // BOOKMARKS-10 — the page-actions menu (bookmark this page / copy its URL /
        // send it in Chat). The SAME three verbs also hang off the address bar's
        // right-click (below), so both the toolbar and the context menu reach them.
        page_actions_button(ui, has_page, &page_url, &page_title);

        // BOOKMARKS-7 — a compact "N blocked" shield when the ad-filter has dropped
        // requests on this page (honest 0 stays hidden). Reads the session's
        // per-page counter; the engine is compiled from the mackesd `adfilter` blob.
        if blocked > 0 {
            ui.add_space(Style::SP_XS);
            ui.label(
                RichText::new(format!("\u{2298} {blocked}"))
                    .size(Style::BODY)
                    .color(Style::TEXT_DIM),
            )
            .on_hover_text(format!(
                "Ad-filter blocked {blocked} request{} on this page",
                if blocked == 1 { "" } else { "s" }
            ));
        }

        ui.add_space(Style::SP_XS);

        // The address bar fills the rest of the row.
        let field = egui::TextEdit::singleline(&mut state.address)
            .desired_width(ui.available_width() - Style::SP_XL * 2.0)
            .hint_text("Enter an address")
            .text_color(Style::TEXT);
        let resp = ui.add_enabled(has_tab && !crashed, field);
        // BOOKMARKS-10 — right-click the address bar for the same page actions
        // (bookmark / copy URL / Send-in-Chat) the toolbar star exposes.
        resp.context_menu(|ui| page_actions_menu(ui, &page_url, &page_title));
        let submit = resp.lost_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Enter))
            && has_tab
            && !crashed;

        let go = ui
            .add_enabled(
                has_tab && !crashed && !state.address.trim().is_empty(),
                egui::Button::new(RichText::new("Go").color(Style::TEXT)),
            )
            .clicked();

        if submit || go {
            let url = state.address.trim().to_owned();
            if let Some(tab) = state.active_tab() {
                tab.session.load(url);
            }
        }
    });
}

/// A compact chrome button in the §4 palette, returning whether it was clicked.
fn nav_button(ui: &mut egui::Ui, glyph: &str, tip: &str, enabled: bool) -> bool {
    let color = if enabled {
        Style::TEXT
    } else {
        Style::TEXT_DIM
    };
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(glyph).size(Style::BODY).color(color))
            .min_size(egui::vec2(Style::SP_L, Style::SP_L)),
    )
    .on_hover_text(tip)
    .clicked()
}

/// Paint the active tab's decoded frame to fill the body and forward this frame's
/// egui input to the session (scaled by `pixels_per_point`).
fn paint_body(ui: &mut egui::Ui, tab: &mut Tab) {
    let Some(texture) = tab.texture.as_ref() else {
        return;
    };
    let tex_id = texture.id();
    let size = ui.available_size();
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    egui::Image::new(egui::load::SizedTexture::new(tex_id, rect.size())).paint_at(ui, rect);
    if resp.clicked() {
        resp.request_focus();
    }

    // Forward this frame's input, scaled to the helper's device pixels.
    let ppp = ui.ctx().pixels_per_point();
    for event in ui.input(|i| i.events.clone()) {
        tab.session.send_input(&event, ppp);
    }
}

/// The honest "page crashed" body: a danger caption naming the reason plus a
/// Reload that respawns the tab (setting `respawn_requested`). Never a fake page.
fn crashed_body(ui: &mut egui::Ui, reason: String, respawn_requested: &mut bool) {
    centered(ui, |ui| {
        ui.label(
            RichText::new("This page crashed")
                .size(Style::HEADING)
                .color(Style::DANGER),
        );
        ui.add_space(Style::SP_S);
        if !reason.is_empty() {
            muted_note(ui, reason);
        }
        ui.add_space(Style::SP_M);
        if ui
            .add(egui::Button::new(
                RichText::new("\u{21BB} Reload").color(Style::TEXT),
            ))
            .clicked()
        {
            *respawn_requested = true;
        }
    });
}

/// The no-session `EmptyState` — an honest gated caption (or the NAMED live-path
/// notice when one is passed), never a placeholder page.
fn empty_body(ui: &mut egui::Ui, notice: Option<&str>) {
    centered(ui, |ui| {
        ui.label(
            RichText::new("Sandboxed browser")
                .size(Style::HEADING)
                .color(Style::TEXT),
        );
        ui.add_space(Style::SP_S);
        muted_note(
            ui,
            notice.unwrap_or(
                "The sandboxed Servo browser renders here in the shell. A live session \
                 attaches on a GPU seat (BOOKMARKS-5/6 live path is gated).",
            ),
        );
    });
}

/// Center `content` vertically + horizontally in the remaining body.
fn centered(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui)) {
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() * 0.5 - Style::SP_XL);
        content(ui);
    });
}

/// The Browser surface's shared **MENUBAR-ALL** top bar (design: `menubar-all.md`).
///
/// The UPPERCASE `BROWSER` title in the Terminals-group accent, the real
/// [`WebSession`] menus, and a live status cluster — the one shared
/// [`mde_egui::menubar::MenuBar`] every surface embeds. Every visible item binds to
/// a seam that already exists on the active session or page (§6 glue, no new
/// behaviour — the SAME seams the toolbar chrome + [`page_actions_menu`] drive); a
/// context-gated item renders **disabled** and an absent capability is **omitted**
/// (§7). The surface honestly has no File (no new-tab/quit seam in the portable
/// build), no page-text Copy/Find, no Zoom, and no keyboard chord table, so those
/// menus are absent — never a dead entry. The status cluster shows the committed
/// URL, the session lifecycle, the http/https security state, and the ad-filter
/// shield (BOOKMARKS-7).
mod menubar {
    use super::{
        bookmark_add_body, chat_share_body, local_hostname, publish, WebState,
        ACTION_BOOKMARKS_ADD, ACTION_CHAT_SEND,
    };
    use mde_egui::egui;
    use mde_egui::menubar::{Entry, Item, Menu, MenuBar, MenuBarModel};
    use mde_egui::{ChipTone, StatusChip, Style};
    use mde_web_preview_client::SessionState;

    /// The lock glyph a secure (https) page wears in the security chip.
    const LOCK: &str = "\u{1F512}";
    /// The open-lock glyph a plain (http) page wears in the security chip.
    const UNLOCK: &str = "\u{1F513}";
    /// The ad-filter shield glyph (matches the toolbar "N blocked" readout).
    const SHIELD: &str = "\u{2298}";
    /// The committed-URL chip truncates to this many characters so a long address
    /// never crowds the status cluster.
    const URL_MAX: usize = 42;

    /// One Browser menu action — each maps to a real [`WebSession`]/page seam in
    /// [`apply`]. `Copy`, so the menu model stays a plain value tree.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) enum MenuAction {
        /// Navigate back (`WebSession::go_back`).
        Back,
        /// Navigate forward (`WebSession::go_forward`).
        Forward,
        /// Reload the page, or respawn a crashed tab (`WebSession::reload` /
        /// `respawn_requested` — the exact toolbar Reload behaviour).
        Reload,
        /// Copy the committed URL to the shell clipboard (the page-actions seam).
        CopyUrl,
        /// Bookmark the live page (`action/bookmarks/add`, BOOKMARKS-10).
        AddBookmark,
        /// Share the live page into Chat (`action/chat/send`, BOOKMARKS-10).
        SendInChat,
    }

    /// A per-frame read-out of the active tab's live state — the single immutable
    /// borrow the menu model + status cluster are both built from, so the render is
    /// a pure function of it (unit-testable without a driven session).
    #[derive(Default)]
    #[allow(
        clippy::struct_excessive_bools,
        reason = "a flat read-out of the active tab's nav flags (can_back/can_forward/\
                  loading, mirroring NavState) plus has_tab/crashed — not a state machine"
    )]
    struct Snapshot {
        /// A tab is attached.
        has_tab: bool,
        /// The active tab has crashed.
        crashed: bool,
        /// A back-history entry exists.
        can_back: bool,
        /// A forward-history entry exists.
        can_forward: bool,
        /// A load is in progress.
        loading: bool,
        /// The ad-filter blocked-request count for this page (BOOKMARKS-7).
        blocked: u32,
        /// The committed URL.
        url: String,
        /// The session lifecycle, or `None` with no tab.
        state: Option<SessionState>,
    }

    impl Snapshot {
        /// Whether there is a live page (a non-crashed tab with a URL) to act on —
        /// the gate the page-family items (Copy URL / bookmark / share) share.
        fn has_page(&self) -> bool {
            self.has_tab && !self.crashed && !self.url.trim().is_empty()
        }
    }

    /// Read the active tab's live state into a [`Snapshot`] (one immutable borrow).
    fn snapshot(state: &WebState) -> Snapshot {
        state
            .tabs
            .get(state.active)
            .map_or_else(Snapshot::default, |tab| {
                let nav = tab.session.nav();
                Snapshot {
                    has_tab: true,
                    crashed: tab.session.is_crashed(),
                    can_back: nav.can_back,
                    can_forward: nav.can_forward,
                    loading: nav.loading,
                    blocked: tab.session.blocked_count(),
                    url: nav.url.clone(),
                    state: Some(tab.session.state().clone()),
                }
            })
    }

    /// The Reload item's label — "Restart Page" on a crashed tab (it respawns),
    /// "Reload" otherwise (mirrors the toolbar tooltip).
    const fn reload_label(crashed: bool) -> &'static str {
        if crashed {
            "Restart Page"
        } else {
            "Reload"
        }
    }

    /// Build the Browser menus from live state (§6/§7): Edit (Copy URL), View
    /// (Reload), History (Back/Forward, gated on the live history), and Bookmarks
    /// (add plus share). File is omitted (no new-tab/quit seam in this surface), and
    /// so are page-text Copy/Find, Zoom, and Help (no chord table) — honest absence,
    /// never a dead entry.
    fn build_menus(s: &Snapshot) -> Vec<Menu<MenuAction>> {
        let has_page = s.has_page();
        vec![
            Menu::new(
                "Edit",
                vec![Entry::Item(
                    Item::new(MenuAction::CopyUrl, "Copy URL").enabled(has_page),
                )],
            ),
            Menu::new(
                "View",
                vec![Entry::Item(
                    Item::new(MenuAction::Reload, reload_label(s.crashed)).enabled(s.has_tab),
                )],
            ),
            Menu::new(
                "History",
                vec![
                    Entry::Item(
                        Item::new(MenuAction::Back, "Back")
                            .enabled(s.has_tab && !s.crashed && s.can_back),
                    ),
                    Entry::Item(
                        Item::new(MenuAction::Forward, "Forward")
                            .enabled(s.has_tab && !s.crashed && s.can_forward),
                    ),
                ],
            ),
            Menu::new(
                "Bookmarks",
                vec![
                    Entry::Item(
                        Item::new(MenuAction::AddBookmark, "Add Bookmark").enabled(has_page),
                    ),
                    Entry::Separator,
                    Entry::Item(
                        Item::new(MenuAction::SendInChat, "Send in Chat").enabled(has_page),
                    ),
                ],
            ),
        ]
    }

    /// The lifecycle status chip: Loading (a load in flight or the pre-first-frame
    /// state), Live (a painted, settled page), Crashed, or an idle "No session"
    /// with no tab.
    fn state_chip(s: &Snapshot) -> StatusChip {
        match &s.state {
            None => StatusChip::new("No session", ChipTone::Neutral),
            Some(SessionState::Crashed { .. }) => StatusChip::new("Crashed", ChipTone::Danger),
            Some(SessionState::Loading) => StatusChip::new("Loading", ChipTone::Info),
            Some(SessionState::Live) => {
                if s.loading {
                    StatusChip::new("Loading", ChipTone::Info)
                } else {
                    StatusChip::new("Live", ChipTone::Ok)
                }
            }
        }
    }

    /// The http/https security chip for the committed URL — a lock (Ok) for https,
    /// an open lock (Warn) for http, or `None` for a schemeless address
    /// (`about:blank`, empty) with no security state to report.
    fn security_chip(s: &Snapshot) -> Option<StatusChip> {
        if !s.has_tab {
            return None;
        }
        let url = s.url.trim();
        if url.starts_with("https://") {
            Some(StatusChip::with_icon(LOCK, "https", ChipTone::Ok))
        } else if url.starts_with("http://") {
            Some(StatusChip::with_icon(UNLOCK, "http", ChipTone::Warn))
        } else {
            None
        }
    }

    /// Truncate a URL to [`URL_MAX`] characters (an ellipsis tail) so the chip
    /// stays compact; a short URL is verbatim.
    fn truncate_url(url: &str) -> String {
        let url = url.trim();
        if url.chars().count() <= URL_MAX {
            return url.to_owned();
        }
        let head: String = url.chars().take(URL_MAX - 1).collect();
        format!("{head}\u{2026}")
    }

    /// Build the live status cluster: the committed URL, the lifecycle state, the
    /// http/https security state, and the ad-filter shield (a `0` count stays
    /// hidden, §7).
    fn build_status(s: &Snapshot) -> Vec<StatusChip> {
        let mut chips = Vec::new();
        if s.has_tab && !s.url.trim().is_empty() {
            chips.push(StatusChip::new(truncate_url(&s.url), ChipTone::Neutral));
        }
        chips.push(state_chip(s));
        if let Some(chip) = security_chip(s) {
            chips.push(chip);
        }
        if s.blocked > 0 {
            chips.push(StatusChip::with_icon(
                SHIELD,
                s.blocked.to_string(),
                ChipTone::Warn,
            ));
        }
        chips
    }

    /// Render the BROWSER bar and return the action the operator picked this frame.
    pub(super) fn show(state: &WebState, ui: &mut egui::Ui) -> Option<MenuAction> {
        let snap = snapshot(state);
        let menus = build_menus(&snap);
        let status = build_status(&snap);
        let model = MenuBarModel {
            // The dock groups Browser under **Terminals** (teal), so the title
            // wears that categorical accent (lock 2).
            title: "Browser",
            accent: Style::ACCENT_TERMINALS,
            menus: &menus,
            status: &status,
        };
        MenuBar::show(ui, &model)
    }

    /// The active tab's committed URL, or empty with no tab.
    fn page_url(state: &WebState) -> String {
        state
            .tabs
            .get(state.active)
            .map(|t| t.session.nav().url.clone())
            .unwrap_or_default()
    }

    /// The active tab's committed URL + title, or empties with no tab.
    fn page_url_title(state: &WebState) -> (String, String) {
        state.tabs.get(state.active).map_or_else(
            || (String::new(), String::new()),
            |t| (t.session.nav().url.clone(), t.session.title().to_owned()),
        )
    }

    /// Dispatch a picked action to its real seam (§6, no new behaviour) — the SAME
    /// seams the toolbar chrome + page-actions menu already drive.
    pub(super) fn apply(ctx: &egui::Context, state: &mut WebState, action: MenuAction) {
        match action {
            MenuAction::Back => {
                if let Some(tab) = state.active_tab() {
                    tab.session.go_back();
                }
            }
            MenuAction::Forward => {
                if let Some(tab) = state.active_tab() {
                    tab.session.go_forward();
                }
            }
            MenuAction::Reload => {
                let crashed = state
                    .tabs
                    .get(state.active)
                    .is_some_and(|t| t.session.is_crashed());
                if crashed {
                    state.respawn_requested = true;
                } else if let Some(tab) = state.active_tab() {
                    tab.session.reload();
                }
            }
            MenuAction::CopyUrl => {
                let url = page_url(state);
                if !url.trim().is_empty() {
                    ctx.copy_text(url);
                }
            }
            MenuAction::AddBookmark => {
                let (url, title) = page_url_title(state);
                if !url.trim().is_empty() {
                    publish(ACTION_BOOKMARKS_ADD, &bookmark_add_body(&url, &title));
                }
            }
            MenuAction::SendInChat => {
                let (url, title) = page_url_title(state);
                if !url.trim().is_empty() {
                    publish(
                        ACTION_CHAT_SEND,
                        &chat_share_body(&local_hostname(), &url, &title),
                    );
                }
            }
        }
    }

    #[cfg(test)]
    #[allow(
        clippy::panic,
        reason = "a let-else in a model test names the expected menu shape (house style, \
                  mirroring the shared menubar.rs tests)"
    )]
    mod tests {
        use super::{
            apply, build_menus, build_status, reload_label, security_chip, show, state_chip,
            truncate_url, MenuAction, Snapshot, WebState, URL_MAX,
        };
        use mde_egui::egui;
        use mde_egui::menubar::Entry;
        use mde_egui::{ChipTone, Style};
        use mde_web_preview_client::SessionState;

        /// A live, navigable https page (a non-crashed tab, one back entry, three
        /// blocked requests) — the model tests read their gating off it.
        fn https_page() -> Snapshot {
            Snapshot {
                has_tab: true,
                crashed: false,
                can_back: true,
                can_forward: false,
                loading: false,
                blocked: 3,
                url: "https://example.com/path".to_owned(),
                state: Some(SessionState::Live),
            }
        }

        #[test]
        fn the_menus_cover_the_real_browser_seams() {
            let menus = build_menus(&https_page());
            let titles: Vec<&str> = menus.iter().map(|m| m.title.as_str()).collect();
            assert_eq!(titles, ["Edit", "View", "History", "Bookmarks"]);
            // File (no new-tab/quit seam) and Help (no chord table) are honestly
            // omitted, not shipped as present-but-dead menus (§7).
            assert!(!titles.contains(&"File"));
            assert!(!titles.contains(&"Help"));
        }

        #[test]
        fn back_and_forward_gate_on_the_live_history() {
            // can_back = true, can_forward = false → Back enabled, Forward greyed —
            // the §7 disable, never an omission.
            let history = build_menus(&https_page())
                .into_iter()
                .find(|m| m.title == "History")
                .expect("History menu present");
            let items: Vec<(String, bool)> = history
                .entries
                .iter()
                .filter_map(|e| match e {
                    Entry::Item(i) => Some((i.label.clone(), i.enabled)),
                    _ => None,
                })
                .collect();
            assert_eq!(
                items,
                [("Back".to_owned(), true), ("Forward".to_owned(), false)]
            );
        }

        #[test]
        fn the_page_family_items_disable_without_a_live_page() {
            // No tab → every item greys (Copy URL / Reload / Back / Forward / Add
            // Bookmark / Send in Chat), all present-but-disabled, none omitted.
            let menus = build_menus(&Snapshot::default());
            for menu in &menus {
                for entry in &menu.entries {
                    if let Entry::Item(item) = entry {
                        assert!(!item.enabled, "{} greys with no live page", item.label);
                    }
                }
            }
        }

        #[test]
        fn reload_relabels_to_restart_and_stays_enabled_on_a_crashed_tab() {
            assert_eq!(reload_label(false), "Reload");
            assert_eq!(reload_label(true), "Restart Page");
            let crashed = Snapshot {
                has_tab: true,
                crashed: true,
                ..Snapshot::default()
            };
            let view = build_menus(&crashed)
                .into_iter()
                .find(|m| m.title == "View")
                .expect("View menu present");
            let Entry::Item(reload) = &view.entries[0] else {
                panic!("View[0] is Reload");
            };
            assert_eq!(reload.label, "Restart Page");
            assert!(
                reload.enabled,
                "Reload stays enabled on a crashed tab (it respawns)"
            );
        }

        #[test]
        fn the_status_cluster_reflects_the_live_page() {
            let chips = build_status(&https_page());
            let texts: Vec<&str> = chips.iter().map(|c| c.text.as_str()).collect();
            // URL · Live · https · 3 blocked, left→right.
            assert_eq!(texts[0], "https://example.com/path");
            assert!(texts.contains(&"Live"), "the lifecycle chip is present");
            assert!(texts.contains(&"https"), "the security chip is present");
            assert!(texts.contains(&"3"), "the ad-filter shield shows the count");
        }

        #[test]
        fn the_state_chip_tracks_the_session_lifecycle() {
            let live = state_chip(&https_page());
            assert_eq!(live.text, "Live");
            assert_eq!(live.tone, ChipTone::Ok);
            let loading = Snapshot {
                has_tab: true,
                loading: true,
                state: Some(SessionState::Live),
                ..Snapshot::default()
            };
            assert_eq!(state_chip(&loading).text, "Loading");
            let crashed = Snapshot {
                has_tab: true,
                state: Some(SessionState::Crashed {
                    reason: "boom".to_owned(),
                }),
                ..Snapshot::default()
            };
            assert_eq!(state_chip(&crashed).tone, ChipTone::Danger);
            // No tab → an honest neutral idle chip, never a fake "Live".
            assert_eq!(state_chip(&Snapshot::default()).tone, ChipTone::Neutral);
        }

        #[test]
        fn the_security_chip_reads_the_url_scheme() {
            assert_eq!(
                security_chip(&https_page()).expect("https chip").tone,
                ChipTone::Ok
            );
            let http = Snapshot {
                has_tab: true,
                url: "http://plain.example/".to_owned(),
                state: Some(SessionState::Live),
                ..Snapshot::default()
            };
            assert_eq!(
                security_chip(&http).expect("http chip").tone,
                ChipTone::Warn
            );
            // A schemeless address (about:blank) and no-tab both omit the chip.
            let blank = Snapshot {
                has_tab: true,
                url: "about:blank".to_owned(),
                state: Some(SessionState::Live),
                ..Snapshot::default()
            };
            assert!(security_chip(&blank).is_none());
            assert!(security_chip(&Snapshot::default()).is_none());
        }

        #[test]
        fn a_long_url_truncates_but_a_short_one_is_verbatim() {
            assert_eq!(truncate_url("https://a.co/"), "https://a.co/");
            let long = "https://example.com/a/very/long/path/that/keeps/going/and/going/on";
            let out = truncate_url(long);
            assert!(out.chars().count() <= URL_MAX, "truncated within the cap");
            assert!(
                out.ends_with('\u{2026}'),
                "a truncated URL wears an ellipsis"
            );
        }

        #[test]
        fn apply_on_an_empty_state_is_a_safe_noop() {
            let ctx = egui::Context::default();
            let mut state = WebState::default();
            for action in [
                MenuAction::Back,
                MenuAction::Forward,
                MenuAction::Reload,
                MenuAction::CopyUrl,
                MenuAction::AddBookmark,
                MenuAction::SendInChat,
            ] {
                apply(&ctx, &mut state, action);
            }
            assert!(!state.respawn_requested, "no tab → Reload is a no-op");
            assert!(state.tabs.is_empty(), "no action attaches or drops a tab");
        }

        #[test]
        fn the_browser_bar_renders_headless() {
            use egui::{pos2, vec2, Rect};
            let ctx = egui::Context::default();
            Style::install(&ctx);
            let state = WebState::default();
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1024.0, 640.0))),
                ..Default::default()
            };
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let _ = show(&state, ui);
                });
            });
            let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
            assert!(!prims.is_empty(), "the Browser bar produced no primitives");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_egui::egui::{pos2, vec2, Rect};
    use mde_web_preview_client::testkit;

    /// A headless 960×640 shell body, mirroring the VDI + shell render tests.
    fn body_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        }
    }

    /// Drive one headless frame of `web_panel` on the CPU tessellation path (the
    /// same `Context::run` → `tessellate` the DRM runner drives, minus the GPU).
    fn run_panel(state: &mut WebState) -> bool {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let out = ctx.run(body_input(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| web_panel(ui, state));
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        !prims.is_empty()
    }

    /// Run frames until the active tab uploads its texture (the fake helper's frame
    /// is already buffered, so this settles immediately).
    fn run_until_texture(state: &mut WebState) -> bool {
        for _ in 0..50 {
            run_panel(state);
            if state
                .tabs
                .get(state.active)
                .is_some_and(|t| t.texture.is_some())
            {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        false
    }

    #[test]
    fn no_session_paints_the_gated_empty_state() {
        let mut state = WebState::default();
        assert!(run_panel(&mut state), "the gated EmptyState drew nothing");
        assert!(state.tabs.is_empty());
    }

    #[test]
    fn a_paint_ready_frame_uploads_to_a_texture_and_paints() {
        let (session, _helper) = testkit::connect().expect("connect");
        let mut state = WebState::default();
        state.push_session(session);
        assert!(
            run_until_texture(&mut state),
            "no frame uploaded to a texture"
        );
        assert!(state.tabs[0].texture.is_some());
        assert!(run_panel(&mut state), "the browser image produced no draw");
    }

    #[test]
    fn a_crashed_tab_paints_the_honest_crashed_state() {
        let (session, helper) = testkit::connect().expect("connect");
        let mut state = WebState::default();
        state.push_session(session);
        run_until_texture(&mut state);

        helper.crash();
        // Poll it to observe the crash, then render the crashed body.
        assert!(run_panel(&mut state));
        assert!(state.tabs[0].session.is_crashed());
        assert!(run_panel(&mut state), "the crashed body produced no draw");
    }

    #[test]
    fn one_tabs_crash_does_not_disturb_another() {
        let (a, helper_a) = testkit::connect().expect("connect a");
        let (b, _helper_b) = testkit::connect().expect("connect b");
        let mut state = WebState::default();
        state.push_session(a); // tab 0
        state.push_session(b); // tab 1 (active)
        run_until_texture(&mut state);

        helper_a.crash();
        run_panel(&mut state); // polls all tabs
        assert!(state.tabs[0].session.is_crashed(), "tab 0 crashed");
        assert!(!state.tabs[1].session.is_crashed(), "tab 1 unaffected");
    }

    #[test]
    fn the_ad_filter_blocked_count_surfaces_on_the_active_tab() {
        use mde_web_preview_client::{
            wire, EventMsg, FilterListStore, RequestFilter, ResourceType, WebSession,
        };
        use std::io::Write as _;
        use std::os::unix::net::UnixStream;

        // A bundled-filter session over a bare socketpair (no shm needed — we only
        // drive the request-policy protocol to bump the per-page counter).
        let (shell, helper) = UnixStream::pair().expect("socketpair");
        let filter = RequestFilter::from_store(&FilterListStore::with_bundled());
        let mut session = WebSession::from_stream(shell, None)
            .expect("session")
            .with_filter(filter);

        let mut peer: &UnixStream = &helper;
        let nav = EventMsg::NavState {
            can_back: false,
            can_forward: false,
            loading: false,
            url: "https://news.example.com/".to_owned(),
        };
        peer.write_all(&wire::frame(&nav.encode())).expect("nav");
        let req = EventMsg::ResourceRequest {
            id: 1,
            url: "https://doubleclick.net/ad".to_owned(),
            resource: mde_web_preview_client::resource_to_wire(ResourceType::Image),
        };
        peer.write_all(&wire::frame(&req.encode())).expect("req");
        session.poll();
        assert_eq!(
            session.blocked_count(),
            1,
            "the tracker was blocked + counted"
        );

        let mut state = WebState::default();
        state.push_session(session);
        // The nav chrome (with the "N blocked" shield) renders without panicking.
        assert!(run_panel(&mut state), "the browser chrome produced no draw");
        assert_eq!(state.tabs[0].session.blocked_count(), 1);
    }

    #[test]
    fn reload_on_a_crashed_tab_respawns_it() {
        let (session, helper) = testkit::connect().expect("connect");
        let mut state = WebState::default();
        state.push_session(session);
        run_until_texture(&mut state);
        helper.crash();
        run_panel(&mut state);
        assert!(state.tabs[0].session.is_crashed());

        // The Reload button on a crashed tab requests a respawn; the shell swaps in
        // a fresh session (here a new fake helper) and the new page flows again.
        state.respawn_requested = true;
        assert!(state.take_respawn_request());
        let (fresh, _helper2) = testkit::connect().expect("respawn connect");
        state.respawn_active_with(fresh);
        assert!(
            !state.tabs[0].session.is_crashed(),
            "respawned session is live-ish"
        );
        assert!(
            run_until_texture(&mut state),
            "the respawned tab never uploaded a frame"
        );
    }

    // ── MENUBAR-ALL: the Browser bar dispatches its actions to real seams ───────

    #[test]
    fn the_menu_reload_on_a_live_tab_reloads_without_a_respawn() {
        // The View→Reload item on a live tab drives `WebSession::reload` (the same
        // seam the toolbar Reload button uses) and is NOT a respawn.
        let (session, _helper) = testkit::connect().expect("connect");
        let mut state = WebState::default();
        state.push_session(session);
        run_until_texture(&mut state);
        let ctx = egui::Context::default();
        super::menubar::apply(&ctx, &mut state, super::menubar::MenuAction::Reload);
        assert!(!state.respawn_requested, "a live reload is not a respawn");
        assert!(!state.tabs[0].session.is_crashed());
    }

    #[test]
    fn the_menu_reload_on_a_crashed_tab_requests_a_respawn() {
        // On a crashed tab the SAME View→Reload item becomes a respawn request —
        // byte-identical to the toolbar Reload's crashed-tab behaviour.
        let (session, helper) = testkit::connect().expect("connect");
        let mut state = WebState::default();
        state.push_session(session);
        run_until_texture(&mut state);
        helper.crash();
        run_panel(&mut state); // the poll observes the crash
        assert!(state.tabs[0].session.is_crashed());
        let ctx = egui::Context::default();
        super::menubar::apply(&ctx, &mut state, super::menubar::MenuAction::Reload);
        assert!(
            state.respawn_requested,
            "menu Reload on a crashed tab requests a respawn (like the toolbar)"
        );
    }

    // ── BOOKMARKS-10: mesh integration ─────────────────────────────────────────

    #[test]
    fn bookmark_add_body_is_the_workers_add_verb_shape() {
        let body = bookmark_add_body("https://example.com/", "Example Domain");
        let v: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(v["url"], "https://example.com/");
        assert_eq!(v["title"], "Example Domain");
        // `source` is omitted so the worker mints the default `Source::Manual`.
        assert!(v.get("source").is_none());
    }

    #[test]
    fn chat_share_body_round_trips_into_a_clipboard_message_kind() {
        let body = chat_share_body("eagle", "https://example.com/", "Example Domain");
        let v: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(v["scope"], "peer");
        assert_eq!(v["to"], "eagle");
        // Prove it's the REAL NOTIFY-CHAT message-kind (snake_case-tagged), not a
        // hand-rolled shape: the `kind` deserializes straight back into MessageKind.
        let kind: MessageKind = serde_json::from_value(v["kind"].clone()).expect("a MessageKind");
        assert!(matches!(kind, MessageKind::Clipboard { .. }));
        assert_eq!(v["kind"]["clipboard"]["preview"], "Example Domain");
        assert_eq!(v["kind"]["clipboard"]["full"], "https://example.com/");
    }

    #[test]
    fn chat_share_preview_falls_back_to_the_url_when_the_title_is_blank() {
        let body = chat_share_body("eagle", "https://example.com/", "   ");
        let v: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(v["kind"]["clipboard"]["preview"], "https://example.com/");
    }

    #[test]
    fn the_live_page_url_and_title_feed_the_page_actions() {
        let (session, _helper) = testkit::connect().expect("connect");
        let mut state = WebState::default();
        state.push_session(session);
        run_until_texture(&mut state);
        // The three page-actions all read the active session's live nav + title
        // (the testkit helper reports `about:blank` for both).
        let tab = &state.tabs[0];
        assert_eq!(tab.session.nav().url, "about:blank");
        assert_eq!(tab.session.title(), "about:blank");
        // The chrome now carrying the page-actions star menu still renders.
        assert!(
            run_panel(&mut state),
            "the page-actions chrome produced no draw"
        );
    }

    // ── live-helper: the real spawn/attach/pump glue ────────────────────────────
    //
    // Exercised through the SAME `open_with` seam the shell's Browser arm drives,
    // but with a `testkit` factory injected in place of the real `Command::spawn`,
    // so the spawn→attach→pump path runs hermetically — no Servo, no real process.

    #[cfg(feature = "live-helper")]
    #[test]
    fn live_open_spawns_attaches_and_pumps_a_frame() {
        use std::cell::RefCell;
        // Hold the fake helpers so the attached session stays live through the pump.
        let helpers: RefCell<Vec<testkit::FakeHelper>> = RefCell::new(Vec::new());
        let mut state = WebState::default();
        // A real, existing path passes the "helper installed" gate; the injected
        // factory returns a testkit session instead of exec'ing Servo.
        let bin = std::env::current_exe().expect("test exe path");
        state.open_with(true, bin, |_spec| {
            let (session, helper) = testkit::connect()?;
            helpers.borrow_mut().push(helper);
            Ok(session)
        });
        assert_eq!(
            state.tabs.len(),
            1,
            "the live open attached exactly one tab"
        );
        assert!(
            state.gate_notice.is_none(),
            "a successful open clears the gate notice"
        );
        assert!(
            run_until_texture(&mut state),
            "the live tab pumped no frame into the texture path"
        );
        assert!(state.tabs[0].texture.is_some());
    }

    #[cfg(feature = "live-helper")]
    #[test]
    fn live_open_with_no_seat_stays_the_honest_empty_state() {
        use std::cell::Cell;
        let spawned = Cell::new(false);
        let mut state = WebState::default();
        let bin = std::env::current_exe().expect("test exe path");
        state.open_with(false, bin, |_spec| {
            spawned.set(true);
            Err(std::io::Error::other(
                "factory must not be called without a seat",
            ))
        });
        assert!(!spawned.get(), "no seat must not spawn a helper");
        assert!(state.tabs.is_empty(), "no tab attaches without a seat");
        assert!(
            state.gate_notice.is_some(),
            "the no-seat gate is named honestly"
        );
        // The panel draws the honest gated EmptyState, never a fake page.
        assert!(run_panel(&mut state));
    }

    #[cfg(feature = "live-helper")]
    #[test]
    fn live_open_with_an_absent_helper_stays_the_honest_empty_state() {
        use std::cell::Cell;
        let spawned = Cell::new(false);
        let mut state = WebState::default();
        let missing = std::path::PathBuf::from("/nonexistent/mde-web-preview-xyz");
        state.open_with(true, missing, |_spec| {
            spawned.set(true);
            Err(std::io::Error::other(
                "factory must not run with an absent helper",
            ))
        });
        assert!(!spawned.get(), "an absent helper binary must not spawn");
        assert!(state.tabs.is_empty());
        let notice = state.gate_notice.as_deref().unwrap_or_default();
        assert!(
            notice.contains("not installed"),
            "the absent-helper gate names it honestly: {notice}"
        );
        assert!(run_panel(&mut state));
    }

    #[cfg(feature = "live-helper")]
    #[test]
    fn a_spawn_failure_surfaces_an_honest_reason_not_a_hang() {
        let mut state = WebState::default();
        let bin = std::env::current_exe().expect("test exe path");
        state.open_with(true, bin, |_spec| {
            Err(std::io::Error::other("exec denied by sandbox"))
        });
        assert!(state.tabs.is_empty(), "a failed spawn attaches no tab");
        let notice = state.gate_notice.as_deref().unwrap_or_default();
        assert!(
            notice.contains("failed to start") && notice.contains("exec denied"),
            "a spawn failure surfaces its reason: {notice}"
        );
        assert!(run_panel(&mut state), "the honest failure notice draws");
    }

    #[cfg(feature = "live-helper")]
    #[test]
    fn helper_bin_path_defaults_and_honors_the_env_override() {
        use std::path::PathBuf;
        std::env::remove_var(HELPER_BIN_ENV);
        assert_eq!(helper_bin_path(), PathBuf::from(DEFAULT_HELPER_BIN));
        std::env::set_var(HELPER_BIN_ENV, "/opt/mde/mde-web-preview");
        assert_eq!(helper_bin_path(), PathBuf::from("/opt/mde/mde-web-preview"));
        std::env::remove_var(HELPER_BIN_ENV);
    }
}
