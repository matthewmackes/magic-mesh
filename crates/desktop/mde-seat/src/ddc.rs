//! The DDC/CI client seam — external-monitor brightness over i2c-dev (lock 13,
//! the external-monitor half; internal panels are [`crate::backlight`]).
//!
//! **E12-18** binds this for real. The i2c/DDC crates (`ddc-hi` / `i2c-linux`)
//! don't resolve under the airgapped 1.94 farm pin, so — exactly like the mixer's
//! `pw-dump`/`wpctl` path — the client's I/O is a narrow typed runner over the
//! `ddcutil` CLI ([`DdcUtil`]): `ddcutil detect` for the monitor inventory and
//! `getvcp`/`setvcp 0x10` for the brightness continuous VCP. The whole fold
//! (detect text → [`DdcDisplay`]s, `getvcp` line → current/max) is pure and
//! unit-tested; only [`DdcUtil`] touches a process.
//!
//! Honest-when-unavailable is the whole point (lock 13 / §7 / interlock 4): a host
//! with no `ddcutil` answers a typed [`SeatError::Unavailable`], and a monitor that
//! rejects DDC answers a typed error for *that bus* — the Display section then
//! renders "external brightness: not controllable" for it, never a dead slider.
//! [`UnboundDdc`] is retained as the explicit no-backend fallback seam.

use crate::error::{Backend, SeatError};

/// The VCP feature code for monitor luminance/brightness (DDC/CI standard).
const VCP_BRIGHTNESS: &str = "0x10";

/// One DDC/CI-controllable external display (an i2c-attached monitor).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DdcDisplay {
    /// The i2c bus label (`i2c-<n>`) the monitor answers on.
    pub bus: String,
    /// The DRM connector `ddcutil` maps this monitor to (`card0-DP-1`), when it
    /// reports one — the key that pairs a DDC monitor to a [`crate::Connector`].
    pub connector: Option<String>,
    /// The monitor model string from its EDID, when read.
    pub model: Option<String>,
    /// Current brightness (VCP 0x10), 0–100.
    pub brightness: u8,
}

impl DdcDisplay {
    /// The i2c bus number parsed from the [`Self::bus`] label (`i2c-4` → `4`).
    #[must_use]
    pub fn bus_number(&self) -> Option<u32> {
        self.bus.rsplit('-').next()?.parse().ok()
    }
}

/// The DDC/CI seam. Production impl ([`DdcCtl`] over the [`DdcUtil`] runner) drives
/// i2c-dev via `ddcutil`; [`UnboundDdc`] is the explicit no-backend answer.
///
/// The enumeration is split into two halves so the shell's off-thread snapshot
/// pump can run each on its own cadence (perf-2): [`detect`](Self::detect) is the
/// cheap `ddcutil detect` inventory the pump caches on the DRM connector set, and
/// [`fill_brightness`](Self::fill_brightness) is the SLOW per-monitor I2C `getvcp`
/// it re-reads on a much slower beat. [`displays`](Self::displays) composes both —
/// the original one-shot seam, unchanged for its existing callers.
pub trait DdcClient: Send {
    /// Detect DDC/CI-controllable external monitors — the inventory only, with
    /// brightness left at 0 for [`fill_brightness`](Self::fill_brightness) to fill.
    /// This is the cheap half the pump caches on the connector set.
    ///
    /// # Errors
    /// [`SeatError::Unavailable`] when no `ddcutil` is present or no i2c-dev bus
    /// answers DDC.
    fn detect(&self) -> Result<Vec<DdcDisplay>, SeatError>;

    /// Fill each already-detected monitor's live brightness (VCP 0x10) in place —
    /// the SLOW per-monitor I2C `getvcp`. A monitor that rejects the read keeps its
    /// current value (never dropped), so a working sibling is unaffected.
    fn fill_brightness(&self, displays: &mut [DdcDisplay]);

    /// Enumerate DDC/CI-controllable external monitors, each with its current
    /// brightness read from VCP 0x10 — [`detect`](Self::detect) then
    /// [`fill_brightness`](Self::fill_brightness) in one call.
    ///
    /// # Errors
    /// [`SeatError::Unavailable`] when no `ddcutil` is present or no i2c-dev bus
    /// answers DDC.
    fn displays(&self) -> Result<Vec<DdcDisplay>, SeatError> {
        let mut out = self.detect()?;
        self.fill_brightness(&mut out);
        Ok(out)
    }

