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
//! emit path (the same `issue` seam the provision lens uses) for `build` /
//! `promote`; a live build/promote requires an explicit Yes/No review and a
//! target/body-bound token minted by the root DRM shell.
//!
//! The roster itself is sourced from the `image-build` `list` reply. Because the
//! shell's lean mutation mirror (this crate's own `CloudReply`) deliberately
//! drops the rich `images` payload, `list` runs through a small self-contained
//! resolve lane here (mirroring `configure.rs`'s inventory/outputs lane): it
//! fetches the full-payload wire [`WireCloudReply`] once on first entry, the
//! operator drives Refresh after, and the roster reads honestly — an empty list
//! is a real "not resolved yet", never fabricated (§7).

use mde_egui::egui::{self, RichText};
use mde_egui::{carbon_icon, Style};

use mackes_mesh_types::cloud::{
    CloudReply as WireCloudReply, DeliveryType, ImageRow, VERB_IMAGE_BUILD,
};

use super::{row_button, WorkloadsState};

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
/// controls target plus the optional name / version overrides, and the
/// self-contained roster-resolve lane.
#[derive(Debug)]
pub(super) struct State {
    /// The delivery type whose golden image the build / promote controls act on.
    dtype: DeliveryType,
    /// An optional image-name override; blank ⇒ the `<delivery_type>-golden`
    /// default the backend derives.
    name: String,
    /// An optional version; blank ⇒ `latest` (build) / the resolved default.
    version: String,
    /// The resolved golden-image roster (the `image-build` `list` reply's
    /// `images` payload) for the selected delivery type. Empty until a resolve
    /// lands — an honest "not resolved yet", never fabricated (§7).
    roster: Vec<ImageRow>,
    /// The expanded roster row, keyed by image name + SHA. Images are not live
    /// workloads, but WL-UX-008 still expects table-row details before mutation
    /// controls.
    expanded_image: Option<String>,
    /// The in-flight roster `list` READ, if any (its reply resolves
    /// [`Self::roster`]).
    roster_req: Option<super::Pending>,
    /// An honest one-line resolve status (resolving / N image(s) / gated /
    /// failed) — the READ lane is never a silent op.
    status: Option<String>,
    /// Whether the first-entry auto-resolve has fired (fetch once on entry, then
    /// the operator drives Refresh — a live panel never re-emits every frame).
    requested: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            dtype: DeliveryType::DesktopVm,
            name: String::new(),
            version: String::new(),
            roster: Vec::new(),
            expanded_image: None,
            roster_req: None,
            status: None,
            requested: false,
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
        "schema_version": mackes_mesh_types::cloud::CLOUD_ACTION_SCHEMA_VERSION,
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
    // A live panel resolves its roster on first entry; the operator drives
    // Refresh after (fetch once, never re-emit every frame).
    if !state.images.requested {
        state.images.requested = true;
        resolve(state);
    }
    advance(state);
    if state.images.roster_req.is_some() {
        ui.ctx().request_repaint_after(super::POLL_REPAINT);
    }

    header(ui);
    ui.add_space(Style::SP_S);

    if let Some(action) = image_lifecycle_table(ui, state) {
        handle_image_action(state, action);
    }

    ui.add_space(Style::SP_S);
    if let Some(action) = build_controls(ui, state) {
        handle_image_action(state, action);
    }
}

enum ImageAction {
    Build,
    PromoteDraft,
    PromoteRow { name: String },
    List,
}

fn handle_image_action(state: &mut WorkloadsState, action: ImageAction) {
    match action {
        ImageAction::List => {
            // The roster read runs through its own self-contained resolve lane
            // (never the shared mutation seam) so it can decode the rich
            // full-payload wire reply.
            resolve(state);
        }
        ImageAction::Build => arm_image_mutation(state, "build", None),
        ImageAction::PromoteDraft => arm_image_mutation(state, "promote", None),
        ImageAction::PromoteRow { name } => arm_image_mutation(state, "promote", Some(name)),
    }
}

