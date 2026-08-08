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
use mde_egui::{card, field, section, Style, TypographyRole};

use mackes_mesh_types::cloud::{StoragePool, WorkloadSpec};

use super::{DeliveryView, ProvisionProgress, WorkloadsState};

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
    /// Host-local VM/container storage target selected in Deployment.
    storage_pool: StoragePool,
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
            storage_pool: StoragePool::default(),
            image: String::new(),
            network_isolation: false,
            raw_hcl: String::new(),
        }
    }
}

impl State {
    /// Stable identity for the exact draft governed by the guided-flow
    /// milestones. Changing any authoring input invalidates the prior plan.
    pub(super) fn workflow_key(&self, view: DeliveryView, node: &str) -> String {
        format!(
            "{:?}|{}|{}|{}|{}|{}|{:?}|{}|{}|{}",
            view,
            node.trim(),
            self.name.trim(),
            self.vcpu,
            self.memory_mb,
            self.disk_gb,
            self.storage_pool,
            self.image.trim(),
            self.network_isolation,
            self.raw_hcl.trim()
        )
    }

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
            storage_pool: self.storage_pool,
            image: non_empty(&self.image),
            image_digest: None,
            network_isolation: self.network_isolation,
            raw_hcl: non_empty(&self.raw_hcl),
            app: None,
        }
    }

    /// Whether the draft can be authored — a non-blank name is required.
    fn is_valid(&self) -> bool {
        !self.name.trim().is_empty()
    }

    /// Test seam for headless render fixtures that need a valid draft without
    /// synthesizing keyboard input through egui.
    #[cfg(test)]
    pub(super) fn set_test_draft(&mut self, name: &str, image: &str, raw_hcl: &str) {
        self.name = name.to_string();
        self.image = image.to_string();
        self.raw_hcl = raw_hcl.to_string();
    }
}

