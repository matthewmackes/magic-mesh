//! Typed systemd service lifecycle control for the This Node Actions surface.
//!
//! Only provider-reported failed service unit names may reach this seam from
//! the shell. The client still validates the unit name and asks systemd to
//! resolve and restart it over D-Bus; it never shells out to `systemctl`.

use crate::bus::SysBus;
use crate::error::{Backend, SeatError};
use zbus::zvariant::OwnedObjectPath;

const SYSTEMD: &str = "org.freedesktop.systemd1";
const MANAGER: &str = "/org/freedesktop/systemd1";
const MANAGER_IFACE: &str = "org.freedesktop.systemd1.Manager";
const UNIT_IFACE: &str = "org.freedesktop.systemd1.Unit";

/// Reject shell-shaped or ambiguous targets before they reach systemd.
#[must_use]
pub fn safe_service_unit(unit: &str) -> bool {
    !unit.is_empty()
        && unit.len() <= 128
        && unit.ends_with(".service")
        && !unit.starts_with('-')
        && unit
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'@'))
}

/// Typed service lifecycle seam; tests can inject a fake without a system bus.
pub trait ServiceClient: Send {
    /// Restart one validated service unit after the caller's confirmation.
    fn restart(&self, unit: &str) -> Result<(), SeatError>;
}

/// Production systemd D-Bus service client.
pub struct ZbusService {
    bus: SysBus,
}

impl ZbusService {
    /// Construct a lazy system-bus client.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            bus: SysBus::new(Backend::SystemdService),
        }
    }
}

impl Default for ZbusService {
    fn default() -> Self {
        Self::new()
    }
}

impl ServiceClient for ZbusService {
    fn restart(&self, unit: &str) -> Result<(), SeatError> {
        if !safe_service_unit(unit) {
            return Err(SeatError::Protocol {
                backend: Backend::SystemdService,
                reason: "service unit target is malformed or not a .service unit".to_owned(),
            });
        }
        let unit_path: OwnedObjectPath =
            self.bus
                .call(SYSTEMD, MANAGER, MANAGER_IFACE, "GetUnit", &(unit,))?;
        self.bus.call_unit(
            SYSTEMD,
            unit_path.as_str(),
            UNIT_IFACE,
            "Restart",
            &("replace",),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_targets_are_bounded_and_not_shell_fragments() {
        assert!(safe_service_unit("mde-shell-egui.service"));
        assert!(safe_service_unit("mde@seat.service"));
        assert!(!safe_service_unit("mde-shell-egui"));
        assert!(!safe_service_unit("mde.service; reboot"));
        assert!(!safe_service_unit("../../reboot.service"));
        assert!(!safe_service_unit("-evil.service"));
    }
}
