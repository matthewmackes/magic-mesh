//! Network ▸ VPN panel — VPN-GW-7.
//!
//! A Workbench surface that lists the node's mesh **VPN-GW tunnels** as
//! professional cards and lets the operator stand up a new one through an
//! add-tunnel wizard. It is glue over the already-spawned VPN-GW Bus responder
//! (`mackesd/src/ipc/vpn_gw.rs`) — every field is real data over the existing
//! `action/vpn/*` verbs, no host-NetworkManager fallback and no new daemon work:
//!
//!   * `list-tunnels` → the durable `TunnelDef`s (provider / server / protocol /
//!     method / the `mvpn-<id>` interface),
//!   * `tunnel-status` → each tunnel's live up/down (and therefore its
//!     kill-switch posture — VPN-GW-3 engages the egress DROP whenever the
//!     tunnel is down, so "down" *is* "kill-switch engaged, egress blocked"),
//!   * `verify-egress` → the live **verified exit IP**, the health verdict
//!     (ok / leaking / dns-leak / down / unverifiable) and the human leak
//!     reason fetched *through* the tunnel (VPN-GW-6),
//!   * `list-providers` → the provider catalog the wizard's first step lists,
//!   * `setup-provider` → builds the verifiable tunnel config from the wizard's
//!     inputs, age-encrypts the secret into the mesh store and persists the
//!     `TunnelDef` (VPN-GW-2/5) — the same verb the wizard re-runs to **edit**
//!     a tunnel (upsert by id),
//!   * `tunnel-up` / `tunnel-down` / `remove-tunnel` → the per-card actions.
//!
//! Each Bus call is request-reply on a `spawn_blocking` task (the Bus client
//! owns a current-thread runtime), exactly like the Routing panel — never the
//! iced executor.

use std::time::Duration;

use cosmic::iced::widget::{column, container, row, scrollable, text, text_input, Space};
use cosmic::iced::{Background, Border, Length, Padding, Task};
use cosmic::{Element, Theme};
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, Rgba, TypeRole};

use crate::controls::{variant_button, ButtonVariant};
use crate::cosmic_compat::prelude::*;

/// Read budget for the `action/vpn/*` config probes (`list-tunnels`,
/// `tunnel-status`, `list-providers`). The responder answers from local config +
/// `ip link` (no network round-trips), so the interactive 2 s window is generous.
const VPN_TIMEOUT: Duration = Duration::from_secs(2);

/// Read budget for the verbs that shell out to `curl` *through* the tunnel —
/// `verify-egress` (exit-IP / leak probe) and `setup-provider`. The responder's
/// own exit check runs `curl -m 10`, so the panel allows the full reflector
/// round-trip plus slack before declaring the responder unreachable.
const VPN_VERIFY_TIMEOUT: Duration = Duration::from_secs(15);

/// One tunnel row: the durable `TunnelDef` (from `list-tunnels`) paired with its
/// live `tunnel-status` liveness and, when verified, the `verify-egress` exit-IP
/// + health verdict. Mirrors the fields the operator acts on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VpnRow {
    /// Operator-chosen tunnel id (the action argument for up/down/remove/verify).
    pub id: String,
    /// Provider label (`mullvad`/`proton`/…/`generic-wg`/`generic-ovpn`).
    pub provider: String,
    /// Server/region selector (may be empty for a generic tunnel).
    pub server: String,
    /// Transport hint (`udp`/`tcp`); empty when the provider doesn't set one.
    pub protocol: String,
    /// How it's brought up (`wg`/`ovpn`/`cli`/`api`) — the wire method label.
    pub method: String,
    /// The dedicated `mvpn-<id>` interface name.
    pub ifname: String,
    /// Live up/down (from the paired `tunnel-status` probe).
    pub up: bool,
    /// The verified provider exit IP fetched through the tunnel (`verify-egress`),
    /// present once the operator runs Verify and the reflector answered.
    pub exit_ip: Option<String>,
    /// The health verdict tag (`ok`/`leaking`/`dns-leak`/`down`/`unverifiable`)
    /// from the last `verify-egress`, or empty when not yet verified.
    pub health: String,
    /// The human leak/health reason line from `verify-egress` (e.g. "exit
    /// confirmed (≠ WAN)" or "LEAK: exit IP equals the WAN IP").
    pub health_detail: String,
}

impl VpnRow {
    /// Is egress for this tunnel currently leak-proof-blocked? VPN-GW-3 installs
    /// the kill-switch DROP whenever the tunnel is down (and clears it on a clean
    /// up), so the kill-switch posture is exactly the inverse of liveness — no
    /// separate verb needed. A down tunnel = marked traffic is blocked (no WAN
    /// leak); an up tunnel = the kill-switch is cleared and egress flows.
    #[must_use]
    pub fn kill_switch_engaged(&self) -> bool {
        !self.up
    }
}

#[derive(Debug, Clone, Default)]
pub struct VpnPanel {
    /// `false` when the VPN responder didn't answer (`mackesd` down / no Bus).
    pub daemon_up: bool,
    pub tunnels: Vec<VpnRow>,
    pub status: String,
    pub busy: bool,
    /// The add/edit-tunnel wizard, present while the operator is running it.
    pub wizard: Option<Wizard>,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// A `list-tunnels` (+ batched per-row `tunnel-status`) fetch landed.
    Loaded(Result<Vec<VpnRow>, String>),
    RefreshClicked,
    /// Up/Down a tunnel by id.
    ToggleClicked {
        id: String,
        up: bool,
    },
    /// Remove a tunnel by id.
    RemoveClicked {
        id: String,
    },
    /// Verify a tunnel's egress now (exit IP / leak), folding the result into
    /// the matching row.
    VerifyClicked {
        id: String,
    },
    /// A `verify-egress` reply landed for `id`.
    Verified {
        id: String,
        result: Result<EgressVerdict, String>,
    },
    /// An up/down/remove action reply landed.
    OperationFinished(Result<String, String>),
    /// Open the add-tunnel wizard (`None` id) or the edit wizard for an id.
    OpenWizard {
        edit_id: Option<String>,
    },
    /// A wizard message (only meaningful while `wizard` is `Some`).
    Wizard(WizardMsg),
    /// The provider catalog for the wizard's first step landed.
    ProvidersLoaded(Result<Vec<ProviderInfo>, String>),
    /// A `setup-provider` (save) reply landed.
    SetupFinished(Result<String, String>),
}

/// The live exit-IP / health verdict for one tunnel, decoded from a
/// `verify-egress` reply. Folded into the matching [`VpnRow`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EgressVerdict {
    pub exit_ip: Option<String>,
    pub health: String,
    pub detail: String,
}