fn arm_image_mutation(state: &mut WorkloadsState, action: &'static str, row_name: Option<String>) {
    if let Some(name) = &row_name {
        state.images.name = name.clone();
    }
    if action == "promote" && state.images.version.trim().is_empty() {
        let name = row_name
            .as_deref()
            .unwrap_or_else(|| state.images.name.trim());
        let name = if name.is_empty() {
            format!("{}-golden", state.images.dtype.as_str())
        } else {
            name.to_string()
        };
        state.images.status = Some(format!(
            "Enter the image version before promoting {name}; the roster carries name/SHA/promoted \
             but not a hidden version."
        ));
        return;
    }

    // Snapshot the request inputs (owned) so the immutable field borrows end
    // before the mutable emit seam runs.
    let Some(node) = state
        .selected_node()
        .map(str::trim)
        .filter(|node| !node.is_empty())
        .map(str::to_string)
    else {
        state.images.status = Some("Select a placement node before changing an image.".to_string());
        return;
    };
    let dtype = state.images.dtype;
    let body_name = row_name.as_deref().unwrap_or(&state.images.name);
    let body = build_request_body(action, dtype, body_name, &state.images.version, &node);
    let label = if action == "promote" {
        format!("promote golden image for {}", dtype.label())
    } else {
        format!("golden image build for {}", dtype.label())
    };
    let name = body_name
        .trim()
        .is_empty()
        .then(|| format!("{}-golden", dtype.as_str()))
        .unwrap_or_else(|| body_name.trim().to_string());
    let version = if state.images.version.trim().is_empty() {
        "latest".to_string()
    } else {
        state.images.version.trim().to_string()
    };
    let target = format!("{action}:{name}@{version}");
    let word = if action == "promote" {
        "Promote"
    } else {
        "Build"
    };
    state.arm_prepared(
        VERB_IMAGE_BUILD,
        node,
        target.clone(),
        body,
        label,
        target.clone(),
        word,
        format!("golden image {target}"),
    );
}

/// Issue the `image-build` `list` READ for the selected delivery type, tracking
/// its reply — an honest resolve (never fabricated rows). A missing Bus degrades
/// to an honest status, never a panic (§7).
fn resolve(state: &mut WorkloadsState) {
    let Some(node) = state
        .selected_node()
        .map(str::trim)
        .filter(|node| !node.is_empty())
        .map(str::to_string)
    else {
        state.images.status = Some("Select a placement node before resolving images.".to_string());
        return;
    };
    let body = build_request_body("list", state.images.dtype, "", "", &node);
    match state.publish(VERB_IMAGE_BUILD, Some(&body)) {
        Ok(pending) => {
            state.images.roster_req = Some(pending);
            state.images.status = Some("Resolving the golden-image roster\u{2026}".to_string());
        }
        Err(e) => {
            state.images.status = Some(format!("Could not request the image roster: {e}"));
        }
    }
}

/// Advance the in-flight roster READ into its resolved rows + honest status (or
/// an honest timeout). Called each frame the lens is shown.
fn advance(state: &mut WorkloadsState) {
    let Some((ulid, sent)) = state
        .images
        .roster_req
        .as_ref()
        .map(|p| (p.ulid.clone(), p.sent))
    else {
        return;
    };
    if let Some(reply) = state.read_wire_reply(&ulid) {
        state.images.roster_req = None;
        if let Some(rows) = reply.images {
            state.images.status = Some(format!("Resolved {} golden image(s).", rows.len()));
            state.images.roster = rows;
        } else if let Some(gated) = reply.gated {
            state.images.status = Some(format!("Image roster staged/gated: {gated}"));
        } else if let Some(error) = reply.error {
            state.images.status = Some(format!("Image roster resolve failed: {error}"));
        } else {
            state.images.status = Some("The image-build verb returned no images.".to_string());
        }
    } else if sent.elapsed() >= super::REQUEST_TIMEOUT {
        state.images.roster_req = None;
        state.images.status = Some(
            "The cloud backend did not answer the image roster request \u{2014} it may not be \
             running on any reachable node."
                .to_string(),
        );
    }
}

