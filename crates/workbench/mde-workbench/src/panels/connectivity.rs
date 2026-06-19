//! CONNECT-6 — Network → Connectivity panel (the exposure matrix).
//!
//! The operator-facing overview of the unified connectivity / exposure model
//! (design: `docs/design/connect.md`). Renders every per-service exposure policy
//! as a row: service id · source node/kind · overlay port/proto · tier (mesh-only
//! vs public-via-ingress) · ingress lighthouse + public hostname. Below the
//! configured matrix it lists auto-discovered **candidates** (this node's
//! listening ports) so the operator sees what *could* be exposed but isn't yet —
//! the opt-in surface CONNECT-7's wizard will drive.
//!
//! Reads two Bus verbs from the connect responder (`crates/mesh/mackesd/src/ipc/
//! connect.rs`):
//!   * `action/connect/list-services`   → `{ ok, services: [ExposurePolicy] }`
//!   * `action/connect/list-candidates` → `{ ok, candidates: [{id,node,kind,
//!     port,proto,label,configured}] }`
//! via the generic [`crate::dbus::action_request`] client (run off the iced
//! executor in `spawn_blocking`, per its current-thread-runtime contract).
//!
//! Read-only this iteration — the matrix + candidate list. Mutation (expose /
//! unexpose) is CONNECT-7's wizard. Carbon tokens only (§4); the public-tier
//! pill uses the warning token to read as "wider surface".

use std::time::{Duration, SystemTime};

use cosmic::iced::widget::{
    button, column, container, pick_list, row, scrollable, text, text_input, Space,
};
use cosmic::iced::{Background, Border, Color, Length, Padding, Task};
use cosmic::{Element, Theme};
use mde_theme::{FontSize, Palette, TypeRole};
use serde::Deserialize;
use serde_json::json;

use crate::controls::{variant_button, ButtonVariant};
use crate::cosmic_compat::prelude::*;

/// Read budget for the connect Bus probes — matches the other panels' 2 s.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Write budget for the expose/unexpose mutations — they only rewrite the
/// policy TOML on the shared substrate (the worker reconciles Caddy/firewalld
/// on its own tick), so the same 2 s read budget is generous.
const APPLY_TIMEOUT: Duration = Duration::from_secs(3);

/// Tier choices for the Expose wizard's pick_list (wire values).
const TIER_CHOICES: [&str; 2] = ["mesh-only", "public-via-ingress"];
/// Source-kind choices.
const KIND_CHOICES: [&str; 3] = ["host", "vm", "container"];
/// Protocol-mode choices (how the ingress carries a public service).
const MODE_CHOICES: [&str; 3] = ["http", "tcp", "udp"];

/// One configured exposure policy row (subset of `exposure::ExposurePolicy` —
/// the fields the matrix renders). Decoded from `list-services`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ServiceRow {
    /// Stable service id.
    pub id: String,
    /// The hosting node + endpoint.
    #[serde(default)]
    pub source: Source,
    /// "mesh-only" | "public-via-ingress" (kebab-case from the wire).
    #[serde(default)]
    pub tier: String,
    /// The ingress binding when public.
    #[serde(default)]
    pub ingress: Option<Ingress>,
    /// "http" | "tcp" | "udp" — only meaningful when public.
    #[serde(default)]
    pub mode: String,
    /// Group template this policy was applied from, if any.
    #[serde(default)]
    pub template: Option<String>,
}

/// Source endpoint of a configured service.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Source {
    #[serde(default)]
    pub node: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub proto: String,
}

/// Ingress binding of a public service.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Ingress {
    #[serde(default)]
    pub lighthouse: String,
    #[serde(default)]
    pub hostname: String,
}

/// One auto-discovered candidate (a listening port not yet a policy, or one that
/// is). Decoded from `list-candidates`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CandidateRow {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub node: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub proto: String,
    /// Friendly well-known label (e.g. "SSH"), if recognised.
    #[serde(default)]
    pub label: Option<String>,
    /// True when this candidate already has an exposure policy.
    #[serde(default)]
    pub configured: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ConnectivityPanel {
    pub services: Vec<ServiceRow>,
    pub candidates: Vec<CandidateRow>,
    pub busy: bool,
    pub last_run_at: Option<SystemTime>,
    /// Last operator-facing line — a summary or the failure mode.
    pub last_op: String,
    // ── CONNECT-7 — Expose wizard form state ───────────────────────
    /// Service id to create/expose.
    pub form_id: String,
    /// Hosting node (defaults to the candidate's node on prefill).
    pub form_node: String,
    /// Service port (text input → parsed u16 on apply).
    pub form_port: String,
    /// L4 protocol the service listens on (`tcp`/`udp`).
    pub form_proto: String,
    /// host / vm / container.
    pub form_kind: String,
    /// mesh-only / public-via-ingress.
    pub form_tier: String,
    /// Ingress lighthouse (public only).
    pub form_lighthouse: String,
    /// Public DDNS hostname (public only).
    pub form_hostname: String,
    /// Ingress protocol mode (http/tcp/udp, public only).
    pub form_mode: String,
    /// True while an expose/unexpose mutation is in flight.
    pub applying: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded {
        services: Vec<ServiceRow>,
        candidates: Vec<CandidateRow>,
        error: Option<String>,
    },
    RefreshClicked,
    // ── CONNECT-7 — Expose wizard ──────────────────────────────────
    FormIdChanged(String),
    FormNodeChanged(String),
    FormPortChanged(String),
    FormKindSelected(String),
    FormTierSelected(String),
    FormLighthouseChanged(String),
    FormHostnameChanged(String),
    FormModeSelected(String),
    /// Prefill the form from a discovered candidate or an existing row
    /// (defaults the tier to public-via-ingress — the usual reason to prefill).
    Prefill {
        id: String,
        node: String,
        kind: String,
        port: u16,
        proto: String,
    },
    /// Validate + apply the form (set-policy, then expose when public).
    ExposeClicked,
    /// Revert a published service to mesh-only.
    UnexposeClicked(String),
    /// A mutation completed (Ok message or Err diagnostic) — triggers reload.
    Applied(Result<String, String>),
}

