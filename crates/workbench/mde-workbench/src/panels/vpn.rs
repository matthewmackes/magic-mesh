//! Network ▸ VPN panel — VPN-GW-7.
//!
//! Renders the node's mesh **VPN-GW tunnels** (the `TunnelDef` model on the
//! shared substrate), NOT NetworkManager connections. It consumes the
//! `action/vpn/list-tunnels` Bus verb (responder: `mackesd/src/ipc/vpn_gw.rs`)
//! and shows one card per tunnel — provider, server/protocol, `mvpn-<id>`
//! interface name, and an up/down liveness badge — with **Up / Down / Remove**
//! actions wired back over the bus (`action/vpn/{tunnel-up,tunnel-down,
//! remove-tunnel}`), request-reply on a `spawn_blocking` task exactly like the
//! Routing panel's `action/route/trace` (`panels/routing.rs`).
//!
//! The list reply also carries each tunnel's live up/down so the panel renders
//! liveness without a second round-trip per row: `list-tunnels` returns the
//! durable `TunnelDef`s and the panel pairs each with a `tunnel-status` probe
//! batched into the same blocking task.
//!
//! The pre-VPN-GW NetworkManager/`nmcli` implementation is retired — the mesh
//! egress tunnels are a first-class mesh feature served over the Bus, not a
//! host NM connection. (Old config import via `nmcli` is out of scope here; the
//! add-tunnel wizard is VPN-GW-5's `setup-provider` flow.)

use std::time::Duration;

use cosmic::iced::widget::{column, container, pick_list, row, scrollable, text, Space};
use cosmic::iced::{Background, Border, Length, Padding, Task};
use cosmic::{Element, Theme};
use mde_theme::{mde_icon, FontSize, Icon, IconSize, Palette, TypeRole};

use crate::controls::{styled_text_input, variant_button, ButtonVariant};
use crate::cosmic_compat::prelude::*;

/// Read budget for the `action/vpn/*` Bus probes. Matches the other panels'
/// interactive 2 s read window — the responder answers from local config +
/// `ip link` (no network round-trips), so 2 s is generous.
const VPN_TIMEOUT: Duration = Duration::from_secs(2);

/// One tunnel row, parsed from a `list-tunnels` `TunnelDef` paired with its
/// `tunnel-status` liveness. Mirrors the fields the operator acts on: which
/// provider/server, how it's brought up, the `mvpn-<id>` interface, up/down.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VpnRow {
    /// Operator-chosen tunnel id (the action argument for up/down/remove).
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
}

/// VPN-GW-7 — one provider from `action/vpn/list-providers`, the facts the
/// add-tunnel wizard needs to drive the right config form + show the exit-check.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderInfo {
    /// Provider id/label (`mullvad` … `generic-wg` / `generic-ovpn`).
    pub id: String,
    /// Bring-up method (`wg`/`ovpn`/`cli`/`api`).
    pub method: String,
    /// Whether the provider permits same-account multi-instance tunnels.
    pub multi_instance: bool,
    /// The exit-IP check target the daemon verifies the egress against.
    pub exit_check: String,
}

