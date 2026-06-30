//! MESHFS-13.1 (v5.0.0) — Workbench "Mesh Storage" panel.
//!
//! SUBSTRATE-V2: the mesh **file plane** is **Syncthing**, not LizardFS. Every
//! node full-mesh-syncs the **Mesh Sync** share at `/mnt/mesh-storage` — a plain
//! local directory (NO FUSE mount) over the Nebula overlay, with trash-can
//! versioning (`.stversions`). Coordination (leader/peer-directory/health) lives
//! in **etcd**, off the filesystem, so a slow or stopped Syncthing degrades file
//! access only and never takes the mesh down (the failure class the old single
//! LizardFS mount caused). See `docs/design/substrate-v2.md`.
//!
//! The panel renders the real substrate status — the local `syncthing` service +
//! version and the `/mnt/mesh-storage` share — plus per-peer share usage
//! (address, used/available bytes), the replication goal, the effective quota
//! cap, and the limiting peer. Peer data comes from `mackesd mesh-fs-status`.

use std::time::SystemTime;

use cosmic::iced::widget::{button, column, container, row, scrollable, svg, text, Space};
use cosmic::iced::{Background, Border, Color, Length, Padding, Task};
use cosmic::{Element, Theme};
use mde_theme::hero::Hero;
use mde_theme::{FontSize, Palette, TypeRole};

use crate::cosmic_compat::prelude::*;
use crate::panel_chrome::{hero_band, pkg_version_cached};

/// The Syncthing package whose version captions the Mesh Storage hero (H8).
const SYNCTHING_PKG: &str = "syncthing";

/// The local `syncthing` system unit backing the Mesh Sync file plane.
const SYNCTHING_UNIT: &str = "syncthing.service";

/// NOTIFY-UI-3 / ICON-MESH — the `folder-remote` network glyph for the
/// mesh-storage surface. Mesh Storage is the Syncthing / Mesh Sync mesh file
/// share, *not* a local mounted volume, so its title takes the network folder
/// icon (the freedesktop `folder-remote` equivalent) the same as the mde-files
/// mesh-storage representation. Same lucide path as `mde_files::icons::FOLDER_REMOTE`.
const FOLDER_REMOTE_NAME: &str = "folder-remote";
const FOLDER_REMOTE_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="square" stroke-linejoin="miter"><path d="M3 6a1 1 0 0 1 1-1h5l2 2h9a1 1 0 0 1 1 1v10a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V6z"/><circle cx="9" cy="14" r="1.3"/><circle cx="16" cy="11" r="1.3"/><circle cx="16" cy="17" r="1.3"/><line x1="10.1" y1="13.4" x2="14.9" y2="11.4"/><line x1="10.1" y1="14.6" x2="14.9" y2="16.6"/></svg>"##;

/// Canonical freedesktop icon name the mesh-storage surface renders — the
/// network folder (`folder-remote`), since Mesh Storage is a mesh file service,
/// not a local volume. NOTIFY-UI-3 / ICON-MESH.
#[must_use]
pub const fn mesh_storage_icon_name() -> &'static str {
    FOLDER_REMOTE_NAME
}

/// The `folder-remote` network icon for the mesh-storage surface, tinted to the
/// supplied colour. NOTIFY-UI-3 / ICON-MESH.
fn mesh_storage_icon<'a>(color: Color) -> Element<'a, crate::Message> {
    svg(svg::Handle::from_memory(FOLDER_REMOTE_SVG))
        .width(Length::Fixed(22.0))
        .height(Length::Fixed(22.0))
        .sty(move |_t: &Theme| svg::Style { color: Some(color) })
        .into()
}

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

/// True only when `systemctl is-active` reported the unit as `active` — the whole
/// word, not the `activating` / `active (auto-restart)` prefixes a starting or
/// flapping unit prints (so those are correctly treated as not-up).
fn is_active_output(stdout: &[u8]) -> bool {
    String::from_utf8_lossy(stdout).trim() == "active"
}

