//! U17 — the **Configure** lens: the Ansible convergence leg. The operator picks
//! a playbook + a target inventory group and converges through the two preserved
//! seams — [`WorkloadsState::check_configure`] (a dry `ansible-playbook --check`)
//! and [`WorkloadsState::arm_configure`] (the typed-arm live apply, RUN-006) —
//! alongside the **live resolved mesh inventory** (the `inventory` verb →
//! [`InventoryHost`]) and the provisioned **tofu outputs** (the `output` verb →
//! [`TofuOutput`], masked when sensitive) the run would target.
//!
//! The playbook + group inputs live here (not on [`WorkloadsState`]) so the U17
//! worker owns them; the preserved arming/emit path reads them via
//! `WorkloadsState::configure_body`.
//!
//! The shell's lean mutation mirror ([`super`]'s own `CloudReply`) deliberately
//! keeps only the `ok`/`gated`/`error` tri-state — it drops the rich `inventory`
//! and `outputs` payloads. So the READ verbs run through a small **self-contained
//! resolve lane** here (reusing the preserved `publish`/`persist` Bus seams and
//! the full-payload wire [`WireCloudReply`]): it fetches once on first entry,
//! the operator drives Refresh after, and every state reads honestly — an empty
//! roster is a real "not resolved yet", never fabricated (§7).

use mde_egui::egui::{self, Color32, RichText};
use mde_egui::{carbon_icon, Style};

use mackes_mesh_types::cloud::{
    CloudReply as WireCloudReply, InventoryHost, TofuOutput, VERB_INVENTORY, VERB_OUTPUT,
};
use mde_bus::rpc::reply_topic;

use super::WorkloadsState;

/// The Configure lens's own state — the Ansible entrypoint the check/apply seams
/// converge, plus the self-contained inventory/outputs resolve lane.
#[derive(Debug)]
pub(super) struct State {
    /// The playbook selection (the Ansible entrypoint).
    pub(super) playbook: String,
    /// The target group (the mesh inventory group to converge).
    pub(super) group: String,
    /// The live resolved mesh inventory (the `inventory` verb reply). Empty until
    /// a resolve lands — an honest "not resolved yet", never fabricated (§7).
    inventory: Vec<InventoryHost>,
    /// The live tofu outputs (the `output` verb reply) — the instance roster / IPs
    /// a provisioned workload exposes. Empty until a resolve lands.
    outputs: Vec<TofuOutput>,
    /// The in-flight `inventory` READ, if any (its reply resolves [`Self::inventory`]).
    inventory_req: Option<super::Pending>,
    /// The in-flight `output` READ, if any (its reply resolves [`Self::outputs`]).
    output_req: Option<super::Pending>,
    /// An honest one-line resolve status (resolving / N hosts / gated / failed /
    /// off-mesh) — the READ lane is never a silent op.
    status: Option<String>,
    /// Whether the first-entry auto-resolve has fired (fetch once on entry, then
    /// the operator drives Refresh — a live panel never re-emits every frame).
    requested: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            playbook: "site.yml".to_string(),
            group: "cloud_vm".to_string(),
            inventory: Vec::new(),
            outputs: Vec::new(),
            inventory_req: None,
            output_req: None,
            status: None,
            requested: false,
        }
    }
}

/// Render the Configure lens: the Ansible run form (playbook + group + the
/// check/apply seams), the live resolved inventory, and the provisioned outputs.
pub(super) fn configure_panel(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    // A live panel resolves its data on first entry; the operator drives Refresh
    // after (fetch once, never re-emit every frame).
    if !state.configure.requested {
        state.configure.requested = true;
        resolve(state);
    }
    advance(state);
    if state.configure.inventory_req.is_some() || state.configure.output_req.is_some() {
        ui.ctx().request_repaint_after(super::POLL_REPAINT);
    }

    lens_heading(
        ui,
        "document-edit",
        "Configure \u{2014} Ansible convergence",
    );

    // ── the run form + the preserved converge seams ──
    let mut do_check = false;
    let mut do_apply = false;
    mde_egui::card().show(ui, |ui| {
        section_label(ui, "Ansible run");
        ui.add_space(Style::SP_XS);
        egui::Grid::new("workloads-configure-form")
            .num_columns(2)
            .spacing([Style::SP_M, Style::SP_S])
            .show(ui, |ui| {
                ui.label(
                    RichText::new("Playbook")
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut state.configure.playbook)
                        .hint_text("site.yml")
                        .desired_width(Style::SP_XL * 6.0),
                );
                ui.end_row();
                ui.label(
                    RichText::new("Inventory group")
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut state.configure.group)
                        .hint_text("cloud_vm")
                        .desired_width(Style::SP_XL * 6.0),
                );
                ui.end_row();
            });
        ui.add_space(Style::SP_S);
        ui.horizontal(|ui| {
            if action_button(ui, "Check (dry-run)", Style::ACCENT).clicked() {
                do_check = true;
            }
            ui.add_space(Style::SP_S);
            if action_button(ui, "Apply\u{2026}", Style::SUPPORT_WARNING).clicked() {
                do_apply = true;
            }
        });
        mde_egui::muted_note(
            ui,
            "Check runs the playbook with --check (no changes). Apply converges live behind the \
             typed-arm confirm above; run results (ok / changed / failed) land in the action note \
             and the Status audit trail.",
        );
    });
    // The preserved seams: a direct dry-run check, or the RUN-006 typed-arm apply.
    if do_check {
        state.check_configure();
    }
    if do_apply {
        state.arm_configure();
    }
    ui.add_space(Style::SP_S);

    inventory_section(ui, state);
    outputs_section(ui, state);
}

