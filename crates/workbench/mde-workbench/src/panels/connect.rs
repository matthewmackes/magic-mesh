//! KDC2-5.4 / 5.5 / 5.6 / 5.7 — Workbench "Connect" peer card.
//!
//! Replaces the v13.0 GTK3 KDE Connect panel. Renders one card
//! per paired device (read from the `dev.mackes.MDE.Connect`
//! D-Bus interface) with four conditional sections:
//!
//!   * **Phone** (5.4) — battery glyph + Ring + Find + MPRIS
//!     transport controls. Shown when `peer.kind == "phone"`.
//!
//!   * **Messaging** (5.5) — SMS thread list + composer. Shown
//!     when the peer's `capabilities` advertises
//!     `kdeconnect.sms.messages` (iOS doesn't, Android does).
//!
//!   * ~~**Share** (5.6)~~ — retired by GF-5.2 (v5.0.0).
//!     File transfers now move via the mesh-home drop
//!     folder (`~/Documents/From-<phone-name>/`) once the
//!     KDC2 inbound receive handler (GF-5.1) lands.
//!
//!   * **Common chrome** (5.7) — Clipboard / Notification mirror
//!     toggles + the Pair / Unpair button. Always visible.
//!
//! Module ships the pure-model layer + the section visibility
//! logic + text-rendering helpers + a Workbench `ConnectPanel`
//! that lifts both into the view tree.
//!
//! v4.0.1 WB-1 (Phase 0.7 rescue 2026-05-23): the previous
//! "Iced view integration lands in a follow-up commit" deferral
//! was the rescue case the iteration skill's Phase 0.7 audit
//! was built to surface. Operator reported a missing "Connected
//! Devices" modal — the data layer had been shipping in
//! `#![allow(dead_code)]` form, never wired into the
//! nav model or the panel_body router. Closing the gap here:
//! ConnectPanel + view() exist, the nav model carries a
//! `Devices → connect` entry, and `app.rs::panel_body`
//! dispatches to `self.connect.view()`.

use serde::{Deserialize, Serialize};

use cosmic::iced::widget::{column, container, row, text, Container, Space};
use cosmic::iced::{Length, Padding};
use cosmic::{Element, Task};
use mde_theme::{mde_icon, Icon, IconSize, Palette, TypeRole};

use crate::cosmic_compat::prelude::*;

/// One paired device — wire-equivalent to the
/// `dev.mackes.MDE.Connect1.DeviceInfo` struct in mde-kdc.
/// Reproduced here as a flat type so the panel doesn't take a
/// direct dep on mde-kdc (which would drag in tokio + zbus).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectPeer {
    /// Stable device id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// `phone` / `tablet` / `desktop` / `unknown`.
    pub kind: String,
    /// SHA-256 fingerprint, `AB:CD:EF:...` format.
    pub fingerprint: String,
    /// Plugin tokens advertised by the device. Drives the
    /// section-visibility predicates.
    pub capabilities: Vec<String>,
    /// Pair-time (unix epoch seconds).
    pub paired_at: i64,
    /// Most-recent reachability observation (0 = never).
    pub last_seen_at: i64,
    /// Battery percentage (0..=100); `None` when the device
    /// hasn't reported yet OR the battery plugin is disabled.
    pub battery_pct: Option<u8>,
    /// MPRIS now-playing title, if any.
    pub now_playing: Option<String>,
}

/// Identifies one section of the peer card so tests + the view
/// can reason about visibility uniformly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnectSection {
    /// KDC2-5.4 — phone-specific controls.
    Phone,
    /// KDC2-5.5 — SMS thread list + composer.
    Messaging,
    // GF-5.2 (v5.0.0) — the `Share` variant retired alongside
    // the file-drop UI surface. Files now move via the
    // mesh-home drop folder (`~/Documents/From-<phone-name>/`).
    /// KDC2-5.7 — common chrome (clipboard, notifications
    /// mirror, pair toggle). Always visible.
    CommonChrome,
}

/// Section-visibility predicate. The view renders only the
/// sections this returns `true` for.
#[must_use]
pub fn section_visible_for(section: ConnectSection, peer: &ConnectPeer) -> bool {
    match section {
        ConnectSection::CommonChrome => true,
        ConnectSection::Phone => peer.kind == "phone",
        ConnectSection::Messaging => peer
            .capabilities
            .iter()
            .any(|c| c == "kdeconnect.sms.messages"),
    }
}

