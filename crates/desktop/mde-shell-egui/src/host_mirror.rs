//! `host_mirror` — the shell side of the E12-19 `host_state` mesh mirror (design
//! `docs/design/quasar-host-controls.md`, lock 1).
//!
//! Lock 1 **splits** host-control ownership: the shell drives the seat hardware
//! in-process ([`crate::system`]), and a thin `mackesd` `host_state` worker mirrors
//! this node's snapshot mesh-wide so every peer's Workbench sees it. The worker can
//! only mirror what the shell publishes — this module is that publisher: each pump
//! it folds the live [`SeatSnapshot`] into the mirror's JSON shape and writes it to
//! the node-local [`LOCAL_SNAPSHOT_TOPIC`], which the worker republishes to the
//! replicated `state/host/<node>/seat` topic.
//!
//! **§6 boundary:** the shell (desktop tier) must not depend on `mackesd` (mesh
//! tier), so this mirrors the worker's `SeatMirror` **JSON contract** with local
//! serde structs — the same JSON-boundary the toast / clipboard / chat lanes use.
//! The field names here (`volume` / `muted` / `bluetooth_powered` / `displays[]{id,
//! enabled}` / `batteries[]`) are the wire contract the worker's `SeatMirror`
//! deserializes; a round-trip test pins the shape.
//!
//! Only the *reading* is published: a present section maps to its live value, an
//! absent one (no `PipeWire` / no adapter / headless) to its honest default (muted,
//! radio off, no displays, no batteries) — never a fabricated control (§7).

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_seat::{ConnectorStatus, Probe, SeatSnapshot};
use serde::Serialize;
use std::path::PathBuf;

/// The node-local topic the shell publishes its seat snapshot on. The `host_state`
/// worker reads it and republishes to the replicated `state/host/<node>/seat`
/// mirror. Kept in lockstep with `mackesd_core::workers::host_state`'s constant of
/// the same value (asserted by the worker's tests against this JSON contract).
const LOCAL_SNAPSHOT_TOPIC: &str = "state/host/local/seat";

/// One display in the mirror — id + whether it is lit (the worker's last-console
/// interlock reads this). Serializes to `{"id":…,"enabled":…}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct MirrorDisplay {
    /// The connector name (`eDP-1`, `HDMI-A-1`, …) — the display verb's target id.
    id: String,
    /// Whether a display is currently attached/lit on this connector.
    enabled: bool,
}

/// The JSON seat snapshot the shell mirrors — the subset the mesh views + the
/// worker's interlocks need. Mirrors `mackesd`'s `SeatMirror` wire shape (§6).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
struct HostMirror {
    /// Master output volume 0–100 (0 when the mixer is absent).
    volume: u8,
    /// Master output muted (defaults muted-off is a present reading; an absent
    /// mixer reports `false` — no fabricated level).
    muted: bool,
    /// Whether any Bluetooth adapter radio is powered.
    bluetooth_powered: bool,
    /// Displays (id + lit) for the never-black-the-last-console guard.
    displays: Vec<MirrorDisplay>,
    /// Battery percentages (system + BT-peripheral) for the remote Power view.
    batteries: Vec<u8>,
}

/// Round a `UPower` percentage (0–100 f64) into the mirror's `u8` domain, clamping
/// out-of-range readings. The cast is safe by construction (the value is clamped to
/// `0.0..=100.0` first), so the truncation/sign lints don't apply.
// Safe by construction: the value is clamped to 0..=100 before the cast.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn clamp_pct(percentage: f64) -> u8 {
    percentage.round().clamp(0.0, 100.0) as u8
}

/// Fold a live [`SeatSnapshot`] into the mirror's wire shape. Pure — a present
/// section maps to its reading, an absent one to its honest default (§7). Unit-
/// tested against fabricated snapshots and the real wire contract.
fn to_mirror(snap: &SeatSnapshot) -> HostMirror {
    let (volume, muted) = snap
        .mixer
        .present()
        .map_or((0, false), |m| (m.master.volume, m.master.muted));
    let bluetooth_powered = snap
        .bluetooth
        .present()
        .is_some_and(|bt| bt.adapters.iter().any(|a| a.powered));
    let displays = snap
        .displays
        .present()
        .map(|conns| {
            conns
                .iter()
                .map(|c| MirrorDisplay {
                    id: c.name.clone(),
                    enabled: matches!(c.status, ConnectorStatus::Connected),
                })
                .collect()
        })
        .unwrap_or_default();

    // System + UPS + internal batteries from UPower, plus any BT peripheral that
    // reports its own charge — both surface in the remote Power view.
    let mut batteries: Vec<u8> = Vec::new();
    if let Probe::Present(cells) = &snap.batteries {
        batteries.extend(cells.iter().map(|b| clamp_pct(b.percentage)));
    }
    if let Some(bt) = snap.bluetooth.present() {
        batteries.extend(bt.devices.iter().filter_map(|d| d.battery_percent));
    }

    HostMirror {
        volume,
        muted,
        bluetooth_powered,
        displays,
        batteries,
    }
}

