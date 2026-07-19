//! CHOOSER-2 render helpers — the **leaf card-grid drawing surface** split out
//! of the Desktop-chooser god-module (pure relocation, no behaviour change).
//!
//! Every item here is a painter/row/form helper the parent `chooser_panel`
//! render loop calls: the node-grouped card grid + "no match" copy, the
//! filter/sort bar, the source card (thumbnail well, body, power row, protocol
//! badges, tooltip, context menu), the CHOOSER-4 connect picker + credential
//! fields, the CHOOSER-8 manual-source edit form, and the accesskit a11y label
//! builders.
//!
//! `use super::*` pulls in the parent's `DesktopSource`/`FilterSort`/`CardAction`
//! wire types, the layout constants, and the egui re-exports; as a child module
//! it reads the parent's private types/fields directly, so the items the parent
//! (and the tests) call back into are `pub(super)`.

use super::*;

const CHOOSER_TOOLTIP_MAX_W: f32 = Style::SP_XL * 12.0;

pub(super) fn chooser_tooltip(ui: &mut egui::Ui, text: &str) {
    egui::Frame::NONE
        .fill(Style::SURFACE)
        .stroke(egui::Stroke::new(1.0, Style::BORDER))
        .corner_radius(Style::RADIUS_S)
        .inner_margin(Style::tooltip_margin())
        .show(ui, |ui| {
            ui.set_max_width(CHOOSER_TOOLTIP_MAX_W);
            ui.add(
                egui::Label::new(RichText::new(text).size(Style::SMALL).color(Style::TEXT)).wrap(),
            );
        });
}

fn chooser_hover_text(response: egui::Response, text: impl Into<String>) -> egui::Response {
    let text = text.into();
    response.on_hover_ui(move |ui| chooser_tooltip(ui, text.as_str()))
}

fn chooser_context_menu(response: &egui::Response, add_contents: impl FnOnce(&mut egui::Ui)) {
    let previous_style = response.ctx.style();
    let mut menu_style = (*previous_style).clone();
    apply_chooser_popup_style(&response.ctx, &mut menu_style);
    response.ctx.set_style(menu_style);
    let _ = response.context_menu(|ui| {
        let ctx = ui.ctx().clone();
        apply_chooser_popup_style(&ctx, ui.style_mut());
        add_contents(ui);
    });
    response.ctx.set_style(previous_style);
}

fn chooser_combo_menu<R>(
    ui: &mut egui::Ui,
    combo: egui::ComboBox,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<Option<R>> {
    ui.scope(|ui| {
        let ctx = ui.ctx().clone();
        apply_chooser_popup_style(&ctx, ui.style_mut());
        combo.show_ui(ui, |ui| {
            let ctx = ui.ctx().clone();
            apply_chooser_popup_style(&ctx, ui.style_mut());
            add_contents(ui)
        })
    })
    .inner
}

pub(super) fn apply_chooser_popup_style(ctx: &egui::Context, style: &mut egui::Style) {
    let palette = Style::current_palette(ctx);
    let border = egui::Stroke::new(1.0, palette.border);
    let text = egui::Stroke::new(1.0, palette.text);
    let text_dim = egui::Stroke::new(1.0, palette.text_dim);
    let accent = Style::resolve_color(ctx, Style::ACCENT);
    let visuals = &mut style.visuals;

    visuals.window_fill = palette.surface;
    visuals.panel_fill = palette.surface;
    visuals.faint_bg_color = palette.surface;
    visuals.extreme_bg_color = palette.bg;
    visuals.window_stroke = border;
    visuals.override_text_color = Some(palette.text);

    visuals.widgets.noninteractive.bg_fill = palette.surface;
    visuals.widgets.noninteractive.weak_bg_fill = palette.surface;
    visuals.widgets.noninteractive.bg_stroke = border;
    visuals.widgets.noninteractive.fg_stroke = text_dim;

    visuals.widgets.inactive.bg_fill = palette.surface;
    visuals.widgets.inactive.weak_bg_fill = palette.surface;
    visuals.widgets.inactive.bg_stroke = border;
    visuals.widgets.inactive.fg_stroke = text;

    visuals.widgets.hovered.bg_fill = palette.surface_hi;
    visuals.widgets.hovered.weak_bg_fill = palette.surface_hi;
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, accent);
    visuals.widgets.hovered.fg_stroke = text;

    visuals.widgets.active.bg_fill = palette.surface_hi;
    visuals.widgets.active.weak_bg_fill = palette.surface_hi;
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, accent);
    visuals.widgets.active.fg_stroke = text;

    visuals.widgets.open.bg_fill = palette.surface_hi;
    visuals.widgets.open.weak_bg_fill = palette.surface_hi;
    visuals.widgets.open.bg_stroke = border;
    visuals.widgets.open.fg_stroke = text;

    visuals.selection.bg_fill = accent.gamma_multiply(0.25);
    visuals.selection.stroke = egui::Stroke::new(1.0, accent);
    style.spacing.button_padding = egui::vec2(Style::SP_S, Style::CONTROL_PAD_Y);
    style.spacing.item_spacing = egui::vec2(Style::SP_XS, Style::TOOLBAR_INSET_Y);
}

