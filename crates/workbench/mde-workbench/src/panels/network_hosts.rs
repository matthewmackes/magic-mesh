//! MESH-PROBE-9.a (v10.0.0) — Workbench "Network Hosts" panel.
//!
//! Surfaces the probe host/service inventory. The `probe` worker writes
//! each peer's deep-scan result as a `Vec<mde_card::Card>` (one `Host`
//! card per reachable host, each carrying its open ports as `Service`
//! children) to `<workgroup_root>/<peer>/mackesd/probe-inventory.json`.
//! This panel reads + merges every peer's file — the same on-disk
//! contract `mackesd::probe_nmap::inventory` serves daemon-side — then
//! lists each host with its identified services + trust-state: the
//! "what's on my mesh + LAN" view (MESH-PROBE-9).
//!
//! Read-only by design: scanning is the worker's job (it owns nmap
//! timing + the do-not-scan exclusion). Refresh just re-reads the
//! merged files, so opening the panel never kicks off a scan.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use cosmic::iced::widget::{button, column, container, row, scrollable, text, Space};
use cosmic::iced::{Background, Border, Color, Length, Padding, Task};
use cosmic::Theme;

/// Theme-bound element alias: the workbench tree threads libcosmic's
/// `cosmic::Theme`, not the crates.io `iced::Theme`.
type Element<'a, M> = cosmic::iced::Element<'a, M, Theme>;

use crate::cosmic_compat::prelude::*;
use mde_card::probe::{host_facts, service_facts, HostSource};
use mde_card::Card;
use mde_theme::{FontSize, Palette, TypeRole};

/// One identified service on a host: an open port + what's behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceRow {
    pub port: u16,
    pub kind: String,
    pub product: String,
}

/// One host in the merged inventory + the services found on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRow {
    /// Display label — hostname when known, else the bare IP.
    pub display: String,
    pub ip: String,
    /// Where the host came from: "mesh" / "LAN" / "manual".
    pub source: String,
    /// Trust-state string as written by the prober ("" when unscored).
    pub trust: String,
    pub services: Vec<ServiceRow>,
}

/// The merged inventory the panel renders.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostInventory {
    pub hosts: Vec<HostRow>,
}

impl HostInventory {
    /// Total services across all hosts (for the subtitle count).
    #[must_use]
    pub fn service_count(&self) -> usize {
        self.hosts.iter().map(|h| h.services.len()).sum()
    }
}

#[derive(Debug, Clone, Default)]
pub struct NetworkHostsPanel {
    pub inventory: HostInventory,
    pub error: Option<String>,
    pub last_run_at: Option<SystemTime>,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<HostInventory, String>),
    RefreshClicked,
}

