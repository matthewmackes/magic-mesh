//! `Surface::About` → the **Device-Manager hardware inspector** (DEVMGR-2, design
//! `docs/design/about-device-manager.md`; locks #1/#2/#18/#19/#20/#24).
//!
//! The About surface body is a faithful Windows-Device-Manager **by-type** tree,
//! rendered entirely in `mde_egui::Style` dark tokens (§4): a compact brand title
//! strip (the brand shrinks off the body, #2/#24) with an ⓘ button that opens the
//! license / credits / mesh-identity dialog; a full menu bar + toolbar (#19); a
//! rich per-host header card (#20); and the all-collapsed category tree (#1/#18).
//!
//! It is a pure **consumer** of the §6 JSON contract in
//! [`mackes_mesh_types::device_inventory`] — the `hardware_probe` worker (DEVMGR-1)
//! publishes `<workgroup_root>/device-inventory/<host>.json` on every node, and
//! this surface reads THIS node's file (the local host) on a cadence + on a Scan.
//! It never enumerates hardware itself (that is the mesh-side worker) and depends
//! on no `mackesd` crate (§6): the wire is the file.
//!
//! **Honest degradation (§7):** before the first read the tree is a dim "reading…"
//! placeholder (no fabricated rows); a host with nothing published reads as an
//! honest "no inventory yet", never a faked tree; absent summary fields render as
//! an em-dash, never invented totals.
//!
//! **Scope is DEVMGR-2** — the by-type tree + header card + chrome + local read.
//! The detail drawer + MDM problem codes (DEVMGR-3), the host rail across peers
//! (DEVMGR-4), the by-connection topology (DEVMGR-5) and export (DEVMGR-6) are
//! later units; their seams here (the disabled view modes, the non-interactive
//! device rows) are left clean, not stubbed to a fake render.

#![allow(
    clippy::redundant_pub_crate,
    reason = "pub(crate) items in a private surface module are this crate's idiom \
              (ChromeState, ChooserState, the About renderer, …); the shell body in \
              main.rs consumes them"
)]

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use mackes_mesh_types::device_inventory::{
    self, DeviceCategory, DeviceInventory, DeviceRecord, DeviceStatus, HostSummary,
};
use mackes_mesh_types::peers::default_workgroup_root;
use mde_egui::egui::{self, Id, RichText};
use mde_egui::{field, muted_note, status_dot, Style};
use mde_theme::brand;

use crate::about;
use crate::explorer::local_hostname;

/// Re-read THIS node's published inventory this often (design #8 — the ~30 s
/// auto-refresh; the producer republishes on its own cadence). A Scan forces an
/// immediate re-read regardless of this gate.
const REFRESH: Duration = Duration::from_secs(30);

/// How the device tree is organised (#3). DEVMGR-2 ships **By type**; By
/// connection (the PCI/USB topology, DEVMGR-5) and By node (the cross-fleet
/// flatten, DEVMGR-4) are later units. The faithful MDM View menu offers all
/// three, with the unbuilt modes **honestly disabled** (§7 — never stubbed to a
/// fake render).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ViewMode {
    /// The classic by-category tree (Processors, Network adapters, …). Wired.
    #[default]
    ByType,
    /// The PCI/USB topology tree (DEVMGR-5) — not yet wired.
    ByConnection,
    /// The cross-fleet flatten of every host's devices (DEVMGR-4) — not yet wired.
    ByNode,
}

impl ViewMode {
    /// The three modes in View-menu / toolbar order.
    const ALL: [Self; 3] = [Self::ByType, Self::ByConnection, Self::ByNode];

    /// The menu / toolbar label.
    const fn label(self) -> &'static str {
        match self {
            Self::ByType => "By type",
            Self::ByConnection => "By connection",
            Self::ByNode => "By node",
        }
    }

    /// Whether this mode is wired in DEVMGR-2 (only [`ByType`](Self::ByType)). The
    /// others render as disabled controls until their unit lands (§7).
    const fn is_available(self) -> bool {
        matches!(self, Self::ByType)
    }
}