/// The scrollable grid body: the CHOOSER-8 live narrowing (filter → node groups
/// ordered favorites-first + by the sort key), the "no match" copy when a filter
/// zeroes the roster, the CHOOSER-4 connect picker, the CHOOSER-8 manual-source
/// edit form, and the honest note + degraded-lane lines. Returns the one card
/// action chosen this frame (applied by [`chooser_panel`] after the render).
#[allow(clippy::too_many_arguments)]
pub(super) fn chooser_grid(
    ui: &mut egui::Ui,
    sources: &[DesktopSource],
    filter: &FilterSort,
    favorites: &HashSet<String>,
    recents: &HashSet<String>,
    pending_id: Option<&str>,
    note: Option<&str>,
    power_gate: Option<&str>,
    degraded: &[String],
    thumbs: &mut ThumbnailCache,
    pending_draft: &mut Option<ConnectDraft>,
    edit_draft: &mut Option<ManualEdit>,
) -> Option<CardAction> {
    let mut action: Option<CardAction> = None;

    // CHOOSER-8 — the live narrowing: filter the roster, then group by node (a
    // filtered subsequence of the worker-sorted roster stays sorted, so
    // consecutive runs are preserved) and order each group favorites-first + by
    // the sort key.
    let visible: Vec<DesktopSource> = sources
        .iter()
        .filter(|s| filter.matches(s))
        .cloned()
        .collect();

    if visible.is_empty() {
        ui.add_space(Style::SP_S);
        muted_note(
            ui,
            "No desktop matches the current search and filters — clear them to see the whole \
             roster.",
        );
    }

    for (node, mut members) in group_by_node(&visible) {
        order_members(&mut members, filter.sort, favorites);
        // The node/host group header (design lock 3).
        ui.add_space(Style::SP_S);
        ui.label(
            RichText::new(node)
                .color(Style::TEXT)
                .size(Style::BODY)
                .strong(),
        );
        ui.add_space(Style::SP_XS);
        ui.horizontal_wrapped(|ui| {
            for source in members {
                let pending = pending_id == Some(source.id.as_str());
                let favorite = favorites.contains(&source.id);
                let recent = recents.contains(&source.id);
                if let Some(a) =
                    source_card(ui, source, pending, favorite, recent, thumbs, power_gate)
                {
                    action = Some(a);
                }
                ui.add_space(Style::SP_S);
            }
        });
    }

    // The CHOOSER-4 always-ask connect picker — nothing connects unless the
    // operator confirms it (lock 6). The radios mutate the live draft.
    if let Some(draft) = pending_draft.as_mut() {
        if let Some(source) = sources.iter().find(|s| s.id == draft.source_id) {
            if let Some(a) = connect_picker(ui, source, draft) {
                action = Some(a);
            }
        }
    }

    // CHOOSER-8 — the manual-source form (mutually exclusive with the connect
    // picker). Its fields mutate the live draft; Save records the prefs register
    // then mirrors through the worker's typed verbs. TESTVM-4's ADD mode (empty
    // `original_id`) has no roster row to require, so it always renders.
    if let Some(edit) = edit_draft.as_mut() {
        if edit.original_id.is_empty() || sources.iter().any(|s| s.id == edit.original_id) {
            if let Some(a) = manual_edit_form(ui, edit) {
                action = Some(a);
            }
        }
    }

    if let Some(note) = note {
        ui.add_space(Style::SP_S);
        muted_note(ui, note);
    }

    // Degraded discovery lanes, named under the grid (§7 — a lane that found
    // nothing says why, instead of silently omitting).
    if !degraded.is_empty() {
        ui.add_space(Style::SP_S);
        for line in degraded {
            muted_note(ui, line);
        }
    }

    action
}

