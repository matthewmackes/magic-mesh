//! VPN-GW-7 — Network ▸ VPN panel: per-tunnel cards + the add-tunnel wizard.
//!
//! This replaces the v1.x nmcli connection lister with the real mesh VPN-Gateway
//! surface. Every datum comes off the live `action/vpn/*` Bus RPCs the `mackesd`
//! `vpn_gateway` worker serves (GW-1..GW-6) — never demo data:
//!
//! * **Cards** merge `list-tunnels` (the durable [`TunnelDef`]s — provider,
//!   server/region, protocol, kill-switch flag), `tunnel-health` (GW-6: the
//!   verified exit IP + the `healthy`/`leaking`/`down` verdict), and the bare
//!   interface up/down from `tunnel-status`. The kill-switch toggle re-issues
//!   `add-tunnel` with a mutated [`EgressPolicy`] (the daemon upserts by id —
//!   GW-3's `egress.kill_switch`); up/down issue `tunnel-up`/`tunnel-down`.
//! * The **add-tunnel wizard** is a step flow (provider → method → config/auth →
//!   server → name → verify → save) whose submit drives GW-5's
//!   `add-from-provider` (a named provider's params → a WG tunnel, secret sealed)
//!   or `import-config` (paste a `wg-quick` `.conf` / import an `.ovpn`, sealed).
//!
//! Throughput is NOT fabricated: `tunnel-status` exposes only `up: bool` and GW-6
//! publishes verdict + exit IP, so the cards show an honest "rx/tx not exposed by
//! the RPC" line rather than invented numbers (the gap is stated, not faked, §7).
//!
//! Carbon tokens only — every colour/space/size reads `mde_theme` via the shared
//! `panel_chrome` / `controls` widgets + `crate::live_theme::palette()` (§4).

use std::time::Duration;

use cosmic::iced::widget::{column, pick_list, row, scrollable, Space};
use cosmic::iced::{Length, Task};
use cosmic::Element;

use mackes_mesh_types::vpn::{EgressPolicy, Method, TunnelDef};
use mackes_mesh_types::vpn_provider::ProviderKind;
use mde_theme::{EmptyState, FontSize, Icon, TypeRole};

use crate::controls::{styled_text_input, toggle, variant_button, ButtonVariant};
use crate::cosmic_compat::prelude::*;
use crate::panel_chrome::{self, BadgeSeverity};

/// Bus RPC timeout for a VPN action/reply round-trip (matches the Connect /
/// Peers panels' interactive budget).
const RPC_TIMEOUT: Duration = Duration::from_secs(3);

/// The error shown when a VPN Bus RPC gets no reply (timeout / bus offline /
/// the `mackesd` VPN responder isn't running).
const RPC_FAIL: &str = "Bus RPC failed — mackesd VPN responder not answering";

/// One verified exit/health row for a tunnel, parsed from `tunnel-health`
/// (GW-6). The `verdict` is the stable `healthy`/`leaking`/`down` token; the
/// `exit_ip` is the worker's tunnel-bound IP echo (absent when down / unprobed).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TunnelHealthRow {
    pub verdict: String,
    pub exit_ip: Option<String>,
    pub live: bool,
}

/// A merged per-tunnel card: the durable def fields + the live health verdict +
/// the bare interface up/down. Every field is sourced from a real RPC reply.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TunnelCard {
    pub id: String,
    pub provider: String,
    /// Server / region selector (may be empty for a generic import).
    pub server: String,
    /// `wg` / `ovpn` (the method, surfaced as the protocol family).
    pub method: String,
    /// `udp` / `tcp` transport hint (may be empty).
    pub protocol: String,
    /// GW-3 selective-egress kill-switch flag (drives the toggle).
    pub kill_switch: bool,
    pub egress_enabled: bool,
    /// Log-safe handle to the sealed secret (`secret://vpn/<id>`). Preserved
    /// across the kill-switch upsert so the link to the sealed creds survives.
    pub creds_ref: String,
    /// GW-6 verified health, when the gateway worker has published any.
    pub health: Option<TunnelHealthRow>,
    /// Bare interface presence from `tunnel-status` (a tunnel can be iface-up but
    /// still `leaking` per the GW-6 verdict — both are shown honestly).
    pub iface_up: bool,
}

/// The add-tunnel wizard step. A linear flow; `Verify` and `Save` are the last
/// two so the operator confirms before the secret is sealed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardStep {
    Provider,
    Method,
    Config,
    Server,
    Name,
    Verify,
}

impl WizardStep {
    const ALL: [WizardStep; 6] = [
        WizardStep::Provider,
        WizardStep::Method,
        WizardStep::Config,
        WizardStep::Server,
        WizardStep::Name,
        WizardStep::Verify,
    ];

    fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    fn title(self) -> &'static str {
        match self {
            WizardStep::Provider => "Provider",
            WizardStep::Method => "Method",
            WizardStep::Config => "Config & auth",
            WizardStep::Server => "Server / region",
            WizardStep::Name => "Name",
            WizardStep::Verify => "Verify & save",
        }
    }

    fn next(self) -> Option<WizardStep> {
        let i = self.index();
        Self::ALL.get(i + 1).copied()
    }

    fn prev(self) -> Option<WizardStep> {
        let i = self.index();
        if i == 0 {
            None
        } else {
            Self::ALL.get(i - 1).copied()
        }
    }
}

/// How the new tunnel is created — maps to a GW-5 RPC at submit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardMethod {
    /// A named provider's params → a WG tunnel (`add-from-provider`).
    Provider,
    /// Paste a `wg-quick` `.conf` (`import-config kind=wg`).
    PasteWg,
    /// Import an OpenVPN `.ovpn` (`import-config kind=ovpn`).
    ImportOvpn,
}

impl WizardMethod {
    fn label(self) -> &'static str {
        match self {
            WizardMethod::Provider => "Provider API / portal peer",
            WizardMethod::PasteWg => "Paste a WireGuard config",
            WizardMethod::ImportOvpn => "Import an OpenVPN .ovpn",
        }
    }
}

/// The wizard's form state. Pure data the operator fills in; turned into a GW-5
/// request body at submit. The provider list is the 5 named adapters + a generic
/// fallback (for paste/import, where no provider API is consulted).
#[derive(Debug, Clone)]
pub struct Wizard {
    pub step: WizardStep,
    /// Selected provider label (one of [`PROVIDER_CHOICES`]).
    pub provider: String,
    pub method: WizardMethod,
    /// `add-from-provider` fields.
    pub account_token: String,
    pub server_pubkey: String,
    pub endpoint: String,
    pub assigned_address: String,
    pub dns: String,
    /// paste/import field — the raw config text (the SECRET; sealed by GW-5).
    pub config_text: String,
    /// Server / region selector.
    pub server: String,
    /// Tunnel id (the multi-instance name — `mvpn-<id>` derives from it).
    pub tunnel_id: String,
    /// Submit feedback from the last `add-from-provider` / `import-config`.
    pub submit_result: Option<Result<String, String>>,
}

