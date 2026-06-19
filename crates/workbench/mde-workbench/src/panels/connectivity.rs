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

use cosmic::iced::widget::{button, column, container, row, scrollable, text, Space};
use cosmic::iced::{Background, Border, Color, Length, Padding, Task};
use cosmic::{Element, Theme};
use mde_theme::{FontSize, Palette, TypeRole};
use serde::Deserialize;

use crate::cosmic_compat::prelude::*;

/// Read budget for the connect Bus probes — matches the other panels' 2 s.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

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
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded {
        services: Vec<ServiceRow>,
        candidates: Vec<CandidateRow>,
        error: Option<String>,
    },
    RefreshClicked,
}

impl ConnectivityPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
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
        }
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

        let body = scrollable(column![services_section, candidates_section].spacing(2))
            .height(Length::FillPortion(1));

        container(column![header, Space::new().height(Length::Fixed(20.0)), body,].spacing(2))
            .padding(Padding::from([24u16, 32u16]))
            .width(Length::Fill)
            .height(Length::Fill)
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
            Space::new().width(Length::FillPortion(3)),
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
}
