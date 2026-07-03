//! The sysfs backlight client — panel brightness over `/sys/class/backlight`
//! (lock 13, the internal-panel half; DDC/CI external monitors are [`crate::ddc`]).
//!
//! Each backlight device exposes `brightness` (current) and `max_brightness`
//! (the ceiling); a write outside `0..=max` is refused typed
//! ([`SeatError::OutOfRange`]), never clamped silently. A host with no backlight
//! class (a desktop with only external monitors) answers
//! [`SeatError::Unavailable`] — an honest "no panel here", never a fake slider.
//!
//! **Writing brightness (BUG-BRIGHTNESS-1).** `/sys/class/backlight/<dev>/brightness`
//! ships `root:root 0644`, so the non-root DRM shell session cannot write it —
//! a raw write is `Permission denied`. Desktops don't write that file directly;
//! they ask logind. So the write path is: try logind's session
//! `SetBrightness` first (the caller's active session is privileged for it — no
//! root, no udev), and fall back to the direct sysfs write only when logind is
//! absent/refuses (a headless or root context). The honest error surfaces only
//! if *both* legs fail.

use std::path::{Path, PathBuf};

use crate::bus::SysBus;
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

/// The session-privileged brightness sink tried before the raw sysfs write.
///
/// Production impl [`LogindBrightness`] calls logind's
/// `org.freedesktop.login1.Session.SetBrightness`; tests inject a fake to
/// exercise the primary-then-fallback chain without a live bus.
trait BrightnessSink: Send {
    /// Set `subsystem`/`name` to raw `value` via the session-privileged path.
    ///
    /// # Errors
    /// Typed: [`SeatError::Unavailable`] when logind / the system bus is absent
    /// (the fallback trigger), [`SeatError::Backend`] on a refusal.
    fn set(&self, subsystem: &str, name: &str, value: u32) -> Result<(), SeatError>;
}

/// The production sink: logind session `SetBrightness` over the system bus.
///
/// The caller's own active logind session is privileged to set its seat's
/// backlight, so this needs no root and no udev rule. `SetBrightness` takes
/// `(subsystem: s, name: s, brightness: u)` on the `session/auto` object (the
/// caller's session), mirroring the [`crate::logind`] self-lock path.
struct LogindBrightness {
    bus: SysBus,
}

impl LogindBrightness {
    const fn new() -> Self {
        Self {
            bus: SysBus::new(Backend::Backlight),
        }
    }
}

impl BrightnessSink for LogindBrightness {
    fn set(&self, subsystem: &str, name: &str, value: u32) -> Result<(), SeatError> {
        self.bus.call_unit(
            "org.freedesktop.login1",
            "/org/freedesktop/login1/session/auto",
            "org.freedesktop.login1.Session",
            "SetBrightness",
            &(subsystem, name, value),
        )
    }
}

/// The production client over `/sys/class/backlight`. The root is injectable
/// ([`SysfsBacklight::with_root`]) so enumeration + range logic test headless.
pub struct SysfsBacklight {
    root: PathBuf,
    /// The session-brightness sink tried before the direct sysfs write. `None`
    /// on the [`SysfsBacklight::with_root`] test seam — hermetic, so a test over
    /// a scratch root can never reach out and dim a real panel.
    logind: Option<Box<dyn BrightnessSink>>,
}

impl SysfsBacklight {
    /// A client over the real `/sys/class/backlight`, preferring logind's
    /// session `SetBrightness` for writes (BUG-BRIGHTNESS-1).
    #[must_use]
    pub fn new() -> Self {
        Self {
            root: PathBuf::from("/sys/class/backlight"),
            logind: Some(Box::new(LogindBrightness::new())),
        }
    }