impl Default for Wizard {
    fn default() -> Self {
        Self {
            step: WizardStep::Provider,
            provider: ProviderKind::Mullvad.label().to_string(),
            method: WizardMethod::Provider,
            account_token: String::new(),
            server_pubkey: String::new(),
            endpoint: String::new(),
            assigned_address: String::new(),
            dns: String::new(),
            config_text: String::new(),
            server: String::new(),
            tunnel_id: String::new(),
            submit_result: None,
        }
    }
}

/// The provider choices the wizard offers: the 5 named GW-5 adapters + a generic
/// label (for paste/import flows, where no provider API is consulted).
pub const PROVIDER_CHOICES: [&str; 6] = [
    "mullvad",
    "protonvpn",
    "ivpn",
    "nordvpn",
    "surfshark",
    "generic",
];

#[derive(Debug, Clone, Default)]
pub struct VpnPanel {
    pub tunnels: Vec<TunnelCard>,
    /// Set when the Bus RPC to `action/vpn/list-tunnels` failed (timeout / bus
    /// offline) — the view shows the error state, never a misleading empty one.
    pub load_error: Option<String>,
    pub busy: bool,
    /// Transient status line (last action result).
    pub status: String,
    /// The local node id (`hostname`) — the gateway a wizard-created secret is
    /// distributed to (GW-2/GW-5's `node_id`). Resolved once at construction.
    pub node_id: String,
    /// The add-tunnel wizard, when open.
    pub wizard: Option<Wizard>,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// `list-tunnels` (+ health + status) merged into cards, or a Bus error.
    Loaded(Result<Vec<TunnelCard>, String>),
    RefreshClicked,
    /// Per-card: bring the tunnel up / down (`tunnel-up` / `tunnel-down`).
    SetUp {
        id: String,
        up: bool,
    },
    /// Per-card: flip the GW-3 kill-switch (re-issues `add-tunnel` with a mutated
    /// `EgressPolicy`).
    ToggleKillSwitch {
        id: String,
        on: bool,
    },
    /// Per-card: remove the tunnel (`remove-tunnel` — rotates its secret).
    RemoveTunnel(String),
    /// A per-card action's RPC finished; carries a status message to show, then
    /// reloads the roster.
    ActionFinished(Result<String, String>),

    // ── wizard ──
    OpenWizard,
    CloseWizard,
    WizardNext,
    WizardBack,
    WizardProvider(String),
    WizardMethod(WizardMethod),
    WizardField {
        field: WizardField,
        value: String,
    },
    WizardSubmit,
    WizardSubmitted(Result<String, String>),
}

/// The wizard text fields (so one `WizardField` arm folds every input).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardField {
    AccountToken,
    ServerPubkey,
    Endpoint,
    AssignedAddress,
    Dns,
    ConfigText,
    Server,
    TunnelId,
}

impl VpnPanel {
    #[must_use]
    pub fn new() -> Self {
        Self {
            node_id: detect_node_id(),
            ..Self::default()
        }
    }

    /// One-shot fetch on nav: `list-tunnels` + `tunnel-health` + `tunnel-status`,
    /// merged into per-tunnel cards. A Bus failure is a load ERROR (not an empty
    /// roster); an empty list is the honest empty state.
    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async {
                tokio::task::spawn_blocking(fetch_cards)
                    .await
                    .map_err(|e| format!("spawn error: {e}"))
                    .and_then(|r| r)
            },
            |result| crate::Message::Vpn(Message::Loaded(result)),
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded(Ok(cards)) => {
                self.tunnels = cards;
                self.load_error = None;
                self.busy = false;
                Task::none()
            }
            Message::Loaded(Err(e)) => {
                self.load_error = Some(e);
                self.busy = false;
                Task::none()
            }
            Message::RefreshClicked => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status.clear();
                Self::load()
            }
            Message::SetUp { id, up } => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = format!("{} {id}…", if up { "Bringing up" } else { "Tearing down" });
                let verb = if up { "tunnel-up" } else { "tunnel-down" };
                run_action(verb.to_string(), Some(id))
            }
            Message::ToggleKillSwitch { id, on } => {
                if self.busy {
                    return Task::none();
                }
                let Some(card) = self.tunnels.iter().find(|c| c.id == id).cloned() else {
                    return Task::none();
                };
                self.busy = true;
                self.status = format!(
                    "{} kill-switch on {id}…",
                    if on { "Enabling" } else { "Disabling" }
                );
                // GW-3: re-issue `add-tunnel` with the mutated EgressPolicy (the
                // daemon upserts by id, so this is an in-place policy edit).
                let def = card.to_tunnel_def_with_killswitch(on);
                let body = serde_json::to_string(&def).unwrap_or_default();
                run_action_with_body("add-tunnel".to_string(), body)
            }
            Message::RemoveTunnel(id) => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = format!("Removing {id}…");
                run_action("remove-tunnel".to_string(), Some(id))
            }
            Message::ActionFinished(result) => {
                self.busy = false;
                self.status = match &result {
                    Ok(msg) => msg.clone(),
                    Err(msg) => msg.clone(),
                };
                Self::load()
            }

            // ── wizard ──
            Message::OpenWizard => {
                self.wizard = Some(Wizard::default());
                Task::none()
            }
            Message::CloseWizard => {
                self.wizard = None;
                Task::none()
            }
            Message::WizardNext => {
                if let Some(w) = &mut self.wizard {
                    if w.can_advance() {
                        if let Some(next) = w.step.next() {
                            w.step = next;
                        }
                    }
                }
                Task::none()
            }
            Message::WizardBack => {
                if let Some(w) = &mut self.wizard {
                    if let Some(prev) = w.step.prev() {
                        w.step = prev;
                    }
                }
                Task::none()
            }
            Message::WizardProvider(p) => {
                if let Some(w) = &mut self.wizard {
                    w.provider = p;
                }
                Task::none()
            }
            Message::WizardMethod(m) => {
                if let Some(w) = &mut self.wizard {
                    w.method = m;
                }
                Task::none()
            }
            Message::WizardField { field, value } => {
                if let Some(w) = &mut self.wizard {
                    w.set_field(field, value);
                }
                Task::none()
            }
            Message::WizardSubmit => {
                if self.busy {
                    return Task::none();
                }
                let Some(req) = self
                    .wizard
                    .as_ref()
                    .map(|w| w.submit_request(&self.node_id))
                else {
                    return Task::none();
                };
                let (verb, body) = req;
                self.busy = true;
                run_wizard_submit(verb, body)
            }
            Message::WizardSubmitted(result) => {
                self.busy = false;
                if let Some(w) = &mut self.wizard {
                    w.submit_result = Some(result.clone());
                }
                if result.is_ok() {
                    // The tunnel + sealed secret landed — close the wizard and
                    // reload so the new card shows from the real `list-tunnels`.
                    self.wizard = None;
                    self.status = "Tunnel created — secret sealed.".to_string();
                    return Self::load();
                }
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let density = crate::live_theme::tokens().density;

        // The wizard takes over the panel body when open.
        if let Some(w) = &self.wizard {
            return panel_chrome::panel_container(wizard_view(w, self.busy, palette), density);
        }

        if let Some(err) = &self.load_error {
            return panel_chrome::panel_container(
                panel_chrome::error_state(err.clone(), palette, || {
                    crate::Message::Vpn(Message::RefreshClicked)
                }),
                density,
            );
        }

        let sizes = FontSize::defaults();
        let title = cosmic::iced::widget::text("VPN tunnels")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());
        let subtitle = cosmic::iced::widget::text("mesh internet-egress gateways")
            .size(TypeRole::Body.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color());

        let add_btn = variant_button(
            "Add tunnel",
            ButtonVariant::Primary,
            Some(crate::Message::Vpn(Message::OpenWizard)),
            palette,
        );
        let refresh_btn = variant_button(
            if self.busy { "…" } else { "Refresh" },
            ButtonVariant::Ghost,
            (!self.busy).then(|| crate::Message::Vpn(Message::RefreshClicked)),
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

        let body: Element<'_, crate::Message> = if self.tunnels.is_empty() {
            panel_chrome::empty_state(
                EmptyState::with_cta(
                    "No VPN tunnels yet",
                    "A tunnel is one internet-egress gateway on top of the mesh — a \
                     WireGuard or OpenVPN link to a provider. Click \"Add tunnel\" to set \
                     one up from a provider's portal, a pasted WireGuard config, or an \
                     imported .ovpn. The key material is sealed under the mesh key — never \
                     stored in the clear.",
                    "Add tunnel",
                )
                .with_icon(Icon::Vpn),
                palette,
                || crate::Message::Vpn(Message::OpenWizard),
            )
        } else {
            let mut col = column![].spacing(12);
            for card in &self.tunnels {
                col = col.push(tunnel_card_view(card, self.busy, palette, density));
            }
            scrollable(col).height(Length::Fill).into()
        };

        let mut root = column![header, Space::new().height(Length::Fixed(16.0)), body].spacing(2);
        if !self.status.is_empty() {
            root = root.push(
                cosmic::iced::widget::text(&self.status)
                    .size(TypeRole::Caption.size_in(sizes))
                    .colr(palette.text_muted.into_cosmic_color()),
            );
        }

        panel_chrome::panel_container(root.into(), density)
    }
}

