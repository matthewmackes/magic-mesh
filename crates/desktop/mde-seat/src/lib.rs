//! `mde-seat` — the E12 **seat hardware-access library** (E12-15, Quasar host
//! controls; design `docs/design/quasar-host-controls.md`).
//!
//! The Quasar shell owns the DRM seat with **no compositor and no settings
//! daemon**, so every "system control" a desktop OS takes for granted — audio,
//! Bluetooth, displays, backlight, power — has no owner until this crate. It is
//! the ONE implementation of each protocol client (lock 1): the shell consumes it
//! directly (in-process, zero-latency), and the `mackesd` `host_state` mirror
//! worker (E12-19) consumes the *same crate* for its Bus mirror.
//!
//! Every client follows the mde-kvm `ChTransport` pattern: a narrow trait seam,
//! typed errors, pure folds unit-tested headless, and a production impl that does
//! only I/O. The seams:
//!
//! - [`BluezClient`] — `BlueZ` adapter/device enumeration (incl. `Battery1`
//!   peripheral batteries) over the system D-Bus `ObjectManager`, **plus the
//!   full pairing-manager verbs** (power/scan/pair/trust/connect/forget +
//!   seat-start auto-reconnect); [`ScanTracker`] folds a scan's polls into
//!   proximity-announce candidates and [`pairing::PairingAgent`] answers the
//!   PIN/passkey prompts (E12-17).
//! - [`UPowerClient`] — battery enumeration (internal, UPS, BT peripherals).
//! - [`LogindClient`] — session lock + suspend/reboot/poweroff verbs and their
//!   `CanX` availability probe (confirm-gating is the caller's duty, lock 12).
//! - [`DisplayProber`] — a **read-only** DRM connector/mode probe (the modeset
//!   drive itself stays in the `mde-egui` DRM runner; multi-CRTC is E12-18).
//! - [`BacklightClient`] — sysfs backlight enumeration + brightness write.
//! - [`MixerClient`] — the `PipeWire` graph client (E12-16).
//! - [`DdcClient`] — DDC/CI external-monitor brightness (E12-18), bound via a typed
//!   `ddcutil` runner ([`DdcCtl`]); a host without `ddcutil` or a monitor that
//!   rejects DDC answers a typed [`SeatError`] — an honest not-controllable state,
//!   never a dead slider (§7). [`UnboundDdc`] stays as the no-backend fallback.
//! - [`DisplayLayout`] — the desired multi-head arrangement (enable/mode/position,
//!   [`MonitorId`]-keyed) with the typed "never black the last console" guard
//!   (E12-18); the DRM drive itself is the `mde-egui` runner's multi-CRTC core.
//!
//! [`Seat::snapshot`] folds every client into a [`SeatSnapshot`] of typed
//! [`Probe`] states — the one model the shell's System surface and chrome status
//! icons render from. The fixed compiled-in hotkey table (lock 9) lives in
//! [`hotkeys`]; its dispatch is E12-19's work.

mod arrange;
mod backlight;
mod bluez;
mod bus;
mod charge_threshold;
mod ddc;
mod display;
mod error;
pub mod hotkeys;
mod lid;
mod logind;
mod mixer;
pub mod pairing;
mod powerprofiles;
mod props;
mod snapshot;
mod upower;

pub use arrange::{ArrangeError, DisplayLayout, MonitorId, OutputArrangement};
pub use backlight::{Backlight, BacklightClient, SysfsBacklight};
pub use bluez::{
    trusted_reconnect_targets, BluezClient, BtAdapter, BtDevice, BtStatus, ReconnectAttempt,
    ScanTracker, ZbusBluez,
};
pub use charge_threshold::{ChargeThresholdClient, SysfsChargeThreshold};
pub use ddc::{
    parse_detect, parse_getvcp_brightness, DdcClient, DdcCtl, DdcDisplay, DdcRunner, DdcUtil,
    UnboundDdc,
};
pub use display::{Connector, ConnectorStatus, DisplayMode, DisplayProber, DrmProber};
pub use error::{Backend, SeatError};
pub use hotkeys::{Hotkey, HotkeyAction, HOTKEYS};
pub use lid::{parse_lid_state, LidClient, LidState, ProcLid};
pub use logind::{Avail, LogindClient, PowerCaps, PowerVerb, ZbusLogind};
pub use mixer::{
    fold_graph, MixerClient, MixerStatus, MixerStrip, PwCli, PwGraph, PwRunner, StripOrigin,
    UnboundMixer,
};
pub use pairing::{
    resolve_confirm, resolve_passkey, resolve_pin, AgentPrompt, PairingAgent, PairingReply,
    PairingResponder, Refusal,
};
pub use powerprofiles::{fold_profiles, ProfileState, ProfilesClient, ZbusProfiles};
pub use snapshot::{Probe, Seat, SeatSnapshot};
pub use upower::{Battery, BatteryKind, BatteryState, UPowerClient, ZbusUPower};
