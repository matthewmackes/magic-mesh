//! `hostkeys` — the seat's **host-key side channel** (E12-19, Construct hotkeys).
//!
//! egui's [`egui::Key`](crate::egui::Key) enum has no XF86 media / system keys
//! (volume, brightness, Bluetooth) and no Super/"leader" key, so those never reach
//! a surface through the normal egui event stream — yet lock 8 needs them
//! **host-first**: the shell must act on them even while a fullscreen guest has
//! focus. The DRM/libinput owner is [`crate::drm::run_drm`]; it is the only code
//! that sees the raw evdev stream, so it forwards the host-relevant scancodes here.
//! The shell drains them each frame ([`drain_host_keys`]) and maps them to its
//! fixed hotkey table — the semantics live in the shell, this module only carries
//! the raw `(evdev code, pressed)` pairs across the runner→surface seam.
//!
//! It is a **process-thread-local queue** (the DRM present loop and the surface's
//! render run on the same thread, so a lock-free thread-local is the right shape,
//! matching the single-threaded decode→UI hand-off the VDI panel uses). The
//! windowed [`crate::run_client`] path never feeds it, so [`drain_host_keys`]
//! there is simply always empty — the hotkey wiring self-gates to the real seat.

use std::cell::RefCell;

thread_local! {
    /// The pending host-key events, in arrival order. Bounded in practice by the
    /// per-frame drain: `run_drm` pushes during its libinput drain, then the same
    /// frame's `ui(ctx)` render drains everything.
    static HOST_KEYS: RefCell<Vec<HostScan>> = const { RefCell::new(Vec::new()) };
}

/// One raw host-key scan the seat forwarded: the Linux evdev keycode and whether
/// it was a press (`true`) or a release (`false`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostScan {
    /// The Linux evdev keycode (`input-event-codes.h`, e.g. `KEY_VOLUMEUP` = 115).
    pub code: u32,
    /// `true` on key-down, `false` on key-up.
    pub pressed: bool,
}

/// The evdev keycodes the seat forwards as host keys (lock 8): the XF86 media /
/// system keys that are always host-first, plus the two Super/"leader" keys.
///
/// Everything else stays in the ordinary egui event stream and reaches the focused
/// guest. Kept here so the runner and the shell agree on the exact set (a leader
/// chord's arming key, and the media keys — the shell owns what each *means*).
pub const HOST_KEY_CODES: &[u32] = &[
    113, // KEY_MUTE            → XF86AudioMute
    114, // KEY_VOLUMEDOWN      → XF86AudioLowerVolume
    115, // KEY_VOLUMEUP        → XF86AudioRaiseVolume
    163, // KEY_NEXTSONG        → XF86AudioNext
    164, // KEY_PLAYPAUSE       → XF86AudioPlay
    165, // KEY_PREVIOUSSONG    → XF86AudioPrev
    166, // KEY_STOPCD          → XF86AudioStop
    200, // KEY_PLAYCD          → XF86AudioPlay
    201, // KEY_PAUSECD         → XF86AudioPause
    207, // KEY_PLAY            → XF86AudioPlay
    224, // KEY_BRIGHTNESSDOWN  → XF86MonBrightnessDown
    225, // KEY_BRIGHTNESSUP    → XF86MonBrightnessUp
    237, // KEY_BLUETOOTH       → XF86Bluetooth
    248, // KEY_MICMUTE         → XF86AudioMicMute
    125, // KEY_LEFTMETA        → the leader (Super)
    126, // KEY_RIGHTMETA       → the leader (Super)
];

/// Whether the seat should forward this evdev keycode as a host key. `run_drm`
/// gates on this so the thread-local queue only ever carries the small host set,
/// never the whole keyboard.
#[must_use]
pub fn is_host_key(code: u32) -> bool {
    HOST_KEY_CODES.contains(&code)
}

/// Forward one raw host-key scan from the seat (the libinput owner) to the surface.
/// A no-op-cheap thread-local push; the caller has already gated on [`is_host_key`].
pub fn push_host_key(code: u32, pressed: bool) {
    HOST_KEYS.with(|q| q.borrow_mut().push(HostScan { code, pressed }));
}

/// Drain every pending host-key scan (the surface calls this once per frame).
///
/// The queue is left empty. On the windowed path (no seat feeding it) this is always
/// an empty vector, so the hotkey dispatch self-gates to the real DRM seat.
#[must_use]
pub fn drain_host_keys() -> Vec<HostScan> {
    HOST_KEYS.with(|q| std::mem::take(&mut *q.borrow_mut()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_then_drain_returns_in_order_and_empties() {
        // A fresh drain clears anything a prior test on this thread left behind.
        let _ = drain_host_keys();
        push_host_key(115, true);
        push_host_key(115, false);
        let drained = drain_host_keys();
        assert_eq!(
            drained,
            vec![
                HostScan {
                    code: 115,
                    pressed: true
                },
                HostScan {
                    code: 115,
                    pressed: false
                },
            ]
        );
        // A second drain with nothing pushed is empty (the windowed-path shape).
        assert!(drain_host_keys().is_empty());
    }

    #[test]
    fn host_key_set_covers_the_media_and_leader_codes_only() {
        // The XF86 media/system keys + the two Super keys are host keys…
        for code in [
            113, 114, 115, 163, 164, 165, 166, 200, 201, 207, 224, 225, 237, 248, 125, 126,
        ] {
            assert!(is_host_key(code), "code {code} should be a host key");
        }
        // …and an ordinary letter / Tab is NOT (it reaches the guest via egui).
        assert!(!is_host_key(15), "Tab is not a host key");
        assert!(!is_host_key(38), "L is not a host key");
    }
}