/// The CHOOSER-8 find bar: a search box + the node / protocol / status / OS
/// filters + the sort key, all `Style`-tokened (§4). Every control mutates the
/// live `filter` in place, so the grid narrows on the same frame — a pure fold
/// over the published roster (§6). `nodes` / `oses` are the roster's distinct
/// values (the combo option lists); the OS combo is omitted when no source
/// carries an OS hint. A Clear button appears while any filter is active.
pub(super) fn filter_bar(
    ui: &mut egui::Ui,
    filter: &mut FilterSort,
    nodes: &[String],
    oses: &[String],
) {
    ui.horizontal_wrapped(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut filter.search)
                .desired_width(Style::SP_XL * 5.0)
                .hint_text("Search name / node / OS…"),
        );
        ui.add_space(Style::SP_S);

        // Node filter.
        chooser_combo_menu(
            ui,
            egui::ComboBox::from_id_salt("chooser-filter-node")
                .selected_text(filter.node.as_deref().unwrap_or("All nodes")),
            |ui| {
                ui.selectable_value(&mut filter.node, None, "All nodes");
                for node in nodes {
                    ui.selectable_value(&mut filter.node, Some(node.clone()), node);
                }
            },
        );
        ui.add_space(Style::SP_S);

        // Protocol filter.
        chooser_combo_menu(
            ui,
            egui::ComboBox::from_id_salt("chooser-filter-proto")
                .selected_text(filter.protocol.map_or("Any protocol", Protocol::badge)),
            |ui| {
                ui.selectable_value(&mut filter.protocol, None, "Any protocol");
                for proto in Protocol::ALL {
                    ui.selectable_value(&mut filter.protocol, Some(proto), proto.badge());
                }
            },
        );
        ui.add_space(Style::SP_S);

        // Status filter.
        chooser_combo_menu(
            ui,
            egui::ComboBox::from_id_salt("chooser-filter-status")
                .selected_text(filter.status.map_or("Any status", Reachability::label)),
            |ui| {
                ui.selectable_value(&mut filter.status, None, "Any status");
                for status in Reachability::ALL {
                    ui.selectable_value(&mut filter.status, Some(status), status.label());
                }
            },
        );
        ui.add_space(Style::SP_S);

        // OS filter — only when the roster carries OS hints.
        if !oses.is_empty() {
            chooser_combo_menu(
                ui,
                egui::ComboBox::from_id_salt("chooser-filter-os")
                    .selected_text(filter.os.as_deref().unwrap_or("Any OS")),
                |ui| {
                    ui.selectable_value(&mut filter.os, None, "Any OS");
                    for os in oses {
                        ui.selectable_value(&mut filter.os, Some(os.clone()), os);
                    }
                },
            );
            ui.add_space(Style::SP_S);
        }

        // Sort key.
        ui.label(
            RichText::new("Sort")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        chooser_combo_menu(
            ui,
            egui::ComboBox::from_id_salt("chooser-sort").selected_text(filter.sort.label()),
            |ui| {
                for key in SortKey::ALL {
                    ui.selectable_value(&mut filter.sort, key, key.label());
                }
            },
        );

        // Clear — only while something is narrowing the grid.
        if filter.is_active() {
            ui.add_space(Style::SP_S);
            if ui
                .button(RichText::new("Clear").size(Style::SMALL))
                .clicked()
            {
                filter.clear();
            }
        }
    });
}

/// The inline controls a card's body surfaced this frame (CHOOSER-7/8): a power
/// op clicked, and/or the offline Retry affordance. Reconciled against the card's
/// primary click + its context menu by [`source_card`].
#[derive(Default)]
struct CardControls {
    /// A CHOOSER-7 local-VM power op clicked this frame.
    power: Option<PowerOp>,
    /// The CHOOSER-8 offline Retry button was clicked.
    retry: bool,
}