/// Pure decoder for the `list-providers` reply
/// `{"ok":true,"providers":[{id,method,multi_instance,exit_check,...}]}`.
#[must_use]
pub fn parse_providers_reply(raw: &str) -> Vec<ProviderInfo> {
    let v: serde_json::Value = match serde_json::from_str(raw.trim()) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    v.get("providers")
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            arr.iter()
                .map(|p| ProviderInfo {
                    id: str_field(p, "id"),
                    method: str_field(p, "method"),
                    multi_instance: p
                        .get("multi_instance")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(true),
                    exit_check: str_field(p, "exit_check"),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Read a string field (empty if absent/non-string).
fn str_field(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// VPN-GW-7 — the add-tunnel wizard's step sequence (Q10:
/// provider → method/config/auth → server → multi-instance name → verify → save).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WizardStep {
    /// Pick a provider (drives the method + the config form).
    #[default]
    Provider,
    /// Enter the WG/OVPN config or auth material.
    Config,
    /// Pick the server/region.
    Server,
    /// Name the tunnel (the multi-instance id → `mvpn-<id>`).
    Name,
    /// Review + save (encrypt the secret into the mesh store).
    Review,
}

/// Which wizard text field an edit targets (one message carries the field).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardField {
    Id,
    Server,
    PrivateKey,
    PeerPublicKey,
    Address,
    Endpoint,
    Dns,
    WgPaste,
    Ovpn,
}

/// VPN-GW-7 — the add-tunnel wizard state. The fields it collects feed
/// `action/vpn/setup-provider` (the existing VPN-GW-5 adapter) in one call on
/// Save; the secret material is age-encrypted into the mesh store daemon-side.
#[derive(Debug, Clone, Default)]
pub struct TunnelWizard {
    pub step: WizardStep,
    /// Selected provider id.
    pub provider: String,
    /// Method derived from the provider (`wg`/`ovpn`).
    pub method: String,
    /// For a WG provider: paste a `.conf` (true) vs. fill structured fields.
    pub paste_mode: bool,
    pub id: String,
    pub server: String,
    pub private_key: String,
    pub peer_public_key: String,
    pub address: String,
    pub endpoint: String,
    pub dns: String,
    pub wg_config: String,
    pub ovpn: String,
    /// The selected provider's exit-check target (shown at Review).
    pub exit_check: String,
    /// In-flight Save (setup-provider request).
    pub saving: bool,
}

impl TunnelWizard {
    /// Is this provider an OpenVPN-only one (`.ovpn` import path)?
    #[must_use]
    pub fn is_ovpn(&self) -> bool {
        self.method == "ovpn"
    }

    /// Can the wizard advance from the current step? (The Next/Save gate.)
    #[must_use]
    pub fn can_advance(&self) -> bool {
        match self.step {
            WizardStep::Provider => !self.provider.trim().is_empty(),
            WizardStep::Config => {
                if self.is_ovpn() {
                    !self.ovpn.trim().is_empty()
                } else if self.paste_mode {
                    !self.wg_config.trim().is_empty()
                } else {
                    !self.private_key.trim().is_empty()
                        && !self.peer_public_key.trim().is_empty()
                        && !self.address.trim().is_empty()
                        && !self.endpoint.trim().is_empty()
                }
            }
            WizardStep::Server => true, // server/region is optional
            WizardStep::Name => !self.id.trim().is_empty(),
            WizardStep::Review => self.build_setup_body().is_some() && !self.saving,
        }
    }

    /// The next step in sequence (Review is terminal).
    #[must_use]
    pub fn next_step(&self) -> WizardStep {
        match self.step {
            WizardStep::Provider => WizardStep::Config,
            WizardStep::Config => WizardStep::Server,
            WizardStep::Server => WizardStep::Name,
            WizardStep::Name | WizardStep::Review => WizardStep::Review,
        }
    }

    /// The previous step (Provider is the first).
    #[must_use]
    pub fn prev_step(&self) -> WizardStep {
        match self.step {
            WizardStep::Provider | WizardStep::Config => WizardStep::Provider,
            WizardStep::Server => WizardStep::Config,
            WizardStep::Name => WizardStep::Server,
            WizardStep::Review => WizardStep::Name,
        }
    }

    /// Build the exact `action/vpn/setup-provider` request body from the
    /// collected fields (pure — the wizard's core, unit-tested). Returns `None`
    /// when the required fields for the chosen path are missing, so the Save
    /// button never publishes an under-specified request. Mirrors the responder's
    /// dispatch: `.ovpn` import, WG paste, or structured WG.
    #[must_use]
    pub fn build_setup_body(&self) -> Option<String> {
        let provider = self.provider.trim();
        let id = self.id.trim();
        if provider.is_empty() || id.is_empty() {
            return None;
        }
        let server = self.server.trim();
        let body = if self.is_ovpn() {
            let ovpn = self.ovpn.trim();
            if ovpn.is_empty() {
                return None;
            }
            serde_json::json!({ "provider": provider, "id": id, "server": server, "ovpn": ovpn })
        } else if self.paste_mode {
            let conf = self.wg_config.trim();
            if conf.is_empty() {
                return None;
            }
            serde_json::json!({ "provider": provider, "id": id, "server": server, "wg_config": conf })
        } else {
            let pk = self.private_key.trim();
            let pub_k = self.peer_public_key.trim();
            let addr = self.address.trim();
            let ep = self.endpoint.trim();
            if pk.is_empty() || pub_k.is_empty() || addr.is_empty() || ep.is_empty() {
                return None;
            }
            serde_json::json!({
                "provider": provider,
                "id": id,
                "server": server,
                "private_key": pk,
                "peer_public_key": pub_k,
                "address": addr,
                "endpoint": ep,
                "dns": self.dns.trim(),
            })
        };
        Some(body.to_string())
    }
}

/// DDNS-EGRESS-5 — one row of the DDNS table: a configured record (`name`
/// template · `source` · `on_down`) joined with its live published state
/// (`fqdn` · current `ip` · `status` · `updated`), from `action/ddns/list-records`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DdnsRow {
    /// The config record's name template (the remove/edit key, e.g. `{node}-{provider}`).
    pub name: String,
    /// IP source (`tunnel:<id>` or `wan`).
    pub source: String,
    /// On-down policy (`remove`/`sentinel`/`keep`).
    pub on_down: String,
    /// The published hostname (`<label>.<zone>`), empty until first published.
    pub fqdn: String,
    /// The current published IP (empty when removed / pending).
    pub ip: String,
    /// Sync status (`synced`/`stale`/`removed`/`sentinel`/`error`/`pending`).
    pub status: String,
    /// TTL the record was written with.
    pub ttl: u32,
    /// Last update (unix ms; 0 = never).
    pub updated_ms: u64,
}

/// DDNS-EGRESS-5 — the DDNS table + the bits the add/edit form needs.
#[derive(Debug, Clone, Default)]
pub struct DdnsTable {
    pub enabled: bool,
    pub zone: String,
    pub ttl: u32,
    pub rows: Vec<DdnsRow>,
}

impl DdnsTable {
    /// The published row for a tunnel id (`tunnel:<id>` source), for a tunnel
    /// card's "published as" line. `None` when no record tracks it.
    #[must_use]
    pub fn published_for(&self, tunnel_id: &str) -> Option<&DdnsRow> {
        let want = format!("tunnel:{tunnel_id}");
        self.rows
            .iter()
            .find(|r| r.source == want && !r.fqdn.is_empty())
    }
}

/// Pure decoder for the `list-records` reply into a [`DdnsTable`]: the configured
/// records (the authoritative list + the remove/edit key) joined with the live
/// published state by `source`. `{"error":m}` or garbage → an empty (disabled)
/// table. The panel renders the empty state in that case.
#[must_use]
pub fn parse_ddns_records_reply(raw: &str) -> DdnsTable {
    let v: serde_json::Value = match serde_json::from_str(raw.trim()) {
        Ok(v) => v,
        Err(_) => return DdnsTable::default(),
    };
    if v.get("error").is_some() {
        return DdnsTable::default();
    }
    // The published per-name state (live truth), indexed for the join by source.
    let published: Vec<serde_json::Value> = v
        .get("records")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let find_pub = |source: &str| {
        published
            .iter()
            .find(|p| p.get("source").and_then(serde_json::Value::as_str) == Some(source))
    };
    let rows = v
        .get("config_records")
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            arr.iter()
                .map(|c| {
                    let source = str_field(c, "source");
                    let pubrec = find_pub(&source);
                    DdnsRow {
                        name: str_field(c, "name"),
                        source: source.clone(),
                        on_down: str_field(c, "on_down"),
                        fqdn: pubrec.map(|p| str_field(p, "fqdn")).unwrap_or_default(),
                        ip: pubrec.map(|p| str_field(p, "ip")).unwrap_or_default(),
                        status: pubrec
                            .map(|p| str_field(p, "status"))
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| "pending".to_string()),
                        ttl: pubrec
                            .and_then(|p| p.get("ttl").and_then(serde_json::Value::as_u64))
                            .unwrap_or(0) as u32,
                        updated_ms: pubrec
                            .and_then(|p| p.get("updated_ms").and_then(serde_json::Value::as_u64))
                            .unwrap_or(0),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    DdnsTable {
        enabled: v
            .get("enabled")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        zone: str_field(&v, "zone"),
        ttl: v
            .get("ttl")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32,
        rows,
    }
}

#[derive(Debug, Clone, Default)]
pub struct VpnPanel {
    /// `false` when the VPN responder didn't answer (`mackesd` down / no Bus).
    pub daemon_up: bool,
    pub tunnels: Vec<VpnRow>,
    pub status: String,
    pub busy: bool,
    /// VPN-GW-7 — the provider catalog (for the add-tunnel wizard).
    pub providers: Vec<ProviderInfo>,
    /// VPN-GW-7 — the add-tunnel wizard, when open.
    pub wizard: Option<TunnelWizard>,
    /// DDNS-EGRESS-5 — the live DDNS table.
    pub ddns: DdnsTable,
    /// DDNS-EGRESS-5 — the inline add-record form fields (name template / source
    /// / on_down); shown under the table when the operator clicks "Add record".
    pub ddns_form: Option<DdnsForm>,
}

/// DDNS-EGRESS-5 — the inline add/edit-record form state.
#[derive(Debug, Clone, Default)]
pub struct DdnsForm {
    pub name: String,
    pub source: String,
    pub on_down: String,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// A `list-tunnels` (+ batched per-row `tunnel-status`) fetch landed.
    Loaded(Result<Vec<VpnRow>, String>),
    /// VPN-GW-7 — the provider catalog landed (`list-providers`).
    ProvidersLoaded(Vec<ProviderInfo>),
    /// DDNS-EGRESS-5 — the DDNS table landed (`list-records`).
    DdnsLoaded(DdnsTable),
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
    /// An up/down/remove action reply landed.
    OperationFinished(Result<String, String>),
    // ── VPN-GW-7 — add-tunnel wizard ──
    /// Open the add-tunnel wizard.
    WizardOpen,
    /// Close it without saving.
    WizardCancel,
    /// A provider was picked (sets the method + exit-check).
    WizardProviderSelected(String),
    /// A wizard text field changed.
    WizardFieldChanged(WizardField, String),
    /// Toggle the WG paste-config mode.
    WizardTogglePaste(bool),
    /// Advance / go back a step.
    WizardNext,
    WizardBack,
    /// Save (encrypt) — runs `setup-provider`.
    WizardSave,
    /// The `setup-provider` reply landed.
    WizardSaved(Result<String, String>),
    // ── DDNS-EGRESS-5 — DDNS table ──
    /// Trigger an immediate DDNS reconcile (`sync-now`).
    DdnsSyncNow,
    /// Open / close the add-record form.
    DdnsAddOpen,
    DdnsAddCancel,
    /// An add-record form field changed (name / source / on_down).
    DdnsFormName(String),
    DdnsFormSource(String),
    DdnsFormOnDown(String),
    /// Submit the add-record form (`add-record`).
    DdnsAddSubmit,
    /// Remove a DDNS record by its name template (`remove-record`).
    DdnsRemove(String),
    /// A DDNS action reply landed (sync-now / add / remove) — re-fetches.
    DdnsOpFinished(Result<String, String>),
}

impl VpnPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Fetch the tunnel list (+ liveness), the provider catalog (VPN-GW-7), and
    /// the DDNS table (DDNS-EGRESS-5) over the Bus — batched so the panel renders
    /// the whole surface in one refresh. The Bus client builds its own
    /// current-thread runtime, so each blocking fetch rides `spawn_blocking`.
    pub fn load() -> Task<crate::Message> {
        Task::batch([
            Task::perform(
                async move {
                    let joined = tokio::task::spawn_blocking(fetch_tunnels).await;
                    let result = joined.unwrap_or_else(|e| Err(format!("vpn fetch task: {e}")));
                    crate::Message::Vpn(Message::Loaded(result))
                },
                |m| m,
            ),
            Task::perform(
                async move {
                    let joined = tokio::task::spawn_blocking(fetch_providers).await;
                    crate::Message::Vpn(Message::ProvidersLoaded(joined.unwrap_or_default()))
                },
                |m| m,
            ),
            Task::perform(
                async move {
                    let joined = tokio::task::spawn_blocking(fetch_ddns).await;
                    crate::Message::Vpn(Message::DdnsLoaded(joined.unwrap_or_default()))
                },
                |m| m,
            ),
        ])
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded(Ok(tunnels)) => {
                self.daemon_up = true;
                self.tunnels = tunnels;
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
            Message::OperationFinished(result) => {
                self.busy = false;
                self.status = match result {
                    Ok(msg) => msg,
                    Err(msg) => msg,
                };
                // Re-fetch so the row badges + presence reflect the change.
                Self::load()
            }
            Message::ProvidersLoaded(providers) => {
                self.providers = providers;
                Task::none()
            }
            Message::DdnsLoaded(table) => {
                self.ddns = table;
                Task::none()
            }
            // ── VPN-GW-7 — add-tunnel wizard ──
            Message::WizardOpen => {
                self.wizard = Some(TunnelWizard::default());
                Task::none()
            }
            Message::WizardCancel => {
                self.wizard = None;
                Task::none()
            }
            Message::WizardProviderSelected(id) => {
                if let Some(w) = self.wizard.as_mut() {
                    w.provider = id.clone();
                    if let Some(p) = self.providers.iter().find(|p| p.id == id) {
                        w.method = p.method.clone();
                        w.exit_check = p.exit_check.clone();
                    }
                    // A generic-ovpn provider has no structured-WG path.
                    if w.is_ovpn() {
                        w.paste_mode = false;
                    }
                }
                Task::none()
            }
            Message::WizardFieldChanged(field, value) => {
                if let Some(w) = self.wizard.as_mut() {
                    match field {
                        WizardField::Id => w.id = value,
                        WizardField::Server => w.server = value,
                        WizardField::PrivateKey => w.private_key = value,
                        WizardField::PeerPublicKey => w.peer_public_key = value,
                        WizardField::Address => w.address = value,
                        WizardField::Endpoint => w.endpoint = value,
                        WizardField::Dns => w.dns = value,
                        WizardField::WgPaste => w.wg_config = value,
                        WizardField::Ovpn => w.ovpn = value,
                    }
                }
                Task::none()
            }
            Message::WizardTogglePaste(on) => {
                if let Some(w) = self.wizard.as_mut() {
                    w.paste_mode = on;
                }
                Task::none()
            }
            Message::WizardNext => {
                if let Some(w) = self.wizard.as_mut() {
                    if w.can_advance() {
                        w.step = w.next_step();
                    }
                }
                Task::none()
            }
            Message::WizardBack => {
                if let Some(w) = self.wizard.as_mut() {
                    w.step = w.prev_step();
                }
                Task::none()
            }
            Message::WizardSave => {
                let Some(body) = self
                    .wizard
                    .as_ref()
                    .and_then(TunnelWizard::build_setup_body)
                else {
                    return Task::none();
                };
                if let Some(w) = self.wizard.as_mut() {
                    w.saving = true;
                }
                self.status = "Saving tunnel (encrypting secret)…".into();
                Task::perform(
                    async move {
                        let joined =
                            tokio::task::spawn_blocking(move || request_setup(&body)).await;
                        let result = joined.unwrap_or_else(|e| Err(format!("setup task: {e}")));
                        crate::Message::Vpn(Message::WizardSaved(result))
                    },
                    |m| m,
                )
            }
            Message::WizardSaved(result) => {
                match result {
                    Ok(msg) => {
                        self.status = msg;
                        self.wizard = None; // close on success
                        Self::load()
                    }
                    Err(msg) => {
                        self.status = msg;
                        if let Some(w) = self.wizard.as_mut() {
                            w.saving = false;
                        }
                        Task::none()
                    }
                }
            }
            // ── DDNS-EGRESS-5 — DDNS table ──
            Message::DdnsSyncNow => {
                self.status = "Requesting DDNS sync…".into();
                ddns_op_task("sync-now", None)
            }
            Message::DdnsAddOpen => {
                self.ddns_form = Some(DdnsForm {
                    on_down: "remove".into(),
                    ..DdnsForm::default()
                });
                Task::none()
            }
            Message::DdnsAddCancel => {
                self.ddns_form = None;
                Task::none()
            }
            Message::DdnsFormName(v) => {
                if let Some(f) = self.ddns_form.as_mut() {
                    f.name = v;
                }
                Task::none()
            }
            Message::DdnsFormSource(v) => {
                if let Some(f) = self.ddns_form.as_mut() {
                    f.source = v;
                }
                Task::none()
            }
            Message::DdnsFormOnDown(v) => {
                if let Some(f) = self.ddns_form.as_mut() {
                    f.on_down = v;
                }
                Task::none()
            }
            Message::DdnsAddSubmit => {
                let Some(f) = self.ddns_form.as_ref() else {
                    return Task::none();
                };
                if f.name.trim().is_empty() || f.source.trim().is_empty() {
                    self.status = "DDNS record needs a name template + source".into();
                    return Task::none();
                }
                let on_down = if f.on_down.trim().is_empty() {
                    "remove".to_string()
                } else {
                    f.on_down.trim().to_string()
                };
                let body = serde_json::json!({
                    "name": f.name.trim(),
                    "source": f.source.trim(),
                    "on_down": on_down,
                })
                .to_string();
                self.ddns_form = None;
                self.status = "Adding DDNS record…".into();
                ddns_op_task("add-record", Some(body))
            }
            Message::DdnsRemove(name) => {
                self.status = format!("Removing DDNS record {name}…");
                ddns_op_task("remove-record", Some(name))
            }
            Message::DdnsOpFinished(result) => {
                self.status = match result {
                    Ok(msg) => msg,
                    Err(msg) => msg,
                };
                Self::load()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let sizes = FontSize::defaults();

        let title = text("VPN")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());
        let subtitle = text("mesh egress tunnels")
            .size(TypeRole::Body.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color());

        let wizard_open = self.wizard.is_some();
        let add_btn = variant_button(
            "Add tunnel",
            ButtonVariant::Primary,
            (!self.busy && !wizard_open).then_some(crate::Message::Vpn(Message::WizardOpen)),
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

        // VPN-GW-7 — the add-tunnel wizard takes over the body when open.
        if let Some(w) = &self.wizard {
            body_col = body_col.push(wizard_view(w, &self.providers, palette));
        } else if !self.daemon_up {
            body_col = body_col.push(empty_state(
                Icon::StatusError,
                palette.danger,
                "VPN gateway unreachable",
                "Couldn't reach the mesh VPN responder over the Bus. Is `mackesd` \
                 running on this node?",
                palette,
            ));
        } else {
            if self.tunnels.is_empty() {
                body_col = body_col.push(empty_state(
                    Icon::Vpn,
                    palette.accent,
                    "No tunnels configured",
                    "This node has no VPN-GW egress tunnels yet. Click \"Add tunnel\" \
                     to run the provider wizard.",
                    palette,
                ));
            } else {
                for t in &self.tunnels {
                    body_col = body_col.push(tunnel_card(t, self.busy, &self.ddns, palette));
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
            // DDNS-EGRESS-5 — the DDNS table section under the tunnels.
            body_col = body_col.push(Space::new().height(Length::Fixed(8.0)));
            body_col = body_col.push(ddns_section(&self.ddns, self.ddns_form.as_ref(), palette));
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

/// One tunnel's card: provider · server/protocol header, the `mvpn-<id>`
/// interface + up/down badge, the Up/Down + Remove actions, and (DDNS-EGRESS-5)
/// a "published as <hostname>" line when a DDNS record tracks this tunnel.
fn tunnel_card<'a>(
    t: &VpnRow,
    busy: bool,
    ddns: &DdnsTable,
    palette: Palette,
) -> Element<'a, crate::Message> {
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
    let detail = detail_line(t);
    let detail_row = text(detail)
        .size(TypeRole::Caption.size_in(sizes))
        .colr(palette.text_muted.into_cosmic_color());
    let iface_row = text(format!("{} · {}", t.id, t.ifname))
        .size(TypeRole::Caption.size_in(sizes))
        .colr(palette.text_muted.into_cosmic_color());

    // Up XOR Down (the relevant transition), plus Remove.
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
    let remove_btn = variant_button(
        "Remove",
        ButtonVariant::Ghost,
        (!busy).then(|| crate::Message::Vpn(Message::RemoveClicked { id: t.id.clone() })),
        palette,
    );
    let actions = row![
        Space::new().width(Length::Fill),
        toggle_btn,
        Space::new().width(Length::Fixed(8.0)),
        remove_btn,
    ]
    .align_y(cosmic::iced::alignment::Vertical::Center);

    let mut inner = column![head, detail_row, iface_row].spacing(6);
    // DDNS-EGRESS-5 — "published as <hostname>" + whether it currently resolves
    // to the verified exit IP (status synced) or is stale/down.
    if let Some(rec) = ddns.published_for(&t.id) {
        let (line, color) = published_as_line(rec, palette);
        inner = inner.push(
            text(line)
                .size(TypeRole::Caption.size_in(sizes))
                .colr(color.into_cosmic_color()),
        );
    }
    inner = inner.push(actions);
    card(inner, palette)
}

/// DDNS-EGRESS-5 — the per-tunnel "published as" caption + its tone: green when
/// the name currently resolves to the verified exit IP (`synced`), muted when
/// removed/pending, danger on `error`/`stale`. Pure — unit-tested.
#[must_use]
pub fn published_as_line(rec: &DdnsRow, palette: Palette) -> (String, mde_theme::Rgba) {
    let (suffix, color) = match rec.status.as_str() {
        "synced" => (
            if rec.ip.is_empty() {
                String::new()
            } else {
                format!(" → {}", rec.ip)
            },
            palette.success,
        ),
        "removed" | "pending" | "" => (" (not published)".to_string(), palette.text_muted),
        "sentinel" => (" (parked — tunnel down)".to_string(), palette.text_muted),
        other => (format!(" ({other})"), palette.danger),
    };
    (format!("published as {}{suffix}", rec.fqdn), color)
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
    icon_color: mde_theme::Rgba,
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

/// A labeled Carbon-token text-input row (shared by the wizard + the DDNS form).
fn labeled_input<'a>(
    label: &'static str,
    hint: &'static str,
    value: &'a str,
    on_input: impl Fn(String) -> crate::Message + 'a,
    palette: Palette,
) -> Element<'a, crate::Message> {
    row![
        text(label)
            .size(11)
            .colr(palette.text_muted.into_cosmic_color())
            .width(Length::Fixed(130.0)),
        styled_text_input(hint, value, on_input, palette),
    ]
    .spacing(8)
    .align_y(cosmic::iced::alignment::Vertical::Center)
    .into()
}

/// VPN-GW-7 — render the add-tunnel wizard (provider → config/auth → server →
/// name → review/save). Carbon tokens throughout (§4).
fn wizard_view<'a>(
    w: &'a TunnelWizard,
    providers: &'a [ProviderInfo],
    palette: Palette,
) -> Element<'a, crate::Message> {
    let sizes = FontSize::defaults();
    let step_n = match w.step {
        WizardStep::Provider => 1,
        WizardStep::Config => 2,
        WizardStep::Server => 3,
        WizardStep::Name => 4,
        WizardStep::Review => 5,
    };
    let title = text(format!("Add VPN tunnel — step {step_n} of 5"))
        .size(TypeRole::Subheading.size_in(sizes))
        .colr(palette.text.into_cosmic_color());
    let subtitle = text(wizard_step_label(w.step))
        .size(TypeRole::Caption.size_in(sizes))
        .colr(palette.text_muted.into_cosmic_color());

    let body: Element<'a, crate::Message> = match w.step {
        WizardStep::Provider => wizard_provider_step(w, providers, palette),
        WizardStep::Config => wizard_config_step(w, palette),
        WizardStep::Server => column![labeled_input(
            "Server / region",
            "e.g. us-nyc (optional)",
            &w.server,
            |v| crate::Message::Vpn(Message::WizardFieldChanged(WizardField::Server, v)),
            palette,
        )]
        .spacing(8)
        .into(),
        WizardStep::Name => wizard_name_step(w, providers, palette),
        WizardStep::Review => wizard_review_step(w, palette),
    };

    let back_btn = variant_button(
        "Back",
        ButtonVariant::Ghost,
        (w.step != WizardStep::Provider && !w.saving)
            .then_some(crate::Message::Vpn(Message::WizardBack)),
        palette,
    );
    let cancel_btn = variant_button(
        "Cancel",
        ButtonVariant::Ghost,
        (!w.saving).then_some(crate::Message::Vpn(Message::WizardCancel)),
        palette,
    );
    let advance_btn = if w.step == WizardStep::Review {
        variant_button(
            if w.saving {
                "Saving…"
            } else {
                "Save (encrypt)"
            },
            ButtonVariant::Primary,
            w.can_advance()
                .then_some(crate::Message::Vpn(Message::WizardSave)),
            palette,
        )
    } else {
        variant_button(
            "Next",
            ButtonVariant::Primary,
            w.can_advance()
                .then_some(crate::Message::Vpn(Message::WizardNext)),
            palette,
        )
    };
    let nav = row![
        back_btn,
        Space::new().width(Length::Fill),
        cancel_btn,
        Space::new().width(Length::Fixed(8.0)),
        advance_btn,
    ]
    .align_y(cosmic::iced::alignment::Vertical::Center);

    card(
        column![
            title,
            subtitle,
            Space::new().height(Length::Fixed(8.0)),
            body,
            Space::new().height(Length::Fixed(8.0)),
            nav,
        ]
        .spacing(6),
        palette,
    )
}

/// The one-line description of a wizard step.
#[must_use]
fn wizard_step_label(step: WizardStep) -> &'static str {
    match step {
        WizardStep::Provider => "Pick a provider — the 5 named or a generic WG/OVPN path.",
        WizardStep::Config => "Enter the tunnel's config / key material.",
        WizardStep::Server => "Pick the server / region (optional).",
        WizardStep::Name => "Name the tunnel (the multi-instance id → mvpn-<id>).",
        WizardStep::Review => "Review, then save (the secret is encrypted into the mesh store).",
    }
}

fn wizard_provider_step<'a>(
    w: &'a TunnelWizard,
    providers: &'a [ProviderInfo],
    palette: Palette,
) -> Element<'a, crate::Message> {
    let ids: Vec<String> = providers.iter().map(|p| p.id.clone()).collect();
    let selected = (!w.provider.is_empty()).then(|| w.provider.clone());
    let picker = pick_list(ids, selected, |v| {
        crate::Message::Vpn(Message::WizardProviderSelected(v))
    })
    .placeholder("Choose a provider")
    .text_size(13);
    let note = if w.provider.is_empty() {
        text("Mullvad · Proton · IVPN · Nord · Surfshark · generic-wg · generic-ovpn")
            .size(TypeRole::Caption.size_in(FontSize::defaults()))
            .colr(palette.text_muted.into_cosmic_color())
    } else {
        let method = if w.method.is_empty() { "wg" } else { &w.method };
        let check = if w.exit_check.is_empty() {
            "ipinfo.io"
        } else {
            &w.exit_check
        };
        text(format!(
            "method {method} · verifies the exit IP against {check}"
        ))
        .size(TypeRole::Caption.size_in(FontSize::defaults()))
        .colr(palette.text_muted.into_cosmic_color())
    };
    column![picker, note].spacing(8).into()
}