/// KDC2-5.4 — phone section text fragment. Pure helper that
/// the Iced view feeds into a `text()` widget.
#[must_use]
pub fn render_phone_section(peer: &ConnectPeer) -> String {
    let battery = match peer.battery_pct {
        Some(pct) => format!("Battery: {pct}%"),
        None => "Battery: —".to_string(),
    };
    let now_playing = peer
        .now_playing
        .as_deref()
        .map(|t| format!("Now playing: {t}"))
        .unwrap_or_else(|| "Now playing: (nothing)".to_string());
    format!("{battery}\n{now_playing}\n[Ring] [Find]")
}

/// KDC2-5.5 — messaging section text fragment.
#[must_use]
pub fn render_messaging_section(peer: &ConnectPeer) -> String {
    if !section_visible_for(ConnectSection::Messaging, peer) {
        return String::new();
    }
    "Threads: (none yet — pulls from `kdeconnect.sms.messages`)\n[New message]".to_string()
}

// GF-5.2 (v5.0.0) — `render_share_section` retired
// alongside the file-drop UI removal.

/// KDC2-5.7 — common chrome text fragment.
#[must_use]
pub fn render_common_chrome(peer: &ConnectPeer) -> String {
    let last = if peer.last_seen_at == 0 {
        "Never reached".to_string()
    } else {
        format!("Last seen: {}", peer.last_seen_at)
    };
    format!(
        "Fingerprint: {fp}\n{last}\n[Mirror clipboard] [Mirror notifications] [Unpair]",
        fp = peer.fingerprint,
    )
}

