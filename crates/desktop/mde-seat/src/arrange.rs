//! The display **arrangement model** — the intent layer over the read-only
//! [`crate::display`] connector probe (lock 6 multi-head, lock 7 roaming prefs).
//!
//! The DRM *drive* (atomic modeset, CRTC assignment, scanout) lives in the
//! `mde-egui` runner (E12-18's multi-CRTC core); this crate owns the **desired
//! arrangement**: which outputs are enabled, at what mode, laid out at what
//! relative position. It is a pure, unit-tested state model the System panel edits
//! and the `host_state` worker (E12-19) mirrors — one model, two views (§6).
//!
//! Two locked invariants live here as *typed guards*, so both the local panel and
//! E12-19's remote verbs enforce the same rule:
//!
//! - **Never black the last console** (interlock 1): disabling the only enabled
//!   output is refused with a typed [`ArrangeError::WouldBlackLastConsole`], never
//!   a silent no-op that leaves a headless seat.
//! - **EDID-keyed identity** (lock 7): each output carries a [`MonitorId`] so an
//!   arrangement preference re-applies to the *same physical monitor* after a
//!   replug (and roams per-peer). Live persist/roam is integration-gated (it plugs
//!   into `session_roaming`); the *keying* is modeled + tested here.

use crate::display::{Connector, ConnectorStatus, DisplayMode};

/// A replug-stable monitor identity (lock 7). Derived from the monitor's EDID when
/// available (manufacturer + model + serial), else the DRM connector name as a
/// best-effort fallback — so an arrangement preference re-binds to the same panel
/// across a replug rather than to a fungible connector slot.
///
/// True EDID reading is the `session_roaming` integration seam (gated); this type
/// fixes the *key shape* now so the roaming layer only supplies the bytes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MonitorId(String);

impl MonitorId {
    /// An EDID-keyed id: `<mfg>:<model>:<serial>`, the stable cross-replug key.
    #[must_use]
    pub fn from_edid(mfg: &str, model: &str, serial: &str) -> Self {
        Self(format!("{mfg}:{model}:{serial}"))
    }

    /// The connector-name fallback id (`connector:<name>`), used until an EDID is
    /// read — stable within a session, not guaranteed across a replug into a
    /// different port. Tagged so it never collides with an EDID key.
    #[must_use]
    pub fn from_connector_name(name: &str) -> Self {
        Self(format!("connector:{name}"))
    }

    /// The raw key string (the mirror/persist form E12-19 roams).
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for MonitorId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One output's desired arrangement — enabled/mode/position keyed by its stable
/// [`MonitorId`] and pinned to the live connector it currently drives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputArrangement {
    /// The replug-stable monitor identity (the roaming key).
    pub id: MonitorId,
    /// The live DRM connector this monitor is on (`HDMI-A-1`), for the drive layer.
    pub connector: String,
    /// Whether a display is physically attached (drives which rows are actionable).
    pub connected: bool,
    /// Whether this output should be lit.
    pub enabled: bool,
    /// The desired mode (resolution/refresh); `None` ⇒ the connector's preferred.
    pub mode: Option<DisplayMode>,
    /// Relative position of this output's top-left in the virtual desktop (px).
    pub position: (i32, i32),
}

impl OutputArrangement {
    /// The effective mode this output would drive: the chosen mode, else the
    /// connector's own preferred/first mode when known.
    #[must_use]
    pub const fn effective_mode(&self) -> Option<DisplayMode> {
        self.mode
    }
}

/// Why an arrangement edit was refused (the typed interlocks — never a silent
/// no-op, so the panel and E12-19's remote verbs both surface the same reason).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ArrangeError {
    /// Disabling this output would leave no lit console (interlock 1).
    #[error("refused: '{0}' is the only lit output — disabling it would black the last console")]
    WouldBlackLastConsole(String),
    /// No output with this [`MonitorId`] is in the layout.
    #[error("no output {0} in the layout")]
    Unknown(MonitorId),
}

/// The whole desired arrangement: every known output, laid out. Built from the
/// read-only connector probe and edited by the System panel; the drive layer
/// (`mde-egui`) and the mirror worker (E12-19) both read it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DisplayLayout {
    /// Every output, in a stable render order (connected first, then by connector).
    pub outputs: Vec<OutputArrangement>,
}

impl DisplayLayout {
    /// Build the initial layout from the read-only connector probe: every connected
    /// connector is enabled at its preferred mode and auto-arranged left-to-right;
    /// disconnected connectors are carried disabled (so a replug re-enables the
    /// same slot). The [`MonitorId`] falls back to the connector name until the
    /// EDID-roaming layer supplies a real key.
    #[must_use]
    pub fn from_connectors(connectors: &[Connector]) -> Self {
        let mut outputs: Vec<OutputArrangement> = connectors
            .iter()
            .map(|c| {
                let connected = c.status == ConnectorStatus::Connected;
                OutputArrangement {
                    id: MonitorId::from_connector_name(&c.name),
                    connector: c.name.clone(),
                    connected,
                    enabled: connected,
                    mode: c.preferred_mode().copied(),
                    position: (0, 0),
                }
            })
            .collect();
        // Stable order: connected outputs first, then by connector name.
        outputs.sort_by(|a, b| {
            b.connected
                .cmp(&a.connected)
                .then_with(|| a.connector.cmp(&b.connector))
        });
        let mut layout = Self { outputs };
        layout.auto_arrange();
        layout
    }

