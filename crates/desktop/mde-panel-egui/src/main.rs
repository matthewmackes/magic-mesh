//! The eframe **panel shell** for `mde-panel-egui` (E12-7) — thin glue over the
//! tested [`PanelModel`]. It polls the two live sources the panel subscribes to
//! (the world-readable mesh-status snapshot file + the mesh-replicated bus DND
//! state) on a fixed cadence, and draws the worst-of mesh-health pip + the
//! Do-Not-Disturb quick action through the shared [`Style`]/[`Motion`].
//!
//! All decision logic + the Style colour mapping live in the `mde_panel_egui`
//! lib (unit-tested without a GPU); this file is IO + draw only.

// The pip pulse eases in f64 off the egui frame clock and is narrowed to f32 for
// the colour alpha (an inherent canvas-math cast), and the alpha lerp reads far
// clearer as `0.55 + 0.45 * pulse` than the `mul_add` rewrite — both the
// established mde-mesh-view idiom, allowed crate-wide here rather than per-site.
#![allow(clippy::cast_possible_truncation, clippy::suboptimal_flops)]

use std::f64::consts::TAU;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mde_egui::egui::{self, RichText};
use mde_egui::{eframe, run_client, Motion, Style};

use mde_bus::dnd;
use mde_panel_egui::{PanelModel, DND_LABEL};

/// Wayland app-id — the compositor groups windows + maps icons by it.
const APP_ID: &str = "org.magicmesh.Panel";

/// Poll cadence for the live sources: a new snapshot or a peer's DND flip
/// surfaces within this window. Matches the cosmic-applet's 5 s refresh.
const REFRESH: Duration = Duration::from_secs(5);

/// The world-readable mesh-status snapshot the root timer writes. The panel runs
/// as the desktop user and cannot read the root-only peer directory, so this is
/// the mesh-health read path (the same source the cosmic-applet's LIGHTHOUSE-7
/// pip used).
const SNAPSHOT_PATH: &str = "/run/mde/mesh-status.json";

/// One full pip-pulse period (seconds), derived from the shared [`Motion`] table
/// so the cadence stays on the harness timing scale, not a bespoke literal.
const PIP_PULSE_SECS: f64 = Motion::SLOW as f64 * 4.0;

/// The eframe panel: the live model plus the small IO context needed to refresh
/// and persist it.
struct Panel {
    /// The render-agnostic model, rebuilt each poll.
    model: PanelModel,
    /// Bus root holding the mesh-replicated `dnd.yaml` (the quick action's
    /// subscription + write target). `None` when no bus dir resolves.
    bus_root: Option<PathBuf>,
    /// Local peer name stamped onto a DND toggle.
    peer: String,
    /// When the live sources were last polled.
    last_poll: Instant,
    /// Set when the last DND write failed — surfaced inline (honest error, no
    /// panic, no silent drop).
    last_error: Option<String>,
}

impl Panel {
    /// Build the panel, priming the model from the live sources immediately so it
    /// reflects current state on first paint.
    fn new() -> Self {
        let bus_root = mde_bus::client_data_dir();
        let model =
            PanelModel::from_state(read_snapshot().as_deref(), load_dnd(bus_root.as_deref()));
        Self {
            model,
            bus_root,
            peer: local_peer(),
            last_poll: Instant::now(),
            last_error: None,
        }
    }

    /// One subscription tick: re-read both live sources into the model.
    fn poll(&mut self) {
        self.model = PanelModel::from_state(
            read_snapshot().as_deref(),
            load_dnd(self.bus_root.as_deref()),
        );
    }

    /// Flip DND and persist it to the bus so the change replicates mesh-wide.
    fn toggle_dnd(&mut self) {
        let next = self.model.toggled_dnd(&self.peer, now_unix_ms());
        match self.bus_root.as_deref() {
            Some(root) => match dnd::save_default(root, &next) {
                Ok(()) => {
                    self.model.set_dnd(next);
                    self.last_error = None;
                }
                Err(e) => self.last_error = Some(format!("Couldn't save DND: {e}")),
            },
            None => {
                self.last_error = Some("No mesh bus directory — DND unavailable".to_string());
            }
        }
    }

