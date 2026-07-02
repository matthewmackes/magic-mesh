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
//!   peripheral batteries) over the system D-Bus `ObjectManager`.
//! - [`UPowerClient`] — battery enumeration (internal, UPS, BT peripherals).
//! - [`LogindClient`] — session lock + suspend/reboot/poweroff verbs and their
//!   `CanX` availability probe (confirm-gating is the caller's duty, lock 12).
//! - [`DisplayProber`] — a **read-only** DRM connector/mode probe (the modeset
//!   drive itself stays in the `mde-egui` DRM runner; multi-CRTC is E12-18).
//! - [`BacklightClient`] — sysfs backlight enumeration + brightness write.
//! - [`MixerClient`] / [`DdcClient`] — the `PipeWire` graph and DDC/CI clients.
//!   Their real bindings are E12-16 / E12-18; until then the bound impls answer
//!   with a typed [`SeatError::Unavailable`] — an honest probe state, never a
//!   stub that lies (§7).
//!
//! [`Seat::snapshot`] folds every client into a [`SeatSnapshot`] of typed
//! [`Probe`] states — the one model the shell's System surface and chrome status
//! icons render from. The fixed compiled-in hotkey table (lock 9) lives in
//! [`hotkeys`]; its dispatch is E12-19's work.

mod backlight;
mod bluez;
mod bus;
mod ddc;
mod display;
mod error;
pub mod hotkeys;
mod logind;
mod mixer;
mod props;
mod snapshot;
mod upower;

pub use backlight::{Backlight, BacklightClient, SysfsBacklight};
pub use bluez::{BluezClient, BtAdapter, BtDevice, BtStatus, ZbusBluez};
pub use ddc::{DdcClient, DdcDisplay, UnboundDdc};
pub use display::{Connector, ConnectorStatus, DisplayMode, DisplayProber, DrmProber};
pub use error::{Backend, SeatError};
pub use hotkeys::{Hotkey, HotkeyAction, HOTKEYS};
pub use logind::{Avail, LogindClient, PowerCaps, PowerVerb, ZbusLogind};
pub use mixer::{MixerClient, MixerStatus, MixerStrip, UnboundMixer};
pub use snapshot::{Probe, Seat, SeatSnapshot};
pub use upower::{Battery, BatteryKind, BatteryState, UPowerClient, ZbusUPower};
