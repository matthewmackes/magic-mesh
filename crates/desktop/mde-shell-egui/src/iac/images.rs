//! U19 — the **Images** lens: the golden per-delivery-type image roster and the
//! `image-build` affordance (`build` / `list` / `promote`).
//!
//! A golden image is a bootc image-mode disk built by `bootc-image-builder`
//! (osbuild under the hood) and landed in the mesh's **Syncthing-replicated image
//! store** — the airgap distribution lane, so a built base replicates to every
//! peer with no egress. A SHA256 content-hash sidecar rides alongside each image
//! ([`mackes_mesh_types::cloud::ImageRow`] carries `name` · `sha256` · `promoted`),
//! and `promote` re-verifies that hash before marking a version the active base.
//!
//! This lens emits the [`VERB_IMAGE_BUILD`] verb through the cockpit's preserved
//! emit path (the same `issue` seam the provision lens uses); a live build/promote
//! is armed-token gated on the placement node, so an un-armed request stages / plans
//! honestly rather than performing. The roster itself is sourced from the
//! `image-build` / `list` reply, but the shell's `CloudReply` mirror does not decode
//! the [`ImageRow`] list yet — so the roster reads an honest "pending backend
//! decode" note (never fabricated rows, §7) until that decode lands.

use mde_egui::egui::{self, RichText};
use mde_egui::{carbon_icon, Style};

use mackes_mesh_types::cloud::{DeliveryType, VERB_IMAGE_BUILD};

use super::WorkloadsState;

/// The delivery types a golden VM image can be built for. A `ServiceContainer`
/// workload has no golden VM disk — it ships via `container-deploy` (the Containers
/// lens), which the backend enforces — so it is omitted here.
const BUILDABLE: [DeliveryType; 4] = [
    DeliveryType::DesktopVm,
    DeliveryType::ServiceVm,
    DeliveryType::AppVm,
    DeliveryType::AndroidVm,
];

/// The Images lens's own state (U19 owns its fields): the delivery type the build
/// controls target plus the optional name / version overrides.
#[derive(Debug)]
pub(super) struct State {
    /// The delivery type whose golden image the build / promote controls act on.
    dtype: DeliveryType,
    /// An optional image-name override; blank ⇒ the `<delivery_type>-golden`
    /// default the backend derives.
    name: String,
    /// An optional version; blank ⇒ `latest` (build) / the resolved default.
    version: String,
}

impl Default for State {
    fn default() -> Self {
        Self {
            dtype: DeliveryType::DesktopVm,
            name: String::new(),
            version: String::new(),
        }
    }
}

/// Build the JSON request body for an `image-build` sub-action (`build` / `list` /
/// `promote`). A blank `name` / `version` is sent through unchanged — the backend
/// trims + filters them and derives the `<delivery_type>-golden` / `latest`
/// defaults, so the shell never invents one.
fn build_request_body(
    action: &str,
    dtype: DeliveryType,
    name: &str,
    version: &str,
    node: &str,
) -> String {
    serde_json::json!({
        "action": action,
        "delivery_type": dtype,
        "name": name.trim(),
        "version": version.trim(),
        "node": node,
    })
    .to_string()
}

/// Render the Images lens.
pub(super) fn images_panel(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    header(ui);
    ui.add_space(Style::SP_S);

    if let Some(action) = build_controls(ui, state) {
        // Snapshot the request inputs (owned) so the immutable field borrows end
        // before the mutable emit seam runs.
        let node = state
            .selected_node()
            .map(str::to_string)
            .unwrap_or_default();
        let dtype = state.images.dtype;
        let body = build_request_body(
            action,
            dtype,
            &state.images.name,
            &state.images.version,
            &node,
        );
        let label = match action {
            "list" => "golden-image roster refresh".to_string(),
            "promote" => format!("promote golden image for {}", dtype.label()),
            _ => format!("golden image build for {}", dtype.label()),
        };
        state.issue(VERB_IMAGE_BUILD, Some(&body), &label);
    }

    ui.add_space(Style::SP_S);
    image_roster(ui);
}

/// The lens header card — the Workloads-accent glyph, the title, and the honest
/// provenance / airgap-lane blurb.
fn header(ui: &mut egui::Ui) {
    mde_egui::card().show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.scope(|ui| {
                ui.visuals_mut().override_text_color = Some(Style::ACCENT_WORKLOADS);
                carbon_icon(ui, "camera-photo", Style::BODY + 2.0);
            });
            ui.add_space(Style::SP_XS);
            ui.label(
                RichText::new("Golden images")
                    .size(Style::BODY)
                    .strong()
                    .color(Style::TEXT),
            );
        });
        mde_egui::muted_note(
            ui,
            "Per-delivery-type bootc / osbuild disks, built by bootc-image-builder and \
             replicated over the Syncthing airgap lane with a SHA256 content-hash sidecar \
             (no egress). Promote re-verifies that hash before it becomes the active base.",
        );
    });
}

