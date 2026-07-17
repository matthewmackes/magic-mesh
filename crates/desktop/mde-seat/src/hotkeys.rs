//! The fixed compiled-in hotkey table (lock 9).
//!
//! Quazar does **not** offer configurable bindings: the table is a compile-time
//! constant so the mapping is auditable, cannot drift, and needs no persistence.
//! Each entry maps a chord (an XF86 media/system key, or a leader-chord combo)
//! to one **typed** [`HotkeyAction`] — never a shell string (§9, no raw exec).
//! Dispatch (turning a matched action into the seat/session call) is E12-19's
//! work; this module is the authoritative *table* the dispatcher and the System
//! surface's read-only Hotkeys section both read.

/// A typed host action a hotkey can fire. The set is closed — the dispatcher
/// matches on it exhaustively, so a new hotkey cannot smuggle in an untyped verb.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    /// Raise the master output volume.
    VolumeUp,
    /// Lower the master output volume.
    VolumeDown,
    /// Toggle master mute.
    VolumeMute,
    /// Toggle media playback for the active media surface.
    MediaPlayPause,
    /// Pause media playback for the active media surface.
    MediaPause,
    /// Stop media playback for the active media surface.
    MediaStop,
    /// Advance active media to the next item.
    MediaNext,
    /// Return active media to the previous item.
    MediaPrevious,
    /// Toggle the active input (microphone) mute.
    MicMute,
    /// Raise the active output's brightness.
    BrightnessUp,
    /// Lower the active output's brightness.
    BrightnessDown,
    /// Toggle the Bluetooth adapter power.
    BluetoothToggle,
    /// Cycle keyboard/pointer focus to the next VM session.
    SessionSwitch,
    /// Move focus to the VM session on the next monitor.
    MonitorFocusSwitch,
    /// Leave a fullscreen guest and return to the chrome bar (the reserved chord,
    /// generalized from the VDI Esc chord).
    ReturnToChrome,
    /// Lock the seat (logind).
    Lock,
    /// Open the System surface.
    OpenSystem,
    /// Open the shell-owned unified search/omnibox front door.
    OpenOmnibox,
}

impl HotkeyAction {
    /// A short operator-facing label for the Hotkeys section.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::VolumeUp => "Volume up",
            Self::VolumeDown => "Volume down",
            Self::VolumeMute => "Mute output",
            Self::MediaPlayPause => "Play/pause media",
            Self::MediaPause => "Pause media",
            Self::MediaStop => "Stop media",
            Self::MediaNext => "Next media",
            Self::MediaPrevious => "Previous media",
            Self::MicMute => "Mute microphone",
            Self::BrightnessUp => "Brightness up",
            Self::BrightnessDown => "Brightness down",
            Self::BluetoothToggle => "Toggle Bluetooth",
            Self::SessionSwitch => "Switch session",
            Self::MonitorFocusSwitch => "Focus next monitor",
            Self::ReturnToChrome => "Return to chrome",
            Self::Lock => "Lock seat",
            Self::OpenSystem => "Open System panel",
            Self::OpenOmnibox => "Open omnibox",
        }
    }

    /// Whether this action is a **host-first** dedicated key (an `XF86*` / power
    /// key that the shell always handles even while a VM session has focus, lock
    /// 8), versus a leader-chord action that only fires after the chord prefix.
    #[must_use]
    pub const fn host_first(self) -> bool {
        matches!(
            self,
            Self::VolumeUp
                | Self::VolumeDown
                | Self::VolumeMute
                | Self::MediaPlayPause
                | Self::MediaPause
                | Self::MediaStop
                | Self::MediaNext
                | Self::MediaPrevious
                | Self::MicMute
                | Self::BrightnessUp
                | Self::BrightnessDown
                | Self::BluetoothToggle
        )
    }
}

/// One row of the fixed table: the chord that fires it and the action it maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    /// The chord label as the operator reads it (`XF86AudioRaiseVolume`,
    /// `Super+Tab`, …). This is documentation + the Hotkeys-section render; the
    /// dispatcher matches the real libinput key, not this string.
    pub chord: &'static str,
    /// The typed action the chord fires.
    pub action: HotkeyAction,
}