fn wizard_config_step<'a>(w: &'a TunnelWizard, palette: Palette) -> Element<'a, crate::Message> {
    let muted = palette.text_muted.into_cosmic_color();
    if w.is_ovpn() {
        return column![
            text("OpenVPN import — paste your .ovpn client config:")
                .size(TypeRole::Caption.size_in(FontSize::defaults()))
                .colr(muted),
            styled_text_input(
                "paste the .ovpn here",
                &w.ovpn,
                |v| crate::Message::Vpn(Message::WizardFieldChanged(WizardField::Ovpn, v)),
                palette,
            ),
        ]
        .spacing(8)
        .into();
    }
    let toggle_row = row![
        text("Paste a WireGuard .conf instead")
            .size(TypeRole::Caption.size_in(FontSize::defaults()))
            .colr(muted),
        Space::new().width(Length::Fill),
        crate::controls::toggle(
            w.paste_mode,
            |on| crate::Message::Vpn(Message::WizardTogglePaste(on)),
            palette,
        ),
    ]
    .spacing(8)
    .align_y(cosmic::iced::alignment::Vertical::Center);
    let body: Element<'a, crate::Message> = if w.paste_mode {
        styled_text_input(
            "paste the [Interface]/[Peer] config",
            &w.wg_config,
            |v| crate::Message::Vpn(Message::WizardFieldChanged(WizardField::WgPaste, v)),
            palette,
        )
    } else {
        column![
            labeled_input(
                "Private key",
                "base64 (44 chars)",
                &w.private_key,
                |v| {
                    crate::Message::Vpn(Message::WizardFieldChanged(WizardField::PrivateKey, v))
                },
                palette
            ),
            labeled_input(
                "Peer public key",
                "base64",
                &w.peer_public_key,
                |v| {
                    crate::Message::Vpn(Message::WizardFieldChanged(WizardField::PeerPublicKey, v))
                },
                palette
            ),
            labeled_input(
                "Address",
                "10.64.0.2/32",
                &w.address,
                |v| { crate::Message::Vpn(Message::WizardFieldChanged(WizardField::Address, v)) },
                palette
            ),
            labeled_input(
                "Endpoint",
                "host:port",
                &w.endpoint,
                |v| { crate::Message::Vpn(Message::WizardFieldChanged(WizardField::Endpoint, v)) },
                palette
            ),
            labeled_input(
                "DNS",
                "10.64.0.1 (recommended — avoids a leak)",
                &w.dns,
                |v| { crate::Message::Vpn(Message::WizardFieldChanged(WizardField::Dns, v)) },
                palette
            ),
        ]
        .spacing(6)
        .into()
    };
    column![toggle_row, body].spacing(8).into()
}