/// Render one desktop card: the thumbnail well (the decoded live preview, or the
/// honest monitor-icon fallback), the display name, the VM power state when there
/// is one, the protocol badge row, and the status pip — greyed with the worker's
/// reason when the source is offline (lock 14), with a pin marker when favorited
/// (CHOOSER-8). A left click on a connectable card activates it; a right click
/// raises the per-card context menu (CHOOSER-8). Returns the chosen action.
pub(super) fn source_card(
    ui: &mut egui::Ui,
    source: &DesktopSource,
    pending: bool,
    favorite: bool,
    recent: bool,
    thumbs: &mut ThumbnailCache,
    gate: Option<&str>,
) -> Option<CardAction> {
    let card = egui::vec2(CARD_WIDTH, CARD_HEIGHT);
    // Every card senses clicks so it can raise the CHOOSER-8 context menu (right
    // click) — but only a connectable card's primary click ACTIVATES (an offline
    // card's left click is a no-op; its Retry/context menu drive it). Its inline
    // buttons stay live regardless.
    let sense = Sense::click();
    // The whole card is ONE interactive container. `UiBuilder::sense` registers the
    // card's click BELOW any widget inside it, so a power/Retry button (added
    // within) receives its own click instead — a Stop/Pause tap never doubles as a
    // console-open activate (the CHOOSER-7 co-existence, egui's documented idiom).
    let scoped = ui.scope_builder(egui::UiBuilder::new().sense(sense), |ui| {
        // Reserve exactly the card so the grid stays regular; the plate is painted
        // over this fixed rect and the body lays out within it.
        ui.set_min_size(card);
        ui.set_max_width(CARD_WIDTH);
        let rect = egui::Rect::from_min_size(ui.min_rect().min, card);
        let hovered = source.connectable() && ui.rect_contains_pointer(rect);

        // The card plate — painted first so the content lays out over it.
        let fill = if hovered {
            Style::SURFACE_HI
        } else {
            Style::SURFACE
        };
        let border = if pending {
            Style::ACCENT_HI
        } else if hovered {
            Style::ACCENT
        } else {
            Style::BORDER
        };
        ui.painter().rect_filled(rect, Style::RADIUS, fill);
        ui.painter().rect_stroke(
            rect,
            Style::RADIUS,
            Stroke::new(1.0, border),
            StrokeKind::Inside,
        );
        // CHOOSER-8 — the pin marker: a small accent dot in the top-right corner
        // (a painter primitive, font-independent) at full strength even on a
        // dimmed offline card.
        if favorite {
            ui.painter().circle_filled(
                rect.right_top() + egui::vec2(-Style::SP_S, Style::SP_S),
                Style::SP_XS * 0.75,
                Style::ACCENT_HI,
            );
        }

        if !source.connectable() {
            ui.set_opacity(OFFLINE_OPACITY);
        }
        ui.horizontal(|ui| {
            ui.add_space(Style::SP_S);
            ui.vertical(|ui| {
                ui.set_width(Style::SP_S.mul_add(-2.0, CARD_WIDTH));
                ui.add_space(Style::SP_S);
                card_body(ui, source, recent, thumbs, gate)
            })
            .inner
        })
        .inner
    });
    let controls = scoped.inner;
    let response = chooser_hover_text(scoped.response, card_tooltip(source));

    // a11y-05 — the card's own accesskit `Button` node (role + name + state +
    // bounds + Click), keyed by the response id so egui merges it onto this
    // sensing scope. Pure metadata: registered every frame, never alters the
    // paint above.
    install_card_accessibility(
        ui.ctx(),
        response.id,
        source,
        favorite,
        pending,
        recent,
        response.rect,
    );

    // CHOOSER-8 — the per-card context menu (right click). A menu pick takes
    // precedence over the inline controls + the primary click.
    let mut menu_action = None;
    chooser_context_menu(&response, |ui| {
        card_context_menu(ui, source, favorite, &mut menu_action);
    });
    if menu_action.is_some() {
        return menu_action;
    }
    // An inline button click takes precedence over (and suppresses) the console-open.
    if let Some(op) = controls.power {
        return Some(CardAction::Power {
            id: source.id.clone(),
            op,
        });
    }
    if controls.retry {
        return Some(CardAction::Retry(source.id.clone()));
    }
    // Only a connectable card's primary click activates (lock 14).
    (response.clicked() && source.connectable()).then(|| CardAction::Activate(source.id.clone()))
}

/// The CHOOSER-8 per-card context menu: Connect (connectable only), Pin/Unpin,
/// Retry discovery (offline only), the KVM power ops for a local VM (reusing the
/// CHOOSER-7 state machine), and Edit / Remove for a manual source. Every item is
/// offered only when it can genuinely act (§7). Writes the chosen action into
/// `out` and closes the menu.
pub(super) fn card_context_menu(
    ui: &mut egui::Ui,
    source: &DesktopSource,
    favorite: bool,
    out: &mut Option<CardAction>,
) {
    if source.connectable() && ui.button("Connect…").clicked() {
        *out = Some(CardAction::Activate(source.id.clone()));
        ui.close_menu();
    }
    let pin_label = if favorite { "Unpin" } else { "Pin to front" };
    if ui.button(pin_label).clicked() {
        *out = Some(CardAction::ToggleFavorite(source.id.clone()));
        ui.close_menu();
    }
    // The offline Retry (lock 14 — a non-blocking discovery re-enumerate, never a
    // probe from here).
    if !source.connectable() && ui.button("Retry discovery").clicked() {
        *out = Some(CardAction::Retry(source.id.clone()));
        ui.close_menu();
    }
    // KVM power — the CHOOSER-7 state-appropriate ops, only for a local VM.
    if source.origin == SourceOrigin::LocalVm {
        let ops = source
            .power_state
            .as_deref()
            .map_or(PowerState::Unknown, PowerState::from_wire)
            .actions();
        if !ops.is_empty() {
            ui.separator();
            for op in ops {
                if ui.button(op.label()).clicked() {
                    *out = Some(CardAction::Power {
                        id: source.id.clone(),
                        op: *op,
                    });
                    ui.close_menu();
                }
            }
        }
    }
    // Manage a manual (operator-added) source.
    if source.origin == SourceOrigin::Manual {
        ui.separator();
        if ui.button("Edit…").clicked() {
            *out = Some(CardAction::EditSource(source.id.clone()));
            ui.close_menu();
        }
        if ui.button("Remove").clicked() {
            *out = Some(CardAction::RemoveSource(source.id.clone()));
            ui.close_menu();
        }
    }
}

