//! The shell's **hotkey dispatch** (E12-19, Quasar host controls; design
//! `docs/design/quasar-host-controls.md`, locks 8/9).
//!
//! `mde_seat::hotkeys::HOTKEYS` is the fixed compiled-in table (chord → typed
//! [`HotkeyAction`]); this module is its **dispatcher** on the shell input path.
//! It applies lock 8's key policy:
//!
//! * the **XF86 media/system keys are host-first** — always matched, even while a
//!   fullscreen guest has focus (they arrive on the [`mde_egui::hostkeys`] side
//!   channel the DRM seat forwards, since egui has no key for them);
//! * **everything else reaches the guest** unless prefixed by the **leader chord**
//!   (Super, the Esc-chord reservation generalized): a named key only fires its
//!   action while the leader is held.
//!
//! Both the code→key map and the key→action map **derive from the fixed table**
//! (`action_for(chord)`), so the dispatcher can never drift from the read-only
//! Hotkeys section the System surface renders. The dispatch of each matched action
//! into a real seat / nav call lives in the shell (`main.rs` + `system.rs`); this
//! module only turns raw input into the typed [`HotkeyAction`]s.
//!
//! **Live seam:** the media-key + Super-leader scancodes only reach here on the
//! DRM seat (`run_drm` forwards them). On the windowed fallback the side channel is
//! empty, so leader chords + media keys are seat-only — hardware-gated, honest.

use mde_egui::egui;
use mde_egui::hostkeys::HostScan;

use mde_seat::hotkeys::{action_for, HotkeyAction};

/// An XF86 media/system key — the host-first set (lock 8). Each maps to a fixed
/// chord string in `mde_seat::hotkeys::HOTKEYS`, so its action is looked up there
/// rather than duplicated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MediaKey {
    /// `XF86AudioRaiseVolume`.
    VolumeUp,
    /// `XF86AudioLowerVolume`.
    VolumeDown,
    /// `XF86AudioMute`.
    Mute,
    /// `XF86AudioMicMute`.
    MicMute,
    /// `XF86MonBrightnessUp`.
    BrightnessUp,
    /// `XF86MonBrightnessDown`.
    BrightnessDown,
    /// `XF86Bluetooth`.
    Bluetooth,
}

impl MediaKey {
    /// The fixed-table chord string this key fires (the single source of truth).
    const fn chord(self) -> &'static str {
        match self {
            Self::VolumeUp => "XF86AudioRaiseVolume",
            Self::VolumeDown => "XF86AudioLowerVolume",
            Self::Mute => "XF86AudioMute",
            Self::MicMute => "XF86AudioMicMute",
            Self::BrightnessUp => "XF86MonBrightnessUp",
            Self::BrightnessDown => "XF86MonBrightnessDown",
            Self::Bluetooth => "XF86Bluetooth",
        }
    }

    /// The typed action, read from the fixed table (never `None` — every media
    /// chord is in `HOTKEYS`, asserted in tests).
    fn action(self) -> HotkeyAction {
        action_for(self.chord()).expect("every media chord is in the fixed table")
    }
}

/// A raw host-key scan the seat forwarded, decoded to what it means to the router:
/// a host-first media key, or a leader (Super) press/release that arms/disarms the
/// leader chord.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostKey {
    /// A host-first XF86 media/system key.
    Media(MediaKey),
    /// A leader (Super) key transition — `true` = pressed (arm), `false` = release.
    Leader(bool),
}

/// Decode a forwarded evdev scancode into a [`HostKey`], or `None` for a code the
/// shell doesn't act on. The code set matches `mde_egui::hostkeys::HOST_KEY_CODES`
/// (asserted in tests), so the runner and the shell never disagree on the host set.
const fn decode_scan(scan: HostScan) -> Option<HostKey> {
    Some(match scan.code {
        115 => HostKey::Media(MediaKey::VolumeUp),
        114 => HostKey::Media(MediaKey::VolumeDown),
        113 => HostKey::Media(MediaKey::Mute),
        248 => HostKey::Media(MediaKey::MicMute),
        225 => HostKey::Media(MediaKey::BrightnessUp),
        224 => HostKey::Media(MediaKey::BrightnessDown),
        237 => HostKey::Media(MediaKey::Bluetooth),
        125 | 126 => HostKey::Leader(scan.pressed),
        _ => return None,
    })
}

/// Map an egui named key to the leader-chord chord string (the keys that only fire
/// while the leader is held). `None` for a key that carries no leader action.
const fn leader_chord(key: egui::Key) -> Option<&'static str> {
    Some(match key {
        egui::Key::Tab => "Super+Tab",
        egui::Key::Backtick => "Super+grave",
        egui::Key::Escape => "Super+Escape",
        egui::Key::L => "Super+l",
        egui::Key::S => "Super+s",
        _ => return None,
    })
}

/// The stateful hotkey dispatcher: it carries only the leader latch (armed while a
/// Super key is held) and turns each frame's raw input into the matched typed
/// actions, applying lock 8. Pure + headless-testable — the shell owns the actual
/// seat / nav effects each action drives.
#[derive(Debug, Default)]
pub(crate) struct HotkeyRouter {
    /// Whether the leader (Super) is currently held — arms the leader chords.
    leader: bool,
}

impl HotkeyRouter {
    /// Whether the leader is currently held (the reserved-chord state).
    #[cfg(test)]
    pub(crate) const fn leader_armed(&self) -> bool {
        self.leader
    }

    /// Fold one forwarded host-key scan: a media key is host-first (always yields
    /// its action, lock 8); a leader transition only updates the latch and yields
    /// nothing itself.
    fn on_host_key(&mut self, scan: HostScan) -> Option<HotkeyAction> {
        match decode_scan(scan)? {
            HostKey::Media(m) => Some(m.action()),
            HostKey::Leader(down) => {
                self.leader = down;
                None
            }
        }
    }