impl TunnelCard {
    /// Rebuild a [`TunnelDef`] from this card's fields with the kill-switch flag
    /// flipped — the body for the GW-3 in-place `add-tunnel` upsert. The durable,
    /// panel-visible fields are reconstructed and the sealed-secret link
    /// (`creds_ref`) is carried through so the upsert doesn't orphan the `.age`
    /// blob (`add-tunnel` replaces the whole def by id).
    #[must_use]
    pub fn to_tunnel_def_with_killswitch(&self, kill_switch: bool) -> TunnelDef {
        TunnelDef {
            id: self.id.clone(),
            provider: self.provider.clone(),
            method: parse_method(&self.method),
            server: self.server.clone(),
            protocol: self.protocol.clone(),
            // Preserve the sealed-secret link across the upsert — `add-tunnel`
            // replaces the whole def, so dropping creds_ref would orphan the
            // sealed `.age` blob.
            creds_ref: self.creds_ref.clone(),
            egress: EgressPolicy {
                // Toggling the kill-switch implies the operator wants egress
                // policy active for this tunnel (a kill-switch on a tunnel that
                // routes nothing is a no-op) — enable egress when arming it.
                enabled: self.egress_enabled || kill_switch,
                kill_switch,
                mark: None,
            },
        }
    }
}

impl Wizard {
    /// Can the wizard advance from the current step (the step's required fields
    /// are filled)? Drives the disabled state of the "Next" button.
    #[must_use]
    pub fn can_advance(&self) -> bool {
        match self.step {
            WizardStep::Provider | WizardStep::Method => true,
            WizardStep::Config => match self.method {
                // A provider with a statically-known portal peer needs the peer
                // pubkey + endpoint + assigned address (the GW-5 adapter rejects
                // an empty peer rather than mint a dead tunnel).
                WizardMethod::Provider => {
                    !self.server_pubkey.trim().is_empty()
                        && !self.endpoint.trim().is_empty()
                        && !self.assigned_address.trim().is_empty()
                }
                WizardMethod::PasteWg | WizardMethod::ImportOvpn => {
                    !self.config_text.trim().is_empty()
                }
            },
            WizardStep::Server => true, // server/region is optional for generic imports
            WizardStep::Name => is_valid_id(&self.tunnel_id),
            WizardStep::Verify => true,
        }
    }

    fn set_field(&mut self, field: WizardField, value: String) {
        match field {
            WizardField::AccountToken => self.account_token = value,
            WizardField::ServerPubkey => self.server_pubkey = value,
            WizardField::Endpoint => self.endpoint = value,
            WizardField::AssignedAddress => self.assigned_address = value,
            WizardField::Dns => self.dns = value,
            WizardField::ConfigText => self.config_text = value,
            WizardField::Server => self.server = value,
            WizardField::TunnelId => self.tunnel_id = value,
        }
    }

    /// Build the GW-5 submit `(verb, body)` for the current form. Provider →
    /// `add-from-provider`; paste/import → `import-config`. The secret material
    /// (token / config) rides in the body and is sealed daemon-side.
    #[must_use]
    pub fn submit_request(&self, node_id: &str) -> (String, String) {
        match self.method {
            WizardMethod::Provider => {
                let body = serde_json::json!({
                    "provider": self.provider,
                    "tunnel_id": self.tunnel_id.trim(),
                    "node_id": node_id,
                    "server": self.server.trim(),
                    "account_token": self.account_token,
                    "server_pubkey": self.server_pubkey.trim(),
                    "endpoint": self.endpoint.trim(),
                    "assigned_address": self.assigned_address.trim(),
                    "dns": option_str(&self.dns),
                })
                .to_string();
                ("add-from-provider".to_string(), body)
            }
            WizardMethod::PasteWg | WizardMethod::ImportOvpn => {
                let kind = if matches!(self.method, WizardMethod::PasteWg) {
                    "wg"
                } else {
                    "ovpn"
                };
                let body = serde_json::json!({
                    "kind": kind,
                    "tunnel_id": self.tunnel_id.trim(),
                    "node_id": node_id,
                    "config": self.config_text,
                })
                .to_string();
                ("import-config".to_string(), body)
            }
        }
    }
}

// ── card view ───────────────────────────────────────────────────────────────

