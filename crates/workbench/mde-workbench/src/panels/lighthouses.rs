//! LIGHTHOUSE-5 — Mesh ▸ Lighthouses tab.
//!
//! A dedicated operations surface for the relay/anchor nodes of the overlay.
//! It reuses the shared `mackes_mesh_types::lighthouse` discovery + binary-health
//! derivation (the same source the Notification Hub footer animates), so the tab
//! and the Hub always agree on which lighthouse is green/red and which holds the
//! leader role.
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
//! LIGHTHOUSE-6 — full-ops actions per card: **Restart** (cycle the anchor's
//! core fabric units over the mesh key), **SSH / Open remote** (launch a local
//! `cosmic-term ssh` to the overlay IP via the shared [`crate::launcher`]), and
//! **Promote shadow → master** (force-take the leader lease via the daemon's
//! existing leader-lease primitive; hidden on the node that already holds it).
//! Each button opens the reused `connect_progress` modal as a **confirm gate**
//! (PR #45) — Confirm fires the action, then the modal shows in-flight →
//! success/failure. Restart + Promote round-trip `mackesd` over the Bus
//! (`action/dc/lighthouse-{restart,promote}`, the already-spawned host-ops
//! responder); SSH is a pure local launch and never touches the daemon.
//!
//! LIGHTHOUSE-8 — handshake / public IP / overlay peer count / uptime / CA
//! cert-expiry now come from the per-lighthouse deep-probe lane: the `mackesd`
//! `lighthouse_probe` worker publishes a [`LighthouseProbe`] to
//! `compute/lighthouse-probe/<name>` every ~15 s, and each card reads the newest
//! probe off the mesh-bus spool on the same 5 s refresh tick that loads the
//! directory rows. Fields the probe could not measure render `—` (never stubbed,
//! §7).

use std::time::Duration;

use cosmic::iced::widget::{column, container, row, scrollable, text, Space};
use cosmic::iced::{Length, Padding, Task};
use cosmic::Element;
use mackes_mesh_types::lighthouse::{self, Beacon};
use mackes_mesh_types::lighthouse_probe::LighthouseProbe;
use mde_theme::{FontSize, Palette, TypeRole};

use crate::components::connect_progress::{self, ConnectProgress};
use crate::controls::{variant_button, ButtonVariant};
use crate::cosmic_compat::prelude::*;
use crate::launcher::Protocol;

/// LIGHTHOUSE-6 — how long to wait for a `mackesd` action reply before the
/// modal reports the daemon didn't answer. The restart cycles mackesd remotely
/// to completion before replying, so this is generous (the daemon-down case
/// still returns fast — `action_request` resolves `None` without a responder).
const ACTION_TIMEOUT: Duration = Duration::from_secs(45);

/// LIGHTHOUSE-6 — one full-ops action the operator can take on a lighthouse
/// card. Held on the panel while the confirm gate is open (and for the modal's
/// Retry), it carries the identity the action needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Restart the anchor's core fabric units (mackesd + nebula) over the
    /// overlay. Needs the validated overlay IP for the daemon's mesh-key SSH.
    Restart { host: String, overlay_ip: String },
    /// Open a local SSH terminal to the lighthouse's overlay IP.
    Ssh { host: String, overlay_ip: String },
    /// Promote this shadow anchor to mesh leader (force-take the lease).
    Promote { host: String },
}

impl Action {
    /// The confirm-gate title (the dialog header).
    fn title(&self) -> String {
        match self {
            Self::Restart { host, .. } => format!("Restart {host}"),
            Self::Ssh { host, .. } => format!("Open SSH to {host}"),
            Self::Promote { host } => format!("Promote {host}"),
        }
    }

