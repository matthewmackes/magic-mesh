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

// CUT-1: cosmic::Element bakes in cosmic::Theme — matching the theme the
// widgets (column/container/scrollable) and the panel_chrome helpers produce.
// cosmic::iced::Element would default to iced's own Theme and mismatch.
use cosmic::iced::widget::{column, container, row, scrollable, text, Space};
use cosmic::iced::{alignment, Background, Border, Color, Length, Task};
use cosmic::Element;
use mackes_mesh_types::PeerProbe;
use mde_theme::{carbon, Density, EmptyState, FontSize, FontWeight, Icon, Palette, TypeRole};

use crate::controls::{variant_button, ButtonVariant};
use crate::cosmic_compat::overlay_white_on;
use crate::cosmic_compat::prelude::*;
use crate::panel_chrome::{card, empty_state, panel_container, status_badge, BadgeSeverity};
use crate::status_strip::{mono_text, pip};

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

/// The replicated peer-probe root — the QNM-Shared workgroup directory,
/// where each node's `hardware_probe` worker publishes its
/// `<peer>/mackesd/probe.json` (SUBAUDIT-D2). Was a per-HOME
/// `~/.cache/mde/peers` that nothing populated, so the panel was
/// permanently empty.
#[must_use]
pub fn peers_cache_dir() -> Option<PathBuf> {
    Some(mackes_mesh_types::peers::default_workgroup_root())
}

/// Read every `<dir>/<peer>/mackesd/probe.json` into a [`PeerProbe`], sorted by
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
        .map(|e| e.path().join("mackesd").join("probe.json"))
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
                    None => {
                        Err("cannot resolve the peer cache dir (no HOME/XDG_CACHE_HOME)".into())
                    }
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
        let sizes = FontSize::defaults();
        let weights = FontWeight::defaults();
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

        // Page header — title + a live count chip + Refresh, in the design's
        // dense node-header rhythm.
        let count = self.probes.len();
        let header = row![
            text("Hardware inventory")
                .size(TypeRole::Heading.size_in(sizes))
                .colr(palette.text.into_cosmic_color()),
            Space::new().width(Length::Fill),
            chip(
                format!("{count} node{}", if count == 1 { "" } else { "s" }),
                palette,
                &sizes,
            ),
            Space::new().width(Length::Fixed(10.0)),
            refresh,
        ]
        .align_y(alignment::Vertical::Center);

        // Dense, zebra-striped, hairline-separated peer rows on one Carbon card.
        let mut rows = column![];
        for (i, p) in self.probes.iter().enumerate() {
            if i > 0 {
                rows = rows.push(hairline(palette));
            }
            rows = rows.push(peer_row(p, i, palette, &sizes, &weights));
        }
        let listing = dense_card("Reporting nodes", rows.into(), palette, density, &sizes);

        panel_container(
            column![header, scrollable(listing).height(Length::Fill)]
                .spacing(16)
                .width(Length::Fill)
                .into(),
            density,
        )
    }

    fn view_detail(&self, peer_id: &str) -> Element<'_, crate::Message> {
        let palette = crate::live_theme::palette();
        let density = crate::live_theme::tokens().density;
        let sizes = FontSize::defaults();
        let weights = FontWeight::defaults();
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

        let (badge_label, severity) = if p.power.on_ac {
            ("AC", BadgeSeverity::Success)
        } else {
            ("battery", BadgeSeverity::Neutral)
        };

        // Node identity header — echoes the design's "This Node" header: a teal
        // node pip, the hostname, a distro chip, the power badge, then a
        // Roboto-Mono identity line (vendor:product · transport · rtt).
        let header = row![
            pip(carbon::TEAL_30),
            Space::new().width(Length::Fixed(11.0)),
            text(p.hostname.clone())
                .size(TypeRole::Heading.size_in(sizes))
                .colr(palette.text.into_cosmic_color()),
            Space::new().width(Length::Fixed(11.0)),
            chip(p.distro.clone(), palette, &sizes),
            Space::new().width(Length::Fixed(11.0)),
            status_badge(badge_label, severity, palette),
            Space::new().width(Length::Fixed(11.0)),
            mono_text(
                format!(
                    "{}:{} · {} · rtt {} ms",
                    p.vendor_id, p.product_id, p.kernel.transport_module, p.bus.rtt_ms
                ),
                TypeRole::Caption,
                &sizes,
                &weights,
            )
            .colr(palette.text_muted.into_cosmic_color()),
        ]
        .align_y(alignment::Vertical::Center);

        // Kernel / power / bus → label-value cards (the design's "Hardware"
        // card pattern). All values are the real probe fields (§7).
        let kernel_rows = vec![
            ("uname", p.kernel.uname.clone()),
            ("Transport module", p.kernel.transport_module.clone()),
            ("mded", p.kernel.mded_version.clone()),
        ];
        let mut power_rows = vec![
            (
                "On AC",
                if p.power.on_ac {
                    "yes".to_string()
                } else {
                    "no".to_string()
                },
            ),
            (
                "Battery",
                p.power
                    .battery_pct
                    .map_or_else(|| "n/a".to_string(), |b| format!("{b}%")),
            ),
        ];
        if let Some(c) = p.power.cpu_pkg_c {
            power_rows.push(("CPU package", format!("{c:.0} °C")));
        }
        if let Some(r) = p.power.fan_rpm {
            power_rows.push(("Fan", format!("{r} rpm")));
        }
        let bus_rows = vec![
            ("RTT", format!("{} ms", p.bus.rtt_ms)),
            ("NAT class", format!("{:?}", p.bus.nat_class)),
            ("ICE candidate", p.bus.ice_candidate.clone()),
            ("Mesh path", p.bus.mesh_path.join("  →  ")),
        ];

        // The list cards (PCI / USB / descriptors) keep every real line, mono.
        let cards: Vec<Element<'_, crate::Message>> = vec![
            dense_card(
                "Kernel & driver",
                kv_body(kernel_rows, palette, &sizes, &weights),
                palette,
                density,
                &sizes,
            ),
            dense_card(
                "Power & thermal",
                kv_body(power_rows, palette, &sizes, &weights),
                palette,
                density,
                &sizes,
            ),
            dense_card(
                "Bus & topology",
                kv_body(bus_rows, palette, &sizes, &weights),
                palette,
                density,
                &sizes,
            ),
            dense_card(
                "PCI bus",
                mono_body(&p.bus.pci_tree, palette, &sizes, &weights),
                palette,
                density,
                &sizes,
            ),
            dense_card(
                "USB bus",
                mono_body(&p.bus.usb_tree, palette, &sizes, &weights),
                palette,
                density,
                &sizes,
            ),
            dense_card(
                "Descriptors — mesh services",
                mono_body(&p.descriptors.mesh_services, palette, &sizes, &weights),
                palette,
                density,
                &sizes,
            ),
            dense_card(
                "Descriptors — sysfs classes",
                mono_body(&p.descriptors.sysfs_classes, palette, &sizes, &weights),
                palette,
                density,
                &sizes,
            ),
            dense_card(
                "Descriptors — USB",
                mono_body(&p.descriptors.usb_descriptors, palette, &sizes, &weights),
                palette,
                density,
                &sizes,
            ),
        ];

        // Deal the cards into the design's two-column (1fr 1fr) card grid.
        let mut left = column![].spacing(11).width(Length::FillPortion(1));
        let mut right = column![].spacing(11).width(Length::FillPortion(1));
        for (i, c) in cards.into_iter().enumerate() {
            if i % 2 == 0 {
                left = left.push(c);
            } else {
                right = right.push(c);
            }
        }
        let grid = row![left, right].spacing(11).width(Length::Fill);

        let body = column![header, grid].spacing(16).width(Length::Fill);

        panel_container(
            column![back, scrollable(body).height(Length::Fill)]
                .spacing(16)
                .into(),
            density,
        )
    }
}

