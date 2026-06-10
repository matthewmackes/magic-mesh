//! v2.0.0 Phase A.3 (locked 2026-05-19) — DBus surface served by
//! `mackesd` (and one cross-process consumer at `mackes-session`).
//!
//! Five services live on the session bus:
//!
//! | Object path                | Interface                          | Owner       |
//! |----------------------------|------------------------------------|-------------|
//! | `/org/mackes/Shell`        | `org.mackes.Shell`                 | mackesd     |
//! | `/org/mackes/Settings`     | `org.mackes.Settings`              | mackesd     |
//! | `/org/freedesktop/Notifications` | `org.freedesktop.Notifications` | mackesd     |
//! | `/org/mackes/Session`      | `org.mackes.Session`               | mackes-session |
//! | `/org/mackes/Fleet`        | `org.mackes.Fleet`                 | mackesd     |
//!
//! Phase A scaffolded the service structs with `#[interface]`
//! decoration in place; Phase B + C filled in the handler bodies,
//! so the historical `UNIMPLEMENTED` placeholder has been retired
//! and every dispatch path returns either a real value or `()`.
//!
//! `Notifications` deliberately matches the spec object path
//! `/org/freedesktop/Notifications` so existing apps (notify-send,
//! libnotify, etc.) reach mackesd transparently.

#![cfg(feature = "async-services")]
// zbus's #[interface] macro expands to additional dispatch methods
// that don't carry doc comments; the workspace's #[warn(missing_docs)]
// would otherwise flag every one. Silence at the module level so the
// rest of the crate's missing_docs hygiene stays loud.
#![allow(missing_docs)]

pub mod bus_bridge;
pub mod directory;
pub mod files;
pub mod fleet;
pub mod jobs;
// NF-Bundle-0 (v2.5) — dev.mackes.MDE.Nebula.Status surface.
// Foundation that NF-10..NF-18 desktop consumers chain on.
// Reachable from run_serve at boot.
pub mod nebula;
pub mod notifications;
// E4.20 (2026-06-04): the `portal` Bus-publish client (action/shell/<verb>) was
// retired with mde-portal — its only caller (alert_relay's CRITICAL goto) is
// redundant with the notify-send → notifyd → Action Center path.
// DBUS-1 (2026-05-30): the `session` D-Bus interface module was retired
// — the session lifecycle surface migrated to Bus `action/session/<verb>`
// in `crates/mde-session/src/session.rs` (Q96 / EPIC-RETIRE-DBUS). The
// `mackesd` placeholder (never served) + its dead `org.mackes.session`
// name + `/org/mackes/Session` path were removed with it.
pub mod settings;
pub mod shell;

/// Convenience: the well-known bus name mackesd registers on the
/// session bus.
pub const MACKESD_BUS_NAME: &str = "org.mackes.mackesd";

/// Convenience: the canonical object path for each service.
pub mod paths {
    /// `/org/mackes/Shell`
    pub const SHELL: &str = "/org/mackes/Shell";
    /// `/org/mackes/Settings`
    pub const SETTINGS: &str = "/org/mackes/Settings";
    /// `/org/freedesktop/Notifications` — matches the freedesktop
    /// spec so libnotify clients work unchanged.
    pub const NOTIFICATIONS: &str = "/org/freedesktop/Notifications";
    /// `/org/mackes/Fleet`
    pub const FLEET: &str = "/org/mackes/Fleet";
    /// `/dev/mackes/MDE/Portal` — v6.0 Portal-1.
    pub const PORTAL: &str = "/dev/mackes/MDE/Portal";
}