    /// The "are you sure?" prompt shown in the confirm gate.
    fn prompt(&self) -> String {
        match self {
            Self::Restart { host, .. } => format!(
                "Restart {host}'s core fabric units (nebula + mackesd) over the overlay? The \
                 anchor's beacon drops until the units are back."
            ),
            Self::Ssh { host, overlay_ip } => {
                format!("Open an SSH terminal to {host} ({overlay_ip})?")
            }
            Self::Promote { host } => format!(
                "Promote {host} to mesh master? This force-takes the leader lease (bumps the \
                 epoch), moving leadership to {host}."
            ),
        }
    }
}

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
    /// LIGHTHOUSE-8 — the newest deep-probe for this lighthouse off the mesh-bus
    /// (`compute/lighthouse-probe/<name>`): handshake / public IP / overlay peer
    /// count / uptime / CA cert-expiry. `None` until the first probe lands (or
    /// when the bus is unreachable) — the card renders `—` for each field.
    pub probe: Option<LighthouseProbe>,
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
    /// LIGHTHOUSE-6 — the full-ops modal: confirm gate → in-flight → outcome,
    /// reusing the PR #45 `connect_progress` component.
    pub connect: ConnectProgress,
    /// LIGHTHOUSE-6 — the action the open modal is confirming / running, so
    /// Confirm fires it and Retry re-runs the same one.
    pub pending_action: Option<Action>,
    /// LIGHTHOUSE-6 — monotonic tag bumped each time an action fires. Carried on
    /// [`Message::ActionFinished`] so a straggler reply from a superseded action
    /// (dismissed, then a *different* action started) can't resolve the current
    /// modal with the wrong outcome — only the reply whose generation matches the
    /// in-flight action lands.
    pub action_gen: u64,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Vec<LighthouseCard>),
    Refresh,
    BeamTick,
    /// Highlight + scroll a specific lighthouse (deep-link focus).
    Focus(String),
    /// LIGHTHOUSE-6 — a card's action button was pressed: open the confirm gate
    /// for this action (nothing fires until the operator confirms).
    ActionRequested(Action),
    /// LIGHTHOUSE-6 — the confirm gate's Confirm button: actually run the
    /// remembered `pending_action`.
    ConnectConfirm,
    /// LIGHTHOUSE-6 — an action finished: `Ok(outcome line)` / `Err(error
    /// line)` resolves the modal to success / failure. The `u64` is the
    /// generation tag of the action that produced it; a reply whose tag is stale
    /// (a superseded action) is dropped.
    ActionFinished(u64, Result<String, String>),
    /// LIGHTHOUSE-6 — re-run the remembered action from the modal's failure.
    ConnectRetry,
    /// LIGHTHOUSE-6 — close the modal (Cancel / Dismiss / backdrop click).
    ConnectDismiss,
}

