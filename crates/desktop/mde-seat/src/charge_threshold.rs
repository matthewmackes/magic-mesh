//! The charge-threshold client — the battery charge-stop cap over
//! `/sys/class/power_supply/*/charge_control_end_threshold` (lock 13 / design
//! lock 4).
//!
//! Many laptops let you cap charging below 100% to spare the pack; the kernel
//! exposes that as a 0–100 sysfs attribute on the battery's power-supply node.
//! **Most machines lack it** — a desktop, or a laptop whose EC driver does not
//! implement it — so the read is `Option<u8>` and folds to an honest `None`,
//! never a fabricated cap (§7).
//!
//! **Writing needs privilege.** `charge_control_end_threshold` ships root-owned
//! `0644`, so the non-root DRM shell session gets `EACCES` on a write — that is
//! surfaced as a typed [`SeatError::Io`] (the honest "not permitted"), never a
//! pretend success. There is no logind side-door for this attribute (unlike
//! backlight), so the write is the direct sysfs write with an honest error.

use std::path::{Path, PathBuf};

use crate::error::{Backend, SeatError};

/// The sysfs power-supply class root.
const CLASS_ROOT: &str = "/sys/class/power_supply";
/// The charge-stop threshold attribute.
const END_ATTR: &str = "charge_control_end_threshold";

/// The charge-threshold seam. Production impl: [`SysfsChargeThreshold`]; tests
/// inject a scratch class root via [`SysfsChargeThreshold::with_root`].
pub trait ChargeThresholdClient: Send {
    /// The current charge-stop cap (0–100), or `None` when no battery on this
    /// host advertises `charge_control_end_threshold` (most machines).
    ///
    /// # Errors
    /// [`SeatError::Unavailable`] when the power-supply class root is unreadable
    /// (no power-supply subsystem at all); [`SeatError::Protocol`] when the
    /// attribute holds a non-integer.
    fn end_threshold(&self) -> Result<Option<u8>, SeatError>;

    /// Set the charge-stop cap (0–100) on the first battery that advertises the
    /// attribute.
    ///
    /// # Errors
    /// [`SeatError::OutOfRange`] when `pct` exceeds 100; [`SeatError::Unavailable`]
    /// when no battery advertises the attribute; [`SeatError::Io`] on the write
    /// (the honest `EACCES` when the session is not privileged), never a
    /// pretend success.
    fn set_end_threshold(&self, pct: u8) -> Result<(), SeatError>;
}

/// The production client over `/sys/class/power_supply`. The root is injectable
/// ([`SysfsChargeThreshold::with_root`]) so the read/range logic tests headless.
pub struct SysfsChargeThreshold {
    root: PathBuf,
}

impl SysfsChargeThreshold {
    /// A client over the real `/sys/class/power_supply`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            root: PathBuf::from(CLASS_ROOT),
        }
    }

    /// A client over an alternate class root (the test seam).
    #[must_use]
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The battery node directories under the class root, sorted for a
    /// deterministic "first advertiser wins" choice.
    fn supply_dirs(&self) -> Result<Vec<PathBuf>, SeatError> {
        let entries = std::fs::read_dir(&self.root).map_err(|e| SeatError::Unavailable {
            backend: Backend::ChargeThreshold,
            reason: format!("{}: {e}", self.root.display()),
        })?;
        let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        dirs.sort();
        Ok(dirs)
    }

    /// Read a 0–100 threshold attribute at `path`.
    fn read_pct(path: &Path) -> Result<u8, SeatError> {
        let raw = std::fs::read_to_string(path).map_err(|source| SeatError::Io {
            backend: Backend::ChargeThreshold,
            path: path.to_path_buf(),
            source,
        })?;
        raw.trim().parse::<u8>().map_err(|e| SeatError::Protocol {
            backend: Backend::ChargeThreshold,
            reason: format!("{}: not a 0-100 threshold: {e}", path.display()),
        })
    }
}

impl Default for SysfsChargeThreshold {
    fn default() -> Self {
        Self::new()
    }
}

impl ChargeThresholdClient for SysfsChargeThreshold {
    fn end_threshold(&self) -> Result<Option<u8>, SeatError> {
        for dir in self.supply_dirs()? {
            let attr = dir.join(END_ATTR);
            if attr.exists() {
                return Ok(Some(Self::read_pct(&attr)?));
            }
        }
        // The class exists but no battery advertises the cap — honest "not
        // supported on this machine", not an error and not a fake value.
        Ok(None)
    }

