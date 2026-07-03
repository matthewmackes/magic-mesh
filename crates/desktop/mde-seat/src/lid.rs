//! The lid-switch client — the laptop lid open/closed state (design lock 3).
//!
//! A compositor-less DRM shell has to read the lid itself to honor a lid-close
//! action (POWER-5). The kernel exposes the switch two ways: the evdev `SW_LID`
//! bit on the ACPI-button input device, and the older
//! `/proc/acpi/button/lid/*/state` text file. This client uses the **`/proc`
//! read** — a pure file read, no new dependency (the airgapped build farm has no
//! vendored `evdev` crate), no `unsafe` (the crate forbids it), honestly typed.
//!
//! A desktop (no lid button) has no such node → the read folds to a typed
//! [`SeatError::Unavailable`], which the snapshot renders as an honest
//! `Absent` — never a fabricated "open" (§7). This pass exposes the state on the
//! snapshot; the poll-driven honorer that acts on a close is POWER-5.

use std::path::PathBuf;

use crate::error::{Backend, SeatError};

/// The ACPI lid-button sysfs/procfs class root.
const LID_ROOT: &str = "/proc/acpi/button/lid";

/// The laptop lid's physical state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LidState {
    /// The lid is open.
    Open,
    /// The lid is closed.
    Closed,
    /// A lid device exists but reported a state this client cannot read — shown
    /// as "unknown", never guessed to open or closed.
    Unknown,
}

impl LidState {
    /// The operator-facing state label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::Unknown => "unknown",
        }
    }
}

/// The lid-switch seam. Production impl: [`ProcLid`]; tests inject a scratch root
/// via [`ProcLid::with_root`].
pub trait LidClient: Send {
    /// Read the current lid state.
    ///
    /// # Errors
    /// [`SeatError::Unavailable`] when there is no lid button (a desktop); the
    /// honest `Absent` on the snapshot. [`SeatError::Io`] on a read failure of a
    /// present device.
    fn state(&self) -> Result<LidState, SeatError>;
}

/// The production client over `/proc/acpi/button/lid`. The root is injectable
/// ([`ProcLid::with_root`]) so the parse logic tests headless.
pub struct ProcLid {
    root: PathBuf,
}

impl ProcLid {
    /// A client over the real `/proc/acpi/button/lid`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            root: PathBuf::from(LID_ROOT),
        }
    }

    /// A client over an alternate root (the test seam).
    #[must_use]
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl Default for ProcLid {
    fn default() -> Self {
        Self::new()
    }
}

impl LidClient for ProcLid {
    fn state(&self) -> Result<LidState, SeatError> {
        let entries = std::fs::read_dir(&self.root).map_err(|e| SeatError::Unavailable {
            backend: Backend::Lid,
            reason: format!("{}: {e}", self.root.display()),
        })?;
        let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        dirs.sort();
        for dir in dirs {
            let state_file = dir.join("state");
            match std::fs::read_to_string(&state_file) {
                Ok(contents) => return Ok(parse_lid_state(&contents)),
                // No `state` file on this node — try the next lid directory.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(SeatError::Io {
                        backend: Backend::Lid,
                        path: state_file,
                        source,
                    })
                }
            }
        }
        // The lid root existed but held no readable `state` — no usable lid
        // device, so honestly Absent rather than a fabricated Open.
        Err(SeatError::Unavailable {
            backend: Backend::Lid,
            reason: format!("{}: no lid button device", self.root.display()),
        })
    }
}

/// Parse a `/proc/acpi/button/lid/*/state` file body into a [`LidState`]. Pure.
/// The line reads `state:      open` (or `closed`); anything else is
/// [`LidState::Unknown`], never guessed.
#[must_use]
pub fn parse_lid_state(contents: &str) -> LidState {
    for line in contents.lines() {
        if let Some((key, value)) = line.split_once(':') {
            if key.trim().eq_ignore_ascii_case("state") {
                return match value.trim().to_ascii_lowercase().as_str() {
                    "open" => LidState::Open,
                    "closed" => LidState::Closed,
                    _ => LidState::Unknown,
                };
            }
        }
    }
    LidState::Unknown
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn scratch() -> PathBuf {
        static NONCE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("mde-seat-lid-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn lid(root: &Path, name: &str, state_line: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("state"), state_line).unwrap();
    }

    #[test]
    fn parses_the_open_and_closed_lines_and_tolerates_junk() {
        assert_eq!(parse_lid_state("state:      open\n"), LidState::Open);
        assert_eq!(parse_lid_state("state:      closed\n"), LidState::Closed);
        // Case + spacing tolerant.
        assert_eq!(parse_lid_state("State: OPEN"), LidState::Open);
        // An unrecognized value → Unknown, never guessed.
        assert_eq!(parse_lid_state("state: ajar"), LidState::Unknown);
        // No state line at all → Unknown.
        assert_eq!(parse_lid_state("garbage\n"), LidState::Unknown);
        assert_eq!(LidState::Open.label(), "open");
    }

    #[test]
    fn reads_the_state_from_the_first_lid_device() {
        let root = scratch();
        lid(&root, "LID0", "state:      closed\n");
        let s = ProcLid::with_root(&root).state().unwrap();
        assert_eq!(s, LidState::Closed);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn no_lid_root_is_typed_unavailable_a_desktop() {
        let e = ProcLid::with_root("/no/such/lid")
            .state()
            .expect_err("a desktop has no lid → not a fabricated Open");
        assert_eq!(e.backend(), Backend::Lid);
        assert!(matches!(e, SeatError::Unavailable { .. }), "{e}");
    }

    #[test]
    fn a_lid_root_with_no_state_file_is_typed_unavailable() {
        let root = scratch();
        std::fs::create_dir_all(root.join("LID0")).unwrap(); // dir but no `state`
        let e = ProcLid::with_root(&root)
            .state()
            .expect_err("no readable state → Absent, not a guess");
        assert!(matches!(e, SeatError::Unavailable { .. }), "{e}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_real_client_on_this_host_answers_typed_never_panics() {
        // The build host is headless (no lid); the read is a typed Lid-tagged
        // Unavailable, or a real Open/Closed/Unknown on a laptop — never a panic.
        match ProcLid::new().state() {
            Ok(_) => {}
            Err(e) => assert_eq!(e.backend(), Backend::Lid),
        }
    }
}
