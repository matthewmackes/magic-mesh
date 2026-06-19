//! LIGHTHOUSE-5 — Mesh ▸ Lighthouses tab.
//!
//! A dedicated operations surface for the relay/anchor nodes of the overlay.
//! It reuses the shared `mackes_mesh_types::lighthouse` discovery + binary-health
//! derivation (the same source the Notification Hub footer animates), so the tab
//! and the Hub always agree on which lighthouse is green/red and which is the
//! lizardfs master.
//!
//! Top: a Nebula hero band (Q25/Q4) + a row of the animated beacons summarizing
//! fleet lighthouse health. Below: one **full card** per lighthouse with the
//! data the replicated directory actually carries — overlay IP, master/shadow
//! badge + failover readiness (Q22), binary status, the raw health tier,
//! presence (last-seen age), a service summary, and the installed version. The
//! deep-link `focus` (from the Hub footer press) highlights + lists the clicked
//! lighthouse first. The beam + a periodic refresh run only while this tab is
//! active (wired in `app::subscription`).
//!
//! Full-ops actions (confirmed restart / SSH / promote-shadow) are LIGHTHOUSE-6;
//! handshake/cert-expiry/uptime need a per-lighthouse probe lane that the
//! replicated directory does not carry today (tracked as a LIGHTHOUSE follow-on)
//! and are deliberately omitted rather than stubbed (§7).

use cosmic::iced::widget::{column, container, row, scrollable, text, Space};
use cosmic::iced::{Length, Padding, Task};
use cosmic::Element;
use mackes_mesh_types::lighthouse::{self, Beacon};
use mde_theme::{FontSize, Palette, TypeRole};

use crate::cosmic_compat::prelude::*;

/// Full display data for one lighthouse card, derived from its replicated
/// directory row + the leader-lease master fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LighthouseCard {
    /// Identity + binary health (hostname, overlay IP, master flag, status).
    pub beacon: Beacon,
    /// The raw Netdata-derived health tier (`healthy`/`degraded`/…).
    pub health: String,
    /// Seconds since this lighthouse last refreshed its directory row.
    pub last_seen_age_s: u64,
    /// One-line service summary from the published descriptors.
    pub services: String,
    /// Installed `magic-mesh` version, if recorded.
    pub mde_version: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct LighthousesPanel {
    pub cards: Vec<LighthouseCard>,
    /// Beam-animation phase (advanced by `BeamTick` while the tab is active).
    pub beam_step: u16,
    /// The deep-linked lighthouse to highlight + list first (Q20).
    pub focus: Option<String>,
    /// Set once the first load has returned (distinguishes "loading" from
    /// "genuinely no lighthouses enrolled").
    pub loaded: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Vec<LighthouseCard>),
    Refresh,
    BeamTick,
    /// Highlight + scroll a specific lighthouse (deep-link focus).
    Focus(String),
}