/// The About → Device-Manager surface state (DEVMGR-2). Holds the last-read local
/// inventory, the fixed-cadence read clock, the per-category expand set, the tree
/// organisation, and the ⓘ dialog latch. Drives no worker — a thin renderer over
/// the replicated snapshot.
pub(crate) struct DeviceManagerState {
    /// The replicated workgroup root the `device-inventory/` dir lives under
    /// (resolved once — the same substrate mount the chrome/grade fold reads).
    workgroup_root: PathBuf,
    /// This node's short hostname — the LOCAL inventory this surface reads
    /// (DEVMGR-2; the host rail across peers is DEVMGR-4).
    local_host: String,
    /// The last-read LOCAL inventory, or `None` when nothing is published yet.
    inventory: Option<DeviceInventory>,
    /// Whether the inventory has been read at least once — the honest pre-poll
    /// gate (§7): a dim "reading…" before the first read, distinct from a
    /// read-but-empty host.
    seen: bool,
    /// When the inventory was last read (drives the fixed [`REFRESH`] cadence).
    last_poll: Option<Instant>,
    /// The category keys currently expanded — empty by default (all-collapsed,
    /// #18). Expand-/Collapse-all fill/clear it; a header click toggles one.
    expanded: BTreeSet<String>,
    /// The active tree organisation (#3) — By type in DEVMGR-2.
    view: ViewMode,
    /// The ⓘ dialog latch — license / credits / mesh-identity (#24).
    show_about: bool,
}

impl Default for DeviceManagerState {
    fn default() -> Self {
        Self {
            workgroup_root: default_workgroup_root(),
            local_host: local_hostname(),
            inventory: None,
            seen: false,
            last_poll: None,
            expanded: BTreeSet::new(),
            view: ViewMode::default(),
            show_about: false,
        }
    }
}

impl DeviceManagerState {
    /// Re-read THIS node's published inventory from the substrate now. An absent /
    /// half-replicated / unreadable file reads as an honest `None` (never a
    /// panic, via [`device_inventory::read_inventory`]); `seen` flips true so the
    /// surface leaves the pre-poll state. Both the Scan action and the cadence
    /// [`poll`](Self::poll) land here.
    fn refresh(&mut self) {
        self.inventory = device_inventory::read_inventory(&self.workgroup_root, &self.local_host);
        self.seen = true;
    }