impl ConnectivityPanel {
    #[must_use]
    pub fn new() -> Self {
        Self {
            form_kind: "host".into(),
            form_proto: "tcp".into(),
            form_tier: "mesh-only".into(),
            form_mode: "http".into(),
            ..Self::default()
        }
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async {
                tokio::task::spawn_blocking(fetch)
                    .await
                    .unwrap_or_else(|_| {
                        (
                            Vec::new(),
                            Vec::new(),
                            Some("connectivity probe task panicked".into()),
                        )
                    })
            },
            |(services, candidates, error)| {
                crate::Message::Connectivity(Message::Loaded {
                    services,
                    candidates,
                    error,
                })
            },
        )
    }

    pub fn update(&mut self, msg: Message) -> Task<crate::Message> {
        match msg {
            Message::Loaded {
                services,
                candidates,
                error,
            } => {
                self.services = services;
                self.candidates = candidates;
                self.busy = false;
                self.last_run_at = Some(SystemTime::now());
                self.last_op = error.unwrap_or_else(|| {
                    let public = self
                        .services
                        .iter()
                        .filter(|s| s.tier == "public-via-ingress")
                        .count();
                    let unconfigured = self.candidates.iter().filter(|c| !c.configured).count();
                    format!(
                        "{} service(s) · {public} public · {unconfigured} discoverable candidate(s)",
                        self.services.len()
                    )
                });
                Task::none()
            }
            Message::RefreshClicked => {
                self.busy = true;
                self.last_op = "refreshing…".into();
                Self::load()
            }
            Message::FormIdChanged(v) => {
                self.form_id = v;
                Task::none()
            }
            Message::FormNodeChanged(v) => {
                self.form_node = v;
                Task::none()
            }
            Message::FormPortChanged(v) => {
                // Keep digits only so the parse on apply can't surprise the operator.
                self.form_port = v.chars().filter(char::is_ascii_digit).collect();
                Task::none()
            }
            Message::FormKindSelected(v) => {
                self.form_kind = v;
                Task::none()
            }
            Message::FormTierSelected(v) => {
                self.form_tier = v;
                Task::none()
            }
            Message::FormLighthouseChanged(v) => {
                self.form_lighthouse = v;
                Task::none()
            }
            Message::FormHostnameChanged(v) => {
                self.form_hostname = v;
                Task::none()
            }
            Message::FormModeSelected(v) => {
                self.form_mode = v;
                Task::none()
            }
            Message::Prefill {
                id,
                node,
                kind,
                port,
                proto,
            } => {
                self.form_id = id;
                self.form_node = node;
                self.form_kind = if kind.is_empty() { "host".into() } else { kind };
                self.form_port = port.to_string();
                self.form_proto = if proto.is_empty() {
                    "tcp".into()
                } else {
                    proto
                };
                self.form_tier = "public-via-ingress".into();
                self.last_op = format!(
                    "form prefilled for '{}' — set the ingress + Expose",
                    self.form_id
                );
                Task::none()
            }
            Message::ExposeClicked => {
                if self.applying {
                    return Task::none();
                }
                let form = match self.validated_form() {
                    Ok(f) => f,
                    Err(e) => {
                        self.last_op = e;
                        return Task::none();
                    }
                };
                self.applying = true;
                self.last_op = format!("applying '{}'…", form.id);
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || apply_expose(&form))
                            .await
                            .unwrap_or_else(|_| Err("apply task panicked".into()))
                    },
                    |res| crate::Message::Connectivity(Message::Applied(res)),
                )
            }
            Message::UnexposeClicked(id) => {
                if self.applying {
                    return Task::none();
                }
                self.applying = true;
                self.last_op = format!("unexposing '{id}'…");
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || apply_unexpose(&id))
                            .await
                            .unwrap_or_else(|_| Err("unexpose task panicked".into()))
                    },
                    |res| crate::Message::Connectivity(Message::Applied(res)),
                )
            }
            Message::Applied(res) => {
                self.applying = false;
                match res {
                    Ok(msg) => {
                        self.last_op = msg;
                        // Reload the matrix so the new policy shows immediately.
                        self.busy = true;
                        Self::load()
                    }
                    Err(e) => {
                        self.last_op = e;
                        Task::none()
                    }
                }
            }
        }
    }

    /// Validate the wizard form into an [`ExposeForm`] ready to apply, or a
    /// human-readable reason. Pure (no I/O) so the update arm stays thin + it's
    /// directly testable.
    fn validated_form(&self) -> Result<ExposeForm, String> {
        let id = self.form_id.trim();
        if id.is_empty() {
            return Err("Expose: a service id is required".into());
        }
        let node = self.form_node.trim();
        if node.is_empty() {
            return Err("Expose: a source node is required".into());
        }
        let port: u16 = match self.form_port.trim().parse() {
            Ok(p) if p > 0 => p,
            _ => return Err("Expose: port must be a number 1–65535".into()),
        };
        let public = self.form_tier == "public-via-ingress";
        if public
            && (self.form_lighthouse.trim().is_empty() || self.form_hostname.trim().is_empty())
        {
            return Err("Expose: a public service needs an ingress lighthouse + hostname".into());
        }
        Ok(ExposeForm {
            id: id.to_string(),
            node: node.to_string(),
            kind: self.form_kind.clone(),
            port,
            proto: self.form_proto.clone(),
            public,
            lighthouse: self.form_lighthouse.trim().to_string(),
            hostname: self.form_hostname.trim().to_string(),
            mode: self.form_mode.clone(),
        })
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let sizes = FontSize::defaults();

        let title = text("Connectivity")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());

        let subtitle_text = if !self.last_op.is_empty() {
            self.last_op.clone()
        } else {
            "exposure matrix — which services are mesh-only vs published to the public".into()
        };
        let subtitle = text(subtitle_text)
            .size(TypeRole::Body.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color());

        let refresh_btn = button(
            text(if self.busy { "Working…" } else { "Refresh" })
                .size(13)
                .colr(Color::WHITE),
        )
        .padding(Padding::from([6u16, 14u16]))
        .sty({
            let accent = palette.accent.into_cosmic_color();
            move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
                let bg = match status {
                    cosmic::iced::widget::button::Status::Hovered => Color {
                        r: accent.r * 1.10,
                        g: accent.g * 1.10,
                        b: accent.b * 1.10,
                        a: accent.a,
                    },
                    _ => accent,
                };
                cosmic::iced::widget::button::Style {
                    snap: false,
                    background: Some(Background::Color(bg)),
                    text_color: Color::WHITE,
                    border: Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: 6.0.into(),
                    },
                    shadow: cosmic::iced::Shadow::default(),
                    ..cosmic::iced::widget::button::Style::default()
                }
            }
        })
        .on_press(crate::Message::Connectivity(Message::RefreshClicked));

        let header = row![
            column![title, subtitle].spacing(2),
            Space::new().width(Length::Fill),
            refresh_btn,
        ]
        .align_y(cosmic::iced::alignment::Vertical::Center);

        // ── Exposure matrix (configured services) ──────────────────
        let services_section: Element<'_, crate::Message> = if self.services.is_empty() {
            empty_services(palette)
        } else {
            let mut col = column![section_label("Exposure matrix", palette)].spacing(6);
            for s in &self.services {
                col = col.push(service_row_view(s, palette));
            }
            col.into()
        };

        // ── Discovered candidates (this node's listening ports) ────
        let candidates_section: Element<'_, crate::Message> = if self.candidates.is_empty() {
            column![].into()
        } else {
            let mut col = column![
                Space::new().height(Length::Fixed(18.0)),
                section_label("Discovered candidates (this node)", palette),
            ]
            .spacing(6);
            for c in &self.candidates {
                col = col.push(candidate_row_view(c, palette));
            }
            col.into()
        };

        let wizard = self.wizard_view(palette);

        let body = scrollable(
            column![
                services_section,
                candidates_section,
                Space::new().height(Length::Fixed(18.0)),
                wizard,
            ]
            .spacing(2),
        )
        .height(Length::FillPortion(1));

        container(column![header, Space::new().height(Length::Fixed(20.0)), body,].spacing(2))
            .padding(Padding::from([24u16, 32u16]))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// CONNECT-7 — the Expose wizard form (service id + source + tier → ingress
    /// binding → Apply). The ingress fields only show when the tier is public.
    fn wizard_view(&self, palette: Palette) -> Element<'_, crate::Message> {
        let label = |t: &str| {
            text(t.to_string())
                .size(12)
                .colr(palette.text_muted.into_cosmic_color())
                .width(Length::Fixed(110.0))
        };

        let id_row = row![
            label("Service id"),
            text_input("e.g. grafana", &self.form_id)
                .on_input(|v| crate::Message::Connectivity(Message::FormIdChanged(v))),
        ]
        .spacing(12)
        .align_y(cosmic::iced::alignment::Vertical::Center);

        let source_row = row![
            label("Source"),
            text_input("node", &self.form_node)
                .on_input(|v| crate::Message::Connectivity(Message::FormNodeChanged(v)))
                .width(Length::FillPortion(2)),
            text_input("port", &self.form_port)
                .on_input(|v| crate::Message::Connectivity(Message::FormPortChanged(v)))
                .width(Length::FillPortion(1)),
            pick_list(
                KIND_CHOICES.map(String::from).to_vec(),
                Some(self.form_kind.clone()),
                |v| crate::Message::Connectivity(Message::FormKindSelected(v)),
            ),
        ]
        .spacing(12)
        .align_y(cosmic::iced::alignment::Vertical::Center);

        let tier_row = row![
            label("Tier"),
            pick_list(
                TIER_CHOICES.map(String::from).to_vec(),
                Some(self.form_tier.clone()),
                |v| crate::Message::Connectivity(Message::FormTierSelected(v)),
            ),
        ]
        .spacing(12)
        .align_y(cosmic::iced::alignment::Vertical::Center);

        let is_public = self.form_tier == "public-via-ingress";
        let mut form = column![id_row, source_row, tier_row].spacing(10);

        if is_public {
            let ingress_row = row![
                label("Ingress"),
                text_input("lighthouse (e.g. Lighthouse-01)", &self.form_lighthouse)
                    .on_input(|v| crate::Message::Connectivity(Message::FormLighthouseChanged(v)))
                    .width(Length::FillPortion(2)),
                text_input("public hostname", &self.form_hostname)
                    .on_input(|v| crate::Message::Connectivity(Message::FormHostnameChanged(v)))
                    .width(Length::FillPortion(2)),
                pick_list(
                    MODE_CHOICES.map(String::from).to_vec(),
                    Some(self.form_mode.clone()),
                    |v| crate::Message::Connectivity(Message::FormModeSelected(v)),
                ),
            ]
            .spacing(12)
            .align_y(cosmic::iced::alignment::Vertical::Center);
            form = form.push(ingress_row);
        }

        // Live preview of what applying will render (Caddy / firewall / overlay).
        let preview = expose_preview(
            &self.form_id,
            &self.form_node,
            &self.form_port,
            is_public,
            &self.form_lighthouse,
            &self.form_hostname,
            &self.form_mode,
        );
        let preview_box = container(
            text(preview)
                .size(12)
                .colr(palette.text_muted.into_cosmic_color()),
        )
        .padding(Padding::from([10u16, 14u16]))
        .width(Length::Fill)
        .style(move |_| container::Style {
            snap: false,
            background: Some(Background::Color(palette.surface.into_cosmic_color())),
            border: Border {
                color: palette.border.into_cosmic_color(),
                width: 1.0,
                radius: 5.0.into(),
            },
            ..container::Style::default()
        });

        let apply_btn = variant_button(
            if self.applying {
                "Applying…"
            } else {
                "Apply"
            },
            ButtonVariant::Primary,
            (!self.applying).then_some(crate::Message::Connectivity(Message::ExposeClicked)),
            palette,
        );

        container(
            column![
                section_label("Expose a service", palette),
                text(
                    "Create or publish a service. Mesh-only keeps it on the overlay; \
                     public-via-ingress renders a lighthouse reverse-proxy + firewall \
                     opening. Writing the policy is the single action — the connectivity \
                     worker reconciles Caddy + firewalld from it."
                )
                .size(11)
                .colr(palette.text_muted.into_cosmic_color()),
                Space::new().height(Length::Fixed(6.0)),
                form,
                preview_box,
                row![Space::new().width(Length::Fill), apply_btn],
            ]
            .spacing(10),
        )
        .padding(Padding::from([16u16, 18u16]))
        .width(Length::Fill)
        .style(move |_| container::Style {
            snap: false,
            background: Some(Background::Color(palette.raised.into_cosmic_color())),
            border: Border {
                color: palette.border.into_cosmic_color(),
                width: 1.0,
                radius: 6.0.into(),
            },
            ..container::Style::default()
        })
        .into()
    }
}