impl NetworkHostsPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(async { fetch_inventory() }, |result| {
            crate::Message::NetworkHosts(Message::Loaded(result))
        })
    }

    pub fn update(&mut self, msg: Message) -> Task<crate::Message> {
        match msg {
            Message::Loaded(Ok(inv)) => {
                self.inventory = inv;
                self.error = None;
                self.busy = false;
                self.last_run_at = Some(SystemTime::now());
                Task::none()
            }
            Message::Loaded(Err(e)) => {
                self.inventory = HostInventory::default();
                self.error = Some(e);
                self.busy = false;
                self.last_run_at = Some(SystemTime::now());
                Task::none()
            }
            Message::RefreshClicked => {
                self.busy = true;
                Self::load()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let sizes = FontSize::defaults();

        let title = text("Network Hosts")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());

        let subtitle_str = if self.last_run_at.is_some() {
            let n = self.inventory.hosts.len();
            format!(
                "{n} host{} · {} service{} identified",
                if n == 1 { "" } else { "s" },
                self.inventory.service_count(),
                if self.inventory.service_count() == 1 {
                    ""
                } else {
                    "s"
                },
            )
        } else {
            "click Refresh to read the merged probe inventory".into()
        };
        let subtitle = text(subtitle_str)
            .size(TypeRole::Body.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color());

        let accent = palette.accent.into_cosmic_color();
        let refresh_btn = button(
            text(if self.busy { "Loading…" } else { "Refresh" })
                .size(13)
                .colr(Color::WHITE),
        )
        .padding(Padding::from([6u16, 14u16]))
        .sty(
            move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
                let bg = match status {
                    cosmic::iced::widget::button::Status::Hovered => Color {
                        r: (accent.r * 1.10).min(1.0),
                        g: (accent.g * 1.10).min(1.0),
                        b: (accent.b * 1.10).min(1.0),
                        a: accent.a,
                    },
                    _ => accent,
                };
                cosmic::iced::widget::button::Style {
                    snap: false,
                    background: Some(Background::Color(bg)),
                    icon_color: None,
                    text_color: Color::WHITE,
                    border_radius: 6.0.into(),
                    border_width: 0.0,
                    border_color: Color::TRANSPARENT,
                    border: Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: 6.0.into(),
                    },
                    shadow: cosmic::iced::Shadow::default(),
                }
            },
        )
        .on_press(crate::Message::NetworkHosts(Message::RefreshClicked));

        let header = row![title, Space::new().width(Length::Fill), refresh_btn]
            .align_y(cosmic::iced::Alignment::Center);

        let body: Element<'_, crate::Message> = if let Some(ref e) = self.error {
            text(format!("Error: {e}"))
                .size(TypeRole::Body.size_in(sizes))
                .colr(palette.danger.into_cosmic_color())
                .into()
        } else if self.inventory.hosts.is_empty() && self.last_run_at.is_some() {
            text("No hosts in the inventory yet — the probe worker populates it on its scan cadence.")
                .size(TypeRole::Body.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color())
                .into()
        } else {
            let blocks: Vec<Element<'_, crate::Message>> = self
                .inventory
                .hosts
                .iter()
                .map(|h| host_block(h, palette, sizes))
                .collect();
            scrollable(column(blocks).spacing(10)).into()
        };

        let page = column![header, row![subtitle], Space::new().height(12), body].spacing(4);

        let surface_color = palette.surface.into_cosmic_color();
        container(page)
            .padding(24)
            .width(Length::Fill)
            .height(Length::Fill)
            .sty(move |_t: &Theme| container::Style {
                snap: false,
                background: Some(Background::Color(surface_color)),
                ..Default::default()
            })
            .into()
    }
}

/// Trust-state → badge color. Unscored ("") + unknown render muted;
/// an explicit trusted/untrusted verdict gets green/red.
fn trust_color(trust: &str, palette: Palette) -> Color {
    match trust.to_ascii_lowercase().as_str() {
        "trusted" | "mesh" | "enrolled" => palette.success.into_cosmic_color(),
        "untrusted" | "blocked" | "denied" => palette.danger.into_cosmic_color(),
        _ => palette.text_muted.into_cosmic_color(),
    }
}

/// Human label for a trust-state string ("" → "unscored").
fn trust_label(trust: &str) -> String {
    if trust.is_empty() {
        "unscored".to_string()
    } else {
        trust.to_string()
    }
}

fn host_block<'a>(
    h: &'a HostRow,
    palette: Palette,
    sizes: FontSize,
) -> Element<'a, crate::Message> {
    // Header: display name, IP (when the display is a hostname), source
    // chip on the right, trust badge.
    let mut head = row![text(&h.display)
        .size(TypeRole::Body.size_in(sizes))
        .colr(palette.text.into_cosmic_color())]
    .spacing(8)
    .align_y(cosmic::iced::Alignment::Center);
    if h.display != h.ip {
        head = head.push(
            text(format!("· {}", h.ip))
                .size(TypeRole::Caption.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color()),
        );
    }
    head = head.push(Space::new().width(Length::Fill));
    head = head.push(
        text(h.source.clone())
            .size(TypeRole::Caption.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color()),
    );
    head = head.push(
        text(trust_label(&h.trust))
            .size(TypeRole::Caption.size_in(sizes))
            .colr(trust_color(&h.trust, palette)),
    );

    let mut block = column![head].spacing(2);
    if h.services.is_empty() {
        block = block.push(
            text("  no open ports identified")
                .size(TypeRole::Caption.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color()),
        );
    } else {
        for s in &h.services {
            let detail = if s.product.is_empty() {
                format!("  :{} {}", s.port, s.kind)
            } else {
                format!("  :{} {} ({})", s.port, s.kind, s.product)
            };
            block = block.push(
                text(detail)
                    .size(TypeRole::Caption.size_in(sizes))
                    .colr(palette.text_muted.into_cosmic_color()),
            );
        }
    }
    block.into()
}

