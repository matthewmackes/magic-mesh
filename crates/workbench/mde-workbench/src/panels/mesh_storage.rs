//! MESHFS-13.1 (v5.0.0) — Workbench "Mesh Storage" panel.
//!
//! Shows per-peer chunkserver status (address, used/available bytes),
//! the current replication goal, the effective quota cap, and which
//! peer is the bottleneck. Data comes from `mackesd meshfs-status --json`.

use std::time::SystemTime;

use cosmic::iced::widget::{button, column, container, row, scrollable, text, Space};
use cosmic::iced::{Background, Border, Color, Length, Padding, Task};
use cosmic::{Element, Theme};
use mde_theme::hero::Hero;
use mde_theme::{FontSize, Palette, TypeRole};

use crate::cosmic_compat::prelude::*;
use crate::panel_chrome::{hero_band, pkg_version_cached};

fn human_bytes(b: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    if b < 1024 {
        return format!("{b} B");
    }
    let mut val = b as f64;
    let mut unit = 0usize;
    while val >= 1024.0 && unit < UNITS.len() - 1 {
        val /= 1024.0;
        unit += 1;
    }
    format!("{val:.1} {}", UNITS[unit])
}

#[derive(Debug, Clone)]
pub struct PeerRow {
    pub addr: String,
    pub used_bytes: u64,
    pub avail_bytes: u64,
}

#[derive(Debug, Clone, Default)]
pub struct StorageStatus {
    pub peers: Vec<PeerRow>,
    pub goal: usize,
    pub quota_cap_bytes: Option<u64>,
    pub limiting_peer_addr: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct MeshStoragePanel {
    pub status: StorageStatus,
    pub error: Option<String>,
    pub last_run_at: Option<SystemTime>,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<StorageStatus, String>),
    RefreshClicked,
}

impl MeshStoragePanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(async { fetch_status() }, |result| {
            crate::Message::MeshStorage(Message::Loaded(result))
        })
    }

    pub fn update(&mut self, msg: Message) -> Task<crate::Message> {
        match msg {
            Message::Loaded(Ok(status)) => {
                self.status = status;
                self.error = None;
                self.busy = false;
                self.last_run_at = Some(SystemTime::now());
                Task::none()
            }
            Message::Loaded(Err(e)) => {
                self.status = StorageStatus::default();
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

        let title = text("Mesh Storage")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());

        let subtitle_str = if let Some(t) = self.last_run_at {
            let age_s = t.elapsed().map(|d| d.as_secs()).unwrap_or(0);
            format!(
                "{} peer{} · goal {} · last refresh {}s ago",
                self.status.peers.len(),
                if self.status.peers.len() == 1 {
                    ""
                } else {
                    "s"
                },
                self.status.goal,
                age_s,
            )
        } else {
            "click Refresh to query the master".into()
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
                    text_color: Color::WHITE,
                    border: Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: 6.0.into(),
                    },
                    shadow: cosmic::iced::Shadow::default(),
                    ..cosmic::iced::widget::button::Style::default()
                }
            },
        )
        .on_press(crate::Message::MeshStorage(Message::RefreshClicked));

        // PLANES-2 — Mesh Storage is the LizardFS surface; carry its hero.
        let lizardfs = hero_band(
            Hero::LizardFs,
            pkg_version_cached("lizardfs-client").as_deref(),
            palette,
        );
        let header = row![
            title,
            Space::new().width(Length::Fill),
            refresh_btn,
            lizardfs
        ]
        .spacing(12)
        .align_y(cosmic::iced::Alignment::Center);
        let sub_row = row![subtitle];

        let body: Element<'_, crate::Message> = if let Some(ref e) = self.error {
            text(format!("Error: {e}"))
                .size(TypeRole::Body.size_in(sizes))
                .colr(Color {
                    r: 1.0,
                    g: 0.35,
                    b: 0.35,
                    a: 1.0,
                })
                .into()
        } else if self.status.peers.is_empty() && self.last_run_at.is_some() {
            text("Master unreachable — mesh-storage not yet active.")
                .size(TypeRole::Body.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color())
                .into()
        } else {
            let rows: Vec<Element<'_, crate::Message>> = self
                .status
                .peers
                .iter()
                .map(|p| peer_row(p, &self.status.limiting_peer_addr, palette, sizes))
                .collect();

            let quota_line = if let Some(cap) = self.status.quota_cap_bytes {
                format!(
                    "Quota cap: {} (0.8 × limiting peer avail)",
                    human_bytes(cap)
                )
            } else {
                String::new()
            };

            let limiting_line = self
                .status
                .limiting_peer_addr
                .as_deref()
                .map(|a| format!("Limiting peer: {a}"))
                .unwrap_or_default();

            let mut content_col = column(rows).spacing(4);
            if !quota_line.is_empty() {
                content_col = content_col.push(
                    text(quota_line)
                        .size(TypeRole::Caption.size_in(sizes))
                        .colr(palette.text_muted.into_cosmic_color()),
                );
            }
            if !limiting_line.is_empty() {
                content_col = content_col.push(
                    text(limiting_line)
                        .size(TypeRole::Caption.size_in(sizes))
                        .colr(palette.text_muted.into_cosmic_color()),
                );
            }
            scrollable(content_col).into()
        };

        let page = column![header, sub_row, Space::new().height(12), body].spacing(4);

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