impl VpnPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Fetch the tunnel list (+ liveness) over the Bus. The Bus client builds
    /// its own current-thread runtime, so the blocking fetch rides
    /// `spawn_blocking`, never the iced executor.
    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move {
                let joined = tokio::task::spawn_blocking(fetch_tunnels).await;
                let result = joined.unwrap_or_else(|e| Err(format!("vpn fetch task: {e}")));
                crate::Message::Vpn(Message::Loaded(result))
            },
            |m| m,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded(Ok(tunnels)) => {
                self.daemon_up = true;
                // Preserve any exit-IP/health a prior Verify already established
                // for a still-present tunnel — a plain list refresh shouldn't
                // wipe the verified exit IP the operator just fetched.
                let mut merged = tunnels;
                for row in &mut merged {
                    if let Some(prev) = self.tunnels.iter().find(|t| t.id == row.id) {
                        row.exit_ip.clone_from(&prev.exit_ip);
                        row.health.clone_from(&prev.health);
                        row.health_detail.clone_from(&prev.health_detail);
                    }
                }
                self.tunnels = merged;
                self.status.clear();
                self.busy = false;
                Task::none()
            }
            Message::Loaded(Err(msg)) => {
                self.daemon_up = false;
                self.tunnels.clear();
                self.status = msg;
                self.busy = false;
                Task::none()
            }
            Message::RefreshClicked => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = "Refreshing…".into();
                Self::load()
            }
            Message::ToggleClicked { id, up } => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = format!("{} {id}…", if up { "Bringing up" } else { "Bringing down" });
                let verb = if up { "tunnel-up" } else { "tunnel-down" };
                op_task(verb, id)
            }
            Message::RemoveClicked { id } => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = format!("Removing {id}…");
                op_task("remove-tunnel", id)
            }
            Message::VerifyClicked { id } => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = format!("Verifying {id} egress…");
                verify_task(id)
            }
            Message::Verified { id, result } => {
                self.busy = false;
                match result {
                    Ok(v) => {
                        self.status = format!("{id}: {}", v.detail);
                        if let Some(row) = self.tunnels.iter_mut().find(|t| t.id == id) {
                            row.exit_ip.clone_from(&v.exit_ip);
                            row.health = v.health;
                            row.health_detail = v.detail;
                        }
                    }
                    Err(msg) => self.status = msg,
                }
                Task::none()
            }
            Message::OperationFinished(result) => {
                self.busy = false;
                self.status = match result {
                    Ok(msg) | Err(msg) => msg,
                };
                // Re-fetch so the row badges + presence reflect the change.
                Self::load()
            }
            Message::OpenWizard { edit_id } => {
                if self.busy {
                    return Task::none();
                }
                // Seed an edit wizard from the existing row so the operator can
                // re-run setup with the same id (the responder upserts by id).
                let seed = edit_id
                    .as_deref()
                    .and_then(|id| self.tunnels.iter().find(|t| t.id == id));
                self.wizard = Some(Wizard::new(seed));
                self.status.clear();
                // Pull the live provider catalog for step 1.
                Self::load_providers()
            }
            Message::ProvidersLoaded(result) => {
                if let Some(w) = self.wizard.as_mut() {
                    match result {
                        Ok(p) => w.providers = p,
                        Err(msg) => w.note = msg,
                    }
                }
                Task::none()
            }
            Message::Wizard(msg) => {
                let Some(w) = self.wizard.as_mut() else {
                    return Task::none();
                };
                match w.update(msg) {
                    WizardAction::None => Task::none(),
                    WizardAction::Cancel => {
                        self.wizard = None;
                        Task::none()
                    }
                    WizardAction::Save(body) => {
                        w.saving = true;
                        self.status = format!("Saving {}…", w.id.trim());
                        setup_task(body)
                    }
                }
            }
            Message::SetupFinished(result) => {
                match result {
                    Ok(msg) => {
                        self.status = msg;
                        self.wizard = None;
                        // Reload so the new/edited tunnel card appears.
                        Self::load()
                    }
                    Err(msg) => {
                        self.status = msg.clone();
                        if let Some(w) = self.wizard.as_mut() {
                            w.saving = false;
                            w.note = msg;
                        }
                        Task::none()
                    }
                }
            }
        }
    }

    /// Pull the provider catalog (`list-providers`) for the wizard's first step.
    fn load_providers() -> Task<crate::Message> {
        Task::perform(
            async move {
                let joined = tokio::task::spawn_blocking(fetch_providers).await;
                let result = joined.unwrap_or_else(|e| Err(format!("vpn providers task: {e}")));
                crate::Message::Vpn(Message::ProvidersLoaded(result))
            },
            |m| m,
        )
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        // The wizard takes the whole surface while it's open (a focused flow).
        if let Some(w) = &self.wizard {
            return w.view();
        }

        let palette = crate::live_theme::palette();
        let sizes = FontSize::defaults();

        let title = text("VPN")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());
        let subtitle = text("mesh egress tunnels")
            .size(TypeRole::Body.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color());

        let add_btn = variant_button(
            "Add tunnel",
            ButtonVariant::Primary,
            (!self.busy).then_some(crate::Message::Vpn(Message::OpenWizard { edit_id: None })),
            palette,
        );
        let refresh_btn = variant_button(
            if self.busy { "…" } else { "Refresh" },
            ButtonVariant::Ghost,
            (!self.busy).then_some(crate::Message::Vpn(Message::RefreshClicked)),
            palette,
        );
        let header = row![
            column![title, subtitle].spacing(2),
            Space::new().width(Length::Fill),
            add_btn,
            Space::new().width(Length::Fixed(8.0)),
            refresh_btn,
        ]
        .align_y(cosmic::iced::alignment::Vertical::Center);

        let mut body_col = column![].spacing(8);
        if !self.status.is_empty() {
            body_col = body_col.push(status_strip(&self.status, palette));
        }

        if !self.daemon_up {
            body_col = body_col.push(empty_state(
                Icon::StatusError,
                palette.danger,
                "VPN gateway unreachable",
                "Couldn't reach the mesh VPN responder over the Bus. Is `mackesd` \
                 running on this node?",
                palette,
            ));
        } else if self.tunnels.is_empty() {
            body_col = body_col.push(empty_state(
                Icon::Vpn,
                palette.accent,
                "No tunnels configured",
                "This node has no VPN-GW egress tunnels yet. Use Add tunnel to set \
                 one up.",
                palette,
            ));
        } else {
            for t in &self.tunnels {
                body_col = body_col.push(tunnel_card(t, self.busy, palette));
            }
            body_col = body_col.push(
                text(format!(
                    "{} tunnel(s) · {} up",
                    self.tunnels.len(),
                    self.tunnels.iter().filter(|t| t.up).count(),
                ))
                .size(TypeRole::Caption.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color()),
            );
        }

        container(
            column![
                header,
                Space::new().height(Length::Fixed(20.0)),
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

/// One tunnel's card: a provider · liveness header, the detail + interface
/// lines, the live verified exit IP + health verdict + kill-switch posture, and
/// the Verify / Up·Down / Edit / Remove actions.
fn tunnel_card<'a>(t: &VpnRow, busy: bool, palette: Palette) -> Element<'a, crate::Message> {
    let sizes = FontSize::defaults();
    let (badge_icon, badge_color, badge_label) = if t.up {
        (Icon::StatusOk, palette.success, "up")
    } else {
        (Icon::StatusUnknown, palette.text_muted, "down")
    };

    let head = row![
        status_icon(badge_icon, badge_color.into_cosmic_color(), 16.0),
        text(t.provider.clone())
            .size(TypeRole::Subheading.size_in(sizes))
            .colr(palette.text.into_cosmic_color()),
        Space::new().width(Length::Fill),
        text(badge_label)
            .size(TypeRole::Caption.size_in(sizes))
            .colr(badge_color.into_cosmic_color()),
    ]
    .spacing(8)
    .align_y(cosmic::iced::alignment::Vertical::Center);

    // Server · protocol · method — the muted detail line (each part dropped when
    // the provider didn't set it, so a generic WG tunnel reads cleanly).
    let detail_row = text(detail_line(t))
        .size(TypeRole::Caption.size_in(sizes))
        .colr(palette.text_muted.into_cosmic_color());
    let iface_row = text(format!("{} · {}", t.id, t.ifname))
        .size(TypeRole::Caption.size_in(sizes))
        .colr(palette.text_muted.into_cosmic_color());

    let mut inner = column![head, detail_row, iface_row].spacing(6);

    // Exit IP + health verdict (filled by Verify) — the real provider exit
    // fetched through the tunnel, with a health-coloured verdict tag.
    inner = inner.push(exit_line(t, palette));

    // Kill-switch posture (VPN-GW-3): engaged ⇒ egress blocked (leak-proof) when
    // the tunnel is down; cleared ⇒ egress flows when up.
    inner = inner.push(kill_switch_line(t, palette));

    // Up XOR Down (the relevant transition), Verify, Edit, Remove.
    let toggle_label = if t.up { "Down" } else { "Up" };
    let toggle_btn = variant_button(
        toggle_label,
        ButtonVariant::Secondary,
        (!busy).then(|| {
            crate::Message::Vpn(Message::ToggleClicked {
                id: t.id.clone(),
                up: !t.up,
            })
        }),
        palette,
    );
    let verify_btn = variant_button(
        "Verify",
        ButtonVariant::Ghost,
        (!busy).then(|| crate::Message::Vpn(Message::VerifyClicked { id: t.id.clone() })),
        palette,
    );
    let edit_btn = variant_button(
        "Edit",
        ButtonVariant::Ghost,
        (!busy).then(|| {
            crate::Message::Vpn(Message::OpenWizard {
                edit_id: Some(t.id.clone()),
            })
        }),
        palette,
    );
    let remove_btn = variant_button(
        "Remove",
        ButtonVariant::Ghost,
        (!busy).then(|| crate::Message::Vpn(Message::RemoveClicked { id: t.id.clone() })),
        palette,
    );
    let actions = row![
        Space::new().width(Length::Fill),
        verify_btn,
        Space::new().width(Length::Fixed(8.0)),
        toggle_btn,
        Space::new().width(Length::Fixed(8.0)),
        edit_btn,
        Space::new().width(Length::Fixed(8.0)),
        remove_btn,
    ]
    .align_y(cosmic::iced::alignment::Vertical::Center);
    inner = inner.push(actions);

    card(inner, palette)
}

/// The verified-exit-IP line: the real provider exit (fetched through the
/// tunnel) plus a health-coloured verdict, or an unverified hint. Pure mapping.
fn exit_line<'a>(t: &VpnRow, palette: Palette) -> Element<'a, crate::Message> {
    let sizes = FontSize::defaults();
    let (color, label) = health_badge(&t.health, palette);
    let body = match (&t.exit_ip, t.health.is_empty()) {
        (Some(ip), _) => format!("exit {ip} · {label}"),
        (None, false) => format!("exit unverified · {label}"),
        (None, true) => "exit not yet verified — run Verify".to_string(),
    };
    row![
        text("Egress")
            .size(TypeRole::Caption.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color())
            .width(Length::Fixed(72.0)),
        text(body)
            .size(TypeRole::Caption.size_in(sizes))
            .colr(color.into_cosmic_color()),
    ]
    .spacing(8)
    .into()
}

/// The kill-switch posture line — engaged (egress blocked, leak-proof) when the
/// tunnel is down, cleared when up. Pure mapping of the row's liveness.
fn kill_switch_line<'a>(t: &VpnRow, palette: Palette) -> Element<'a, crate::Message> {
    let sizes = FontSize::defaults();
    let (color, body) = if t.kill_switch_engaged() {
        (palette.warning, "engaged — egress blocked (no WAN leak)")
    } else {
        (palette.success, "cleared — egress flows out the tunnel")
    };
    row![
        text("Kill-switch")
            .size(TypeRole::Caption.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color())
            .width(Length::Fixed(72.0)),
        text(body)
            .size(TypeRole::Caption.size_in(sizes))
            .colr(color.into_cosmic_color()),
    ]
    .spacing(8)
    .into()
}

/// Map a `verify-egress` health tag → (colour, short label). Pure — unit-tested.
#[must_use]
pub fn health_badge(tag: &str, palette: Palette) -> (Rgba, &'static str) {
    match tag {
        "ok" => (palette.success, "exit confirmed"),
        "leaking" => (palette.danger, "LEAKING"),
        "dns-leak" => (palette.danger, "DNS leak"),
        "down" => (palette.text_muted, "interface down"),
        "unverifiable" => (palette.warning, "unverifiable"),
        _ => (palette.text_muted, "unknown"),
    }
}

/// The muted `server · protocol · method` line, dropping empty parts so a
/// generic tunnel (no server/protocol) reads cleanly. Pure — unit-tested.
#[must_use]
pub fn detail_line(t: &VpnRow) -> String {
    [t.server.as_str(), t.protocol.as_str(), t.method.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// A short status strip above the cards (last action result / refresh note).
fn status_strip<'a>(msg: &str, palette: Palette) -> Element<'a, crate::Message> {
    let bg = palette.raised.into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    container(
        text(msg.to_string())
            .size(TypeRole::Caption.size_in(FontSize::defaults()))
            .colr(palette.text.into_cosmic_color()),
    )
    .padding(Padding::from([8u16, 14u16]))
    .width(Length::Fill)
    .style(move |_| container::Style {
        snap: false,
        background: Some(Background::Color(bg)),
        border: Border {
            color: border,
            width: 1.0,
            radius: 5.0.into(),
        },
        ..container::Style::default()
    })
    .into()
}

/// Centered empty/error state with an icon + heading + body.
fn empty_state<'a>(
    icon: Icon,
    icon_color: Rgba,
    heading: &'a str,
    body: &'a str,
    palette: Palette,
) -> Element<'a, crate::Message> {
    let sizes = FontSize::defaults();
    container(
        column![
            status_icon(icon, icon_color.into_cosmic_color(), 32.0),
            Space::new().height(Length::Fixed(8.0)),
            text(heading)
                .size(TypeRole::Subheading.size_in(sizes))
                .colr(palette.text.into_cosmic_color()),
            text(body)
                .size(TypeRole::Caption.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color()),
        ]
        .spacing(2)
        .align_x(cosmic::iced::alignment::Horizontal::Center),
    )
    .padding(Padding::from([32u16, 16u16]))
    .width(Length::Fill)
    .into()
}

/// Render an [`Icon`] as an SVG tinted to `color`, falling back to its glyph.
fn status_icon<'a>(icon: Icon, color: cosmic::iced::Color, px: f32) -> Element<'a, crate::Message> {
    let resolved = mde_icon(icon, IconSize::Inline);
    if let Some(b) = resolved.svg_bytes() {
        use cosmic::iced::widget::svg as widget_svg;
        widget_svg(widget_svg::Handle::from_memory(b))
            .width(Length::Fixed(px))
            .height(Length::Fixed(px))
            .sty(move |_t: &Theme| widget_svg::Style { color: Some(color) })
            .into()
    } else {
        text(resolved.fallback_glyph).size(px).colr(color).into()
    }
}

/// Shared card chrome (raised surface, 1 px border, 5 px corners).
fn card<'a>(
    inner: impl Into<Element<'a, crate::Message>>,
    palette: Palette,
) -> Element<'a, crate::Message> {
    let bg = palette.raised.into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    container(inner)
        .padding(Padding::from([12u16, 16u16]))
        .width(Length::Fill)
        .style(move |_| container::Style {
            snap: false,
            background: Some(Background::Color(bg)),
            border: Border {
                color: border,
                width: 1.0,
                radius: 5.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

// ============================================================================
// Add / edit-tunnel wizard
// ============================================================================

/// One provider's wizard-relevant facts, decoded from a `list-providers` entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderInfo {
    /// Provider label (`mullvad`/…/`generic-wg`/`generic-ovpn`).
    pub id: String,
    /// Bring-up method (`wg`/`ovpn`/`cli`/`api`).
    pub method: String,
    /// Whether several instances of this provider can coexist (drives the
    /// multi-instance name hint).
    pub multi_instance: bool,
    /// The exit-IP reflector the responder will curl through the tunnel to
    /// verify (shown on the verify step so the operator knows the check).
    pub exit_check: Option<String>,
}

/// The wizard's ordered steps: provider → method/config (auth/config blob) →
/// server → name → verify/save. The "method" is provider-derived (shown, not
/// chosen) so the config step asks for exactly the inputs that method needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Provider,
    Config,
    Server,
    Name,
    Review,
}

impl Step {
    /// 1-based index for the "step N of 5" header.
    #[must_use]
    pub const fn ordinal(self) -> u8 {
        match self {
            Self::Provider => 1,
            Self::Config => 2,
            Self::Server => 3,
            Self::Name => 4,
            Self::Review => 5,
        }
    }
    const fn next(self) -> Self {
        match self {
            Self::Provider => Self::Config,
            Self::Config => Self::Server,
            Self::Server => Self::Name,
            Self::Name | Self::Review => Self::Review,
        }
    }
    const fn prev(self) -> Self {
        match self {
            Self::Provider | Self::Config => Self::Provider,
            Self::Server => Self::Config,
            Self::Name => Self::Server,
            Self::Review => Self::Name,
        }
    }
}

/// Wizard form state.
#[derive(Debug, Clone, Default)]
pub struct Wizard {
    /// `true` when re-running setup for an existing id (edit), so the id field
    /// is shown pre-filled + read-only (the responder upserts by id).
    pub editing: bool,
    pub step_idx: u8,
    /// Provider catalog (from `list-providers`); empty until it lands.
    pub providers: Vec<ProviderInfo>,
    /// The chosen provider label.
    pub provider: String,
    /// Operator-chosen tunnel id (drives `mvpn-<id>`).
    pub id: String,
    /// Server/region selector.
    pub server: String,
    // -- WireGuard structured fields (when not pasting a config) --
    pub private_key: String,
    pub address: String,
    pub peer_public_key: String,
    pub endpoint: String,
    pub dns: String,
    pub preshared_key: String,
    /// A pasted WireGuard `.conf` blob (any WG provider) — takes the paste path.
    pub wg_config: String,
    /// A pasted `.ovpn` blob (generic-ovpn).
    pub ovpn: String,
    /// `true` while the `setup-provider` save is in flight.
    pub saving: bool,
    /// A note line (provider-load error / save error).
    pub note: String,
}

/// What the parent should do after folding a [`WizardMsg`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WizardAction {
    None,
    Cancel,
    /// The assembled `setup-provider` request body (JSON) to publish.
    Save(String),
}

/// Wizard form messages.
#[derive(Debug, Clone)]
pub enum WizardMsg {
    SelectProvider(String),
    IdInput(String),
    ServerInput(String),
    PrivateKeyInput(String),
    AddressInput(String),
    PeerPublicKeyInput(String),
    EndpointInput(String),
    DnsInput(String),
    PresharedKeyInput(String),
    WgConfigInput(String),
    OvpnInput(String),
    Next,
    Back,
    Cancel,
    Save,
}

impl Wizard {
    /// Open a fresh wizard, or seed an edit from an existing row (id locked,
    /// provider/server pre-filled — the operator re-supplies the secret).
    #[must_use]
    pub fn new(seed: Option<&VpnRow>) -> Self {
        match seed {
            Some(r) => Self {
                editing: true,
                step_idx: Step::Provider.ordinal(),
                provider: r.provider.clone(),
                id: r.id.clone(),
                server: r.server.clone(),
                ..Self::default()
            },
            None => Self {
                step_idx: Step::Provider.ordinal(),
                ..Self::default()
            },
        }
    }

    /// The current step (derived from the 1-based index).
    #[must_use]
    pub fn step(&self) -> Step {
        match self.step_idx {
            1 => Step::Provider,
            2 => Step::Config,
            3 => Step::Server,
            4 => Step::Name,
            _ => Step::Review,
        }
    }

    /// The selected provider's facts, if its catalog entry is known.
    #[must_use]
    pub fn selected(&self) -> Option<&ProviderInfo> {
        self.providers.iter().find(|p| p.id == self.provider)
    }

    /// The bring-up method for the chosen provider (`wg`/`ovpn`/…). Falls back
    /// to `wg` until the catalog lands (the dominant path).
    #[must_use]
    pub fn method(&self) -> &str {
        self.selected().map_or("wg", |p| p.method.as_str())
    }

    /// Can the wizard advance from the current step? Gates on the inputs the
    /// step needs so a half-filled tunnel can't reach Save.
    #[must_use]
    pub fn can_advance(&self) -> bool {
        match self.step() {
            Step::Provider => !self.provider.trim().is_empty(),
            // Config completeness is provider-shaped (below). The Name step
            // re-checks the id; Server is free-form/optional.
            Step::Config => self.config_ready(),
            Step::Server => true,
            Step::Name => id_valid(&self.id),
            Step::Review => id_valid(&self.id) && self.config_ready(),
        }
    }

    /// Does the config step carry enough to build the tunnel for this provider's
    /// method? OpenVPN needs an `.ovpn`; WireGuard needs either a pasted config
    /// or the four structured WG fields (key/address/peer/endpoint).
    #[must_use]
    pub fn config_ready(&self) -> bool {
        if self.method() == "ovpn" {
            return !self.ovpn.trim().is_empty();
        }
        if !self.wg_config.trim().is_empty() {
            return true;
        }
        !self.private_key.trim().is_empty()
            && !self.address.trim().is_empty()
            && !self.peer_public_key.trim().is_empty()
            && !self.endpoint.trim().is_empty()
    }

    pub fn update(&mut self, msg: WizardMsg) -> WizardAction {
        match msg {
            WizardMsg::SelectProvider(p) => {
                self.provider = p;
                WizardAction::None
            }
            WizardMsg::IdInput(s) => {
                if !self.editing {
                    self.id = sanitize_id(&s);
                }
                WizardAction::None
            }
            WizardMsg::ServerInput(s) => {
                self.server = s;
                WizardAction::None
            }
            WizardMsg::PrivateKeyInput(s) => {
                self.private_key = s;
                WizardAction::None
            }
            WizardMsg::AddressInput(s) => {
                self.address = s;
                WizardAction::None
            }
            WizardMsg::PeerPublicKeyInput(s) => {
                self.peer_public_key = s;
                WizardAction::None
            }
            WizardMsg::EndpointInput(s) => {
                self.endpoint = s;
                WizardAction::None
            }
            WizardMsg::DnsInput(s) => {
                self.dns = s;
                WizardAction::None
            }
            WizardMsg::PresharedKeyInput(s) => {
                self.preshared_key = s;
                WizardAction::None
            }
            WizardMsg::WgConfigInput(s) => {
                self.wg_config = s;
                WizardAction::None
            }
            WizardMsg::OvpnInput(s) => {
                self.ovpn = s;
                WizardAction::None
            }
            WizardMsg::Next => {
                if self.step() != Step::Review && self.can_advance() {
                    self.step_idx = self.step().next().ordinal();
                }
                WizardAction::None
            }
            WizardMsg::Back => {
                if self.step() != Step::Provider {
                    self.step_idx = self.step().prev().ordinal();
                }
                WizardAction::None
            }
            WizardMsg::Cancel => WizardAction::Cancel,
            WizardMsg::Save => {
                if self.step() == Step::Review && id_valid(&self.id) && self.config_ready() {
                    WizardAction::Save(self.setup_body())
                } else {
                    WizardAction::None
                }
            }
        }
    }

    /// Assemble the `setup-provider` request body from the form. Pure — the
    /// shape the responder's `setup_provider` decodes (provider + id + server,
    /// then either a pasted blob or the structured WG fields).
    #[must_use]
    pub fn setup_body(&self) -> String {
        let mut obj = serde_json::Map::new();
        obj.insert("provider".into(), self.provider.trim().into());
        obj.insert("id".into(), self.id.trim().into());
        obj.insert("server".into(), self.server.trim().into());
        if self.method() == "ovpn" {
            obj.insert("ovpn".into(), self.ovpn.trim().into());
        } else if !self.wg_config.trim().is_empty() {
            obj.insert("wg_config".into(), self.wg_config.trim().into());
        } else {
            obj.insert("private_key".into(), self.private_key.trim().into());
            obj.insert("address".into(), self.address.trim().into());
            obj.insert("peer_public_key".into(), self.peer_public_key.trim().into());
            obj.insert("endpoint".into(), self.endpoint.trim().into());
            if !self.dns.trim().is_empty() {
                obj.insert("dns".into(), self.dns.trim().into());
            }
            if !self.preshared_key.trim().is_empty() {
                obj.insert("preshared_key".into(), self.preshared_key.trim().into());
            }
        }
        serde_json::Value::Object(obj).to_string()
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let sizes = FontSize::defaults();
        let heading = if self.editing {
            "Edit tunnel"
        } else {
            "Add tunnel"
        };
        let title = text(format!("{heading} — step {} of 5", self.step().ordinal()))
            .size(TypeRole::Subheading.size_in(sizes))
            .colr(palette.text.into_cosmic_color());

        let body = match self.step() {
            Step::Provider => self.step_provider(palette),
            Step::Config => self.step_config(palette),
            Step::Server => self.step_server(palette),
            Step::Name => self.step_name(palette),
            Step::Review => self.step_review(palette),
        };

        let mut nav = row![wbtn(
            palette,
            "Cancel",
            ButtonVariant::Ghost,
            (!self.saving).then_some(WizardMsg::Cancel),
        )]
        .spacing(8);
        if self.step() != Step::Provider {
            nav = nav.push(wbtn(
                palette,
                "Back",
                ButtonVariant::Ghost,
                (!self.saving).then_some(WizardMsg::Back),
            ));
        }
        nav = nav.push(Space::new().width(Length::Fill));
        if self.step() == Step::Review {
            let save = (!self.saving && id_valid(&self.id) && self.config_ready())
                .then_some(WizardMsg::Save);
            nav = nav.push(wbtn(
                palette,
                if self.saving {
                    "Saving…"
                } else {
                    "Save tunnel"
                },
                ButtonVariant::Primary,
                save,
            ));
        } else {
            let next = self.can_advance().then_some(WizardMsg::Next);
            nav = nav.push(wbtn(palette, "Next", ButtonVariant::Primary, next));
        }

        let mut col = column![title, body].spacing(16);
        if !self.note.is_empty() {
            col = col.push(status_strip(&self.note, palette));
        }
        col = col.push(nav);

        container(scrollable(col.width(Length::Fill)).height(Length::Fill))
            .padding(Padding::from([24u16, 32u16]))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn step_provider(&self, palette: Palette) -> Element<'_, crate::Message> {
        let mut col = column![wlabel(palette, "Choose a provider")].spacing(8);
        if self.providers.is_empty() {
            col = col.push(wmuted(palette, "Loading provider catalog…"));
        }
        for p in &self.providers {
            let selected = p.id == self.provider;
            let variant = if selected {
                ButtonVariant::Secondary
            } else {
                ButtonVariant::Ghost
            };
            let lbl = format!("{} · {}", p.id, p.method);
            col = col.push(wbtn(
                palette,
                &lbl,
                variant,
                Some(WizardMsg::SelectProvider(p.id.clone())),
            ));
        }
        col.into()
    }

    fn step_config(&self, palette: Palette) -> Element<'_, crate::Message> {
        let method = self.method();
        let mut col = column![wlabel(
            palette,
            &format!("{} configuration ({method})", self.provider),
        )]
        .spacing(8);
        if method == "ovpn" {
            col = col.push(wmuted(palette, "Paste the provider's .ovpn config:"));
            col = col.push(winput(".ovpn config", &self.ovpn, WizardMsg::OvpnInput));
        } else {
            col = col.push(wmuted(
                palette,
                "Paste a WireGuard .conf, or fill the fields below.",
            ));
            col = col.push(winput(
                "WireGuard .conf (paste, optional)",
                &self.wg_config,
                WizardMsg::WgConfigInput,
            ));
            if self.wg_config.trim().is_empty() {
                col = col
                    .push(winput(
                        "PrivateKey",
                        &self.private_key,
                        WizardMsg::PrivateKeyInput,
                    ))
                    .push(winput("Address", &self.address, WizardMsg::AddressInput))
                    .push(winput(
                        "Peer PublicKey",
                        &self.peer_public_key,
                        WizardMsg::PeerPublicKeyInput,
                    ))
                    .push(winput(
                        "Endpoint (host:port)",
                        &self.endpoint,
                        WizardMsg::EndpointInput,
                    ))
                    .push(winput("DNS (optional)", &self.dns, WizardMsg::DnsInput))
                    .push(winput(
                        "PresharedKey (optional)",
                        &self.preshared_key,
                        WizardMsg::PresharedKeyInput,
                    ));
            }
        }
        col.into()
    }

    fn step_server(&self, palette: Palette) -> Element<'_, crate::Message> {
        column![
            wlabel(palette, "Server / region"),
            wmuted(
                palette,
                "Provider-specific selector (e.g. us-nyc); leave blank for a \
                 generic tunnel.",
            ),
            winput("Server / region", &self.server, WizardMsg::ServerInput),
        ]
        .spacing(8)
        .into()
    }

    fn step_name(&self, palette: Palette) -> Element<'_, crate::Message> {
        let mut col = column![
            wlabel(palette, "Tunnel name"),
            wmuted(
                palette,
                "A unique id on this node — drives the mvpn-<id> interface. Add a \
                 suffix (mullvad2) for a second instance of the same provider.",
            ),
        ]
        .spacing(8);
        if self.editing {
            col = col.push(wmuted(palette, &format!("Editing tunnel '{}'.", self.id)));
        } else {
            col = col.push(winput(
                "tunnel id (e.g. mullvad1)",
                &self.id,
                WizardMsg::IdInput,
            ));
            if !id_valid(&self.id) {
                col = col.push(wmuted(
                    palette,
                    "Id must have at least one letter or digit (letters/digits/-).",
                ));
            }
            if let Some(p) = self.selected() {
                if !p.multi_instance {
                    col = col.push(wmuted(
                        palette,
                        "This provider runs a single instance per node.",
                    ));
                }
            }
        }
        col.into()
    }

    fn step_review(&self, palette: Palette) -> Element<'_, crate::Message> {
        let ifname = mackes_mesh_types::vpn::TunnelDef {
            id: self.id.clone(),
            ..Default::default()
        }
        .ifname();
        let check = self
            .selected()
            .and_then(|p| p.exit_check.clone())
            .unwrap_or_else(|| "no first-party reflector — generic exit check".into());
        let cfg_src = if self.method() == "ovpn" {
            "pasted .ovpn"
        } else if !self.wg_config.trim().is_empty() {
            "pasted WireGuard .conf"
        } else {
            "structured WireGuard fields"
        };
        column![
            wkv(palette, "Provider", &self.provider),
            wkv(palette, "Method", self.method()),
            wkv(palette, "Tunnel id", &self.id),
            wkv(palette, "Interface", &ifname),
            wkv(
                palette,
                "Server",
                if self.server.trim().is_empty() {
                    "(generic)"
                } else {
                    self.server.trim()
                },
            ),
            wkv(palette, "Config", cfg_src),
            wkv(palette, "Exit check", &check),
            wmuted(
                palette,
                "Save age-encrypts the secret into the mesh store and persists the \
                 tunnel; then bring it up and Verify the exit IP.",
            ),
        ]
        .spacing(6)
        .into()
    }
}

