//! `Surface::System` settings — the **Mesh & System settings-section render
//! group** (SETTINGS-4), split out of the System god-module as a pure leaf
//! relocation (no behaviour change). The `identity` / `role` / `pairing` /
//! `network` / `remote proofing` section bodies the master-detail rail dispatches to, plus their
//! private `mesh_field` / `mesh_reading` / `role_description` render helpers.
//!
//! The `MeshFacts` data model + its snapshot folding stay in the parent (next to
//! the `SystemState` field they feed); as a child module `use super::*` pulls in
//! that `MeshFacts`, the shared frame helpers (`column_card` / `across_grid`), the
//! `field` / `muted_note` primitives + the egui/Style/seat re-exports, and the
//! parent reads these section bodies back only through the four `pub(super)` fns
//! its `settings_detail` dispatch calls.

use super::*;

/// One mesh fact as a [`field`] row — the toned value when the snapshot carried it,
/// a dim honest "unknown" when it didn't (§7 — never a fabricated value).
fn mesh_field(ui: &mut egui::Ui, label: &str, value: Option<&str>) {
    match value {
        Some(v) => field(ui, label, v, Style::TEXT),
        None => field(ui, label, "unknown", Style::TEXT_DIM),
    }
}

/// The shared "reading the snapshot" note a Mesh & System section shows before the
/// first mesh-status poll lands.
fn mesh_reading(ui: &mut egui::Ui) {
    muted_note(ui, SYSTEM_MESH_READING_COPY);
}

/// The Identity section (SETTINGS-4) — this node's mesh identity name + overlay
/// address + tunnel cipher, folded from the world-readable snapshot. The Nebula
/// certificate fingerprint is honestly `unknown`: the shell reads the world-readable
/// mesh-status surface, not the root-only cert (§6/§7 — the same honest boundary the
/// This Node plane draws for node-local telemetry).
pub(super) fn identity_section(ui: &mut egui::Ui, mesh: &MeshFacts) {
    if !mesh.seen {
        mesh_reading(ui);
        return;
    }
    tile(ui, |ui| {
        mesh_field(ui, "Mesh name", mesh.identity.as_deref());
        // Not on the world-readable surface — honest-unknown, never a fake digest.
        field(ui, "Certificate fingerprint", "unknown", Style::TEXT_DIM);
        mesh_field(ui, "Overlay address", mesh.overlay_ip.as_deref());
        mesh_field(ui, "Tunnel cipher", mesh.cipher.as_deref());
    });
    ui.add_space(Style::SP_S);
    muted_note(
        ui,
        "Identity folds from the world-readable mesh-status snapshot; the Nebula \
         certificate fingerprint isn't published to this surface (the shell reads no \
         root-only cert).",
    );
}

/// The Role section (SETTINGS-4) — this node's pinned deployment role, a one-line
/// description of what the tier means, and a leader-lease marker. Honest-`unknown`
/// when the node hasn't published a directory row yet (§7).
pub(super) fn role_section(ui: &mut egui::Ui, mesh: &MeshFacts) {
    if !mesh.seen {
        mesh_reading(ui);
        return;
    }
    let accent = SettingsGroup::MeshSystem.accent();
    tile(ui, |ui| {
        match mesh.role.as_deref() {
            Some(role) => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(DOT).color(accent).size(Style::SMALL));
                    ui.add_space(Style::SP_XS);
                    ui.label(RichText::new(role).color(accent).size(Style::BODY).strong());
                });
                ui.add_space(Style::SP_XS);
                muted_note(ui, role_description(role));
            }
            None => field(
                ui,
                "Role",
                "unknown — not yet pinned in the peer directory",
                Style::TEXT_DIM,
            ),
        }
        if mesh.is_leader() {
            ui.add_space(Style::SP_XS);
            ui.horizontal(|ui| {
                ui.label(RichText::new(DOT).color(Style::OK).size(Style::SMALL));
                ui.add_space(Style::SP_XS);
                ui.colored_label(
                    Style::OK,
                    RichText::new("holds the mesh leader lease").size(Style::SMALL),
                );
            });
        }
    });
}

/// A one-line description of a pinned role for the Role section — honest for the
/// three deployment tiers the fleet pins, a neutral line for any other value.
fn role_description(role: &str) -> &'static str {
    match role {
        "lighthouse" => {
            "Anchors the overlay — a stable public endpoint peers discover the mesh through."
        }
        "server" => "A headless mesh member running shared workloads and services.",
        "workstation" => "An interactive seat — this desktop rides the mesh as a workstation.",
        _ => "A pinned mesh member.",
    }
}