fn wizard_name_step<'a>(
    w: &'a TunnelWizard,
    providers: &'a [ProviderInfo],
    palette: Palette,
) -> Element<'a, crate::Message> {
    let multi = providers
        .iter()
        .find(|p| p.id == w.provider)
        .is_none_or(|p| p.multi_instance);
    let note = if multi {
        "This provider allows multiple concurrent tunnels — use a distinct name per instance."
    } else {
        "This provider does not permit multi-instance — one tunnel per account."
    };
    column![
        labeled_input(
            "Tunnel name",
            "e.g. mullvad1 (→ mvpn-mullvad1)",
            &w.id,
            |v| crate::Message::Vpn(Message::WizardFieldChanged(WizardField::Id, v)),
            palette,
        ),
        text(note)
            .size(TypeRole::Caption.size_in(FontSize::defaults()))
            .colr(palette.text_muted.into_cosmic_color()),
    ]
    .spacing(8)
    .into()
}

fn wizard_review_step<'a>(w: &'a TunnelWizard, palette: Palette) -> Element<'a, crate::Message> {
    let ifname = mackes_mesh_types::vpn::TunnelDef {
        id: w.id.clone(),
        ..Default::default()
    }
    .ifname();
    let method = if w.method.is_empty() { "wg" } else { &w.method };
    let check = if w.exit_check.is_empty() {
        "ipinfo.io"
    } else {
        &w.exit_check
    };
    let server = if w.server.trim().is_empty() {
        "(provider default)"
    } else {
        w.server.trim()
    };
    column![
        review_kv("Provider", &w.provider, palette),
        review_kv("Method", method, palette),
        review_kv("Tunnel", &w.id, palette),
        review_kv("Interface", &ifname, palette),
        review_kv("Server", server, palette),
        review_kv("Verifies via", check, palette),
        text(
            "Save encrypts the secret into the mesh store (age) and distributes it to the \
             assigned gateways. Live exit-IP verification runs after bring-up (VPN-GW-6)."
        )
        .size(TypeRole::Caption.size_in(FontSize::defaults()))
        .colr(palette.text_muted.into_cosmic_color()),
    ]
    .spacing(6)
    .into()
}

