//! U15 — the **provision form**: author a [`mackes_mesh_types::cloud::WorkloadSpec`]
//! (delivery type · sizing · image · network isolation) with the raw-HCL escape
//! hatch, then hand it to `set-desired` / `plan` / armed `provision`.
//!
//! The delivery type follows the active [`DeliveryView`]; the node is whatever the
//! placement picker selected (`None` reads as an honest "pick a node first").
//! **Set desired** persists the authored spec, **Plan** dry-runs it (counts only),
//! and **Provision** opens the typed-arm before any live apply — never a fake
//! apply (§7): a live apply only ever reaches the Bus past the arming gate.

use mde_egui::egui::{self, Color32, Response, RichText};
use mde_egui::{card, section, Style};

use mackes_mesh_types::cloud::WorkloadSpec;

use super::{DeliveryView, WorkloadsState};

/// The provision form's own draft spec (U15 owns these fields). Defaults size a
/// modest VM; the operator tunes them before authoring.
#[derive(Debug)]
pub(super) struct State {
    /// The workload name (unique within the placement node) — required.
    name: String,
    /// Virtual CPUs.
    vcpu: u16,
    /// Memory in MiB.
    memory_mb: u32,
    /// Root disk in GiB.
    disk_gb: u32,
    /// The base image name; blank = the delivery type's golden default (`None`).
    image: String,
    /// Whether the workload gets its own isolated network segment.
    network_isolation: bool,
    /// The raw-HCL escape hatch merged into the rendered tfvars; blank = `None`.
    raw_hcl: String,
}

impl Default for State {
    fn default() -> Self {
        Self {
            name: String::new(),
            vcpu: 2,
            memory_mb: 4096,
            disk_gb: 40,
            image: String::new(),
            network_isolation: false,
            raw_hcl: String::new(),
        }
    }
}

impl State {
    /// Author the wire [`WorkloadSpec`] from the draft, for `view`'s delivery type
    /// on `node`. Blank image / raw-HCL fold to `None` (the honest "unset" — a
    /// golden default / pure-form authoring), and the name is trimmed.
    fn build_spec(&self, view: DeliveryView, node: &str) -> WorkloadSpec {
        WorkloadSpec {
            name: self.name.trim().to_string(),
            delivery_type: view.delivery_type(),
            node: node.to_string(),
            vcpu: self.vcpu,
            memory_mb: self.memory_mb,
            disk_gb: self.disk_gb,
            image: non_empty(&self.image),
            network_isolation: self.network_isolation,
            raw_hcl: non_empty(&self.raw_hcl),
        }
    }

    /// Whether the draft can be authored — a non-blank name is required.
    fn is_valid(&self) -> bool {
        !self.name.trim().is_empty()
    }
}

/// Trim a field, folding blank to `None` (the honest "unset").
fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// Render the provision form for the placement node the picker selected.
#[allow(clippy::too_many_lines)]
pub(super) fn provision_form(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    let view = state.view;
    let node = state.selected_node().map(str::to_string);

    section().show(ui, |ui| {
        ui.label(
            RichText::new("Provision")
                .size(Style::BODY)
                .strong()
                .color(Style::TEXT),
        );
        mde_egui::muted_note(
            ui,
            "Author a workload spec for the selected node, then set it desired, plan it, or \
             provision it.",
        );
    });

    let Some(node) = node else {
        crate::empty_state::show(
            ui,
            "No placement node selected",
            "Pick a node in the placement picker above; the provision form targets it.",
        );
        return;
    };

    mde_egui::field(ui, "Node", &node, Style::ACCENT_WORKLOADS);
    mde_egui::field(ui, "Delivery type", view.label(), Style::TEXT);
    ui.add_space(Style::SP_S);

    card().show(ui, |ui| {
        let form = &mut state.form;

        labelled(ui, "Name", |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut form.name)
                    .hint_text("workload name")
                    .desired_width(Style::SP_XL * 6.0),
            );
        });

        ui.horizontal(|ui| {
            size_field(ui, "vCPU");
            ui.add(egui::DragValue::new(&mut form.vcpu).range(1..=256));
            ui.add_space(Style::SP_M);
            size_field(ui, "Memory");
            ui.add(
                egui::DragValue::new(&mut form.memory_mb)
                    .range(256..=1_048_576)
                    .suffix(" MiB"),
            );
            ui.add_space(Style::SP_M);
            size_field(ui, "Disk");
            ui.add(
                egui::DragValue::new(&mut form.disk_gb)
                    .range(1..=8192)
                    .suffix(" GiB"),
            );
        });
        ui.add_space(Style::SP_XS);

        labelled(ui, "Image", |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut form.image)
                    .hint_text("golden default")
                    .desired_width(Style::SP_XL * 6.0),
            );
        });

        ui.checkbox(
            &mut form.network_isolation,
            RichText::new("Isolated network segment")
                .size(Style::SMALL)
                .color(Style::TEXT),
        );
        ui.add_space(Style::SP_S);

        ui.label(
            RichText::new("Raw HCL (advanced)")
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
        mde_egui::muted_note(
            ui,
            "Merged into the rendered tfvars and validated before tofu. Leave blank for pure form \
             authoring.",
        );
        mde_egui::inset().show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut form.raw_hcl)
                    .font(egui::TextStyle::Monospace)
                    .desired_rows(4)
                    .desired_width(f32::INFINITY)
                    .hint_text("# optional HCL fragment"),
            );
        });
    });

    ui.add_space(Style::SP_S);
    let valid = state.form.is_valid();
    if !valid {
        mde_egui::muted_note(
            ui,
            "A workload name is required before it can be set desired, planned, or provisioned.",
        );
    }

    let mut set_desired = false;
    let mut plan = false;
    let mut provision = false;
    ui.horizontal(|ui| {
        if action_button(ui, valid, "Set desired", Style::ACCENT_WORKLOADS).clicked() {
            set_desired = true;
        }
        ui.add_space(Style::SP_S);
        if action_button(ui, valid, "Plan", Style::ACCENT).clicked() {
            plan = true;
        }
        ui.add_space(Style::SP_S);
        if action_button(ui, valid, "Provision\u{2026}", Style::DANGER).clicked() {
            provision = true;
        }
    });
    mde_egui::muted_note(
        ui,
        "Set desired persists the spec; Plan is a dry-run (counts only); Provision opens a \
         typed-arm before any live apply.",
    );

    // Dispatch past the form's `&mut` borrow — one distinct emit per button, so no
    // two mutations race the single in-flight reply slot.
    if set_desired {
        let spec = state.form.build_spec(view, &node);
        state.set_desired(&spec);
    }
    if plan {
        state.plan_provision();
    }
    if provision {
        state.arm_provision();
    }
}