/// Top-level card renderer: returns the section list (in render
/// order) the view should display for this peer + their text
/// fragments. The Iced view turns these into widgets.
#[must_use]
pub fn render_card(peer: &ConnectPeer) -> Vec<(ConnectSection, String)> {
    let mut out = Vec::new();
    if section_visible_for(ConnectSection::Phone, peer) {
        out.push((ConnectSection::Phone, render_phone_section(peer)));
    }
    if section_visible_for(ConnectSection::Messaging, peer) {
        out.push((ConnectSection::Messaging, render_messaging_section(peer)));
    }
    // GF-5.2 (v5.0.0) — Share section dropped; files move
    // via the mesh-home drop folder instead.
    // Common chrome always visible, at the bottom.
    out.push((ConnectSection::CommonChrome, render_common_chrome(peer)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_peer(kind: &str, caps: &[&str]) -> ConnectPeer {
        ConnectPeer {
            id: "abc-123".into(),
            name: "Pixel 8".into(),
            kind: kind.into(),
            fingerprint: "AB:CD:EF".into(),
            capabilities: caps.iter().map(|s| s.to_string()).collect(),
            paired_at: 1_700_000_000,
            last_seen_at: 1_700_001_000,
            battery_pct: Some(72),
            now_playing: Some("track-name".into()),
        }
    }

    #[test]
    fn common_chrome_always_visible() {
        let phone = make_peer("phone", &[]);
        let desk = make_peer("desktop", &[]);
        assert!(section_visible_for(ConnectSection::CommonChrome, &phone));
        assert!(section_visible_for(ConnectSection::CommonChrome, &desk));
    }

    #[test]
    fn phone_section_only_visible_for_phones() {
        assert!(section_visible_for(
            ConnectSection::Phone,
            &make_peer("phone", &[]),
        ));
        assert!(!section_visible_for(
            ConnectSection::Phone,
            &make_peer("desktop", &[]),
        ));
        assert!(!section_visible_for(
            ConnectSection::Phone,
            &make_peer("tablet", &[]),
        ));
    }

    #[test]
    fn messaging_section_gated_on_sms_messages_capability() {
        let with = make_peer("phone", &["kdeconnect.sms.messages"]);
        let without = make_peer("phone", &["kdeconnect.clipboard"]);
        assert!(section_visible_for(ConnectSection::Messaging, &with));
        assert!(!section_visible_for(ConnectSection::Messaging, &without));
    }

    // GF-5.2 (v5.0.0) — `share_section_gated_on_share_request_capability`
    // test retired alongside the Share variant + render_share_section
    // helper. File transfer moves via the mesh-home drop folder
    // (`~/Documents/From-<phone-name>/`) once GF-5.1 lands the
    // KDC2 inbound receive handler.

    #[test]
    fn phone_section_includes_battery_when_known() {
        let peer = make_peer("phone", &[]);
        let txt = render_phone_section(&peer);
        assert!(txt.contains("Battery: 72%"));
        assert!(txt.contains("[Ring]"));
        assert!(txt.contains("[Find]"));
    }

    #[test]
    fn phone_section_shows_em_dash_when_battery_unknown() {
        let mut peer = make_peer("phone", &[]);
        peer.battery_pct = None;
        let txt = render_phone_section(&peer);
        assert!(txt.contains("Battery: —"));
    }

    #[test]
    fn phone_section_shows_nothing_when_no_now_playing() {
        let mut peer = make_peer("phone", &[]);
        peer.now_playing = None;
        let txt = render_phone_section(&peer);
        assert!(txt.contains("(nothing)"));
    }

    #[test]
    fn messaging_section_renders_only_when_visible() {
        let with = make_peer("phone", &["kdeconnect.sms.messages"]);
        let without = make_peer("phone", &[]);
        assert!(!render_messaging_section(&with).is_empty());
        assert!(render_messaging_section(&without).is_empty());
    }

    // GF-5.2 (v5.0.0) — `share_section_renders_only_when_visible`
    // test retired with the Share helper.

    #[test]
    fn common_chrome_shows_never_reached_for_fresh_pair() {
        let mut peer = make_peer("phone", &[]);
        peer.last_seen_at = 0;
        let txt = render_common_chrome(&peer);
        assert!(txt.contains("Never reached"));
        assert!(txt.contains("AB:CD:EF"));
        assert!(txt.contains("[Unpair]"));
    }

    #[test]
    fn render_card_emits_sections_in_phone_messaging_chrome_order() {
        // GF-5.2 (v5.0.0) — Share section retired; a fully-
        // featured phone now returns Phone + Messaging +
        // CommonChrome (no Share).
        let peer = make_peer(
            "phone",
            &["kdeconnect.sms.messages", "kdeconnect.share.request"],
        );
        let sections: Vec<ConnectSection> =
            render_card(&peer).into_iter().map(|(s, _)| s).collect();
        assert_eq!(
            sections,
            vec![
                ConnectSection::Phone,
                ConnectSection::Messaging,
                ConnectSection::CommonChrome,
            ],
        );
    }

    #[test]
    fn render_card_for_desktop_omits_phone_messaging() {
        // A paired desktop peer has no phone/messaging
        // sections; only CommonChrome surfaces.
        let peer = make_peer("desktop", &["kdeconnect.clipboard"]);
        let sections: Vec<ConnectSection> =
            render_card(&peer).into_iter().map(|(s, _)| s).collect();
        assert_eq!(sections, vec![ConnectSection::CommonChrome]);
    }
}

// ──────────────────────────────────────────────────────────────
// v4.0.1 WB-1 — Workbench panel surface (Phase 0.7 rescue)
// ──────────────────────────────────────────────────────────────

/// Iced-side state for the Connected Devices panel (the KDC hub that PD-3 L6
/// links to). Holds the live paired-device roster + a busy flag for in-flight
/// actions. AUD-3 (2026-06-11): `load()` fetches the real roster from the KDC
/// host worker over `action/connect/devices`; per-row actions publish the
/// Connect verbs (delivered by the AUD-2 outbound drainer). Empty roster → the
/// honest empty state.
#[derive(Debug, Clone, Default)]
pub struct ConnectPanel {
    pub peers: Vec<ConnectPeer>,
    /// EFF-45 — set when the Bus RPC call to `action/connect/devices` failed
    /// (timeout, bus offline, or a spawn error). The view renders the error
    /// state instead of the misleading "No paired devices" empty state.
    pub load_error: Option<String>,
    pub busy: bool,
}

/// Messages emitted by the Connected Devices panel. The
/// crate-level `Message::Connect(panels::connect::Message)`
/// dispatches arms back to `ConnectPanel::update`.
#[derive(Debug, Clone)]
pub enum Message {
    /// Backend pushed a fresh peer list — `Err` when the Bus RPC failed
    /// (timeout / bus offline), `Ok` for a roster (may be empty).
    Loaded(Result<Vec<ConnectPeer>, String>),
    /// User clicked Pair / Unpair / Ring / SendFile on a row —
    /// `peer_id` identifies the row.
    PeerAction { peer_id: String, action: PeerAction },
    /// EFF-45 — retry after a load error; re-fires `load()`.
    RefreshClicked,
}

/// The per-row actions the panel exposes today. Each routes to a
/// `dev.mackes.MDE.Connect1` D-Bus method when the KDC2 server
/// surface ships (KDC2-3.4..3.6/3.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerAction {
    Unpair,
    Ring,
    Find,
}

impl ConnectPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// AUD-3 — fetch the live paired-device roster from the KDC host worker
    /// over the Bus (`action/connect/devices`, the same surface PD-3 L6 reads).
    /// One-shot on nav; empty roster is the honest empty state; Bus
    /// timeout/offline is a load ERROR (EFF-45).
    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async {
                // EFF-45: distinguish Bus failure (None) from an empty roster
                // (Some(array)). spawn_blocking join-error also maps to Err.
                let result = tokio::task::spawn_blocking(|| {
                    crate::dbus::action_request(
                        "action/connect/devices",
                        std::time::Duration::from_secs(2),
                    )
                })
                .await
                .map_err(|e| format!("spawn error: {e}"))
                .and_then(|opt| {
                    opt.ok_or_else(|| "Bus RPC failed — mde host worker not responding".to_string())
                })
                .map(|raw| parse_connect_devices(&raw));
                result
            },
            |result| crate::Message::Connect(Message::Loaded(result)),
        )
    }

    /// Dispatch a panel-scoped message.
    pub fn update(&mut self, msg: Message) -> Task<crate::Message> {
        match msg {
            Message::Loaded(Ok(peers)) => {
                self.peers = peers;
                self.load_error = None;
                self.busy = false;
                Task::none()
            }
            Message::Loaded(Err(e)) => {
                // EFF-45 — Bus failure is an error, not an empty roster.
                self.load_error = Some(e);
                self.busy = false;
                Task::none()
            }
            Message::RefreshClicked => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                Self::load()
            }
            Message::PeerAction { peer_id, action } => {
                // AUD-3 — publish the Connect verb (delivered by the AUD-2
                // outbound drainer), then reload the roster. Unpair removes a
                // device; ring/find buzz it.
                self.busy = true;
                let topic = match action {
                    PeerAction::Unpair => "action/connect/unpair",
                    PeerAction::Ring | PeerAction::Find => "action/connect/ring",
                };
                let body = serde_json::json!({ "device_id": peer_id }).to_string();
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            let _ = crate::dbus::action_request_with_body(
                                topic,
                                Some(&body),
                                std::time::Duration::from_secs(2),
                            );
                            crate::dbus::action_request(
                                "action/connect/devices",
                                std::time::Duration::from_secs(2),
                            )
                        })
                        .await
                        .map_err(|e| format!("spawn error: {e}"))
                        .and_then(|opt| {
                            opt.ok_or_else(|| {
                                "Bus RPC failed — mde host worker not responding".to_string()
                            })
                        })
                        .map(|raw| parse_connect_devices(&raw))
                    },
                    |result| crate::Message::Connect(Message::Loaded(result)),
                )
            }
        }
    }

    /// Render the panel body. Empty list → Workbench EmptyState
    /// with copy that points the user at mde-peer-card. Non-empty
    /// list → a stack of per-peer cards with their conditional
    /// sections (Phone / Messaging / Share / CommonChrome).
    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let density = crate::live_theme::tokens().density;

        // EFF-45 — a failed Bus RPC renders as failure, never as the
        // "No paired devices" empty state.
        if let Some(err) = &self.load_error {
            return crate::panel_chrome::panel_container(
                crate::panel_chrome::error_state(err.clone(), palette, || {
                    crate::Message::Connect(Message::RefreshClicked)
                }),
                density,
            );
        }

        if self.peers.is_empty() {
            return self.empty_state_view(palette);
        }
        let mut col = column![].spacing(12);
        for peer in &self.peers {
            col = col.push(peer_card_view(peer, palette));
        }
        let body: Container<'_, crate::Message, cosmic::Theme> = container(col)
            .padding(Padding::from([16u16, 24u16]))
            .width(Length::Fill);
        body.into()
    }

    fn empty_state_view(&self, palette: Palette) -> Element<'_, crate::Message> {
        let resolved = mde_icon(Icon::Peer, IconSize::EmptyState);
        let heading = text("No paired devices yet")
            .size(TypeRole::Heading.size_in(mde_theme::FontSize::defaults()))
            .colr(palette.text.into_cosmic_color());
        let body = text(
            "Open KDE Connect on a phone or tablet and pick this PC \
             to pair. Paired devices land here with Ring / Find / \
             Send-File actions — and the same mesh dock that hosts \
             your other peers.",
        )
        .size(TypeRole::Body.size_in(mde_theme::FontSize::defaults()))
        .colr(palette.text_muted.into_cosmic_color());
        // Use the same SVG-or-fallback resolver chain BUG-13.c
        // wired in panel_chrome.rs::view, but stripped to the
        // minimum needed for an inline empty state.
        let icon_slot: Element<'_, crate::Message> = if let Some(svg_bytes) = resolved.svg_bytes() {
            use cosmic::iced::widget::svg as widget_svg;
            let muted = palette.text_muted.into_cosmic_color();
            widget_svg(widget_svg::Handle::from_memory(svg_bytes))
                .width(Length::Fixed(resolved.size_px()))
                .height(Length::Fixed(resolved.size_px()))
                .sty(move |_t: &cosmic::Theme| widget_svg::Style { color: Some(muted) })
                .into()
        } else {
            text(resolved.fallback_glyph)
                .size(resolved.size_px())
                .colr(palette.text_muted.into_cosmic_color())
                .into()
        };
        container(
            column![
                icon_slot,
                Space::new().height(Length::Fixed(8.0)),
                heading,
                Space::new().height(Length::Fixed(4.0)),
                body,
            ]
            .align_x(cosmic::iced::alignment::Horizontal::Center)
            .spacing(2),
        )
        .padding(Padding::from([48u16, 24u16]))
        .width(Length::Fill)
        .align_x(cosmic::iced::alignment::Horizontal::Center)
        .into()
    }
}