/// A muted "key  value" review row.
fn review_kv<'a>(key: &'static str, value: &str, palette: Palette) -> Element<'a, crate::Message> {
    row![
        text(key)
            .size(12)
            .colr(palette.text_muted.into_cosmic_color())
            .width(Length::Fixed(110.0)),
        text(value.to_string())
            .size(12)
            .colr(palette.text.into_cosmic_color()),
    ]
    .spacing(8)
    .into()
}

/// DDNS-EGRESS-5 — the DDNS table section: the live records (hostname · source ·
/// IP · TTL · status) with Sync-now + add/remove, over `action/ddns/*`.
fn ddns_section<'a>(
    ddns: &'a DdnsTable,
    form: Option<&'a DdnsForm>,
    palette: Palette,
) -> Element<'a, crate::Message> {
    let sizes = FontSize::defaults();
    let muted = palette.text_muted.into_cosmic_color();
    let title = text("Dynamic DNS")
        .size(TypeRole::Subheading.size_in(sizes))
        .colr(palette.text.into_cosmic_color());
    let subtitle = text(if ddns.enabled {
        format!("zone {} · TTL {}s", ddns.zone, ddns.ttl)
    } else {
        "disabled — enable the [ddns] block in config".to_string()
    })
    .size(TypeRole::Caption.size_in(sizes))
    .colr(muted);
    let sync_btn = variant_button(
        "Sync now",
        ButtonVariant::Ghost,
        Some(crate::Message::Vpn(Message::DdnsSyncNow)),
        palette,
    );
    let add_btn = variant_button(
        "Add record",
        ButtonVariant::Secondary,
        form.is_none()
            .then_some(crate::Message::Vpn(Message::DdnsAddOpen)),
        palette,
    );
    let head = row![
        column![title, subtitle].spacing(2),
        Space::new().width(Length::Fill),
        sync_btn,
        Space::new().width(Length::Fixed(8.0)),
        add_btn,
    ]
    .align_y(cosmic::iced::alignment::Vertical::Center);

    let mut col = column![head].spacing(8);
    if ddns.rows.is_empty() {
        col = col.push(
            text(
                "No DDNS records. Add one to publish a tunnel's verified exit IP \
                 (or the node WAN IP) under your zone.",
            )
            .size(TypeRole::Caption.size_in(sizes))
            .colr(muted),
        );
    } else {
        col = col.push(ddns_header_row(palette));
        for r in &ddns.rows {
            col = col.push(ddns_data_row(r, palette));
        }
    }
    if let Some(f) = form {
        col = col.push(ddns_add_form(f, palette));
    }
    card(col, palette)
}

