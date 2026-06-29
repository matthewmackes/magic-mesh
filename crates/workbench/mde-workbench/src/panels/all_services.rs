//! COMPUTE/SVC-VIEW (#6) — "All Services" unified panel.
//!
//! Unions the THREE existing service-data sources into one source-tagged
//! table so the operator has one truthful place to see every service on
//! the mesh. Today they live in three separate panels:
//!
//! 1. **Published** — canonical Nebula-registered services
//!    (`panels/service_publishing.rs`, `fleet_rows_from_peers`).
//! 2. **Discovered** — nmap-probe host/service inventory
//!    (`panels/network_hosts.rs`, `read_inventory_cards` + `inventory_from_cards`).
//! 3. **VM-internal** — compute/inventory per-peer bus documents
//!    (`panels/compute.rs`, `read_shared_inventories`).
//!
//! The panel is read-only: it reuses the existing readers verbatim and
//! never kicks off a scan or probe. Refresh just re-reads all three.

use std::time::SystemTime;

use cosmic::iced::widget::{button, column, container, row, scrollable, text, Space};
use cosmic::iced::{Background, Border, Color, Length, Padding, Task};
use cosmic::Theme;

/// Theme-bound element alias (mirrors network_hosts.rs).
type Element<'a, M> = cosmic::iced::Element<'a, M, Theme>;

use crate::cosmic_compat::prelude::*;
use mde_theme::{FontSize, Palette, TypeRole};

/// One row in the merged "All Services" table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnifiedServiceRow {
    /// Data provenance: `"published"` | `"discovered"` | `"vm-internal"`.
    pub source: &'static str,
    /// Service or VM/container name (canonical id for published, hostname
    /// for discovered, instance name for vm-internal).
    pub name: String,
    /// The node or host carrying the service.
    pub host: String,
    /// Human-readable detail line (port/protocol, state, kind, etc.).
    pub detail: String,
}

#[derive(Debug, Clone, Default)]
pub struct AllServicesPanel {
    pub rows: Vec<UnifiedServiceRow>,
    pub error: Option<String>,
    pub last_run_at: Option<SystemTime>,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<Vec<UnifiedServiceRow>, String>),
    RefreshClicked,
}