    /// Set an external monitor's brightness (VCP 0x10), 0–100, addressed by its
    /// i2c bus label (`i2c-<n>`).
    ///
    /// # Errors
    /// [`SeatError::OutOfRange`] if the percentage exceeds 100;
    /// [`SeatError::Unavailable`] when `ddcutil` is absent; [`SeatError::Backend`]
    /// when the monitor rejects the write (the "not controllable" state).
    fn set_brightness(&self, bus: &str, percent: u8) -> Result<(), SeatError>;
}

/// The `ddcutil` I/O seam: detect monitors, read + write VCP 0x10. Production impl
/// is [`DdcUtil`]; tests inject a fake so the whole client folds headless.
pub trait DdcRunner: Send {
    /// The raw `ddcutil detect --terse` text (the monitor inventory).
    ///
    /// # Errors
    /// [`SeatError::Unavailable`] when `ddcutil` is absent; [`SeatError::Backend`]
    /// when it runs but fails.
    fn detect(&self) -> Result<String, SeatError>;

    /// The raw `ddcutil --brief --bus <n> getvcp 0x10` text for one monitor.
    ///
    /// # Errors
    /// [`SeatError::Backend`] when the monitor rejects the read (DDC refused);
    /// [`SeatError::Unavailable`] when `ddcutil` is absent.
    fn getvcp_brightness(&self, bus: u32) -> Result<String, SeatError>;

    /// Write `value` (0–100) to VCP 0x10 on the monitor at i2c bus `bus`.
    ///
    /// # Errors
    /// [`SeatError::Backend`] when the monitor rejects the write;
    /// [`SeatError::Unavailable`] when `ddcutil` is absent.
    fn setvcp_brightness(&self, bus: u32, value: u8) -> Result<(), SeatError>;
}

/// The real DDC/CI client (E12-18): folds `ddcutil detect` into [`DdcDisplay`]s and
/// reads/writes brightness through a [`DdcRunner`].
pub struct DdcCtl {
    runner: Box<dyn DdcRunner>,
}

impl DdcCtl {
    /// A client over the real host `ddcutil`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            runner: Box::new(DdcUtil),
        }
    }

    /// A client over an injected runner — the headless test / mirror seam.
    #[must_use]
    pub fn with_runner(runner: Box<dyn DdcRunner>) -> Self {
        Self { runner }
    }
}

impl Default for DdcCtl {
    fn default() -> Self {
        Self::new()
    }
}

impl DdcClient for DdcCtl {
    fn detect(&self) -> Result<Vec<DdcDisplay>, SeatError> {
        Ok(parse_detect(&self.runner.detect()?))
    }

    fn fill_brightness(&self, displays: &mut [DdcDisplay]) {
        // Fill each monitor's live brightness. A monitor that rejects the getvcp
        // (DDC refused) keeps its current value and stays in the list — the Display
        // section can still render it as an honest "not controllable" row rather
        // than dropping it, and a working sibling on another bus is unaffected.
        for disp in displays {
            if let Some(bus) = disp.bus_number() {
                if let Ok(text) = self.runner.getvcp_brightness(bus) {
                    if let Some((cur, max)) = parse_getvcp_brightness(&text) {
                        disp.brightness = percent_of(cur, max);
                    }
                }
            }
        }
    }

    // `displays()` uses the trait default (detect + fill_brightness) — the same
    // one-shot inventory-with-brightness it always produced.

    fn set_brightness(&self, bus: &str, percent: u8) -> Result<(), SeatError> {
        if percent > 100 {
            return Err(SeatError::OutOfRange {
                backend: Backend::Ddc,
                value: u32::from(percent),
                max: 100,
            });
        }
        let n = parse_bus_label(bus).ok_or_else(|| SeatError::Protocol {
            backend: Backend::Ddc,
            reason: format!("{bus:?} is not an i2c bus label"),
        })?;
        self.runner.setvcp_brightness(n, percent)
    }
}

/// The production `ddcutil` runner. `ddcutil` ships with the seat image's i2c
/// tooling (E12-19's packaging); a host without it surfaces as
/// [`SeatError::Unavailable`] — the honest "DDC not available here" state.
#[derive(Debug, Clone, Copy, Default)]
pub struct DdcUtil;