/// The card's thumbnail well: the source's decoded live preview when its
/// `thumbnail_ref` resolves (aspect-fit, letterboxed so a 16:9 desktop never
/// stretches), else the honest shared monitor glyph — never a fake screenshot
/// (§7). The decode is bounded + throttled by [`ThumbnailCache`] (Q7).
pub(super) fn thumbnail_well(
    ui: &mut egui::Ui,
    source: &DesktopSource,
    thumbs: &mut ThumbnailCache,
) {
    let well = egui::vec2(ui.available_width(), THUMB_HEIGHT);
    let (rect, _) = ui.allocate_exact_size(well, Sense::hover());
    // The recessed plate the icon sat on / the snapshot is letterboxed over.
    ui.painter().rect_filled(rect, Style::RADIUS, Style::BG);
    if let Some(tex) = thumbs.texture_for(ui.ctx(), source) {
        // A live snapshot decoded: aspect-fit (letterbox) inside the well.
        let fit = fit_centered(rect.shrink(Style::SP_XS), tex.size_vec2());
        egui::Image::new(egui::load::SizedTexture::new(tex.id(), fit.size())).paint_at(ui, fit);
    } else {
        // Honest fallback: the shared monitor glyph, never a fake screenshot.
        let glyph = egui::Rect::from_center_size(
            rect.center(),
            egui::vec2(Style::SP_XL * 2.0, Style::SP_XL * 1.6),
        );
        crate::session::draw_monitor(&ui.painter().clone(), glyph);
    }
}

/// The largest rect of `img`'s aspect ratio centered inside `bounds` (letterbox
/// fit — never upscale-stretch a snapshot to the well's aspect). A degenerate
/// image size falls back to the full bounds.
pub(super) fn fit_centered(bounds: egui::Rect, img: egui::Vec2) -> egui::Rect {
    if img.x <= 0.0 || img.y <= 0.0 {
        return bounds;
    }
    let scale = (bounds.width() / img.x).min(bounds.height() / img.y);
    egui::Rect::from_center_size(bounds.center(), egui::vec2(img.x * scale, img.y * scale))
}

/// The card's content rows, top to bottom inside the plate. Returns the inline
/// controls surfaced this frame — a CHOOSER-7 power op on a local-VM card, and/or
/// the CHOOSER-8 offline Retry.
fn card_body(
    ui: &mut egui::Ui,
    source: &DesktopSource,
    recent: bool,
    thumbs: &mut ThumbnailCache,
    gate: Option<&str>,
) -> CardControls {
    thumbnail_well(ui, source, thumbs);
    ui.add_space(Style::SP_XS);

    // Name + (for a VM) its live power state.
    ui.label(
        RichText::new(&source.name)
            .color(Style::TEXT)
            .size(Style::BODY)
            .strong(),
    );
    if let Some(power) = source.power_state.as_deref() {
        let tone = if power.trim() == "running" {
            Style::OK
        } else {
            Style::TEXT_DIM
        };
        ui.colored_label(
            tone,
            RichText::new(format!("vm {power}")).size(Style::SMALL),
        );
    }
    ui.add_space(Style::SP_XS);

    // Protocol badges (design lock 2 — protocol is a per-card badge).
    ui.horizontal(|ui| {
        for offer in &source.protocols {
            protocol_badge(ui, *offer);
            ui.add_space(Style::SP_XS);
        }
    });
    ui.add_space(Style::SP_XS);

    // The status pip + the origin caption; a greyed card carries the
    // worker's reason instead of the caption (lock 14).
    ui.horizontal(|ui| {
        status_dot(ui, source.reachability.pip());
        ui.add_space(Style::SP_XS);
        match source.reason.as_deref() {
            Some(reason) if !source.connectable() => {
                muted_note(ui, reason);
            }
            _ => {
                // CHOOSER-9 — a recently-used desktop reads "recently used" wherever
                // the operator sits (the synced recents cache drives this marker).
                let caption = if recent {
                    format!(
                        "{} \u{00B7} {} \u{00B7} recently used",
                        source.reachability.label(),
                        source.origin.label()
                    )
                } else {
                    format!(
                        "{} \u{00B7} {}",
                        source.reachability.label(),
                        source.origin.label()
                    )
                };
                muted_note(ui, caption);
            }
        }
    });

    let mut controls = CardControls::default();

    // CHOOSER-7 — the local-VM power controls. Only a local VM (this node's
    // libvirt) is powered from here; a peer VM is powered from its own node.
    if source.origin == SourceOrigin::LocalVm {
        ui.add_space(Style::SP_XS);
        controls.power = power_row(ui, source, gate);
    }

    // CHOOSER-8 — the offline Retry affordance (lock 14). A local VM already
    // exposes its Start button (the bring-online path), so Retry is offered on the
    // OTHER offline cards (a peer/LAN endpoint) — a non-blocking discovery
    // re-enumerate, never a probe. It reads at full strength on the dimmed card.
    if !source.connectable() && source.origin != SourceOrigin::LocalVm {
        ui.add_space(Style::SP_XS);
        ui.set_opacity(1.0);
        let retry = ui.add(egui::Button::new(RichText::new("Retry").size(Style::SMALL)));
        if chooser_hover_text(retry, "Re-check discovery — nothing is probed from here").clicked()
        {
            controls.retry = true;
        }
    }

    controls
}