    /// The poll seam (self-gating): re-read on the fixed cadence while the About
    /// surface is in view, then keep the repaint heartbeat alive so a fresh
    /// publish surfaces without operator input. Cheap — one local file read.
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        let due = self.last_poll.is_none_or(|t| t.elapsed() >= REFRESH);
        if due {
            self.last_poll = Some(Instant::now());
            self.refresh();
        }
        ctx.request_repaint_after(REFRESH);
    }

    /// Expand every published category (Expand-all, #19).
    fn expand_all(&mut self) {
        if let Some(inv) = &self.inventory {
            self.expanded = inv.categories.iter().map(|c| c.key.clone()).collect();
        }
    }

    /// Collapse every category (Collapse-all, #19 — also the #18 default).
    fn collapse_all(&mut self) {
        self.expanded.clear();
    }

    /// Toggle one category's expansion.
    fn toggle(&mut self, key: &str) {
        if !self.expanded.remove(key) {
            self.expanded.insert(key.to_string());
        }
    }

    /// Render the whole surface into `ui` (the body of `Surface::About`).
    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        self.title_strip(ui);
        ui.separator();
        self.menu_bar(ui);
        self.toolbar(ui);
        ui.separator();
        ui.add_space(Style::SP_XS);

        if !self.seen {
            // Honest pre-poll (§7) — no fabricated tree before the first read.
            pre_poll(ui, &self.local_host);
        } else if self.inventory.is_none() {
            // Read, but this host has published nothing yet.
            empty_host(ui, &self.local_host);
        } else {
            // The header reads the inventory immutably, then the tree takes `&mut
            // self` to mutate the expand set — so the header borrow is scoped
            // closed (a plain `if let`) before `tree` is called.
            if let Some(inv) = self.inventory.as_ref() {
                header_card(ui, inv);
            }
            ui.add_space(Style::SP_S);
            self.tree(ui);
        }

        self.about_dialog(ui);
    }

    /// The compact brand title strip (#2/#24): the `◈` mark + product name +
    /// version on the left, the ⓘ button on the right. Single-sourced from
    /// [`mde_theme::brand`] (§4/§6) so it can never drift from `--version`.
    fn title_strip(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("\u{25C8}") // ◈ — the mesh-node mark
                    .color(Style::ACCENT)
                    .size(Style::TITLE),
            );
            ui.add_space(Style::SP_XS);
            ui.label(
                RichText::new(brand::logo::PRODUCT_NAME)
                    .color(Style::TEXT)
                    .size(Style::BODY)
                    .strong(),
            );
            ui.label(
                RichText::new(format!("v{}", brand::build::info().version))
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(
                        RichText::new("\u{24D8}") // ⓘ
                            .size(Style::BODY)
                            .color(Style::TEXT),
                    )
                    .on_hover_text("About \u{2014} license, credits, mesh identity")
                    .clicked()
                {
                    self.show_about = true;
                }
            });
        });
    }

    /// The faithful MDM menu bar (#19): Action (Scan) · View (the three modes +
    /// Expand/Collapse-all) · Help (the ⓘ dialog).
    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::menu::bar(ui, |ui| {
            ui.menu_button("Action", |ui| {
                if ui.button("Scan for hardware changes").clicked() {
                    self.refresh();
                    ui.close_menu();
                }
            });
            ui.menu_button("View", |ui| {
                for mode in ViewMode::ALL {
                    let resp = ui.add_enabled(
                        mode.is_available(),
                        egui::SelectableLabel::new(self.view == mode, mode.label()),
                    );
                    if resp.clicked() {
                        self.view = mode;
                        ui.close_menu();
                    }
                    if !mode.is_available() {
                        resp.on_hover_text("Arrives in a later DEVMGR unit");
                    }
                }
                ui.separator();
                if ui.button("Expand all").clicked() {
                    self.expand_all();
                    ui.close_menu();
                }
                if ui.button("Collapse all").clicked() {
                    self.collapse_all();
                    ui.close_menu();
                }
            });
            ui.menu_button("Help", |ui| {
                if ui.button("About Magic-Mesh").clicked() {
                    self.show_about = true;
                    ui.close_menu();
                }
            });
        });
    }

    /// The faithful MDM toolbar (#19): Scan (re-read the published inventory),
    /// Expand/Collapse-all, and the view-mode segmented control — By type live,
    /// the others disabled seams (§7).
    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(Style::SP_XS);
        ui.horizontal(|ui| {
            ui.add_space(Style::SP_XS);
            if ui
                .button(RichText::new("\u{21BB}  Scan").size(Style::SMALL)) // ↻
                .on_hover_text("Re-read the published hardware inventory")
                .clicked()
            {
                self.refresh();
            }
            ui.separator();
            if ui
                .button(RichText::new("Expand all").size(Style::SMALL))
                .clicked()
            {
                self.expand_all();
            }
            if ui
                .button(RichText::new("Collapse all").size(Style::SMALL))
                .clicked()
            {
                self.collapse_all();
            }
            ui.separator();
            for mode in ViewMode::ALL {
                let resp = ui.add_enabled(
                    mode.is_available(),
                    egui::SelectableLabel::new(self.view == mode, mode.label()),
                );
                if resp.clicked() {
                    self.view = mode;
                }
            }
        });
        ui.add_space(Style::SP_XS);
    }

    /// The by-type device tree (#1/#18) in a vertical scroll: each category is a
    /// forced-state [`egui::CollapsingHeader`] whose open/closed is driven from
    /// [`Self::expanded`] (so Expand-/Collapse-all and per-header clicks all route
    /// through the one set), amber-tinted when it holds a problem device.
    fn tree(&mut self, ui: &mut egui::Ui) {
        // The category a header click toggled this frame — applied AFTER the read
        // borrow ends so the immutable inventory read + the mutable toggle never
        // alias.
        let mut toggled: Option<String> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let Some(inv) = self.inventory.as_ref() else {
                    return;
                };
                for cat in &inv.categories {
                    let open = self.expanded.contains(cat.key.as_str());
                    if category_header(ui, cat, open) {
                        toggled = Some(cat.key.clone());
                    }
                }
            });
        if let Some(key) = toggled {
            self.toggle(&key);
        }
    }

    /// The ⓘ dialog (#24): the canonical identity screen (QBRAND-6 —
    /// [`about::about_panel`]) reused verbatim as the modal body (§6, one About
    /// renderer), with a top-bar close. Closes on the `×`, the backdrop, or Esc.
    fn about_dialog(&mut self, ui: &mut egui::Ui) {
        if !self.show_about {
            return;
        }
        let mut close = false;
        let modal = egui::Modal::new(Id::new("devmgr-about-dialog")).show(ui.ctx(), |ui| {
            ui.set_width(Style::SP_XL * 16.0);
            ui.set_max_height(Style::SP_XL * 18.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("About")
                        .color(Style::TEXT_DIM)
                        .size(Style::SMALL)
                        .strong(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    close = ui
                        .button(RichText::new("\u{00D7}").size(Style::BODY)) // ×
                        .on_hover_text("Close")
                        .clicked();
                });
            });
            ui.separator();
            about::about_panel(ui);
        });
        if close || modal.should_close() {
            self.show_about = false;
        }
    }
}