impl DdcUtil {
    /// Classify a failed spawn — a missing `ddcutil` is the honest "no DDC tooling"
    /// ([`SeatError::Unavailable`]); anything else is a real failure.
    fn spawn_error(ctx: &str, e: &std::io::Error) -> SeatError {
        if e.kind() == std::io::ErrorKind::NotFound {
            SeatError::Unavailable {
                backend: Backend::Ddc,
                reason: format!("{ctx} not found — ddcutil / i2c tooling absent"),
            }
        } else {
            SeatError::Backend {
                backend: Backend::Ddc,
                reason: format!("{ctx}: {e}"),
            }
        }
    }

    /// Run `ddcutil <args>`, returning stdout on success. A non-zero exit is a
    /// typed [`SeatError::Backend`] (a monitor rejecting DDC lands here), a missing
    /// binary is [`SeatError::Unavailable`].
    fn run(args: &[String]) -> Result<String, SeatError> {
        let out = std::process::Command::new("ddcutil")
            .args(args)
            .output()
            .map_err(|e| Self::spawn_error("ddcutil", &e))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(SeatError::Backend {
                backend: Backend::Ddc,
                reason: format!(
                    "ddcutil {}: {}",
                    args.join(" "),
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            })
        }
    }
}

impl DdcRunner for DdcUtil {
    fn detect(&self) -> Result<String, SeatError> {
        Self::run(&["detect".to_owned(), "--terse".to_owned()])
    }

    fn getvcp_brightness(&self, bus: u32) -> Result<String, SeatError> {
        Self::run(&[
            "--brief".to_owned(),
            "--bus".to_owned(),
            bus.to_string(),
            "getvcp".to_owned(),
            VCP_BRIGHTNESS.to_owned(),
        ])
    }

    fn setvcp_brightness(&self, bus: u32, value: u8) -> Result<(), SeatError> {
        Self::run(&[
            "--bus".to_owned(),
            bus.to_string(),
            "setvcp".to_owned(),
            VCP_BRIGHTNESS.to_owned(),
            value.min(100).to_string(),
        ])
        .map(|_| ())
    }
}

/// The not-yet-bound / no-backend DDC client: a typed [`SeatError::Unavailable`]
/// for every call. Retained as the explicit no-backend seam (the same honest
/// answer [`DdcCtl`] gives on a host without `ddcutil`).
///
/// This is a deliberate honest seam, not a stub that lies — the Display section
/// shows "external brightness: not controllable" rather than a dead control.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnboundDdc;

impl UnboundDdc {
    /// The reason string every call reports.
    const REASON: &'static str = "no DDC/CI backend bound";

    fn unavailable() -> SeatError {
        SeatError::Unavailable {
            backend: Backend::Ddc,
            reason: Self::REASON.to_owned(),
        }
    }
}

impl DdcClient for UnboundDdc {
    fn detect(&self) -> Result<Vec<DdcDisplay>, SeatError> {
        Err(Self::unavailable())
    }

    fn fill_brightness(&self, _displays: &mut [DdcDisplay]) {
        // No backend bound — nothing to read; leaves the (empty) list untouched.
    }

    // `displays()` uses the trait default: `detect()` Errs first, so it propagates
    // the same honest `Unavailable` it always did.

    fn set_brightness(&self, _bus: &str, _percent: u8) -> Result<(), SeatError> {
        Err(Self::unavailable())
    }
}

// ── pure folds (unit-tested headless) ────────────────────────────────────────

/// Parse an `i2c-<n>` bus label to its number.
fn parse_bus_label(bus: &str) -> Option<u32> {
    bus.rsplit('-').next()?.parse().ok()
}

/// Current/max VCP value → a 0–100 percentage (max 0 ⇒ 0, never a divide panic).
fn percent_of(cur: u16, max: u16) -> u8 {
    if max == 0 {
        0
    } else {
        u8::try_from((u32::from(cur) * 100 / u32::from(max)).min(100)).unwrap_or(100)
    }
}