/// The DDNS table column header.
fn ddns_header_row<'a>(palette: Palette) -> Element<'a, crate::Message> {
    let muted = palette.text_muted.into_cosmic_color();
    let cell = move |s: &'static str, w: f32| text(s).size(11).colr(muted).width(Length::Fixed(w));
    row![
        cell("Hostname", 230.0),
        cell("Source", 120.0),
        cell("IP", 120.0),
        cell("TTL", 45.0),
        cell("Status", 90.0),
    ]
    .spacing(8)
    .align_y(cosmic::iced::alignment::Vertical::Center)
    .into()
}

/// One DDNS table data row with a Remove action (keyed by the name template).
fn ddns_data_row<'a>(r: &'a DdnsRow, palette: Palette) -> Element<'a, crate::Message> {
    let text_color = palette.text.into_cosmic_color();
    let muted = palette.text_muted.into_cosmic_color();
    let host = if r.fqdn.is_empty() {
        format!("(pending) {}", r.name)
    } else {
        r.fqdn.clone()
    };
    let ip = if r.ip.is_empty() {
        "—".to_string()
    } else {
        r.ip.clone()
    };
    let ttl = if r.ttl == 0 {
        "—".to_string()
    } else {
        format!("{}s", r.ttl)
    };
    row![
        text(host)
            .size(12)
            .colr(text_color)
            .width(Length::Fixed(230.0)),
        text(r.source.clone())
            .size(12)
            .colr(muted)
            .width(Length::Fixed(120.0)),
        text(ip).size(12).colr(muted).width(Length::Fixed(120.0)),
        text(ttl).size(12).colr(muted).width(Length::Fixed(45.0)),
        text(r.status.clone())
            .size(12)
            .colr(ddns_status_color(&r.status, palette).into_cosmic_color())
            .width(Length::Fixed(90.0)),
        Space::new().width(Length::Fill),
        variant_button(
            "Remove",
            ButtonVariant::Ghost,
            Some(crate::Message::Vpn(Message::DdnsRemove(r.name.clone()))),
            palette,
        ),
    ]
    .spacing(8)
    .align_y(cosmic::iced::alignment::Vertical::Center)
    .into()
}

/// The tone for a DDNS sync status. Pure — unit-tested.
#[must_use]
pub fn ddns_status_color(status: &str, palette: Palette) -> mde_theme::Rgba {
    match status {
        "synced" => palette.success,
        "error" | "stale" => palette.danger,
        _ => palette.text_muted,
    }
}

/// DDNS-EGRESS-5 — the inline add-record form.
fn ddns_add_form<'a>(f: &'a DdnsForm, palette: Palette) -> Element<'a, crate::Message> {
    let muted = palette.text_muted.into_cosmic_color();
    let on_down_opts = vec![
        "remove".to_string(),
        "sentinel".to_string(),
        "keep".to_string(),
    ];
    let selected = (!f.on_down.is_empty()).then(|| f.on_down.clone());
    let on_down_picker = pick_list(on_down_opts, selected, |v| {
        crate::Message::Vpn(Message::DdnsFormOnDown(v))
    })
    .placeholder("on-down")
    .text_size(13);
    let add_btn = variant_button(
        "Add",
        ButtonVariant::Primary,
        Some(crate::Message::Vpn(Message::DdnsAddSubmit)),
        palette,
    );
    let cancel_btn = variant_button(
        "Cancel",
        ButtonVariant::Ghost,
        Some(crate::Message::Vpn(Message::DdnsAddCancel)),
        palette,
    );
    card(
        column![
            text("New DDNS record")
                .size(TypeRole::Caption.size_in(FontSize::defaults()))
                .colr(muted),
            labeled_input(
                "Name template",
                "{node}-{provider}",
                &f.name,
                |v| crate::Message::Vpn(Message::DdnsFormName(v)),
                palette,
            ),
            labeled_input(
                "Source",
                "tunnel:<id> or wan",
                &f.source,
                |v| crate::Message::Vpn(Message::DdnsFormSource(v)),
                palette,
            ),
            row![
                text("On down")
                    .size(11)
                    .colr(muted)
                    .width(Length::Fixed(130.0)),
                on_down_picker,
            ]
            .spacing(8)
            .align_y(cosmic::iced::alignment::Vertical::Center),
            row![
                Space::new().width(Length::Fill),
                cancel_btn,
                Space::new().width(Length::Fixed(8.0)),
                add_btn,
            ]
            .align_y(cosmic::iced::alignment::Vertical::Center),
        ]
        .spacing(6),
        palette,
    )
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

/// Request one `action/vpn/<verb>` over the Bus with `id` as the body, decoding
/// the `{ok, detail?}` reply into a human-readable result. Blocking.
fn request_op(verb: &str, id: &str) -> Result<String, String> {
    let topic = format!("action/vpn/{verb}");
    let raw = crate::dbus::action_request_with_body(&topic, Some(id), VPN_TIMEOUT)
        .ok_or_else(|| format!("mackesd not reachable over the Bus (vpn/{verb})"))?;
    parse_op_reply(verb, id, &raw)
}

/// VPN-GW-7 — read budget for `setup-provider` (it renders + age-encrypts the
/// secret into the store), a touch longer than the interactive probe window.
const SETUP_TIMEOUT: Duration = Duration::from_secs(6);