/// The resolved mesh inventory the configure run would target — the `inventory`
/// verb's live rows, with a Refresh affordance and the honest resolve status.
fn inventory_section(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    let mut do_refresh = false;
    mde_egui::toolbar().show(ui, |ui| {
        ui.horizontal(|ui| {
            section_label(ui, "Resolved mesh inventory");
            ui.add_space(Style::SP_M);
            if action_button(ui, "Refresh", Style::ACCENT).clicked() {
                do_refresh = true;
            }
        });
    });
    if let Some(status) = state.configure.status.clone() {
        mde_egui::muted_note(ui, status);
    }
    if state.configure.inventory.is_empty() {
        if state.configure.inventory_req.is_none() {
            mde_egui::muted_note(
                ui,
                "No inventory resolved yet \u{2014} Refresh to fetch the live mesh Ansible \
                 inventory the configure run would target.",
            );
        }
    } else {
        let group = state.configure.group.trim().to_string();
        for host in &state.configure.inventory {
            inventory_row(ui, host, &group);
        }
    }
    ui.add_space(Style::SP_S);
    if do_refresh {
        resolve(state);
    }
}

/// One resolved inventory host — a reachability dot, its id · address, and the
/// inventory groups it lands in (the selected group tinted the Workloads accent so
/// the operator sees which hosts the run targets).
fn inventory_row(ui: &mut egui::Ui, host: &InventoryHost, selected_group: &str) {
    mde_egui::inset().show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            mde_egui::status_dot(
                ui,
                if host.reachable {
                    Style::SUPPORT_SUCCESS
                } else {
                    Style::SUPPORT_ERROR
                },
            );
            ui.add_space(Style::SP_XS);
            ui.label(
                RichText::new(&host.id)
                    .size(Style::BODY)
                    .strong()
                    .color(Style::TEXT),
            );
            ui.add_space(Style::SP_S);
            ui.label(
                RichText::new(&host.node)
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
            ui.add_space(Style::SP_S);
            let (word, tone) = if host.reachable {
                ("reachable", Style::SUPPORT_SUCCESS)
            } else {
                ("unreachable", Style::SUPPORT_ERROR)
            };
            ui.label(RichText::new(word).size(Style::SMALL).color(tone));
        });
        if !host.groups.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new("groups")
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
                ui.add_space(Style::SP_XS);
                for group in &host.groups {
                    let tone = if group == selected_group {
                        Style::ACCENT_WORKLOADS
                    } else {
                        Style::TEXT_DIM
                    };
                    ui.label(RichText::new(group).size(Style::SMALL).color(tone).strong());
                    ui.add_space(Style::SP_XS);
                }
            });
        }
    });
    ui.add_space(Style::SP_XS);
}

/// The provisioned tofu outputs (the `output` verb reply) — the instance roster /
/// IPs a workload exposes. Sensitive outputs render masked, never in the clear (§7).
fn outputs_section(ui: &mut egui::Ui, state: &WorkloadsState) {
    section_label(ui, "Provisioned outputs");
    if state.configure.outputs.is_empty() {
        mde_egui::muted_note(
            ui,
            "No tofu outputs resolved \u{2014} a provisioned workload's instance roster / IP \
             outputs appear here once the output verb replies.",
        );
    } else {
        mde_egui::card().show(ui, |ui| {
            for output in &state.configure.outputs {
                let (value, tone) = if output.sensitive {
                    (
                        "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022} (sensitive)".to_string(),
                        Style::TEXT_DIM,
                    )
                } else {
                    (output.value.clone(), Style::TEXT)
                };
                mde_egui::field(ui, &output.name, &value, tone);
            }
        });
    }
    ui.add_space(Style::SP_S);
}