/// The build / promote / list controls. Returns the chosen `image-build`
/// sub-action (if a button was clicked this frame) so the caller can emit it past
/// the lens's own state borrow.
fn build_controls(ui: &mut egui::Ui, state: &mut WorkloadsState) -> Option<&'static str> {
    let mut action: Option<&'static str> = None;
    mde_egui::card().show(ui, |ui| {
        ui.label(
            RichText::new("Build a golden image")
                .size(Style::BODY)
                .strong()
                .color(Style::TEXT),
        );
        ui.add_space(Style::SP_XS);

        // Delivery-type selector (the VM types; containers ship via container-deploy).
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = Style::SP_XS;
            for dt in BUILDABLE {
                if ui
                    .selectable_label(state.images.dtype == dt, dt.label())
                    .clicked()
                {
                    state.images.dtype = dt;
                }
            }
        });
        ui.add_space(Style::SP_XS);

        // Optional name + version overrides.
        let name_hint = format!("{}-golden", state.images.dtype.as_str());
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Name")
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
            ui.add(
                egui::TextEdit::singleline(&mut state.images.name)
                    .hint_text(name_hint)
                    .desired_width(Style::SP_XL * 5.0),
            );
            ui.add_space(Style::SP_S);
            ui.label(
                RichText::new("Version")
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
            ui.add(
                egui::TextEdit::singleline(&mut state.images.version)
                    .hint_text("latest")
                    .desired_width(Style::SP_XL * 3.0),
            );
        });
        ui.add_space(Style::SP_S);

        ui.horizontal(|ui| {
            if ui
                .add(egui::Button::new(
                    RichText::new("Build\u{2026}")
                        .size(Style::SMALL)
                        .color(Style::ACCENT),
                ))
                .clicked()
            {
                action = Some("build");
            }
            if ui
                .add(egui::Button::new(
                    RichText::new("Promote\u{2026}")
                        .size(Style::SMALL)
                        .color(Style::TEXT),
                ))
                .clicked()
            {
                action = Some("promote");
            }
            if ui
                .add(egui::Button::new(
                    RichText::new("Refresh roster")
                        .size(Style::SMALL)
                        .color(Style::TEXT),
                ))
                .clicked()
            {
                action = Some("list");
            }
        });
        mde_egui::muted_note(
            ui,
            "Build and promote are armed-token gated on the placement node — an un-armed \
             request stages / plans honestly and installs nothing.",
        );
    });
    action
}

/// The image-roster card. The `image-build` / `list` reply carries the golden-image
/// rows, but the shell's `CloudReply` mirror does not decode the `ImageRow` list
/// yet, so this reads an honest "pending backend decode" note rather than
/// fabricating rows (§7).
fn image_roster(ui: &mut egui::Ui) {
    mde_egui::card().show(ui, |ui| {
        ui.label(
            RichText::new("Image roster")
                .size(Style::BODY)
                .strong()
                .color(Style::TEXT),
        );
        mde_egui::muted_note(
            ui,
            "Roster pending backend decode \u{2014} the image-build / list reply carries the \
             golden-image rows (name \u{00B7} SHA256 \u{00B7} promoted), but the shell's reply \
             mirror does not decode the ImageRow list yet. Rows appear once that decode lands; \
             nothing is fabricated here. Promoted images wear a success badge when they render.",
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_body_carries_the_action_delivery_type_and_node() {
        let body = build_request_body("build", DeliveryType::AppVm, "  ", "", "eagle");
        assert!(body.contains(r#""action":"build""#), "{body}");
        // DeliveryType serializes as its snake_case token.
        assert!(body.contains(r#""delivery_type":"app_vm""#), "{body}");
        assert!(body.contains(r#""node":"eagle""#), "{body}");
        // A blank name is sent empty; the backend derives `<delivery_type>-golden`.
        assert!(body.contains(r#""name":"""#), "{body}");
    }

    #[test]
    fn the_container_type_is_not_buildable_here() {
        // A ServiceContainer ships via container-deploy, not image-build.
        assert!(!BUILDABLE.contains(&DeliveryType::ServiceContainer));
        assert_eq!(BUILDABLE.len(), 4);
    }
}