fn tunnel_card_view<'a>(
    card: &'a TunnelCard,
    busy: bool,
    palette: mde_theme::Palette,
    density: mde_theme::Density,
) -> Element<'a, crate::Message> {
    use cosmic::iced::widget::text;
    let sizes = FontSize::defaults();

    let id_label = text(&card.id)
        .size(TypeRole::Subheading.size_in(sizes))
        .colr(palette.text.into_cosmic_color());

    // Status badge from the GW-6 verdict, falling back to the bare interface
    // check when no worker has published health yet.
    let (badge_label, badge_sev) = status_for(card);
    let badge = panel_chrome::status_badge(badge_label, badge_sev, palette);

    let header = row![
        id_label,
        Space::new().width(Length::Fixed(10.0)),
        badge,
        Space::new().width(Length::Fill),
        text(card.provider.clone())
            .size(TypeRole::Caption.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color()),
    ]
    .align_y(cosmic::iced::alignment::Vertical::Center);

    // Real RPC fields. Empty values render as an honest "—"/"unknown".
    let meta = |label: &'static str, value: String| {
        row![
            text(label)
                .size(TypeRole::Caption.size_in(sizes))
                .width(Length::Fixed(96.0))
                .colr(palette.text_muted.into_cosmic_color()),
            text(if value.is_empty() {
                "—".to_string()
            } else {
                value
            })
            .size(TypeRole::Caption.size_in(sizes))
            .colr(palette.text.into_cosmic_color()),
        ]
        .spacing(8)
    };

    let exit_ip = card
        .health
        .as_ref()
        .and_then(|h| h.exit_ip.clone())
        .unwrap_or_default();

    let mut meta_col = column![
        meta("Server", card.server.clone()),
        meta(
            "Protocol",
            format!(
                "{}{}",
                card.method.to_uppercase(),
                if card.protocol.is_empty() {
                    String::new()
                } else {
                    format!(" / {}", card.protocol)
                }
            )
        ),
        meta("Exit IP", exit_ip),
    ]
    .spacing(4);

    // §7 honesty — `tunnel-status` exposes only up/down and GW-6 publishes
    // verdict + exit IP; rx/tx throughput is NOT carried by any current RPC, so
    // state the gap rather than fabricate numbers.
    meta_col = meta_col.push(
        text("Throughput — not exposed by tunnel-status RPC")
            .size(TypeRole::Caption.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color()),
    );

    // Kill-switch toggle (GW-3) + actions.
    let ks_id = card.id.clone();
    let ks_toggle = toggle(
        card.kill_switch,
        move |on| {
            crate::Message::Vpn(Message::ToggleKillSwitch {
                id: ks_id.clone(),
                on,
            })
        },
        palette,
    );
    let ks_row = row![
        text("Kill-switch")
            .size(TypeRole::Caption.size_in(sizes))
            .colr(palette.text.into_cosmic_color()),
        Space::new().width(Length::Fixed(10.0)),
        ks_toggle,
    ]
    .align_y(cosmic::iced::alignment::Vertical::Center);

    let up_btn = variant_button(
        if card.iface_up {
            "Bring down"
        } else {
            "Bring up"
        },
        ButtonVariant::Secondary,
        (!busy).then(|| {
            crate::Message::Vpn(Message::SetUp {
                id: card.id.clone(),
                up: !card.iface_up,
            })
        }),
        palette,
    );
    let remove_btn = variant_button(
        "Remove",
        ButtonVariant::Ghost,
        (!busy).then(|| crate::Message::Vpn(Message::RemoveTunnel(card.id.clone()))),
        palette,
    );

    let actions = row![
        ks_row,
        Space::new().width(Length::Fill),
        up_btn,
        Space::new().width(Length::Fixed(8.0)),
        remove_btn,
    ]
    .align_y(cosmic::iced::alignment::Vertical::Center);

    let body = column![
        header,
        Space::new().height(Length::Fixed(8.0)),
        meta_col,
        Space::new().height(Length::Fixed(10.0)),
        actions,
    ]
    .spacing(2);

    panel_chrome::card(body.into(), palette, density)
}

/// Derive the card's status badge from the GW-6 verdict (preferred) or the bare
/// interface check (fallback when no worker has published health).
fn status_for(card: &TunnelCard) -> (String, BadgeSeverity) {
    if let Some(h) = &card.health {
        return match h.verdict.as_str() {
            "healthy" => ("healthy".to_string(), BadgeSeverity::Success),
            "leaking" => ("leaking".to_string(), BadgeSeverity::Danger),
            "down" => ("down".to_string(), BadgeSeverity::Neutral),
            other => (other.to_string(), BadgeSeverity::Neutral),
        };
    }
    if card.iface_up {
        ("up".to_string(), BadgeSeverity::Info)
    } else {
        ("down".to_string(), BadgeSeverity::Neutral)
    }
}

// ── wizard view ───────────────────────────────────────────────────────────────

fn wizard_view<'a>(
    w: &'a Wizard,
    busy: bool,
    palette: mde_theme::Palette,
) -> Element<'a, crate::Message> {
    use cosmic::iced::widget::text;
    let sizes = FontSize::defaults();

    // Stepper breadcrumb — current step accented, the rest muted.
    let mut steps = row![].spacing(8);
    for (i, s) in WizardStep::ALL.iter().enumerate() {
        let current = *s == w.step;
        let color = if current {
            palette.accent
        } else {
            palette.text_muted
        };
        if i > 0 {
            steps = steps.push(
                text("›")
                    .size(TypeRole::Caption.size_in(sizes))
                    .colr(palette.text_muted.into_cosmic_color()),
            );
        }
        steps = steps.push(
            text(format!("{}. {}", i + 1, s.title()))
                .size(TypeRole::Caption.size_in(sizes))
                .colr(color.into_cosmic_color()),
        );
    }

    let title = text(w.step.title())
        .size(TypeRole::Display.size_in(sizes))
        .colr(palette.text.into_cosmic_color());

    let step_body = match w.step {
        WizardStep::Provider => provider_step(w, palette),
        WizardStep::Method => method_step(w, palette),
        WizardStep::Config => config_step(w, palette),
        WizardStep::Server => server_step(w, palette),
        WizardStep::Name => name_step(w, palette),
        WizardStep::Verify => verify_step(w, palette),
    };

    // Footer: Back / Cancel + Next-or-Save.
    let back_btn = variant_button(
        "Back",
        ButtonVariant::Ghost,
        w.step
            .prev()
            .map(|_| crate::Message::Vpn(Message::WizardBack)),
        palette,
    );
    let cancel_btn = variant_button(
        "Cancel",
        ButtonVariant::Ghost,
        Some(crate::Message::Vpn(Message::CloseWizard)),
        palette,
    );
    let advance_btn = if matches!(w.step, WizardStep::Verify) {
        variant_button(
            if busy { "Saving…" } else { "Save tunnel" },
            ButtonVariant::Primary,
            (!busy && w.can_advance()).then(|| crate::Message::Vpn(Message::WizardSubmit)),
            palette,
        )
    } else {
        variant_button(
            "Next",
            ButtonVariant::Primary,
            w.can_advance()
                .then(|| crate::Message::Vpn(Message::WizardNext)),
            palette,
        )
    };

    let footer = row![
        cancel_btn,
        Space::new().width(Length::Fixed(8.0)),
        back_btn,
        Space::new().width(Length::Fill),
        advance_btn,
    ]
    .align_y(cosmic::iced::alignment::Vertical::Center);

    let mut root = column![
        steps,
        Space::new().height(Length::Fixed(8.0)),
        title,
        Space::new().height(Length::Fixed(12.0)),
        scrollable(step_body).height(Length::Fill),
        Space::new().height(Length::Fixed(12.0)),
        footer,
    ]
    .spacing(2);

    if let Some(res) = &w.submit_result {
        if let Err(e) = res {
            root = root.push(
                text(format!("Save failed — {e}"))
                    .size(TypeRole::Caption.size_in(sizes))
                    .colr(palette.danger.into_cosmic_color()),
            );
        }
    }

    root.into()
}