/// A labelled single-line text input (the wizard's field widget).
fn winput<'a>(
    placeholder: &str,
    value: &str,
    on_input: impl Fn(String) -> WizardMsg + 'a,
) -> Element<'a, crate::Message> {
    text_input(placeholder, value)
        .on_input(move |s| crate::Message::Vpn(Message::Wizard(on_input(s))))
        .padding(8)
        .size(TypeRole::Body.size_in(FontSize::defaults()))
        .into()
}

fn wlabel<'a>(palette: Palette, t: &str) -> Element<'a, crate::Message> {
    text(t.to_string())
        .size(TypeRole::Body.size_in(FontSize::defaults()))
        .colr(palette.text.into_cosmic_color())
        .into()
}

fn wmuted<'a>(palette: Palette, t: &str) -> Element<'a, crate::Message> {
    text(t.to_string())
        .size(TypeRole::Caption.size_in(FontSize::defaults()))
        .colr(palette.text_muted.into_cosmic_color())
        .into()
}

fn wkv<'a>(palette: Palette, k: &str, v: &str) -> Element<'a, crate::Message> {
    row![
        text(k.to_string())
            .size(TypeRole::Caption.size_in(FontSize::defaults()))
            .colr(palette.text_muted.into_cosmic_color())
            .width(Length::Fixed(110.0)),
        text(v.to_string())
            .size(TypeRole::Body.size_in(FontSize::defaults()))
            .colr(palette.text.into_cosmic_color()),
    ]
    .spacing(8)
    .into()
}