    /// How many outputs are currently lit — the count the last-console guard reads.
    #[must_use]
    pub fn active_count(&self) -> usize {
        self.outputs.iter().filter(|o| o.enabled).count()
    }

    /// The output with this id, if present.
    #[must_use]
    pub fn get(&self, id: &MonitorId) -> Option<&OutputArrangement> {
        self.outputs.iter().find(|o| &o.id == id)
    }

    /// The typed last-console guard (interlock 1), standalone so E12-19's remote
    /// display verbs enforce the exact same rule before disabling a peer's output.
    ///
    /// # Errors
    /// [`ArrangeError::Unknown`] if `id` isn't in the layout;
    /// [`ArrangeError::WouldBlackLastConsole`] if it is the only lit output.
    pub fn guard_disable(&self, id: &MonitorId) -> Result<(), ArrangeError> {
        let out = self
            .get(id)
            .ok_or_else(|| ArrangeError::Unknown(id.clone()))?;
        if out.enabled && self.active_count() <= 1 {
            return Err(ArrangeError::WouldBlackLastConsole(out.connector.clone()));
        }
        Ok(())
    }

    /// Enable or disable an output. Enabling reflows the arrangement; disabling is
    /// gated by [`Self::guard_disable`] (never black the last console).
    ///
    /// # Errors
    /// The [`Self::guard_disable`] errors when disabling would leave no lit output.
    pub fn set_enabled(&mut self, id: &MonitorId, enabled: bool) -> Result<(), ArrangeError> {
        if !enabled {
            self.guard_disable(id)?;
        }
        let out = self
            .outputs
            .iter_mut()
            .find(|o| &o.id == id)
            .ok_or_else(|| ArrangeError::Unknown(id.clone()))?;
        out.enabled = enabled;
        self.auto_arrange();
        Ok(())
    }

    /// Choose an output's mode; reflows so downstream outputs shift by the new width.
    ///
    /// # Errors
    /// [`ArrangeError::Unknown`] if `id` isn't in the layout.
    pub fn set_mode(&mut self, id: &MonitorId, mode: DisplayMode) -> Result<(), ArrangeError> {
        let out = self
            .outputs
            .iter_mut()
            .find(|o| &o.id == id)
            .ok_or_else(|| ArrangeError::Unknown(id.clone()))?;
        out.mode = Some(mode);
        self.auto_arrange();
        Ok(())
    }

    /// Reorder an enabled output one slot left/right in the left-to-right row, then
    /// reflow positions. A no-op at the ends. Returns whether anything moved.
    pub fn nudge(&mut self, id: &MonitorId, left: bool) -> bool {
        let Some(pos) = self.outputs.iter().position(|o| &o.id == id) else {
            return false;
        };
        let swap = if left {
            pos.checked_sub(1)
        } else {
            (pos + 1 < self.outputs.len()).then_some(pos + 1)
        };
        let Some(swap) = swap else { return false };
        self.outputs.swap(pos, swap);
        self.auto_arrange();
        true
    }

    /// Reflow enabled outputs into a left-to-right row (each abutting the previous),
    /// in current list order. Disabled outputs collapse to the origin. This is the
    /// v1 relative-arrangement policy (single-row); free 2-D placement is a later
    /// refinement the model already has room for (`position` is a full `(i32,i32)`).
    pub fn auto_arrange(&mut self) {
        let mut x = 0_i32;
        for out in &mut self.outputs {
            if out.enabled {
                out.position = (x, 0);
                let w = out.mode.map_or(0, |m| i32::from(m.width));
                x += w;
            } else {
                out.position = (0, 0);
            }
        }
    }

