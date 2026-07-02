//! The systemd-logind client — session lock + host power verbs (lock 12).
//!
//! Everything speaks the standard `org.freedesktop.login1` interfaces (the §2
//! FDO-interop exception). Two halves:
//!
//! - **Probe**: `CanSuspend`/`CanReboot`/`CanPowerOff` → a typed [`PowerCaps`],
//!   so the System surface can render honest availability (a verb polkit would
//!   refuse shows as such, never a dead button — interlock 4).
//! - **Verbs**: [`PowerVerb`] → the logind call. **Confirm-gating is the
//!   caller's duty** (the System surface's inline confirm; the remote two-phase
//!   handshake is E12-19) — this client only executes.

use crate::bus::SysBus;
use crate::error::{Backend, SeatError};

/// The logind well-known bus name.
const LOGIN1: &str = "org.freedesktop.login1";
/// The manager object path.
const MANAGER_PATH: &str = "/org/freedesktop/login1";
/// The manager interface.
const MANAGER_IFACE: &str = "org.freedesktop.login1.Manager";

/// A seat power action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerVerb {
    /// Lock this session (logind's `session/auto` — the caller's own session).
    Lock,
    /// Suspend the host.
    Suspend,
    /// Reboot the host.
    Reboot,
    /// Power the host off.
    PowerOff,
}

impl PowerVerb {
    /// The operator-facing verb label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            PowerVerb::Lock => "Lock",
            PowerVerb::Suspend => "Suspend",
            PowerVerb::Reboot => "Reboot",
            PowerVerb::PowerOff => "Power off",
        }
    }

    /// Does this verb need an explicit confirm before executing (lock 12)?
    /// Locking is benign; everything that takes the host down is gated.
    #[must_use]
    pub const fn needs_confirm(self) -> bool {
        !matches!(self, PowerVerb::Lock)
    }

    /// The logind **Manager** method behind this verb — `None` for
    /// [`PowerVerb::Lock`], which targets the caller's session object instead.
    #[must_use]
    pub(crate) const fn manager_method(self) -> Option<&'static str> {
        match self {
            PowerVerb::Lock => None,
            PowerVerb::Suspend => Some("Suspend"),
            PowerVerb::Reboot => Some("Reboot"),
            PowerVerb::PowerOff => Some("PowerOff"),
        }
    }

    /// The `CanX` probe method for this verb, when logind has one.
    #[must_use]
    pub(crate) const fn can_method(self) -> Option<&'static str> {
        match self {
            PowerVerb::Lock => None,
            PowerVerb::Suspend => Some("CanSuspend"),
            PowerVerb::Reboot => Some("CanReboot"),
            PowerVerb::PowerOff => Some("CanPowerOff"),
        }
    }
}

/// logind's answer to a `CanX` probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Avail {
    /// Allowed.
    Yes,
    /// Refused by policy.
    No,
    /// Needs interactive authorization (polkit challenge).
    Challenge,
    /// Not supported by this host at all.
    Na,
}

impl Avail {
    /// Fold logind's reply string. Anything unrecognized reads as [`Avail::Na`]
    /// (the conservative honest answer).
    #[must_use]
    pub fn from_reply(reply: &str) -> Self {
        match reply {
            "yes" => Avail::Yes,
            "no" => Avail::No,
            "challenge" => Avail::Challenge,
            _ => Avail::Na,
        }
    }

    /// Can the verb be offered as an affordance (it could succeed)?
    #[must_use]
    pub const fn offerable(self) -> bool {
        matches!(self, Avail::Yes | Avail::Challenge)
    }

    /// The operator-facing availability label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Avail::Yes => "available",
            Avail::No => "refused by policy",
            Avail::Challenge => "needs authorization",
            Avail::Na => "not supported",
        }
    }
}

/// The host's power-action availability, one probe per gated verb.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerCaps {
    /// `CanSuspend`.
    pub suspend: Avail,
    /// `CanReboot`.
    pub reboot: Avail,
    /// `CanPowerOff`.
    pub poweroff: Avail,
}

impl PowerCaps {
    /// The availability of `verb` ([`Avail::Yes`] for the ungated Lock).
    #[must_use]
    pub const fn for_verb(&self, verb: PowerVerb) -> Avail {
        match verb {
            PowerVerb::Lock => Avail::Yes,
            PowerVerb::Suspend => self.suspend,
            PowerVerb::Reboot => self.reboot,
            PowerVerb::PowerOff => self.poweroff,
        }
    }
}

/// The logind client seam. Production impl: [`ZbusLogind`]; tests inject fakes.
pub trait LogindClient: Send {
    /// Probe the host's power-action availability.
    ///
    /// # Errors
    /// Typed: [`SeatError::Unavailable`] when logind / the system bus is absent.
    fn caps(&self) -> Result<PowerCaps, SeatError>;