/// The authoritative compiled-in table. Host-first XF86/system keys first, then
/// the leader-chord actions.
pub static HOTKEYS: &[Hotkey] = &[
    Hotkey {
        chord: "XF86AudioRaiseVolume",
        action: HotkeyAction::VolumeUp,
    },
    Hotkey {
        chord: "XF86AudioLowerVolume",
        action: HotkeyAction::VolumeDown,
    },
    Hotkey {
        chord: "XF86AudioMute",
        action: HotkeyAction::VolumeMute,
    },
    Hotkey {
        chord: "XF86AudioPlay",
        action: HotkeyAction::MediaPlayPause,
    },
    Hotkey {
        chord: "XF86AudioPause",
        action: HotkeyAction::MediaPause,
    },
    Hotkey {
        chord: "XF86AudioStop",
        action: HotkeyAction::MediaStop,
    },
    Hotkey {
        chord: "XF86AudioNext",
        action: HotkeyAction::MediaNext,
    },
    Hotkey {
        chord: "XF86AudioPrev",
        action: HotkeyAction::MediaPrevious,
    },
    Hotkey {
        chord: "XF86AudioMicMute",
        action: HotkeyAction::MicMute,
    },
    Hotkey {
        chord: "XF86MonBrightnessUp",
        action: HotkeyAction::BrightnessUp,
    },
    Hotkey {
        chord: "XF86MonBrightnessDown",
        action: HotkeyAction::BrightnessDown,
    },
    Hotkey {
        chord: "XF86Bluetooth",
        action: HotkeyAction::BluetoothToggle,
    },
    Hotkey {
        chord: "Super+Tab",
        action: HotkeyAction::SessionSwitch,
    },
    Hotkey {
        chord: "Super+grave",
        action: HotkeyAction::MonitorFocusSwitch,
    },
    Hotkey {
        chord: "Super+Escape",
        action: HotkeyAction::ReturnToChrome,
    },
    Hotkey {
        chord: "Super+l",
        action: HotkeyAction::Lock,
    },
    Hotkey {
        chord: "Super+s",
        action: HotkeyAction::OpenSystem,
    },
    Hotkey {
        chord: "Super+Space",
        action: HotkeyAction::OpenOmnibox,
    },
];

/// Look up the action bound to a chord label, if any.
#[must_use]
pub fn action_for(chord: &str) -> Option<HotkeyAction> {
    HOTKEYS.iter().find(|h| h.chord == chord).map(|h| h.action)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_chord_is_unique() {
        for (i, a) in HOTKEYS.iter().enumerate() {
            for b in &HOTKEYS[i + 1..] {
                assert_ne!(a.chord, b.chord, "duplicate chord {}", a.chord);
            }
        }
    }

    #[test]
    fn xf86_media_keys_are_host_first_chords_are_not() {
        // The dedicated media/system keys must be host-first (lock 8)…
        assert!(action_for("XF86AudioMute").unwrap().host_first());
        assert!(action_for("XF86AudioPlay").unwrap().host_first());
        assert!(action_for("XF86AudioPause").unwrap().host_first());
        assert!(action_for("XF86AudioStop").unwrap().host_first());
        assert!(action_for("XF86AudioNext").unwrap().host_first());
        assert!(action_for("XF86AudioPrev").unwrap().host_first());
        assert!(action_for("XF86MonBrightnessUp").unwrap().host_first());
        // …and the leader-chord actions must not be (they reach the guest first).
        assert!(!action_for("Super+Tab").unwrap().host_first());
        assert!(!action_for("Super+Escape").unwrap().host_first());
    }

    #[test]
    fn unknown_chord_maps_to_nothing() {
        assert_eq!(action_for("XF86Calculator"), None);
    }

    #[test]
    fn every_action_has_a_nonempty_label() {
        for h in HOTKEYS {
            assert!(!h.action.label().is_empty());
        }
    }
}