fn wbtn<'a>(
    palette: Palette,
    label: &str,
    variant: ButtonVariant,
    msg: Option<WizardMsg>,
) -> Element<'a, crate::Message> {
    variant_button(
        label.to_string(),
        variant,
        msg.map(|m| crate::Message::Vpn(Message::Wizard(m))),
        palette,
    )
}

/// Sanitize a tunnel id as typed: ASCII alphanumeric + hyphen only (the chars
/// that survive `TunnelDef::ifname`'s sanitize plus a readable separator).
fn sanitize_id(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect()
}

/// A tunnel id is valid when it has at least one alphanumeric char (so its
/// `mvpn-<body>` interface name isn't the bare prefix — the model's rule).
#[must_use]
pub fn id_valid(id: &str) -> bool {
    id.chars().any(|c| c.is_ascii_alphanumeric())
}

// ---- I/O ------------------------------------------------------

/// Build the task for an up/down/remove action: publish `action/vpn/<verb>`
/// with the tunnel id as the body, decode the reply, and route it back as an
/// [`Message::OperationFinished`]. Blocking (the Bus client owns a
/// current-thread runtime) → `spawn_blocking`, never the iced executor.
fn op_task(verb: &'static str, id: String) -> Task<crate::Message> {
    Task::perform(
        async move {
            let joined = tokio::task::spawn_blocking(move || request_op(verb, &id)).await;
            let result = joined.unwrap_or_else(|e| Err(format!("vpn op task: {e}")));
            crate::Message::Vpn(Message::OperationFinished(result))
        },
        |m| m,
    )
}

