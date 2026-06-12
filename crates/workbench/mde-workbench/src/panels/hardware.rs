//! PLANES-5 — the **Hardware Inventory** panel (Controller plane).
//!
//! Renders the replicated [`PeerProbe`] each peer publishes — PCI/USB bus
//! topology, kernel + driver, power + thermal, and the
//! descriptors/capabilities section — with **no new collectors** (W19):
//! the data is exactly what `peer_join` already caches per peer at
//! `~/.cache/mde/peers/<id>/probe.json`. This is the read-only hardware
//! view, distinct from Fleet → Inventory (the node roster).
//!
//! Build-now-defer-visual (PD-3/5/7 pattern): the load + projection are
//! pure and unit-tested here; the on-Cosmic `/preview` pass against the
//! Carbon reference is the hardware-gated tail.

use std::path::{Path, PathBuf};

use iced::widget::{column, container, row, scrollable, text};
use iced::{Element, Length, Padding, Task};
use mackes_mesh_types::PeerProbe;
use mde_theme::{EmptyState, Icon};

use crate::controls::{variant_button, ButtonVariant};
use crate::panel_chrome::{empty_state, panel_container, status_badge, BadgeSeverity};

/// The Hardware Inventory panel state.
#[derive(Debug, Clone, Default)]
pub struct HardwarePanel {
    /// One entry per peer whose probe we hold, sorted by hostname.
    pub probes: Vec<PeerProbe>,
    pub status: String,
    /// EFF-45 — set when the probe LOAD failed (vs legitimately
    /// empty). The view renders the error state instead of the
    /// misleading "No probes yet" empty state.
    pub load_error: Option<String>,
    pub busy: bool,
    /// `peer_id` of the drilled-in peer; `None` = list view.
    pub focused: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<Vec<PeerProbe>, String>),
    Error(String),
    Focus(String),
    Back,
    RefreshClicked,
}

/// The per-peer probe cache dir (`$XDG_CACHE_HOME`/`~/.cache` → mde/peers).
#[must_use]
pub fn peers_cache_dir() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
        return Some(PathBuf::from(xdg).join("mde").join("peers"));
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".cache").join("mde").join("peers"))
}

/// Read every `<dir>/<peer>/probe.json` into a [`PeerProbe`], sorted by
/// hostname. Junk-tolerant per FILE (an unparseable probe.json is
/// skipped) — but EFF-45-honest per DIRECTORY: a missing dir is the
/// legitimate empty state (no peer has published yet), while an
/// EXISTING dir we cannot read (permissions, I/O) is a load FAILURE,
/// not "no probes".
pub fn load_probes(dir: &Path) -> Result<Vec<PeerProbe>, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("reading {}: {e}", dir.display())),
    };
    let mut out: Vec<PeerProbe> = entries
        .filter_map(Result::ok)
        .map(|e| e.path().join("probe.json"))
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .filter_map(|raw| serde_json::from_str::<PeerProbe>(&raw).ok())
        .collect();
    out.sort_by(|a, b| a.hostname.cmp(&b.hostname));
    Ok(out)
}

impl HardwarePanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load() -> Task<crate::Message> {
        Task::perform(
            async move {
                let result = match peers_cache_dir() {
                    Some(d) => load_probes(&d),
                    // No HOME/XDG at all — a real environment failure,
                    // not an empty roster.
                    None => Err("cannot resolve the peer cache dir (no HOME/XDG_CACHE_HOME)".into()),
                };
                Message::Loaded(result)
            },
            crate::Message::Hardware,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Loaded(Ok(probes)) => {
                self.probes = probes;
                self.status.clear();
                self.load_error = None;
                self.busy = false;
                Task::none()
            }
            Message::Loaded(Err(e)) | Message::Error(e) => {
                // EFF-45 — a failed load is an ERROR state, never an
                // empty roster.
                self.load_error = Some(e);
                self.busy = false;
                Task::none()
            }
            Message::Focus(peer_id) => {
                self.focused = Some(peer_id);
                Task::none()
            }
            Message::Back => {
                self.focused = None;
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
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        match &self.focused {
            Some(peer_id) => self.view_detail(peer_id),
            None => self.view_list(),
        }
    }

    fn view_list(&self) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let density = crate::live_theme::tokens().density;
        let refresh = variant_button(
            "Refresh",
            ButtonVariant::Ghost,
            (!self.busy).then(|| crate::Message::Hardware(Message::RefreshClicked)),
            palette,
        );

        // EFF-45 — a failed load renders as failure, never as the
        // "No probes yet" empty state.
        if let Some(err) = &self.load_error {
            return panel_container(
                crate::panel_chrome::error_state(err.clone(), palette, || {
                    crate::Message::Hardware(Message::RefreshClicked)
                }),
                density,
            );
        }

        if self.probes.is_empty() {
            let state = EmptyState::with_cta(
                "No hardware probes yet",
                "Each peer publishes a hardware probe on join (PLANES-5 / W19). \
                 Probes appear here as peers enroll and replicate — no new \
                 collector runs.",
                "Refresh",
            )
            .with_icon(Icon::Inventory);
            return panel_container(
                empty_state(state, palette, || {
                    crate::Message::Hardware(Message::RefreshClicked)
                }),
                density,
            );
        }

        let mut list = column![].spacing(8);
        for p in &self.probes {
            let (badge_label, severity) = if p.power.on_ac {
                ("AC", BadgeSeverity::Success)
            } else {
                ("battery", BadgeSeverity::Neutral)
            };
            let summary = text(format!(
                "{} · {} · {} PCI / {} USB",
                p.distro,
                p.kernel.uname,
                p.bus.pci_tree.len(),
                p.bus.usb_tree.len(),
            ))
            .size(13);
            let open = variant_button(
                "Inspect",
                ButtonVariant::Ghost,
                Some(crate::Message::Hardware(Message::Focus(p.peer_id.clone()))),
                palette,
            );
            list = list.push(
                container(
                    row![
                        column![text(p.hostname.clone()).size(16), summary].spacing(2),
                        status_badge(badge_label, severity, palette),
                        open,
                    ]
                    .spacing(12)
                    .align_y(iced::Alignment::Center),
                )
                .padding(Padding::from(12)),
            );
        }

        panel_container(
            column![refresh, scrollable(list).height(Length::Fill)]
                .spacing(16)
                .width(Length::Fill)
                .into(),
            density,
        )
    }