/// Issue the `inventory` + `output` READ verbs, tracking each reply — an honest
/// resolve (never fabricated rows). A missing Bus degrades to an honest status,
/// never a panic (§7).
fn resolve(state: &mut WorkloadsState) {
    match state.publish(VERB_INVENTORY, None) {
        Ok(pending) => {
            state.configure.inventory_req = Some(pending);
            state.configure.status = Some("Resolving the live mesh inventory\u{2026}".to_string());
        }
        Err(e) => {
            state.configure.status = Some(format!("Could not request the inventory: {e}"));
        }
    }
    state.configure.output_req = state.publish(VERB_OUTPUT, None).ok();
}

/// Advance any in-flight inventory / output READ into its resolved rows + honest
/// status (or an honest timeout). Called each frame the lens is shown.
fn advance(state: &mut WorkloadsState) {
    if let Some((ulid, sent)) = state
        .configure
        .inventory_req
        .as_ref()
        .map(|p| (p.ulid.clone(), p.sent))
    {
        if let Some(reply) = read_reply(state, &ulid) {
            state.configure.inventory_req = None;
            if let Some(hosts) = reply.inventory {
                state.configure.status =
                    Some(format!("Resolved {} inventory host(s).", hosts.len()));
                state.configure.inventory = hosts;
            } else if let Some(gated) = reply.gated {
                state.configure.status = Some(format!("Inventory staged/gated: {gated}"));
            } else if let Some(error) = reply.error {
                state.configure.status = Some(format!("Inventory resolve failed: {error}"));
            } else {
                state.configure.status = Some("The inventory verb returned no hosts.".to_string());
            }
        } else if sent.elapsed() >= super::REQUEST_TIMEOUT {
            state.configure.inventory_req = None;
            state.configure.status = Some(
                "The cloud backend did not answer the inventory request \u{2014} it may not be \
                 running on any reachable node."
                    .to_string(),
            );
        }
    }

    if let Some((ulid, sent)) = state
        .configure
        .output_req
        .as_ref()
        .map(|p| (p.ulid.clone(), p.sent))
    {
        if let Some(reply) = read_reply(state, &ulid) {
            state.configure.output_req = None;
            state.configure.outputs = reply.outputs.unwrap_or_default();
        } else if sent.elapsed() >= super::REQUEST_TIMEOUT {
            state.configure.output_req = None;
        }
    }
}

/// Read the wire cloud reply on `reply/<ulid>` off the Bus, if one has landed —
/// the full-payload [`WireCloudReply`] (carrying `inventory` / `outputs`), which
/// the shell's own lean mutation mirror deliberately drops. Returns owned data so
/// the immutable Bus borrow ends before the caller writes the result back.
fn read_reply(state: &WorkloadsState, ulid: &str) -> Option<WireCloudReply> {
    let persist = state.persist()?;
    let msgs = persist.list_since(&reply_topic(ulid), None).ok()?;
    let body = msgs.first()?.body.as_deref()?;
    serde_json::from_str(body).ok()
}

/// A lens section heading — a Workloads-accent Carbon glyph + a strong label.
fn lens_heading(ui: &mut egui::Ui, icon: &str, label: &str) {
    ui.horizontal(|ui| {
        ui.scope(|ui| {
            ui.visuals_mut().override_text_color = Some(Style::ACCENT_WORKLOADS);
            carbon_icon(ui, icon, Style::ICON_M);
        });
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new(label)
                .size(Style::BODY)
                .strong()
                .color(Style::TEXT),
        );
    });
    ui.add_space(Style::SP_XS);
}

/// A strong body-size subsection label.
fn section_label(ui: &mut egui::Ui, label: &str) {
    ui.label(
        RichText::new(label)
            .size(Style::BODY)
            .strong()
            .color(Style::TEXT),
    );
}

/// A small text button tinted `tone` — the shared shape the run + refresh actions
/// wear.
fn action_button(ui: &mut egui::Ui, label: &str, tone: Color32) -> egui::Response {
    ui.add(egui::Button::new(
        RichText::new(label).size(Style::SMALL).color(tone),
    ))
}