/// Fold `ddcutil detect --terse` text into the monitor inventory (brightness left
/// at 0 for the caller to fill via `getvcp`).
///
/// The terse format is line-oriented blocks; a valid monitor block opens with
/// `Display <n>` and carries `I2C bus: /dev/i2c-<n>`, optionally a `DRM connector:`
/// and a `Monitor:` synopsis (`mfg:model:serial`). `Invalid display` blocks (a bus
/// that answered but isn't a usable monitor) are skipped — never a fake monitor.
#[must_use]
pub fn parse_detect(text: &str) -> Vec<DdcDisplay> {
    let mut out = Vec::new();
    let mut cur: Option<DdcDisplay> = None;
    let mut valid = false;

    let flush = |out: &mut Vec<DdcDisplay>, cur: Option<DdcDisplay>, valid: bool| {
        if let Some(d) = cur {
            if valid && !d.bus.is_empty() {
                out.push(d);
            }
        }
    };

    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with("Display ") {
            flush(&mut out, cur.take(), valid);
            valid = true;
            cur = Some(DdcDisplay {
                bus: String::new(),
                connector: None,
                model: None,
                brightness: 0,
            });
        } else if line.starts_with("Invalid display") || line.starts_with("Display slave") {
            // A bus that answered but is not a usable DDC monitor — open a block
            // so its I2C/connector lines don't attach to the previous monitor, but
            // mark it invalid so it never enters the inventory.
            flush(&mut out, cur.take(), valid);
            valid = false;
            cur = Some(DdcDisplay {
                bus: String::new(),
                connector: None,
                model: None,
                brightness: 0,
            });
        } else if let Some(d) = cur.as_mut() {
            if let Some(rest) = line.strip_prefix("I2C bus:") {
                // `/dev/i2c-4` → `i2c-4`.
                if let Some(n) = rest.trim().rsplit('/').next() {
                    d.bus = n.trim().to_owned();
                }
            } else if let Some(rest) = line.strip_prefix("DRM connector:") {
                let c = rest.trim();
                if !c.is_empty() {
                    d.connector = Some(c.to_owned());
                }
            } else if let Some(rest) = line.strip_prefix("Monitor:") {
                // `DEL:DELL U2415:7MT018...` — the middle field is the model.
                let syn = rest.trim();
                let model = syn
                    .split(':')
                    .nth(1)
                    .map(str::trim)
                    .filter(|m| !m.is_empty());
                d.model = model
                    .map(str::to_owned)
                    .or_else(|| (!syn.is_empty()).then(|| syn.to_owned()));
            }
        }
    }
    flush(&mut out, cur.take(), valid);
    out
}

