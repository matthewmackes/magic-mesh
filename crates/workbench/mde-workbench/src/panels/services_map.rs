//! SVC-VIEW — the unified "Services across the mesh" view.
//!
//! One truthful place that UNIONS the three service surfaces the operator
//! otherwise had to cross-reference by hand (SVC-VIEW-2, operator bug-testing
//! 2026-06-18):
//!
//!   1. **canonical-published** services — the 7 Nebula fabric services per
//!      enrolled peer (PD-2 / the replicated peer roster), reused verbatim from
//!      [`service_publishing::fetch_summary`];
//!   2. **probe-discovered** services — open ports the `probe` worker found on
//!      mesh + LAN hosts, reused from [`network_hosts::fetch_inventory`]
//!      (`probe-inventory.json`); and
//!   3. **VM-internal where available** — the VMs/containers the compute
//!      inventory enumerates fleet-wide (e.g. an `airsonic` container is the
//!      very service SVC-VIEW-2 wanted surfaced), reused from
//!      [`compute::enumerate`].
//!
//! It does NOT re-read any of those data paths — it calls the existing readers
//! and merges their outputs through the pure [`unify_services`] union/dedup, so
//! a service found by two surfaces (e.g. SSH:22 both published and probe-found
//! on the same host) collapses to one row. Every row is tagged with its **host**.
//! Carbon tokens only (§4) — all colour/metrics via the `mde-theme` palette.

use std::collections::BTreeMap;
use std::time::{Instant, SystemTime};

use cosmic::iced::widget::{button, column, container, row, scrollable, text, Space};
use cosmic::iced::{Background, Border, Color, Length, Padding, Task};
use cosmic::{Element, Theme};
use mde_theme::{FontSize, Palette, TypeRole};

use crate::cosmic_compat::prelude::*;
use crate::panels::{compute, network_hosts, service_publishing};

/// Which surface a unified row came from. Also the dedup precedence: a service
/// seen by several surfaces keeps the highest-precedence one (canonical
/// **Published** > **Probe** > **VM/container**), since the published row
/// carries the authoritative service identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceSource {
    /// Canonical Nebula-published fabric service (PD-2 / peer roster).
    Published,
    /// Open port found by the nmap probe (`probe-inventory.json`).
    Probe,
    /// A VM or container from the compute inventory.
    Vm,
}

impl ServiceSource {
    /// Short display tag.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Published => "published",
            Self::Probe => "probe",
            Self::Vm => "vm/container",
        }
    }

    /// Dedup precedence — lower wins (kept on a key collision).
    #[must_use]
    fn precedence(self) -> u8 {
        match self {
            Self::Published => 0,
            Self::Probe => 1,
            Self::Vm => 2,
        }
    }
}

/// One unified service row, tagged with the host it runs on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnifiedService {
    /// The node/host this service runs on (the per-row tag).
    pub host: String,
    /// Service display name.
    pub name: String,
    /// Secondary detail line (id / product / VM state, source-specific).
    pub detail: String,
    /// Port, or `None` for a portless workload (VM/container).
    pub port: Option<u16>,
    /// "tcp" / "udp" / "" (probe + VM rows leave it empty).
    pub proto: String,
    /// Which surface contributed the row.
    pub source: ServiceSource,
    /// Whether it's actually serving / running (drives the status pill).
    pub reachable: bool,
}

/// Dedup key tuple: `(host, port, discriminator)`. The discriminator is empty
/// for a ported service (so `host + port` alone identifies it) and the service
/// name for a portless workload.
type DedupKey = (String, Option<u16>, String);

impl UnifiedService {
    /// Dedup identity: a ported service is `host + port` — **proto-agnostic**,
    /// because the probe surface reports an open port without a transport, so a
    /// probe-found SSH:22 must still collapse against the canonical published
    /// SSH:22/tcp on the same host. A portless workload (VM/container) is keyed
    /// by `host + name`. Case-insensitive on the host + name.
    #[must_use]
    fn dedup_key(&self) -> DedupKey {
        let host = self.host.to_ascii_lowercase();
        match self.port {
            Some(p) => (host, Some(p), String::new()),
            None => (host, None, self.name.to_ascii_lowercase()),
        }
    }

    /// MOTION-TRANS-3 — a stable, unique string key for the row-insert reveal,
    /// mirroring [`Self::dedup_key`] (rows are already unique by it after the
    /// union/dedup) so a row keeps the same identity across refreshes.
    #[must_use]
    fn row_key(&self) -> String {
        let host = self.host.to_ascii_lowercase();
        match self.port {
            Some(p) => format!("{host}|{p}|"),
            None => format!("{host}||{}", self.name.to_ascii_lowercase()),
        }
    }
}

