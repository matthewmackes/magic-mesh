//! U14 — the **placement picker**: choose the mesh node (local or a remote peer
//! over Nebula) a new workload is placed on, with per-node capacity bars driven
//! by [`mackes_mesh_types::cloud::NodeCapacity`]. Each node card shows its
//! used/total vCPU + memory as a load-toned bar and whether armed live apply is
//! available; selecting one returns its host id, which mod.rs stores as the
//! provision panel's placement target.

use mde_egui::egui::{self, Color32, RichText, Sense, Stroke};
use mde_egui::{card, section, status_dot, Style};

use super::WorkloadsState;

/// The near-full capacity threshold: at/above this fill fraction a bar warns
/// (below it reads the calm accent). A separate token so the escalation lives in
/// one place, not re-decided per bar.
const NEAR_FULL: f32 = 0.8;

/// The height of a capacity bar's track (a slim inset well).
const BAR_H: f32 = 6.0;

/// The placement picker's own state. The chosen node lives on
/// [`WorkloadsState::selected_node`] (the picker returns it; mod.rs stores it),
/// so the picker itself needs no persistent field.
#[derive(Debug, Default)]
pub(super) struct State;

/// Fraction of a capacity axis in use (`0.0`–, may exceed `1.0` when
/// over-committed). A node reporting zero total capacity reads `0.0` — an honest
/// "not reported", never a divide-by-zero (§7).
#[allow(clippy::cast_precision_loss)]
fn used_fraction(used: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        used as f32 / total as f32
    }
}

/// The capacity-bar tone for a fill fraction — all `Style` tokens (§4): the calm
/// [`Style::ACCENT`] under load, [`Style::SUPPORT_WARNING`] near-full, and
/// [`Style::DANGER`] at/over capacity, so a saturated node reads hot without a
/// minted colour.
fn bar_tone(fraction: f32) -> Color32 {
    if fraction >= 1.0 {
        Style::DANGER
    } else if fraction >= NEAR_FULL {
        Style::SUPPORT_WARNING
    } else {
        Style::ACCENT
    }
}

/// Render a value in MiB as GiB with one decimal (the readable memory scale).
#[allow(clippy::cast_precision_loss)]
fn fmt_gib(mib: u32) -> String {
    format!("{:.1}", mib as f32 / 1024.0)
}

/// Render the placement picker; return the node the operator chose this frame, if
/// any. An empty mirror reads as an honest "no nodes reporting" (§7), never a
/// fabricated node.
///
/// The `&mut WorkloadsState` is the mod.rs seam signature (the dispatcher writes
/// the returned node into `state.selected_node`); the picker reads state only, so
/// the mutable borrow is honestly unused here.
#[allow(clippy::needless_pass_by_ref_mut)]
pub(super) fn placement_picker(ui: &mut egui::Ui, state: &mut WorkloadsState) -> Option<String> {
    section().show(ui, |ui| {
        ui.label(
            RichText::new("Placement")
                .size(Style::BODY)
                .strong()
                .color(Style::TEXT),
        );
        mde_egui::muted_note(
            ui,
            "Choose the mesh node this workload runs on. Bars show used / total capacity per node.",
        );
    });

    let selected = state.selected_node().map(str::to_string);

    if state.states().is_empty() {
        crate::empty_state::show(
            ui,
            "No mesh nodes reporting",
            "A node appears here once its state/cloud mirror lands on the Bus with its capacity. \
             Off-mesh, this stays honestly empty.",
        );
        return None;
    }

    let mut chosen: Option<String> = None;
    for cs in state.states() {
        let is_selected = selected.as_deref() == Some(cs.host.as_str());
        let mut frame = card();
        if is_selected {
            frame = frame.stroke(Stroke::new(Style::FOCUS_RING_W, Style::ACCENT_WORKLOADS));
        }
        let picked = frame.show(ui, |ui| node_card(ui, cs, is_selected)).inner;
        if picked {
            chosen = Some(cs.host.clone());
        }
        ui.add_space(Style::SP_S);
    }
    chosen
}