fn provider_step<'a>(w: &'a Wizard, palette: mde_theme::Palette) -> Element<'a, crate::Message> {
    use cosmic::iced::widget::text;
    let sizes = FontSize::defaults();
    let choices: Vec<String> = PROVIDER_CHOICES.iter().map(|s| (*s).to_string()).collect();
    let dropdown = pick_list(choices, Some(w.provider.clone()), |p| {
        crate::Message::Vpn(Message::WizardProvider(p))
    });
    column![
        text(
            "Which provider hosts this tunnel? The five named providers can mint a \
              WireGuard config from their portal peer; pick \"generic\" for a pasted \
              config or an imported .ovpn."
        )
        .size(TypeRole::Body.size_in(sizes))
        .colr(palette.text_muted.into_cosmic_color()),
        Space::new().height(Length::Fixed(12.0)),
        dropdown,
    ]
    .spacing(4)
    .into()
}

fn method_step<'a>(w: &'a Wizard, palette: mde_theme::Palette) -> Element<'a, crate::Message> {
    use cosmic::iced::widget::text;
    let sizes = FontSize::defaults();
    let mut col = column![text(
        "How is the tunnel configured? A provider peer mints a WireGuard tunnel from \
         the portal details; or paste a wg-quick config / import an OpenVPN .ovpn."
    )
    .size(TypeRole::Body.size_in(sizes))
    .colr(palette.text_muted.into_cosmic_color())]
    .spacing(8);
    for m in [
        WizardMethod::Provider,
        WizardMethod::PasteWg,
        WizardMethod::ImportOvpn,
    ] {
        let selected = w.method == m;
        let variant = if selected {
            ButtonVariant::Primary
        } else {
            ButtonVariant::Secondary
        };
        col = col.push(variant_button(
            m.label(),
            variant,
            Some(crate::Message::Vpn(Message::WizardMethod(m))),
            palette,
        ));
    }
    col.into()
}

fn config_step<'a>(w: &'a Wizard, palette: mde_theme::Palette) -> Element<'a, crate::Message> {
    match w.method {
        WizardMethod::Provider => {
            let field = |label: &'a str, placeholder: &'a str, value: &'a str, f: WizardField| {
                labeled_input(label, placeholder, value, f, palette)
            };
            column![
                field(
                    "Server public key",
                    "the provider portal's [Peer] PublicKey",
                    &w.server_pubkey,
                    WizardField::ServerPubkey,
                ),
                field(
                    "Endpoint",
                    "host:port (e.g. 198.51.100.10:51820)",
                    &w.endpoint,
                    WizardField::Endpoint,
                ),
                field(
                    "Assigned address",
                    "the address the portal assigned (e.g. 10.64.0.2/32)",
                    &w.assigned_address,
                    WizardField::AssignedAddress,
                ),
                field(
                    "Account token (sealed)",
                    "provider account/session token — sealed, never logged",
                    &w.account_token,
                    WizardField::AccountToken,
                ),
                field(
                    "DNS (optional)",
                    "leave blank for the provider default",
                    &w.dns,
                    WizardField::Dns,
                ),
            ]
            .spacing(10)
            .into()
        }
        WizardMethod::PasteWg | WizardMethod::ImportOvpn => {
            use cosmic::iced::widget::text;
            let sizes = FontSize::defaults();
            let label = if matches!(w.method, WizardMethod::PasteWg) {
                "Paste the full wg-quick config ([Interface] + [Peer]). It carries the \
                 private key — it is sealed under the mesh key, never stored in the clear."
            } else {
                "Paste the full OpenVPN .ovpn (inline certs/keys). It is sealed under the \
                 mesh key, never stored in the clear."
            };
            column![
                text(label)
                    .size(TypeRole::Body.size_in(sizes))
                    .colr(palette.text_muted.into_cosmic_color()),
                Space::new().height(Length::Fixed(8.0)),
                styled_text_input(
                    "paste config text here",
                    &w.config_text,
                    move |s| crate::Message::Vpn(Message::WizardField {
                        field: WizardField::ConfigText,
                        value: s,
                    }),
                    palette,
                ),
            ]
            .spacing(4)
            .into()
        }
    }
}

fn server_step<'a>(w: &'a Wizard, palette: mde_theme::Palette) -> Element<'a, crate::Message> {
    use cosmic::iced::widget::text;
    let sizes = FontSize::defaults();
    column![
        text("Server / region selector (provider-specific; optional for a generic import).")
            .size(TypeRole::Body.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color()),
        Space::new().height(Length::Fixed(8.0)),
        labeled_input(
            "Server / region",
            "e.g. us-nyc-wg-001",
            &w.server,
            WizardField::Server,
            palette,
        ),
    ]
    .spacing(4)
    .into()
}

fn name_step<'a>(w: &'a Wizard, palette: mde_theme::Palette) -> Element<'a, crate::Message> {
    use cosmic::iced::widget::text;
    let sizes = FontSize::defaults();
    let valid = is_valid_id(&w.tunnel_id);
    let hint = if w.tunnel_id.is_empty() {
        "Pick a unique id for this tunnel. The interface name mvpn-<id> derives from it."
            .to_string()
    } else if valid {
        format!("Interface: mvpn-{}", sanitize_ifname_body(&w.tunnel_id))
    } else {
        "The id needs at least one letter or digit (it forms the interface name).".to_string()
    };
    let hint_color = if valid || w.tunnel_id.is_empty() {
        palette.text_muted
    } else {
        palette.danger
    };
    column![
        text(
            "Name this tunnel instance — run several (e.g. mullvad-us, mullvad-uk) by \
              giving each a distinct id."
        )
        .size(TypeRole::Body.size_in(sizes))
        .colr(palette.text_muted.into_cosmic_color()),
        Space::new().height(Length::Fixed(8.0)),
        labeled_input(
            "Tunnel id",
            "e.g. mullvad-us",
            &w.tunnel_id,
            WizardField::TunnelId,
            palette,
        ),
        text(hint)
            .size(TypeRole::Caption.size_in(sizes))
            .colr(hint_color.into_cosmic_color()),
    ]
    .spacing(4)
    .into()
}

