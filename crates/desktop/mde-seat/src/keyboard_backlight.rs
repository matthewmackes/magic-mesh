//! Typed keyboard-backlight control over `/sys/class/leds`.
//!
//! Kernel LED names are capability data, not UI paths. The client only admits
//! entries whose leaf name identifies a keyboard backlight and validates the
//! provider-issued name again before every write.

use std::path::{Path, PathBuf};

use crate::error::{Backend, SeatError};

const CLASS_ROOT: &str = "/sys/class/leds";

/// One kernel keyboard-backlight LED.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyboardBacklight {
    /// The provider-issued LED leaf name.
    pub name: String,
    /// Current raw brightness.
    pub brightness: u32,
    /// Advertised raw maximum.
    pub max: u32,
}

impl KeyboardBacklight {
    /// Current brightness as a bounded percentage.
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

/// Typed keyboard-backlight provider.
pub trait KeyboardBacklightClient: Send {
    /// Enumerate kernel LEDs that advertise keyboard-backlight semantics.
    fn devices(&self) -> Result<Vec<KeyboardBacklight>, SeatError>;

    /// Set a provider-issued LED's raw brightness.
    fn set_brightness(&self, name: &str, value: u32) -> Result<(), SeatError>;
}

/// Production provider over `/sys/class/leds`.
pub struct SysfsKeyboardBacklight {
    root: PathBuf,
}

impl SysfsKeyboardBacklight {
    /// The live kernel LED class.
    #[must_use]
    pub fn new() -> Self {
        Self {
            root: PathBuf::from(CLASS_ROOT),
        }
    }

    /// Injectable class root for hermetic tests.
    #[must_use]
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn read_u32(path: &Path) -> Result<u32, SeatError> {
        let raw = std::fs::read_to_string(path).map_err(|source| SeatError::Io {
            backend: Backend::KeyboardBacklight,
            path: path.to_path_buf(),
            source,
        })?;
        raw.trim().parse::<u32>().map_err(|e| SeatError::Protocol {
            backend: Backend::KeyboardBacklight,
            reason: format!("{}: not a u32: {e}", path.display()),
        })
    }

    fn is_keyboard_led(name: &str) -> bool {
        let lower = name.to_ascii_lowercase();
        (lower.ends_with("::kbd_backlight") || lower.contains("kbd_backlight"))
            && name.len() <= 160
            && !name.contains('/')
            && !name.contains('\\')
    }
}

impl Default for SysfsKeyboardBacklight {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyboardBacklightClient for SysfsKeyboardBacklight {
    fn devices(&self) -> Result<Vec<KeyboardBacklight>, SeatError> {
        let entries = std::fs::read_dir(&self.root).map_err(|e| SeatError::Unavailable {
            backend: Backend::KeyboardBacklight,
            reason: format!("{}: {e}", self.root.display()),
        })?;
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !Self::is_keyboard_led(&name) {
                continue;
            }
            let dir = entry.path();
            out.push(KeyboardBacklight {
                name,
                brightness: Self::read_u32(&dir.join("brightness"))?,
                max: Self::read_u32(&dir.join("max_brightness"))?,
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    fn set_brightness(&self, name: &str, value: u32) -> Result<(), SeatError> {
        if !Self::is_keyboard_led(name) {
            return Err(SeatError::Protocol {
                backend: Backend::KeyboardBacklight,
                reason: "not a provider-issued keyboard LED name".to_owned(),
            });
        }
        let dir = self.root.join(name);
        let max = Self::read_u32(&dir.join("max_brightness")).map_err(|_| {
            SeatError::Unavailable {
                backend: Backend::KeyboardBacklight,
                reason: format!("no keyboard backlight device {name}"),
            }
        })?;
        if value > max {
            return Err(SeatError::OutOfRange {
                backend: Backend::KeyboardBacklight,
                value,
                max,
            });
        }
        let path = dir.join("brightness");
        std::fs::write(&path, value.to_string()).map_err(|source| SeatError::Io {
            backend: Backend::KeyboardBacklight,
            path,
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "mde-seat-kbd-bl-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn folds_only_keyboard_leds_and_writes_bounded_values() {
        let root = scratch();
        let led = root.join("platform::kbd_backlight");
        std::fs::create_dir_all(&led).unwrap();
        std::fs::write(led.join("brightness"), "2").unwrap();
        std::fs::write(led.join("max_brightness"), "3").unwrap();
        let other = root.join("status");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("brightness"), "1").unwrap();
        std::fs::write(other.join("max_brightness"), "1").unwrap();

        let client = SysfsKeyboardBacklight::with_root(&root);
        let devices = client.devices().unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].percent(), 66);
        client.set_brightness(&devices[0].name, 3).unwrap();
        assert_eq!(std::fs::read_to_string(led.join("brightness")).unwrap(), "3");
        assert!(matches!(
            client.set_brightness(&devices[0].name, 4),
            Err(SeatError::OutOfRange { .. })
        ));
        let _ = std::fs::remove_dir_all(root);
    }
}
