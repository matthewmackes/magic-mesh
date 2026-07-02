//! The sysfs backlight client — panel brightness over `/sys/class/backlight`
//! (lock 13, the internal-panel half; DDC/CI external monitors are [`crate::ddc`]).
//!
//! Each backlight device exposes `brightness` (current) and `max_brightness`
//! (the ceiling); a write outside `0..=max` is refused typed
//! ([`SeatError::OutOfRange`]), never clamped silently. A host with no backlight
//! class (a desktop with only external monitors) answers
//! [`SeatError::Unavailable`] — an honest "no panel here", never a fake slider.

use std::path::{Path, PathBuf};

use crate::error::{Backend, SeatError};

/// One sysfs backlight device (a laptop panel, typically `intel_backlight`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backlight {
    /// The device name (the `/sys/class/backlight/<name>` leaf).
    pub name: String,
    /// Current raw brightness.
    pub brightness: u32,
    /// The device's advertised maximum raw brightness.
    pub max: u32,
}

impl Backlight {
    /// Brightness as a 0–100 percentage of the device max (max 0 ⇒ 0).
    #[must_use]
    pub fn percent(&self) -> u8 {
        if self.max == 0 {
            0
        } else {
            u8::try_from((u64::from(self.brightness) * 100 / u64::from(self.max)).min(100))
                .unwrap_or(100)
        }
    }
}

/// The backlight seam. Production impl: [`SysfsBacklight`]; tests inject a root.
pub trait BacklightClient: Send {
    /// Enumerate every backlight device.
    ///
    /// # Errors
    /// [`SeatError::Unavailable`] when the host has no backlight class.
    fn devices(&self) -> Result<Vec<Backlight>, SeatError>;

    /// Set a device's raw brightness.
    ///
    /// # Errors
    /// [`SeatError::OutOfRange`] if `value` exceeds the device max;
    /// [`SeatError::Io`] on a write failure; [`SeatError::Unavailable`] if absent.
    fn set_brightness(&self, name: &str, value: u32) -> Result<(), SeatError>;
}

/// The production client over `/sys/class/backlight`. The root is injectable
/// ([`SysfsBacklight::with_root`]) so enumeration + range logic test headless.
pub struct SysfsBacklight {
    root: PathBuf,
}

impl SysfsBacklight {
    /// A client over the real `/sys/class/backlight`.
    #[must_use]
    pub fn new() -> Self {
        Self::with_root("/sys/class/backlight")
    }

    /// A client over an alternate class root (the test seam).
    #[must_use]
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn read_u32(path: &Path) -> Result<u32, SeatError> {
        let raw = std::fs::read_to_string(path).map_err(|source| SeatError::Io {
            backend: Backend::Backlight,
            path: path.to_path_buf(),
            source,
        })?;
        raw.trim().parse::<u32>().map_err(|e| SeatError::Protocol {
            backend: Backend::Backlight,
            reason: format!("{}: not a u32: {e}", path.display()),
        })
    }
}

impl Default for SysfsBacklight {
    fn default() -> Self {
        Self::new()
    }
}

impl BacklightClient for SysfsBacklight {
    fn devices(&self) -> Result<Vec<Backlight>, SeatError> {
        let entries = std::fs::read_dir(&self.root).map_err(|e| SeatError::Unavailable {
            backend: Backend::Backlight,
            reason: format!("{}: {e}", self.root.display()),
        })?;
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let dir = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let brightness = Self::read_u32(&dir.join("brightness"))?;
            let max = Self::read_u32(&dir.join("max_brightness"))?;
            out.push(Backlight {
                name,
                brightness,
                max,
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    fn set_brightness(&self, name: &str, value: u32) -> Result<(), SeatError> {
        let dir = self.root.join(name);
        let max =
            Self::read_u32(&dir.join("max_brightness")).map_err(|_| SeatError::Unavailable {
                backend: Backend::Backlight,
                reason: format!("no backlight device {name}"),
            })?;
        if value > max {
            return Err(SeatError::OutOfRange {
                backend: Backend::Backlight,
                value,
                max,
            });
        }
        let path = dir.join("brightness");
        std::fs::write(&path, value.to_string()).map_err(|source| SeatError::Io {
            backend: Backend::Backlight,
            path,
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_device(root: &Path, name: &str, cur: u32, max: u32) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("brightness"), cur.to_string()).unwrap();
        std::fs::write(dir.join("max_brightness"), max.to_string()).unwrap();
    }

    fn scratch() -> PathBuf {
        // A process-unique, monotonically increasing nonce: `line!()` here would
        // expand to THIS line for every caller (colliding under parallel tests),
        // so use an atomic counter instead.
        static NONCE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("mde-seat-bl-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn percent_is_a_ratio_of_max_and_zero_max_is_zero() {
        assert_eq!(
            Backlight {
                name: "x".into(),
                brightness: 96,
                max: 192
            }
            .percent(),
            50
        );
        assert_eq!(
            Backlight {
                name: "x".into(),
                brightness: 5,
                max: 0
            }
            .percent(),
            0
        );
        assert_eq!(
            Backlight {
                name: "x".into(),
                brightness: 999,
                max: 100
            }
            .percent(),
            100
        );
    }

    #[test]
    fn enumerates_devices_sorted_and_reads_values() {
        let root = scratch();
        fake_device(&root, "intel_backlight", 120, 240);
        let bls = SysfsBacklight::with_root(&root).devices().unwrap();
        assert_eq!(bls.len(), 1);
        assert_eq!(bls[0].name, "intel_backlight");
        assert_eq!(bls[0].percent(), 50);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_class_root_is_typed_unavailable() {
        let e = SysfsBacklight::with_root("/no/such/backlight/root")
            .devices()
            .expect_err("missing root must not enumerate");
        assert_eq!(e.backend(), Backend::Backlight);
        assert!(matches!(e, SeatError::Unavailable { .. }), "{e}");
    }

    #[test]
    fn a_write_above_max_is_refused_out_of_range_not_clamped() {
        let root = scratch();
        fake_device(&root, "edp", 50, 100);
        let cl = SysfsBacklight::with_root(&root);
        let e = cl
            .set_brightness("edp", 250)
            .expect_err("over-max must be refused");
        assert!(
            matches!(
                e,
                SeatError::OutOfRange {
                    max: 100,
                    value: 250,
                    ..
                }
            ),
            "{e}"
        );
        // an in-range write lands
        cl.set_brightness("edp", 75).unwrap();
        assert_eq!(cl.devices().unwrap()[0].brightness, 75);
        let _ = std::fs::remove_dir_all(&root);
    }
}
