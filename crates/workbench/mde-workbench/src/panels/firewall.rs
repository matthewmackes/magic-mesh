//! Network → Firewall panel — firewalld via `firewall-cmd`.
//!
//! CB-1.8 partial: replaces the v1.x
//! `mackes/workbench/network/firewall.py`. Two controls:
//! a default-zone pick_list + a per-service toggle for the
//! enabled service set. Reads via `firewall-cmd --get-…` /
//! `--list-…`; writes via `pkexec firewall-cmd …` for the
//! state-change paths (permanent + reload).
//!
//! FWMON-5 (v5.0.0) adds an Activity section that reads the
//! union of `<mesh-storage>/firewall/*.jsonl` written by
//! `mackesd::firewall_monitor` and shows recent denials, the
//! top offending sources, and per-peer denial counts.

use cosmic::iced::widget::{checkbox, column, container, pick_list, row, scrollable, text};
use cosmic::iced::{Element, Length, Task};

use crate::controls::{variant_button, ButtonVariant};
use tokio::process::Command;

/// Firewall JSONL subdirectory under mesh-storage.
pub const FIREWALL_SUBDIR: &str = "firewall";

/// How many recent events to display in the Activity section.
pub const ACTIVITY_DISPLAY_LIMIT: usize = 20;

/// How many top sources to display in the rollup.
pub const TOP_SOURCES_LIMIT: usize = 5;

/// Curated list of common firewalld services the panel exposes
/// as per-row toggles. Matches the canonical set the v1.x
/// Python panel rendered; users with custom services can still
/// edit them via `firewall-cmd` directly.
pub const COMMON_SERVICES: &[&str] = &[
    "ssh",
    "http",
    "https",
    "dhcpv6-client",
    "mdns",
    "samba-client",
    "cockpit",
    "vnc-server",
];

/// Partial deserialization of one `firewall_monitor` JSONL record.
/// Extra fields are ignored so future schema additions don't break
/// the panel.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct DeniedRecord {
    pub ts_ms: i64,
    pub host: String,
    pub src_ip: String,
    pub dport: u16,
    pub proto: String,
    #[serde(default)]
    pub iface: String,
}

#[derive(Debug, Clone, Default)]
pub struct FirewallPanel {
    pub firewalld_available: bool,
    pub zones: Vec<String>,
    pub default_zone: String,
    pub enabled_services: Vec<String>,
    pub status: String,
    pub busy: bool,
    /// All denied-packet records from the mesh-storage union.
    pub activity_events: Vec<DeniedRecord>,
    /// True once the first `ActivityLoaded` has arrived.
    pub activity_loaded: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded {
        firewalld_available: bool,
        zones: Vec<String>,
        default_zone: String,
        enabled_services: Vec<String>,
    },
    ActivityLoaded {
        events: Vec<DeniedRecord>,
    },
    Error(String),
    DefaultZoneSelected(String),
    ServiceToggled {
        service: String,
        enable: bool,
    },
    OperationFinished(Result<String, String>),
    RefreshClicked,
}

