//! CURTAIN-3 — the logind session **Lock/Unlock** signal listener.
//!
//! A desktop environment is expected to react to logind's session `Lock` signal
//! (`loginctl lock-session`, or any session-manager lock) by engaging its lock
//! screen. The DM-less DRM shell has no screensaver daemon standing in for it, so
//! the shell listens itself: a background thread subscribes to the session object's
//! `org.freedesktop.login1.Session` signals over the system bus and forwards
//! `Lock`/`Unlock` to the render loop, which drops the in-process
//! [`crate::curtain::Curtain`] (the same curtain Super+L and the idle/lid honorer
//! drive).
//!
//! **The source is a seam.** [`LockSignals`] is the trait the shell polls each
//! frame; [`LogindLockSource`] is the real zbus-backed implementation, and the
//! routing ([`apply_lock_signals`]) is a pure fold tested against a scripted source
//! — so `loginctl lock-session` → `curtain.lock()` is a unit test that never needs a
//! live bus (§7: the real bus path is host-gated, honest when absent).
//!
//! **Single seat (design lock 9).** The match filters logind's `Session` signals by
//! interface + member, not by a resolved session path — the DRM shell owns the one
//! graphical seat, so its lock is the only session a terminal in it can lock. An
//! `Unlock` signal is **received but never lifts the curtain**: the seat lifts ONLY
//! through the PAM verify seam (design lock 1, fail-closed) — `loginctl
//! unlock-session` cannot bypass authentication here.

#![allow(
    clippy::redundant_pub_crate,
    reason = "pub(crate) items in a private shell module are this crate's idiom \
              (curtain, power_honor, dock, …); main.rs's render consumes the \
              lock-signal seam"
)]

use std::sync::mpsc::{self, Receiver};
use std::thread;

use mde_egui::egui;

use crate::curtain::Curtain;

/// logind's well-known bus name.
const LOGIN1: &str = "org.freedesktop.login1";
/// The session object's interface — `Lock`/`Unlock` are broadcast on it.
const SESSION_IFACE: &str = "org.freedesktop.login1.Session";

/// A session lock-state signal logind emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LockSignal {
    /// `loginctl lock-session` — drop the curtain.
    Lock,
    /// `loginctl unlock-session` — received, but the curtain still lifts only through
    /// PAM (design lock 1); see [`apply_lock_signals`].
    Unlock,
}

impl LockSignal {
    /// The logind `Session` signal member this variant maps to, or `None` for a
    /// member the shell does not act on (`PauseDevice`, `PropertiesChanged`, …).
    fn from_member(member: &str) -> Option<Self> {
        match member {
            "Lock" => Some(Self::Lock),
            "Unlock" => Some(Self::Unlock),
            _ => None,
        }
    }
}

/// The receive seam: yields any session lock signals that have arrived since the last
/// poll (never blocks). The real implementation is [`LogindLockSource`]; tests script
/// a queue, so the shell's routing is exercised without a live bus.
pub(crate) trait LockSignals {
    /// Drain the signals seen since the previous call (empty when none / no bus).
    fn poll(&mut self) -> Vec<LockSignal>;
}

/// The production source: a background thread subscribes to logind's session
/// `Lock`/`Unlock` over the system bus and forwards each to this channel; [`poll`]
/// drains it. When there is no system bus / logind (headless CI, the windowed
/// fallback), the thread exits after an honest note and [`poll`] simply stays empty —
/// never a panic, never a faked signal.
///
/// [`poll`]: LockSignals::poll
pub(crate) struct LogindLockSource {
    /// The receive end of the listener thread's forwarding channel.
    rx: Receiver<LockSignal>,
}

impl LogindLockSource {
    /// Spawn the listener over the system bus. Returns immediately — the blocking
    /// bus dial + signal loop run on the thread. `ctx` is cloned so an arriving
    /// signal wakes the render loop (the shell-worker repaint idiom), so a lock lands
    /// promptly even on an otherwise idle frame cadence.
    pub(crate) fn new(ctx: &egui::Context) -> Self {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        if let Err(e) = thread::Builder::new()
            .name("curtain-logind-lock".to_owned())
            .spawn(move || listen(&tx, &ctx))
        {
            eprintln!("curtain: could not spawn the logind lock listener: {e}");
        }
        Self { rx }
    }
}

impl LockSignals for LogindLockSource {
    fn poll(&mut self) -> Vec<LockSignal> {
        // Drain everything queued this frame. `try_recv` never blocks; both Empty
        // (nothing new) and Disconnected (the thread exited — no bus) end the loop
        // honestly, so an absent bus simply yields no signals.
        let mut out = Vec::new();
        while let Ok(sig) = self.rx.try_recv() {
            out.push(sig);
        }
        out
    }
}