/// The lens header card — the Workloads-accent glyph, the title, and the honest
/// provenance / airgap-lane blurb.
fn header(ui: &mut egui::Ui) {
    mde_egui::card().show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.scope(|ui| {
                ui.visuals_mut().override_text_color = Some(Style::ACCENT_WORKLOADS);
                carbon_icon(ui, "camera-photo", Style::ICON_S);
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

/// The build / promote / list controls. Returns the chosen `image-build` action
/// (if a button was clicked this frame) so the caller can emit it past the lens's
/// own state borrow.
fn build_controls(ui: &mut egui::Ui, state: &mut WorkloadsState) -> Option<ImageAction> {
    let mut action: Option<ImageAction> = None;
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
                action = Some(ImageAction::Build);
            }
            if ui
                .add(egui::Button::new(
                    RichText::new("Promote\u{2026}")
                        .size(Style::SMALL)
                        .color(Style::TEXT),
                ))
                .clicked()
            {
                action = Some(ImageAction::PromoteDraft);
            }
            if ui
                .add(egui::Button::new(
                    RichText::new("Refresh roster")
                        .size(Style::SMALL)
                        .color(Style::TEXT),
                ))
                .clicked()
            {
                action = Some(ImageAction::List);
            }
        });
        mde_egui::muted_note(
            ui,
            "Build and promote open a Yes/No review. The root DRM shell then mints \
             a single-use capability bound to the selected node and frozen request.",
        );
    });
    action
}

/// The image-roster table — the resolved `image-build` `list` reply's rows (name
/// · SHA256 · promoted), plus expandable row details and row-local promote
/// affordances before the build/promote form. An empty roster after a real resolve
/// reads honestly; nothing here is fabricated (§7).
fn image_lifecycle_table(ui: &mut egui::Ui, state: &mut WorkloadsState) -> Option<ImageAction> {
    let mut action = None;
    mde_egui::card().show(ui, |ui| {
        ui.label(
            RichText::new("Image lifecycle table")
                .size(Style::BODY)
                .strong()
                .color(Style::TEXT),
        );
        mde_egui::muted_note(
            ui,
            format!(
                "Resolved golden-image rows for {}. Dense row height: {:.0}px; details expose \
                 placement, content hash, promotion state, and the frozen command preview before \
                 the image-build flow.",
                state.images.dtype.label(),
                state.density.row_height()
            ),
        );
        if let Some(status) = &state.images.status {
            mde_egui::muted_note(ui, status.clone());
        }
        if state.images.roster.is_empty() {
            if state.images.roster_req.is_none() {
                mde_egui::muted_note(
                    ui,
                    "No golden images resolved yet \u{2014} Refresh roster to fetch the \
                     airgap-lane image list for this delivery type.",
                );
            }
        } else {
            ui.add_space(Style::SP_XS);
            image_table_header(ui);
            ui.separator();
            let rows = state.images.roster.clone();
            for row in rows {
                if let Some(row_action) = image_table_row(ui, state, &row) {
                    action = Some(row_action);
                }
            }
        }
    });
    action
}

fn image_table_header(ui: &mut egui::Ui) {
    let h = 30.0;
    ui.horizontal(|ui| {
        image_cell(ui, "Details", 78.0, h, Style::TEXT_DIM);
        image_cell(ui, "Image", 190.0, h, Style::TEXT_DIM);
        image_cell(ui, "SHA256", 132.0, h, Style::TEXT_DIM);
        image_cell(ui, "Active base", 96.0, h, Style::TEXT_DIM);
        image_cell(ui, "Image Actions", 190.0, h, Style::TEXT_DIM);
    });
}

/// One resolved golden-image row — the name, a shortened SHA256, promotion state,
/// row-local action, and optional row details.
fn image_table_row(
    ui: &mut egui::Ui,
    state: &mut WorkloadsState,
    row: &ImageRow,
) -> Option<ImageAction> {
    let key = image_row_key(row);
    let expanded = state.images.expanded_image.as_deref() == Some(key.as_str());
    let row_height = state.density.row_height();
    let mut action = None;
    ui.horizontal(|ui| {
        if ui
            .add_sized(
                [78.0, row_height],
                egui::Button::new(
                    RichText::new(if expanded { "Collapse" } else { "Details" }).size(Style::SMALL),
                ),
            )
            .clicked()
        {
            if expanded {
                state.images.expanded_image = None;
            } else {
                state.images.expanded_image = Some(key.clone());
            }
        }
        image_cell(ui, &row.name, 190.0, row_height, Style::TEXT);
        image_cell(
            ui,
            &sha_short(&row.sha256),
            132.0,
            row_height,
            Style::TEXT_DIM,
        );
        image_cell(
            ui,
            if row.promoted {
                "promoted"
            } else {
                "candidate"
            },
            96.0,
            row_height,
            if row.promoted {
                Style::SUPPORT_SUCCESS
            } else {
                Style::TEXT_DIM
            },
        );
        ui.horizontal(|ui| {
            if row.promoted {
                ui.colored_label(
                    Style::SUPPORT_SUCCESS,
                    RichText::new("Active base").size(Style::SMALL).strong(),
                );
            } else if row_button(ui, "Promote\u{2026}", true).clicked() {
                action = Some(ImageAction::PromoteRow {
                    name: row.name.clone(),
                });
            }
        });
    });
    if expanded {
        image_expanded_row(ui, state, row);
    }
    ui.add_space(Style::SP_XS);
    action
}