/// The local-VM power-control row (CHOOSER-7): buttons appropriate to the VM's
/// live power state — Start a stopped desktop (one click away), Stop/Pause a
/// running one, Resume a paused one. When the node has no local hypervisor
/// (`gate` is `Some`) the buttons render disabled with the honest reason, never a
/// control that pretends to act (§7). Returns the op clicked this frame, if any.
pub(super) fn power_row(
    ui: &mut egui::Ui,
    source: &DesktopSource,
    gate: Option<&str>,
) -> Option<PowerOp> {
    let state = source
        .power_state
        .as_deref()
        .map_or(PowerState::Unknown, PowerState::from_wire);
    let ops = state.actions();
    // A live hypervisor + an unmapped state offers no honest action — draw nothing.
    if ops.is_empty() && gate.is_none() {
        return None;
    }
    // Power controls read at full strength even on a dimmed (offline) card — a
    // stopped desktop's Start button must look one click away, not greyed out.
    ui.set_opacity(1.0);
    let enabled = gate.is_none();
    let mut clicked = None;
    ui.horizontal(|ui| {
        for op in ops {
            if ui
                .add_enabled(
                    enabled,
                    egui::Button::new(RichText::new(op.label()).size(Style::SMALL)),
                )
                .clicked()
            {
                clicked = Some(*op);
            }
            ui.add_space(Style::SP_XS);
        }
    });
    if let Some(reason) = gate {
        muted_note(ui, format!("no local hypervisor — {reason}"));
    }
    clicked
}

/// The card tooltip — the honest connection detail (origin, dial address, OS
/// hint when genuinely known).
pub(super) fn card_tooltip(source: &DesktopSource) -> String {
    let mut text = format!("{} \u{00B7} {}", source.origin.label(), source.host);
    if let Some(os) = source.os_hint.as_deref() {
        text.push_str(" \u{00B7} ");
        text.push_str(os);
    }
    text
}

/// One protocol badge chip. The known port rides the hover (the chip stays a
/// clean three-letter badge — lock 2).
pub(super) fn protocol_badge(ui: &mut egui::Ui, offer: ProtocolOffer) {
    let galley = ui.painter().layout_no_wrap(
        offer.protocol.badge().to_string(),
        FontId::proportional(Style::SMALL),
        Style::ACCENT_HI,
    );
    let pad = egui::vec2(Style::SP_XS * 2.0, Style::SP_XS);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + pad * 2.0, Sense::hover());
    ui.painter()
        .rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    ui.painter()
        .galley(rect.min + pad, galley, Style::ACCENT_HI);
    if let Some(port) = offer.port {
        let _ = chooser_hover_text(resp, format!("port {port}"));
    }
}

/// The CHOOSER-4 always-ask connect picker (§7, never a silent stub): a protocol
/// radio row when the source offered several routable protocols (lock 6 — never a
/// silent default), the fullscreen/windowed choice (lock 9), and the single/span-
/// all monitor choice (lock 12), then Connect / Cancel. The radios mutate the live
/// `draft`; §4 chrome via `Style` tokens. Returns the confirm/cancel action.
pub(super) fn connect_picker(
    ui: &mut egui::Ui,
    source: &DesktopSource,
    draft: &mut ConnectDraft,
) -> Option<CardAction> {
    let mut action = None;
    ui.add_space(Style::SP_M);
    ui.separator();
    ui.add_space(Style::SP_S);
    ui.label(
        RichText::new(format!("Connect to {}", source.name))
            .color(Style::TEXT)
            .size(Style::BODY)
            .strong(),
    );
    ui.add_space(Style::SP_XS);

    // The routable offers this source advertises, in the worker's stable order.
    let routable: Vec<VdiProtocol> = source
        .protocols
        .iter()
        .filter_map(|o| o.protocol.route())
        .collect();

    // Protocol — always-ask as a radio row when several are routable (lock 6).
    // A single routable protocol is stated (no false choice) so WHAT will be used
    // is still explicit.
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Protocol")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        if routable.len() > 1 {
            let before = draft.protocol;
            for proto in &routable {
                ui.radio_value(&mut draft.protocol, *proto, proto.label());
            }
            // CHOOSER-6 — a sealed credential is keyed per protocol, so switching
            // protocol invalidates any raised prompt (re-resolved on next Connect).
            if draft.protocol != before {
                draft.cred_prompt = None;
            }
        } else {
            ui.label(RichText::new(draft.protocol.label()).color(Style::TEXT));
        }
    });

    // Display mode — fullscreen or windowed (lock 9).
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Display")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.radio_value(&mut draft.display, DisplayMode::Fullscreen, "Fullscreen");
        ui.radio_value(&mut draft.display, DisplayMode::Windowed, "Windowed");
    });

    // Monitor span — a single display or span all (lock 12).
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Monitors")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.radio_value(&mut draft.monitors, MonitorSpan::Single, "Single display");
        ui.radio_value(&mut draft.monitors, MonitorSpan::All, "Span all");
    });

    // Future protocol routes without a client must say so, never imply a live
    // session (§7).
    if !draft.protocol.has_client() {
        ui.add_space(Style::SP_XS);
        muted_note(
            ui,
            format!(
                "The {} client is not wired yet — the request is recorded, but no session is faked.",
                draft.protocol.label()
            ),
        );
    }

    // CHOOSER-6 — the one-time credential prompt for an external endpoint with no
    // sealed credential yet (raised on the first Connect). Filled once, sealed on
    // the next Connect, then remembered.
    let prompting = draft.cred_prompt.is_some();
    if let Some(prompt) = draft.cred_prompt.as_mut() {
        credential_prompt_fields(ui, prompt);
    }

    ui.add_space(Style::SP_S);
    ui.horizontal(|ui| {
        // Once the prompt is up the Connect action seals the entered credential
        // before connecting — the label says so honestly.
        let connect_label = if prompting {
            format!("Save and connect via {}", draft.protocol.label())
        } else {
            format!("Connect via {}", draft.protocol.label())
        };
        if ui
            .button(RichText::new(connect_label).size(Style::BODY))
            .clicked()
        {
            action = Some(CardAction::Confirm);
        }
        ui.add_space(Style::SP_S);
        if ui
            .button(RichText::new("Cancel").size(Style::BODY))
            .clicked()
        {
            action = Some(CardAction::Cancel);
        }
    });
    action
}