    /// Fold one egui key press: a leader-chord named key fires its action **only**
    /// while the leader is held (lock 8 — otherwise it reaches the focused guest).
    fn on_egui_key(&self, key: egui::Key) -> Option<HotkeyAction> {
        if !self.leader {
            return None;
        }
        leader_chord(key).and_then(action_for)
    }

    /// The per-frame dispatch: drain the seat's forwarded host keys (media +
    /// leader), then fold this frame's egui key presses against the leader latch.
    /// Returns every matched typed action, in input order — the shell applies each
    /// to its seat / nav. Host keys are processed first so a same-frame
    /// `Super`+named-key chord sees the freshly-armed latch.
    pub(crate) fn dispatch(
        &mut self,
        host_keys: &[HostScan],
        egui_presses: &[egui::Key],
    ) -> Vec<HotkeyAction> {
        let mut actions = Vec::new();
        for scan in host_keys {
            if let Some(a) = self.on_host_key(*scan) {
                actions.push(a);
            }
        }
        for key in egui_presses {
            if let Some(a) = self.on_egui_key(*key) {
                actions.push(a);
            }
        }
        actions
    }
}

/// The egui key **presses** in this frame's input (a press, not a release), the
/// leader-chord half of the dispatch input. Kept tiny so the shell's render can
/// build it inline from `ctx.input`.
pub(crate) fn egui_key_presses(events: &[egui::Event]) -> Vec<egui::Key> {
    events
        .iter()
        .filter_map(|e| match e {
            egui::Event::Key {
                key, pressed: true, ..
            } => Some(*key),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_egui::hostkeys::HOST_KEY_CODES;

    fn scan(code: u32, pressed: bool) -> HostScan {
        HostScan { code, pressed }
    }

    #[test]
    fn every_media_chord_resolves_in_the_fixed_table_and_is_host_first() {
        for m in [
            MediaKey::VolumeUp,
            MediaKey::VolumeDown,
            MediaKey::Mute,
            MediaKey::MicMute,
            MediaKey::BrightnessUp,
            MediaKey::BrightnessDown,
            MediaKey::Bluetooth,
        ] {
            // Derived from the one table, and lock 8: a media key is host-first.
            assert!(m.action().host_first(), "{m:?} must be host-first");
        }
    }

    #[test]
    fn media_keys_fire_host_first_even_with_no_leader() {
        let mut r = HotkeyRouter::default();
        // No leader held, over a "fullscreen guest": the volume key still acts.
        let acts = r.dispatch(&[scan(115, true)], &[]);
        assert_eq!(acts, vec![HotkeyAction::VolumeUp]);
        let acts = r.dispatch(&[scan(225, true)], &[]);
        assert_eq!(acts, vec![HotkeyAction::BrightnessUp]);
        let acts = r.dispatch(&[scan(237, true)], &[]);
        assert_eq!(acts, vec![HotkeyAction::BluetoothToggle]);
    }

    #[test]
    fn a_named_key_reaches_the_guest_until_the_leader_is_held() {
        let mut r = HotkeyRouter::default();
        // Bare Tab: no action — it reaches the focused guest (lock 8).
        assert!(r.dispatch(&[], &[egui::Key::Tab]).is_empty());
        assert!(!r.leader_armed());

        // Press the leader (Super, evdev 125), then Tab in the same frame → the
        // session-switch chord fires; the guest never sees the Tab.
        let acts = r.dispatch(&[scan(125, true)], &[egui::Key::Tab]);
        assert_eq!(acts, vec![HotkeyAction::SessionSwitch]);
        assert!(r.leader_armed());

        // Still armed on a later frame: L → Lock, S → OpenSystem.
        assert_eq!(r.dispatch(&[], &[egui::Key::L]), vec![HotkeyAction::Lock]);
        assert_eq!(
            r.dispatch(&[], &[egui::Key::S]),
            vec![HotkeyAction::OpenSystem]
        );

        // Release the leader → the chord disarms; Tab reaches the guest again.
        assert!(r
            .dispatch(&[scan(125, false)], &[egui::Key::Tab])
            .is_empty());
        assert!(!r.leader_armed());
    }

    #[test]
    fn the_full_leader_chord_set_maps_to_its_typed_actions() {
        let mut r = HotkeyRouter::default();
        let _ = r.dispatch(&[scan(126, true)], &[]); // right-Super arms too
        assert_eq!(
            r.dispatch(&[], &[egui::Key::Backtick]),
            vec![HotkeyAction::MonitorFocusSwitch]
        );
        assert_eq!(
            r.dispatch(&[], &[egui::Key::Escape]),
            vec![HotkeyAction::ReturnToChrome]
        );
    }

    #[test]
    fn decode_scan_covers_exactly_the_runners_host_key_codes() {
        // Every code the seat forwards decodes to a host key; nothing else does.
        for &code in HOST_KEY_CODES {
            assert!(
                decode_scan(scan(code, true)).is_some(),
                "code {code} the runner forwards must decode"
            );
        }
        assert!(
            decode_scan(scan(30, true)).is_none(),
            "'A' is not a host key"
        );
    }

    #[test]
    fn egui_key_presses_keeps_presses_and_drops_releases_and_other_events() {
        let events = vec![
            egui::Event::Key {
                key: egui::Key::L,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            },
            egui::Event::Key {
                key: egui::Key::S,
                physical_key: None,
                pressed: false, // a release — dropped
                repeat: false,
                modifiers: egui::Modifiers::default(),
            },
            egui::Event::PointerGone,
        ];
        assert_eq!(egui_key_presses(&events), vec![egui::Key::L]);
    }
}