/// Insert `svc` keeping the highest-precedence source on a key collision.
fn upsert(map: &mut BTreeMap<DedupKey, UnifiedService>, svc: UnifiedService) {
    let key = svc.dedup_key();
    match map.get(&key) {
        // An equal-or-higher-precedence row already holds the key — keep it.
        Some(existing) if existing.source.precedence() <= svc.source.precedence() => {}
        _ => {
            map.insert(key, svc);
        }
    }
}

/// SVC-VIEW — the pure union/dedup of the three service surfaces (unit-tested).
/// Reuses the existing reader output types verbatim (no re-read): the published
/// `ServiceRow`s, the probe `HostInventory`, and the compute `Instance`s. Each
/// row is tagged with its host; duplicates (same host+port+proto across
/// surfaces) collapse to the highest-precedence source; the result is sorted by
/// host, then port, then name for a stable render.
#[must_use]
pub fn unify_services(
    published: &[service_publishing::ServiceRow],
    probe: &network_hosts::HostInventory,
    vms: &[compute::Instance],
) -> Vec<UnifiedService> {
    let mut by_key: BTreeMap<DedupKey, UnifiedService> = BTreeMap::new();

    // 1) Canonical published services (highest precedence — authoritative id).
    for r in published {
        let host = if r.node.is_empty() {
            "this node".to_string()
        } else {
            r.node.clone()
        };
        let detail = match r.overlay_ip.as_deref() {
            Some(ip) if !ip.is_empty() => format!("id {} · {ip}", r.id),
            _ => format!("id {}", r.id),
        };
        upsert(
            &mut by_key,
            UnifiedService {
                host,
                name: r.name.clone(),
                detail,
                port: Some(r.port),
                proto: r.proto.clone(),
                source: ServiceSource::Published,
                reachable: r.is_publishable,
            },
        );
    }

    // 2) Probe-discovered open ports.
    for h in &probe.hosts {
        let host = if h.display.is_empty() {
            h.ip.clone()
        } else {
            h.display.clone()
        };
        for s in &h.services {
            let name = if !s.product.is_empty() {
                s.product.clone()
            } else if !s.kind.is_empty() {
                s.kind.clone()
            } else {
                format!("port {}", s.port)
            };
            let mut detail = format!("{} · {}", h.source, s.kind);
            if !h.trust.is_empty() {
                detail.push_str(&format!(" · {}", h.trust));
            }
            upsert(
                &mut by_key,
                UnifiedService {
                    host: host.clone(),
                    name,
                    detail,
                    port: Some(s.port),
                    proto: String::new(),
                    source: ServiceSource::Probe,
                    reachable: true, // an open port the probe saw is, by definition, up
                },
            );
        }
    }

    // 3) VM/container workloads (VM-internal where available).
    for inst in vms {
        let host = if inst.node.is_empty() {
            "this node".to_string()
        } else {
            inst.node.clone()
        };
        upsert(
            &mut by_key,
            UnifiedService {
                host,
                name: inst.name.clone(),
                detail: format!("{} · {}", inst.kind.label(), inst.state),
                port: None,
                proto: String::new(),
                source: ServiceSource::Vm,
                reachable: compute::state_is_running(&inst.state),
            },
        );
    }

    let mut out: Vec<UnifiedService> = by_key.into_values().collect();
    out.sort_by(|a, b| {
        a.host
            .to_ascii_lowercase()
            .cmp(&b.host.to_ascii_lowercase())
            .then(a.port.unwrap_or(0).cmp(&b.port.unwrap_or(0)))
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

#[derive(Debug, Clone, Default)]
pub struct ServicesMapPanel {
    pub rows: Vec<UnifiedService>,
    pub busy: bool,
    pub last_run_at: Option<SystemTime>,
    /// Operator-facing status — the per-source counts, or the failure mode.
    pub last_op: String,
    /// MOTION-TRANS-3 — the keyed-diff reveal: a service that just appeared on a
    /// refresh slides up + fades into the list, while rows already on screen stay
    /// put (the list doesn't restroke). Keyed by [`UnifiedService::row_key`].
    reveal: mde_theme::animation::KeyedListReveal,
}

/// MOTION-TRANS-3 — stable scrollable id so the unified-services list keeps its
/// scroll position across a refresh (the list doesn't jump on add/remove).
const LIST_ID: &str = "services-map-list";

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Vec<UnifiedService>),
    RefreshClicked,
    /// MOTION-TRANS-3 — advance the row-insert reveal one frame (in-flight-only
    /// tick; GC's settled reveals so it self-stops at rest).
    AnimTick,
}

impl ServicesMapPanel {
    #[must_use]
    pub fn new() -> Self {
        Self {
            reveal: mde_theme::animation::KeyedListReveal::new(crate::live_theme::reduce_motion()),
            ..Self::default()
        }
    }

    /// MOTION-TRANS-3 — does the row-insert reveal still have a frame to draw? The
    /// app gates the panel's per-frame tick subscription on this (idle ⇒ no tick).
    #[must_use]
    pub fn needs_tick(&self, now: Instant) -> bool {
        !self.reveal.is_idle(now)
    }

    /// Gather the three surfaces + union them. The two blocking readers ride
    /// `spawn_blocking` (they build their own current-thread runtimes — the same
    /// nested-runtime contract every other panel honours); `compute::enumerate`
    /// is already async.
    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async {
                let published = tokio::task::spawn_blocking(service_publishing::fetch_summary)
                    .await
                    .map(|(rows, _err)| rows)
                    .unwrap_or_default();
                let probe = tokio::task::spawn_blocking(network_hosts::fetch_inventory)
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .unwrap_or_default();
                let vms = compute::enumerate().await.instances;
                unify_services(&published, &probe, &vms)
            },
            |rows| crate::Message::ServicesMap(Message::Loaded(rows)),
        )
    }

    pub fn update(&mut self, msg: Message) -> Task<crate::Message> {
        match msg {
            Message::Loaded(rows) => {
                // MOTION-TRANS-3 — diff the freshly-unioned roster against the last
                // frame so any newly-appeared service reveals in (the first load is
                // treated as the list appearing — no mass reveal).
                let now = Instant::now();
                self.reveal
                    .sync(rows.iter().map(UnifiedService::row_key), now);
                self.reveal.gc(now);
                self.last_op = summarise(&rows);
                self.rows = rows;
                self.busy = false;
                self.last_run_at = Some(SystemTime::now());
                Task::none()
            }
            // MOTION-TRANS-3 — drop settled reveals so the tick subscription stops.
            Message::AnimTick => {
                self.reveal.gc(Instant::now());
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

        let title = text("Services Across the Mesh")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());

        let subtitle_text = if !self.last_op.is_empty() {
            self.last_op.clone()
        } else if let Some(t) = self.last_run_at {
            format!("last refresh {}", fmt_age(t))
        } else {
            "click Refresh to union published + probed + VM services".into()
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
        .on_press(crate::Message::ServicesMap(Message::RefreshClicked));

        let header = row![
            column![title, subtitle].spacing(2),
            Space::new().width(Length::Fill),
            refresh_btn,
        ]
        .align_y(cosmic::iced::alignment::Vertical::Center);

        let rows_widget: Element<'_, crate::Message> = if self.rows.is_empty() {
            empty_state(palette)
        } else {
            // MOTION-TRANS-3 — a single clock read for this frame's reveal sampling.
            let now = Instant::now();
            let mut col = column![].spacing(6);
            for r in &self.rows {
                // A freshly-inserted row starts a few px low and rises to rest,
                // applied as decaying top padding (iced 0.13 has no transform
                // widget — the translate-as-padding idiom). `0` once settled, so
                // the resting layout is unchanged.
                let slide = self
                    .reveal
                    .row_params(&r.row_key(), now)
                    .translate_y
                    .max(0.0);
                col = col.push(container(unified_row_view(r, palette)).padding(Padding {
                    top: slide,
                    right: 0.0,
                    bottom: 0.0,
                    left: 0.0,
                }));
            }
            scrollable(col)
                .id(cosmic::iced::widget::Id::new(LIST_ID))
                .height(Length::FillPortion(1))
                .into()
        };

        container(
            column![
                header,
                Space::new().height(Length::Fixed(20.0)),
                rows_widget
            ]
            .spacing(2),
        )
        .padding(Padding::from([24u16, 32u16]))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

/// The per-source counts summary line.
#[must_use]
fn summarise(rows: &[UnifiedService]) -> String {
    let (mut pub_n, mut probe_n, mut vm_n) = (0u32, 0u32, 0u32);
    for r in rows {
        match r.source {
            ServiceSource::Published => pub_n += 1,
            ServiceSource::Probe => probe_n += 1,
            ServiceSource::Vm => vm_n += 1,
        }
    }
    let hosts = rows
        .iter()
        .map(|r| r.host.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    format!(
        "{} services across {hosts} host(s) — {pub_n} published · {probe_n} probed · {vm_n} vm/container",
        rows.len()
    )
}

fn empty_state<'a>(palette: Palette) -> Element<'a, crate::Message> {
    container(
        column![
            text("No services discovered yet")
                .size(13)
                .colr(palette.text.into_cosmic_color()),
            Space::new().height(Length::Fixed(6.0)),
            text(
                "This view unions the canonical Published Services, the nmap \
                 probe inventory (Discovered Hosts), and the VM/container compute \
                 inventory — one truthful place for every service on the mesh. \
                 Empty means no peers are enrolled and no probe has run yet — \
                 click Refresh once the mesh is up."
            )
            .size(12)
            .colr(palette.text_muted.into_cosmic_color()),
        ]
        .spacing(2),
    )
    .padding(Padding::from([18u16, 22u16]))
    .width(Length::Fill)
    .sty(move |_| container::Style {
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

fn pill<'a>(label: &'a str, color: Color) -> Element<'a, crate::Message> {
    container(text(label.to_string()).size(10).colr(Color::WHITE))
        .padding(Padding::from([2u16, 8u16]))
        .sty(move |_| container::Style {
            snap: false,
            background: Some(Background::Color(color)),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 10.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

fn unified_row_view<'a>(r: &UnifiedService, palette: Palette) -> Element<'a, crate::Message> {
    // Source tag — accent for the authoritative published rows, a muted overlay
    // chip for probe/VM (all Carbon palette tokens, no raw hex).
    let source_color = match r.source {
        ServiceSource::Published => palette.accent.into_cosmic_color(),
        ServiceSource::Probe => palette.text_muted.into_cosmic_color(),
        ServiceSource::Vm => palette.overlay.into_cosmic_color(),
    };
    // Reachable/running status pill.
    let (status_label, status_color) = if r.reachable {
        ("up", palette.success.into_cosmic_color())
    } else {
        ("down", palette.warning.into_cosmic_color())
    };

    let port_proto = match (r.port, r.proto.as_str()) {
        (Some(p), proto) if !proto.is_empty() => format!("{p}/{proto}"),
        (Some(p), _) => p.to_string(),
        (None, _) => "—".to_string(),
    };

    let bg = palette.raised.into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    container(
        row![
            column![
                text(r.name.clone())
                    .size(13)
                    .colr(palette.text.into_cosmic_color()),
                text(r.detail.clone())
                    .size(10)
                    .colr(palette.text_muted.into_cosmic_color()),
            ]
            .spacing(2)
            .width(Length::FillPortion(3)),
            // The host tag — every row carries its host.
            text(r.host.clone())
                .size(12)
                .colr(palette.text.into_cosmic_color())
                .width(Length::FillPortion(2)),
            text(port_proto)
                .size(12)
                .colr(palette.text.into_cosmic_color())
                .width(Length::FillPortion(1)),
            pill(r.source.label(), source_color),
            pill(status_label, status_color),
        ]
        .spacing(12)
        .align_y(cosmic::iced::alignment::Vertical::Center),
    )
    .padding(Padding::from([10u16, 16u16]))
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

fn fmt_age(t: SystemTime) -> String {
    use std::time::Duration;
    let Ok(elapsed) = t.elapsed() else {
        return "—".into();
    };
    let secs = elapsed.as_secs();
    if elapsed < Duration::from_secs(60) {
        format!("{secs} s ago")
    } else if elapsed < Duration::from_secs(3600) {
        format!("{} min ago", secs / 60)
    } else if elapsed < Duration::from_secs(86_400) {
        format!("{} h ago", secs / 3600)
    } else {
        format!("{} d ago", secs / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panels::compute::{Instance, InstanceKind};
    use crate::panels::network_hosts::{HostInventory, HostRow, ServiceRow as ProbeServiceRow};
    use crate::panels::service_publishing::ServiceRow as PublishedServiceRow;

    fn published(
        node: &str,
        id: &str,
        name: &str,
        port: u16,
        proto: &str,
        ok: bool,
    ) -> PublishedServiceRow {
        PublishedServiceRow {
            node: node.to_string(),
            id: id.to_string(),
            name: name.to_string(),
            port,
            proto: proto.to_string(),
            overlay_ip: Some("10.42.0.9".to_string()),
            is_publishable: ok,
        }
    }

    fn probe_host(display: &str, ip: &str, ports: &[(u16, &str, &str)]) -> HostRow {
        HostRow {
            display: display.to_string(),
            ip: ip.to_string(),
            source: "LAN".to_string(),
            trust: "trusted".to_string(),
            services: ports
                .iter()
                .map(|(port, kind, product)| ProbeServiceRow {
                    port: *port,
                    kind: kind.to_string(),
                    product: product.to_string(),
                })
                .collect(),
        }
    }

    fn vm(name: &str, kind: InstanceKind, state: &str, node: &str) -> Instance {
        Instance {
            name: name.to_string(),
            kind,
            state: state.to_string(),
            node: node.to_string(),
            local: false,
        }
    }

    #[test]
    fn unions_all_three_sources_tagged_by_host() {
        let pubs = vec![published("node-a", "ssh", "SSH", 22, "tcp", true)];
        let probe = HostInventory {
            hosts: vec![probe_host(
                "node-b",
                "172.20.0.2",
                &[(4040, "http", "Airsonic")],
            )],
        };
        let vms = vec![vm(
            "airsonic-ctr",
            InstanceKind::Container,
            "running",
            "node-c",
        )];

        let rows = unify_services(&pubs, &probe, &vms);
        assert_eq!(rows.len(), 3, "one row per distinct service");
        // Every row carries its host tag.
        assert!(rows
            .iter()
            .any(|r| r.host == "node-a" && r.source == ServiceSource::Published));
        assert!(rows
            .iter()
            .any(|r| r.host == "node-b" && r.source == ServiceSource::Probe));
        assert!(rows
            .iter()
            .any(|r| r.host == "node-c" && r.source == ServiceSource::Vm));
        // The VM/container row is portless + carries its running state.
        let ctr = rows.iter().find(|r| r.source == ServiceSource::Vm).unwrap();
        assert_eq!(ctr.port, None);
        assert!(ctr.reachable, "a running container is up");
        assert!(ctr.detail.contains("running"));
    }

    #[test]
    fn dedups_same_host_port_keeping_published() {
        // SSH:22 on node-a is BOTH a canonical published row AND found open by the
        // probe on the same host — must collapse to one row, the published one.
        let pubs = vec![published("node-a", "ssh", "SSH", 22, "tcp", true)];
        let probe = HostInventory {
            hosts: vec![probe_host("node-a", "10.42.0.9", &[(22, "ssh", "OpenSSH")])],
        };
        let rows = unify_services(&pubs, &probe, &[]);
        assert_eq!(rows.len(), 1, "duplicate host+port collapses");
        assert_eq!(rows[0].source, ServiceSource::Published, "published wins");
        assert_eq!(rows[0].name, "SSH");
    }

    #[test]
    fn distinct_ports_on_same_host_are_kept() {
        let pubs = vec![published("node-a", "ssh", "SSH", 22, "tcp", true)];
        let probe = HostInventory {
            hosts: vec![probe_host("node-a", "10.42.0.9", &[(80, "http", "nginx")])],
        };
        let rows = unify_services(&pubs, &probe, &[]);
        assert_eq!(
            rows.len(),
            2,
            "different ports on the same host are distinct"
        );
        // Sorted by host then port: 22 before 80.
        assert_eq!(rows[0].port, Some(22));
        assert_eq!(rows[1].port, Some(80));
    }

    #[test]
    fn probe_only_host_appears() {
        let probe = HostInventory {
            hosts: vec![probe_host(
                "airsonic-host",
                "172.20.0.2",
                &[(4040, "http", "Airsonic")],
            )],
        };
        let rows = unify_services(&[], &probe, &[]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].host, "airsonic-host");
        assert_eq!(rows[0].port, Some(4040));
        assert_eq!(rows[0].name, "Airsonic");
        assert_eq!(rows[0].source, ServiceSource::Probe);
    }

    #[test]
    fn stopped_vm_is_marked_down() {
        let vms = vec![vm("idle-vm", InstanceKind::Vm, "shut off", "node-x")];
        let rows = unify_services(&[], &HostInventory::default(), &vms);
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].reachable, "a shut-off VM is down");
        assert!(rows[0].detail.contains("VM"));
    }

    #[test]
    fn rows_sorted_by_host_then_port() {
        let pubs = vec![
            published("zeta", "ssh", "SSH", 22, "tcp", true),
            published("alpha", "nats", "NATS", 4222, "tcp", true),
            published("alpha", "ssh", "SSH", 22, "tcp", true),
        ];
        let rows = unify_services(&pubs, &HostInventory::default(), &[]);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].host, "alpha");
        assert_eq!(rows[0].port, Some(22)); // ssh before nats on alpha
        assert_eq!(rows[1].host, "alpha");
        assert_eq!(rows[1].port, Some(4222));
        assert_eq!(rows[2].host, "zeta");
    }

    #[test]
    fn empty_sources_yield_no_rows() {
        assert!(unify_services(&[], &HostInventory::default(), &[]).is_empty());
    }

    #[test]
    fn summarise_counts_per_source_and_hosts() {
        let rows = vec![
            UnifiedService {
                host: "a".into(),
                name: "SSH".into(),
                detail: String::new(),
                port: Some(22),
                proto: "tcp".into(),
                source: ServiceSource::Published,
                reachable: true,
            },
            UnifiedService {
                host: "b".into(),
                name: "http".into(),
                detail: String::new(),
                port: Some(80),
                proto: String::new(),
                source: ServiceSource::Probe,
                reachable: true,
            },
        ];
        let s = summarise(&rows);
        assert!(s.contains("2 services"));
        assert!(s.contains("2 host"));
        assert!(s.contains("1 published"));
        assert!(s.contains("1 probed"));
    }

    #[test]
    fn view_renders_empty_without_panic() {
        let p = ServicesMapPanel::new();
        let _ = p.view();
    }

    #[test]
    fn view_renders_rows_without_panic() {
        let mut p = ServicesMapPanel::new();
        p.rows = unify_services(
            &[published("node-a", "ssh", "SSH", 22, "tcp", true)],
            &HostInventory {
                hosts: vec![probe_host(
                    "node-b",
                    "172.20.0.2",
                    &[(4040, "http", "Airsonic")],
                )],
            },
            &[vm(
                "airsonic-ctr",
                InstanceKind::Container,
                "running",
                "node-c",
            )],
        );
        let _ = p.view();
    }

    #[test]
    fn update_loaded_clears_busy_and_summarises() {
        let mut p = ServicesMapPanel::new();
        p.busy = true;
        let _ = p.update(Message::Loaded(unify_services(
            &[published("node-a", "ssh", "SSH", 22, "tcp", true)],
            &HostInventory::default(),
            &[],
        )));
        assert!(!p.busy);
        assert!(p.last_op.contains("1 service"));
        assert!(p.last_run_at.is_some());
    }

    /// A unified row keyed by its host+port (the reveal key).
    fn svc(host: &str, port: u16) -> UnifiedService {
        UnifiedService {
            host: host.to_string(),
            name: format!("svc-{port}"),
            detail: String::new(),
            port: Some(port),
            proto: "tcp".to_string(),
            source: ServiceSource::Published,
            reachable: true,
        }
    }

    #[test]
    fn first_load_seeds_without_revealing_then_an_insert_reveals() {
        // MOTION-TRANS-3 — opening the panel (first Loaded) seeds the roster with
        // no mass reveal; a later refresh that adds a service reveals it, so the
        // panel reports it needs a per-frame tick.
        let mut p = ServicesMapPanel::new();
        let now = Instant::now();
        let _ = p.update(Message::Loaded(vec![svc("a", 22), svc("a", 80)]));
        assert!(
            !p.needs_tick(now),
            "the initial roster must not mass-reveal (no tick needed)"
        );
        let _ = p.update(Message::Loaded(vec![
            svc("a", 22),
            svc("a", 80),
            svc("b", 443),
        ]));
        assert!(
            p.needs_tick(Instant::now()),
            "an inserted service reveals → the panel needs a tick"
        );
    }

    #[test]
    fn anim_tick_settles_the_reveal_and_stops_the_clock() {
        // MOTION-TRANS-3 — once the reveal's panel-mount duration has elapsed, an
        // AnimTick GC's it and the panel goes idle (the subscription stops).
        let mut p = ServicesMapPanel::new();
        let _ = p.update(Message::Loaded(vec![svc("a", 22)]));
        let _ = p.update(Message::Loaded(vec![svc("a", 22), svc("b", 443)]));
        let _ = p.update(Message::AnimTick);
        let later = Instant::now() + mde_theme::motion::Motion::panel_mount().duration;
        assert!(
            !p.needs_tick(later),
            "a settled reveal needs no further ticks"
        );
    }
}
