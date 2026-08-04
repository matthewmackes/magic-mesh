//! DEVMGR **detail-drawer render cluster** — the leaf drawing/text surface split
//! out of the Device-Manager god-module (pure relocation, no behaviour change).
//!
//! The device **property sheet**: the plain-text `render_device_details` clipboard
//! dump (#12), the bottom drawer chrome (`drawer_header`/`drawer_tabs`/`drawer_body`)
//! and its five leaf tabs (General/Driver/Details/Events/Resources, #9/#10) plus the
//! `optional_field` row helper.
//!
//! `use super::*` pulls in the parent's `DeviceRecord`/`DrawerTab`/`Style` + the egui
//! re-exports and the sibling `dash` helper; as a child
//! module it reads the parent's private items directly, so only the items the parent
//! (and the tests) call back into are `pub(super)`.

use super::*;

/// The full device detail dump copied to the clipboard by DEVMGR-7's **Copy device
/// details** action — every field the five drawer tabs render (#10), as a plain-text
/// block: identity (name / manufacturer / model / hardware IDs), the MDM device
/// bound driver, the sysfs details, and the resources + recent events.
/// Absent scalar fields read as an honest em-dash and empty lists as a "none
/// reported" line (§7 — never a fabricated value), mirroring the tabs. Pure, so the
/// dump is unit-tested without a render.
pub(super) fn render_device_details(dev: &DeviceRecord) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let opt = |v: Option<&str>| v.map_or_else(dash, str::to_string);
    let _ = writeln!(out, "Device: {}", dev.name);
    let _ = writeln!(out, "Manufacturer: {}", opt(dev.vendor.as_deref()));
    let _ = writeln!(out, "Model: {}", opt(dev.model.as_deref()));
    let _ = writeln!(out, "Hardware IDs: {}", opt(dev.ids.as_deref()));
    let _ = writeln!(out, "Driver: {}", opt(dev.driver.as_deref()));
    let _ = writeln!(
        out,
        "Driver version: {}",
        opt(dev.driver_version.as_deref())
    );
    let _ = writeln!(out, "sysfs path: {}", opt(dev.sysfs_path.as_deref()));
    // Resources (the Resources tab, #10) — each present line, else an honest none.
    let r = &dev.resources;
    let _ = writeln!(out, "Resources:");
    if r.is_empty() {
        let _ = writeln!(out, "  none reported");
    } else {
        if let Some(irq) = r.irq {
            let _ = writeln!(out, "  IRQ: {irq}");
        }
        for (label, list) in [
            ("I/O ports", &r.io_ports),
            ("Memory range", &r.memory),
            ("DMA", &r.dma),
        ] {
            for value in list {
                let _ = writeln!(out, "  {label}: {value}");
            }
        }
    }
    // Events (the Events tab, #10) — recent dmesg / udev lines, else an honest none.
    let _ = writeln!(out, "Events:");
    if dev.events.is_empty() {
        let _ = writeln!(out, "  none reported");
    } else {
        for line in &dev.events {
            let _ = writeln!(out, "  {line}");
        }
    }
    out
}

// ─────────────────────────── the detail drawer (#9/#10) ─────────────────────

/// The filled glyph used by neutral inventory chips.
pub(super) const DOT: &str = "\u{25CF}";

/// The drawer's title row (#9): the selected device name, with a
/// DEVMGR-7 **Copy details** button + a `×` close button on the right — the
/// non-right-click path to the honest Copy-info action (#12).
pub(super) fn drawer_header(
    ui: &mut egui::Ui,
    dev: &DeviceRecord,
    close: &mut bool,
    copy: &mut bool,
) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(&dev.name)
                .color(Style::TEXT_STRONG)
                .size(Style::BODY)
                .strong(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let close_response = ui.button(RichText::new("\u{00D7}").size(Style::BODY)); // ×
            if devmgr_hover_text(close_response, "Close the device details").clicked() {
                *close = true;
            }
            let copy_response = ui.button(RichText::new("\u{29C9}").size(Style::BODY)); // ⧉ — copy details
            if devmgr_hover_text(copy_response, "Copy this device's details to the clipboard")
                .clicked()
            {
                *copy = true;
            }
        });
    });
}