    /// A client over an alternate class root (the test seam). No logind sink:
    /// writes land straight on the scratch sysfs so enumeration + range logic
    /// test headless without touching a real session bus.
    #[must_use]
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            logind: None,
        }
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
        // Primary: logind's session `SetBrightness` — the privileged path a
        // non-root desktop session uses, so `brightness` staying root:root 0644
        // is not a wall (BUG-BRIGHTNESS-1). On absence/refusal (headless or a
        // root context with no session bus) fall through to the raw write.
        if let Some(sink) = &self.logind {
            if sink.set("backlight", name, value).is_ok() {
                return Ok(());
            }
        }
        // Fallback: the direct sysfs write. Its error is the honest final word
        // (the `Permission denied` the operator saw) when logind is unavailable
        // AND this too fails.
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
        // an in-range write lands (with_root has no logind sink → sysfs write)
        cl.set_brightness("edp", 75).unwrap();
        assert_eq!(cl.devices().unwrap()[0].brightness, 75);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A recording fake for the logind seam — captures every `SetBrightness`
    /// call and answers Ok/Err on demand, so the primary-then-fallback chain is
    /// testable without a live session bus.
    #[derive(Clone)]
    struct FakeSink {
        fail: bool,
        seen: std::sync::Arc<std::sync::Mutex<Vec<(String, String, u32)>>>,
    }

    impl FakeSink {
        fn new(fail: bool) -> Self {
            Self {
                fail,
                seen: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            }
        }
        fn calls(&self) -> Vec<(String, String, u32)> {
            self.seen.lock().unwrap().clone()
        }
    }

    impl BrightnessSink for FakeSink {
        fn set(&self, subsystem: &str, name: &str, value: u32) -> Result<(), SeatError> {
            self.seen
                .lock()
                .unwrap()
                .push((subsystem.to_string(), name.to_string(), value));
            if self.fail {
                Err(SeatError::Unavailable {
                    backend: Backend::Backlight,
                    reason: "fake: no logind session".into(),
                })
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn logind_takes_the_write_with_the_right_args_and_skips_sysfs() {
        let root = scratch();
        fake_device(&root, "intel_backlight", 100, 255);
        let sink = FakeSink::new(false);
        let probe = sink.clone();
        let cl = SysfsBacklight {
            root: root.clone(),
            logind: Some(Box::new(sink)),
        };
        cl.set_brightness("intel_backlight", 200).unwrap();
        // logind carried the write with (subsystem, name, raw value)...
        assert_eq!(
            probe.calls(),
            vec![("backlight".into(), "intel_backlight".into(), 200)]
        );
        // ...and the sysfs file was NOT touched — still the seeded 100.
        assert_eq!(cl.devices().unwrap()[0].brightness, 100);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_logind_failure_falls_back_to_the_sysfs_write() {
        let root = scratch();
        fake_device(&root, "intel_backlight", 100, 255);
        let sink = FakeSink::new(true);
        let probe = sink.clone();
        let cl = SysfsBacklight {
            root: root.clone(),
            logind: Some(Box::new(sink)),
        };
        cl.set_brightness("intel_backlight", 200).unwrap();
        // logind was tried first...
        assert_eq!(probe.calls().len(), 1);
        // ...and on its failure the raw sysfs write landed the new value.
        assert_eq!(cl.devices().unwrap()[0].brightness, 200);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn out_of_range_is_refused_before_either_write_path() {
        let root = scratch();
        fake_device(&root, "intel_backlight", 100, 255);
        let sink = FakeSink::new(false);
        let probe = sink.clone();
        let cl = SysfsBacklight {
            root: root.clone(),
            logind: Some(Box::new(sink)),
        };
        let e = cl
            .set_brightness("intel_backlight", 999)
            .expect_err("over-max must be refused");
        assert!(matches!(e, SeatError::OutOfRange { max: 255, .. }), "{e}");
        // Neither logind nor sysfs was written — the range check is the gate.
        assert!(probe.calls().is_empty());
        assert_eq!(cl.devices().unwrap()[0].brightness, 100);
        let _ = std::fs::remove_dir_all(&root);
    }
}