/// The Pairing section (SETTINGS-4) — folds in the pairing responder the surface
/// already drives while Settings is open ([`SystemState::sync_pairing_agent`], §6).
/// It surfaces the responder's honest live state — whether an adapter is present for
/// it to bind, whether it's registered, and whether a pairing prompt is in flight
/// (answered in the shared modal) — and offers a Retry that re-arms the SAME seam
/// after a transient failure (never a second agent, §6 one-owner).
pub(super) fn pairing_section(
    ui: &mut egui::Ui,
    snap: Option<&SeatSnapshot>,
    agent_active: bool,
    prompt_in_flight: bool,
    actions: &mut Vec<SysAction>,
) {
    // The responder binds the host Bluetooth adapter — no adapter, nothing to pair.
    let adapter_present = matches!(
        snap.map(|s| &s.bluetooth),
        Some(Probe::Present(bt)) if !bt.adapters.is_empty()
    );
    tile(ui, |ui| {
        let (dot, word, tone) = if !adapter_present {
            (
                Style::TEXT_DIM,
                "no adapter — nothing to pair",
                Style::TEXT_DIM,
            )
        } else if agent_active {
            (Style::OK, "registered", Style::OK)
        } else {
            (
                Style::WARN,
                "adapter present — not yet registered",
                Style::WARN,
            )
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(DOT).color(dot).size(Style::SMALL));
            ui.add_space(Style::SP_XS);
            ui.label(
                RichText::new("Pairing responder")
                    .color(Style::TEXT)
                    .size(Style::SMALL)
                    .strong(),
            );
            ui.add_space(Style::SP_S);
            ui.colored_label(tone, RichText::new(word).size(Style::SMALL));
        });
        // A prompt in flight — the operator answers it in the shared modal.
        if prompt_in_flight {
            ui.add_space(Style::SP_XS);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.add_space(Style::SP_XS);
                ui.colored_label(
                    Style::ACCENT,
                    RichText::new("A pairing prompt is waiting — respond in the dialog.")
                        .size(Style::SMALL),
                );
            });
        }
        // Retry re-arms the responder main.rs drives on visibility — disabled
        // honestly when there is no adapter to bind.
        ui.add_space(Style::SP_XS);
        if ui
            .add_enabled(
                adapter_present,
                egui::Button::new(RichText::new("Retry pairing").size(Style::SMALL)),
            )
            .clicked()
        {
            actions.push(SysAction::PairingRetry);
        }
    });
    ui.add_space(Style::SP_S);
    muted_note(
        ui,
        "The pairing responder answers incoming device PIN / passkey prompts while \
         Settings is open; it binds the host Bluetooth adapter (§6 — one responder, \
         driven by this surface's visibility).",
    );
}

/// The Network section (SETTINGS-4) — the overlay (Nebula) facts and the mesh links /
/// underlay reachability, laid side by side across the wide pane (SETTINGS-3). Every
/// field is the node's real snapshot reality, honest-`unknown` where absent (§7).
/// Live per-link throughput / handshake state isn't on the world-readable surface
/// (§6) — the same honest boundary the Network plane draws.
pub(super) fn network_section(ui: &mut egui::Ui, mesh: &MeshFacts) {
    // The middle-dot joiner the device-meta / Network rows use for a list value.
    const SEP: &str = "  \u{00B7}  ";
    if !mesh.seen {
        mesh_reading(ui);
        return;
    }
    let overlay = |ui: &mut egui::Ui| {
        mesh_field(ui, "Overlay IP", mesh.overlay_ip.as_deref());
        mesh_field(ui, "Interface", mesh.overlay_if.as_deref());
        mesh_field(ui, "Subnet", mesh.overlay_cidr.as_deref());
        mesh_field(ui, "Cipher", mesh.cipher.as_deref());
    };
    let links = |ui: &mut egui::Ui| {
        // Live peer count — green when all live, warn when some are down.
        let tone = if mesh.peers_total == 0 {
            Style::TEXT_DIM
        } else if mesh.peers_online == mesh.peers_total {
            Style::OK
        } else {
            Style::WARN
        };
        field(
            ui,
            "Peers",
            &format!("{}/{} live", mesh.peers_online, mesh.peers_total),
            tone,
        );
        // The elected leader (with a this-node marker when we hold the lease).
        match mesh.leader.as_deref() {
            Some(leader) if mesh.is_leader() => {
                field(ui, "Leader", &format!("{leader} (this node)"), Style::OK);
            }
            Some(leader) => field(ui, "Leader", leader, Style::TEXT),
            None => field(ui, "Leader", "no leader elected", Style::TEXT_DIM),
        }
        // Lighthouses anchoring the overlay.
        if mesh.lighthouses.is_empty() {
            field(ui, "Lighthouses", "unknown", Style::TEXT_DIM);
        } else {
            field(ui, "Lighthouses", &mesh.lighthouses.join(SEP), Style::TEXT);
        }
        // Underlay reachability: the public endpoints + the default gateway (both
        // honestly omitted / dim when the snapshot doesn't carry them).
        if !mesh.gateways.is_empty() {
            field(
                ui,
                "Public endpoints",
                &mesh.gateways.join(SEP),
                Style::TEXT,
            );
        }
        mesh_field(ui, "Default gateway", mesh.default_gw.as_deref());
    };
    if fit_columns(ui.available_width(), 2) == 2 {
        ui.columns(2, |columns| {
            column_card(&mut columns[0], "Overlay", |ui| overlay(ui));
            column_card(&mut columns[1], "Mesh links", |ui| links(ui));
        });
    } else {
        column_card(ui, "Overlay", |ui| overlay(ui));
        ui.add_space(Style::SP_S);
        column_card(ui, "Mesh links", |ui| links(ui));
    }
}