/// SUBSTRATE-V2 — the live state of the local Syncthing file plane: whether the
/// `syncthing` unit is active and whether the `/mnt/mesh-storage` Mesh Sync share
/// is present on disk. The panel reflects the real substrate rather than a
/// placeholder (§7); [`SubstrateStatus::cached`] keeps the `view()` repaint cheap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SubstrateStatus {
    /// The `syncthing.service` unit is `active` per `systemctl is-active`.
    service_active: bool,
    /// `/mnt/mesh-storage` (the Mesh Sync share root) exists as a directory.
    share_present: bool,
}

/// How long a [`SubstrateStatus::cached`] probe is reused before re-shelling
/// `systemctl`. Long enough that a repaint storm (input events, a 60 Hz
/// transition tick) never spawns a subprocess per frame, short enough that the
/// status line tracks the service within a couple of seconds.
const SUBSTRATE_TTL: std::time::Duration = std::time::Duration::from_secs(2);

impl SubstrateStatus {
    /// The live substrate state, memoized for [`SUBSTRATE_TTL`]. `view()` runs on
    /// every repaint (a 60 Hz transition tick, every input event), so an
    /// un-memoized probe would `fork`/`exec` `systemctl` dozens of times a second
    /// and stutter the UI — the same reason [`pkg_version_cached`] memoizes its
    /// `rpm -q`. Returns the cached value when fresh, else re-reads.
    fn cached() -> Self {
        use std::sync::{Mutex, OnceLock};
        use std::time::Instant;
        static CACHE: OnceLock<Mutex<Option<(Instant, SubstrateStatus)>>> = OnceLock::new();
        let cell = CACHE.get_or_init(|| Mutex::new(None));
        if let Ok(guard) = cell.lock() {
            if let Some((at, st)) = *guard {
                if at.elapsed() < SUBSTRATE_TTL {
                    return st;
                }
            }
        }
        let fresh = Self::read();
        if let Ok(mut guard) = cell.lock() {
            *guard = Some((Instant::now(), fresh));
        }
        fresh
    }

    /// Read the live Syncthing substrate state. `systemctl is-active` prints
    /// exactly `active` (with a trailing newline) only when the unit is up, and
    /// `inactive`/`failed`/`activating`/… otherwise; an absent `systemctl`
    /// (CI/headless) degrades to "not active" rather than a panic, matching the
    /// rest of the panel's honest-when-offline contract.
    fn read() -> Self {
        let service_active = std::process::Command::new("systemctl")
            .args(["is-active", SYNCTHING_UNIT])
            .output()
            .map(|o| is_active_output(&o.stdout))
            .unwrap_or(false);
        let share_present = mackes_mesh_types::peers::default_workgroup_root().is_dir();
        Self {
            service_active,
            share_present,
        }
    }

    /// A one-line operator summary of the file plane, e.g.
    /// `"Mesh Sync · syncthing active · /mnt/mesh-storage present"`.
    fn summary_line(self) -> String {
        let svc = if self.service_active {
            "syncthing active"
        } else {
            "syncthing not active"
        };
        let share = if self.share_present {
            "/mnt/mesh-storage present"
        } else {
            "/mnt/mesh-storage missing"
        };
        format!("Mesh Sync · {svc} · {share}")
    }
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
    /// MESHFS-3 — Mesh-Sync folder completion percent from Syncthing's REST API
    /// (`None` when Syncthing is unreachable / unprovisioned).
    pub sync_completion_pct: Option<f64>,
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

        // NOTIFY-UI-3 / ICON-MESH — Mesh Storage is the mesh file plane (the
        // Syncthing / Mesh Sync share), so its title leads with the
        // `folder-remote` network icon rather than a local-volume glyph.
        let title_icon = mesh_storage_icon(palette.text.into_cosmic_color());
        let title = text("Mesh Storage")
            .size(TypeRole::Display.size_in(sizes))
            .colr(palette.text.into_cosmic_color());