    /// Execute one power verb. The caller has already confirm-gated it.
    ///
    /// # Errors
    /// Typed: refused / absent logind comes back as a [`SeatError`], never a
    /// silent no-op.
    fn act(&self, verb: PowerVerb) -> Result<(), SeatError>;
}

/// The production logind client.
pub struct ZbusLogind {
    bus: SysBus,
}

impl ZbusLogind {
    /// A client over the system bus. No I/O until the first call.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            bus: SysBus::new(Backend::Logind),
        }
    }
}

impl Default for ZbusLogind {
    fn default() -> Self {
        Self::new()
    }
}

impl ZbusLogind {
    /// One verb's `CanX` probe → typed [`Avail`]. A verb without a probe (Lock)
    /// is always available — it targets the caller's own session.
    fn can(&self, verb: PowerVerb) -> Result<Avail, SeatError> {
        let Some(method) = verb.can_method() else {
            return Ok(Avail::Yes);
        };
        let reply: String = self
            .bus
            .call(LOGIN1, MANAGER_PATH, MANAGER_IFACE, method, &())?;
        Ok(Avail::from_reply(&reply))
    }
}

impl LogindClient for ZbusLogind {
    fn caps(&self) -> Result<PowerCaps, SeatError> {
        Ok(PowerCaps {
            suspend: self.can(PowerVerb::Suspend)?,
            reboot: self.can(PowerVerb::Reboot)?,
            poweroff: self.can(PowerVerb::PowerOff)?,
        })
    }

    fn act(&self, verb: PowerVerb) -> Result<(), SeatError> {
        match verb.manager_method() {
            // `session/auto` is the caller's own session — the always-allowed
            // self-lock, no polkit round-trip.
            None => self.bus.call_unit(
                LOGIN1,
                "/org/freedesktop/login1/session/auto",
                "org.freedesktop.login1.Session",
                "Lock",
                &(),
            ),
            // interactive=false: the shell already confirm-gated the action;
            // a polkit refusal comes back as a typed error, not a GUI prompt
            // (there is no agent on a bare seat).
            Some(method) => self
                .bus
                .call_unit(LOGIN1, MANAGER_PATH, MANAGER_IFACE, method, &(false,)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_fold_to_typed_availability() {
        assert_eq!(Avail::from_reply("yes"), Avail::Yes);
        assert_eq!(Avail::from_reply("no"), Avail::No);
        assert_eq!(Avail::from_reply("challenge"), Avail::Challenge);
        assert_eq!(Avail::from_reply("na"), Avail::Na);
        // Unknown answers degrade conservatively — never offered as available.
        assert_eq!(Avail::from_reply("maybe?"), Avail::Na);

        assert!(Avail::Yes.offerable());
        assert!(Avail::Challenge.offerable());
        assert!(!Avail::No.offerable());
        assert!(!Avail::Na.offerable());
    }

    #[test]
    fn verbs_map_to_the_right_logind_calls_and_gates() {
        // Lock targets the session object, not the Manager.
        assert_eq!(PowerVerb::Lock.manager_method(), None);
        assert_eq!(PowerVerb::Suspend.manager_method(), Some("Suspend"));
        assert_eq!(PowerVerb::Reboot.manager_method(), Some("Reboot"));
        assert_eq!(PowerVerb::PowerOff.manager_method(), Some("PowerOff"));

        assert_eq!(PowerVerb::Suspend.can_method(), Some("CanSuspend"));
        assert_eq!(PowerVerb::Reboot.can_method(), Some("CanReboot"));
        assert_eq!(PowerVerb::PowerOff.can_method(), Some("CanPowerOff"));

        // Lock 12: everything that takes the host down is confirm-gated;
        // locking is not.
        assert!(!PowerVerb::Lock.needs_confirm());
        assert!(PowerVerb::Suspend.needs_confirm());
        assert!(PowerVerb::Reboot.needs_confirm());
        assert!(PowerVerb::PowerOff.needs_confirm());
    }

    #[test]
    fn caps_route_each_verb_to_its_probe() {
        let caps = PowerCaps {
            suspend: Avail::Yes,
            reboot: Avail::Challenge,
            poweroff: Avail::No,
        };
        assert_eq!(caps.for_verb(PowerVerb::Lock), Avail::Yes);
        assert_eq!(caps.for_verb(PowerVerb::Suspend), Avail::Yes);
        assert_eq!(caps.for_verb(PowerVerb::Reboot), Avail::Challenge);
        assert_eq!(caps.for_verb(PowerVerb::PowerOff), Avail::No);
    }

    #[test]
    fn the_real_probe_on_this_host_answers_typed_never_panics() {
        // The build host may or may not run logind; either way the probe is a
        // typed answer, and this test never executes a verb (no reboots from
        // CI, ever).
        match ZbusLogind::new().caps() {
            Ok(caps) => {
                let _ = caps.for_verb(PowerVerb::Reboot).label();
            }
            Err(e) => assert_eq!(e.backend(), Backend::Logind),
        }
    }
}