impl LighthousesPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(async { load_cards() }, |cards| {
            crate::Message::Lighthouses(Message::Loaded(cards))
        })
    }

    /// Set the deep-link focus target (the Hub footer passed `lighthouses:<host>`).
    pub fn set_focus(&mut self, host: &str) {
        self.focus = Some(host.to_string());
    }

    pub fn update(&mut self, msg: Message) -> Task<crate::Message> {
        match msg {
            Message::Loaded(cards) => {
                self.cards = cards;
                self.loaded = true;
                Task::none()
            }
            Message::Refresh => Self::load(),
            Message::BeamTick => {
                self.beam_step = self.beam_step.wrapping_add(1);
                Task::none()
            }
            Message::Focus(host) => {
                self.focus = Some(host);
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let sizes = FontSize::defaults();

        let title = text("Lighthouses")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());
        let subtitle = text("overlay anchor nodes — relay + storage master")
            .size(TypeRole::Body.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color());

        // Hero band (Nebula line-art) + the summarizing animated beacon row (Q25).
        let hero = crate::panel_chrome::hero_band(
            mde_theme::hero::Hero::Nebula,
            crate::panel_chrome::pkg_version_cached("nebula").as_deref(),
            palette,
        );
        let (healthy, total) = (
            self.cards.iter().filter(|c| c.beacon.healthy()).count(),
            self.cards.len(),
        );
        let count_color = if healthy == total && total > 0 {
            palette.beacon_healthy
        } else {
            palette.danger
        };
        let beacon_strip = row(self
            .cards
            .iter()
            .map(|c| beacon_glyph(&c.beacon, self.beam_step, palette))
            .collect::<Vec<_>>())
        .spacing(10)
        .align_y(cosmic::iced::alignment::Vertical::Center);
        let hero_row = row![
            hero,
            Space::new().width(Length::Fixed(16.0)),
            column![
                beacon_strip,
                text(format!("{healthy}/{total} healthy"))
                    .size(12)
                    .colr(count_color.into_cosmic_color()),
            ]
            .spacing(8),
        ]
        .align_y(cosmic::iced::alignment::Vertical::Center);

        let header = row![
            column![title, subtitle].spacing(2),
            Space::new().width(Length::Fill),
        ]
        .align_y(cosmic::iced::alignment::Vertical::Center);

        // Cards, focus-first (Q20): the deep-linked lighthouse listed at the top.
        let mut ordered: Vec<&LighthouseCard> = self.cards.iter().collect();
        if let Some(f) = &self.focus {
            ordered.sort_by_key(|c| c.beacon.hostname != *f);
        }
        let mut body_col = column![].spacing(10);
        if self.cards.is_empty() {
            let msg = if self.loaded {
                "No lighthouses enrolled yet."
            } else {
                "Loading lighthouses…"
            };
            body_col = body_col.push(
                text(msg)
                    .size(TypeRole::Body.size_in(sizes))
                    .colr(palette.text_muted.into_cosmic_color()),
            );
        } else {
            for c in ordered {
                let focused = self.focus.as_deref() == Some(c.beacon.hostname.as_str());
                body_col = body_col.push(lighthouse_card(c, self.beam_step, focused, palette));
            }
        }

        container(
            column![
                header,
                Space::new().height(Length::Fixed(12.0)),
                hero_row,
                Space::new().height(Length::Fixed(16.0)),
                scrollable(body_col).height(Length::Fill),
            ]
            .spacing(2),
        )
        .padding(Padding::from([24u16, 32u16]))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

/// A small animated beacon glyph for the summary strip (Q25).
fn beacon_glyph<'a>(b: &Beacon, beam_step: u16, p: Palette) -> Element<'a, crate::Message> {
    let color = if b.healthy() {
        p.beacon_healthy
    } else {
        p.danger
    };
    container(
        text(lighthouse::beam_frame(b.healthy(), beam_step).to_string())
            .size(18)
            .colr(color.into_cosmic_color()),
    )
    .center_x(Length::Fixed(40.0))
    .center_y(Length::Fixed(40.0))
    .into()
}

/// One full lighthouse card (Q21): the animated beam square + name/master badge,
/// then overlay IP, status, health tier, presence, services, and version.
fn lighthouse_card<'a>(
    c: &LighthouseCard,
    beam_step: u16,
    focused: bool,
    p: Palette,
) -> Element<'a, crate::Message> {
    let color = if c.beacon.healthy() {
        p.beacon_healthy
    } else {
        p.danger
    };
    let sizes = FontSize::defaults();

    let square = container(
        text(lighthouse::beam_frame(c.beacon.healthy(), beam_step).to_string())
            .size(26)
            .colr(color.into_cosmic_color()),
    )
    .center_x(Length::Fixed(64.0))
    .center_y(Length::Fixed(64.0))
    .style(
        move |_t: &cosmic::Theme| cosmic::iced::widget::container::Style {
            background: Some(cosmic::iced::Background::Color(
                p.surface.into_cosmic_color(),
            )),
            border: cosmic::iced::Border {
                color: color.into_cosmic_color(),
                width: 2.0,
                radius: 6.0.into(),
            },
            ..Default::default()
        },
    );

    let role_badge = if c.beacon.is_master {
        "MASTER"
    } else {
        "shadow"
    };
    let name_row = row![
        text(c.beacon.hostname.clone())
            .size(TypeRole::Heading.size_in(sizes))
            .colr(p.text.into_cosmic_color()),
        Space::new().width(Length::Fixed(10.0)),
        text(role_badge).size(11).colr(if c.beacon.is_master {
            p.accent.into_cosmic_color()
        } else {
            p.text_muted.into_cosmic_color()
        }),
    ]
    .align_y(cosmic::iced::alignment::Vertical::Center);

    let ip = c
        .beacon
        .overlay_ip
        .clone()
        .unwrap_or_else(|| "no overlay IP".to_string());
    let presence = if c.last_seen_age_s < 1 {
        "just now".to_string()
    } else {
        format!("{}s ago", c.last_seen_age_s)
    };
    // Failover readiness (Q22): the shadow is ready to take the master SPOF when
    // it's itself healthy; the master line states it holds the SPOF.
    let failover = if c.beacon.is_master {
        "holds the lizardfs-master SPOF".to_string()
    } else if c.beacon.healthy() {
        "ready to take over as master".to_string()
    } else {
        "NOT ready for failover".to_string()
    };

    let meta = column![
        text(format!("{}  ·  {}", c.beacon.status.word(), ip))
            .size(13)
            .colr(color.into_cosmic_color()),
        text(format!(
            "health: {}  ·  seen {}  ·  {}",
            c.health, presence, failover
        ))
        .size(12)
        .colr(p.text_muted.into_cosmic_color()),
        text(format!(
            "services: {}  ·  {}",
            if c.services.is_empty() {
                "—"
            } else {
                c.services.as_str()
            },
            c.mde_version.as_deref().unwrap_or("version unknown"),
        ))
        .size(12)
        .colr(p.text_muted.into_cosmic_color()),
    ]
    .spacing(3);

    let inner = row![
        square,
        Space::new().width(Length::Fixed(16.0)),
        name_col(name_row, meta)
    ]
    .align_y(cosmic::iced::alignment::Vertical::Center);

    // Focused card gets an accent left border; others a subtle border.
    let border_color = if focused { p.accent } else { p.border };
    container(inner)
        .padding(Padding::from([14u16, 16u16]))
        .width(Length::Fill)
        .style(
            move |_t: &cosmic::Theme| cosmic::iced::widget::container::Style {
                background: Some(cosmic::iced::Background::Color(
                    p.surface.into_cosmic_color(),
                )),
                border: cosmic::iced::Border {
                    color: border_color.into_cosmic_color(),
                    width: if focused { 2.0 } else { 1.0 },
                    radius: 8.0.into(),
                },
                ..Default::default()
            },
        )
        .into()
}

