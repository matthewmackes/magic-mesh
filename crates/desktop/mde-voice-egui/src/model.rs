//! The render-agnostic view-model for the voice surface (E12-11).
//!
//! This module holds **no egui or socket types** — only the data the UI renders
//! and the small state machine that advances it. The worker threads (the SIP
//! agent + outbound dialing) send [`Update`]s in; [`VoiceState::apply`] folds
//! them into the state; the egui view reads the state and emits [`Command`]s
//! back. Because it touches neither a GPU, a socket, nor a sound device, the
//! whole thing is unit-tested below.
//!
//! It reuses `mde-voice-hud`'s own types directly (§6 glue, not reimplementation):
//! the [`RegistrationState`] and [`CallState`] the shipped SIP state machine
//! already defines and labels.

use mde_voice_hud::sip::{CallState, RegistrationState};

/// The complete render-agnostic state of the voice surface.
///
/// Holds the live SIP registration mirror, the current call lifecycle, and a
/// transient error banner. The egui view renders this; the worker drives it
/// forward through [`Update`]s.
#[derive(Debug)]
pub struct VoiceState {
    /// The account's registration with its registrar (or the P2P-overlay
    /// "registered" pseudo-state for a registrar-less node).
    pub registration: RegistrationState,
    /// The current call lifecycle (idle, dialing, ringing-in, connected, …).
    pub call: CallState,
    /// A transient error to surface (e.g. a media device failure); cleared when
    /// the next call starts.
    pub error: Option<String>,
}

impl Default for VoiceState {
    fn default() -> Self {
        Self {
            // Honest "nothing has reported in yet" — the agent's first event
            // replaces this with the real registration state.
            registration: RegistrationState::NoAccount,
            call: CallState::Idle,
            error: None,
        }
    }
}

/// A result message a worker thread sends back to the UI, folded into the
/// [`VoiceState`] by [`VoiceState::apply`].
#[derive(Debug)]
pub enum Update {
    /// The SIP agent's registration state changed.
    Registration(RegistrationState),
    /// An inbound call is ringing — the agent replied 180 and awaits the local
    /// Answer/Decline.
    Incoming {
        /// The caller's display name / address.
        from: String,
    },
    /// An outbound INVITE is in flight to `peer`.
    Dialing {
        /// The dialed peer / number.
        peer: String,
    },
    /// A call (in- or outbound) is now connected — the dialog is up and media is
    /// (best-effort) attached.
    Connected {
        /// The far end.
        peer: String,
    },
    /// The active call ended (local or remote hang-up, or a declined inbound).
    Ended,
    /// A call attempt failed (busy, declined, timeout, unreachable, …).
    Failed(String),
    /// A non-fatal error to surface in the banner (e.g. no audio device).
    Error(String),
}

/// An intent the UI sends to the worker.
#[derive(Debug, Clone)]
pub enum Command {
    /// Place an outbound call to the dialed string (a mesh peer name → a direct
    /// overlay call; a number → the registrar).
    Dial(String),
    /// Answer the ringing inbound call.
    Answer,
    /// Decline the ringing inbound call.
    Decline,
    /// Hang up the active call.
    HangUp,
    /// Re-attempt registration now.
    Reregister,
}

/// A coarse, render-agnostic status tone — the view maps it to a `Style` colour
/// so the colour choice stays out of the (GPU-free, testable) model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Settled / connected / registered.
    Ok,
    /// In progress (registering, dialing, ringing).
    Busy,
    /// Failed / error.
    Bad,
    /// Idle / nothing notable.
    Neutral,
}

impl VoiceState {
    /// A fresh, idle state — not registered, no call.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold a worker [`Update`] into the state.
    pub fn apply(&mut self, update: Update) {
        match update {
            Update::Registration(reg) => self.registration = reg,
            Update::Incoming { from } => self.call = CallState::Incoming { from },
            Update::Dialing { peer } => {
                self.call = CallState::Calling { peer };
                // A fresh attempt clears a stale error banner.
                self.error = None;
            }
            Update::Connected { peer } => self.call = CallState::InCall { peer },
            Update::Ended => self.call = CallState::Ended,
            Update::Failed(why) => self.call = CallState::Failed(why),
            Update::Error(e) => self.error = Some(e),
        }
    }

    /// Whether the dialer should be shown — i.e. no call is set up or ringing.
    /// (`Ended`/`Failed` return to the dialer, carrying their status as a hint.)
    #[must_use]
    pub const fn show_dialer(&self) -> bool {
        matches!(
            self.call,
            CallState::Idle | CallState::Ended | CallState::Failed(_)
        )
    }

    /// Whether an inbound call is ringing and awaiting Answer/Decline.
    #[must_use]
    pub const fn ringing_in(&self) -> bool {
        matches!(self.call, CallState::Incoming { .. })
    }
}