/// Publishes the shell's seat snapshot to the node-local mirror topic each pump.
///
/// Holds only the Bus root; a publish opens the spool, writes the folded JSON, and
/// drops it (the same cheap open-write the other shell lanes use). A missing Bus
/// root (no `$XDG_DATA_HOME`, no system spool) is honest silence — the mirror stays
/// dark rather than faking a publish.
pub(crate) struct HostMirrorPublisher {
    bus_root: Option<PathBuf>,
}

impl Default for HostMirrorPublisher {
    fn default() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
        }
    }
}

impl HostMirrorPublisher {
    /// Fold the snapshot and write it to [`LOCAL_SNAPSHOT_TOPIC`] (the worker mirrors
    /// the newest one mesh-wide). A serialize/spool failure is dropped — a dark
    /// mirror never wedges the shell's pump (the same fail-soft `let _ =` publish the
    /// Chat / Storage lanes use).
    pub(crate) fn publish(&self, snap: &SeatSnapshot) {
        let Some(root) = self.bus_root.clone() else {
            return;
        };
        let mirror = to_mirror(snap);
        let Ok(body) = serde_json::to_string(&mirror) else {
            return;
        };
        // arch-11: best-effort writer — kept on Persist::open (the shared
        // BusReader seam is read-only).
        let _ = Persist::open(root)
            .and_then(|p| p.write(LOCAL_SNAPSHOT_TOPIC, Priority::Default, None, Some(&body)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_seat::Seat;

    /// A snapshot from the real headless seat: every D-Bus/DRM/PipeWire backend is
    /// legitimately Absent, so the fold must produce the honest all-default mirror —
    /// never panic, never a fabricated reading (§7).
    #[test]
    fn a_headless_snapshot_folds_to_the_honest_default_mirror() {
        let snap = Seat::new().snapshot();
        let m = to_mirror(&snap);
        // Absent mixer → 0 / not-muted; absent adapter → radio off; no displays.
        assert_eq!(m.volume, 0);
        assert!(!m.muted);
        assert!(!m.bluetooth_powered);
        assert!(m.displays.is_empty());
        assert!(m.batteries.is_empty());
    }

    /// The wire contract: the JSON the shell emits carries exactly the field names
    /// (and the display sub-shape) the worker's `SeatMirror` deserializes. This pins
    /// the §6 boundary — a rename here would silently blind every peer's mirror.
    #[test]
    fn the_mirror_serializes_the_worker_wire_contract() {
        let m = HostMirror {
            volume: 55,
            muted: true,
            bluetooth_powered: true,
            displays: vec![
                MirrorDisplay {
                    id: "eDP-1".into(),
                    enabled: true,
                },
                MirrorDisplay {
                    id: "HDMI-A-1".into(),
                    enabled: false,
                },
            ],
            batteries: vec![88, 42],
        };
        let json = serde_json::to_string(&m).expect("serializes");
        // Every wire key the worker's SeatMirror reads is present, named exactly.
        assert!(json.contains("\"volume\":55"), "{json}");
        assert!(json.contains("\"muted\":true"), "{json}");
        assert!(json.contains("\"bluetooth_powered\":true"), "{json}");
        assert!(json.contains("\"id\":\"eDP-1\""), "{json}");
        assert!(json.contains("\"enabled\":true"), "{json}");
        assert!(json.contains("\"enabled\":false"), "{json}");
        assert!(json.contains("\"batteries\":[88,42]"), "{json}");
    }

    /// The publish path is inert without a Bus root — honest silence, no panic (a
    /// headless CI host with no spool must not wedge the pump).
    #[test]
    fn publish_without_a_bus_root_is_a_silent_no_op() {
        let pubr = HostMirrorPublisher { bus_root: None };
        // No spool, no panic — the mirror simply stays dark.
        pubr.publish(&Seat::new().snapshot());
    }

    /// Round-trip the publish over a real temp Bus and read the newest message back:
    /// the folded mirror lands on the exact topic the worker drains.
    #[test]
    fn publish_writes_the_folded_mirror_to_the_local_topic() {
        let dir = std::env::temp_dir().join(format!(
            "host_mirror_pub_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let pubr = HostMirrorPublisher {
            bus_root: Some(dir.clone()),
        };
        pubr.publish(&Seat::new().snapshot());

        let persist = Persist::open(dir).expect("open temp bus");
        let latest = persist
            .list_since(LOCAL_SNAPSHOT_TOPIC, None)
            .expect("list")
            .into_iter()
            .next_back()
            .and_then(|m| m.body)
            .expect("a published mirror");
        // A well-formed mirror body the worker's SeatMirror will accept.
        assert!(latest.contains("\"volume\":"), "{latest}");
        assert!(latest.contains("\"displays\":"), "{latest}");
    }
}