        let subtitle_str = if let Some(t) = self.last_run_at {
            let age_s = t.elapsed().map(|d| d.as_secs()).unwrap_or(0);
            let repl = match self.status.sync_completion_pct {
                Some(p) => format!(" · replication {p:.0}%"),
                None => String::new(),
            };
            format!(
                "{} peer{} · goal {} · last refresh {}s ago{}",
                self.status.peers.len(),
                if self.status.peers.len() == 1 {
                    ""
                } else {
                    "s"
                },
                self.status.goal,
                age_s,
                repl,
            )
        } else {
            "click Refresh to query Mesh Sync share usage".into()
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

        // PLANES-2 / SUBSTRATE-V2 — Mesh Storage is the Syncthing file plane;
        // carry the Syncthing hero captioned with the live `syncthing` version.
        let syncthing = hero_band(
            Hero::Syncthing,
            pkg_version_cached(SYNCTHING_PKG).as_deref(),
            palette,
        );
        let header = row![
            title_icon,
            title,
            Space::new().width(Length::Fill),
            refresh_btn,
            syncthing
        ]
        .spacing(12)
        .align_y(cosmic::iced::Alignment::Center);

        // SUBSTRATE-V2 — real file-plane status (service active + share present).
        // This is the substrate the panel describes, so it renders even before /
        // independent of the per-peer `mackesd` query. Memoized (`cached`) so a
        // repaint storm doesn't spawn `systemctl` per frame.
        let substrate = SubstrateStatus::cached();
        let substrate_color = if substrate.service_active && substrate.share_present {
            palette.success
        } else {
            palette.warning
        };
        let substrate_row = row![text(substrate.summary_line())
            .size(TypeRole::Body.size_in(sizes))
            .colr(substrate_color.into_cosmic_color())];
        let sub_row = row![subtitle];

        let body: Element<'_, crate::Message> = if let Some(ref e) = self.error {
            text(format!("Error: {e}"))
                .size(TypeRole::Body.size_in(sizes))
                .colr(palette.danger.into_cosmic_color())
                .into()
        } else if self.status.peers.is_empty() && self.last_run_at.is_some() {
            text("No peer share usage reported yet — Mesh Sync may still be settling.")
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

        let page = column![
            header,
            sub_row,
            substrate_row,
            Space::new().height(12),
            body
        ]
        .spacing(4);

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
        palette.warning.into_cosmic_color()
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
        .args(["mesh-fs-status"])
        .output()
        .map_err(|e| format!("mackesd mesh-fs-status failed to spawn: {e}"))?;
    // `mackesd` may exit non-zero when the share usage is unavailable but still
    // emits a (possibly empty) JSON body — parse stdout regardless of exit code.
    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.trim().is_empty() {
        return Err("mackesd mesh-fs-status returned no output".to_string());
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
    // MESHFS-3 — Syncthing folder completion (None when the daemon is unreachable).
    let sync_completion_pct = v["sync_completion_pct"].as_f64();
    Ok(StorageStatus {
        peers,
        goal,
        quota_cap_bytes,
        limiting_peer_addr,
        sync_completion_pct,
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
    fn mesh_storage_surface_uses_the_network_folder_icon() {
        // NOTIFY-UI-3 / ICON-MESH: Mesh Storage is a mesh file share (the
        // Syncthing / Mesh Sync plane), not a local volume, so its surface
        // renders the `folder-remote` network icon — matching the mde-files
        // mesh-storage representation. Guard the SVG envelope too so the glyph
        // can't silently rot into something un-renderable.
        assert_eq!(mesh_storage_icon_name(), "folder-remote");
        assert_eq!(mesh_storage_icon_name(), FOLDER_REMOTE_NAME);
        let s = std::str::from_utf8(FOLDER_REMOTE_SVG).expect("icon bytes utf8");
        assert!(s.starts_with("<svg "), "mesh-storage icon must be an <svg>");
        assert!(s.ends_with("</svg>"), "mesh-storage icon must close </svg>");
    }

    #[test]
    fn mesh_storage_carries_the_syncthing_hero_not_lizardfs() {
        // SUBSTRATE-V2 — the file plane is Syncthing; the panel's hero must be
        // the Syncthing hero captioned with the `syncthing` package (NOT the
        // retired LizardFS / `lizardfs-client` pairing). Pin the constants the
        // header uses so a regression back to LizardFS is a test failure.
        assert_eq!(SYNCTHING_PKG, "syncthing");
        assert_eq!(SYNCTHING_UNIT, "syncthing.service");
        assert_eq!(Hero::Syncthing.name(), "Syncthing");
    }

    #[test]
    fn substrate_status_summary_reflects_service_and_share() {
        // §7 — the file-plane status line is real, not a placeholder: it names
        // the service liveness and the /mnt/mesh-storage share presence, and
        // brands the share "Mesh Sync" (SUBSTRATE-V2 lock #12), never "LizardFS".
        let up = SubstrateStatus {
            service_active: true,
            share_present: true,
        };
        let s = up.summary_line();
        assert!(
            s.starts_with("Mesh Sync"),
            "must brand the share Mesh Sync: {s}"
        );
        assert!(s.contains("syncthing active"), "{s}");
        assert!(s.contains("/mnt/mesh-storage present"), "{s}");
        assert!(!s.contains("LizardFS"), "must not mention LizardFS: {s}");

        let down = SubstrateStatus {
            service_active: false,
            share_present: false,
        };
        let s = down.summary_line();
        assert!(s.contains("syncthing not active"), "{s}");
        assert!(s.contains("/mnt/mesh-storage missing"), "{s}");
    }

    #[test]
    fn is_active_output_matches_active_only_not_activating() {
        // `systemctl is-active` prints the unit's state word; only `active`
        // (whole word, trailing newline) means up. A starting or auto-restart-
        // flapping unit prints `activating` / `active (auto-restart)` and MUST
        // NOT read as up (else the status line goes green while the file plane
        // is not serving).
        assert!(is_active_output(b"active\n"));
        assert!(is_active_output(b"active"));
        assert!(!is_active_output(b"activating\n"));
        assert!(!is_active_output(b"active (auto-restart)\n"));
        assert!(!is_active_output(b"inactive\n"));
        assert!(!is_active_output(b"failed\n"));
        assert!(!is_active_output(b""));
    }

    #[test]
    fn substrate_status_read_does_not_panic_headless() {
        // In CI / headless environments `systemctl` may be absent and the share
        // unmounted — read()/cached() must degrade to a value, never panic.
        let st = SubstrateStatus::read();
        // Both fields are valid in either state; just assert the read completes
        // and the summary renders.
        let _ = st.summary_line();
        // The memoized path returns the same shape and must not panic either.
        let cached = SubstrateStatus::cached();
        let _ = cached.summary_line();
    }

    #[test]
    fn mesh_storage_view_renders_without_panicking() {
        // Reachability: the panel's view() builds (incl. the title icon) for
        // both the empty and the populated state.
        let empty = MeshStoragePanel::new();
        let _ = empty.view();

        let mut populated = MeshStoragePanel::new();
        let _ = populated.update(Message::Loaded(Ok(StorageStatus {
            peers: vec![PeerRow {
                addr: "10.42.0.5".to_string(),
                used_bytes: 1_000_000,
                avail_bytes: 9_000_000,
            }],
            goal: 1,
            quota_cap_bytes: Some(7_200_000),
            limiting_peer_addr: Some("10.42.0.5".to_string()),
            sync_completion_pct: None,
        })));
        let _ = populated.view();
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
            sync_completion_pct: None,
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
                sync_completion_pct: None,
            },
            ..Default::default()
        };
        let _ = panel.update(Message::Loaded(Err("timeout".to_string())));
        assert!(panel.status.peers.is_empty());
        assert_eq!(panel.error.as_deref(), Some("timeout"));
    }
}