/// The drawer's tab strip (#10): the five MDM tabs as selectable labels, updating
/// the caller's active-tab.
pub(super) fn drawer_tabs(ui: &mut egui::Ui, tab: &mut DrawerTab) {
    ui.horizontal(|ui| {
        for t in DrawerTab::ALL {
            if ui.selectable_label(*tab == t, t.label()).clicked() {
                *tab = t;
            }
        }
    });
}

/// The drawer's body (#10): the active tab's fields, in a scroll so a long Events /
/// Resources list never blows the panel. Every tab renders only real record data,
/// with an honest empty state when a field is absent (§7).
pub(super) fn drawer_body(ui: &mut egui::Ui, dev: &DeviceRecord, tab: DrawerTab) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| match tab {
            DrawerTab::General => general_tab(ui, dev),
            DrawerTab::Driver => driver_tab(ui, dev),
            DrawerTab::Details => details_tab(ui, dev),
            DrawerTab::Events => events_tab(ui, dev),
            DrawerTab::Resources => resources_tab(ui, dev),
        });
}

/// The **General** tab (#10): identity (name / manufacturer / model) plus the MDM
/// inventory identity. Platform faults are deliberately presented by the central
/// System and Mesh Health modal, not duplicated in this hardware browser.
fn general_tab(ui: &mut egui::Ui, dev: &DeviceRecord) {
    field(ui, "Device name", &dev.name, Style::TEXT);
    optional_field(ui, "Manufacturer", dev.vendor.as_deref());
    optional_field(ui, "Model", dev.model.as_deref());
}

/// The **Driver** tab (#10): the bound kernel driver / module + its version. An
/// honestly-empty tab when no driver is bound.
fn driver_tab(ui: &mut egui::Ui, dev: &DeviceRecord) {
    if dev.driver.is_none() && dev.driver_version.is_none() {
        muted_note(ui, "No kernel driver is bound to this device.");
        return;
    }
    optional_field(ui, "Driver", dev.driver.as_deref());
    optional_field(ui, "Driver version", dev.driver_version.as_deref());
}

/// The **Details** tab (#10): the sysfs path + the `vendor:product` hardware IDs —
/// the Linux mapping of MDM's property IDs. Honestly empty when neither was read.
fn details_tab(ui: &mut egui::Ui, dev: &DeviceRecord) {
    if dev.sysfs_path.is_none() && dev.ids.is_none() {
        muted_note(ui, "No sysfs path or hardware IDs were reported.");
        return;
    }
    optional_field(ui, "Hardware IDs", dev.ids.as_deref());
    optional_field(ui, "sysfs path", dev.sysfs_path.as_deref());
}

/// The **Events** tab (#10): the recent dmesg / udev lines mentioning this device,
/// in mono. Honestly empty when none were captured.
fn events_tab(ui: &mut egui::Ui, dev: &DeviceRecord) {
    if dev.events.is_empty() {
        muted_note(ui, "No recent kernel or udev events for this device.");
        return;
    }
    for line in &dev.events {
        ui.label(
            RichText::new(line)
                .family(egui::FontFamily::Monospace)
                .color(Style::TEXT)
                .size(Style::SMALL),
        );
    }
}

/// The **Resources** tab (#10): the IRQ / I/O-port / memory-window / DMA resources
/// the device holds. Honestly empty when the enumerator resolved none.
fn resources_tab(ui: &mut egui::Ui, dev: &DeviceRecord) {
    let r = &dev.resources;
    if r.is_empty() {
        muted_note(ui, "No IRQ, I/O, memory, or DMA resources were reported.");
        return;
    }
    if let Some(irq) = r.irq {
        field(ui, "IRQ", &irq.to_string(), Style::TEXT);
    }
    for (label, list) in [
        ("I/O ports", &r.io_ports),
        ("Memory range", &r.memory),
        ("DMA", &r.dma),
    ] {
        for value in list {
            field(ui, label, value, Style::TEXT);
        }
    }
}

/// A labelled field that renders an honest em-dash when the value is absent (§7),
/// so a drawer tab never leaves a blank or fabricates a value.
fn optional_field(ui: &mut egui::Ui, label: &str, value: Option<&str>) {
    match value {
        Some(v) => field(ui, label, v, Style::TEXT),
        None => field(ui, label, &dash(), Style::TEXT_DIM),
    }
}