/// A dim caption for a sizing control (the shared `vCPU`/`Memory`/`Disk` label).
fn size_field(ui: &mut egui::Ui, label: &str) {
    ui.label(
        RichText::new(label)
            .size(Style::SMALL)
            .color(Style::TEXT_DIM),
    );
}

/// A labelled input row — a dim caption, a gutter, then the caller's widget.
fn labelled(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label)
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
        ui.add_space(Style::SP_S);
        add(ui);
    });
}

/// A form action button, `accent`-toned and disabled (never hidden) until the
/// draft is valid.
fn action_button(ui: &mut egui::Ui, enabled: bool, label: &str, accent: Color32) -> Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(label).size(Style::SMALL).color(accent)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::cloud::DeliveryType;

    #[test]
    fn build_spec_authors_the_wire_shape_from_the_view_and_node() {
        let form = State {
            name: "  seat-1  ".to_string(),
            vcpu: 4,
            memory_mb: 8192,
            disk_gb: 60,
            network_isolation: true,
            ..State::default()
        };
        let spec = form.build_spec(DeliveryView::DesktopVm, "eagle");
        assert_eq!(spec.name, "seat-1", "the name is trimmed");
        assert_eq!(spec.delivery_type, DeliveryType::DesktopVm);
        assert_eq!(spec.node, "eagle");
        assert_eq!(spec.vcpu, 4);
        assert_eq!(spec.memory_mb, 8192);
        assert_eq!(spec.disk_gb, 60);
        assert!(spec.network_isolation);
        assert!(spec.image.is_none(), "blank image → golden default (None)");
        assert!(spec.raw_hcl.is_none(), "blank HCL → None");
    }

    #[test]
    fn image_and_raw_hcl_escape_hatches_fill_when_set() {
        let form = State {
            name: "svc".to_string(),
            image: "fedora-42".to_string(),
            raw_hcl: "  memory = 2048  ".to_string(),
            ..State::default()
        };
        let spec = form.build_spec(DeliveryView::ServiceVm, "bigboy");
        assert_eq!(spec.image.as_deref(), Some("fedora-42"));
        assert_eq!(spec.raw_hcl.as_deref(), Some("memory = 2048"), "trimmed");
        assert_eq!(spec.delivery_type, DeliveryType::ServiceVm);
    }

    #[test]
    fn a_blank_name_is_not_valid() {
        let mut form = State::default();
        assert!(!form.is_valid(), "empty name blocks authoring");
        form.name = "   ".to_string();
        assert!(!form.is_valid(), "whitespace-only name blocks authoring");
        form.name = "ok".to_string();
        assert!(form.is_valid());
    }

    #[test]
    fn set_desired_serializes_the_worker_envelope() {
        let form = State {
            name: "seat".to_string(),
            ..State::default()
        };
        let spec = form.build_spec(DeliveryView::DesktopVm, "eagle");
        let body: serde_json::Value =
            serde_json::from_str(&super::super::set_desired_request_body(&spec))
                .expect("the set-desired envelope encodes");
        assert_eq!(body["node"], "eagle");
        assert_eq!(body["schema_version"], 1);
        assert_eq!(body["spec"], serde_json::to_value(&spec).unwrap());
        assert!(
            body.get("name").is_none(),
            "the workload spec must not be published bare at the JSON root"
        );
    }
}