/// The rich per-host header card (#20): the hostname, the device count + problem
/// badge, and the summary fields — over a [`Style`]-token group.
fn header_card(ui: &mut egui::Ui, inv: &DeviceInventory) {
    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(&inv.host)
                    .color(Style::TEXT_STRONG)
                    .size(Style::TITLE)
                    .strong(),
            );
            ui.add_space(Style::SP_S);
            muted_note(ui, format!("{} devices", inv.device_count()));
            let problems = inv.problem_count();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if problems > 0 {
                    ui.colored_label(
                        Style::DANGER,
                        RichText::new(format!("\u{26A0} {problems} with problems")) // ⚠
                            .size(Style::SMALL),
                    );
                } else {
                    ui.colored_label(
                        Style::OK,
                        RichText::new("All devices OK").size(Style::SMALL),
                    );
                }
            });
        });
        ui.add_space(Style::SP_XS);
        for (label, value) in header_lines(inv) {
            field(ui, label, &value, Style::TEXT);
        }
        // Honest hint when the deep-detail tools were missing at enumeration (#15)
        // — so a thin tree reads as "tool absent", not "hardware broken".
        if !inv.tools.lshw {
            ui.add_space(Style::SP_XS);
            muted_note(ui, "Install lshw for deep DMI / firmware details.");
        }
    });
}

/// The header-card field rows (#20), derived purely from [`HostSummary`] so the
/// mapping (uptime humanized, memory in GiB, honest em-dashes) is unit-tested
/// without a GPU. An absent optional renders as an em-dash, never a fabricated
/// value (§7). Note: the published summary carries no disk total (it is not in
/// the DEVMGR-1 schema), so disk is represented by the Disk-drives category in
/// the tree rather than a header figure — no invented capacity.
fn header_lines(inv: &DeviceInventory) -> Vec<(&'static str, String)> {
    let s = &inv.summary;
    vec![
        ("OS", s.os.clone().unwrap_or_else(dash)),
        ("Kernel", s.kernel.clone().unwrap_or_else(dash)),
        ("Uptime", s.uptime_secs.map_or_else(dash, humanize_uptime)),
        ("CPU", cpu_line(s)),
        ("Memory", s.mem_total_kb.map_or_else(dash, format_mem_kb)),
    ]
}

/// The CPU field: model + logical count, whichever the summary carries (an em-dash
/// when neither).
fn cpu_line(s: &HostSummary) -> String {
    match (&s.cpu_model, s.cpu_count) {
        (Some(m), Some(n)) => format!("{m} ({n} logical)"),
        (Some(m), None) => m.clone(),
        (None, Some(n)) => format!("{n} logical CPUs"),
        (None, None) => dash(),
    }
}