/// The listener thread body: dial the system bus, match logind's session signals, and
/// forward each `Lock`/`Unlock` (waking the shell). Any bus/match failure is an honest
/// note + a clean exit — the shell keeps running unlocked-capable, the seam just stays
/// quiet (the real path is host-gated).
fn listen(tx: &mpsc::Sender<LockSignal>, ctx: &egui::Context) {
    let conn = match zbus::blocking::Connection::system() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("curtain: logind lock listener — no system bus ({e}); lock-session inert");
            return;
        }
    };
    let rule = match zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender(LOGIN1)
        .and_then(|b| b.interface(SESSION_IFACE))
    {
        Ok(b) => b.build(),
        Err(e) => {
            eprintln!("curtain: logind lock listener — bad match rule ({e})");
            return;
        }
    };
    let iter = match zbus::blocking::MessageIterator::for_match_rule(rule, &conn, None) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("curtain: logind lock listener — could not subscribe ({e})");
            return;
        }
    };
    for msg in iter {
        let Ok(msg) = msg else { continue };
        let header = msg.header();
        let Some(member) = header.member() else {
            continue;
        };
        let Some(sig) = LockSignal::from_member(member.as_str()) else {
            continue;
        };
        if tx.send(sig).is_err() {
            // The shell dropped the receiver — nothing to notify, stop listening.
            break;
        }
        ctx.request_repaint();
    }
}

/// Route this frame's drained lock signals onto the curtain (CURTAIN-3): a `Lock`
/// drops the in-process curtain (idempotent while already engaged); an `Unlock` is
/// deliberately a no-op — the seat lifts ONLY through the PAM verify seam (design lock
/// 1, fail-closed), so a logind unlock never bypasses authentication.
pub(crate) fn apply_lock_signals(signals: &[LockSignal], curtain: &mut Curtain) {
    for sig in signals {
        // A Lock drops the curtain (idempotent while engaged). An Unlock is
        // intentionally NOT acted on — the seat lifts ONLY through the PAM verify
        // seam (design lock 1, fail-closed), so `loginctl unlock-session` never
        // bypasses authentication here.
        if matches!(sig, LockSignal::Lock) {
            curtain.lock();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// A scripted source — the exact seam [`LogindLockSource`]'s channel fills at
    /// runtime, so the routing is tested with no bus.
    struct Scripted {
        queued: VecDeque<LockSignal>,
    }

    impl LockSignals for Scripted {
        fn poll(&mut self) -> Vec<LockSignal> {
            self.queued.drain(..).collect()
        }
    }

    #[test]
    fn a_logind_lock_signal_drops_the_curtain_through_the_seam() {
        // The simulated `loginctl lock-session`: a scripted source yields Lock, and
        // the routing drops the curtain — no live bus in the test.
        let mut source = Scripted {
            queued: VecDeque::from(vec![LockSignal::Lock]),
        };
        let mut curtain = Curtain::default();
        assert!(!curtain.engaged(), "starts unlocked");

        apply_lock_signals(&source.poll(), &mut curtain);
        assert!(
            curtain.engaged(),
            "a logind Lock signal must drop the curtain (loginctl lock-session)"
        );

        // A second Lock is idempotent — never re-drops a standing curtain.
        let mut again = Scripted {
            queued: VecDeque::from(vec![LockSignal::Lock]),
        };
        apply_lock_signals(&again.poll(), &mut curtain);
        assert!(curtain.engaged());
    }

    #[test]
    fn an_unlock_signal_never_lifts_the_curtain_without_pam() {
        // A locked curtain + a logind Unlock: the curtain stays engaged — only PAM
        // (design lock 1) may lift it, so unlock-session cannot bypass auth.
        let mut curtain = Curtain::default();
        curtain.lock();
        assert!(curtain.engaged());

        let mut source = Scripted {
            queued: VecDeque::from(vec![LockSignal::Unlock, LockSignal::Unlock]),
        };
        apply_lock_signals(&source.poll(), &mut curtain);
        assert!(
            curtain.engaged(),
            "a logind Unlock must NOT lift the curtain (PAM-only, fail-closed)"
        );
    }

    #[test]
    fn only_lock_and_unlock_members_map_to_a_signal() {
        assert_eq!(LockSignal::from_member("Lock"), Some(LockSignal::Lock));
        assert_eq!(LockSignal::from_member("Unlock"), Some(LockSignal::Unlock));
        // Every other Session signal logind broadcasts is ignored (never a lock).
        assert_eq!(LockSignal::from_member("PauseDevice"), None);
        assert_eq!(LockSignal::from_member("PropertiesChanged"), None);
        assert_eq!(LockSignal::from_member(""), None);
    }

    #[test]
    fn an_empty_poll_leaves_the_curtain_untouched() {
        let mut source = Scripted {
            queued: VecDeque::new(),
        };
        let mut curtain = Curtain::default();
        apply_lock_signals(&source.poll(), &mut curtain);
        assert!(!curtain.engaged(), "no signal, no lock");
    }
}