fn image_cell(ui: &mut egui::Ui, text: &str, width: f32, height: f32, color: egui::Color32) {
    ui.add_sized(
        [width, height],
        egui::Label::new(RichText::new(text).size(Style::SMALL).color(color)),
    );
}

fn image_expanded_row(ui: &mut egui::Ui, state: &WorkloadsState, row: &ImageRow) {
    let node = state
        .selected_node()
        .map(str::trim)
        .filter(|node| !node.is_empty())
        .unwrap_or("no placement selected");
    let version = state
        .images
        .version
        .trim()
        .is_empty()
        .then_some("enter version")
        .unwrap_or_else(|| state.images.version.trim());
    egui::Frame::group(ui.style())
        .fill(Style::SURFACE_HI)
        .stroke(egui::Stroke::new(Style::STROKE_HAIRLINE, Style::BORDER))
        .corner_radius(Style::RADIUS_S)
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                mde_egui::field(ui, "image", &row.name, Style::ACCENT_WORKLOADS);
                ui.add_space(Style::SP_M);
                mde_egui::field(
                    ui,
                    "placement",
                    node,
                    if node == "no placement selected" {
                        Style::WARN
                    } else {
                        Style::TEXT
                    },
                );
                ui.add_space(Style::SP_M);
                mde_egui::field(
                    ui,
                    "promotion",
                    if row.promoted {
                        "active base"
                    } else {
                        "candidate"
                    },
                    if row.promoted {
                        Style::SUPPORT_SUCCESS
                    } else {
                        Style::TEXT_DIM
                    },
                );
                ui.add_space(Style::SP_M);
                mde_egui::field(ui, "version", version, Style::TEXT_DIM);
            });
            ui.add_space(Style::SP_XS);
            mde_egui::muted_note(
                ui,
                format!(
                    "Content hash \u{2014} sha256:{}",
                    if row.sha256.is_empty() {
                        "(missing sidecar)"
                    } else {
                        &row.sha256
                    }
                ),
            );
            mde_egui::muted_note(
                ui,
                format!(
                    "Command preview \u{2014} action/cloud/{VERB_IMAGE_BUILD}: list reads the \
                     airgap roster; Promote publishes action=`promote`, node=`{node}`, \
                     name=`{}`, version=`{}` only after exact typed echo.",
                    row.name, version
                ),
            );
        });
}

pub(super) fn image_row_key(row: &ImageRow) -> String {
    format!("image:{}:{}", row.name, row.sha256)
}

/// A SHA256 shown short (its first 12 hex chars + an ellipsis) — the full-length
/// hex wraps ugly in a compact row; the full value still lives in [`ImageRow`].
fn sha_short(sha: &str) -> String {
    match sha.get(..12) {
        Some(head) => format!("{head}\u{2026}"),
        None => sha.to_string(),
    }
}

#[cfg(test)]
impl State {
    pub(super) fn set_test_roster(&mut self, rows: Vec<ImageRow>) {
        self.status = Some(format!("Resolved {} golden image(s).", rows.len()));
        self.roster = rows;
        self.roster_req = None;
        self.requested = true;
    }

    pub(super) fn expand_test_row(&mut self, key: String) {
        self.expanded_image = Some(key);
    }