fn peer_row<'a>(
    p: &'a PeerRow,
    limiting: &Option<String>,
    palette: Palette,
    sizes: FontSize,
) -> Element<'a, crate::Message> {
    let is_limiting = limiting.as_deref() == Some(p.addr.as_str());
    let addr_color = if is_limiting {
        Color {
            r: 1.0,
            g: 0.75,
            b: 0.3,
            a: 1.0,
        }
    } else {
        palette.text.into_cosmic_color()
    };
    let pct_used = if p.used_bytes + p.avail_bytes > 0 {
        format!(
            "{:.0}%",
            p.used_bytes as f64 / (p.used_bytes + p.avail_bytes) as f64 * 100.0
        )
    } else {
        "—".to_string()
    };
    let label = if is_limiting { " (limiting)" } else { "" };
    row![
        text(format!("{}{label}", p.addr))
            .size(TypeRole::Body.size_in(sizes))
            .colr(addr_color)
            .width(Length::FillPortion(4)),
        text(format!("used {}", human_bytes(p.used_bytes)))
            .size(TypeRole::Caption.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color())
            .width(Length::FillPortion(3)),
        text(format!("avail {}", human_bytes(p.avail_bytes)))
            .size(TypeRole::Caption.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color())
            .width(Length::FillPortion(3)),
        text(pct_used)
            .size(TypeRole::Caption.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color())
            .width(Length::FillPortion(2)),
    ]
    .spacing(8)
    .align_y(cosmic::iced::Alignment::Center)
    .into()
}

pub fn fetch_status() -> Result<StorageStatus, String> {
    let out = std::process::Command::new("mackesd")
        .args(["meshfs-status", "--json"])
        .output()
        .map_err(|e| format!("mackesd meshfs-status failed to spawn: {e}"))?;
    // Exit 1 means master unreachable — still parse the JSON.
    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.trim().is_empty() {
        return Err("mackesd meshfs-status returned no output".to_string());
    }
    let v: serde_json::Value =
        serde_json::from_str(stdout.trim()).map_err(|e| format!("JSON parse: {e}"))?;
    let peers = v["peers"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|p| {
            Some(PeerRow {
                addr: p["addr"].as_str()?.to_owned(),
                used_bytes: p["used_bytes"].as_u64()?,
                avail_bytes: p["avail_bytes"].as_u64()?,
            })
        })
        .collect();
    let goal = v["goal"].as_u64().unwrap_or(0) as usize;
    let quota_cap_bytes = v["quota_cap_bytes"].as_u64();
    let limiting_peer_addr = v["limiting_peer_addr"].as_str().map(str::to_owned);
    Ok(StorageStatus {
        peers,
        goal,
        quota_cap_bytes,
        limiting_peer_addr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_bytes_scales() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1024), "1.0 KiB");
        assert_eq!(human_bytes(5 * 1024 * 1024), "5.0 MiB");
    }

    #[test]
    fn fetch_status_fails_gracefully_when_mackesd_absent() {
        // In CI / headless environments mackesd is not running.
        // fetch_status() returns Err (spawn or empty output) — not a panic.
        let result = fetch_status();
        // Either Ok (mackesd installed + responding) or Err (absent/offline).
        // Both are valid outcomes; we just assert the function completes.
        let _ = result;
    }

    #[test]
    fn mesh_storage_panel_defaults() {
        let panel = MeshStoragePanel::new();
        assert!(panel.status.peers.is_empty());
        assert_eq!(panel.status.goal, 0);
        assert!(panel.status.quota_cap_bytes.is_none());
        assert!(panel.status.limiting_peer_addr.is_none());
        assert!(panel.error.is_none());
        assert!(!panel.busy);
    }

    #[test]
    fn loaded_ok_updates_state() {
        let mut panel = MeshStoragePanel::new();
        let status = StorageStatus {
            peers: vec![PeerRow {
                addr: "10.42.0.5".to_string(),
                used_bytes: 1_000_000,
                avail_bytes: 9_000_000,
            }],
            goal: 1,
            quota_cap_bytes: Some(7_200_000),
            limiting_peer_addr: Some("10.42.0.5".to_string()),
        };
        let _ = panel.update(Message::Loaded(Ok(status)));
        assert_eq!(panel.status.peers.len(), 1);
        assert_eq!(panel.status.goal, 1);
        assert_eq!(panel.status.quota_cap_bytes, Some(7_200_000));
        assert!(panel.error.is_none());
        assert!(!panel.busy);
    }

    #[test]
    fn loaded_err_clears_peers() {
        let mut panel = MeshStoragePanel {
            status: StorageStatus {
                peers: vec![PeerRow {
                    addr: "10.42.0.5".to_string(),
                    used_bytes: 0,
                    avail_bytes: 0,
                }],
                goal: 1,
                quota_cap_bytes: None,
                limiting_peer_addr: None,
            },
            ..Default::default()
        };
        let _ = panel.update(Message::Loaded(Err("timeout".to_string())));
        assert!(panel.status.peers.is_empty());
        assert_eq!(panel.error.as_deref(), Some("timeout"));
    }
}