fn verify_step<'a>(w: &'a Wizard, palette: mde_theme::Palette) -> Element<'a, crate::Message> {
    use cosmic::iced::widget::text;
    let sizes = FontSize::defaults();
    let line = |label: &'static str, value: String| {
        row![
            text(label)
                .size(TypeRole::Caption.size_in(sizes))
                .width(Length::Fixed(140.0))
                .colr(palette.text_muted.into_cosmic_color()),
            text(if value.is_empty() {
                "—".to_string()
            } else {
                value
            })
            .size(TypeRole::Caption.size_in(sizes))
            .colr(palette.text.into_cosmic_color()),
        ]
        .spacing(8)
    };
    let method_label = match w.method {
        WizardMethod::Provider => "Provider peer",
        WizardMethod::PasteWg => "Pasted WireGuard config",
        WizardMethod::ImportOvpn => "Imported .ovpn",
    };
    let mut col = column![
        text(
            "Review, then save. The tunnel definition is written and the secret material \
             is sealed under the mesh key on save — live exit-IP verification (GW-6) runs \
             on the gateway once the tunnel is brought up."
        )
        .size(TypeRole::Body.size_in(sizes))
        .colr(palette.text_muted.into_cosmic_color()),
        Space::new().height(Length::Fixed(10.0)),
        line("Tunnel id", w.tunnel_id.trim().to_string()),
        line("Provider", w.provider.clone()),
        line("Method", method_label.to_string()),
        line("Server / region", w.server.trim().to_string()),
    ]
    .spacing(4);
    if matches!(w.method, WizardMethod::Provider) {
        col = col
            .push(line("Endpoint", w.endpoint.trim().to_string()))
            .push(line(
                "Assigned address",
                w.assigned_address.trim().to_string(),
            ))
            // The token is a SECRET — confirm presence, never echo it.
            .push(line(
                "Account token",
                if w.account_token.trim().is_empty() {
                    "(none)".to_string()
                } else {
                    "(set — sealed)".to_string()
                },
            ));
    } else {
        col = col.push(line(
            "Config",
            if w.config_text.trim().is_empty() {
                "(empty)".to_string()
            } else {
                format!("({} chars — sealed)", w.config_text.len())
            },
        ));
    }
    col.into()
}

/// A labelled single-line text field (label above the input).
fn labeled_input<'a>(
    label: &'a str,
    placeholder: &'a str,
    value: &'a str,
    field: WizardField,
    palette: mde_theme::Palette,
) -> Element<'a, crate::Message> {
    use cosmic::iced::widget::text;
    let sizes = FontSize::defaults();
    column![
        text(label)
            .size(TypeRole::Caption.size_in(sizes))
            .colr(palette.text.into_cosmic_color()),
        styled_text_input(
            placeholder,
            value,
            move |s| crate::Message::Vpn(Message::WizardField { field, value: s }),
            palette,
        ),
    ]
    .spacing(4)
    .into()
}

// ── I/O + pure helpers ────────────────────────────────────────────────────────

/// Issue a `tunnel-up`/`tunnel-down`/`remove-tunnel` action carrying the tunnel
/// id as the bare request body, then reload via [`Message::ActionFinished`].
fn run_action(verb: String, id: Option<String>) -> Task<crate::Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || {
                let topic = format!("action/vpn/{verb}");
                crate::dbus::action_request_with_body(&topic, id.as_deref(), RPC_TIMEOUT)
                    .map(|raw| summarize_reply(&verb, &raw))
                    .ok_or_else(|| RPC_FAIL.to_string())
            })
            .await
            .map_err(|e| format!("spawn error: {e}"))
            .and_then(|r| r)
        },
        |result| crate::Message::Vpn(Message::ActionFinished(result)),
    )
}

/// Issue an action carrying a JSON `body` (e.g. the GW-3 kill-switch upsert).
fn run_action_with_body(verb: String, body: String) -> Task<crate::Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || {
                let topic = format!("action/vpn/{verb}");
                crate::dbus::action_request_with_body(&topic, Some(&body), RPC_TIMEOUT)
                    .map(|raw| summarize_reply(&verb, &raw))
                    .ok_or_else(|| RPC_FAIL.to_string())
            })
            .await
            .map_err(|e| format!("spawn error: {e}"))
            .and_then(|r| r)
        },
        |result| crate::Message::Vpn(Message::ActionFinished(result)),
    )
}

/// Issue the GW-5 wizard submit (`add-from-provider`/`import-config`), reporting
/// the reply via [`Message::WizardSubmitted`].
fn run_wizard_submit(verb: String, body: String) -> Task<crate::Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || {
                let topic = format!("action/vpn/{verb}");
                crate::dbus::action_request_with_body(&topic, Some(&body), RPC_TIMEOUT)
                    .ok_or_else(|| RPC_FAIL.to_string())
                    .and_then(|raw| reply_ok_or_err(&raw))
            })
            .await
            .map_err(|e| format!("spawn error: {e}"))
            .and_then(|r| r)
        },
        |result| crate::Message::Vpn(Message::WizardSubmitted(result)),
    )
}

/// Blocking fetch: `list-tunnels` (+ `tunnel-health` + per-tunnel `tunnel-status`)
/// merged into cards. Runs on a blocking pool (the Bus client is sync). A failed
/// `list-tunnels` is an error; health/status are best-effort enrichments.
fn fetch_cards() -> Result<Vec<TunnelCard>, String> {
    let list_raw = crate::dbus::action_request("action/vpn/list-tunnels", RPC_TIMEOUT)
        .ok_or_else(|| RPC_FAIL.to_string())?;
    let mut cards = parse_tunnels(&list_raw);

    // GW-6 verified health (best-effort — empty when no worker has published).
    let health = crate::dbus::action_request("action/vpn/tunnel-health", RPC_TIMEOUT)
        .map(|raw| parse_health(&raw))
        .unwrap_or_default();

    for card in &mut cards {
        if let Some(h) = health.iter().find(|(id, _)| *id == card.id) {
            card.health = Some(h.1.clone());
        }
        // Bare interface up/down (GW-1's `tunnel-status` — the responder takes
        // the tunnel id as the bare request body).
        if let Some(raw) = crate::dbus::action_request_with_body(
            "action/vpn/tunnel-status",
            Some(&card.id),
            RPC_TIMEOUT,
        ) {
            card.iface_up = parse_iface_up(&raw);
        }
    }
    Ok(cards)
}