impl FirewallPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move {
                let version = run_firewall_cmd(&["--version"]).await;
                let firewalld_available = !version.is_empty();
                if !firewalld_available {
                    return Message::Loaded {
                        firewalld_available,
                        zones: Vec::new(),
                        default_zone: String::new(),
                        enabled_services: Vec::new(),
                    };
                }
                let zones_raw = run_firewall_cmd(&["--get-zones"]).await;
                let default_zone = run_firewall_cmd(&["--get-default-zone"]).await;
                let services_raw = run_firewall_cmd(&["--list-services"]).await;
                Message::Loaded {
                    firewalld_available,
                    zones: parse_space_separated(&zones_raw),
                    default_zone: default_zone.trim().to_string(),
                    enabled_services: parse_space_separated(&services_raw),
                }
            },
            crate::Message::Firewall,
        )
    }

    /// FWMON-5: Load the union of `<mesh-storage>/firewall/*.jsonl` files
    /// written by `mackesd::firewall_monitor` on every peer.
    pub fn load_activity() -> Task<crate::Message> {
        Task::perform(
            async move {
                // Single-sourced with `mackesd` so the union is read from
                // the real mount (`~/QNM-Shared`), not a phantom path.
                let root = mackes_mesh_types::peers::default_workgroup_root();
                let events = read_activity_jsonl(&root.to_string_lossy(), FIREWALL_SUBDIR);
                Message::ActivityLoaded { events }
            },
            crate::Message::Firewall,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded {
                firewalld_available,
                zones,
                default_zone,
                enabled_services,
            } => {
                self.firewalld_available = firewalld_available;
                self.zones = zones;
                self.default_zone = if self.zones.contains(&default_zone) {
                    default_zone
                } else {
                    self.zones.first().cloned().unwrap_or_default()
                };
                self.enabled_services = enabled_services;
                self.status.clear();
                self.busy = false;
                Self::load_activity()
            }
            Message::ActivityLoaded { events } => {
                self.activity_events = events;
                self.activity_loaded = true;
                Task::none()
            }
            Message::Error(msg) => {
                self.status = msg;
                self.busy = false;
                Task::none()
            }
            Message::DefaultZoneSelected(zone) => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.default_zone = zone.clone();
                self.status = format!("Setting default zone to {zone} (polkit will prompt)…");
                Task::perform(
                    async move { Message::OperationFinished(set_default_zone(&zone).await) },
                    crate::Message::Firewall,
                )
            }
            Message::ServiceToggled { service, enable } => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = format!(
                    "{} {service} (polkit will prompt)…",
                    if enable { "Enabling" } else { "Disabling" },
                );
                Task::perform(
                    async move { Message::OperationFinished(toggle_service(&service, enable).await) },
                    crate::Message::Firewall,
                )
            }
            Message::OperationFinished(result) => {
                self.busy = false;
                self.status = match result {
                    Ok(msg) => msg,
                    Err(msg) => msg,
                };
                // Reload to reflect the new state.
                Self::load()
            }
            Message::RefreshClicked => {
                if self.busy {
                    return Task::none();
                }
                self.busy = true;
                self.status = "Refreshing…".into();
                Task::batch([Self::load(), Self::load_activity()])
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message, cosmic::Theme> {
        if !self.firewalld_available {
            return column![
                text("firewalld unavailable").size(18),
                text(
                    "MDE talks to the firewall through `firewall-cmd`. \
                     Install firewalld and ensure the service is running, \
                     then refresh this panel.",
                )
                .size(13),
            ]
            .spacing(8)
            .width(Length::Fill)
            .into();
        }

        // UX-7.a — refresh routed through the shared button variant.
        let refresh_btn = variant_button(
            "Refresh",
            ButtonVariant::Ghost,
            (!self.busy).then(|| crate::Message::Firewall(Message::RefreshClicked)),
            crate::live_theme::palette(),
        );

        let zone_pick: pick_list::PickList<'_, String, _, _, crate::Message, cosmic::Theme> =
            pick_list(
                self.zones.clone(),
                current_or_none(&self.zones, &self.default_zone),
                |v| crate::Message::Firewall(Message::DefaultZoneSelected(v)),
            );

        let service_rows = COMMON_SERVICES.iter().fold(column![], |col, service| {
            let svc = (*service).to_string();
            let is_on = self.enabled_services.iter().any(|s| s == service);
            let busy = self.busy;
            let cb = checkbox(is_on).label(*service).on_toggle(move |enable| {
                let _ = busy;
                crate::Message::Firewall(Message::ServiceToggled {
                    service: svc.clone(),
                    enable,
                })
            });
            col.push(cb)
        });

        // FWMON-5: Activity section — recent denials, top sources,
        // per-peer counts from the mesh-storage firewall JSONL union.
        let activity_section: Element<'_, crate::Message, cosmic::Theme> = if !self.activity_loaded
        {
            text("Activity loading…").size(13).into()
        } else if self.activity_events.is_empty() {
            text("No firewall denials recorded. Denied packets appear here once firewalld LogDenied=all is active and external traffic is seen.").size(13).into()
        } else {
            let recent = recent_events(&self.activity_events, ACTIVITY_DISPLAY_LIMIT);
            let recent_rows = recent.iter().fold(column![].spacing(2), |col, e| {
                col.push(
                    text(format!(
                        "{:>5}/{:<4}  {}  →  port {}  ({})",
                        e.proto, e.dport, e.src_ip, e.dport, e.host,
                    ))
                    .size(12),
                )
            });

            let top = top_sources(&self.activity_events, TOP_SOURCES_LIMIT);
            let top_rows = top.iter().fold(column![].spacing(2), |col, (src, count)| {
                col.push(text(format!("{src}  ×{count}")).size(12))
            });

            let peer_counts = per_peer_counts(&self.activity_events);
            let peer_rows = peer_counts
                .iter()
                .fold(column![].spacing(2), |col, (host, count)| {
                    col.push(text(format!("{host}: {count}")).size(12))
                });

            column![
                text(format!(
                    "Recent denials ({} of {})",
                    recent.len(),
                    self.activity_events.len()
                ))
                .size(14),
                scrollable(container(recent_rows)).height(Length::Fixed(120.0)),
                text("Top sources").size(14),
                top_rows,
                text("Per-peer counts").size(14),
                peer_rows,
            ]
            .spacing(8)
            .into()
        };

        column![
            row![
                text("Default zone").width(Length::Fixed(180.0)),
                zone_pick,
                refresh_btn,
            ]
            .spacing(12),
            text("Services").size(16),
            scrollable(container(service_rows.spacing(4))).height(Length::Fixed(240.0)),
            text(format!(
                "{} service(s) enabled in zone {}",
                self.enabled_services.len(),
                self.default_zone,
            ))
            .size(13),
            text(&self.status).size(13),
            text("Activity").size(16),
            activity_section,
        ]
        .spacing(12)
        .width(Length::Fill)
        .into()
    }
}