impl AllServicesPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Kick off an async read of all three sources on the iced executor.
    /// Uses `spawn_blocking` for the I/O-heavy readers (same pattern as
    /// `service_publishing::ServicePublishingPanel::load` — these helpers
    /// build their own current-thread runtime inside and would panic if
    /// called directly from an iced async Task on the multi-thread runtime).
    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async {
                tokio::task::spawn_blocking(fetch_all)
                    .await
                    .unwrap_or_else(|_| Err("all-services fetch task panicked".into()))
            },
            |result| crate::Message::AllServices(Message::Loaded(result)),
        )
    }

    pub fn update(&mut self, msg: Message) -> Task<crate::Message> {
        match msg {
            Message::Loaded(Ok(rows)) => {
                self.rows = rows;
                self.error = None;
                self.busy = false;
                self.last_run_at = Some(SystemTime::now());
                Task::none()
            }
            Message::Loaded(Err(e)) => {
                self.rows = Vec::new();
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

        let title = text("All Services")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());

        let subtitle_str = if self.last_run_at.is_some() {
            let n = self.rows.len();
            format!(
                "{n} service{} across published · discovered · vm-internal",
                if n == 1 { "" } else { "s" },
            )
        } else {
            "click Refresh to read all three service sources".into()
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
        .on_press(crate::Message::AllServices(Message::RefreshClicked));

        let header = row![title, Space::new().width(Length::Fill), refresh_btn]
            .align_y(cosmic::iced::Alignment::Center);

        let body: Element<'_, crate::Message> = if let Some(ref e) = self.error {
            text(format!("Error: {e}"))
                .size(TypeRole::Body.size_in(sizes))
                .colr(palette.danger.into_cosmic_color())
                .into()
        } else if self.rows.is_empty() && self.last_run_at.is_some() {
            text(
                "No services found — try Refresh once the mesh is up \
                 and at least one probe has run.",
            )
            .size(TypeRole::Body.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color())
            .into()
        } else {
            let blocks: Vec<Element<'_, crate::Message>> = self
                .rows
                .iter()
                .map(|r| row_view(r, palette, sizes))
                .collect();
            scrollable(column(blocks).spacing(6)).into()
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

/// Source badge color: published = accent, discovered = success, vm-internal = warning.
fn source_color(source: &str, palette: Palette) -> Color {
    match source {
        "published" => palette.accent.into_cosmic_color(),
        "discovered" => palette.success.into_cosmic_color(),
        "vm-internal" => palette.warning.into_cosmic_color(),
        _ => palette.text_muted.into_cosmic_color(),
    }
}

/// One unified service row: source badge · name · host · detail.
fn row_view<'a>(
    r: &'a UnifiedServiceRow,
    palette: Palette,
    sizes: FontSize,
) -> Element<'a, crate::Message> {
    let badge_color = source_color(r.source, palette);
    let bg = palette.raised.into_cosmic_color();
    let border = palette.border.into_cosmic_color();

    let badge = container(
        text(r.source)
            .size(TypeRole::Caption.size_in(sizes))
            .colr(Color::WHITE),
    )
    .padding(Padding::from([2u16, 8u16]))
    .sty(move |_| container::Style {
        snap: false,
        background: Some(Background::Color(badge_color)),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 10.0.into(),
        },
        ..container::Style::default()
    });

    container(
        row![
            badge,
            text(r.name.clone())
                .size(TypeRole::Body.size_in(sizes))
                .colr(palette.text.into_cosmic_color())
                .width(Length::FillPortion(3)),
            text(r.host.clone())
                .size(TypeRole::Body.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color())
                .width(Length::FillPortion(2)),
            text(r.detail.clone())
                .size(TypeRole::Caption.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color())
                .width(Length::FillPortion(3)),
        ]
        .spacing(12)
        .align_y(cosmic::iced::alignment::Vertical::Center),
    )
    .padding(Padding::from([8u16, 12u16]))
    .width(Length::Fill)
    .sty(move |_| container::Style {
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

// ── I/O ──────────────────────────────────────────────────────────────────────

/// Read all three sources and return the merged, sorted, deduped row list.
/// Called inside a `spawn_blocking` future.
fn fetch_all() -> Result<Vec<UnifiedServiceRow>, String> {
    let mut rows: Vec<UnifiedServiceRow> = Vec::new();

    // 1. Published services — canonical registered via the mesh directory.
    let peers = crate::mesh_directory::fetch_peers();
    for svc in crate::panels::service_publishing::fleet_rows_from_peers(&peers) {
        let detail = if svc.is_publishable {
            format!("{}/{} published", svc.port, svc.proto)
        } else {
            format!("{}/{} not enrolled", svc.port, svc.proto)
        };
        rows.push(UnifiedServiceRow {
            source: "published",
            name: svc.name.clone(),
            host: if svc.node.is_empty() {
                "this node".to_string()
            } else {
                svc.node.clone()
            },
            detail,
        });
    }

    // 2. Discovered services — nmap probe inventory off mesh-storage.
    let root = mackes_mesh_types::peers::default_workgroup_root();
    if root.exists() {
        let cards = crate::panels::network_hosts::read_inventory_cards(&root);
        let inv = crate::panels::network_hosts::inventory_from_cards(&cards);
        for host in &inv.hosts {
            for svc in &host.services {
                let detail = if svc.product.is_empty() {
                    format!(":{} {}", svc.port, svc.kind)
                } else {
                    format!(":{} {} ({})", svc.port, svc.kind, svc.product)
                };
                rows.push(UnifiedServiceRow {
                    source: "discovered",
                    name: svc.kind.clone(),
                    host: host.display.clone(),
                    detail,
                });
            }
        }
    }

    // 3. VM-internal — compute/inventory per-peer bus documents.
    let bus_invs = crate::panels::compute::read_shared_inventories();
    for inv in &bus_invs {
        let node = {
            let h = inv.hostname.trim();
            let h = h.strip_prefix("peer:").unwrap_or(h);
            if h.is_empty() {
                "unknown".to_string()
            } else {
                h.to_string()
            }
        };
        for vm in &inv.vms {
            if vm.name.trim().is_empty() {
                continue;
            }
            rows.push(UnifiedServiceRow {
                source: "vm-internal",
                name: vm.name.clone(),
                host: node.clone(),
                detail: format!("VM · {}", vm.state),
            });
        }
        for ct in &inv.containers {
            if ct.name.trim().is_empty() {
                continue;
            }
            rows.push(UnifiedServiceRow {
                source: "vm-internal",
                name: ct.name.clone(),
                host: node.clone(),
                detail: format!("Container · {}", ct.state),
            });
        }
    }

    Ok(merge_rows(rows))
}

/// Pure merge: sort by (source, host, name), then dedup by (source, name, host).
#[must_use]
pub fn merge_rows(mut rows: Vec<UnifiedServiceRow>) -> Vec<UnifiedServiceRow> {
    rows.sort_by(|a, b| {
        a.source
            .cmp(b.source)
            .then_with(|| a.host.cmp(&b.host))
            .then_with(|| a.name.cmp(&b.name))
    });
    // Dedup by the (source, name, host) key — keep the first occurrence
    // (after sort, duplicates are adjacent).
    rows.dedup_by(|b, a| a.source == b.source && a.name == b.name && a.host == b.host);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(source: &'static str, name: &str, host: &str, detail: &str) -> UnifiedServiceRow {
        UnifiedServiceRow {
            source,
            name: name.into(),
            host: host.into(),
            detail: detail.into(),
        }
    }

    #[test]
    fn merge_rows_sorts_by_source_host_name() {
        let rows = vec![
            row("vm-internal", "web", "node-b", "Container · running"),
            row("published", "SSH", "node-a", "22/tcp published"),
            row("discovered", "ssh", "router", ":22 ssh"),
        ];
        let merged = merge_rows(rows);
        assert_eq!(merged.len(), 3);
        // Alphabetically: "discovered" < "published" < "vm-internal".
        assert_eq!(merged[0].source, "discovered");
        assert_eq!(merged[1].source, "published");
        assert_eq!(merged[2].source, "vm-internal");
    }

    #[test]
    fn merge_rows_dedup_by_source_name_host() {
        // Two identical (source, name, host) rows — only one survives.
        let rows = vec![
            row("published", "SSH", "node-a", "22/tcp published"),
            row("published", "SSH", "node-a", "22/tcp published"),
            row("published", "NATS broker", "node-a", "4222/tcp published"),
        ];
        let merged = merge_rows(rows);
        assert_eq!(merged.len(), 2, "duplicate must be deduped: {merged:?}");
        assert!(merged.iter().any(|r| r.name == "SSH"));
        assert!(merged.iter().any(|r| r.name == "NATS broker"));
    }

    #[test]
    fn merge_rows_same_name_different_source_both_kept() {
        // "ssh" from discovered and "SSH" from published differ by source + name.
        let rows = vec![
            row("discovered", "ssh", "router", ":22 ssh"),
            row("published", "SSH", "node-a", "22/tcp published"),
        ];
        let merged = merge_rows(rows);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn merge_rows_same_name_different_host_both_kept() {
        // Same source + name but on two different hosts — both are real entries.
        let rows = vec![
            row("published", "SSH", "node-a", "22/tcp published"),
            row("published", "SSH", "node-b", "22/tcp published"),
        ];
        let merged = merge_rows(rows);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn merge_rows_empty_input_is_noop() {
        assert!(merge_rows(vec![]).is_empty());
    }

    #[test]
    fn panel_defaults_are_clean() {
        let p = AllServicesPanel::new();
        assert!(p.rows.is_empty());
        assert!(p.error.is_none());
        assert!(!p.busy);
        assert!(p.last_run_at.is_none());
    }

    #[test]
    fn update_loaded_ok_populates_rows() {
        let mut p = AllServicesPanel::new();
        let _ = p.update(Message::Loaded(Ok(vec![row(
            "published",
            "SSH",
            "node-a",
            "22/tcp published",
        )])));
        assert_eq!(p.rows.len(), 1);
        assert!(p.error.is_none());
        assert!(!p.busy);
        assert!(p.last_run_at.is_some());
    }

    #[test]
    fn update_loaded_err_sets_error() {
        let mut p = AllServicesPanel::new();
        let _ = p.update(Message::Loaded(Err("no mount".into())));
        assert!(p.rows.is_empty());
        assert_eq!(p.error.as_deref(), Some("no mount"));
        assert!(!p.busy);
        assert!(p.last_run_at.is_some());
    }

    #[test]
    fn update_refresh_clicked_sets_busy() {
        let mut p = AllServicesPanel::new();
        let _ = p.update(Message::RefreshClicked);
        assert!(p.busy);
    }

    #[test]
    fn view_renders_empty_state_without_panic() {
        let p = AllServicesPanel::new();
        let _ = p.view();
    }

    #[test]
    fn view_renders_with_rows_without_panic() {
        let mut p = AllServicesPanel::new();
        p.rows = vec![
            row("published", "SSH", "node-a", "22/tcp published"),
            row("discovered", "ssh", "router", ":22 ssh"),
            row("vm-internal", "fedora-vm", "node-b", "VM · running"),
        ];
        let _ = p.view();
    }

    #[test]
    fn source_color_returns_distinct_tokens() {
        let palette = crate::live_theme::palette();
        let pub_c = source_color("published", palette);
        let disc_c = source_color("discovered", palette);
        let vm_c = source_color("vm-internal", palette);
        // Each source tag resolves to a distinct (non-muted) color.
        assert_ne!(pub_c, palette.text_muted.into_cosmic_color());
        assert_ne!(disc_c, palette.text_muted.into_cosmic_color());
        assert_ne!(vm_c, palette.text_muted.into_cosmic_color());
        // All three are distinct from each other.
        assert_ne!(pub_c, disc_c);
        assert_ne!(pub_c, vm_c);
        assert_ne!(disc_c, vm_c);
    }

    #[test]
    fn fetch_all_does_not_panic_on_missing_mount() {
        // The workgroup root and bus spool almost certainly don't exist in the
        // build environment; the contract is "never panics, returns Ok(empty)".
        let result = fetch_all();
        assert!(
            result.is_ok(),
            "fetch_all must not return Err on a bare host"
        );
    }
}