fn section_label<'a>(label: &str, palette: Palette) -> Element<'a, crate::Message> {
    text(label.to_string())
        .size(11)
        .colr(palette.text_muted.into_cosmic_color())
        .into()
}

fn empty_services<'a>(palette: Palette) -> Element<'a, crate::Message> {
    container(
        column![
            text("No exposure policies configured")
                .size(13)
                .colr(palette.text.into_cosmic_color()),
            Space::new().height(Length::Fixed(6.0)),
            text(
                "Every service defaults to mesh-only (reachable on the Nebula \
                 overlay, never the public internet). Discovered candidates below \
                 list this node's listening ports — opt one in to publish it \
                 through a lighthouse ingress. Empty here means mackesd isn't \
                 reachable on the Bus, or nothing has been exposed yet."
            )
            .size(12)
            .colr(palette.text_muted.into_cosmic_color()),
        ]
        .spacing(2),
    )
    .padding(Padding::from([18u16, 22u16]))
    .width(Length::Fill)
    .style(move |_| container::Style {
        snap: false,
        background: Some(Background::Color(palette.raised.into_cosmic_color())),
        border: Border {
            color: palette.border.into_cosmic_color(),
            width: 1.0,
            radius: 6.0.into(),
        },
        ..container::Style::default()
    })
    .into()
}