/// Parse the `list-tunnels` reply (`{ "ok": true, "tunnels": [TunnelDef…] }`)
/// into cards. A bad/non-ok reply degrades to an empty roster (honest empty
/// state) — never a panic.
#[must_use]
pub fn parse_tunnels(raw: &str) -> Vec<TunnelCard> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw.trim()) else {
        return Vec::new();
    };
    let Some(arr) = v.get("tunnels").and_then(|t| t.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|t| {
            let id = t.get("id")?.as_str()?.to_string();
            let egress = t.get("egress");
            Some(TunnelCard {
                id,
                provider: str_field(t, "provider"),
                server: str_field(t, "server"),
                method: str_field(t, "method"),
                protocol: str_field(t, "protocol"),
                kill_switch: egress
                    .and_then(|e| e.get("kill_switch"))
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                egress_enabled: egress
                    .and_then(|e| e.get("enabled"))
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                creds_ref: str_field(t, "creds_ref"),
                health: None,
                iface_up: false,
            })
        })
        .collect()
}

/// Parse the `tunnel-health` reply (`{ "tunnels": [{tunnel, verdict, exit_ip,
/// live, …}] }`) into `(tunnel_id, health)` pairs.
#[must_use]
pub fn parse_health(raw: &str) -> Vec<(String, TunnelHealthRow)> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw.trim()) else {
        return Vec::new();
    };
    let Some(arr) = v.get("tunnels").and_then(|t| t.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|h| {
            let id = h.get("tunnel")?.as_str()?.to_string();
            Some((
                id,
                TunnelHealthRow {
                    verdict: str_field(h, "verdict"),
                    exit_ip: h
                        .get("exit_ip")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    live: h
                        .get("live")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                },
            ))
        })
        .collect()
}

/// Parse the `tunnel-status` reply (`{ "ok": true, "ifname": .., "up": bool }`).
#[must_use]
pub fn parse_iface_up(raw: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(raw.trim())
        .ok()
        .and_then(|v| v.get("up").and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}

/// Summarize an action reply into a human status line (`ok`/`error`/`detail`).
#[must_use]
pub fn summarize_reply(verb: &str, raw: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw.trim()) else {
        return format!("{verb}: (unparseable reply)");
    };
    if let Some(e) = v.get("error").and_then(serde_json::Value::as_str) {
        return e.to_string();
    }
    if let Some(detail) = v.get("detail").and_then(serde_json::Value::as_str) {
        return format!("{verb}: {detail}");
    }
    if v.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        return format!("{verb}: ok");
    }
    format!("{verb}: {raw}")
}

/// Map a GW-5 reply to `Ok(summary)` / `Err(reason)` for the wizard.
#[must_use]
pub fn reply_ok_or_err(raw: &str) -> Result<String, String> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw.trim()) else {
        return Err(format!("unparseable reply: {raw}"));
    };
    if let Some(e) = v.get("error").and_then(serde_json::Value::as_str) {
        return Err(e.to_string());
    }
    if v.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        return Ok("created".to_string());
    }
    Err(format!("unexpected reply: {raw}"))
}