    fn set_end_threshold(&self, pct: u8) -> Result<(), SeatError> {
        if pct > 100 {
            return Err(SeatError::OutOfRange {
                backend: Backend::ChargeThreshold,
                value: u32::from(pct),
                max: 100,
            });
        }
        for dir in self.supply_dirs()? {
            let attr = dir.join(END_ATTR);
            if attr.exists() {
                // The direct sysfs write. `EACCES` on the root-owned attribute
                // surfaces as a typed Io error (the honest "not permitted"),
                // never a silent success.
                return std::fs::write(&attr, pct.to_string()).map_err(|source| SeatError::Io {
                    backend: Backend::ChargeThreshold,
                    path: attr,
                    source,
                });
            }
        }
        Err(SeatError::Unavailable {
            backend: Backend::ChargeThreshold,
            reason: "no battery advertises charge_control_end_threshold".to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        static NONCE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("mde-seat-ct-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn supply(root: &Path, name: &str, end: Option<u8>) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        if let Some(v) = end {
            std::fs::write(dir.join(END_ATTR), v.to_string()).unwrap();
        }
    }

    #[test]
    fn reads_the_end_threshold_from_the_first_battery_that_advertises_it() {
        let root = scratch();
        supply(&root, "AC", None); // the mains adapter has no threshold
        supply(&root, "BAT0", Some(80));
        let v = SysfsChargeThreshold::with_root(&root)
            .end_threshold()
            .unwrap();
        assert_eq!(v, Some(80));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_class_with_no_threshold_attr_reads_honest_none_not_a_fake_cap() {
        let root = scratch();
        supply(&root, "AC", None);
        supply(&root, "BAT0", None);
        assert_eq!(
            SysfsChargeThreshold::with_root(&root)
                .end_threshold()
                .unwrap(),
            None
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_class_root_is_typed_unavailable() {
        let e = SysfsChargeThreshold::with_root("/no/such/power_supply")
            .end_threshold()
            .expect_err("missing root must not read");
        assert_eq!(e.backend(), Backend::ChargeThreshold);
        assert!(matches!(e, SeatError::Unavailable { .. }), "{e}");
    }

    #[test]
    fn a_non_integer_threshold_is_typed_protocol_not_a_panic() {
        let root = scratch();
        let dir = root.join("BAT0");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(END_ATTR), "garbage").unwrap();
        let e = SysfsChargeThreshold::with_root(&root)
            .end_threshold()
            .expect_err("a non-integer must not fold to a value");
        assert!(matches!(e, SeatError::Protocol { .. }), "{e}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_in_range_write_lands_on_the_attribute() {
        let root = scratch();
        supply(&root, "BAT0", Some(100));
        let cl = SysfsChargeThreshold::with_root(&root);
        cl.set_end_threshold(75).unwrap();
        assert_eq!(cl.end_threshold().unwrap(), Some(75));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_write_above_100_is_refused_out_of_range_not_clamped() {
        let root = scratch();
        supply(&root, "BAT0", Some(80));
        let e = SysfsChargeThreshold::with_root(&root)
            .set_end_threshold(120)
            .expect_err("over-100 must be refused");
        assert!(
            matches!(
                e,
                SeatError::OutOfRange {
                    max: 100,
                    value: 120,
                    ..
                }
            ),
            "{e}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_write_with_no_advertising_battery_is_typed_unavailable() {
        let root = scratch();
        supply(&root, "BAT0", None);
        let e = SysfsChargeThreshold::with_root(&root)
            .set_end_threshold(80)
            .expect_err("no attr → no write target");
        assert!(matches!(e, SeatError::Unavailable { .. }), "{e}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_real_client_on_this_host_answers_typed_never_panics() {
        // The build host has no charge-threshold attribute (headless); the read
        // is either Unavailable (no class), Ok(None) (class, no cap), or a real
        // cap — always typed, never a panic.
        match SysfsChargeThreshold::new().end_threshold() {
            Ok(cap) => assert!(cap.is_none_or(|p| p <= 100)),
            Err(e) => assert_eq!(e.backend(), Backend::ChargeThreshold),
        }
    }
}