    /// The virtual-desktop span (total width, max height) the drive layer must
    /// allocate a framebuffer for. Zero when nothing is lit.
    #[must_use]
    pub fn virtual_span(&self) -> (u32, u32) {
        let mut w = 0_u32;
        let mut h = 0_u32;
        for out in self.outputs.iter().filter(|o| o.enabled) {
            if let Some(m) = out.mode {
                w = w.max(u32::try_from(out.position.0).unwrap_or(0) + u32::from(m.width));
                h = h.max(u32::from(m.height));
            }
        }
        (w, h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::{Connector, ConnectorStatus, DisplayMode};

    fn mode(w: u16, h: u16, pref: bool) -> DisplayMode {
        DisplayMode {
            width: w,
            height: h,
            refresh_hz: 60,
            preferred: pref,
        }
    }

    fn connected(name: &str, w: u16, h: u16) -> Connector {
        Connector {
            name: name.to_owned(),
            status: ConnectorStatus::Connected,
            size_mm: Some((600, 340)),
            modes: vec![mode(w, h, true), mode(1024, 768, false)],
        }
    }

    #[test]
    fn monitor_id_edid_and_connector_keys_never_collide() {
        let edid = MonitorId::from_edid("DEL", "U2415", "7MT01");
        let conn = MonitorId::from_connector_name("DP-1");
        assert_ne!(edid, conn);
        assert_eq!(edid.as_str(), "DEL:U2415:7MT01");
        assert!(conn.as_str().starts_with("connector:"));
    }

    #[test]
    fn builds_a_left_to_right_layout_from_two_connected_monitors() {
        let layout = DisplayLayout::from_connectors(&[
            connected("HDMI-A-1", 1920, 1080),
            connected("DP-2", 2560, 1440),
        ]);
        assert_eq!(layout.outputs.len(), 2);
        assert_eq!(layout.active_count(), 2);
        // Sorted by connector name: HDMI-A-1 at origin, DP-2… wait, DP < HDMI.
        assert_eq!(layout.outputs[0].connector, "DP-2");
        assert_eq!(layout.outputs[0].position, (0, 0));
        // The second output abuts the first at its width (2560).
        assert_eq!(layout.outputs[1].connector, "HDMI-A-1");
        assert_eq!(layout.outputs[1].position, (2560, 0));
        // Virtual span is the sum of widths, tallest height.
        assert_eq!(layout.virtual_span(), (2560 + 1920, 1440));
    }

    #[test]
    fn a_disconnected_connector_is_carried_disabled_for_a_later_replug() {
        let layout = DisplayLayout::from_connectors(&[
            connected("eDP-1", 1920, 1080),
            Connector {
                name: "HDMI-A-1".to_owned(),
                status: ConnectorStatus::Disconnected,
                size_mm: None,
                modes: vec![],
            },
        ]);
        // Connected first, then the disconnected slot (disabled, no mode).
        assert!(layout.outputs[0].enabled);
        assert!(!layout.outputs[1].enabled);
        assert_eq!(layout.active_count(), 1);
    }

    #[test]
    fn disabling_the_only_lit_output_is_refused_typed_never_blacks_the_console() {
        let mut layout = DisplayLayout::from_connectors(&[connected("eDP-1", 1920, 1080)]);
        let id = layout.outputs[0].id.clone();
        let e = layout
            .set_enabled(&id, false)
            .expect_err("the last console must not go dark");
        assert!(matches!(e, ArrangeError::WouldBlackLastConsole(_)), "{e}");
        // The output is still enabled — the refusal was not a silent partial edit.
        assert!(layout.outputs[0].enabled);
        assert_eq!(layout.active_count(), 1);
    }

    #[test]
    fn disabling_one_of_two_is_allowed_and_reflows() {
        let mut layout = DisplayLayout::from_connectors(&[
            connected("DP-1", 1920, 1080),
            connected("DP-2", 1920, 1080),
        ]);
        let second = layout.outputs[1].id.clone();
        layout
            .set_enabled(&second, false)
            .expect("two → one is fine");
        assert_eq!(layout.active_count(), 1);
        // The remaining lit output sits at the origin after the reflow.
        assert_eq!(layout.outputs[0].position, (0, 0));
        // And now IT is the last console — refused.
        let first = layout.outputs[0].id.clone();
        assert!(layout.set_enabled(&first, false).is_err());
    }

    #[test]
    fn choosing_a_mode_reflows_downstream_positions() {
        let mut layout = DisplayLayout::from_connectors(&[
            connected("DP-1", 2560, 1440),
            connected("DP-2", 1920, 1080),
        ]);
        let first = layout.outputs[0].id.clone();
        // Shrink the first output → the second shifts left to abut the new width.
        layout.set_mode(&first, mode(1280, 720, false)).unwrap();
        assert_eq!(layout.outputs[1].position, (1280, 0));
    }

    #[test]
    fn nudge_reorders_the_row_and_no_ops_at_the_ends() {
        let mut layout = DisplayLayout::from_connectors(&[
            connected("DP-1", 1000, 1000),
            connected("DP-2", 1000, 1000),
        ]);
        let first = layout.outputs[0].id.clone();
        assert!(!layout.nudge(&first, true), "already leftmost — no move");
        assert!(layout.nudge(&first, false), "moves right past DP-2");
        assert_eq!(layout.outputs[1].connector, "DP-1");
    }

    #[test]
    fn guard_disable_on_an_unknown_id_is_typed_not_a_panic() {
        let layout = DisplayLayout::from_connectors(&[connected("DP-1", 1920, 1080)]);
        let e = layout
            .guard_disable(&MonitorId::from_connector_name("ghost"))
            .expect_err("unknown id is typed");
        assert!(matches!(e, ArrangeError::Unknown(_)), "{e}");
    }
}