/// The CHOOSER-6 one-time credential fields for an external endpoint: a masked
/// username/password pair under an honest note. §4 `Style` tokens throughout (no
/// raw hex); the password field is masked and the secret is never logged (the
/// [`CredentialPrompt`] buffer redacts through `Debug`).
pub(super) fn credential_prompt_fields(ui: &mut egui::Ui, prompt: &mut CredentialPrompt) {
    ui.add_space(Style::SP_S);
    ui.separator();
    ui.add_space(Style::SP_XS);
    muted_note(
        ui,
        "This endpoint isn't on the mesh — enter its credentials once. They're sealed in the \
         secret store and remembered for next time; never stored in plaintext.",
    );
    ui.add_space(Style::SP_XS);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Username")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add(
            egui::TextEdit::singleline(&mut prompt.username)
                .desired_width(Style::SP_XL * 6.0)
                .hint_text("optional for VNC"),
        );
    });
    ui.add_space(Style::SP_XS);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Password")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add(
            egui::TextEdit::singleline(&mut prompt.password)
                .desired_width(Style::SP_XL * 6.0)
                .password(true),
        );
    });
}

/// The CHOOSER-8 manual-source edit form (context-menu → Edit): editable name /
/// host / port / protocol with an inline validation error, then Save / Cancel.
/// Save republishes through the worker's typed add/remove verbs (§6, never a
/// command string). The fields mutate the live `edit` draft; §4 `Style` tokens
/// throughout. Returns the save/cancel action.
pub(super) fn manual_edit_form(ui: &mut egui::Ui, edit: &mut ManualEdit) -> Option<CardAction> {
    let mut action = None;
    ui.add_space(Style::SP_M);
    ui.separator();
    ui.add_space(Style::SP_S);
    let title = if edit.original_id.is_empty() {
        "Pin a desktop endpoint" // TESTVM-4 ADD mode
    } else {
        "Edit manual desktop"
    };
    ui.label(
        RichText::new(title)
            .color(Style::TEXT)
            .size(Style::BODY)
            .strong(),
    );
    ui.add_space(Style::SP_XS);

    edit_field(
        ui,
        "Name",
        &mut edit.name,
        "optional \u{2014} defaults to host:port",
    );
    edit_field(ui, "Host", &mut edit.host, "10.0.0.5 or host.local");
    edit_field(ui, "Port", &mut edit.port, "3389");

    // Protocol radio row (rdp / vnc / spice).
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Protocol")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        for proto in Protocol::ALL {
            ui.radio_value(&mut edit.protocol, proto, proto.badge());
        }
    });
    ui.add_space(Style::SP_XS);

    // TESTVM-4 — the optional stored credential. Filled → Connect goes straight
    // through with it (a pinned lab/test endpoint); left empty → the CHOOSER-6
    // one-time prompt + sealed store applies, exactly as before.
    edit_field(
        ui,
        "Username",
        &mut edit.username,
        "optional \u{2014} login user (RDP)",
    );
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Password")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add(
            egui::TextEdit::singleline(&mut edit.password)
                .desired_width(Style::SP_XL * 6.0)
                .password(true)
                .hint_text("optional \u{2014} stored with this endpoint"),
        );
    });
    ui.add_space(Style::SP_XS);
    muted_note(
        ui,
        "A stored password rides the roaming chooser prefs and connects with no prompt — \
         meant for lab/test endpoints. Leave it empty to be asked once and sealed in the \
         secret store instead.",
    );

    // The inline validation error (empty host / bad port) — never a silent drop.
    if let Some(err) = edit.error.as_deref() {
        ui.add_space(Style::SP_XS);
        ui.colored_label(Style::DANGER, err);
    }

    ui.add_space(Style::SP_S);
    ui.horizontal(|ui| {
        if ui.button(RichText::new("Save").size(Style::BODY)).clicked() {
            action = Some(CardAction::SaveEdit);
        }
        ui.add_space(Style::SP_S);
        if ui
            .button(RichText::new("Cancel").size(Style::BODY))
            .clicked()
        {
            action = Some(CardAction::CancelEdit);
        }
    });
    action
}