    pub(super) fn set_test_version(&mut self, version: &str) {
        self.version = version.to_string();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_body_carries_the_action_delivery_type_and_node() {
        let body = build_request_body("build", DeliveryType::AppVm, "  ", "", "eagle");
        assert!(body.contains(r#""action":"build""#), "{body}");
        assert!(body.contains(r#""schema_version":1"#), "{body}");
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

    #[test]
    fn a_wire_reply_with_images_decodes_into_rows() {
        let body = serde_json::json!({
            "ok": true,
            "verb": "image-build",
            "images": [
                {
                    "name": "desktop_vm-golden",
                    "sha256": "abc123def456789000000000000000000000000000000000000000000000",
                    "promoted": true
                },
                {
                    "name": "desktop_vm-golden",
                    "sha256": "999888777666000000000000000000000000000000000000000000000000",
                    "promoted": false
                }
            ]
        })
        .to_string();
        let reply: WireCloudReply = serde_json::from_str(&body).expect("the wire reply parses");
        let rows = reply.images.expect("the images payload decodes");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "desktop_vm-golden");
        assert!(rows[0].promoted);
        assert!(!rows[1].promoted);
    }

    #[test]
    fn the_roster_renders_the_decoded_rows_never_the_pending_decode_note() {
        let mut state = WorkloadsState::default();
        state.images.set_test_roster(vec![
            ImageRow {
                name: "desktop_vm-golden".to_string(),
                sha256: "abc123def456789000000000000000000000000000000000000000000000".to_string(),
                promoted: true,
            },
            ImageRow {
                name: "app_vm-golden".to_string(),
                sha256: "0011223344".to_string(),
                promoted: false,
            },
        ]);
        let text = rendered_text(|ui| {
            let _ = image_lifecycle_table(ui, &mut state);
        });
        assert!(text.contains("desktop_vm-golden"), "{text}");
        assert!(text.contains("app_vm-golden"), "{text}");
        assert!(text.contains("promoted"), "{text}");
        assert!(
            !text.contains("pending backend decode"),
            "the decoded roster must replace the honest-pending note: {text}"
        );
    }

    #[test]
    fn an_empty_roster_still_reads_honestly() {
        let mut state = WorkloadsState::default();
        let text = rendered_text(|ui| {
            let _ = image_lifecycle_table(ui, &mut state);
        });
        assert!(
            text.contains("No golden images resolved"),
            "an empty roster must read honestly, never fabricated: {text}"
        );
    }

    #[test]
    fn promote_requires_explicit_version_before_the_review_sheet_opens() {
        let mut state = WorkloadsState::default();
        state.selected_node = Some("eagle".to_string());

        handle_image_action(
            &mut state,
            ImageAction::PromoteRow {
                name: "desktop_vm-golden".to_string(),
            },
        );

        assert!(
            !state.has_arming(),
            "a promote without a version must not open review"
        );
        assert!(
            state
                .images
                .status
                .as_deref()
                .is_some_and(|status| status.contains("Enter the image version")),
            "blank-version promote should explain the blocked seam: {:?}",
            state.images.status
        );

        state.images.version = "1.0".to_string();
        handle_image_action(
            &mut state,
            ImageAction::PromoteRow {
                name: "desktop_vm-golden".to_string(),
            },
        );

        let review = state
            .arming
            .as_ref()
            .expect("versioned promote opens review");
        assert_eq!(review.action.echo(), "promote:desktop_vm-golden@1.0");
        match &review.action {
            super::super::ArmAction::Prepared {
                verb, body, node, ..
            } => {
                assert_eq!(*verb, VERB_IMAGE_BUILD);
                assert_eq!(node, "eagle");
                assert!(body.contains(r#""action":"promote""#), "{body}");
                assert!(body.contains(r#""name":"desktop_vm-golden""#), "{body}");
                assert!(body.contains(r#""version":"1.0""#), "{body}");
            }
            other => panic!("expected prepared image-build review, got {other:?}"),
        }
    }

    /// Drive `run` in a headless frame and collect every text run painted — the
    /// pixel-feed proof a fixture decode actually renders (the same
    /// `Context::run` path the DRM runner drives, minus the GPU).
    fn rendered_text(mut run: impl FnMut(&mut egui::Ui)) -> String {
        fn collect(shape: &egui::epaint::Shape, out: &mut String) {
            match shape {
                egui::epaint::Shape::Text(t) => {
                    out.push_str(t.galley.text());
                    out.push('\n');
                }
                egui::epaint::Shape::Vec(shapes) => {
                    for s in shapes {
                        collect(s, out);
                    }
                }
                _ => {}
            }
        }
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1100.0, 720.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| run(ui));
        });
        let mut text = String::new();
        for clipped in &out.shapes {
            collect(&clipped.shape, &mut text);
        }
        text
    }
}