/// Read all `*.jsonl` files from `<mount>/<subdir>/` and return
/// parsed records sorted by `ts_ms` descending (newest first).
/// Missing mount or empty dir returns an empty Vec (no error — the
/// mount may not be up yet).
pub fn read_activity_jsonl(mount: &str, subdir: &str) -> Vec<DeniedRecord> {
    let dir = std::path::Path::new(mount).join(subdir);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return vec![];
    };
    let mut records: Vec<DeniedRecord> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        for line in content.lines() {
            if let Ok(r) = serde_json::from_str::<DeniedRecord>(line) {
                records.push(r);
            }
        }
    }
    records.sort_by(|a, b| b.ts_ms.cmp(&a.ts_ms));
    records
}

/// N most recent events (already sorted newest-first by
/// `read_activity_jsonl`).
pub fn recent_events(events: &[DeniedRecord], n: usize) -> &[DeniedRecord] {
    &events[..n.min(events.len())]
}

/// Top `n` source IPs by denial count across all records.
pub fn top_sources(events: &[DeniedRecord], n: usize) -> Vec<(String, usize)> {
    let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for e in events {
        *counts.entry(e.src_ip.as_str()).or_insert(0) += 1;
    }
    let mut sorted: Vec<(String, usize)> = counts
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    sorted.truncate(n);
    sorted
}

/// Denial count per originating peer host.
pub fn per_peer_counts(events: &[DeniedRecord]) -> Vec<(String, usize)> {
    let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for e in events {
        *counts.entry(e.host.as_str()).or_insert(0) += 1;
    }
    let mut sorted: Vec<(String, usize)> = counts
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    sorted
}

fn current_or_none(list: &[String], value: &str) -> Option<String> {
    list.iter().find(|v| *v == value).cloned()
}

/// firewall-cmd's `--get-zones` / `--list-services` output is a
/// single line of whitespace-separated tokens. Empty input
/// produces an empty Vec.
#[must_use]
pub fn parse_space_separated(raw: &str) -> Vec<String> {
    raw.split_whitespace().map(String::from).collect()
}

/// Shell out to `firewall-cmd` with the given args. Returns
/// stdout on success; empty on failure (used as the
/// "unavailable" signal in the read paths).
pub async fn run_firewall_cmd(args: &[&str]) -> String {
    let Ok(output) = Command::new("firewall-cmd").args(args).output().await else {
        return String::new();
    };
    if !output.status.success() {
        return String::new();
    }
    String::from_utf8(output.stdout).unwrap_or_default()
}

/// Set the default zone. Returns a human-readable status
/// message on success or failure.
pub async fn set_default_zone(zone: &str) -> Result<String, String> {
    let success = run_pkexec_firewall_cmd(&["--set-default-zone", zone]).await;
    if success {
        Ok(format!("Default zone set to {zone}."))
    } else {
        Err(format!(
            "Setting default zone to {zone} failed (polkit cancelled or daemon down)."
        ))
    }
}