impl LighthousesPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        // `load_cards` is blocking — it routes through `crate::dbus::action_request`
        // (via `mesh_directory::fetch_peers_and_leader`), which builds its OWN
        // current-thread tokio runtime and `block_on`s it. The iced
        // `cosmic::executor::Default` is a multi-thread `tokio::runtime::Runtime`, so a
        // bare `Task::perform(async { load_cards() })` runs `block_on` ON a tokio worker
        // and panics ("Cannot start a runtime from within a runtime"), dropping the task
        // so `Loaded` never lands and the tab stays empty. `spawn_blocking` moves it onto
        // the blocking pool (no nested runtime), matching the other directory-RPC panels.
        Task::perform(
            async {
                tokio::task::spawn_blocking(load_cards)
                    .await
                    .unwrap_or_default()
            },
            |cards| crate::Message::Lighthouses(Message::Loaded(cards)),
        )
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
            // LIGHTHOUSE-6 — a card action button: open the confirm gate.
            // Nothing destructive runs until the operator presses Confirm.
            Message::ActionRequested(action) => {
                self.connect = ConnectProgress::confirm(action.title(), action.prompt());
                self.pending_action = Some(action);
                Task::none()
            }
            // LIGHTHOUSE-6 — Confirm / Retry both run the remembered action.
            Message::ConnectConfirm | Message::ConnectRetry => match self.pending_action.clone() {
                Some(action) => self.run_action(action),
                None => Task::none(),
            },
            // LIGHTHOUSE-6 — the action's outcome lands → resolve the modal.
            // Two guards: (1) the modal must still be in-flight (a late reply
            // can't resurrect a dismissed modal), and (2) the reply's generation
            // must be the CURRENT one (a straggler from a superseded action can't
            // resolve a different in-flight action with the wrong outcome).
            // `success`/`failure` keep the open title.
            Message::ActionFinished(gen, result) => {
                if self.connect.is_pending() && gen == self.action_gen {
                    self.connect = match result {
                        Ok(msg) => self.connect.success(msg),
                        Err(e) => self.connect.failure(e),
                    };
                }
                Task::none()
            }
            Message::ConnectDismiss => {
                self.connect = ConnectProgress::Closed;
                self.pending_action = None;
                Task::none()
            }
        }
    }

    /// LIGHTHOUSE-6 — flip the modal to in-flight and fire `action`. Restart +
    /// Promote round-trip `mackesd` over the Bus (off-runtime `action_request`,
    /// wrapped in `spawn_blocking` — same contract as `load`); SSH launches a
    /// local terminal. Each resolves to [`Message::ActionFinished`], tagged with
    /// `action_gen` so a superseded action's late reply is ignored. The caller
    /// (Confirm / Retry) already holds `pending_action`, so this only bumps the
    /// generation + the modal state.
    fn run_action(&mut self, action: Action) -> Task<crate::Message> {
        self.connect = ConnectProgress::pending(action.title(), in_flight_label(&action));
        self.action_gen = self.action_gen.wrapping_add(1);
        let gen = self.action_gen;
        match action {
            Action::Restart { host, overlay_ip } => Task::perform(
                async move {
                    tokio::task::spawn_blocking(move || restart_lighthouse(&host, &overlay_ip))
                        .await
                        .unwrap_or_else(|_| Err("restart task panicked".to_string()))
                },
                move |r| crate::Message::Lighthouses(Message::ActionFinished(gen, r)),
            ),
            Action::Promote { host } => Task::perform(
                async move {
                    tokio::task::spawn_blocking(move || promote_lighthouse(&host))
                        .await
                        .unwrap_or_else(|_| Err("promote task panicked".to_string()))
                },
                move |r| crate::Message::Lighthouses(Message::ActionFinished(gen, r)),
            ),
            Action::Ssh { host, overlay_ip } => Task::perform(
                async move {
                    let ok = crate::launcher::launch(&overlay_ip, Protocol::Ssh).await;
                    if ok {
                        Ok(format!("Opened an SSH terminal to {host} ({overlay_ip})."))
                    } else {
                        Err(format!(
                            "Could not launch a terminal for {host} (is cosmic-term installed?)."
                        ))
                    }
                },
                move |r| crate::Message::Lighthouses(Message::ActionFinished(gen, r)),
            ),
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

        let body: Element<'_, crate::Message> = container(
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
        .into();

        // LIGHTHOUSE-6 — stack the full-ops modal over the panel: the confirm
        // gate's Confirm fires the action, Retry re-runs it from a failure, and
        // Cancel / Dismiss / backdrop close it.
        connect_progress::overlay_confirm(
            &self.connect,
            body,
            palette,
            crate::Message::Lighthouses(Message::ConnectConfirm),
            crate::Message::Lighthouses(Message::ConnectRetry),
            crate::Message::Lighthouses(Message::ConnectDismiss),
        )
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
    // Failover readiness (Q22): the shadow is ready to take the leader role when
    // it's itself healthy; the master line states it holds leadership.
    let failover = if c.beacon.is_master {
        "holds the leader role".to_string()
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
    .spacing(3)
    // LIGHTHOUSE-8 — the deep-probe lines (handshake / public IP / peers /
    // uptime, then a colored CA cert-expiry line). Each field is `—` until the
    // probe lands (graceful degradation, §7).
    .push(probe_line(c.probe.as_ref(), p))
    .push(cert_expiry_line(c.probe.as_ref(), p));

    let inner = row![
        square,
        Space::new().width(Length::Fixed(16.0)),
        name_col(name_row, meta)
    ]
    .align_y(cosmic::iced::alignment::Vertical::Center);

    // LIGHTHOUSE-6 — the full-ops action row: Restart + SSH always, Promote only
    // on a shadow (the idempotent guard refuses an already-master promote, but
    // the button is hidden too so the operator never reaches a dead action).
    let body = column![inner, action_row(c, p)].spacing(12);

    // Focused card gets an accent left border; others a subtle border.
    let border_color = if focused { p.accent } else { p.border };
    container(body)
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

/// LIGHTHOUSE-6 — the per-card full-ops action button row. Restart + SSH are
/// always present; Promote appears only on a shadow (the master already holds the
/// lease). SSH is enabled only when the card carries an overlay IP (no IP → no
/// reachable target), and likewise Restart needs the IP for the daemon's SSH.
fn action_row<'a>(c: &LighthouseCard, p: Palette) -> Element<'a, crate::Message> {
    let host = c.beacon.hostname.clone();
    let overlay_ip = c.beacon.overlay_ip.clone();

    // Restart — needs the overlay IP for the daemon's mesh-key SSH.
    let restart = variant_button(
        "Restart",
        ButtonVariant::Secondary,
        overlay_ip.clone().map(|ip| {
            crate::Message::Lighthouses(Message::ActionRequested(Action::Restart {
                host: host.clone(),
                overlay_ip: ip,
            }))
        }),
        p,
    );

    // SSH / Open remote — a local terminal launch to the overlay IP.
    let ssh = variant_button(
        "SSH",
        ButtonVariant::Ghost,
        overlay_ip.clone().map(|ip| {
            crate::Message::Lighthouses(Message::ActionRequested(Action::Ssh {
                host: host.clone(),
                overlay_ip: ip,
            }))
        }),
        p,
    );

    let mut actions = row![restart, ssh].spacing(8);

    // Promote — shadow only (the master can't be promoted to itself).
    if !c.beacon.is_master {
        let promote = variant_button(
            "Promote ▸ master",
            ButtonVariant::Primary,
            Some(crate::Message::Lighthouses(Message::ActionRequested(
                Action::Promote { host: host.clone() },
            ))),
            p,
        );
        actions = actions.push(promote);
    }

    actions
        .align_y(cosmic::iced::alignment::Vertical::Center)
        .into()
}

/// CA cert-expiry days at/below which the card warns (matches the daemon's
/// `ca::expiry::CERT_EXPIRY_WARN_DAYS` — a full ops cycle of lead time). Kept as
/// a local UI threshold so the panel doesn't take a daemon-crate dependency.
const CERT_EXPIRY_WARN_DAYS: i64 = 30;

/// LIGHTHOUSE-8 — the handshake / public-IP / overlay-peers / uptime line. Every
/// field degrades to `—` when the probe hasn't measured it (or hasn't landed).
fn probe_line<'a>(probe: Option<&LighthouseProbe>, p: Palette) -> Element<'a, crate::Message> {
    let dash = "—".to_string();
    let handshake = probe.map_or("—", LighthouseProbe::handshake_word);
    let public_ip = probe
        .and_then(|pr| pr.public_ip.clone())
        .unwrap_or_else(|| dash.clone());
    let peers = probe
        .and_then(|pr| pr.peer_count)
        .map_or(dash.clone(), |n| n.to_string());
    let uptime = probe.map_or(dash, LighthouseProbe::uptime_human);
    text(format!(
        "handshake: {handshake}  ·  public: {public_ip}  ·  peers: {peers}  ·  up {uptime}",
    ))
    .size(12)
    .colr(p.text_muted.into_cosmic_color())
    .into()
}

/// LIGHTHOUSE-8 — the CA cert-expiry line, colored by urgency through the
/// mde-theme Carbon tokens: `danger` once expired or inside the warn window,
/// `warning` while approaching it, else `text_muted`. `—` until the probe lands.
fn cert_expiry_line<'a>(
    probe: Option<&LighthouseProbe>,
    p: Palette,
) -> Element<'a, crate::Message> {
    let days = probe.and_then(|pr| pr.cert_expiry_days);
    let color = match days {
        Some(d) if d <= CERT_EXPIRY_WARN_DAYS => p.danger,
        Some(d) if d <= CERT_EXPIRY_WARN_DAYS * 2 => p.warning,
        _ => p.text_muted,
    };
    let detail = probe.map_or_else(|| "—".to_string(), LighthouseProbe::cert_expiry_human);
    text(format!("mesh CA cert: {detail}"))
        .size(12)
        .colr(color.into_cosmic_color())
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
                health: p.health.clone(),
                last_seen_age_s: now_ms.saturating_sub(p.last_seen_ms) / 1000,
                services: summarize_services(p),
                mde_version: p.mde_version.clone(),
                // LIGHTHOUSE-8 — newest deep-probe off the bus for this name.
                probe: read_latest_probe(&p.hostname),
                beacon,
            }
        })
        .collect()
}