    fn view_detail(&self, peer_id: &str) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let density = crate::live_theme::tokens().density;
        let back = variant_button(
            "‹ Back",
            ButtonVariant::Ghost,
            Some(crate::Message::Hardware(Message::Back)),
            palette,
        );
        let Some(p) = self.probes.iter().find(|p| p.peer_id == peer_id) else {
            return panel_container(
                column![back, text("Probe no longer present.")]
                    .spacing(16)
                    .into(),
                density,
            );
        };

        let section = |title: &str, lines: Vec<String>| {
            let mut col = column![text(title.to_string()).size(15)].spacing(4);
            if lines.is_empty() {
                col = col.push(text("(none reported)").size(12));
            } else {
                for l in lines {
                    col = col.push(text(l).size(12));
                }
            }
            container(col).padding(Padding::from(10))
        };

        let power = {
            let mut v = vec![
                format!("On AC: {}", p.power.on_ac),
                p.power
                    .battery_pct
                    .map_or_else(|| "Battery: n/a".into(), |b| format!("Battery: {b}%")),
            ];
            if let Some(c) = p.power.cpu_pkg_c {
                v.push(format!("CPU package: {c:.0} °C"));
            }
            if let Some(r) = p.power.fan_rpm {
                v.push(format!("Fan: {r} rpm"));
            }
            v
        };

        let body = column![
            text(format!("{} — {}", p.hostname, p.distro)).size(20),
            text(format!("vendor:product {}:{}", p.vendor_id, p.product_id)).size(12),
            section("PCI bus", p.bus.pci_tree.clone()),
            section("USB bus", p.bus.usb_tree.clone()),
            section(
                "Kernel & driver",
                vec![
                    format!("uname: {}", p.kernel.uname),
                    format!("transport module: {}", p.kernel.transport_module),
                    format!("mded: {}", p.kernel.mded_version),
                ],
            ),
            section("Power & thermal", power),
            section(
                "Descriptors — mesh services",
                p.descriptors.mesh_services.clone()
            ),
            section(
                "Descriptors — sysfs classes",
                p.descriptors.sysfs_classes.clone()
            ),
            section("Descriptors — USB", p.descriptors.usb_descriptors.clone()),
        ]
        .spacing(10);

        panel_container(
            column![back, scrollable(body).height(Length::Fill)]
                .spacing(16)
                .into(),
            density,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_probe(dir: &Path, peer: &str, hostname: &str) {
        let pdir = dir.join(peer);
        std::fs::create_dir_all(&pdir).unwrap();
        let mut probe = PeerProbe::fixture();
        probe.peer_id = peer.to_string();
        probe.hostname = hostname.to_string();
        std::fs::write(
            pdir.join("probe.json"),
            serde_json::to_string(&probe).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn load_probes_reads_each_peer_sorted_and_skips_junk() {
        let tmp = tempfile::tempdir().unwrap();
        write_probe(tmp.path(), "p-2", "zeta");
        write_probe(tmp.path(), "p-1", "alpha");
        // A junk peer dir with an unparseable probe is skipped, not fatal.
        let junk = tmp.path().join("p-3");
        std::fs::create_dir_all(&junk).unwrap();
        std::fs::write(junk.join("probe.json"), "not json").unwrap();

        let probes = load_probes(tmp.path()).expect("should succeed");
        assert_eq!(probes.len(), 2, "two valid probes, junk skipped");
        assert_eq!(probes[0].hostname, "alpha", "sorted by hostname");
        assert_eq!(probes[1].hostname, "zeta");
    }

    #[test]
    fn load_probes_missing_dir_is_empty_not_panic() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(load_probes(&tmp.path().join("nope")).unwrap().is_empty());
    }

    #[test]
    fn update_loaded_populates_and_clears_busy() {
        let mut panel = HardwarePanel::new();
        panel.busy = true;
        let mut probe = PeerProbe::fixture();
        probe.hostname = "node".into();
        let _ = panel.update(Message::Loaded(Ok(vec![probe])));
        assert_eq!(panel.probes.len(), 1);
        assert!(!panel.busy);
    }

    #[test]
    fn focus_and_back_toggle_the_detail_view() {
        let mut panel = HardwarePanel::new();
        let _ = panel.update(Message::Focus("p-1".into()));
        assert_eq!(panel.focused.as_deref(), Some("p-1"));
        let _ = panel.update(Message::Back);
        assert!(panel.focused.is_none());
    }
}