/// A 1 px full-width hairline divider in the border token — the design's
/// `1px solid #1f1f1f` inter-row rule, here a single-sourced token.
fn hairline<'a, Message: 'a>(palette: Palette) -> Element<'a, Message> {
    let color = palette.border.into_cosmic_color();
    container(Space::new().height(Length::Fixed(1.0)).width(Length::Fill))
        .style(move |_| container::Style {
            snap: false,
            icon_color: None,
            background: Some(Background::Color(color)),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 0.0.into(),
            },
            shadow: Default::default(),
            text_color: None,
        })
        .into()
}

/// A small bordered identity chip — the design's role/distro chip
/// (`padding:3px 9px;border:1px solid <border>`), Roboto caption.
fn chip<'a, Message: 'a>(
    label: impl Into<String>,
    palette: Palette,
    sizes: &FontSize,
) -> Element<'a, Message> {
    let border = palette.border.into_cosmic_color();
    container(
        text(label.into())
            .size(TypeRole::Caption.size_in(*sizes))
            .colr(palette.text_muted.into_cosmic_color()),
    )
    .padding([3u16, 9u16])
    .style(move |_| container::Style {
        snap: false,
        icon_color: None,
        background: None,
        border: Border {
            color: border,
            width: 1.0,
            radius: 0.0.into(),
        },
        shadow: Default::default(),
        text_color: None,
    })
    .into()
}

/// A dense Carbon card — an uppercase section label, a hairline, then the body
/// — on the shared `card` surface (§6 reuse).
fn dense_card<'a>(
    title: &str,
    body: Element<'a, crate::Message>,
    palette: Palette,
    density: Density,
    sizes: &FontSize,
) -> Element<'a, crate::Message> {
    let header = text(title.to_uppercase())
        .size(TypeRole::Caption.size_in(*sizes))
        .colr(palette.text_muted.into_cosmic_color());
    card(
        column![header, hairline(palette), body].spacing(8).into(),
        palette,
        density,
    )
}