/// Build the task for a `verify-egress` probe, folding the verdict back into the
/// matching row via [`Message::Verified`]. Blocking → `spawn_blocking`.
fn verify_task(id: String) -> Task<crate::Message> {
    Task::perform(
        async move {
            let probe_id = id.clone();
            let joined = tokio::task::spawn_blocking(move || request_verify(&probe_id)).await;
            let result = joined.unwrap_or_else(|e| Err(format!("vpn verify task: {e}")));
            crate::Message::Vpn(Message::Verified { id, result })
        },
        |m| m,
    )
}

/// Build the task for a `setup-provider` save (the wizard's terminal action).
/// Blocking → `spawn_blocking`.
fn setup_task(body: String) -> Task<crate::Message> {
    Task::perform(
        async move {
            let joined = tokio::task::spawn_blocking(move || request_setup(&body)).await;
            let result = joined.unwrap_or_else(|e| Err(format!("vpn setup task: {e}")));
            crate::Message::Vpn(Message::SetupFinished(result))
        },
        |m| m,
    )
}

/// Fetch the tunnel list over the Bus and pair each with its liveness. Returns
/// `Err` when the responder doesn't answer (daemon down / no Bus). Blocking.
fn fetch_tunnels() -> Result<Vec<VpnRow>, String> {
    let raw = crate::dbus::action_request("action/vpn/list-tunnels", VPN_TIMEOUT)
        .ok_or_else(|| "mackesd not reachable over the Bus (vpn/list-tunnels)".to_string())?;
    let mut rows = parse_tunnels_reply(&raw)?;
    // Pair each tunnel with its live up/down. A status probe that doesn't answer
    // leaves the row's default `up=false` — the list still renders.
    for row in &mut rows {
        if let Some(reply) = crate::dbus::action_request_with_body(
            "action/vpn/tunnel-status",
            Some(&row.id),
            VPN_TIMEOUT,
        ) {
            row.up = parse_status_up(&reply);
        }
    }
    Ok(rows)
}

/// Fetch the provider catalog over the Bus (`list-providers`). Blocking.
fn fetch_providers() -> Result<Vec<ProviderInfo>, String> {
    let raw = crate::dbus::action_request("action/vpn/list-providers", VPN_TIMEOUT)
        .ok_or_else(|| "mackesd not reachable over the Bus (vpn/list-providers)".to_string())?;
    parse_providers_reply(&raw)
}

/// Request one `action/vpn/<verb>` over the Bus with `id` as the body, decoding
/// the `{ok, detail?}` reply into a human-readable result. Blocking.
fn request_op(verb: &str, id: &str) -> Result<String, String> {
    let topic = format!("action/vpn/{verb}");
    let raw = crate::dbus::action_request_with_body(&topic, Some(id), VPN_TIMEOUT)
        .ok_or_else(|| format!("mackesd not reachable over the Bus (vpn/{verb})"))?;
    parse_op_reply(verb, id, &raw)
}

/// Request `verify-egress` for `id` over the Bus, decoding the report into an
/// [`EgressVerdict`]. Blocking; uses the longer reflector-round-trip budget.
fn request_verify(id: &str) -> Result<EgressVerdict, String> {
    let raw = crate::dbus::action_request_with_body(
        "action/vpn/verify-egress",
        Some(id),
        VPN_VERIFY_TIMEOUT,
    )
    .ok_or_else(|| "mackesd not reachable over the Bus (vpn/verify-egress)".to_string())?;
    parse_verify_reply(&raw)
}

/// Request `setup-provider` with the wizard's assembled body over the Bus,
/// decoding the reply into a human result. Blocking.
fn request_setup(body: &str) -> Result<String, String> {
    let raw = crate::dbus::action_request_with_body(
        "action/vpn/setup-provider",
        Some(body),
        VPN_VERIFY_TIMEOUT,
    )
    .ok_or_else(|| "mackesd not reachable over the Bus (vpn/setup-provider)".to_string())?;
    parse_setup_reply(&raw)
}

/// Pure decoder for the `list-tunnels` reply envelope
/// `{"ok":true,"tunnels":[<TunnelDef>...]}` → one [`VpnRow`] per tunnel (live
/// up/down filled in later by the paired status probe). `{"error":m}` → `Err`.
pub fn parse_tunnels_reply(raw: &str) -> Result<Vec<VpnRow>, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw.trim()).map_err(|e| format!("bad list-tunnels reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(serde_json::Value::as_str) {
        return Err(err.to_string());
    }
    let tunnels = v
        .get("tunnels")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "list-tunnels reply missing 'tunnels'".to_string())?;
    Ok(tunnels.iter().map(row_from_tunnel).collect())
}

/// Build a [`VpnRow`] from one `TunnelDef` JSON object. The `mvpn-<id>`
/// interface is derived the same way the model does
/// (`mackes_mesh_types::vpn::TunnelDef::ifname`) so the card shows the real
/// device name. `up`/`exit_ip`/`health` default empty — the probes fill them.
fn row_from_tunnel(t: &serde_json::Value) -> VpnRow {
    let s = |k: &str| {
        t.get(k)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let def = mackes_mesh_types::vpn::TunnelDef {
        id: s("id"),
        ..Default::default()
    };
    VpnRow {
        ifname: def.ifname(),
        id: s("id"),
        provider: s("provider"),
        server: s("server"),
        protocol: s("protocol"),
        // `method` is a kebab-case enum on the wire (`wg`/`ovpn`/`cli`/`api`);
        // default to `wg` when absent, matching `Method::default`.
        method: {
            let m = s("method");
            if m.is_empty() {
                "wg".to_string()
            } else {
                m
            }
        },
        up: false,
        exit_ip: None,
        health: String::new(),
        health_detail: String::new(),
    }
}

/// Pure decoder for the `list-providers` reply
/// `{"ok":true,"providers":[{id,method,multi_instance,exit_check,...}]}` →
/// the wizard's [`ProviderInfo`] catalog. `{"error":m}` → `Err`.
pub fn parse_providers_reply(raw: &str) -> Result<Vec<ProviderInfo>, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw.trim()).map_err(|e| format!("bad list-providers reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(serde_json::Value::as_str) {
        return Err(err.to_string());
    }
    let providers = v
        .get("providers")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "list-providers reply missing 'providers'".to_string())?;
    Ok(providers
        .iter()
        .map(|p| ProviderInfo {
            id: p
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string(),
            method: p
                .get("method")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("wg")
                .to_string(),
            multi_instance: p
                .get("multi_instance")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            exit_check: p
                .get("exit_check")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
        })
        .filter(|p| !p.id.is_empty())
        .collect())
}