    /// Draw the whole panel (called inside the central panel).
    fn show(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add_space(Style::SP_M);
            ui.heading(
                RichText::new("MCNF · Mesh")
                    .color(Style::TEXT)
                    .size(Style::HEADING),
            );
        });
        ui.add_space(Style::SP_S);

        self.show_pip(ui);

        ui.add_space(Style::SP_M);
        ui.separator();
        ui.add_space(Style::SP_M);

        self.show_dnd(ui);
    }

    /// The worst-of mesh-health pip: a coloured dot (pulsing while degraded — to
    /// draw the eye to a problem — or while connecting; steady, zero-CPU idle,
    /// when all-healthy) + a status line + an inline `healthy/total` count, with a
    /// hover tooltip. Amber "Connecting…" until the first snapshot lands; then a
    /// dim "no lighthouses" line (no dot) if the snapshot in hand names none.
    fn show_pip(&self, ui: &mut egui::Ui) {
        let pip = self.model.pip();
        let (label, label_color) = pip.label();
        let tooltip = self.model.pip_tooltip();

        let resp = ui
            .horizontal(|ui| {
                ui.add_space(Style::SP_M);
                if let Some(base) = pip.dot_color() {
                    let color = if pip.pulses() {
                        // Keep the pulse animating while degraded or connecting.
                        ui.ctx().request_repaint();
                        let t = ui.input(|i| i.time);
                        let pulse = 0.5 - 0.5 * ((t / PIP_PULSE_SECS) * TAU).cos();
                        base.gamma_multiply(0.55 + 0.45 * pulse as f32)
                    } else {
                        base
                    };
                    ui.label(RichText::new("\u{25CF}").color(color).size(Style::BODY));
                    ui.add_space(Style::SP_XS);
                }
                let r = ui.label(RichText::new(label).color(label_color).size(Style::BODY));
                let (healthy, total) = self.model.counts();
                if total > 0 {
                    ui.add_space(Style::SP_S);
                    ui.label(
                        RichText::new(format!("{healthy}/{total} up"))
                            .color(Style::TEXT_DIM)
                            .size(Style::SMALL),
                    );
                }
                r
            })
            .inner;

        if let Some(tip) = tooltip {
            let _ = resp.on_hover_text(tip);
        }
    }

    /// The Do-Not-Disturb quick action: a real toggle (accent-selected when on)
    /// + a status line + any inline write error.
    fn show_dnd(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add_space(Style::SP_M);
            ui.label(
                RichText::new("Quick action")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
        });
        ui.add_space(Style::SP_XS);

        let active = self.model.dnd_active();
        let clicked = ui
            .horizontal(|ui| {
                ui.add_space(Style::SP_M);
                ui.selectable_label(active, RichText::new(DND_LABEL).size(Style::BODY))
                    .clicked()
            })
            .inner;

        ui.add_space(Style::SP_XS);
        ui.horizontal(|ui| {
            ui.add_space(Style::SP_M);
            let color = if active {
                Style::ACCENT
            } else {
                Style::TEXT_DIM
            };
            ui.label(
                RichText::new(self.model.dnd_status())
                    .color(color)
                    .size(Style::SMALL),
            );
        });

        if let Some(err) = &self.last_error {
            ui.add_space(Style::SP_XS);
            ui.horizontal(|ui| {
                ui.add_space(Style::SP_M);
                ui.label(
                    RichText::new(err.as_str())
                        .color(Style::DANGER)
                        .size(Style::SMALL),
                );
            });
        }

        if clicked {
            self.toggle_dnd();
        }
    }
}

impl eframe::App for Panel {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.last_poll.elapsed() >= REFRESH {
            self.poll();
            self.last_poll = Instant::now();
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(Style::SP_M);
            self.show(ui);
        });

        // Keep the poll cadence alive even with no input.
        ctx.request_repaint_after(REFRESH);
    }
}

/// Read the mesh-status snapshot, or `None` when there is nothing to read yet:
/// the file is absent (a fresh boot, before the root timer's first write to the
/// tmpfs `/run`), unreadable, or empty. The model then shows the honest
/// **connecting** pip rather than a misleading "no lighthouses", and never panics.
fn read_snapshot() -> Option<String> {
    match std::fs::read_to_string(SNAPSHOT_PATH) {
        Ok(s) if !s.trim().is_empty() => Some(s),
        _ => None,
    }
}

/// Load the mesh-wide DND state from the bus root (DND off when the dir doesn't
/// resolve or the file is absent/unreadable — the safe default).
fn load_dnd(bus_root: Option<&Path>) -> dnd::DndState {
    bus_root.map_or_else(dnd::DndState::default, dnd::load_default)
}

/// The local peer name stamped onto a DND toggle: `$HOSTNAME` → `/etc/hostname`
/// → `"localhost"`.
fn local_peer() -> String {
    if let Ok(h) = std::env::var("HOSTNAME") {
        let h = h.trim();
        if !h.is_empty() {
            return h.to_string();
        }
    }
    if let Ok(h) = std::fs::read_to_string("/etc/hostname") {
        let h = h.trim();
        if !h.is_empty() {
            return h.to_string();
        }
    }
    "localhost".to_string()
}

/// Wall-clock milliseconds since the Unix epoch (`0` if the clock is before the
/// epoch; saturated rather than panicking on an impossibly-large value).
fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn main() -> eframe::Result<()> {
    run_client(APP_ID, "MCNF Panel", |_cc| Panel::new())
}

#[cfg(test)]
mod tests {
    use super::{load_dnd, now_unix_ms};

    #[test]
    fn now_unix_ms_is_positive() {
        // The build host's clock is well past the epoch.
        assert!(now_unix_ms() > 1_600_000_000_000);
    }

    #[test]
    fn load_dnd_without_a_bus_root_is_off() {
        // No bus dir → the safe default (DND off), never a panic.
        assert!(!load_dnd(None).active);
    }
}