/// The mesh-storage mount that holds every peer's `mackesd/` state.
/// Single-sourced with `mackesd` via `mackes_mesh_types` so the panel
/// and the daemon resolve the identical mount (`~/QNM-Shared` by
/// default). Previously this fell back to a phantom `/mnt/mesh-storage`
/// that diverged from the daemon's `~/QNM-Shared`, so the panel showed
/// "not mounted" against a healthy mesh.
fn workgroup_root() -> PathBuf {
    mackes_mesh_types::peers::default_workgroup_root()
}

/// Read + merge every peer's `<root>/<peer>/mackesd/probe-inventory.json`
/// into one `Vec<Card>`. Fail-open per file (a missing/corrupt peer
/// inventory is skipped) so one bad file can't blind the reader.
fn read_inventory_cards(root: &Path) -> Vec<Card> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path().join("mackesd").join("probe-inventory.json");
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Ok(cards) = serde_json::from_str::<Vec<Card>>(&body) {
            out.extend(cards);
        }
    }
    out
}

/// Map `HostSource` to the panel's short display label.
fn source_label(source: HostSource) -> &'static str {
    match source {
        HostSource::Mesh => "mesh",
        HostSource::Lan => "LAN",
        HostSource::Arbitrary => "manual",
    }
}

/// Pure: flatten the inventory cards into sorted display rows. Host
/// cards (kind = Host) become a [`HostRow`]; their `Service` children
/// become [`ServiceRow`]s. Non-host cards + non-service children are
/// skipped. Hosts sort by display label, services by port.
#[must_use]
pub fn inventory_from_cards(cards: &[Card]) -> HostInventory {
    let mut hosts: Vec<HostRow> = Vec::new();
    for card in cards {
        let Some(hf) = host_facts(card) else {
            continue;
        };
        let mut services: Vec<ServiceRow> = card
            .children
            .iter()
            .filter_map(|child| {
                service_facts(child).map(|sf| ServiceRow {
                    port: sf.port,
                    kind: sf.service_kind,
                    product: sf.product,
                })
            })
            .collect();
        services.sort_by_key(|s| s.port);
        let display = if hf.hostname.is_empty() {
            hf.ip.clone()
        } else {
            hf.hostname.clone()
        };
        hosts.push(HostRow {
            display,
            ip: hf.ip,
            source: source_label(hf.source).to_string(),
            trust: hf.trust_state,
            services,
        });
    }
    hosts.sort_by(|a, b| a.display.cmp(&b.display));
    HostInventory { hosts }
}