/// Pure decoder for the `tunnel-status` reply `{"ok":true,"up":bool}` → the
/// `up` bool (false on any error/missing field — a status we can't read is
/// "not known up").
#[must_use]
pub fn parse_status_up(raw: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(raw.trim())
        .ok()
        .and_then(|v| v.get("up").and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}

/// Pure decoder for the `verify-egress` reply
/// `{"ok":true,"report":{"verified_exit_ip":...,"health":"ok","detail":...}}`
/// → the [`EgressVerdict`] folded into the row. `{"error":m}` → `Err`.
pub fn parse_verify_reply(raw: &str) -> Result<EgressVerdict, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw.trim()).map_err(|e| format!("bad verify-egress reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(serde_json::Value::as_str) {
        return Err(err.to_string());
    }
    let report = v
        .get("report")
        .ok_or_else(|| "verify-egress reply missing 'report'".to_string())?;
    let detail = report
        .get("detail")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    Ok(EgressVerdict {
        exit_ip: report
            .get("verified_exit_ip")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        health: report
            .get("health")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string(),
        detail,
    })
}

/// Pure decoder for a `setup-provider` reply into a human result line. The
/// responder replies `{ok:true,id,ifname,secret_distributed,secret_note,…}` or
/// `{error:m}`. A successful save with the secret undistributed surfaces the
/// honest note, not a silent success.
pub fn parse_setup_reply(raw: &str) -> Result<String, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw.trim()).map_err(|e| format!("bad setup-provider reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(serde_json::Value::as_str) {
        return Err(err.to_string());
    }
    if !v
        .get("ok")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return Err("setup-provider did not run".into());
    }
    let id = v
        .get("id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let distributed = v
        .get("secret_distributed")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if distributed {
        Ok(format!("Saved {id} — secret distributed to the mesh."))
    } else {
        let note = v
            .get("secret_note")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or("secret not distributed");
        Ok(format!("Saved {id} — {note}."))
    }
}