/// Fold a `ddcutil --brief getvcp 0x10` line into `(current, max)`.
///
/// The brief form is `VCP <feature> <type> <current> <max>` for a continuous
/// feature (type `C`). Anything else (a non-continuous reply, a truncated line)
/// folds to `None` — the caller keeps the last-known value rather than inventing one.
#[must_use]
pub fn parse_getvcp_brightness(text: &str) -> Option<(u16, u16)> {
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        // VCP 10 C <cur> <max>
        if f.first() == Some(&"VCP") && f.len() >= 5 && f.get(2) == Some(&"C") {
            let cur = f[3].parse().ok()?;
            let max = f[4].parse().ok()?;
            return Some((cur, max));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[test]
    fn unbound_ddc_is_honestly_unavailable_not_a_lie() {
        let d = UnboundDdc;
        let e = d.displays().expect_err("must not fabricate monitors");
        assert_eq!(e.backend(), Backend::Ddc);
        assert!(matches!(e, SeatError::Unavailable { .. }), "{e}");
        let e = d
            .set_brightness("i2c-4", 50)
            .expect_err("must not fake a write");
        assert!(matches!(e, SeatError::Unavailable { .. }), "{e}");
    }

    #[test]
    fn parses_a_ddcutil_detect_with_a_valid_and_an_invalid_bus() {
        // A real `ddcutil detect --terse`: one working DP monitor, one bus that
        // answered but is not a usable monitor (must be skipped, never faked).
        let text = "\
Display 1
   I2C bus:  /dev/i2c-4
   DRM connector: card0-DP-1
   Monitor:      DEL:DELL U2415:7MT018AK1JZL
   VCP version:  2.1
Invalid display
   I2C bus:  /dev/i2c-5
   EDID synopsis:
      Mfg id:  BOE
";
        let got = parse_detect(text);
        assert_eq!(got.len(), 1, "the invalid bus must not enter the inventory");
        assert_eq!(got[0].bus, "i2c-4");
        assert_eq!(got[0].bus_number(), Some(4));
        assert_eq!(got[0].connector.as_deref(), Some("card0-DP-1"));
        assert_eq!(got[0].model.as_deref(), Some("DELL U2415"));
        assert_eq!(got[0].brightness, 0, "brightness is filled later by getvcp");
    }

    #[test]
    fn parses_the_brief_getvcp_brightness_line() {
        assert_eq!(parse_getvcp_brightness("VCP 10 C 65 100"), Some((65, 100)));
        // A monitor that reports a non-100 max still maps to a real percentage.
        assert_eq!(parse_getvcp_brightness("VCP 10 C 50 200"), Some((50, 200)));
        // A non-continuous / malformed reply is honestly None (keep last-known).
        assert_eq!(parse_getvcp_brightness("VCP 10 SNC x1 x2"), None);
        assert_eq!(parse_getvcp_brightness(""), None);
    }

    #[test]
    fn percent_of_is_a_ratio_and_zero_max_is_zero_not_a_panic() {
        assert_eq!(percent_of(65, 100), 65);
        assert_eq!(percent_of(50, 200), 25);
        assert_eq!(percent_of(5, 0), 0);
        assert_eq!(percent_of(300, 100), 100, "over-max clamps");
    }

    /// A fake runner: canned detect + getvcp text, recording every setvcp.
    #[derive(Clone, Default)]
    struct FakeRunner {
        detect: String,
        getvcp: String,
        getvcp_fails: bool,
        writes: Arc<Mutex<Vec<(u32, u8)>>>,
    }

    impl DdcRunner for FakeRunner {
        fn detect(&self) -> Result<String, SeatError> {
            Ok(self.detect.clone())
        }
        fn getvcp_brightness(&self, _bus: u32) -> Result<String, SeatError> {
            if self.getvcp_fails {
                Err(SeatError::Backend {
                    backend: Backend::Ddc,
                    reason: "DDC null response".into(),
                })
            } else {
                Ok(self.getvcp.clone())
            }
        }
        fn setvcp_brightness(&self, bus: u32, value: u8) -> Result<(), SeatError> {
            self.writes.lock().expect("lock").push((bus, value));
            Ok(())
        }
    }

    #[test]
    fn ddcctl_folds_detect_and_fills_live_brightness() {
        let runner = FakeRunner {
            detect: "Display 1\n   I2C bus:  /dev/i2c-4\n   Monitor: DEL:DELL U2415:X\n".into(),
            getvcp: "VCP 10 C 75 100".into(),
            ..Default::default()
        };
        let ctl = DdcCtl::with_runner(Box::new(runner));
        let disps = ctl.displays().expect("folds the fake detect");
        assert_eq!(disps.len(), 1);
        assert_eq!(disps[0].brightness, 75, "getvcp filled the live brightness");
    }

    #[test]
    fn a_monitor_that_rejects_getvcp_stays_listed_at_last_known_not_dropped() {
        // DDC refused on read: the monitor is still enumerated (so the UI can show
        // its honest "not controllable" row), just without a live brightness.
        let runner = FakeRunner {
            detect: "Display 1\n   I2C bus:  /dev/i2c-9\n".into(),
            getvcp_fails: true,
            ..Default::default()
        };
        let disps = DdcCtl::with_runner(Box::new(runner))
            .displays()
            .expect("detect still succeeds");
        assert_eq!(disps.len(), 1);
        assert_eq!(disps[0].brightness, 0);
    }

    #[test]
    fn set_brightness_reaches_the_runner_with_the_bus_number_and_refuses_over_100() {
        let runner = FakeRunner::default();
        let writes = runner.writes.clone();
        let ctl = DdcCtl::with_runner(Box::new(runner));
        ctl.set_brightness("i2c-4", 60).expect("write lands");
        assert_eq!(*writes.lock().expect("lock"), vec![(4, 60)]);

        // Over-100 is refused typed OutOfRange, never clamped silently.
        let e = ctl
            .set_brightness("i2c-4", 150)
            .expect_err("over-100 refused");
        assert!(matches!(e, SeatError::OutOfRange { max: 100, .. }), "{e}");
        // A non-bus label is a typed Protocol error, not a silent drop.
        assert!(matches!(
            ctl.set_brightness("nonsense", 10),
            Err(SeatError::Protocol { .. })
        ));
    }

    #[test]
    fn the_real_client_on_this_host_answers_typed_never_panics() {
        // With or without ddcutil, DdcCtl::new().displays() returns a real list or
        // a typed SeatError tagged Ddc — the §7 contract (the farm host has no
        // ddcutil, so this exercises the Unavailable path).
        match DdcCtl::new().displays() {
            Ok(list) => {
                for d in list {
                    assert!(d.brightness <= 100);
                }
            }
            Err(e) => assert_eq!(e.backend(), Backend::Ddc),
        }
    }
}
