//! The typed error surface every seat client speaks — which backend failed, and
//! whether it is *absent* (the honest "not available on this host" state the UI
//! renders, §7) or *present but failing*.

use std::path::PathBuf;

use thiserror::Error;

/// The seat backends this crate fronts. Every [`SeatError`] carries one, so a
/// probe failure folds into the right section's typed not-available state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// The PipeWire audio graph (the mixer). Real binding: E12-16.
    PipeWire,
    /// BlueZ over the system D-Bus.
    Bluetooth,
    /// UPower over the system D-Bus.
    UPower,
    /// systemd-logind over the system D-Bus.
    Logind,
    /// The DRM/KMS connector probe (`/dev/dri`).
    Display,
    /// The sysfs backlight class (`/sys/class/backlight`).
    Backlight,
    /// DDC/CI monitor control over i2c-dev. Real binding: E12-18.
    Ddc,
}

impl Backend {
    /// The operator-facing backend name.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Backend::PipeWire => "PipeWire",
            Backend::Bluetooth => "Bluetooth (BlueZ)",
            Backend::UPower => "UPower",
            Backend::Logind => "logind",
            Backend::Display => "DRM display",
            Backend::Backlight => "backlight",
            Backend::Ddc => "DDC/CI",
        }
    }
}

impl std::fmt::Display for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Why a seat client call failed. `Unavailable` is the load-bearing variant: it
/// is the *typed* "this backend does not exist here" state the System surface
/// and chrome icons render honestly instead of a fake control (§7 / interlock 4).
#[derive(Debug, Error)]
pub enum SeatError {
    /// The backend is not present on this host (service not running, no device
    /// node, no client bound yet). The honest render is "not available".
    #[error("{backend} is not available: {reason}")]
    Unavailable {
        /// Which backend is absent.
        backend: Backend,
        /// Why the probe concluded absence (bus unreachable, service unknown…).
        reason: String,
    },
    /// The backend is present but a call to it failed.
    #[error("{backend} call failed: {reason}")]
    Backend {
        /// Which backend failed.
        backend: Backend,
        /// The failing call + the backend's error.
        reason: String,
    },
    /// A device-node / sysfs I/O failure.
    #[error("{backend} I/O on {}: {source}", path.display())]
    Io {
        /// Which backend was being read/written.
        backend: Backend,
        /// The path the I/O touched.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The backend answered with something this client cannot fold.
    #[error("{backend} reply malformed: {reason}")]
    Protocol {
        /// Which backend answered.
        backend: Backend,
        /// What was malformed.
        reason: String,
    },
    /// A control write outside the device's advertised range (e.g. a brightness
    /// above `max_brightness`) — refused typed, never clamped silently.
    #[error("{backend}: {value} is out of range (device max {max})")]
    OutOfRange {
        /// Which backend refused the write.
        backend: Backend,
        /// The refused value.
        value: u32,
        /// The device's advertised maximum.
        max: u32,
    },
}

impl SeatError {
    /// Which backend this error belongs to — the key that routes a failure into
    /// the right [`crate::Probe`] section.
    #[must_use]
    pub const fn backend(&self) -> Backend {
        match self {
            SeatError::Unavailable { backend, .. }
            | SeatError::Backend { backend, .. }
            | SeatError::Io { backend, .. }
            | SeatError::Protocol { backend, .. }
            | SeatError::OutOfRange { backend, .. } => *backend,
        }
    }
}

/// Is this D-Bus error name the "service is simply not running here" family?
/// Those map to [`SeatError::Unavailable`] (an absent backend), not a failure.
#[must_use]
pub(crate) fn unavailable_error_name(name: &str) -> bool {
    name.ends_with(".ServiceUnknown") || name.ends_with(".NameHasNoOwner")
}

/// Fold a zbus call error into a typed [`SeatError`]: a missing service / dead
/// bus is [`SeatError::Unavailable`] (the backend is absent), anything else is
/// [`SeatError::Backend`] (present but failing). `ctx` names the failing call.
pub(crate) fn classify_call(backend: Backend, ctx: &str, e: &zbus::Error) -> SeatError {
    match e {
        zbus::Error::MethodError(name, detail, _) => {
            if unavailable_error_name(name.as_str()) {
                SeatError::Unavailable {
                    backend,
                    reason: format!("{ctx}: {}", name.as_str()),
                }
            } else {
                let detail = detail.as_deref().unwrap_or("");
                SeatError::Backend {
                    backend,
                    reason: format!("{ctx}: {} {detail}", name.as_str()),
                }
            }
        }
        zbus::Error::FDO(fdo) => match fdo.as_ref() {
            zbus::fdo::Error::ServiceUnknown(m) | zbus::fdo::Error::NameHasNoOwner(m) => {
                SeatError::Unavailable {
                    backend,
                    reason: format!("{ctx}: {m}"),
                }
            }
            other => SeatError::Backend {
                backend,
                reason: format!("{ctx}: {other}"),
            },
        },
        zbus::Error::InputOutput(io) => SeatError::Unavailable {
            backend,
            reason: format!("{ctx}: bus I/O: {io}"),
        },
        other => SeatError::Backend {
            backend,
            reason: format!("{ctx}: {other}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_names_its_backend() {
        let cases: Vec<SeatError> = vec![
            SeatError::Unavailable {
                backend: Backend::PipeWire,
                reason: "x".into(),
            },
            SeatError::Backend {
                backend: Backend::Bluetooth,
                reason: "x".into(),
            },
            SeatError::Io {
                backend: Backend::Backlight,
                path: PathBuf::from("/sys/class/backlight/x/brightness"),
                source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"),
            },
            SeatError::Protocol {
                backend: Backend::UPower,
                reason: "x".into(),
            },
            SeatError::OutOfRange {
                backend: Backend::Backlight,
                value: 999,
                max: 100,
            },
        ];
        let want = [
            Backend::PipeWire,
            Backend::Bluetooth,
            Backend::Backlight,
            Backend::UPower,
            Backend::Backlight,
        ];
        for (e, b) in cases.iter().zip(want) {
            assert_eq!(e.backend(), b, "{e}");
            // Every rendered message carries the backend label — the operator
            // always learns WHICH subsystem spoke.
            assert!(e.to_string().contains(b.label()), "{e}");
        }
    }

    #[test]
    fn service_absence_names_classify_as_unavailable() {
        assert!(unavailable_error_name(
            "org.freedesktop.DBus.Error.ServiceUnknown"
        ));
        assert!(unavailable_error_name(
            "org.freedesktop.DBus.Error.NameHasNoOwner"
        ));
        assert!(!unavailable_error_name(
            "org.freedesktop.DBus.Error.AccessDenied"
        ));
        assert!(!unavailable_error_name("org.bluez.Error.Failed"));
    }

    #[test]
    fn dead_bus_and_unknown_service_fold_to_unavailable() {
        // A dead/unreachable bus socket is the headless-host case — Unavailable.
        let io = zbus::Error::InputOutput(std::sync::Arc::new(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no socket",
        )));
        let e = classify_call(Backend::Bluetooth, "GetManagedObjects", &io);
        assert!(matches!(e, SeatError::Unavailable { .. }), "{e}");

        // A named ServiceUnknown from the fdo layer is the "bluetoothd not
        // running" case — also Unavailable, not a failure.
        let fdo = zbus::Error::FDO(Box::new(zbus::fdo::Error::ServiceUnknown(
            "org.bluez not activatable".into(),
        )));
        let e = classify_call(Backend::Bluetooth, "GetManagedObjects", &fdo);
        assert!(matches!(e, SeatError::Unavailable { .. }), "{e}");

        // Any other fdo error is a real Backend failure.
        let denied = zbus::Error::FDO(Box::new(zbus::fdo::Error::AccessDenied("no".into())));
        let e = classify_call(Backend::Logind, "PowerOff", &denied);
        assert!(matches!(e, SeatError::Backend { .. }), "{e}");
    }
}