/// A compact selectable settings tile for Remote Proofing enum choices.
fn proofing_choice_tile(ui: &mut egui::Ui, selected: bool, label: &str, description: &str) -> bool {
    let mut clicked = false;
    tile(ui, |ui| {
        if ui
            .add_sized(
                [ui.available_width(), Style::SP_L],
                egui::SelectableLabel::new(selected, RichText::new(label).size(Style::BODY)),
            )
            .clicked()
            && !selected
        {
            clicked = true;
        }
        ui.label(
            RichText::new(description)
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
    });
    clicked
}

/// Mesh & System → Remote Proofing — the single Settings workspace for
/// Sunshine/Moonlight console shadowing and VNC fallback. It intentionally keeps the
/// whole operator policy together: service enablement, exposure, capture, encoder,
/// pairing approval, shadowing indicator, remote input, frame target, and fallback
/// admin channel.
pub(super) fn remote_proofing_section(
    ui: &mut egui::Ui,
    config: &mut RemoteProofingConfig,
    mesh: &MeshFacts,
) {
    tile(ui, |ui| {
        let mut enabled = config.enabled;
        if ui
            .checkbox(
                &mut enabled,
                RichText::new("Enable Sunshine/Moonlight proofing").size(Style::BODY),
            )
            .changed()
        {
            config.enabled = enabled;
        }

        ui.add_space(Style::SP_XS);
        field(
            ui,
            "Primary surface",
            "Sunshine/Moonlight",
            if config.enabled {
                Style::OK
            } else {
                Style::TEXT_DIM
            },
        );
        field(
            ui,
            "Fallback",
            if config.vnc_fallback {
                "VNC admin channel retained"
            } else {
                "VNC fallback disabled"
            },
            Style::TEXT_DIM,
        );
    });

    ui.add_space(Style::SP_M);
    ui.label(
        RichText::new("Exposure")
            .color(Style::TEXT_DIM)
            .size(Style::SMALL)
            .strong(),
    );
    ui.add_space(Style::SP_XS);
    across_grid(ui, &RemoteProofingExposure::ALL, 3, |ui, &mode| {
        if proofing_choice_tile(
            ui,
            config.exposure == mode,
            mode.label(),
            mode.description(),
        ) {
            config.exposure = mode;
        }
    });
    if config.exposure == RemoteProofingExposure::Public {
        ui.add_space(Style::SP_XS);
        ui.colored_label(
            Style::DANGER,
            RichText::new("All-interfaces exposure requires explicit firewall policy.")
                .size(Style::SMALL),
        );
    }
    ui.add_space(Style::SP_XS);
    tile(ui, |ui| match config.exposure {
        RemoteProofingExposure::MeshOnly => {
            mesh_field(ui, "Bind target", mesh.overlay_ip.as_deref())
        }
        RemoteProofingExposure::Lan => field(ui, "Bind target", "LAN address", Style::TEXT),
        RemoteProofingExposure::Public => {
            field(ui, "Bind target", "0.0.0.0 / all interfaces", Style::WARN)
        }
    });

    ui.add_space(Style::SP_M);
    if fit_columns(ui.available_width(), 2) == 2 {
        ui.columns(2, |columns| {
            column_card(&mut columns[0], "Capture", |ui| {
                for capture in RemoteProofingCapture::ALL {
                    if proofing_choice_tile(
                        ui,
                        config.capture == capture,
                        capture.label(),
                        capture.description(),
                    ) {
                        config.capture = capture;
                    }
                }
            });
            column_card(&mut columns[1], "Encoder", |ui| {
                for encoder in RemoteProofingEncoder::ALL {
                    if proofing_choice_tile(
                        ui,
                        config.encoder == encoder,
                        encoder.label(),
                        encoder.description(),
                    ) {
                        config.encoder = encoder;
                    }
                }
            });
        });
    } else {
        column_card(ui, "Capture", |ui| {
            for capture in RemoteProofingCapture::ALL {
                if proofing_choice_tile(
                    ui,
                    config.capture == capture,
                    capture.label(),
                    capture.description(),
                ) {
                    config.capture = capture;
                }
            }
        });
        ui.add_space(Style::SP_S);
        column_card(ui, "Encoder", |ui| {
            for encoder in RemoteProofingEncoder::ALL {
                if proofing_choice_tile(
                    ui,
                    config.encoder == encoder,
                    encoder.label(),
                    encoder.description(),
                ) {
                    config.encoder = encoder;
                }
            }
        });
    }

    ui.add_space(Style::SP_M);
    column_card(ui, "Authorization and controls", |ui| {
        let mut native_prompt = config.native_pairing_prompt;
        if ui
            .checkbox(
                &mut native_prompt,
                RichText::new("Native shell pairing prompt").size(Style::SMALL),
            )
            .changed()
        {
            config.native_pairing_prompt = native_prompt;
        }

        let mut approval = config.require_local_approval;
        if ui
            .checkbox(
                &mut approval,
                RichText::new("Require local approval").size(Style::SMALL),
            )
            .changed()
        {
            config.require_local_approval = approval;
        }

        let mut indicator = config.show_shadowing_indicator;
        if ui
            .checkbox(
                &mut indicator,
                RichText::new("Show on-seat shadowing indicator").size(Style::SMALL),
            )
            .changed()
        {
            config.show_shadowing_indicator = indicator;
        }

        let mut input = config.allow_remote_input;
        if ui
            .checkbox(
                &mut input,
                RichText::new("Allow remote keyboard and mouse").size(Style::SMALL),
            )
            .changed()
        {
            config.allow_remote_input = input;
        }

        let mut vnc = config.vnc_fallback;
        if ui
            .checkbox(
                &mut vnc,
                RichText::new("Keep VNC fallback for rescue/admin").size(Style::SMALL),
            )
            .changed()
        {
            config.vnc_fallback = vnc;
        }

        ui.add_space(Style::SP_S);
        let mut fps = u32::from(config.min_fps_target);
        if ui
            .add(Slider::new(&mut fps, 15..=120).text("minimum proof FPS"))
            .changed()
        {
            config.min_fps_target = fps.clamp(15, 120) as u8;
        }
    });

    ui.add_space(Style::SP_M);
    let plan = config.service_plan(mesh);
    column_card(ui, "Effective service plan", |ui| {
        field(
            ui,
            "Service",
            if plan.enabled { "enabled" } else { "disabled" },
            if plan.enabled {
                Style::OK
            } else {
                Style::TEXT_DIM
            },
        );
        field(ui, "Bind scope", plan.bind_scope.label(), Style::TEXT);
        field(
            ui,
            "Bind address",
            plan.bind_address
                .as_deref()
                .unwrap_or("resolved by service"),
            if plan.bind_address.is_some() {
                Style::TEXT
            } else {
                Style::TEXT_DIM
            },
        );
        field(ui, "Firewall", plan.firewall.label(), Style::TEXT);
        field(ui, "Capture", plan.sunshine_capture, Style::TEXT);
        field(ui, "Encoder", plan.sunshine_encoder, Style::TEXT);
        field(
            ui,
            "Frame target",
            &format!("{} FPS minimum", plan.min_fps_target),
            Style::TEXT,
        );
        field(
            ui,
            "Pairing prompt",
            if plan.native_pairing_prompt {
                "native shell prompt"
            } else {
                "Sunshine prompt only"
            },
            Style::TEXT,
        );
        field(
            ui,
            "Local approval",
            if plan.require_local_approval {
                "required"
            } else {
                "not required"
            },
            if plan.require_local_approval {
                Style::OK
            } else {
                Style::WARN
            },
        );
        field(
            ui,
            "Remote input",
            if plan.allow_remote_input {
                "authorized after approval"
            } else {
                "view only"
            },
            Style::TEXT,
        );
        field(
            ui,
            "On-seat indicator",
            if plan.show_shadowing_indicator {
                "visible"
            } else {
                "off"
            },
            if plan.show_shadowing_indicator {
                Style::OK
            } else {
                Style::WARN
            },
        );
        field(
            ui,
            "VNC fallback",
            if plan.vnc_fallback {
                "available"
            } else {
                "disabled"
            },
            Style::TEXT_DIM,
        );
        for warning in &plan.warnings {
            ui.colored_label(Style::WARN, RichText::new(*warning).size(Style::SMALL));
        }
    });
}