/// Fetch the provider catalog (`list-providers`). Empty on no answer. Blocking.
fn fetch_providers() -> Vec<ProviderInfo> {
    crate::dbus::action_request("action/vpn/list-providers", VPN_TIMEOUT)
        .map(|raw| parse_providers_reply(&raw))
        .unwrap_or_default()
}

/// Fetch the DDNS table (`list-records`). Empty/disabled on no answer. Blocking.
fn fetch_ddns() -> DdnsTable {
    crate::dbus::action_request("action/ddns/list-records", VPN_TIMEOUT)
        .map(|raw| parse_ddns_records_reply(&raw))
        .unwrap_or_default()
}

/// VPN-GW-7 — run `setup-provider` with the wizard body; decode the reply.
fn request_setup(body: &str) -> Result<String, String> {
    let raw = crate::dbus::action_request_with_body(
        "action/vpn/setup-provider",
        Some(body),
        SETUP_TIMEOUT,
    )
    .ok_or_else(|| "mackesd not reachable over the Bus (vpn/setup-provider)".to_string())?;
    parse_setup_reply(&raw)
}

/// Pure decoder for the `setup-provider` reply into a human-readable status.
#[must_use]
pub fn parse_setup_reply(raw: &str) -> Result<String, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw.trim()).map_err(|e| format!("bad setup-provider reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(serde_json::Value::as_str) {
        return Err(err.to_string());
    }
    if v.get("ok")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        let id = v
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("tunnel");
        let distributed = v
            .get("secret_distributed")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let note = if distributed {
            "secret encrypted + distributed"
        } else {
            "saved (secret not yet distributed)"
        };
        Ok(format!("Tunnel {id} saved — {note}"))
    } else {
        Err("setup-provider did not succeed".to_string())
    }
}

/// DDNS-EGRESS-5 — build the task for a `action/ddns/<verb>` op (sync-now /
/// add-record / remove-record), routing the reply back as [`Message::DdnsOpFinished`].
fn ddns_op_task(verb: &'static str, body: Option<String>) -> Task<crate::Message> {
    Task::perform(
        async move {
            let joined =
                tokio::task::spawn_blocking(move || request_ddns_op(verb, body.as_deref())).await;
            let result = joined.unwrap_or_else(|e| Err(format!("ddns op task: {e}")));
            crate::Message::Vpn(Message::DdnsOpFinished(result))
        },
        |m| m,
    )
}

/// Request one `action/ddns/<verb>` over the Bus, decoding the reply. Blocking.
fn request_ddns_op(verb: &str, body: Option<&str>) -> Result<String, String> {
    let topic = format!("action/ddns/{verb}");
    let raw = crate::dbus::action_request_with_body(&topic, body, VPN_TIMEOUT)
        .ok_or_else(|| format!("mackesd not reachable over the Bus (ddns/{verb})"))?;
    parse_ddns_op_reply(verb, &raw)
}

/// Pure decoder for a DDNS op reply into a human-readable status line.
#[must_use]
pub fn parse_ddns_op_reply(verb: &str, raw: &str) -> Result<String, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw.trim()).map_err(|e| format!("bad {verb} reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(serde_json::Value::as_str) {
        return Err(err.to_string());
    }
    if v.get("ok")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        Ok(match verb {
            "sync-now" => "DDNS sync requested.".to_string(),
            "add-record" => "DDNS record added.".to_string(),
            "remove-record" => "DDNS record removed.".to_string(),
            _ => format!("{verb}: ok"),
        })
    } else {
        Err(format!("{verb} did not succeed"))
    }
}

/// Pure decoder for the `list-tunnels` reply envelope
/// `{"ok":true,"tunnels":[<TunnelDef>...]}` → one [`VpnRow`] per tunnel (live
/// up/down filled in later by the paired status probe). `{"error":m}` → `Err`.
/// Split out so the wire contract is unit-testable without the Bus.
#[must_use = "returns the parsed rows or the responder error"]
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

/// Build a [`VpnRow`] from one `TunnelDef` JSON object. The `TunnelDef` model
/// (`mackes_mesh_types::vpn`) carries `id`/`provider`/`method`/`server`/
/// `protocol`; the `mvpn-<id>` interface is derived the same way the model does
/// (`mackes_mesh_types::vpn::TunnelDef::ifname`) so the card shows the real
/// device name. `up` defaults false — the status probe fills it.
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
    }
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