/// Add or remove a service from the default zone, permanent +
/// reload. firewalld's `--permanent` is needed so the change
/// survives a daemon restart; the `--reload` makes it active
/// immediately.
pub async fn toggle_service(service: &str, enable: bool) -> Result<String, String> {
    let flag = if enable {
        "--add-service"
    } else {
        "--remove-service"
    };
    let ok = run_pkexec_firewall_cmd(&[flag, service, "--permanent"]).await;
    if !ok {
        return Err(format!(
            "{} {service} failed (polkit cancelled or service unknown).",
            if enable { "Enabling" } else { "Disabling" },
        ));
    }
    let reload_ok = run_pkexec_firewall_cmd(&["--reload"]).await;
    if !reload_ok {
        return Err("Service updated but firewall-cmd --reload failed.".into());
    }
    Ok(format!(
        "{} {service}.",
        if enable { "Enabled" } else { "Disabled" },
    ))
}

async fn run_pkexec_firewall_cmd(args: &[&str]) -> bool {
    let mut argv = vec!["firewall-cmd"];
    argv.extend_from_slice(args);
    let Ok(output) = Command::new("pkexec").args(&argv).output().await else {
        return false;
    };
    output.status.success()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_record(host: &str, src_ip: &str, ts_ms: i64, dport: u16) -> DeniedRecord {
        DeniedRecord {
            ts_ms,
            host: host.into(),
            src_ip: src_ip.into(),
            dport,
            proto: "TCP".into(),
            iface: "eth0".into(),
        }
    }

    #[test]
    fn top_sources_returns_sorted_by_count_desc() {
        let records = vec![
            make_record("a", "1.1.1.1", 100, 22),
            make_record("a", "1.1.1.1", 200, 22),
            make_record("a", "2.2.2.2", 300, 22),
        ];
        let top = top_sources(&records, 5);
        assert_eq!(top[0].0, "1.1.1.1");
        assert_eq!(top[0].1, 2);
        assert_eq!(top[1].0, "2.2.2.2");
        assert_eq!(top[1].1, 1);
    }

    #[test]
    fn top_sources_truncates_to_n() {
        let records: Vec<_> = (0..10u8)
            .map(|i| make_record("h", &format!("{i}.0.0.1"), i as i64, 22))
            .collect();
        assert_eq!(top_sources(&records, 3).len(), 3);
    }

    #[test]
    fn per_peer_counts_aggregates_by_host() {
        let records = vec![
            make_record("peer-a", "1.1.1.1", 100, 22),
            make_record("peer-a", "2.2.2.2", 200, 22),
            make_record("peer-b", "3.3.3.3", 300, 22),
        ];
        let counts = per_peer_counts(&records);
        assert_eq!(counts[0], ("peer-a".into(), 2));
        assert_eq!(counts[1], ("peer-b".into(), 1));
    }

    #[test]
    fn recent_events_caps_at_n() {
        let records: Vec<_> = (0..30i64)
            .map(|i| make_record("h", "1.1.1.1", i * 1000, 22))
            .collect();
        assert_eq!(recent_events(&records, 10).len(), 10);
        assert_eq!(recent_events(&records, 100).len(), 30);
    }

    #[test]
    fn read_activity_jsonl_parses_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("firewall")).unwrap();
        let path = tmp.path().join("firewall/peer-a.jsonl");
        std::fs::write(
            &path,
            r#"{"ts_ms":2000,"host":"peer-a","src_ip":"1.2.3.4","dport":22,"proto":"TCP","iface":"eth0"}
{"ts_ms":1000,"host":"peer-a","src_ip":"5.6.7.8","dport":80,"proto":"TCP","iface":"eth0"}
"#,
        )
        .unwrap();
        let events = read_activity_jsonl(tmp.path().to_str().unwrap(), "firewall");
        assert_eq!(events.len(), 2);
        // newest first
        assert_eq!(events[0].ts_ms, 2000);
        assert_eq!(events[1].ts_ms, 1000);
    }

    #[test]
    fn read_activity_jsonl_skips_invalid_lines() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("firewall")).unwrap();
        let path = tmp.path().join("firewall/peer-x.jsonl");
        std::fs::write(&path, "not json\n{\"ts_ms\":5000,\"host\":\"h\",\"src_ip\":\"9.9.9.9\",\"dport\":22,\"proto\":\"TCP\",\"iface\":\"eth0\"}\n").unwrap();
        let events = read_activity_jsonl(tmp.path().to_str().unwrap(), "firewall");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].src_ip, "9.9.9.9");
    }

    #[test]
    fn read_activity_jsonl_missing_mount_returns_empty() {
        let events = read_activity_jsonl("/nonexistent/path/xyzzy", "firewall");
        assert!(events.is_empty());
    }

    #[test]
    fn activity_loaded_message_populates_state() {
        let mut panel = FirewallPanel::new();
        assert!(!panel.activity_loaded);
        let _ = panel.update(Message::ActivityLoaded {
            events: vec![make_record("h", "1.1.1.1", 999, 22)],
        });
        assert!(panel.activity_loaded);
        assert_eq!(panel.activity_events.len(), 1);
    }

    #[test]
    fn common_services_lock_is_eight_entries() {
        assert_eq!(COMMON_SERVICES.len(), 8);
        assert!(COMMON_SERVICES.contains(&"ssh"));
        assert!(COMMON_SERVICES.contains(&"https"));
    }

    #[test]
    fn parse_space_separated_handles_typical_input() {
        let raw = "FedoraServer FedoraWorkstation block dmz drop\n";
        let zones = parse_space_separated(raw);
        assert_eq!(zones.len(), 5);
        assert_eq!(zones[0], "FedoraServer");
        assert_eq!(zones[4], "drop");
    }

    #[test]
    fn parse_space_separated_collapses_runs_of_whitespace() {
        let raw = "  ssh    http https \t mdns  ";
        let services = parse_space_separated(raw);
        assert_eq!(services, vec!["ssh", "http", "https", "mdns"]);
    }

    #[test]
    fn parse_space_separated_empty_on_empty_or_whitespace() {
        assert!(parse_space_separated("").is_empty());
        assert!(parse_space_separated("   \n  \t  ").is_empty());
    }

    #[test]
    fn loaded_records_state_and_falls_back_to_first_zone_when_default_unknown() {
        let mut panel = FirewallPanel::new();
        let _ = panel.update(Message::Loaded {
            firewalld_available: true,
            zones: vec!["public".into(), "trusted".into()],
            default_zone: "vanished".into(),
            enabled_services: vec!["ssh".into(), "http".into()],
        });
        assert!(panel.firewalld_available);
        assert_eq!(panel.default_zone, "public");
        assert_eq!(panel.enabled_services, vec!["ssh", "http"]);
    }

    #[test]
    fn loaded_preserves_known_default_zone() {
        let mut panel = FirewallPanel::new();
        let _ = panel.update(Message::Loaded {
            firewalld_available: true,
            zones: vec!["public".into(), "trusted".into()],
            default_zone: "trusted".into(),
            enabled_services: vec![],
        });
        assert_eq!(panel.default_zone, "trusted");
    }

    #[test]
    fn loaded_firewalld_unavailable_clears_state() {
        let mut panel = FirewallPanel::new();
        let _ = panel.update(Message::Loaded {
            firewalld_available: false,
            zones: Vec::new(),
            default_zone: String::new(),
            enabled_services: Vec::new(),
        });
        assert!(!panel.firewalld_available);
    }

    #[test]
    fn default_zone_selected_while_busy_is_noop() {
        let mut panel = FirewallPanel::new();
        panel.busy = true;
        panel.default_zone = "public".into();
        let _ = panel.update(Message::DefaultZoneSelected("trusted".into()));
        assert_eq!(panel.default_zone, "public");
    }

    #[test]
    fn service_toggled_while_busy_is_noop() {
        let mut panel = FirewallPanel::new();
        panel.busy = true;
        panel.status = "Applying…".into();
        let _ = panel.update(Message::ServiceToggled {
            service: "ssh".into(),
            enable: true,
        });
        assert_eq!(panel.status, "Applying…");
    }

    #[test]
    fn operation_finished_ok_carries_status_and_clears_busy() {
        let mut panel = FirewallPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::OperationFinished(Ok("Enabled ssh.".into())));
        assert!(!panel.busy);
        assert_eq!(panel.status, "Enabled ssh.");
    }

    #[test]
    fn operation_finished_err_carries_error_and_clears_busy() {
        let mut panel = FirewallPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::OperationFinished(Err("polkit denied".into())));
        assert!(!panel.busy);
        assert_eq!(panel.status, "polkit denied");
    }

    #[test]
    fn refresh_clicked_while_busy_is_noop() {
        let mut panel = FirewallPanel::new();
        panel.busy = true;
        panel.status = "stale".into();
        let _ = panel.update(Message::RefreshClicked);
        assert_eq!(panel.status, "stale");
    }

    #[test]
    fn error_message_clears_busy_and_stores_msg() {
        let mut panel = FirewallPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::Error("firewall-cmd not found".into()));
        assert_eq!(panel.status, "firewall-cmd not found");
        assert!(!panel.busy);
    }
}