/// The em-dash placeholder for an absent field (never a blank / a fake value).
fn dash() -> String {
    "\u{2014}".to_string()
}

/// Humanize an uptime in seconds to `d h m` (dropping leading zero units), e.g.
/// `48_120` → `"13h 22m"`, `90_061` → `"1d 1h 1m"`.
fn humanize_uptime(secs: u64) -> String {
    let d = secs / 86_400;
    let h = (secs % 86_400) / 3_600;
    let m = (secs % 3_600) / 60;
    if d > 0 {
        format!("{d}d {h}h {m}m")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

/// Format a `MemTotal` (in kB, as `/proc/meminfo` reports) to GiB with one
/// decimal — `16_072_192` → `"15.3 GiB"`.
#[allow(
    clippy::cast_precision_loss,
    reason = "RAM in kB is far below f32/f64's exact-integer range; a GiB display \
              rounded to one decimal loses no meaningful precision"
)]
fn format_mem_kb(kb: u64) -> String {
    let gib = kb as f64 / (1024.0 * 1024.0);
    format!("{gib:.1} GiB")
}

/// One category branch — a forced-state collapsing header (its open/closed driven
/// by the caller's expand set, #18). Returns `true` when the header was clicked
/// (the caller toggles the set). The header tints amber when the category holds a
/// problem device — a faithful MDM "attention on this branch" cue; the rich
/// per-device MDM problem codes are DEVMGR-3.
fn category_header(ui: &mut egui::Ui, cat: &DeviceCategory, open: bool) -> bool {
    let problems = cat.problem_count();
    let tone = if problems > 0 {
        Style::WARN
    } else {
        Style::TEXT
    };
    let mut title = cat.label.clone();
    if problems > 0 {
        use std::fmt::Write as _;
        let _ = write!(title, "   \u{26A0} {problems}"); // ⚠ N
    }
    let resp = egui::CollapsingHeader::new(RichText::new(title).color(tone).size(Style::BODY))
        .id_salt(("dm-cat", cat.key.as_str()))
        .open(Some(open))
        .show(ui, |ui| {
            for dev in &cat.devices {
                device_row(ui, dev);
            }
        });
    resp.header_response.clicked()
}

/// One device row — a status dot in the device's [`status_tone`], the name, and
/// (for a problem device) the honest Linux reason from the schema, dimmed. The
/// row is non-interactive in DEVMGR-2: the bottom detail drawer is DEVMGR-3, a
/// clean seam left here rather than a dead click handler.
fn device_row(ui: &mut egui::Ui, dev: &DeviceRecord) {
    ui.horizontal(|ui| {
        status_dot(ui, status_tone(dev.status));
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new(&dev.name)
                .color(Style::TEXT)
                .size(Style::SMALL),
        );
        if let Some(reason) = &dev.problem {
            ui.add_space(Style::SP_XS);
            muted_note(ui, format!("\u{2014} {reason}")); // — reason
        }
    });
}

/// The status-dot tone for a device state — the honest Linux state coloured, not
/// yet the MDM problem code (DEVMGR-3). Ok is green; a driverless device warns;
/// a degraded (error) device is danger; disabled / unknown are dim (not alarms).
const fn status_tone(status: DeviceStatus) -> egui::Color32 {
    match status {
        DeviceStatus::Ok => Style::OK,
        DeviceStatus::Degraded => Style::DANGER,
        DeviceStatus::NoDriver => Style::WARN,
        DeviceStatus::Disabled | DeviceStatus::Unknown => Style::TEXT_DIM,
    }
}

/// The honest pre-poll state (§7): a dim "?" over "reading…", drawn before the
/// first inventory read — never a fabricated tree.
fn pre_poll(ui: &mut egui::Ui, host: &str) {
    ui.add_space(Style::SP_L);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new("?")
                .color(Style::TEXT_DIM)
                .size(Style::DISPLAY),
        );
        muted_note(
            ui,
            format!("Reading the device inventory for {host}\u{2026}"),
        );
    });
}