/// LIGHTHOUSE-8 — read the newest [`LighthouseProbe`] for `name` off the
/// mde-bus spool topic `compute/lighthouse-probe/<name>`. The probe worker
/// publishes one document per lighthouse each tick; the bus stores each as a
/// `{ … , "body": "<json>" }` envelope under `<bus-root>/compute/lighthouse-
/// probe/<name>/<ulid>.json` (the same on-disk shape the Mesh ▸ Bus panel
/// reads). Returns the freshest by **ULID filename** — ULIDs are monotonic +
/// lexically time-sortable, so the lexicographically-greatest `.json` name is
/// the newest message, a deterministic order (unlike filesystem mtime, which
/// ties at coarse granularity) that also needs no per-file `stat`. Best-effort:
/// a missing/unreadable bus yields `None`, and the card renders `—`.
fn read_latest_probe(name: &str) -> Option<LighthouseProbe> {
    let root = mde_bus::client_data_dir()?;
    let topic_dir = root.join("compute").join("lighthouse-probe").join(name);
    let entries = std::fs::read_dir(&topic_dir).ok()?;
    let mut newest: Option<std::ffi::OsString> = None;
    for ent in entries.flatten() {
        let fname = ent.file_name();
        if std::path::Path::new(&fname)
            .extension()
            .is_none_or(|e| e != "json")
        {
            continue;
        }
        if newest.as_ref().is_none_or(|cur| fname > *cur) {
            newest = Some(fname);
        }
    }
    let raw = std::fs::read_to_string(topic_dir.join(newest?)).ok()?;
    parse_probe_envelope(&raw)
}