/// Pure decoder for an up/down/remove reply into a human-readable status line.
/// The responder replies `{"ok":bool,"detail":...}` (tunnel-up/down) or
/// `{"ok":true}` (remove), or `{"error":m}`. Split out for unit-testing.
pub fn parse_op_reply(verb: &str, id: &str, raw: &str) -> Result<String, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw.trim()).map_err(|e| format!("bad {verb} reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(serde_json::Value::as_str) {
        return Err(err.to_string());
    }
    let ok = v
        .get("ok")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let detail = v.get("detail").and_then(serde_json::Value::as_str);
    match (verb, ok) {
        ("tunnel-up", true) => Ok(format!("{id} up — {}", detail.unwrap_or("brought up"))),
        ("tunnel-down", true) => Ok(format!("{id} down — {}", detail.unwrap_or("brought down"))),
        ("remove-tunnel", true) => Ok(format!("Removed {id}.")),
        // ok:false carries the responder's honest detail (e.g. "config missing").
        (_, false) => Err(detail.map_or_else(
            || format!("{verb} {id} did not run"),
            |d| format!("{verb} {id}: {d}"),
        )),
        (_, true) => Ok(format!("{verb} {id}: ok")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tunnels_reply_maps_tunneldefs_to_rows() {
        // The exact `list-tunnels` envelope the responder emits
        // (`json!({"ok":true,"tunnels":cfg.tunnel})`) over the `TunnelDef`
        // serde shape.
        let raw = r#"{"ok":true,"tunnels":[
            {"id":"mullvad1","provider":"mullvad","method":"wg",
             "server":"us-nyc","protocol":"udp","creds_ref":""},
            {"id":"work","provider":"generic-ovpn","method":"ovpn",
             "server":"","protocol":"tcp","creds_ref":""}
        ]}"#;
        let rows = parse_tunnels_reply(raw).expect("ok envelope decodes");
        assert_eq!(rows.len(), 2);

        let m = &rows[0];
        assert_eq!(m.id, "mullvad1");
        assert_eq!(m.provider, "mullvad");
        assert_eq!(m.method, "wg");
        assert_eq!(m.server, "us-nyc");
        assert_eq!(m.protocol, "udp");
        // The interface name is derived the same way the model does.
        assert_eq!(m.ifname, "mvpn-mullvad1");
        assert!(
            !m.up,
            "liveness defaults down until the status probe fills it"
        );
        assert!(m.exit_ip.is_none(), "exit IP defaults empty until Verify");
        assert!(m.health.is_empty());

        let w = &rows[1];
        assert_eq!(w.provider, "generic-ovpn");
        assert_eq!(w.method, "ovpn");
        assert_eq!(w.ifname, "mvpn-work");
    }

    #[test]
    fn parse_tunnels_reply_defaults_absent_method_to_wg() {
        let raw = r#"{"ok":true,"tunnels":[{"id":"x","provider":"generic-wg"}]}"#;
        let rows = parse_tunnels_reply(raw).unwrap();
        assert_eq!(rows[0].method, "wg");
        assert_eq!(rows[0].ifname, "mvpn-x");
    }

    #[test]
    fn parse_tunnels_reply_empty_list_is_ok() {
        let rows = parse_tunnels_reply(r#"{"ok":true,"tunnels":[]}"#).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn parse_tunnels_reply_surfaces_error_envelope() {
        let err = parse_tunnels_reply(r#"{"error":"vpn config unreadable"}"#).unwrap_err();
        assert!(err.contains("vpn config unreadable"));
    }

    #[test]
    fn parse_tunnels_reply_rejects_garbage_and_missing_field() {
        assert!(parse_tunnels_reply("not json").is_err());
        assert!(
            parse_tunnels_reply(r#"{"ok":true}"#).is_err(),
            "missing 'tunnels' is an error, not an empty list"
        );
    }

    #[test]
    fn parse_status_up_reads_the_up_bool() {
        assert!(parse_status_up(
            r#"{"ok":true,"ifname":"mvpn-x","up":true}"#
        ));
        assert!(!parse_status_up(
            r#"{"ok":true,"ifname":"mvpn-x","up":false}"#
        ));
        assert!(!parse_status_up(r#"{"error":"no such tunnel"}"#));
        assert!(!parse_status_up("garbage"));
    }

    #[test]
    fn parse_verify_reply_reads_exit_ip_health_and_detail() {
        // The exact `verify-egress` envelope: `{ok, report:<TunnelReport>}`.
        let raw = r#"{"ok":true,"report":{
            "id":"mullvad1","ifname":"mvpn-mullvad1","health":"ok",
            "verified_exit_ip":"203.0.113.7","wan_ip":"198.51.100.2",
            "detail":"exit confirmed (provider self-attested)"}}"#;
        let v = parse_verify_reply(raw).expect("ok envelope decodes");
        assert_eq!(v.exit_ip.as_deref(), Some("203.0.113.7"));
        assert_eq!(v.health, "ok");
        assert!(v.detail.contains("exit confirmed"));
    }

    #[test]
    fn parse_verify_reply_surfaces_a_leak_and_a_null_exit() {
        let leak = parse_verify_reply(
            r#"{"ok":true,"report":{"id":"m1","ifname":"mvpn-m1","health":"leaking",
                "verified_exit_ip":"198.51.100.2","wan_ip":"198.51.100.2",
                "detail":"LEAK: exit IP equals the WAN IP — egress is not tunneling"}}"#,
        )
        .unwrap();
        assert_eq!(leak.health, "leaking");
        assert!(leak.detail.contains("LEAK"));

        // A down/unverifiable tunnel reports a null exit IP — decoded as None.
        let down = parse_verify_reply(
            r#"{"ok":true,"report":{"id":"m1","ifname":"mvpn-m1","health":"down",
                "verified_exit_ip":null,"wan_ip":null,"detail":"interface mvpn-m1 absent"}}"#,
        )
        .unwrap();
        assert!(down.exit_ip.is_none());
        assert_eq!(down.health, "down");
    }

    #[test]
    fn parse_verify_reply_errors_on_error_envelope_and_missing_report() {
        assert!(parse_verify_reply(r#"{"error":"no such tunnel 'x'"}"#).is_err());
        assert!(parse_verify_reply(r#"{"ok":true}"#).is_err());
        assert!(parse_verify_reply("not json").is_err());
    }

    #[test]
    fn parse_providers_reply_maps_the_catalog() {
        // The exact `list-providers` envelope the responder emits.
        let raw = r#"{"ok":true,"providers":[
            {"id":"mullvad","method":"wg","cli":"mullvad","wg_port":51820,
             "multi_instance":true,"exit_check":"https://am.i.mullvad.net/json"},
            {"id":"generic-ovpn","method":"ovpn","cli":null,"wg_port":0,
             "multi_instance":true,"exit_check":null}
        ]}"#;
        let cat = parse_providers_reply(raw).expect("ok envelope decodes");
        assert_eq!(cat.len(), 2);
        assert_eq!(cat[0].id, "mullvad");
        assert_eq!(cat[0].method, "wg");
        assert!(cat[0].multi_instance);
        assert_eq!(
            cat[0].exit_check.as_deref(),
            Some("https://am.i.mullvad.net/json")
        );
        assert_eq!(cat[1].method, "ovpn");
        assert!(cat[1].exit_check.is_none(), "empty/null exit_check → None");
    }

    #[test]
    fn parse_providers_reply_errors_on_error_and_missing() {
        assert!(parse_providers_reply(r#"{"error":"x"}"#).is_err());
        assert!(parse_providers_reply(r#"{"ok":true}"#).is_err());
    }

    #[test]
    fn parse_setup_reply_distributed_and_pending_and_error() {
        let ok = parse_setup_reply(
            r#"{"ok":true,"id":"mullvad1","ifname":"mvpn-mullvad1",
                "secret_distributed":true,"secret_note":""}"#,
        )
        .unwrap();
        assert!(ok.contains("mullvad1"));
        assert!(ok.contains("distributed"));

        // Save succeeded but the store was unreachable — surface the honest note.
        let pending = parse_setup_reply(
            r#"{"ok":true,"id":"m1","secret_distributed":false,
                "secret_note":"secret not distributed: store offline"}"#,
        )
        .unwrap();
        assert!(pending.contains("store offline"), "{pending}");

        assert!(parse_setup_reply(r#"{"error":"unknown provider 'x'"}"#).is_err());
    }

    #[test]
    fn parse_op_reply_up_down_and_remove_humanise() {
        let up = parse_op_reply(
            "tunnel-up",
            "mullvad1",
            r#"{"ok":true,"ifname":"mvpn-mullvad1","detail":"wg-quick up"}"#,
        )
        .unwrap();
        assert!(up.contains("mullvad1 up"));
        assert!(up.contains("wg-quick up"));

        let down = parse_op_reply(
            "tunnel-down",
            "mullvad1",
            r#"{"ok":true,"ifname":"mvpn-mullvad1","detail":"wg-quick down"}"#,
        )
        .unwrap();
        assert!(down.contains("mullvad1 down"));

        let removed = parse_op_reply("remove-tunnel", "old", r#"{"ok":true}"#).unwrap();
        assert_eq!(removed, "Removed old.");
    }

    #[test]
    fn parse_op_reply_ok_false_is_an_honest_error_with_detail() {
        let e = parse_op_reply(
            "tunnel-up",
            "ovpn1",
            r#"{"ok":false,"ifname":"mvpn-ovpn1","detail":"openvpn config missing"}"#,
        )
        .unwrap_err();
        assert!(e.contains("openvpn config missing"), "{e}");
        assert!(e.contains("ovpn1"));
    }

    #[test]
    fn parse_op_reply_error_envelope_and_garbage() {
        let e = parse_op_reply(
            "remove-tunnel",
            "ghost",
            r#"{"error":"no such tunnel 'ghost'"}"#,
        )
        .unwrap_err();
        assert!(e.contains("no such tunnel"));
        assert!(parse_op_reply("tunnel-up", "x", "not json").is_err());
    }

    #[test]
    fn detail_line_drops_empty_parts() {
        let full = VpnRow {
            server: "us-nyc".into(),
            protocol: "udp".into(),
            method: "wg".into(),
            ..Default::default()
        };
        assert_eq!(detail_line(&full), "us-nyc · udp · wg");
        let bare = VpnRow {
            method: "wg".into(),
            ..Default::default()
        };
        assert_eq!(detail_line(&bare), "wg");
    }

    #[test]
    fn kill_switch_is_the_inverse_of_liveness() {
        let up = VpnRow {
            up: true,
            ..Default::default()
        };
        assert!(!up.kill_switch_engaged(), "an up tunnel clears the switch");
        let down = VpnRow {
            up: false,
            ..Default::default()
        };
        assert!(
            down.kill_switch_engaged(),
            "a down tunnel engages the kill-switch (egress blocked)"
        );
    }

    #[test]
    fn health_badge_maps_every_verdict_tag() {
        let p = Palette::dark();
        // Each tag yields a stable label; unknown falls through to "unknown".
        assert_eq!(health_badge("ok", p).1, "exit confirmed");
        assert_eq!(health_badge("leaking", p).1, "LEAKING");
        assert_eq!(health_badge("dns-leak", p).1, "DNS leak");
        assert_eq!(health_badge("down", p).1, "interface down");
        assert_eq!(health_badge("unverifiable", p).1, "unverifiable");
        assert_eq!(health_badge("", p).1, "unknown");
    }

    // ── panel state machine ──

    #[test]
    fn loaded_ok_records_tunnels_and_clears_busy() {
        let mut panel = VpnPanel::new();
        panel.busy = true;
        let rows = parse_tunnels_reply(
            r#"{"ok":true,"tunnels":[{"id":"m1","provider":"mullvad","method":"wg"}]}"#,
        )
        .unwrap();
        let _ = panel.update(Message::Loaded(Ok(rows)));
        assert!(panel.daemon_up);
        assert!(!panel.busy);
        assert_eq!(panel.tunnels.len(), 1);
        assert_eq!(panel.tunnels[0].ifname, "mvpn-m1");
    }

    #[test]
    fn loaded_ok_preserves_a_prior_verified_exit_ip() {
        // A plain list refresh must not wipe an exit IP the operator just
        // verified for a still-present tunnel.
        let mut panel = VpnPanel::new();
        panel.tunnels = vec![VpnRow {
            id: "m1".into(),
            provider: "mullvad".into(),
            exit_ip: Some("203.0.113.7".into()),
            health: "ok".into(),
            health_detail: "exit confirmed".into(),
            ..Default::default()
        }];
        let fresh = parse_tunnels_reply(
            r#"{"ok":true,"tunnels":[{"id":"m1","provider":"mullvad","method":"wg"}]}"#,
        )
        .unwrap();
        let _ = panel.update(Message::Loaded(Ok(fresh)));
        assert_eq!(panel.tunnels[0].exit_ip.as_deref(), Some("203.0.113.7"));
        assert_eq!(panel.tunnels[0].health, "ok");
    }

    #[test]
    fn loaded_err_marks_daemon_down_and_clears_tunnels() {
        let mut panel = VpnPanel::new();
        panel.daemon_up = true;
        panel.tunnels = vec![VpnRow::default()];
        let _ = panel.update(Message::Loaded(Err("Bus unreachable".into())));
        assert!(!panel.daemon_up);
        assert!(panel.tunnels.is_empty());
        assert_eq!(panel.status, "Bus unreachable");
    }

    #[test]
    fn toggle_and_remove_while_busy_are_noops() {
        let mut panel = VpnPanel::new();
        panel.busy = true;
        panel.status = "stale".into();
        let _ = panel.update(Message::ToggleClicked {
            id: "m1".into(),
            up: true,
        });
        assert_eq!(panel.status, "stale");
        let _ = panel.update(Message::RemoveClicked { id: "m1".into() });
        assert_eq!(panel.status, "stale");
    }

    #[test]
    fn toggle_sets_busy_and_a_pending_status() {
        let mut panel = VpnPanel::new();
        let _ = panel.update(Message::ToggleClicked {
            id: "m1".into(),
            up: true,
        });
        assert!(panel.busy);
        assert!(panel.status.contains("Bringing up"));
        assert!(panel.status.contains("m1"));
    }

    #[test]
    fn verified_folds_exit_ip_into_the_matching_row() {
        let mut panel = VpnPanel::new();
        panel.tunnels = vec![VpnRow {
            id: "m1".into(),
            ..Default::default()
        }];
        panel.busy = true;
        let _ = panel.update(Message::Verified {
            id: "m1".into(),
            result: Ok(EgressVerdict {
                exit_ip: Some("203.0.113.7".into()),
                health: "ok".into(),
                detail: "exit confirmed (≠ WAN)".into(),
            }),
        });
        assert!(!panel.busy);
        assert_eq!(panel.tunnels[0].exit_ip.as_deref(), Some("203.0.113.7"));
        assert_eq!(panel.tunnels[0].health, "ok");
        assert!(panel.status.contains("exit confirmed"));
    }

    #[test]
    fn verify_error_records_status_without_touching_the_row() {
        let mut panel = VpnPanel::new();
        panel.tunnels = vec![VpnRow {
            id: "m1".into(),
            exit_ip: Some("203.0.113.7".into()),
            ..Default::default()
        }];
        panel.busy = true;
        let _ = panel.update(Message::Verified {
            id: "m1".into(),
            result: Err("verify-egress: no such tunnel".into()),
        });
        assert!(!panel.busy);
        assert_eq!(panel.status, "verify-egress: no such tunnel");
        // The prior exit IP is untouched by a failed verify.
        assert_eq!(panel.tunnels[0].exit_ip.as_deref(), Some("203.0.113.7"));
    }

    #[test]
    fn operation_finished_clears_busy_and_records_status() {
        let mut panel = VpnPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::OperationFinished(Ok("m1 up — wg-quick up".into())));
        assert!(!panel.busy);
        assert!(panel.status.contains("m1 up"));

        panel.busy = true;
        let _ = panel.update(Message::OperationFinished(Err("auth failed".into())));
        assert_eq!(panel.status, "auth failed");
        assert!(!panel.busy);
    }

    #[test]
    fn refresh_while_busy_is_a_noop() {
        let mut panel = VpnPanel::new();
        panel.busy = true;
        panel.status = "stale".into();
        let _ = panel.update(Message::RefreshClicked);
        assert_eq!(panel.status, "stale");
    }

    // ── wizard ──

    #[test]
    fn open_wizard_seeds_an_edit_from_the_row() {
        let mut panel = VpnPanel::new();
        panel.tunnels = vec![VpnRow {
            id: "mullvad1".into(),
            provider: "mullvad".into(),
            server: "us-nyc".into(),
            ..Default::default()
        }];
        let _ = panel.update(Message::OpenWizard {
            edit_id: Some("mullvad1".into()),
        });
        let w = panel.wizard.as_ref().expect("wizard opened");
        assert!(w.editing);
        assert_eq!(w.provider, "mullvad");
        assert_eq!(w.id, "mullvad1");
        assert_eq!(w.server, "us-nyc");
    }

    #[test]
    fn open_wizard_fresh_has_no_seed() {
        let mut panel = VpnPanel::new();
        let _ = panel.update(Message::OpenWizard { edit_id: None });
        let w = panel.wizard.as_ref().unwrap();
        assert!(!w.editing);
        assert!(w.provider.is_empty());
        assert_eq!(w.step(), Step::Provider);
    }

    #[test]
    fn providers_loaded_fills_the_wizard_catalog() {
        let mut panel = VpnPanel::new();
        let _ = panel.update(Message::OpenWizard { edit_id: None });
        let cat = parse_providers_reply(
            r#"{"ok":true,"providers":[{"id":"mullvad","method":"wg","multi_instance":true}]}"#,
        )
        .unwrap();
        let _ = panel.update(Message::ProvidersLoaded(Ok(cat)));
        assert_eq!(panel.wizard.as_ref().unwrap().providers.len(), 1);
    }

    #[test]
    fn wizard_walks_steps_only_when_each_gate_is_satisfied() {
        let mut w = Wizard::new(None);
        w.providers = vec![ProviderInfo {
            id: "mullvad".into(),
            method: "wg".into(),
            multi_instance: true,
            exit_check: Some("https://am.i.mullvad.net/json".into()),
        }];
        // Provider step blocks Next until a provider is chosen.
        assert!(!w.can_advance());
        let _ = w.update(WizardMsg::Next);
        assert_eq!(w.step(), Step::Provider);
        let _ = w.update(WizardMsg::SelectProvider("mullvad".into()));
        assert!(w.can_advance());
        let _ = w.update(WizardMsg::Next);
        assert_eq!(w.step(), Step::Config);

        // Config (wg) needs the four structured fields (or a pasted config).
        assert!(!w.can_advance());
        let _ = w.update(WizardMsg::PrivateKeyInput("k".into()));
        let _ = w.update(WizardMsg::AddressInput("10.0.0.2/32".into()));
        let _ = w.update(WizardMsg::PeerPublicKeyInput("pk".into()));
        let _ = w.update(WizardMsg::EndpointInput("1.2.3.4:51820".into()));
        assert!(w.can_advance());
        let _ = w.update(WizardMsg::Next);
        assert_eq!(w.step(), Step::Server);

        // Server is optional.
        let _ = w.update(WizardMsg::Next);
        assert_eq!(w.step(), Step::Name);

        // Name gates on a usable id.
        assert!(!w.can_advance());
        let _ = w.update(WizardMsg::IdInput("mullvad1".into()));
        assert!(w.can_advance());
        let _ = w.update(WizardMsg::Next);
        assert_eq!(w.step(), Step::Review);
    }

    #[test]
    fn wizard_pasted_wg_config_satisfies_the_config_gate() {
        let mut w = Wizard::new(None);
        w.provider = "mullvad".into();
        w.providers = vec![ProviderInfo {
            id: "mullvad".into(),
            method: "wg".into(),
            ..Default::default()
        }];
        assert!(!w.config_ready());
        let _ = w.update(WizardMsg::WgConfigInput(
            "[Interface]\nPrivateKey = x\n".into(),
        ));
        assert!(w.config_ready(), "a pasted config alone is enough");
    }

    #[test]
    fn wizard_ovpn_method_needs_an_ovpn_blob() {
        let mut w = Wizard::new(None);
        w.provider = "generic-ovpn".into();
        w.providers = vec![ProviderInfo {
            id: "generic-ovpn".into(),
            method: "ovpn".into(),
            ..Default::default()
        }];
        assert_eq!(w.method(), "ovpn");
        assert!(!w.config_ready());
        let _ = w.update(WizardMsg::OvpnInput("client\ndev tun\n".into()));
        assert!(w.config_ready());
    }

    #[test]
    fn wizard_save_body_uses_the_paste_path_when_a_config_is_present() {
        let mut w = Wizard::new(None);
        w.provider = "mullvad".into();
        w.id = "mullvad1".into();
        w.server = "us-nyc".into();
        w.providers = vec![ProviderInfo {
            id: "mullvad".into(),
            method: "wg".into(),
            ..Default::default()
        }];
        w.wg_config = "[Interface]\nPrivateKey = x\n".into();
        let body: serde_json::Value = serde_json::from_str(&w.setup_body()).unwrap();
        assert_eq!(body["provider"], "mullvad");
        assert_eq!(body["id"], "mullvad1");
        assert_eq!(body["server"], "us-nyc");
        // The blob is trimmed before it goes on the wire.
        assert_eq!(body["wg_config"], "[Interface]\nPrivateKey = x");
        assert!(body.get("private_key").is_none(), "paste path omits fields");
    }

    #[test]
    fn wizard_save_body_uses_structured_wg_fields_when_no_paste() {
        let mut w = Wizard::new(None);
        w.provider = "ivpn".into();
        w.id = "ivpn1".into();
        w.providers = vec![ProviderInfo {
            id: "ivpn".into(),
            method: "wg".into(),
            ..Default::default()
        }];
        w.private_key = "k".into();
        w.address = "10.0.0.2/32".into();
        w.peer_public_key = "pk".into();
        w.endpoint = "1.2.3.4:51820".into();
        let body: serde_json::Value = serde_json::from_str(&w.setup_body()).unwrap();
        assert_eq!(body["private_key"], "k");
        assert_eq!(body["endpoint"], "1.2.3.4:51820");
        assert!(body.get("wg_config").is_none());
        // Optional fields absent → omitted, not empty strings.
        assert!(body.get("dns").is_none());
    }

    #[test]
    fn wizard_ovpn_save_body_carries_the_blob() {
        let mut w = Wizard::new(None);
        w.provider = "generic-ovpn".into();
        w.id = "work".into();
        w.providers = vec![ProviderInfo {
            id: "generic-ovpn".into(),
            method: "ovpn".into(),
            ..Default::default()
        }];
        w.ovpn = "client\ndev tun\n".into();
        let body: serde_json::Value = serde_json::from_str(&w.setup_body()).unwrap();
        // The blob is trimmed before it goes on the wire.
        assert_eq!(body["ovpn"], "client\ndev tun");
        assert!(body.get("private_key").is_none());
    }

    #[test]
    fn wizard_save_emits_a_setup_body_only_from_review_when_complete() {
        let mut w = Wizard::new(None);
        w.provider = "mullvad".into();
        w.providers = vec![ProviderInfo {
            id: "mullvad".into(),
            method: "wg".into(),
            ..Default::default()
        }];
        w.id = "mullvad1".into();
        w.wg_config = "[Interface]\n".into();
        // Not on the review step yet → Save is a no-op.
        assert_eq!(w.update(WizardMsg::Save), WizardAction::None);
        w.step_idx = Step::Review.ordinal();
        let action = w.update(WizardMsg::Save);
        let WizardAction::Save(body) = action else {
            unreachable!("expected Save on a complete review step, got {action:?}");
        };
        assert!(body.contains("mullvad1"));
    }

    #[test]
    fn editing_locks_the_id_field() {
        let mut w = Wizard::new(Some(&VpnRow {
            id: "mullvad1".into(),
            provider: "mullvad".into(),
            ..Default::default()
        }));
        let _ = w.update(WizardMsg::IdInput("hacked".into()));
        assert_eq!(w.id, "mullvad1", "edit must not change the id");
    }

    #[test]
    fn setup_finished_ok_closes_the_wizard() {
        let mut panel = VpnPanel::new();
        let _ = panel.update(Message::OpenWizard { edit_id: None });
        assert!(panel.wizard.is_some());
        let _ = panel.update(Message::SetupFinished(Ok("Saved m1.".into())));
        assert!(panel.wizard.is_none());
        assert!(panel.status.contains("Saved"));
    }

    #[test]
    fn setup_finished_err_keeps_the_wizard_open_with_the_note() {
        let mut panel = VpnPanel::new();
        let _ = panel.update(Message::OpenWizard { edit_id: None });
        if let Some(w) = panel.wizard.as_mut() {
            w.saving = true;
        }
        let _ = panel.update(Message::SetupFinished(Err("bad json".into())));
        let w = panel.wizard.as_ref().expect("wizard stays open on error");
        assert!(!w.saving);
        assert_eq!(w.note, "bad json");
    }

    #[test]
    fn id_valid_requires_an_alphanumeric() {
        assert!(id_valid("mullvad1"));
        assert!(id_valid("a"));
        assert!(!id_valid("---"));
        assert!(!id_valid(""));
    }

    #[test]
    fn view_renders_all_states_without_panic() {
        let mut panel = VpnPanel::new();
        let _ = panel.view(); // default (daemon down → unreachable card)
        panel.daemon_up = true;
        let _ = panel.view(); // up, no tunnels → empty card
        panel.tunnels = parse_tunnels_reply(
            r#"{"ok":true,"tunnels":[
                {"id":"m1","provider":"mullvad","method":"wg","server":"us-nyc","protocol":"udp"}
            ]}"#,
        )
        .unwrap();
        panel.tunnels[0].up = true;
        panel.tunnels[0].exit_ip = Some("203.0.113.7".into());
        panel.tunnels[0].health = "ok".into();
        panel.status = "m1 up — wg-quick up".into();
        let _ = panel.view(); // a live tunnel card + exit line + status strip

        // The wizard surface renders every step without panic.
        let _ = panel.update(Message::OpenWizard { edit_id: None });
        if let Some(w) = panel.wizard.as_mut() {
            w.providers = vec![ProviderInfo {
                id: "mullvad".into(),
                method: "wg".into(),
                multi_instance: true,
                exit_check: Some("https://am.i.mullvad.net/json".into()),
            }];
            w.provider = "mullvad".into();
        }
        for step in [
            Step::Provider,
            Step::Config,
            Step::Server,
            Step::Name,
            Step::Review,
        ] {
            if let Some(w) = panel.wizard.as_mut() {
                w.step_idx = step.ordinal();
            }
            let _ = panel.view();
        }
    }
}