fn pill<'a>(label: String, bg: Color) -> Element<'a, crate::Message> {
    container(text(label).size(10).colr(Color::WHITE))
        .padding(Padding::from([2u16, 8u16]))
        .style(move |_| container::Style {
            snap: false,
            background: Some(Background::Color(bg)),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 10.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

fn service_row_view<'a>(s: &ServiceRow, palette: Palette) -> Element<'a, crate::Message> {
    let is_public = s.tier == "public-via-ingress";
    let (tier_label, tier_color) = if is_public {
        // A public service widens the surface → the warning token.
        ("Public", palette.warning.into_cosmic_color())
    } else {
        ("Mesh-only", palette.accent.into_cosmic_color())
    };

    // The ingress column: "<hostname> via <lighthouse> (<mode>)" when public, else —.
    let ingress_text = if is_public {
        match &s.ingress {
            Some(b) => format!(
                "{} via {}{}",
                b.hostname,
                b.lighthouse,
                if s.mode.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", s.mode)
                }
            ),
            None => "(public, binding missing)".to_string(),
        }
    } else {
        "—".to_string()
    };

    let kind = if s.source.kind.is_empty() {
        "host".to_string()
    } else {
        s.source.kind.clone()
    };
    let node_kind = if s.source.node.is_empty() {
        kind
    } else {
        format!("{} · {kind}", s.source.node)
    };
    let proto = if s.source.proto.is_empty() {
        "tcp"
    } else {
        s.source.proto.as_str()
    };
    let port_proto = format!("{}/{proto}", s.source.port);

    let id_sub = match &s.template {
        Some(t) if !t.is_empty() => format!("id: {} · template: {t}", s.id),
        _ => format!("id: {}", s.id),
    };

    // Public rows get an Unexpose (→ mesh-only); mesh-only rows get an Expose
    // that prefills the wizard from this policy's source.
    let action_btn = if is_public {
        variant_button(
            "Unexpose",
            ButtonVariant::Secondary,
            Some(crate::Message::Connectivity(Message::UnexposeClicked(
                s.id.clone(),
            ))),
            palette,
        )
    } else {
        variant_button(
            "Expose",
            ButtonVariant::Ghost,
            Some(crate::Message::Connectivity(Message::Prefill {
                id: s.id.clone(),
                node: s.source.node.clone(),
                kind: if s.source.kind.is_empty() {
                    "host".into()
                } else {
                    s.source.kind.clone()
                },
                port: s.source.port,
                proto: if s.source.proto.is_empty() {
                    "tcp".into()
                } else {
                    s.source.proto.clone()
                },
            })),
            palette,
        )
    };

    let bg = palette.raised.into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    container(
        row![
            column![
                text(s.id.clone())
                    .size(13)
                    .colr(palette.text.into_cosmic_color()),
                text(id_sub)
                    .size(10)
                    .colr(palette.text_muted.into_cosmic_color()),
            ]
            .spacing(2)
            .width(Length::FillPortion(3)),
            text(node_kind)
                .size(12)
                .colr(palette.text.into_cosmic_color())
                .width(Length::FillPortion(2)),
            text(port_proto)
                .size(12)
                .colr(palette.text.into_cosmic_color())
                .width(Length::FillPortion(1)),
            text(ingress_text)
                .size(12)
                .colr(palette.text_muted.into_cosmic_color())
                .width(Length::FillPortion(3)),
            pill(tier_label.to_string(), tier_color),
            action_btn,
        ]
        .spacing(12)
        .align_y(cosmic::iced::alignment::Vertical::Center),
    )
    .padding(Padding::from([10u16, 16u16]))
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

fn candidate_row_view<'a>(c: &CandidateRow, palette: Palette) -> Element<'a, crate::Message> {
    let (pill_label, pill_color) = if c.configured {
        ("Configured", palette.accent.into_cosmic_color())
    } else {
        ("Available", palette.text_muted.into_cosmic_color())
    };

    let name = c
        .label
        .clone()
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| format!("port {}", c.port));
    let proto = if c.proto.is_empty() {
        "tcp"
    } else {
        c.proto.as_str()
    };
    let kind = if c.kind.is_empty() {
        "host"
    } else {
        c.kind.as_str()
    };
    let node_kind = if c.node.is_empty() {
        kind.to_string()
    } else {
        format!("{} · {kind}", c.node)
    };

    // An "Expose" button prefills the wizard from this candidate (id/node/port).
    let expose_btn = variant_button(
        "Expose",
        ButtonVariant::Ghost,
        Some(crate::Message::Connectivity(Message::Prefill {
            id: c.id.clone(),
            node: c.node.clone(),
            kind: kind.to_string(),
            port: c.port,
            proto: proto.to_string(),
        })),
        palette,
    );

    let bg = palette.surface.into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    container(
        row![
            column![
                text(name).size(12).colr(palette.text.into_cosmic_color()),
                text(format!("id: {}", c.id))
                    .size(10)
                    .colr(palette.text_muted.into_cosmic_color()),
            ]
            .spacing(2)
            .width(Length::FillPortion(3)),
            text(node_kind)
                .size(12)
                .colr(palette.text_muted.into_cosmic_color())
                .width(Length::FillPortion(2)),
            text(format!("{}/{proto}", c.port))
                .size(12)
                .colr(palette.text.into_cosmic_color())
                .width(Length::FillPortion(1)),
            container(expose_btn).width(Length::FillPortion(3)),
            pill(pill_label.to_string(), pill_color),
        ]
        .spacing(12)
        .align_y(cosmic::iced::alignment::Vertical::Center),
    )
    .padding(Padding::from([8u16, 16u16]))
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

// ---- Expose wizard (CONNECT-7) --------------------------------

/// A validated Expose-wizard submission, ready to apply over the Bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExposeForm {
    pub id: String,
    pub node: String,
    pub kind: String,
    pub port: u16,
    pub proto: String,
    pub public: bool,
    pub lighthouse: String,
    pub hostname: String,
    pub mode: String,
}

/// A human-readable preview of what applying the form will render — the one-action
/// "wires proxy + firewall + DNS" summary (CONNECT-7 acceptance). Pure + testable;
/// mirrors `exposure::render_caddyfile` / `connect_firewall::desired_*` shapes.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn expose_preview(
    id: &str,
    node: &str,
    port: &str,
    public: bool,
    lighthouse: &str,
    hostname: &str,
    mode: &str,
) -> String {
    let id = if id.trim().is_empty() {
        "<id>"
    } else {
        id.trim()
    };
    let node = if node.trim().is_empty() {
        "<node>"
    } else {
        node.trim()
    };
    let port = if port.trim().is_empty() {
        "<port>"
    } else {
        port.trim()
    };
    if !public {
        return format!(
            "Mesh-only: '{id}' reachable at {node}.mesh:{port} over the Nebula \
             overlay. No public surface, no firewall change."
        );
    }
    let lh = if lighthouse.trim().is_empty() {
        "<lighthouse>"
    } else {
        lighthouse.trim()
    };
    let host = if hostname.trim().is_empty() {
        "<hostname>"
    } else {
        hostname.trim()
    };
    match mode {
        "http" => format!(
            "Public (HTTP) via {lh}: Caddy auto-HTTPS site `{host}` → reverse_proxy \
             {node}.mesh:{port}; firewalld opens 80/443 on {lh}. Let's Encrypt issues \
             the cert. No ingress auth — {id} handles its own."
        ),
        other => format!(
            "Public ({other}) via {lh}: firewalld forwards {port}/{} on {lh} → \
             {node} overlay IP:{port} (raw stream, no TLS). No ingress auth — {id} \
             handles its own.",
            if other == "udp" { "udp" } else { "tcp" }
        ),
    }
}

/// True when a connect reply is an `{"error": "..."}` envelope; returns the
/// message. `None` for an `{"ok":true,...}` reply.
fn reply_error(raw: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(raw.trim()).ok()?;
    v.get("error")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// Apply the Expose wizard: `set-policy` to create/update the (mesh-only) service
/// source, then — when public — `expose` to flip it public with the ingress
/// binding. Writing the policy is the single durable action; the
/// `connect_firewall` worker reconciles Caddy + firewalld from it. Blocking
/// (Bus client builds its own runtime) — call from `spawn_blocking`.
fn apply_expose(form: &ExposeForm) -> Result<String, String> {
    // 1. set-policy — create/update the source as mesh-only first (a public
    //    policy must already exist before `expose` can flip it).
    let policy = json!({
        "id": form.id,
        "source": { "node": form.node, "kind": form.kind, "port": form.port, "proto": form.proto },
        "tier": "mesh-only",
    });
    let r1 = crate::dbus::action_request_with_body(
        "action/connect/set-policy",
        Some(&policy.to_string()),
        APPLY_TIMEOUT,
    )
    .ok_or("mackesd not reachable over the Bus (set-policy)")?;
    if let Some(e) = reply_error(&r1) {
        return Err(format!("set-policy failed: {e}"));
    }
    if !form.public {
        return Ok(format!("Saved '{}' (mesh-only).", form.id));
    }
    // 2. expose — flip public with the ingress binding + mode.
    let body = json!({
        "id": form.id,
        "lighthouse": form.lighthouse,
        "hostname": form.hostname,
        "mode": form.mode,
    });
    let r2 = crate::dbus::action_request_with_body(
        "action/connect/expose",
        Some(&body.to_string()),
        APPLY_TIMEOUT,
    )
    .ok_or("mackesd not reachable over the Bus (expose)")?;
    if let Some(e) = reply_error(&r2) {
        return Err(format!("expose failed: {e}"));
    }
    Ok(format!(
        "Exposed '{}' → {} via {} ({}).",
        form.id, form.hostname, form.lighthouse, form.mode
    ))
}

/// Revert a published service to mesh-only over the Bus (`unexpose`). Blocking —
/// call from `spawn_blocking`.
fn apply_unexpose(id: &str) -> Result<String, String> {
    let r =
        crate::dbus::action_request_with_body("action/connect/unexpose", Some(id), APPLY_TIMEOUT)
            .ok_or("mackesd not reachable over the Bus (unexpose)")?;
    if let Some(e) = reply_error(&r) {
        return Err(format!("unexpose failed: {e}"));
    }
    Ok(format!("Unexposed '{id}' (back to mesh-only)."))
}

// ---- I/O ------------------------------------------------------

/// Probe both connect verbs over the Bus and decode them. Blocking (builds its
/// own current-thread runtime via [`crate::dbus::action_request`]) — call from
/// `spawn_blocking`, never on the iced executor.
#[must_use]
fn fetch() -> (Vec<ServiceRow>, Vec<CandidateRow>, Option<String>) {
    let services_json = crate::dbus::action_request("action/connect/list-services", PROBE_TIMEOUT);
    let candidates_json =
        crate::dbus::action_request("action/connect/list-candidates", PROBE_TIMEOUT);
    match services_json {
        Some(sj) => {
            let (services, serr) = parse_services(&sj);
            let candidates = candidates_json
                .as_deref()
                .map(|cj| parse_candidates(cj).0)
                .unwrap_or_default();
            (services, candidates, serr)
        }
        None => (
            Vec::new(),
            Vec::new(),
            Some("mackesd not reachable over the Bus — connectivity unavailable".into()),
        ),
    }
}

/// Parse the `list-services` reply envelope `{ ok, services: [...] }`. Pulled out
/// for direct testing. An `{"error":...}` envelope surfaces as the error string.
#[must_use]
pub fn parse_services(raw: &str) -> (Vec<ServiceRow>, Option<String>) {
    parse_envelope(raw, "services")
}

/// Parse the `list-candidates` reply envelope `{ ok, candidates: [...] }`.
#[must_use]
pub fn parse_candidates(raw: &str) -> (Vec<CandidateRow>, Option<String>) {
    parse_envelope(raw, "candidates")
}

/// Shared envelope decoder: pull the `key` array out of the connect reply, or
/// surface its `{"error":...}` message.
fn parse_envelope<T: for<'de> Deserialize<'de>>(raw: &str, key: &str) -> (Vec<T>, Option<String>) {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return (Vec::new(), Some(format!("empty reply for {key}")));
    }
    let v: serde_json::Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(e) => return (Vec::new(), Some(format!("invalid JSON: {e}"))),
    };
    if let Some(msg) = v.get("error").and_then(serde_json::Value::as_str) {
        return (Vec::new(), Some(msg.to_string()));
    }
    match v.get(key) {
        Some(arr) => match serde_json::from_value::<Vec<T>>(arr.clone()) {
            Ok(rows) => (rows, None),
            Err(e) => (Vec::new(), Some(format!("bad {key} shape: {e}"))),
        },
        None => (Vec::new(), Some(format!("reply missing '{key}'"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_services_decodes_the_connect_envelope() {
        // The exact shape mackesd's `action/connect/list-services` emits.
        let raw = r#"{"ok":true,"services":[
            {"id":"grafana","source":{"node":"eagle","kind":"container","port":3000,"proto":"tcp"},
             "tier":"public-via-ingress",
             "ingress":{"lighthouse":"Lighthouse-01","hostname":"grafana.services.example"},
             "mode":"http","template":"web-apps"},
            {"id":"db","source":{"node":"eagle","kind":"host","port":5432,"proto":"tcp"},
             "tier":"mesh-only"}
        ]}"#;
        let (rows, err) = parse_services(raw);
        assert!(err.is_none(), "expected no error, got {err:?}");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "grafana");
        assert_eq!(rows[0].tier, "public-via-ingress");
        assert_eq!(rows[0].source.port, 3000);
        assert_eq!(rows[0].source.kind, "container");
        assert_eq!(
            rows[0].ingress.as_ref().unwrap().hostname,
            "grafana.services.example"
        );
        assert_eq!(rows[0].template.as_deref(), Some("web-apps"));
        assert_eq!(rows[1].tier, "mesh-only");
        assert!(rows[1].ingress.is_none());
    }

    #[test]
    fn parse_candidates_decodes_and_marks_configured() {
        let raw = r#"{"ok":true,"candidates":[
            {"id":"eagle-22","node":"eagle","kind":"host","port":22,"proto":"tcp",
             "label":"SSH","configured":false},
            {"id":"eagle-3000","node":"eagle","kind":"host","port":3000,"proto":"tcp",
             "label":null,"configured":true}
        ]}"#;
        let (rows, err) = parse_candidates(raw);
        assert!(err.is_none(), "{err:?}");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].label.as_deref(), Some("SSH"));
        assert!(!rows[0].configured);
        assert!(rows[1].label.is_none());
        assert!(rows[1].configured);
    }

    #[test]
    fn parse_surfaces_error_envelope() {
        let (rows, err) = parse_services(r#"{"error":"boom"}"#);
        assert!(rows.is_empty());
        assert_eq!(err.as_deref(), Some("boom"));
    }

    #[test]
    fn parse_empty_and_garbage() {
        let (r1, e1) = parse_services("");
        assert!(r1.is_empty() && e1.is_some());
        let (r2, e2) = parse_services("{not json");
        assert!(r2.is_empty() && e2.unwrap().contains("invalid JSON"));
        // ok envelope but missing key.
        let (r3, e3) = parse_services(r#"{"ok":true}"#);
        assert!(r3.is_empty() && e3.unwrap().contains("missing 'services'"));
    }

    #[test]
    fn empty_services_array_is_not_an_error() {
        let (rows, err) = parse_services(r#"{"ok":true,"services":[]}"#);
        assert!(rows.is_empty());
        assert!(err.is_none());
    }

    #[test]
    fn view_renders_empty_without_panic() {
        let p = ConnectivityPanel::new();
        let _ = p.view();
    }

    #[test]
    fn view_renders_with_rows_without_panic() {
        let mut p = ConnectivityPanel::new();
        let (services, _) = parse_services(
            r#"{"ok":true,"services":[
                {"id":"grafana","source":{"node":"eagle","kind":"container","port":3000,"proto":"tcp"},
                 "tier":"public-via-ingress",
                 "ingress":{"lighthouse":"LH-01","hostname":"g.example"},"mode":"http"},
                {"id":"db","source":{"node":"eagle","kind":"host","port":5432,"proto":"tcp"},
                 "tier":"mesh-only"}
            ]}"#,
        );
        let (candidates, _) = parse_candidates(
            r#"{"ok":true,"candidates":[
                {"id":"eagle-22","node":"eagle","kind":"host","port":22,"proto":"tcp",
                 "label":"SSH","configured":false}
            ]}"#,
        );
        p.services = services;
        p.candidates = candidates;
        let _ = p.view();
    }

    #[test]
    fn update_loaded_summarises_public_and_candidates() {
        let mut p = ConnectivityPanel::new();
        p.busy = true;
        let (services, _) = parse_services(
            r#"{"ok":true,"services":[
                {"id":"a","source":{"node":"n","kind":"host","port":80,"proto":"tcp"},
                 "tier":"public-via-ingress","ingress":{"lighthouse":"l","hostname":"h"},"mode":"http"}
            ]}"#,
        );
        let (candidates, _) = parse_candidates(
            r#"{"ok":true,"candidates":[
                {"id":"n-22","node":"n","kind":"host","port":22,"proto":"tcp","configured":false},
                {"id":"n-80","node":"n","kind":"host","port":80,"proto":"tcp","configured":true}
            ]}"#,
        );
        let _ = p.update(Message::Loaded {
            services,
            candidates,
            error: None,
        });
        assert!(!p.busy);
        assert!(p.last_op.contains("1 service"));
        assert!(p.last_op.contains("1 public"));
        assert!(p.last_op.contains("1 discoverable"));
    }

    #[test]
    fn update_loaded_with_error_surfaces_message() {
        let mut p = ConnectivityPanel::new();
        let _ = p.update(Message::Loaded {
            services: Vec::new(),
            candidates: Vec::new(),
            error: Some("mackesd not reachable over the Bus".into()),
        });
        assert_eq!(p.last_op, "mackesd not reachable over the Bus");
    }

    // ── CONNECT-7 — Expose wizard ──────────────────────────────────

    #[test]
    fn new_seeds_sensible_form_defaults() {
        let p = ConnectivityPanel::new();
        assert_eq!(p.form_kind, "host");
        assert_eq!(p.form_proto, "tcp");
        assert_eq!(p.form_tier, "mesh-only");
        assert_eq!(p.form_mode, "http");
    }

    #[test]
    fn port_input_keeps_digits_only() {
        let mut p = ConnectivityPanel::new();
        let _ = p.update(Message::FormPortChanged("3a0b0".into()));
        assert_eq!(p.form_port, "300");
    }

    #[test]
    fn prefill_populates_form_and_defaults_public() {
        let mut p = ConnectivityPanel::new();
        let _ = p.update(Message::Prefill {
            id: "eagle-3000".into(),
            node: "eagle".into(),
            kind: "container".into(),
            port: 3000,
            proto: "tcp".into(),
        });
        assert_eq!(p.form_id, "eagle-3000");
        assert_eq!(p.form_node, "eagle");
        assert_eq!(p.form_kind, "container");
        assert_eq!(p.form_port, "3000");
        assert_eq!(p.form_tier, "public-via-ingress");
    }

    #[test]
    fn validated_form_requires_id_node_and_port() {
        let mut p = ConnectivityPanel::new();
        assert!(p.validated_form().unwrap_err().contains("service id"));
        p.form_id = "svc".into();
        assert!(p.validated_form().unwrap_err().contains("source node"));
        p.form_node = "eagle".into();
        assert!(p.validated_form().unwrap_err().contains("port"));
        p.form_port = "0".into();
        assert!(p.validated_form().unwrap_err().contains("port"));
        p.form_port = "8080".into();
        // mesh-only now validates.
        let f = p.validated_form().unwrap();
        assert_eq!(f.id, "svc");
        assert_eq!(f.port, 8080);
        assert!(!f.public);
    }

    #[test]
    fn validated_form_public_requires_ingress() {
        let mut p = ConnectivityPanel::new();
        p.form_id = "svc".into();
        p.form_node = "eagle".into();
        p.form_port = "3000".into();
        p.form_tier = "public-via-ingress".into();
        assert!(p
            .validated_form()
            .unwrap_err()
            .contains("ingress lighthouse"));
        p.form_lighthouse = "Lighthouse-01".into();
        p.form_hostname = "g.example".into();
        let f = p.validated_form().unwrap();
        assert!(f.public);
        assert_eq!(f.lighthouse, "Lighthouse-01");
        assert_eq!(f.hostname, "g.example");
    }

    #[test]
    fn expose_preview_mesh_only_has_no_public_surface() {
        let s = expose_preview("grafana", "eagle", "3000", false, "", "", "http");
        assert!(s.contains("Mesh-only"));
        assert!(s.contains("eagle.mesh:3000"));
        assert!(s.contains("No public surface"));
    }

    #[test]
    fn expose_preview_public_http_describes_caddy_and_firewall() {
        let s = expose_preview(
            "grafana",
            "eagle",
            "3000",
            true,
            "Lighthouse-01",
            "g.example",
            "http",
        );
        assert!(s.contains("auto-HTTPS"));
        assert!(s.contains("g.example"));
        assert!(s.contains("eagle.mesh:3000"));
        assert!(s.contains("80/443"));
        assert!(s.contains("No ingress auth"));
    }

    #[test]
    fn expose_preview_public_tcp_describes_stream_forward() {
        let s = expose_preview(
            "game",
            "eagle",
            "25565",
            true,
            "LH-01",
            "game.example",
            "tcp",
        );
        assert!(s.contains("firewalld forwards"));
        assert!(s.contains("25565/tcp"));
        assert!(s.contains("raw stream"));
    }

    #[test]
    fn reply_error_detects_envelope() {
        assert_eq!(reply_error(r#"{"error":"boom"}"#).as_deref(), Some("boom"));
        assert!(reply_error(r#"{"ok":true}"#).is_none());
        assert!(reply_error("garbage").is_none());
    }

    #[test]
    fn expose_clicked_with_invalid_form_surfaces_error_no_apply() {
        let mut p = ConnectivityPanel::new();
        // empty id → validation error, applying stays false.
        let _ = p.update(Message::ExposeClicked);
        assert!(!p.applying);
        assert!(p.last_op.contains("service id"));
    }

    #[test]
    fn applied_ok_clears_applying_and_reloads() {
        let mut p = ConnectivityPanel::new();
        p.applying = true;
        let _ = p.update(Message::Applied(Ok("Exposed 'x'.".into())));
        assert!(!p.applying);
        assert_eq!(p.last_op, "Exposed 'x'.");
        assert!(p.busy, "reload kicked off");
    }

    #[test]
    fn applied_err_clears_applying_keeps_state() {
        let mut p = ConnectivityPanel::new();
        p.applying = true;
        let _ = p.update(Message::Applied(Err("expose failed: nope".into())));
        assert!(!p.applying);
        assert_eq!(p.last_op, "expose failed: nope");
    }

    #[test]
    fn unexpose_while_applying_is_noop() {
        let mut p = ConnectivityPanel::new();
        p.applying = true;
        p.last_op = "busy".into();
        let _ = p.update(Message::UnexposeClicked("x".into()));
        assert_eq!(p.last_op, "busy");
    }
}