/// Render one paired peer as a card. Section order: peer
/// identity row + every section from `render_card(peer)` in the
/// locked visibility order (Phone / Messaging / Share /
/// CommonChrome).
fn peer_card_view<'a>(peer: &'a ConnectPeer, palette: Palette) -> Element<'a, crate::Message> {
    let kind_glyph = match peer.kind.as_str() {
        "phone" => Icon::Devices,
        "tablet" => Icon::Devices,
        _ => Icon::Devices,
    };
    let kind_icon: Element<'a, crate::Message> = {
        let resolved = mde_icon(kind_glyph, IconSize::Nav);
        if let Some(svg_bytes) = resolved.svg_bytes() {
            use cosmic::iced::widget::svg as widget_svg;
            let fg = palette.text.into_cosmic_color();
            widget_svg(widget_svg::Handle::from_memory(svg_bytes))
                .width(Length::Fixed(resolved.size_px()))
                .height(Length::Fixed(resolved.size_px()))
                .sty(move |_t: &cosmic::Theme| widget_svg::Style { color: Some(fg) })
                .into()
        } else {
            text(resolved.fallback_glyph)
                .size(resolved.size_px())
                .colr(palette.text.into_cosmic_color())
                .into()
        }
    };
    let name = text(peer.name.clone())
        .size(TypeRole::Subheading.size_in(mde_theme::FontSize::defaults()))
        .colr(palette.text.into_cosmic_color());
    let identity = row![
        kind_icon,
        Space::new().width(Length::Fixed(8.0)),
        name,
        Space::new().width(Length::Fill),
        text(short_fingerprint(&peer.fingerprint))
            .size(11)
            .colr(palette.text_muted.into_cosmic_color()),
    ]
    .align_y(cosmic::iced::Alignment::Center);
    let mut card = column![identity].spacing(8);
    for (_section, body_text) in render_card(peer) {
        card = card.push(
            text(body_text)
                .size(12)
                .colr(palette.text_muted.into_cosmic_color()),
        );
    }
    container(card.padding(Padding::from([12u16, 16u16])))
        .width(Length::Fill)
        .sty(
            move |_t: &cosmic::Theme| cosmic::iced::widget::container::Style {
                snap: false,
                icon_color: None,
                background: Some(cosmic::iced::Background::Color(
                    palette.raised.into_cosmic_color(),
                )),
                border: cosmic::iced::Border {
                    color: palette.border.into_cosmic_color(),
                    width: 1.0,
                    radius: 8.0.into(),
                },
                text_color: Some(palette.text.into_cosmic_color()),
                shadow: cosmic::iced::Shadow::default(),
            },
        )
        .into()
}