/// Build a label/value card body — muted Roboto label, Roboto-Mono value,
/// right-aligned, hairline-separated (the design's Hardware card rows; Mono
/// for the metric per §4).
fn kv_body<'a>(
    rows: Vec<(&'static str, String)>,
    palette: Palette,
    sizes: &FontSize,
    weights: &FontWeight,
) -> Element<'a, crate::Message> {
    let mut col = column![];
    for (i, (label, value)) in rows.into_iter().enumerate() {
        if i > 0 {
            col = col.push(hairline(palette));
        }
        let r = row![
            text(label.to_string())
                .size(TypeRole::Caption.size_in(*sizes))
                .colr(palette.text_muted.into_cosmic_color())
                .width(Length::FillPortion(2)),
            mono_text(value, TypeRole::Caption, sizes, weights)
                .colr(palette.text.into_cosmic_color())
                .width(Length::FillPortion(3))
                .align_x(alignment::Horizontal::Right),
        ]
        .align_y(alignment::Vertical::Center);
        col = col.push(container(r).padding([6u16, 0u16]).width(Length::Fill));
    }
    col.into()
}

/// Build a mono list card body (PCI / USB / descriptors) — every real line in
/// Roboto-Mono, hairline-separated; an honest "(none reported)" when empty.
fn mono_body<'a>(
    lines: &[String],
    palette: Palette,
    sizes: &FontSize,
    weights: &FontWeight,
) -> Element<'a, crate::Message> {
    if lines.is_empty() {
        return container(
            text("(none reported)")
                .size(TypeRole::Caption.size_in(*sizes))
                .colr(palette.text_muted.into_cosmic_color()),
        )
        .padding([6u16, 0u16])
        .into();
    }
    let mut col = column![];
    for (i, l) in lines.iter().enumerate() {
        if i > 0 {
            col = col.push(hairline(palette));
        }
        col = col.push(
            container(
                mono_text(l.clone(), TypeRole::Caption, sizes, weights)
                    .colr(palette.text.into_cosmic_color()),
            )
            .padding([6u16, 0u16])
            .width(Length::Fill),
        );
    }
    col.into()
}

/// One inventory row — status pip · hostname (Roboto) · Roboto-Mono summary
/// metric · power badge · Inspect, zebra-tinted on odd rows for the dense
/// table feel.
fn peer_row<'a>(
    p: &PeerProbe,
    idx: usize,
    palette: Palette,
    sizes: &FontSize,
    weights: &FontWeight,
) -> Element<'a, crate::Message> {
    let (badge_label, severity) = if p.power.on_ac {
        ("AC", BadgeSeverity::Success)
    } else {
        ("battery", BadgeSeverity::Neutral)
    };
    let summary = format!(
        "{} · {} · {} PCI / {} USB",
        p.distro,
        p.kernel.uname,
        p.bus.pci_tree.len(),
        p.bus.usb_tree.len(),
    );
    let inspect = variant_button(
        "Inspect",
        ButtonVariant::Ghost,
        Some(crate::Message::Hardware(Message::Focus(p.peer_id.clone()))),
        palette,
    );
    let body = row![
        pip(palette.success),
        Space::new().width(Length::Fixed(10.0)),
        text(p.hostname.clone())
            .size(TypeRole::Body.size_in(*sizes))
            .colr(palette.text.into_cosmic_color())
            .width(Length::FillPortion(3)),
        mono_text(summary, TypeRole::Caption, sizes, weights)
            .colr(palette.text_muted.into_cosmic_color())
            .width(Length::FillPortion(7)),
        Space::new().width(Length::Fixed(10.0)),
        status_badge(badge_label, severity, palette),
        Space::new().width(Length::Fixed(10.0)),
        inspect,
    ]
    .align_y(alignment::Vertical::Center);

    // Zebra: a faint white overlay on the surface for odd rows (the design's
    // alternating-row striping), token-derived, no raw hex.
    let zebra = if idx % 2 == 1 {
        Some(Background::Color(overlay_white_on(palette.surface, 0.04)))
    } else {
        None
    };
    container(body)
        .padding([7u16, 8u16])
        .width(Length::Fill)
        .style(move |_| container::Style {
            snap: false,
            icon_color: None,
            background: zebra,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 0.0.into(),
            },
            shadow: Default::default(),
            text_color: None,
        })
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_probe(dir: &Path, peer: &str, hostname: &str) {
        // Mirrors the replicated layout: <root>/<peer>/mackesd/probe.json.
        let pdir = dir.join(peer).join("mackesd");
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
        let junk = tmp.path().join("p-3").join("mackesd");
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
