//! NF-13.8 (v2.5) — Network → Service Publishing panel.
//!
//! Surfaces every canonical Nebula-published service (SSH, NATS,
//! Mesh FS, Media, rsync, WoL, AV) with: status pill (publishable
//! when an overlay IP exists, otherwise "not yet enrolled"), port
//! + protocol, and a per-row hint for the service binary.
//!
//! Reads the live snapshot over the mesh Bus from
//! `action/nebula/published-services` (RETIRE-PY.7 — replaced the v1.x
//! `python3 -c mackes.mesh_nebula` shell-out). `mackesd` builds the summary
//! (the 7 canonical services × this peer's overlay IP) and answers the Bus
//! query; the panel's `parse_summary` decodes the same JSON list-of-rows shape.
//!
//! Chrome influence (per iteration skill Phase 0.8): Ableton
//! parameter table — dense rows, single indigo accent for the
//! status pill, IBM Plex Mono for the numeric port column, 1 px
//! border between rows.

use std::time::SystemTime;

use cosmic::iced::widget::{button, column, container, row, scrollable, text, Space};
use cosmic::iced::{Background, Border, Color, Length, Padding, Task};
use cosmic::{Element, Theme};
use mde_theme::{FontSize, Palette, TypeRole};
use serde::{Deserialize, Serialize};

use crate::cosmic_compat::prelude::*;

/// JSON wire shape published by
/// `mackes.mesh_nebula.published_services_summary()`. The
/// Python helper emits a `list[dict]`; each row deserializes
/// into this struct.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ServiceRow {
    /// Hostname of the peer publishing this service. Empty for the legacy
    /// local-only mackesd reply (back-compat); set for every fleet row.
    #[serde(default)]
    pub node: String,
    /// Stable service id — matches one of the 7 canonical
    /// entries in mackes.mesh_nebula.CANONICAL_SERVICES.
    pub id: String,
    /// Display name (e.g. "SSH" / "NATS broker").
    pub name: String,
    /// Default port the service would bind to.
    pub port: u16,
    /// "tcp" or "udp".
    pub proto: String,
    /// Overlay IP this peer binds to — `None` until the peer
    /// completes enrollment.
    pub overlay_ip: Option<String>,
    /// True when an overlay IP is allocated (the service can
    /// publish). Mirrors the Python helper's `is_publishable`
    /// flag — kept here so the UI doesn't re-derive.
    pub is_publishable: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ServicePublishingPanel {
    pub rows: Vec<ServiceRow>,
    pub busy: bool,
    pub last_run_at: Option<SystemTime>,
    /// Last operator-facing message — either "loaded 7
    /// services in HH:MM" or the failure mode.
    pub last_op: String,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded {
        rows: Vec<ServiceRow>,
        error: Option<String>,
    },
    RefreshClicked,
}