/// The live provision affordance requires both a valid draft and positive
/// capability evidence from the selected placement node. Plan-only nodes can
/// still author/plan, but must not open a live-apply arm that the worker cannot
/// honor.
pub(super) const fn live_provision_enabled(valid: bool, apply_armed: bool) -> bool {
    valid && apply_armed
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

    let valid = state.form.is_valid();
    let live_apply_available = state.selected_node_apply_armed();
    let draft_key = state.form.workflow_key(view, &node);
    state.sync_provision_draft(draft_key);
    let progress = state.provision_progress();
    let request_pending = state.mutation_is_pending();
    provision_target_summary(ui, view, &node, live_apply_available);
    ui.add_space(Style::SP_S);

    let android_name = state.form.name.trim().to_string();

    let mut set_desired = false;
    let mut plan = false;
    let mut provision = false;
    let mut android_prepare = false;
    provision_workspace(
        ui,
        view,
        &mut state.form,
        valid,
        live_apply_available,
        progress,
        request_pending,
        &mut set_desired,
        &mut plan,
        &mut provision,
        &mut android_prepare,
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
    if android_prepare {
        state.arm_android_provision(&android_name);
    }
}

#[allow(clippy::too_many_arguments)]
fn provision_workspace(
    ui: &mut egui::Ui,
    view: DeliveryView,
    form: &mut State,
    valid: bool,
    live_apply_available: bool,
    progress: ProvisionProgress,
    request_pending: bool,
    set_desired: &mut bool,
    plan: &mut bool,
    provision: &mut bool,
    android_prepare: &mut bool,
) {
    guided_progress(ui, progress, valid);
    ui.add_space(Style::SP_S);
    next_step_panel(ui, |ui| {
        provision_action_controls(
            ui,
            view,
            valid,
            live_apply_available,
            progress,
            request_pending,
            set_desired,
            plan,
            provision,
            android_prepare,
        );
    });
    ui.add_space(Style::SP_S);
    provision_editor(ui, form);
    ui.add_space(Style::SP_S);
    hcl_override_section(ui, form, false);
    ui.add_space(Style::SP_S);
    validation_section(ui, form, live_apply_available, false);
}

#[allow(clippy::too_many_arguments)]
fn provision_action_controls(
    ui: &mut egui::Ui,
    view: DeliveryView,
    valid: bool,
    live_apply_available: bool,
    progress: ProvisionProgress,
    request_pending: bool,
    set_desired: &mut bool,
    plan: &mut bool,
    provision: &mut bool,
    android_prepare: &mut bool,
) {
    if request_pending {
        mde_egui::muted_note(
            ui,
            "Waiting for the cloud backend. This step remains active until its typed result returns.",
        );
        return;
    }

    match progress {
        ProvisionProgress::Draft => {
            let (label, detail) = if view == DeliveryView::AndroidVm {
                (
                    "Prepare Android VM\u{2026}",
                    "Step 3 of 5 — save the governed Cuttlefish desired state.",
                )
            } else {
                (
                    "Save desired state\u{2026}",
                    "Step 3 of 5 — save this workload draft before planning it.",
                )
            };
            mde_egui::muted_note(ui, detail);
            if action_button(ui, valid, label, Style::ACCENT_WORKLOADS).clicked() {
                if view == DeliveryView::AndroidVm {
                    *android_prepare = true;
                } else {
                    *set_desired = true;
                }
            }
        }
        ProvisionProgress::DesiredSaved => {
            mde_egui::muted_note(
                ui,
                "Step 4 of 5 — run the required dry-run plan. Nothing is applied.",
            );
            if action_button(ui, valid, "Run dry-run plan", Style::ACCENT).clicked() {
                *plan = true;
            }
        }
        ProvisionProgress::Planned => {
            mde_egui::muted_note(
                ui,
                "Step 5 of 5 — review and apply the plan to the selected node.",
            );
            if action_button(
                ui,
                live_provision_enabled(valid, live_apply_available),
                "Provision live\u{2026}",
                Style::DANGER,
            )
            .clicked()
            {
                *provision = true;
            }
            if !live_apply_available {
                mde_egui::muted_note(
                    ui,
                    "Live apply is unavailable because this node currently reports plan-only.",
                );
            }
        }
        ProvisionProgress::Applied => {
            ui.colored_label(
                Style::ACCENT,
                RichText::new("✓ Provisioning complete")
                    .size(Style::BODY)
                    .strong(),
            );
        }
    }
    if !live_apply_available && progress < ProvisionProgress::Applied {
        mde_egui::muted_note(
            ui,
            "Plan-only node: desired state and dry-run are available; live provision will remain disabled.",
        );
    }
}

fn provision_target_summary(
    ui: &mut egui::Ui,
    view: DeliveryView,
    node: &str,
    live_apply_available: bool,
) {
    let compact = ui.available_width() >= 760.0;
    card().show(ui, |ui| {
        ui.label(
            RichText::new("Placement & delivery")
                .size(Style::SMALL)
                .strong()
                .color(Style::TEXT_DIM),
        );
        if compact {
            ui.horizontal_wrapped(|ui| {
                inline_summary_field(ui, "Placement node", node, Style::ACCENT_WORKLOADS);
                inline_summary_field(ui, "Delivery filter", view.label(), Style::TEXT);
                inline_summary_field(
                    ui,
                    "Live apply gate",
                    if live_apply_available {
                        "Armed by current mirror"
                    } else {
                        "Plan-only / not armed"
                    },
                    if live_apply_available {
                        Style::ACCENT
                    } else {
                        Style::TEXT_DIM
                    },
                );
            });
        } else {
            ui.add_space(Style::SP_XS);
            field(ui, "Placement node", node, Style::ACCENT_WORKLOADS);
            field(ui, "Delivery filter", view.label(), Style::TEXT);
            field(
                ui,
                "Live apply gate",
                if live_apply_available {
                    "Armed by current mirror"
                } else {
                    "Plan-only / not armed"
                },
                if live_apply_available {
                    Style::ACCENT
                } else {
                    Style::TEXT_DIM
                },
            );
        }
    });
}

fn inline_summary_field(ui: &mut egui::Ui, label: &str, value: &str, tone: Color32) {
    ui.label(
        RichText::new(label)
            .size(Style::SMALL)
            .color(Style::TEXT_DIM),
    );
    ui.label(RichText::new(value).size(Style::SMALL).color(tone));
    ui.add_space(Style::SP_M);
}

fn provision_editor(ui: &mut egui::Ui, form: &mut State) {
    card().show(ui, |ui| {
        ui.label(
            RichText::new("Identity")
                .size(Style::SMALL)
                .strong()
                .color(Style::TEXT_DIM),
        );
        labelled(ui, "Name", |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut form.name)
                    .hint_text("workload name")
                    .desired_width(Style::SP_XL * 6.0),
            );
        });
    });
    ui.add_space(Style::SP_S);

    card().show(ui, |ui| {
        ui.label(
            RichText::new("Sizing")
                .size(Style::SMALL)
                .strong()
                .color(Style::TEXT_DIM),
        );
        ui.add_space(Style::SP_XS);
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
    });
    ui.add_space(Style::SP_S);

    card().show(ui, |ui| {
        ui.label(
            RichText::new("Image & network")
                .size(Style::SMALL)
                .strong()
                .color(Style::TEXT_DIM),
        );
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
        labelled(ui, "Storage", |ui| {
            egui::ComboBox::from_id_salt("provision-storage-pool")
                .selected_text(form.storage_pool.label())
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut form.storage_pool,
                        StoragePool::LocalXfs,
                        StoragePool::LocalXfs.label(),
                    );
                    ui.selectable_value(
                        &mut form.storage_pool,
                        StoragePool::DefaultLibvirt,
                        StoragePool::DefaultLibvirt.label(),
                    );
                });
        });
        mde_egui::muted_note(
            ui,
            "Local XFS stores VM disks and container data on the host's dedicated /var/lib/mde-vms partition.",
        );
    });
}