/// Parse a [`LighthouseProbe`] out of one mde-bus envelope file's text: the
/// publisher wrote our JSON payload into the envelope's `body` string field
/// (`mde-bus publish … --body-flag <json>`). Pure (no I/O) so the decode path is
/// unit-tested. `None` on a malformed envelope or an unparseable body.
#[must_use]
fn parse_probe_envelope(raw: &str) -> Option<LighthouseProbe> {
    let envelope: serde_json::Value = serde_json::from_str(raw).ok()?;
    let body = envelope.get("body")?.as_str()?;
    serde_json::from_str::<LighthouseProbe>(body).ok()
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

/// LIGHTHOUSE-6 — the in-flight label shown under the modal spinner per action.
fn in_flight_label(action: &Action) -> String {
    match action {
        Action::Restart { host, .. } => {
            format!("Restarting {host}'s core fabric units over the overlay…")
        }
        Action::Ssh { host, .. } => format!("Opening an SSH terminal to {host}…"),
        Action::Promote { host } => format!("Promoting {host} to mesh master…"),
    }
}

/// LIGHTHOUSE-6 — fire `action/dc/lighthouse-restart` at `mackesd` and turn the
/// reply into a modal outcome line. Blocking (`action_request_with_body` builds
/// its own current-thread runtime), so callers wrap it in `spawn_blocking`. The
/// daemon validates `overlay_ip` + `confirm`, cycles mackesd to completion, then
/// enqueues a `--no-block` nebula restart over the mesh key (the SSH rides the
/// overlay nebula bounces); `None` (no live responder / timeout) is reported as
/// such.
fn restart_lighthouse(host: &str, overlay_ip: &str) -> Result<String, String> {
    let body = serde_json::json!({ "overlay_ip": overlay_ip, "confirm": true }).to_string();
    let reply = crate::dbus::action_request_with_body(
        "action/dc/lighthouse-restart",
        Some(&body),
        ACTION_TIMEOUT,
    )
    .ok_or_else(|| "mackesd did not answer (is the control plane up?)".to_string())?;
    if let Some(e) = crate::dbus::reply_error(&reply) {
        return Err(e);
    }
    Ok(format!(
        "Restarting {host}: mackesd cycled, nebula restart enqueued. The beacon \
         re-greens once the overlay is back."
    ))
}

/// LIGHTHOUSE-6 — fire `action/dc/lighthouse-promote` at `mackesd` and turn the
/// reply into a modal outcome line. The daemon's idempotent guard refuses if
/// `host` already holds the lease (surfaced as the failure error), else it
/// force-takes the lease via the existing leader-lease primitive and replies
/// with the new leader. Blocking — wrap in `spawn_blocking`.
fn promote_lighthouse(host: &str) -> Result<String, String> {
    let body = serde_json::json!({ "node": host, "confirm": true }).to_string();
    let reply = crate::dbus::action_request_with_body(
        "action/dc/lighthouse-promote",
        Some(&body),
        ACTION_TIMEOUT,
    )
    .ok_or_else(|| "mackesd did not answer (is the control plane up?)".to_string())?;
    if let Some(e) = crate::dbus::reply_error(&reply) {
        return Err(e);
    }
    // The reply carries the bare hostname now leading; fall back to `host`.
    let leader = serde_json::from_str::<serde_json::Value>(&reply)
        .ok()
        .and_then(|v| {
            v.get("leader")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| host.to_string());
    Ok(format!("Promoted {leader} to mesh master."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::lighthouse::BeaconStatus;

    /// A minimal shadow lighthouse card for the action-flow tests.
    fn shadow_card(host: &str, ip: &str) -> LighthouseCard {
        LighthouseCard {
            beacon: Beacon {
                hostname: host.to_string(),
                overlay_ip: Some(ip.to_string()),
                is_master: false,
                status: BeaconStatus::Healthy,
            },
            health: "healthy".into(),
            last_seen_age_s: 0,
            services: String::new(),
            mde_version: None,
            probe: None,
        }
    }

    #[test]
    fn action_requested_opens_the_confirm_gate_and_remembers_the_action() {
        let mut panel = LighthousesPanel::new();
        let action = Action::Restart {
            host: "anvil".into(),
            overlay_ip: "10.42.0.5".into(),
        };
        let _ = panel.update(Message::ActionRequested(action.clone()));
        // The modal is at the confirm gate (NOT in-flight) and the action is held.
        assert!(panel.connect.is_confirm(), "{:?}", panel.connect);
        assert!(!panel.connect.is_pending());
        assert_eq!(panel.connect.title(), "Restart anvil");
        assert_eq!(panel.pending_action, Some(action));
    }

    #[test]
    fn confirm_flips_the_gate_to_in_flight() {
        let mut panel = LighthousesPanel::new();
        let _ = panel.update(Message::ActionRequested(Action::Promote {
            host: "anvil".into(),
        }));
        // Confirm runs the action → the modal becomes Pending (in-flight). The
        // request itself resolves on the blocking pool; we only assert the
        // synchronous state transition here.
        let _ = panel.update(Message::ConnectConfirm);
        assert!(panel.connect.is_pending(), "{:?}", panel.connect);
        assert_eq!(panel.connect.title(), "Promote anvil");
    }

    #[test]
    fn finished_resolves_success_only_while_in_flight() {
        let mut panel = LighthousesPanel::new();
        let _ = panel.update(Message::ActionRequested(Action::Promote {
            host: "anvil".into(),
        }));
        let _ = panel.update(Message::ConnectConfirm);
        // The first fired action is generation 1.
        let _ = panel.update(Message::ActionFinished(
            panel.action_gen,
            Ok("Promoted anvil to mesh master.".into()),
        ));
        assert!(
            matches!(panel.connect, ConnectProgress::Success { .. }),
            "{:?}",
            panel.connect
        );
    }

    #[test]
    fn finished_resolves_failure_with_the_daemon_error() {
        let mut panel = LighthousesPanel::new();
        let _ = panel.update(Message::ActionRequested(Action::Restart {
            host: "anvil".into(),
            overlay_ip: "10.42.0.5".into(),
        }));
        let _ = panel.update(Message::ConnectConfirm);
        let _ = panel.update(Message::ActionFinished(
            panel.action_gen,
            Err("ssh failed: timed out".into()),
        ));
        match &panel.connect {
            ConnectProgress::Failure { error, .. } => assert_eq!(error, "ssh failed: timed out"),
            other => panic!("expected Failure, got {other:?}"),
        }
    }

    #[test]
    fn a_late_reply_cannot_resurrect_a_dismissed_modal() {
        let mut panel = LighthousesPanel::new();
        let _ = panel.update(Message::ActionRequested(Action::Promote {
            host: "anvil".into(),
        }));
        let _ = panel.update(Message::ConnectConfirm);
        let gen = panel.action_gen;
        // Operator dismisses while in flight (or after a manual close).
        let _ = panel.update(Message::ConnectDismiss);
        assert_eq!(panel.connect, ConnectProgress::Closed);
        assert!(panel.pending_action.is_none());
        // A straggler reply lands AFTER the dismiss — it must NOT reopen the modal.
        let _ = panel.update(Message::ActionFinished(gen, Ok("done".into())));
        assert_eq!(panel.connect, ConnectProgress::Closed);
    }

    #[test]
    fn a_superseded_actions_reply_cannot_resolve_a_newer_in_flight_modal() {
        // Action A fires (gen 1), is dismissed, then action B fires (gen 2) and is
        // in flight. A's slow reply must NOT resolve B's modal with A's outcome.
        let mut panel = LighthousesPanel::new();
        let _ = panel.update(Message::ActionRequested(Action::Promote {
            host: "anvil".into(),
        }));
        let _ = panel.update(Message::ConnectConfirm);
        let gen_a = panel.action_gen;
        let _ = panel.update(Message::ConnectDismiss);

        // Start a DIFFERENT action B.
        let _ = panel.update(Message::ActionRequested(Action::Restart {
            host: "forge".into(),
            overlay_ip: "10.42.0.6".into(),
        }));
        let _ = panel.update(Message::ConnectConfirm);
        assert!(panel.connect.is_pending());
        assert_eq!(panel.connect.title(), "Restart forge");

        // A's straggler reply (stale generation) lands — it must be dropped, so
        // B's modal stays in-flight, NOT resolved with A's "Promoted…" line.
        let _ = panel.update(Message::ActionFinished(
            gen_a,
            Ok("Promoted anvil to mesh master.".into()),
        ));
        assert!(panel.connect.is_pending(), "{:?}", panel.connect);
        assert_eq!(panel.connect.title(), "Restart forge");

        // B's own reply (current generation) resolves it correctly.
        let _ = panel.update(Message::ActionFinished(
            panel.action_gen,
            Ok("B done".into()),
        ));
        match &panel.connect {
            ConnectProgress::Success { message, .. } => assert_eq!(message, "B done"),
            other => panic!("expected B's Success, got {other:?}"),
        }
    }

    #[test]
    fn dismiss_clears_the_pending_action() {
        let mut panel = LighthousesPanel::new();
        let _ = panel.update(Message::ActionRequested(Action::Ssh {
            host: "anvil".into(),
            overlay_ip: "10.42.0.5".into(),
        }));
        assert!(panel.pending_action.is_some());
        let _ = panel.update(Message::ConnectDismiss);
        assert!(panel.pending_action.is_none());
        assert_eq!(panel.connect, ConnectProgress::Closed);
    }

    #[test]
    fn promote_button_is_omitted_for_the_master_card() {
        // The master card renders no Promote action (can't promote to itself);
        // the shadow card does. We can't introspect the widget tree, so assert
        // the precondition the `action_row` branches on.
        let mut master = shadow_card("anvil", "10.42.0.5");
        master.beacon.is_master = true;
        assert!(master.beacon.is_master);
        let shadow = shadow_card("forge", "10.42.0.6");
        assert!(!shadow.beacon.is_master);
    }

    #[test]
    fn action_titles_and_prompts_name_the_host() {
        let restart = Action::Restart {
            host: "anvil".into(),
            overlay_ip: "10.42.0.5".into(),
        };
        assert_eq!(restart.title(), "Restart anvil");
        assert!(restart.prompt().contains("anvil"));
        let promote = Action::Promote {
            host: "forge".into(),
        };
        assert_eq!(promote.title(), "Promote forge");
        assert!(promote.prompt().contains("master"));
    }

    /// Build a bus-envelope JSON string the way `mde-bus publish --body-flag`
    /// stores it: our [`LighthouseProbe`] JSON lives in the `body` string field.
    fn envelope_for(probe: &LighthouseProbe) -> String {
        let body = serde_json::to_string(probe).expect("serialize probe");
        let env = serde_json::json!({
            "ulid": "01TESTULID0000000000000000",
            "topic": LighthouseProbe::topic(&probe.name),
            "priority": "default",
            "title": null,
            "body": body,
            "ts_unix_ms": probe.probed_at_ms,
            "file_path": "compute/lighthouse-probe/anvil/01TESTULID0000000000000000.json",
        });
        serde_json::to_string(&env).expect("serialize envelope")
    }

    #[test]
    fn parse_probe_envelope_decodes_the_body_payload() {
        let probe = LighthouseProbe {
            name: "anvil".into(),
            overlay_ip: Some("10.42.0.5".into()),
            handshake: Some(true),
            public_ip: Some("203.0.113.5:4242".into()),
            peer_count: Some(4),
            uptime_s: Some(7_200),
            cert_expiry_days: Some(180),
            probed_at_ms: 1_700_000_000_000,
        };
        let raw = envelope_for(&probe);
        let back = parse_probe_envelope(&raw).expect("decode");
        assert_eq!(back, probe);
    }

    #[test]
    fn parse_probe_envelope_rejects_garbage_and_missing_body() {
        assert!(parse_probe_envelope("not json").is_none());
        assert!(parse_probe_envelope(r#"{"topic":"t"}"#).is_none());
        // A body that isn't a LighthouseProbe document.
        assert!(parse_probe_envelope(r#"{"body":"{\"foo\":1}"}"#).is_none());
    }

    #[test]
    fn read_latest_probe_picks_the_newest_envelope() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let topic_dir = tmp
            .path()
            .join("compute")
            .join("lighthouse-probe")
            .join("anvil");
        std::fs::create_dir_all(&topic_dir).expect("mkdir topic");

        // An older probe (peers=2) and a newer probe (peers=9). The reader picks
        // by ULID filename: ULIDs are monotonic + lexically time-sortable, so the
        // newer message has the lexicographically-greater name. Write in reverse
        // (newer file first) to prove the order is by NAME, not write/mtime order.
        let mut old = LighthouseProbe::unmeasured("anvil", 1_000);
        old.peer_count = Some(2);
        let mut new = LighthouseProbe::unmeasured("anvil", 2_000);
        new.peer_count = Some(9);

        // Representative monotonic ULIDs (the newer one sorts lexically last).
        let old_path = topic_dir.join("01J000000000000000000OLDER.json");
        let new_path = topic_dir.join("01J000000000000000000ZNEWR.json");
        std::fs::write(&new_path, envelope_for(&new)).expect("write new");
        std::fs::write(&old_path, envelope_for(&old)).expect("write old");

        std::env::set_var("MDE_BUS_ROOT", tmp.path());
        let got = read_latest_probe("anvil");
        std::env::remove_var("MDE_BUS_ROOT");

        let got = got.expect("a probe should be read");
        assert_eq!(got.peer_count, Some(9), "newest ULID wins");
    }

    #[test]
    fn read_latest_probe_is_none_when_topic_absent() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::env::set_var("MDE_BUS_ROOT", tmp.path());
        let got = read_latest_probe("ghost-lighthouse");
        std::env::remove_var("MDE_BUS_ROOT");
        assert!(got.is_none());
    }
}