/// One node card: the host + its armed-apply posture, the vCPU + memory capacity
/// bars, and a select affordance. Returns whether "Select" was clicked this
/// frame.
fn node_card(
    ui: &mut egui::Ui,
    cs: &mackes_mesh_types::cloud::CloudState,
    is_selected: bool,
) -> bool {
    ui.horizontal(|ui| {
        status_dot(
            ui,
            if cs.apply_armed {
                Style::OK
            } else {
                Style::TEXT_DIM
            },
        );
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new(&cs.host)
                .size(Style::BODY)
                .strong()
                .color(Style::TEXT),
        );
        ui.add_space(Style::SP_S);
        let (badge, tone) = if cs.apply_armed {
            ("live apply", Style::OK)
        } else {
            ("plan-only", Style::WARN)
        };
        ui.label(RichText::new(badge).size(Style::SMALL).color(tone));
    });
    ui.add_space(Style::SP_XS);

    let cap = &cs.node_capacity;
    capacity_bar(
        ui,
        "vCPU",
        &format!("{} / {}", cap.vcpu_used, cap.vcpu_total),
        used_fraction(cap.vcpu_used.into(), cap.vcpu_total.into()),
    );
    capacity_bar(
        ui,
        "Memory",
        &format!(
            "{} / {} GiB",
            fmt_gib(cap.mem_used_mb),
            fmt_gib(cap.mem_total_mb)
        ),
        used_fraction(cap.mem_used_mb.into(), cap.mem_total_mb.into()),
    );
    ui.add_space(Style::SP_XS);

    if is_selected {
        ui.label(
            RichText::new("Selected \u{2014} the provision form targets this node")
                .size(Style::SMALL)
                .color(Style::ACCENT_WORKLOADS),
        );
        false
    } else {
        ui.add(egui::Button::new(
            RichText::new("Select this node")
                .size(Style::SMALL)
                .color(Style::ACCENT_WORKLOADS),
        ))
        .clicked()
    }
}

/// One labelled capacity bar — `label  detail`, then a track with a fill sized to
/// the used fraction and toned by load ([`bar_tone`]). Pure token colours (§4).
fn capacity_bar(ui: &mut egui::Ui, label: &str, detail: &str, fraction: f32) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label)
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );
        ui.add_space(Style::SP_S);
        ui.label(RichText::new(detail).size(Style::SMALL).color(Style::TEXT));
    });

    let width = ui.available_width().max(Style::SP_XL);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, BAR_H), Sense::hover());
    ui.painter()
        .rect_filled(rect, Style::RADIUS_S, Style::LAYER_01);
    ui.painter().rect_stroke(
        rect,
        Style::RADIUS_S,
        Style::hairline(),
        egui::StrokeKind::Inside,
    );
    let fill_w = rect.width() * fraction.clamp(0.0, 1.0);
    if fill_w > 0.0 {
        let filled = egui::Rect::from_min_size(rect.min, egui::vec2(fill_w, rect.height()));
        ui.painter()
            .rect_filled(filled, Style::RADIUS_S, bar_tone(fraction));
    }
    ui.add_space(Style::SP_XS);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::float_cmp)]
    fn used_fraction_is_honest_and_guards_a_zero_total() {
        assert!((used_fraction(4, 16) - 0.25).abs() < 1e-6);
        assert_eq!(used_fraction(0, 0), 0.0);
        // A node that reports no total capacity reads 0.0 — never NaN/inf (§7).
        assert_eq!(used_fraction(8, 0), 0.0);
    }

    #[test]
    fn bar_tone_escalates_accent_then_warning_then_danger() {
        assert_eq!(bar_tone(0.2), Style::ACCENT);
        assert_eq!(bar_tone(NEAR_FULL), Style::SUPPORT_WARNING);
        assert_eq!(bar_tone(0.95), Style::SUPPORT_WARNING);
        assert_eq!(bar_tone(1.0), Style::DANGER);
        assert_eq!(bar_tone(1.5), Style::DANGER);
    }

    #[test]
    fn memory_renders_in_gib() {
        assert_eq!(fmt_gib(32768), "32.0");
        assert_eq!(fmt_gib(4096), "4.0");
        assert_eq!(fmt_gib(0), "0.0");
    }
}