/// The display tone for a [`RegistrationState`].
#[must_use]
pub const fn registration_tone(reg: &RegistrationState) -> Tone {
    match reg {
        RegistrationState::Registered { .. } => Tone::Ok,
        RegistrationState::Registering => Tone::Busy,
        RegistrationState::Failed(_) => Tone::Bad,
        RegistrationState::NoAccount => Tone::Neutral,
    }
}

/// The display tone for a [`CallState`].
#[must_use]
pub const fn call_tone(call: &CallState) -> Tone {
    match call {
        CallState::InCall { .. } => Tone::Ok,
        CallState::Incoming { .. } | CallState::Calling { .. } | CallState::Ringing { .. } => {
            Tone::Busy
        }
        CallState::Failed(_) => Tone::Bad,
        CallState::Idle | CallState::Ended => Tone::Neutral,
    }
}

/// Whether the dialed string is callable (non-empty after trimming). The
/// peer-vs-number routing itself is `mde_voice_hud::sip::looks_like_peer`.
#[must_use]
pub fn dial_ready(input: &str) -> bool {
    !input.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_state_is_idle_and_unregistered() {
        // The empty / no-account starting state — honest, not a faked "registered".
        let s = VoiceState::new();
        assert_eq!(s.registration, RegistrationState::NoAccount);
        assert_eq!(s.call, CallState::Idle);
        assert!(s.error.is_none());
        assert!(s.show_dialer());
        assert!(!s.ringing_in());
        assert_eq!(registration_tone(&s.registration), Tone::Neutral);
    }

    #[test]
    fn registration_updates_replace_state_and_tone() {
        let mut s = VoiceState::new();
        s.apply(Update::Registration(RegistrationState::Registering));
        assert_eq!(registration_tone(&s.registration), Tone::Busy);

        s.apply(Update::Registration(RegistrationState::Registered {
            server: "sip.example.com:5060".to_string(),
            expires: 3600,
        }));
        assert_eq!(registration_tone(&s.registration), Tone::Ok);

        // A later failure replaces the registered state (honest, not kept).
        s.apply(Update::Registration(RegistrationState::Failed(
            "timeout".to_string(),
        )));
        assert_eq!(registration_tone(&s.registration), Tone::Bad);
    }

    #[test]
    fn outbound_call_lifecycle_and_error_clear() {
        let mut s = VoiceState::new();
        // A stale error banner is cleared the moment a new dial starts.
        s.apply(Update::Error("no audio device".to_string()));
        s.apply(Update::Dialing {
            peer: "pine".to_string(),
        });
        assert!(s.error.is_none());
        assert!(matches!(&s.call, CallState::Calling { peer } if peer == "pine"));
        assert!(!s.show_dialer());
        assert_eq!(call_tone(&s.call), Tone::Busy);

        s.apply(Update::Connected {
            peer: "pine".to_string(),
        });
        assert!(matches!(&s.call, CallState::InCall { peer } if peer == "pine"));
        assert_eq!(call_tone(&s.call), Tone::Ok);

        // Hanging up returns to the dialer.
        s.apply(Update::Ended);
        assert_eq!(s.call, CallState::Ended);
        assert!(s.show_dialer());
    }

    #[test]
    fn inbound_call_rings_then_connects_then_ends() {
        let mut s = VoiceState::new();
        s.apply(Update::Incoming {
            from: "Bob".to_string(),
        });
        assert!(s.ringing_in());
        assert!(!s.show_dialer());
        assert_eq!(call_tone(&s.call), Tone::Busy);

        s.apply(Update::Connected {
            peer: "Bob".to_string(),
        });
        assert!(matches!(&s.call, CallState::InCall { peer } if peer == "Bob"));
        assert!(!s.ringing_in());

        s.apply(Update::Ended);
        assert!(s.show_dialer());
    }

    #[test]
    fn failed_call_surfaces_and_returns_to_dialer() {
        let mut s = VoiceState::new();
        s.apply(Update::Dialing {
            peer: "1009".to_string(),
        });
        s.apply(Update::Failed("1009: busy".to_string()));
        assert!(matches!(&s.call, CallState::Failed(why) if why == "1009: busy"));
        assert_eq!(call_tone(&s.call), Tone::Bad);
        // A failed attempt still drops back to the dialer (the hint shows why).
        assert!(s.show_dialer());
    }

    #[test]
    fn error_update_sets_the_banner() {
        let mut s = VoiceState::new();
        s.apply(Update::Error("no audio device".to_string()));
        assert_eq!(s.error.as_deref(), Some("no audio device"));
    }

    #[test]
    fn dial_ready_requires_non_blank_input() {
        assert!(!dial_ready(""));
        assert!(!dial_ready("   "));
        assert!(dial_ready("pine"));
        assert!(dial_ready("1004"));
    }
}