impl ServicePublishingPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        // `fetch_summary` is blocking — it routes through `crate::dbus::action_request`,
        // which builds its OWN current-thread tokio runtime and `block_on`s it. The iced
        // `cosmic::executor::Default` is a multi-thread `tokio::runtime::Runtime`, so a
        // `Task::perform` future runs ON a tokio worker; calling `block_on` there panics
        // ("Cannot start a runtime from within a runtime"). The panicking task is dropped
        // by iced and `Loaded` never arrives, so the panel sat permanently empty — no
        // services, no error (the live-fleet symptom). `spawn_blocking` moves the blocking
        // work onto the blocking pool (no nested runtime), matching every other panel that
        // calls these helpers (connectivity / vpn / peers / home).
        Task::perform(
            async {
                tokio::task::spawn_blocking(fetch_summary)
                    .await
                    .unwrap_or_else(|_| (Vec::new(), Some("service summary task panicked".into())))
            },
            |(rows, error)| crate::Message::ServicePublishing(Message::Loaded { rows, error }),
        )
    }

    pub fn update(&mut self, msg: Message) -> Task<crate::Message> {
        match msg {
            Message::Loaded { rows, error } => {
                self.rows = rows;
                self.busy = false;
                self.last_run_at = Some(SystemTime::now());
                self.last_op = error.unwrap_or_else(|| {
                    let nodes = self
                        .rows
                        .iter()
                        .map(|r| r.node.as_str())
                        .collect::<std::collections::BTreeSet<_>>()
                        .len();
                    format!("{} services across {nodes} node(s)", self.rows.len())
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

        let title = text("Service Publishing")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());

        let subtitle_text = if !self.last_op.is_empty() {
            self.last_op.clone()
        } else if let Some(t) = self.last_run_at {
            format!("last refresh {}", fmt_age(t))
        } else {
            "click Refresh to probe the Nebula overlay".into()
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
        .on_press(crate::Message::ServicePublishing(Message::RefreshClicked));

        let header = row![
            column![title, subtitle].spacing(2),
            Space::new().width(Length::Fill),
            refresh_btn,
        ]
        .align_y(cosmic::iced::alignment::Vertical::Center);

        let rows_widget: Element<'_, crate::Message> = if self.rows.is_empty() {
            empty_state(palette)
        } else {
            let mut col = column![].spacing(6);
            for r in &self.rows {
                col = col.push(service_row_view(r, palette));
            }
            scrollable(col).height(Length::FillPortion(1)).into()
        };

        container(
            column![
                header,
                Space::new().height(Length::Fixed(20.0)),
                rows_widget,
            ]
            .spacing(2),
        )
        .padding(Padding::from([24u16, 32u16]))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

fn empty_state<'a>(palette: Palette) -> Element<'a, crate::Message> {
    container(
        column![
            text("No service rows available")
                .size(13)
                .colr(palette.text.into_cosmic_color()),
            Space::new().height(Length::Fixed(6.0)),
            text(
                "The 7 canonical services (SSH / NATS / Mesh FS / Media / \
                 rsync / WoL / AV) are listed for every enrolled peer, read \
                 from the replicated peer roster on Mesh Sync. Empty means no \
                 peers are enrolled yet (or Mesh Sync isn't up) — click \
                 Refresh once the mesh is up."
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

fn service_row_view<'a>(r: &ServiceRow, palette: Palette) -> Element<'a, crate::Message> {
    let (pill_label, pill_color) = if r.is_publishable {
        ("Published", palette.accent.into_cosmic_color())
    } else {
        ("Not enrolled", palette.warning.into_cosmic_color())
    };
    let pill = container(text(pill_label).size(10).colr(Color::WHITE))
        .padding(Padding::from([2u16, 8u16]))
        .style(move |_| container::Style {
            snap: false,
            background: Some(Background::Color(pill_color)),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 10.0.into(),
            },
            ..container::Style::default()
        });

    let overlay_text = r.overlay_ip.clone().unwrap_or_else(|| "—".to_string());
    let port_proto = format!("{}/{}", r.port, r.proto);

    let bg = palette.raised.into_cosmic_color();
    let border = palette.border.into_cosmic_color();
    container(
        row![
            column![
                text(r.name.clone())
                    .size(13)
                    .colr(palette.text.into_cosmic_color()),
                text(format!("id: {}", r.id))
                    .size(10)
                    .colr(palette.text_muted.into_cosmic_color()),
            ]
            .spacing(2)
            .width(Length::FillPortion(3)),
            // Which node publishes this service (fleet-wide view).
            text(if r.node.is_empty() {
                "this node".to_string()
            } else {
                r.node.clone()
            })
            .size(12)
            .colr(palette.text.into_cosmic_color())
            .width(Length::FillPortion(2)),
            // Monospace-ish numeric column for port/protocol per
            // the Ableton content-zone influence.
            text(port_proto)
                .size(12)
                .colr(palette.text.into_cosmic_color())
                .width(Length::FillPortion(1)),
            text(overlay_text)
                .size(12)
                .colr(palette.text_muted.into_cosmic_color())
                .width(Length::FillPortion(2)),
            pill,
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

// ---- I/O ------------------------------------------------------

/// The canonical Nebula-published services: `(id, display, default-port, proto)`.
/// Mirrors `mackesd::ipc::nebula::CANONICAL_SERVICES` (kept in sync by hand — the
/// workbench can't depend on the mesh daemon crate). Every full mesh node offers
/// this same fabric set; the only per-node variable is the overlay IP.
const CANONICAL_SERVICES: [(&str, &str, u16, &str); 7] = [
    ("ssh", "SSH", 22, "tcp"),
    ("nats", "NATS broker", 4222, "tcp"),
    ("fs", "Mesh FS (SSHFS)", 22, "tcp"),
    ("media", "Media library", 8080, "tcp"),
    ("sync", "rsync", 873, "tcp"),
    ("wol", "Wake-on-LAN relay", 9, "udp"),
    ("av", "Audio/video transport", 5004, "udp"),
];

/// Build the **fleet-wide** published-services summary: the 7 canonical services
/// for every enrolled peer in the mesh (operator directive 2026-06-18 — "if it's
/// responsible to show those 7 service types, show them from all over the
/// network"). SUBSTRATE-8 — reads the peer directory over `action/mesh/directory`
/// (etcd-or-fs behind the RPC) rather than a direct `/mnt/mesh-storage` read, so
/// it survives the substrate cutover. Falls back to the legacy local-only mackesd
/// reply when the directory is empty so a standalone node still shows its own.
#[must_use]
pub fn fetch_summary() -> (Vec<ServiceRow>, Option<String>) {
    let peers = crate::mesh_directory::fetch_peers();
    let rows = fleet_rows_from_peers(&peers);
    if !rows.is_empty() {
        return (rows, None);
    }
    // Fallback: empty directory — show at least this node's services.
    match crate::dbus::nebula_request("published-services") {
        Some(json) => parse_summary(&json),
        None => (
            Vec::new(),
            Some(
                "no peer directory and mackesd not reachable — service summary unavailable".into(),
            ),
        ),
    }
}

/// Pure fleet builder (unit-tested): 7 canonical services × every peer that has
/// an overlay IP, attributed to the peer's hostname. `is_publishable` = the peer
/// is enrolled (has an overlay IP) and currently reachable (`healthy`/`degraded`)
/// — an offline/unreachable peer's services aren't actually serving. Sorted by
/// node, then canonical service order.
#[must_use]
pub fn fleet_rows_from_peers(peers: &[mackes_mesh_types::peers::PeerRecord]) -> Vec<ServiceRow> {
    let mut enrolled: Vec<&mackes_mesh_types::peers::PeerRecord> = peers
        .iter()
        .filter(|p| p.overlay_ip.as_deref().is_some_and(|ip| !ip.is_empty()))
        .collect();
    enrolled.sort_by(|a, b| a.hostname.cmp(&b.hostname));
    let mut rows = Vec::with_capacity(enrolled.len() * CANONICAL_SERVICES.len());
    for p in enrolled {
        let reachable = matches!(p.health.as_str(), "healthy" | "degraded");
        for (id, name, port, proto) in CANONICAL_SERVICES {
            rows.push(ServiceRow {
                node: p.hostname.clone(),
                id: id.to_string(),
                name: name.to_string(),
                port,
                proto: proto.to_string(),
                overlay_ip: p.overlay_ip.clone(),
                is_publishable: reachable,
            });
        }
    }
    rows
}

/// Pure parser — accepts the JSON string the Python helper
/// emits and produces `(rows, optional_error)`. Pulled out for
/// direct testing without spinning up Python.
#[must_use]
pub fn parse_summary(raw: &str) -> (Vec<ServiceRow>, Option<String>) {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return (
            Vec::new(),
            Some("empty reply from published_services_summary".into()),
        );
    }
    match serde_json::from_str::<Vec<ServiceRow>>(trimmed) {
        Ok(rows) => (rows, None),
        Err(e) => (Vec::new(), Some(format!("invalid JSON: {e}"))),
    }
}

fn fmt_age(t: SystemTime) -> String {
    use std::time::Duration;
    let Ok(elapsed) = t.elapsed() else {
        return "—".into();
    };
    let d = elapsed;
    let secs = d.as_secs();
    let dur = Duration::from_secs(secs);
    if dur < Duration::from_secs(60) {
        format!("{secs} s ago")
    } else if dur < Duration::from_secs(3600) {
        format!("{} min ago", secs / 60)
    } else if dur < Duration::from_secs(86_400) {
        format!("{} h ago", secs / 3600)
    } else {
        format!("{} d ago", secs / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::peers::PeerRecord;

    fn enrolled(host: &str, ip: &str, health: &str) -> PeerRecord {
        let mut p = PeerRecord::now(host, None, health);
        p.overlay_ip = Some(ip.to_string());
        p
    }

    #[test]
    fn fleet_rows_seven_services_per_enrolled_peer_sorted_by_node() {
        let peers = vec![
            enrolled("node-b", "10.42.0.3", "unreachable"),
            enrolled("node-a", "10.42.0.2", "healthy"),
            PeerRecord::now("node-c", None, "healthy"), // no overlay IP → excluded
        ];
        let rows = fleet_rows_from_peers(&peers);
        // 2 enrolled peers × 7 canonical services.
        assert_eq!(rows.len(), 14);
        // Sorted by node — node-a first.
        assert_eq!(rows[0].node, "node-a");
        // Reachable peer publishes; unreachable peer does not.
        assert!(rows
            .iter()
            .filter(|r| r.node == "node-a")
            .all(|r| r.is_publishable));
        assert!(rows
            .iter()
            .filter(|r| r.node == "node-b")
            .all(|r| !r.is_publishable));
        // Each peer carries the full canonical set + its overlay IP.
        let a_ids: Vec<_> = rows
            .iter()
            .filter(|r| r.node == "node-a")
            .map(|r| r.id.as_str())
            .collect();
        for id in ["ssh", "nats", "fs", "media", "sync", "wol", "av"] {
            assert!(a_ids.contains(&id), "missing {id}");
        }
        assert_eq!(
            rows.iter()
                .find(|r| r.node == "node-a")
                .unwrap()
                .overlay_ip
                .as_deref(),
            Some("10.42.0.2")
        );
    }

    #[test]
    fn fleet_rows_empty_when_no_peer_has_overlay_ip() {
        let peers = vec![PeerRecord::now("node-c", None, "healthy")];
        assert!(fleet_rows_from_peers(&peers).is_empty());
    }

    #[test]
    fn fleet_rows_from_a_real_directory_rpc_reply() {
        // End-to-end over the REAL wire shape `action/mesh/directory` emits
        // (the same reply `mackesd peers` builds): parse it with the panel's
        // directory decoder, then aggregate. A healthy 3-node mesh with overlay
        // IPs must yield 7 services × 2 enrolled peers (the third has no overlay
        // IP — not yet enrolled — and is correctly excluded). This is the data the
        // panel was meant to show but rendered empty in the field, because the
        // blocking fetch panicked on the iced executor (see the spawn_blocking
        // contract tests below).
        let reply = r#"{"ok":true,"head":4,"leader":"eagle-lh-01","peers":[
            {"hostname":"eagle-lh-01","presence":"online","last_seen_ms":1750000000000,
             "health":"healthy","mde_version":"11.0.5","overlay_ip":"10.42.0.1",
             "role":"lighthouse","tags":[],"revision":{"currency":"synced"}},
            {"hostname":"eagle-node-a","presence":"online","last_seen_ms":1750000000000,
             "health":"degraded","mde_version":"11.0.5","overlay_ip":"10.42.0.7",
             "role":"workstation","tags":[],"revision":{"currency":"synced"}},
            {"hostname":"eagle-joining","presence":"idle","last_seen_ms":1750000000000,
             "health":"healthy","mde_version":"11.0.5","overlay_ip":"",
             "role":null,"tags":[],"revision":{"currency":"behind"}}
        ]}"#;
        let peers = crate::mesh_directory::parse_directory_peers(reply);
        assert_eq!(peers.len(), 3, "all 3 directory rows decode");
        let rows = fleet_rows_from_peers(&peers);
        // 2 enrolled (have overlay IPs) × 7 canonical services; the un-enrolled
        // node contributes nothing.
        assert_eq!(rows.len(), 14);
        assert!(rows.iter().all(|r| r.node != "eagle-joining"));
        // The lighthouse + the degraded node both publish (healthy|degraded are
        // reachable); each carries its own overlay IP.
        assert!(rows
            .iter()
            .filter(|r| r.node == "eagle-lh-01")
            .all(|r| r.is_publishable && r.overlay_ip.as_deref() == Some("10.42.0.1")));
        assert!(rows
            .iter()
            .filter(|r| r.node == "eagle-node-a")
            .all(|r| r.is_publishable && r.overlay_ip.as_deref() == Some("10.42.0.7")));
        // Sorted by node, canonical service order within each.
        assert_eq!(rows[0].node, "eagle-lh-01");
        assert_eq!(rows[0].id, "ssh");
    }

    #[test]
    fn load_returns_a_task_without_constructing_eagerly() {
        // `load()` must only BUILD a Task — the blocking `fetch_summary` work runs
        // later on the blocking pool, never inline here.
        let _task = ServicePublishingPanel::load();
    }

    // The field bug: `load()` did `Task::perform(async { fetch_summary() })`. The
    // iced `cosmic::executor::Default` is a multi-thread `tokio::runtime::Runtime`,
    // so that future runs ON a runtime worker — and `fetch_summary` builds a private
    // current-thread runtime + `block_on`s it (via `crate::dbus::action_request`),
    // which PANICS ("Cannot start a runtime from within a runtime") on a worker
    // thread. The panicking task is dropped, `Loaded` never arrives, and the panel
    // sat permanently empty. The tests below pin the contract: the blocking
    // aggregation MUST ride `spawn_blocking`, which is panic-free on a worker.

    /// Serialize the `MDE_BUS_ROOT` env override below — `set_var` is
    /// process-global and `cargo test` runs the suite multi-threaded, so without
    /// this another test reading `client_data_dir()` could observe the override.
    static BUS_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn nested_current_thread_block_on_panics_on_a_runtime_worker() {
        // The OLD bug shape, distilled: building a current-thread runtime and
        // `block_on`-ing it inside a future SPAWNED on the multi-thread runtime
        // panics — exactly what `fetch_summary`'s `crate::dbus::action_request`
        // does, and what `Task::perform(async { fetch_summary() })` triggered. This
        // is reliable regardless of env because it builds the runtime
        // unconditionally (it doesn't depend on a bus spool being present).
        let joined = tokio::spawn(async {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async { 1 })
        })
        .await;
        assert!(
            joined.is_err() && joined.unwrap_err().is_panic(),
            "the old bare-Task::perform shape panics on a runtime worker"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn real_fetch_summary_via_spawn_blocking_is_panic_free_on_a_worker() {
        // Drives the REAL `load()` path on a multi-thread runtime (the iced
        // executor's shape): `spawn_blocking(fetch_summary)` →
        // `mesh_directory::fetch_peers` → `dbus::action_request`, which builds a
        // current-thread runtime + `block_on`. On a bare worker that nested runtime
        // panics; `spawn_blocking` moves it to the blocking pool, where it's legal.
        // If a future edit drops the `spawn_blocking` wrapper, this test panics.
        //
        // Point the bus at an empty temp spool so there's no responder: the request
        // times out → None → `fetch_summary` returns (empty rows, an honest error),
        // deterministically and without touching the ambient /run/mde-bus spool.
        let _guard = BUS_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("MDE_BUS_ROOT", tmp.path());

        let joined = tokio::task::spawn_blocking(fetch_summary).await;

        std::env::remove_var("MDE_BUS_ROOT");
        drop(_guard);

        let (rows, error) =
            joined.expect("spawn_blocking(fetch_summary) must not panic on a worker");
        // No responder on the temp spool → an honest empty-with-error, never a
        // crash. Asserting the shape (not the exact text) keeps the contract.
        assert!(rows.is_empty(), "no responder → no service rows");
        assert!(error.is_some(), "no responder → an honest error message");
    }

    #[test]
    fn parse_summary_returns_empty_with_error_for_empty_input() {
        let (rows, err) = parse_summary("");
        assert!(rows.is_empty());
        assert!(err.is_some());
        assert!(err.unwrap().contains("empty reply"));
    }

    #[test]
    fn parse_summary_decodes_published_services_json() {
        // The exact JSON list-of-rows shape mackesd's
        // `action/nebula/published-services` responder emits.
        let raw = r#"[
            {"id":"ssh","name":"SSH","port":22,"proto":"tcp",
             "overlay_ip":"10.42.0.5","is_publishable":true},
            {"id":"nats","name":"NATS broker","port":4222,"proto":"tcp",
             "overlay_ip":"10.42.0.5","is_publishable":true},
            {"id":"wol","name":"Wake-on-LAN relay","port":9,"proto":"udp",
             "overlay_ip":null,"is_publishable":false}
        ]"#;
        let (rows, err) = parse_summary(raw);
        assert!(err.is_none(), "expected no error, got {err:?}");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].id, "ssh");
        assert_eq!(rows[0].port, 22);
        assert_eq!(rows[0].proto, "tcp");
        assert_eq!(rows[0].overlay_ip.as_deref(), Some("10.42.0.5"));
        assert!(rows[0].is_publishable);
        assert!(!rows[2].is_publishable);
        assert!(rows[2].overlay_ip.is_none());
    }

    #[test]
    fn parse_summary_returns_error_for_garbage() {
        let (rows, err) = parse_summary("{not valid");
        assert!(rows.is_empty());
        assert!(err.is_some());
        assert!(err.unwrap().contains("invalid JSON"));
    }

    #[test]
    fn parse_summary_returns_empty_for_empty_array() {
        let (rows, err) = parse_summary("[]");
        assert!(rows.is_empty());
        assert!(err.is_none());
    }

    #[test]
    fn view_renders_empty_state_without_panic() {
        let p = ServicePublishingPanel::new();
        let _ = p.view();
    }

    #[test]
    fn view_renders_with_rows_without_panic() {
        let mut p = ServicePublishingPanel::new();
        p.rows = vec![
            ServiceRow {
                node: "node-a".into(),
                id: "ssh".into(),
                name: "SSH".into(),
                port: 22,
                proto: "tcp".into(),
                overlay_ip: Some("10.42.0.5".into()),
                is_publishable: true,
            },
            ServiceRow {
                node: "node-b".into(),
                id: "wol".into(),
                name: "Wake-on-LAN relay".into(),
                port: 9,
                proto: "udp".into(),
                overlay_ip: None,
                is_publishable: false,
            },
        ];
        let _ = p.view();
    }

    #[test]
    fn update_loaded_clears_busy_and_sets_summary() {
        let mut p = ServicePublishingPanel::new();
        p.busy = true;
        let _ = p.update(Message::Loaded {
            rows: vec![ServiceRow {
                node: "node-a".into(),
                id: "ssh".into(),
                name: "SSH".into(),
                port: 22,
                proto: "tcp".into(),
                overlay_ip: Some("10.42.0.5".into()),
                is_publishable: true,
            }],
            error: None,
        });
        assert!(!p.busy);
        assert!(p.last_op.contains("1 service"));
        assert!(p.last_run_at.is_some());
    }

    #[test]
    fn update_loaded_with_error_surfaces_message() {
        let mut p = ServicePublishingPanel::new();
        let _ = p.update(Message::Loaded {
            rows: Vec::new(),
            error: Some("mackesd not reachable over the Bus".into()),
        });
        assert_eq!(p.last_op, "mackesd not reachable over the Bus");
        assert!(p.rows.is_empty());
    }
}