/// The read-but-empty state (§7): the inventory dir was read but this host has
/// published nothing yet — an honest note, distinct from the pre-poll dim.
fn empty_host(ui: &mut egui::Ui, host: &str) {
    ui.add_space(Style::SP_L);
    ui.vertical_centered(|ui| {
        muted_note(ui, format!("No device inventory published for {host} yet."));
        ui.add_space(Style::SP_XS);
        muted_note(
            ui,
            "The hardware probe republishes periodically \u{2014} or press Scan.",
        );
    });
}

#[cfg(test)]
mod tests {
    use super::{
        cpu_line, format_mem_kb, header_lines, humanize_uptime, status_tone, DeviceManagerState,
        ViewMode,
    };
    use mackes_mesh_types::device_inventory::{
        category, DeviceInventory, DeviceStatus, HostSummary,
    };
    use mde_egui::{egui, Style};
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    /// A state carrying a chosen inventory + seen flag, rooted at a non-existent
    /// path so `refresh` reads an honest `None` (no real substrate touched).
    fn state_with(inv: Option<DeviceInventory>, seen: bool) -> DeviceManagerState {
        DeviceManagerState {
            workgroup_root: PathBuf::from("/nonexistent-devmgr-test-root"),
            local_host: "laptop-mm".to_string(),
            inventory: inv,
            seen,
            last_poll: None,
            expanded: BTreeSet::new(),
            view: ViewMode::ByType,
            show_about: false,
        }
    }