/// Stack the name row over the meta block (a tiny helper to keep `row!` tidy).
fn name_col<'a>(
    name_row: cosmic::iced::widget::Row<'a, crate::Message, cosmic::Theme>,
    meta: cosmic::iced::widget::Column<'a, crate::Message, cosmic::Theme>,
) -> Element<'a, crate::Message> {
    column![name_row, Space::new().height(Length::Fixed(6.0)), meta]
        .width(Length::Fill)
        .into()
}

/// Fill in `role` from each node's `shell-status.json` sidecar for any peer
/// whose replicated record predates the role-stamping heartbeat. The sidecar
/// (written by every node's `mesh-status` snapshot timer) already carries the
/// pinned role, so the lighthouse surfaces work mesh-wide before the new
/// `mackesd` rolls everywhere — the directory record is the canonical source,
/// this is just the back-fill. Shared by the Hub footer + this tab.
pub fn enrich_roles(root: &std::path::Path, peers: &mut [mackes_mesh_types::peers::PeerRecord]) {
    // LIGHTHOUSE-9 — the authoritative "is a lighthouse" signal is Nebula
    // membership (the static_host_map / lighthouse-hosts overlay IPs), not the
    // deployment role.toml: the anchor nodes run Server tier for storage, so
    // role==lighthouse under-reports. The root snapshot publishes the real
    // lighthouse overlay IPs at network.lighthouse_ips (world-readable
    // /run/mde/mesh-status.json); a peer whose overlay_ip is in that set IS a
    // lighthouse regardless of its role. Fall back to the shell-status sidecar
    // role for the role string when the directory record predates role-stamping.
    let lighthouse_ips: Vec<String> = std::fs::read_to_string("/run/mde/mesh-status.json")
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| {
            v.get("network")?
                .get("lighthouse_ips")?
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
        })
        .unwrap_or_default();
    for p in peers.iter_mut() {
        // Nebula membership wins: tag as lighthouse if the overlay IP matches.
        if let Some(ip) = &p.overlay_ip {
            if lighthouse_ips.iter().any(|lh| lh == ip) {
                p.role = Some(lighthouse::LIGHTHOUSE_ROLE.to_string());
                continue;
            }
        }
        if p.role.is_some() {
            continue;
        }
        let sf = root.join(&p.hostname).join("shell-status.json");
        if let Ok(text) = std::fs::read_to_string(&sf) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(r) = v.get("role").and_then(serde_json::Value::as_str) {
                    p.role = Some(r.to_string());
                }
            }
        }
    }
}

/// Build the lighthouse cards from the mesh directory + the leader.
/// SUBSTRATE-8 — peers + leader both come from `action/mesh/directory` (etcd-or-fs
/// behind the RPC) instead of a direct `/mnt/mesh-storage` read of the roster +
/// `.mackesd-leader.lock`, so the panel survives the substrate cutover.
fn load_cards() -> Vec<LighthouseCard> {
    let root = mackes_mesh_types::peers::default_workgroup_root();
    let (mut peers, master) = crate::mesh_directory::fetch_peers_and_leader();
    // Back-fill role from the shell-status sidecar (still on the replicated share)
    // for any record that predates role-stamping.
    enrich_roles(&root, &mut peers);
    let now_ms = now_ms();
    lighthouse::lighthouse_records(&peers)
        .iter()
        .map(|p| {
            let is_master = master.as_deref() == Some(p.hostname.as_str());
            let beacon = lighthouse::beacon_for(p, is_master, now_ms, lighthouse::DEFAULT_STALE_MS);
            LighthouseCard {
                beacon,
                health: p.health.clone(),
                last_seen_age_s: now_ms.saturating_sub(p.last_seen_ms) / 1000,
                services: summarize_services(p),
                mde_version: p.mde_version.clone(),
            }
        })
        .collect()
}

/// A one-line service summary from a peer's published descriptors.
fn summarize_services(p: &mackes_mesh_types::peers::PeerRecord) -> String {
    let Some(d) = &p.descriptors else {
        return String::new();
    };
    let mut parts: Vec<String> = Vec::new();
    if d.remote_access.ssh {
        parts.push("ssh".to_string());
    }
    if !d.media.is_empty() {
        parts.push(format!("media:{}", d.media.len()));
    }
    if !d.vms.is_empty() {
        parts.push(format!("vms:{}", d.vms.len()));
    }
    if !d.containers.is_empty() {
        parts.push(format!("pods:{}", d.containers.len()));
    }
    parts.join(" · ")
}

fn now_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0),
    )
    .unwrap_or(0)
}