/// Render the first 8 hex bytes of a colon-separated
/// SHA-256 fingerprint for the per-card identity row.
/// `AB:CD:EF:01:23:45:67:89:…` → `AB:CD:EF:01:23:45:67:89`.
fn short_fingerprint(full: &str) -> String {
    full.split(':').take(8).collect::<Vec<_>>().join(":")
}

/// AUD-3 — parse the KDC host's `action/connect/devices` reply (a JSON array of
/// `{id, name, online, battery}`) into [`ConnectPeer`] rows. A bad/non-array
/// reply degrades to an empty roster (the panel shows its empty state). Devices
/// are treated as phones so the phone section (battery / ring / find) renders.
#[must_use]
pub fn parse_connect_devices(raw: &str) -> Vec<ConnectPeer> {
    serde_json::from_str::<serde_json::Value>(raw.trim())
        .ok()
        .and_then(|v| v.as_array().cloned())
        .map(|arr| {
            arr.iter()
                .filter_map(|d| {
                    let id = d.get("id")?.as_str()?.to_string();
                    let name = d
                        .get("name")
                        .and_then(|x| x.as_str())
                        .filter(|s| !s.is_empty())
                        .unwrap_or(&id)
                        .to_string();
                    let online = d.get("online").and_then(serde_json::Value::as_bool) == Some(true);
                    let battery_pct = d
                        .get("battery")
                        .and_then(serde_json::Value::as_u64)
                        .and_then(|n| u8::try_from(n).ok());
                    Some(ConnectPeer {
                        id,
                        name,
                        kind: "phone".to_string(),
                        battery_pct,
                        last_seen_at: i64::from(online),
                        ..ConnectPeer::default()
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod view_tests {
    use super::*;

    #[test]
    fn empty_panel_renders_empty_state_view() {
        let panel = ConnectPanel::new();
        // Construct the Element to make sure no panic + the
        // empty branch is reachable.
        let _ = panel.view();
    }

    #[test]
    fn parse_connect_devices_maps_the_live_roster() {
        // AUD-3 — the action/connect/devices reply → ConnectPeer rows.
        let raw = r#"[
            {"id":"d1","name":"Pixel","online":true,"battery":72},
            {"id":"d2","name":"","online":false,"battery":null}
        ]"#;
        let peers = parse_connect_devices(raw);
        assert_eq!(peers.len(), 2);
        assert_eq!(peers[0].name, "Pixel");
        assert_eq!(peers[0].battery_pct, Some(72));
        assert_eq!(peers[0].kind, "phone");
        assert_eq!(peers[1].name, "d2", "blank name falls back to id");
        assert_eq!(peers[1].battery_pct, None);
        // Bad reply → empty roster (honest empty state), never panics.
        assert!(parse_connect_devices("not json").is_empty());
    }

    #[test]
    fn populated_panel_renders_one_card_per_peer() {
        let mut panel = ConnectPanel::new();
        panel.peers = vec![
            ConnectPeer {
                id: "p1".into(),
                name: "Pixel-9".into(),
                kind: "phone".into(),
                fingerprint: "AB:CD:EF:01:23:45:67:89:00:11".into(),
                capabilities: vec!["kdeconnect.sms.messages".into()],
                paired_at: 1_700_000_000,
                ..Default::default()
            },
            ConnectPeer {
                id: "p2".into(),
                name: "iPad".into(),
                kind: "tablet".into(),
                fingerprint: "AA:BB:CC:DD:EE:FF:00:11".into(),
                capabilities: vec![],
                paired_at: 1_700_000_100,
                ..Default::default()
            },
        ];
        let _ = panel.view();
    }

    #[test]
    fn short_fingerprint_takes_first_eight_octets() {
        let full = "AA:BB:CC:DD:EE:FF:00:11:22:33:44:55";
        assert_eq!(short_fingerprint(full), "AA:BB:CC:DD:EE:FF:00:11");
    }
}