fn hcl_override_section(ui: &mut egui::Ui, form: &mut State, compact: bool) {
    card().show(ui, |ui| {
        ui.label(
            RichText::new("HCL override")
                .size(Style::SMALL)
                .strong()
                .color(Style::TEXT_DIM),
        );
        if compact {
            ui.add(
                egui::TextEdit::multiline(&mut form.raw_hcl)
                    .font(Style::typography_font(TypographyRole::Mono))
                    .desired_rows(1)
                    .desired_width(f32::INFINITY)
                    .hint_text("# optional HCL fragment"),
            );
        } else {
            mde_egui::muted_note(
                ui,
                "Advanced raw-HCL fragment. It is merged into rendered tfvars and validated before tofu; \
                 leave blank for pure form authoring.",
            );
            mde_egui::inset().show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut form.raw_hcl)
                        .font(Style::typography_font(TypographyRole::Mono))
                        .desired_rows(4)
                        .desired_width(f32::INFINITY)
                        .hint_text("# optional HCL fragment"),
                );
            });
        }
    });
}

fn validation_section(ui: &mut egui::Ui, form: &State, live_apply_available: bool, compact: bool) {
    card().show(ui, |ui| {
        ui.label(
            RichText::new("Validation")
                .size(Style::SMALL)
                .strong()
                .color(Style::TEXT_DIM),
        );
        if compact {
            validation_live_apply_row(ui, live_apply_available);
        }
        validation_row(
            ui,
            form.is_valid(),
            "Name",
            if form.is_valid() {
                "ready"
            } else {
                "required before Set desired, Plan, or Provision"
            },
        );
        validation_row(
            ui,
            true,
            "Sizing",
            if compact {
                "bounded"
            } else {
                "bounded to the Workloads contract before request emission"
            },
        );
        if !compact {
            validation_live_apply_row(ui, live_apply_available);
        }
    });
}

fn validation_live_apply_row(ui: &mut egui::Ui, live_apply_available: bool) {
    validation_row(
        ui,
        live_apply_available,
        "Live apply",
        if live_apply_available {
            "selected node reports an armed apply capability"
        } else {
            "Plan remains available; live Provision stays disabled"
        },
    );
}

fn validation_row(ui: &mut egui::Ui, ok: bool, label: &str, detail: &str) {
    ui.horizontal_wrapped(|ui| {
        let (glyph, tone) = if ok {
            ("✓", Style::ACCENT)
        } else {
            ("!", Style::DANGER)
        };
        ui.label(RichText::new(glyph).size(Style::SMALL).color(tone).strong());
        ui.label(
            RichText::new(label)
                .size(Style::SMALL)
                .color(Style::TEXT)
                .strong(),
        );
        ui.label(
            RichText::new(detail)
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
    });
}

/// Always-visible map of the one shared provisioning sequence. It replaces the
/// unrelated-looking "Sticky actions" tray with an explicit ordered contract.
fn guided_progress(ui: &mut egui::Ui, progress: ProvisionProgress, valid: bool) {
    card().show(ui, |ui| {
        ui.label(
            RichText::new("Guided provisioning")
                .size(Style::BODY)
                .strong()
                .color(Style::TEXT),
        );
        ui.horizontal_wrapped(|ui| {
            for (index, (label, complete)) in [
                ("Placement", true),
                ("Workload details", valid),
                ("Desired state", progress >= ProvisionProgress::DesiredSaved),
                ("Dry-run plan", progress >= ProvisionProgress::Planned),
                ("Live provision", progress >= ProvisionProgress::Applied),
            ]
            .into_iter()
            .enumerate()
            {
                if index > 0 {
                    ui.label(RichText::new("→").color(Style::TEXT_DIM));
                }
                let tone = if complete { Style::ACCENT } else { Style::TEXT_DIM };
                ui.label(
                    RichText::new(format!("{} {label}", index + 1))
                        .size(Style::SMALL)
                        .strong()
                        .color(tone),
                );
            }
        });
        mde_egui::muted_note(
            ui,
            "Complete the highlighted sequence in order. Only the next valid action is offered below.",
        );
    });
}

/// The single next action, placed in document order after validation. Only one
/// primary control is shown so navigation labels cannot be mistaken for work.
fn next_step_panel(ui: &mut egui::Ui, add_action: impl FnOnce(&mut egui::Ui)) {
    ui.scope(|ui| {
        ui.visuals_mut().widgets.noninteractive.bg_fill = Style::SURFACE_HI;
        card().show(ui, |ui| {
            ui.label(
                RichText::new("Next step")
                    .size(Style::BODY)
                    .strong()
                    .color(Style::ACCENT_WORKLOADS),
            );
            ui.add_space(Style::SP_XS);
            add_action(ui);
        });
    });
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
    fn live_provision_requires_a_valid_draft_and_armed_node() {
        assert!(!live_provision_enabled(false, true));
        assert!(!live_provision_enabled(true, false));
        assert!(live_provision_enabled(true, true));
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

    #[test]
    fn android_provision_body_keeps_the_dedicated_contract_and_default_name() {
        let body: serde_json::Value = serde_json::from_str(
            &super::super::android_provision_request_body(" eagle ", "  "),
        )
        .expect("android-provision request encodes");
        assert_eq!(body["schema_version"], 1);
        assert_eq!(body["node"], "eagle");
        assert_eq!(body["name"], "");
    }
}
