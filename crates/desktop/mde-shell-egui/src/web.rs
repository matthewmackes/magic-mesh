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
        not(test),
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
        not(test),
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

    // 3. The navigation chrome (back / forward / reload / address bar), wired to
    //    the active session's control socket.
    nav_chrome(ui, state);
    ui.add_space(Style::SP_XS);

    // 4. The body. Read the active tab's status first so the crashed arm can set
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
        None => empty_body(ui),
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

/// The no-session `EmptyState` — an honest gated caption, never a placeholder page.
fn empty_body(ui: &mut egui::Ui) {
    centered(ui, |ui| {
        ui.label(
            RichText::new("Sandboxed browser")
                .size(Style::HEADING)
                .color(Style::TEXT),
        );
        ui.add_space(Style::SP_S);
        muted_note(
            ui,
            "The sandboxed Servo browser renders here in the shell. A live session \
             attaches on a GPU seat (BOOKMARKS-5/6 live path is gated).",
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
}