/// One labelled single-line edit row for the manual-source form (§4 tokens).
pub(super) fn edit_field(ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label)
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add(
            egui::TextEdit::singleline(value)
                .desired_width(Style::SP_XL * 6.0)
                .hint_text(hint),
        );
    });
    ui.add_space(Style::SP_XS);
}

// ── accesskit (a11y-05 / shell-ux-6) ─────────────────────────────────────────
//
// The Desktop Chooser's grid is the shell's primary entry-navigation surface,
// yet every [`source_card`] is a hand-rolled `scope_builder(...).sense(click)`
// container painted with raw [`egui::Painter`] calls — egui only auto-generates
// accesskit nodes for real widgets (`Button`/`TextEdit`) via
// `Response::widget_info`, never for these raw sensing scopes (the same gap
// dock.rs/console.rs closed for their own cells under WIN7-5/WIN7-7). So a
// screen reader walking this grid heard nothing. This section gives each
// pickable card its own `Role::Button` node keyed by the card response's id —
// the identical shape (`role` + fixed identity `label` + current `value` +
// `bounds` + `Click` action) `console.rs`'s `install_row_accessibility` and
// `dock.rs`'s `install_cell_accessibility` already use, restated module-locally
// per this crate's established per-module-copy convention. Selected/offline/
// pinned state rides in the `value` string, exactly as dock.rs carries its
// pin/notification state in the accesskit value (no `set_disabled`/
// `set_selected` — the crate's whole shell keeps to the five setters).

/// Convert an egui rect to an accesskit one (the `console.rs`/`dock.rs` helper,
/// restated module-locally — the established per-module-copy idiom).
pub(super) fn accesskit_rect(rect: egui::Rect) -> egui::accesskit::Rect {
    egui::accesskit::Rect {
        x0: rect.min.x.into(),
        y0: rect.min.y.into(),
        x1: rect.max.x.into(),
        y1: rect.max.y.into(),
    }
}

/// The accessible **name** of a desktop card — its display identity, the same
/// string the card paints in bold ([`card_body`]). A screen reader announces
/// "button, {name}, {state}", so the name is the identity and the reading rides
/// in the value ([`card_a11y_state`]).
pub(super) fn card_a11y_name(source: &DesktopSource) -> String {
    source.name.clone()
}

/// The accessible **state/value** of a desktop card — mirrors the visible
/// caption ([`card_body`]) so the two can never drift: the greyed card's worker
/// `reason` when offline, else "{reachability} · {origin}[ · recently used]",
/// plus the VM power reading, and the pinned / connecting markers the card
/// paints as a corner dot / accent border. Screen-reader users get exactly what
/// a sighted operator reads off the card.
pub(super) fn card_a11y_state(
    source: &DesktopSource,
    favorite: bool,
    pending: bool,
    recent: bool,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    // The primary caption line — the offline reason replaces the status/origin
    // caption exactly as the painted `muted_note` does (lock 14).
    match source.reason.as_deref() {
        Some(reason) if !source.connectable() => parts.push(reason.to_owned()),
        _ => {
            parts.push(source.reachability.label().to_owned());
            parts.push(source.origin.label().to_owned());
            if recent {
                parts.push("recently used".to_owned());
            }
        }
    }
    if let Some(power) = source.power_state.as_deref() {
        parts.push(format!("vm {power}"));
    }
    if favorite {
        parts.push("pinned".to_owned());
    }
    if pending {
        parts.push("connecting".to_owned());
    }
    parts.join(" \u{00B7} ")
}

/// Install one desktop card's accesskit `Button` node, keyed by the card
/// response's own id so egui merges it onto the sensing scope (the same
/// id-keyed merge dock.rs relies on for its raw cells). A no-op-shaped call
/// when the `accesskit` feature is off (egui returns `None` and the builder is
/// never invoked); a pure metadata write when on — never touches rendering.
pub(super) fn install_card_accessibility(
    ctx: &egui::Context,
    id: egui::Id,
    source: &DesktopSource,
    favorite: bool,
    pending: bool,
    recent: bool,
    rect: egui::Rect,
) {
    let name = card_a11y_name(source);
    let state = card_a11y_state(source, favorite, pending, recent);
    let _ = ctx.accesskit_node_builder(id, |node| {
        node.set_role(egui::accesskit::Role::Button);
        node.set_label(name);
        node.set_value(state);
        node.set_bounds(accesskit_rect(rect));
        node.add_action(egui::accesskit::Action::Click);
    });
}