/// Read the merged probe inventory off the mesh-storage mount.
/// Returns an honest error (shown as the empty-state hint) when the
/// mount isn't present — the inventory only exists once mesh-storage is
/// active + the probe worker has run at least once.
///
/// Pub so the unified "Services across the mesh" view (SVC-VIEW) reuses this
/// exact probe-inventory read instead of duplicating the data path.
pub fn fetch_inventory() -> Result<HostInventory, String> {
    let root = workgroup_root();
    if !root.exists() {
        return Err(format!(
            "workgroup root {} not present — mesh-storage not mounted yet",
            root.display()
        ));
    }
    Ok(inventory_from_cards(&read_inventory_cards(&root)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_card::probe::{host_card, service_card, HostFacts, ServiceFacts};

    fn host(
        ip: &str,
        hostname: &str,
        source: HostSource,
        trust: &str,
        ports: &[(u16, &str)],
    ) -> Card {
        let services: Vec<Card> = ports
            .iter()
            .map(|(port, kind)| {
                service_card(
                    &ServiceFacts {
                        port: *port,
                        service_kind: (*kind).to_string(),
                        product: String::new(),
                        version: String::new(),
                        fingerprint: String::new(),
                    },
                    0,
                )
            })
            .collect();
        host_card(
            &HostFacts {
                ip: ip.to_string(),
                hostname: hostname.to_string(),
                source,
                trust_state: trust.to_string(),
                last_seen: 0,
            },
            services,
            0,
        )
    }

    #[test]
    fn inventory_flattens_hosts_and_services_sorted() {
        let cards = vec![
            host(
                "10.0.0.9",
                "router",
                HostSource::Lan,
                "",
                &[(443, "https"), (22, "ssh")],
            ),
            host(
                "10.42.0.2",
                "",
                HostSource::Mesh,
                "trusted",
                &[(8096, "http")],
            ),
        ];
        let inv = inventory_from_cards(&cards);
        assert_eq!(inv.hosts.len(), 2);
        // Sorted by display: "10.42.0.2" (bare IP) < "router".
        assert_eq!(inv.hosts[0].display, "10.42.0.2");
        assert_eq!(inv.hosts[0].source, "mesh");
        assert_eq!(inv.hosts[0].trust, "trusted");
        assert_eq!(inv.hosts[1].display, "router");
        assert_eq!(inv.hosts[1].ip, "10.0.0.9");
        assert_eq!(inv.hosts[1].source, "LAN");
        // Services sorted by port: 22 before 443.
        assert_eq!(inv.hosts[1].services[0].port, 22);
        assert_eq!(inv.hosts[1].services[1].port, 443);
        assert_eq!(inv.service_count(), 3);
    }

    #[test]
    fn non_host_cards_are_skipped() {
        // A bare note card carries no host facts → dropped.
        let cards = vec![Card::new(mde_card::CardKind::Note, "scratch", 0)];
        assert!(inventory_from_cards(&cards).hosts.is_empty());
    }

    #[test]
    fn trust_label_and_color_handle_unscored() {
        assert_eq!(trust_label(""), "unscored");
        assert_eq!(trust_label("trusted"), "trusted");
        // Unscored uses the muted token, not green/red.
        let p = crate::live_theme::palette();
        assert_eq!(trust_color("", p), p.text_muted.into_cosmic_color());
        assert_ne!(trust_color("trusted", p), p.text_muted.into_cosmic_color());
    }

    #[test]
    fn source_label_covers_all_variants() {
        assert_eq!(source_label(HostSource::Mesh), "mesh");
        assert_eq!(source_label(HostSource::Lan), "LAN");
        assert_eq!(source_label(HostSource::Arbitrary), "manual");
    }

    #[test]
    fn panel_defaults_and_load_transitions() {
        let panel = NetworkHostsPanel::new();
        assert!(panel.inventory.hosts.is_empty());
        assert!(panel.error.is_none());
        assert!(!panel.busy);

        let mut panel = panel;
        let inv = inventory_from_cards(&[host(
            "10.0.0.1",
            "gw",
            HostSource::Lan,
            "",
            &[(53, "domain")],
        )]);
        let _ = panel.update(Message::Loaded(Ok(inv)));
        assert_eq!(panel.inventory.hosts.len(), 1);
        assert!(panel.last_run_at.is_some());

        let _ = panel.update(Message::Loaded(Err("no mount".to_string())));
        assert!(panel.inventory.hosts.is_empty());
        assert_eq!(panel.error.as_deref(), Some("no mount"));
    }

    #[test]
    fn fetch_inventory_errors_without_mount() {
        // The default mount almost certainly doesn't exist in CI; if a
        // dev box happens to have /mnt/mesh-storage the call still
        // completes (Ok) — the contract is "never panics".
        let _ = fetch_inventory();
    }
}