fn str_field(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The local node id — the gateway a wizard-created secret is distributed to.
fn detect_node_id() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

/// `Method` token (`wg`/`ovpn`/…) → the typed [`Method`].
fn parse_method(s: &str) -> Method {
    match s.to_ascii_lowercase().as_str() {
        "ovpn" | "openvpn" => Method::Ovpn,
        "cli" => Method::Cli,
        "api" => Method::Api,
        _ => Method::Wg,
    }
}

/// Mirror of [`TunnelDef::ifname`]'s body derivation, for the live name-step hint
/// (the same sanitize: alphanumerics only, bounded to the 10-char body).
fn sanitize_ifname_body(id: &str) -> String {
    id.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(10)
        .collect()
}

/// Is the typed id usable (has at least one alphanumeric for the interface body)?
#[must_use]
pub fn is_valid_id(id: &str) -> bool {
    !sanitize_ifname_body(id).is_empty()
}

/// `""` → `null`, else `Some(s)` — for the optional `dns` field.
fn option_str(s: &str) -> Option<&str> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tunnels_reads_def_fields_and_egress() {
        let raw = r#"{"ok":true,"tunnels":[
            {"id":"mullvad1","provider":"mullvad","method":"wg","server":"us-nyc",
             "protocol":"udp","creds_ref":"secret://vpn/mullvad1",
             "egress":{"enabled":true,"kill_switch":true}}]}"#;
        let cards = parse_tunnels(raw);
        assert_eq!(cards.len(), 1);
        let c = &cards[0];
        assert_eq!(c.id, "mullvad1");
        assert_eq!(c.provider, "mullvad");
        assert_eq!(c.method, "wg");
        assert_eq!(c.server, "us-nyc");
        assert!(c.kill_switch);
        assert!(c.egress_enabled);
        assert_eq!(c.creds_ref, "secret://vpn/mullvad1");
    }

    #[test]
    fn parse_tunnels_degrades_to_empty_on_garbage() {
        assert!(parse_tunnels("not json").is_empty());
        assert!(parse_tunnels(r#"{"error":"boom"}"#).is_empty());
        assert!(parse_tunnels(r#"{"ok":true,"tunnels":[]}"#).is_empty());
    }

    #[test]
    fn parse_health_maps_verdict_and_exit_ip() {
        let raw = r#"{"ok":true,"tunnels":[
            {"tunnel":"m1","verdict":"healthy","exit_ip":"185.65.1.1","live":true},
            {"tunnel":"m2","verdict":"leaking","exit_ip":null,"live":true}]}"#;
        let h = parse_health(raw);
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].0, "m1");
        assert_eq!(h[0].1.verdict, "healthy");
        assert_eq!(h[0].1.exit_ip.as_deref(), Some("185.65.1.1"));
        assert_eq!(h[1].1.verdict, "leaking");
        assert!(h[1].1.exit_ip.is_none());
    }

    #[test]
    fn parse_iface_up_reads_the_bool() {
        assert!(parse_iface_up(
            r#"{"ok":true,"ifname":"mvpn-m1","up":true}"#
        ));
        assert!(!parse_iface_up(r#"{"ok":true,"up":false}"#));
        assert!(!parse_iface_up("garbage"));
    }

    #[test]
    fn status_for_prefers_health_verdict_then_iface() {
        let mut c = TunnelCard {
            id: "m1".into(),
            iface_up: true,
            ..Default::default()
        };
        // No health → bare interface check.
        assert_eq!(status_for(&c).0, "up");
        c.iface_up = false;
        assert_eq!(status_for(&c).0, "down");
        // Health verdict wins over the interface check.
        c.iface_up = true;
        c.health = Some(TunnelHealthRow {
            verdict: "leaking".into(),
            exit_ip: None,
            live: true,
        });
        let (label, sev) = status_for(&c);
        assert_eq!(label, "leaking");
        assert_eq!(sev, BadgeSeverity::Danger);
    }

    #[test]
    fn killswitch_upsert_round_trips_through_tunnel_def() {
        let card = TunnelCard {
            id: "mullvad1".into(),
            provider: "mullvad".into(),
            method: "wg".into(),
            server: "us-nyc".into(),
            protocol: "udp".into(),
            kill_switch: false,
            egress_enabled: false,
            creds_ref: "secret://vpn/mullvad1".into(),
            ..Default::default()
        };
        let def = card.to_tunnel_def_with_killswitch(true);
        assert_eq!(def.id, "mullvad1");
        assert_eq!(def.method, Method::Wg);
        assert!(def.egress.kill_switch);
        // The sealed-secret link survives the upsert (else the .age blob orphans).
        assert_eq!(def.creds_ref, "secret://vpn/mullvad1");
        // Arming the kill-switch enables egress (a kill-switch with no egress is
        // a no-op) — and the def serializes to the add-tunnel body.
        assert!(def.egress.enabled);
        let body = serde_json::to_string(&def).unwrap();
        assert!(body.contains("mullvad1"));
        assert!(def.validate().is_ok());
    }

    #[test]
    fn wizard_advance_gating_per_step() {
        let mut w = Wizard::default();
        assert_eq!(w.step, WizardStep::Provider);
        assert!(w.can_advance()); // provider always selectable
        w.step = WizardStep::Method;
        assert!(w.can_advance());
        // Config (provider method) needs the portal peer fields.
        w.step = WizardStep::Config;
        w.method = WizardMethod::Provider;
        assert!(!w.can_advance());
        w.server_pubkey = "k".into();
        w.endpoint = "h:1".into();
        w.assigned_address = "10.0.0.2/32".into();
        assert!(w.can_advance());
        // Paste needs config text.
        w.method = WizardMethod::PasteWg;
        assert!(!w.can_advance());
        w.config_text = "[Interface]\n".into();
        assert!(w.can_advance());
        // Name step needs a usable id.
        w.step = WizardStep::Name;
        assert!(!w.can_advance());
        w.tunnel_id = "mullvad-us".into();
        assert!(w.can_advance());
    }

    #[test]
    fn wizard_submit_request_provider_and_import() {
        let mut w = Wizard {
            method: WizardMethod::Provider,
            provider: "mullvad".into(),
            tunnel_id: " exit1 ".into(),
            server: " us-nyc ".into(),
            account_token: "TOKEN".into(),
            server_pubkey: " PK ".into(),
            endpoint: " h:1 ".into(),
            assigned_address: " 10.0.0.2/32 ".into(),
            ..Wizard::default()
        };
        let (verb, body) = w.submit_request("peer:gw");
        assert_eq!(verb, "add-from-provider");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["provider"], "mullvad");
        assert_eq!(v["tunnel_id"], "exit1"); // trimmed
        assert_eq!(v["node_id"], "peer:gw");
        assert_eq!(v["server"], "us-nyc");
        assert_eq!(v["server_pubkey"], "PK");
        assert_eq!(v["account_token"], "TOKEN");

        w.method = WizardMethod::ImportOvpn;
        w.config_text = "client\n".into();
        let (verb, body) = w.submit_request("peer:gw");
        assert_eq!(verb, "import-config");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["kind"], "ovpn");
        assert_eq!(v["tunnel_id"], "exit1");
        assert_eq!(v["config"], "client\n");
    }

    #[test]
    fn reply_ok_or_err_maps_both_outcomes() {
        assert!(reply_ok_or_err(r#"{"ok":true,"tunnel_id":"x"}"#).is_ok());
        assert_eq!(
            reply_ok_or_err(r#"{"error":"no mesh key"}"#).unwrap_err(),
            "no mesh key"
        );
        assert!(reply_ok_or_err("garbage").is_err());
    }

    #[test]
    fn summarize_reply_picks_error_detail_or_ok() {
        assert_eq!(
            summarize_reply("tunnel-up", r#"{"error":"no such tunnel 'x'"}"#),
            "no such tunnel 'x'"
        );
        assert_eq!(
            summarize_reply("tunnel-up", r#"{"ok":true,"detail":"wg-quick up"}"#),
            "tunnel-up: wg-quick up"
        );
        assert_eq!(
            summarize_reply("add-tunnel", r#"{"ok":true}"#),
            "add-tunnel: ok"
        );
    }

    #[test]
    fn is_valid_id_requires_an_alphanumeric() {
        assert!(is_valid_id("mullvad-us"));
        assert!(is_valid_id("a"));
        assert!(!is_valid_id("___"));
        assert!(!is_valid_id(""));
        assert_eq!(sanitize_ifname_body("mull-vad-1"), "mullvad1");
        assert_eq!(sanitize_ifname_body("abcdefghijklmnop").len(), 10);
    }

    #[test]
    fn panel_loaded_records_cards_and_clears_busy() {
        let mut p = VpnPanel::new();
        p.busy = true;
        let _ = p.update(Message::Loaded(Ok(vec![TunnelCard {
            id: "m1".into(),
            ..Default::default()
        }])));
        assert!(!p.busy);
        assert_eq!(p.tunnels.len(), 1);
        assert!(p.load_error.is_none());
    }

    #[test]
    fn panel_loaded_err_sets_error_not_empty_roster() {
        let mut p = VpnPanel::new();
        let _ = p.update(Message::Loaded(Err("bus down".into())));
        assert_eq!(p.load_error.as_deref(), Some("bus down"));
        assert!(p.tunnels.is_empty());
    }

    #[test]
    fn wizard_open_close_and_step_nav() {
        let mut p = VpnPanel::new();
        let _ = p.update(Message::OpenWizard);
        assert!(p.wizard.is_some());
        assert_eq!(p.wizard.as_ref().unwrap().step, WizardStep::Provider);
        // Provider/Method always advance.
        let _ = p.update(Message::WizardNext);
        assert_eq!(p.wizard.as_ref().unwrap().step, WizardStep::Method);
        let _ = p.update(Message::WizardBack);
        assert_eq!(p.wizard.as_ref().unwrap().step, WizardStep::Provider);
        let _ = p.update(Message::CloseWizard);
        assert!(p.wizard.is_none());
    }

    #[test]
    fn toggle_killswitch_for_unknown_card_is_noop() {
        let mut p = VpnPanel::new();
        // No card with this id → no task, busy stays false.
        let _ = p.update(Message::ToggleKillSwitch {
            id: "ghost".into(),
            on: true,
        });
        assert!(!p.busy);
    }

    #[test]
    fn action_finished_clears_busy_and_records_status() {
        let mut p = VpnPanel::new();
        p.busy = true;
        let _ = p.update(Message::ActionFinished(Ok("tunnel-up: ok".into())));
        assert!(!p.busy);
        assert_eq!(p.status, "tunnel-up: ok");
    }
}