    /// Drive one headless frame of the surface (the same `Context::run` →
    /// tessellate path the DRM runner uses, minus the GPU) and return the drawn
    /// primitive count — proving it is a live render, not dead code.
    fn drive(state: &mut DeviceManagerState) -> usize {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1000.0, 800.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| state.show(ui));
        });
        ctx.tessellate(out.shapes, out.pixels_per_point).len()
    }

    #[test]
    fn the_tree_renders_headless_from_a_fixture_inventory() {
        let mut s = state_with(Some(DeviceInventory::fixture()), true);
        // Expand so category bodies (the device rows) render too.
        s.expand_all();
        assert!(drive(&mut s) > 0, "the device tree drew nothing");
    }

    #[test]
    fn categories_default_all_collapsed_then_expand_and_collapse_all() {
        let mut s = state_with(Some(DeviceInventory::fixture()), true);
        // #18 — every category is collapsed on open.
        assert!(s.expanded.is_empty(), "all categories collapsed on open");
        s.expand_all();
        // The fixture publishes exactly the Display + System(PCI) categories.
        assert_eq!(s.expanded.len(), 2);
        assert!(s.expanded.contains(category::DISPLAY));
        assert!(s.expanded.contains(category::PCI_DEVICES));
        // Toggling one collapses just it; toggling again re-expands it.
        s.toggle(category::DISPLAY);
        assert!(!s.expanded.contains(category::DISPLAY));
        assert!(s.expanded.contains(category::PCI_DEVICES));
        s.toggle(category::DISPLAY);
        assert!(s.expanded.contains(category::DISPLAY));
        // Collapse-all clears everything back to the #18 default.
        s.collapse_all();
        assert!(s.expanded.is_empty());
    }

    #[test]
    fn header_card_fields_derive_from_the_summary() {
        let inv = DeviceInventory::fixture();
        let lines = header_lines(&inv);
        let get = |k: &str| {
            lines
                .iter()
                .find(|(l, _)| *l == k)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        assert!(get("OS").contains("Fedora"), "OS: {}", get("OS"));
        assert!(get("Kernel").contains("fc44"), "kernel: {}", get("Kernel"));
        assert!(get("CPU").contains("i7-8650U"), "cpu: {}", get("CPU"));
        assert!(
            get("CPU").contains('8'),
            "logical count folded in: {}",
            get("CPU")
        );
        assert!(get("Memory").ends_with("GiB"), "memory: {}", get("Memory"));
        assert_ne!(get("Uptime"), "\u{2014}", "uptime present in the fixture");
        // The header badge counts (#20) come straight off the schema helpers.
        assert_eq!(inv.device_count(), 2);
        assert_eq!(inv.problem_count(), 1);
    }

    #[test]
    fn absent_summary_fields_render_as_an_em_dash_not_a_fake_value() {
        // A shallow / non-PC host (#22) carries a bare summary — every field is an
        // honest em-dash, never a fabricated total (§7).
        let inv = DeviceInventory {
            host: "vyos-edge".to_string(),
            published_at_ms: 0,
            summary: HostSummary::default(),
            tools: mackes_mesh_types::device_inventory::ToolAvailability::default(),
            categories: vec![],
        };
        for (_, value) in header_lines(&inv) {
            assert_eq!(value, "\u{2014}", "an absent field must dash, not fake");
        }
    }

    #[test]
    fn honest_pre_poll_then_an_empty_host_read() {
        // Fresh: unseen + no inventory — the dim pre-poll (§7), no fake tree.
        let mut s = state_with(None, false);
        assert!(!s.seen);
        assert!(drive(&mut s) > 0, "the pre-poll state drew nothing");
        // A read of a missing inventory dir flips `seen` but yields an honest None.
        s.refresh();
        assert!(s.seen, "seen after the first read");
        assert!(
            s.inventory.is_none(),
            "a missing inventory reads as None, not a fabricated tree"
        );
        assert!(drive(&mut s) > 0, "the empty-host state drew nothing");
    }

    #[test]
    fn only_by_type_is_wired_the_other_modes_are_disabled_seams() {
        // #3 — the View menu offers all three modes, but DEVMGR-2 wires only By
        // type; the others are honest disabled seams (§7), not stubbed renders.
        assert_eq!(ViewMode::ALL.len(), 3);
        assert!(ViewMode::ByType.is_available());
        assert!(!ViewMode::ByConnection.is_available());
        assert!(!ViewMode::ByNode.is_available());
        assert_eq!(ViewMode::default(), ViewMode::ByType);
    }

    #[test]
    fn the_info_dialog_opens_and_renders_the_about_content() {
        // #24 — the ⓘ dialog reuses the canonical identity screen; opening it must
        // render (the modal + the About body) without panicking.
        let mut s = state_with(Some(DeviceInventory::fixture()), true);
        s.show_about = true;
        assert!(drive(&mut s) > 0, "the about dialog drew nothing");
    }

    #[test]
    fn uptime_and_memory_format_honestly() {
        assert_eq!(humanize_uptime(48_120), "13h 22m");
        assert_eq!(humanize_uptime(90_061), "1d 1h 1m");
        assert_eq!(humanize_uptime(59), "0m");
        let m = format_mem_kb(16_072_192);
        assert!(m.ends_with(" GiB"), "memory unit: {m}");
        assert!(m.starts_with("15."), "16 GB laptop reads ~15.3 GiB: {m}");
    }

    #[test]
    fn status_tones_separate_ok_from_problems() {
        // Ok is the success green; the problem states are visibly distinct tones,
        // and none of them read as Ok (so a problem never renders "healthy").
        assert_eq!(status_tone(DeviceStatus::Ok), Style::OK);
        for bad in [
            DeviceStatus::NoDriver,
            DeviceStatus::Degraded,
            DeviceStatus::Disabled,
            DeviceStatus::Unknown,
        ] {
            assert_ne!(status_tone(bad), Style::OK, "{bad:?} must not read as Ok");
        }
        // A hard error is the danger tone; a driverless device warns.
        assert_eq!(status_tone(DeviceStatus::Degraded), Style::DANGER);
        assert_eq!(status_tone(DeviceStatus::NoDriver), Style::WARN);
    }

    #[test]
    fn cpu_line_degrades_over_a_partial_summary() {
        let mut s = HostSummary {
            cpu_model: Some("Intel Xeon".to_string()),
            cpu_count: Some(16),
            ..Default::default()
        };
        assert!(cpu_line(&s).contains("Intel Xeon") && cpu_line(&s).contains("16"));
        s.cpu_count = None;
        assert_eq!(cpu_line(&s), "Intel Xeon");
        s.cpu_model = None;
        s.cpu_count = Some(4);
        assert!(cpu_line(&s).contains('4'));
        s.cpu_count = None;
        assert_eq!(cpu_line(&s), "\u{2014}");
    }
}