/// Pure decoder for an up/down/remove reply into a human-readable status line.
/// The responder replies `{"ok":bool,"detail":...}` (tunnel-up/down) or
/// `{"ok":true}` (remove), or `{"error":m}`. Split out for unit-testing.
#[must_use]
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

        let w = &rows[1];
        assert_eq!(w.provider, "generic-ovpn");
        assert_eq!(w.method, "ovpn");
        assert_eq!(w.ifname, "mvpn-work");
    }

    #[test]
    fn parse_tunnels_reply_defaults_absent_method_to_wg() {
        // `method` is `#[serde(default)]` on TunnelDef — an absent field means
        // the WG default, which the row mirrors so the detail line isn't blank.
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
        // An unreadable/garbage status is "not known up".
        assert!(!parse_status_up(r#"{"error":"no such tunnel"}"#));
        assert!(!parse_status_up("garbage"));
    }

    #[test]
    fn parse_op_reply_up_down_and_remove_humanise() {
        // tunnel-up ok carries the responder detail.
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
        // tunnel-up with spawn disabled / config missing replies ok:false +
        // detail — the panel surfaces that as an error, not a success.
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
        // A generic tunnel with no server/protocol reads as just the method.
        let bare = VpnRow {
            method: "wg".into(),
            ..Default::default()
        };
        assert_eq!(detail_line(&bare), "wg");
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
        panel.status = "m1 up — wg-quick up".into();
        // DDNS table + a per-tunnel "published as" line.
        panel.ddns = parse_ddns_records_reply(
            r#"{"ok":true,"enabled":true,"zone":"services.matthewmackes.com","ttl":60,
                "records":[{"fqdn":"eagle-mullvad.services.matthewmackes.com","source":"tunnel:m1",
                            "ip":"1.2.3.4","status":"synced","ttl":60,"updated_ms":1}],
                "config_records":[{"name":"{node}-{provider}","source":"tunnel:m1","on_down":"remove"}]}"#,
        );
        let _ = panel.view(); // a live tunnel card + DDNS table + status strip
                              // The wizard takes over the body.
        panel.providers = parse_providers_reply(
            r#"{"ok":true,"providers":[{"id":"mullvad","method":"wg","multi_instance":true,"exit_check":"https://am.i.mullvad.net/json"}]}"#,
        );
        let _ = panel.update(Message::WizardOpen);
        for step in [
            WizardStep::Provider,
            WizardStep::Config,
            WizardStep::Server,
            WizardStep::Name,
            WizardStep::Review,
        ] {
            if let Some(w) = panel.wizard.as_mut() {
                w.step = step;
            }
            let _ = panel.view();
        }
        // The DDNS add-record form renders too.
        panel.wizard = None;
        let _ = panel.update(Message::DdnsAddOpen);
        let _ = panel.view();
    }

    // ── VPN-GW-7 — wizard ──

    #[test]
    fn parse_providers_reply_maps_the_catalog() {
        let raw = r#"{"ok":true,"providers":[
            {"id":"mullvad","method":"wg","multi_instance":true,"exit_check":"https://am.i.mullvad.net/json"},
            {"id":"generic-ovpn","method":"ovpn","multi_instance":true,"exit_check":"https://ipinfo.io/json"}
        ]}"#;
        let p = parse_providers_reply(raw);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].id, "mullvad");
        assert_eq!(p[0].exit_check, "https://am.i.mullvad.net/json");
        assert_eq!(p[1].method, "ovpn");
        assert!(parse_providers_reply("garbage").is_empty());
    }

    #[test]
    fn wizard_build_setup_body_structured_paste_and_ovpn() {
        // Structured WG.
        let mut w = TunnelWizard {
            provider: "mullvad".into(),
            method: "wg".into(),
            id: "m1".into(),
            server: "us-nyc".into(),
            private_key: "k".into(),
            peer_public_key: "p".into(),
            address: "10.64.0.2/32".into(),
            endpoint: "h:51820".into(),
            ..Default::default()
        };
        let body = w.build_setup_body().expect("structured body");
        assert!(body.contains("\"private_key\":\"k\"") && body.contains("\"id\":\"m1\""));
        // Missing a required field → None.
        w.endpoint.clear();
        assert!(w.build_setup_body().is_none());
        // Paste mode.
        let wp = TunnelWizard {
            provider: "generic-wg".into(),
            method: "wg".into(),
            id: "p1".into(),
            paste_mode: true,
            wg_config: "[Interface]\n".into(),
            ..Default::default()
        };
        assert!(wp.build_setup_body().unwrap().contains("\"wg_config\""));
        // OVPN.
        let wo = TunnelWizard {
            provider: "generic-ovpn".into(),
            method: "ovpn".into(),
            id: "o1".into(),
            ovpn: "client\nremote h 1194\n".into(),
            ..Default::default()
        };
        assert!(wo.build_setup_body().unwrap().contains("\"ovpn\""));
    }

    #[test]
    fn wizard_can_advance_and_step_nav() {
        let mut w = TunnelWizard::default();
        assert!(!w.can_advance(), "provider step needs a provider");
        w.provider = "mullvad".into();
        w.method = "wg".into();
        assert!(w.can_advance());
        assert_eq!(w.next_step(), WizardStep::Config);
        w.step = WizardStep::Config;
        assert!(!w.can_advance(), "config needs key material");
        w.private_key = "k".into();
        w.peer_public_key = "p".into();
        w.address = "a".into();
        w.endpoint = "e".into();
        assert!(w.can_advance());
        w.step = WizardStep::Name;
        assert!(!w.can_advance());
        w.id = "m1".into();
        assert!(w.can_advance());
        assert_eq!(w.prev_step(), WizardStep::Server);
    }

    #[test]
    fn wizard_open_provider_select_and_save_state() {
        let mut panel = VpnPanel::new();
        panel.providers = parse_providers_reply(
            r#"{"ok":true,"providers":[{"id":"mullvad","method":"wg","multi_instance":true,"exit_check":"x"}]}"#,
        );
        let _ = panel.update(Message::WizardOpen);
        assert!(panel.wizard.is_some());
        let _ = panel.update(Message::WizardProviderSelected("mullvad".into()));
        let w = panel.wizard.as_ref().unwrap();
        assert_eq!(w.provider, "mullvad");
        assert_eq!(w.method, "wg");
        assert_eq!(w.exit_check, "x");
        // Cancel closes it.
        let _ = panel.update(Message::WizardCancel);
        assert!(panel.wizard.is_none());
    }

    #[test]
    fn parse_setup_reply_humanises_and_errors() {
        let ok = parse_setup_reply(
            r#"{"ok":true,"id":"m1","secret_distributed":true,"creds_ref":"vpn/mvpn-m1"}"#,
        )
        .unwrap();
        assert!(ok.contains("m1") && ok.contains("distributed"));
        assert!(parse_setup_reply(r#"{"error":"bad key"}"#)
            .unwrap_err()
            .contains("bad key"));
    }

    // ── DDNS-EGRESS-5 ──

    #[test]
    fn parse_ddns_records_joins_config_with_published() {
        let raw = r#"{"ok":true,"enabled":true,"zone":"services.matthewmackes.com","ttl":60,
            "records":[{"fqdn":"eagle-mullvad.services.matthewmackes.com","source":"tunnel:m1",
                        "ip":"1.2.3.4","status":"synced","ttl":60,"updated_ms":9}],
            "config_records":[
                {"name":"{node}-{provider}","source":"tunnel:m1","on_down":"remove"},
                {"name":"{node}-wan","source":"wan","on_down":"keep"}
            ]}"#;
        let t = parse_ddns_records_reply(raw);
        assert!(t.enabled);
        assert_eq!(t.rows.len(), 2);
        // The tunnel record is joined to its published state.
        let m = t.rows.iter().find(|r| r.source == "tunnel:m1").unwrap();
        assert_eq!(m.ip, "1.2.3.4");
        assert_eq!(m.status, "synced");
        assert_eq!(m.name, "{node}-{provider}");
        // The wan record has no published row yet → pending.
        let wan = t.rows.iter().find(|r| r.source == "wan").unwrap();
        assert_eq!(wan.status, "pending");
        assert!(wan.ip.is_empty());
        // published_for resolves the tunnel card's line.
        assert_eq!(
            t.published_for("m1").unwrap().fqdn,
            "eagle-mullvad.services.matthewmackes.com"
        );
        assert!(t.published_for("ghost").is_none());
    }

    #[test]
    fn published_as_line_tone_tracks_status() {
        let palette = crate::live_theme::palette();
        let synced = DdnsRow {
            fqdn: "eagle-mullvad.x".into(),
            ip: "1.2.3.4".into(),
            status: "synced".into(),
            ..Default::default()
        };
        let (line, color) = published_as_line(&synced, palette);
        assert!(line.contains("published as eagle-mullvad.x") && line.contains("1.2.3.4"));
        assert_eq!(color, palette.success);
        let err = DdnsRow {
            fqdn: "x".into(),
            status: "error".into(),
            ..Default::default()
        };
        assert_eq!(published_as_line(&err, palette).1, palette.danger);
        assert_eq!(ddns_status_color("synced", palette), palette.success);
        assert_eq!(ddns_status_color("error", palette), palette.danger);
    }

    #[test]
    fn ddns_form_and_op_state_machine() {
        let mut panel = VpnPanel::new();
        let _ = panel.update(Message::DdnsAddOpen);
        assert!(panel.ddns_form.is_some());
        let _ = panel.update(Message::DdnsFormName("{node}-{provider}".into()));
        let _ = panel.update(Message::DdnsFormSource("tunnel:m1".into()));
        let _ = panel.update(Message::DdnsFormOnDown("sentinel".into()));
        // Submit closes the form (and fires the add-record task).
        let _ = panel.update(Message::DdnsAddSubmit);
        assert!(panel.ddns_form.is_none());
        // A finished op records status.
        let _ = panel.update(Message::DdnsOpFinished(Ok("DDNS record added.".into())));
        assert!(panel.status.contains("added"));
    }

    #[test]
    fn parse_ddns_op_reply_humanises() {
        assert_eq!(
            parse_ddns_op_reply("sync-now", r#"{"ok":true}"#).unwrap(),
            "DDNS sync requested."
        );
        assert!(parse_ddns_op_reply("add-record", r#"{"error":"x"}"#).is_err());
    }
}
