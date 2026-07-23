//! The shell's **hotkey dispatch** (E12-19, Construct host controls; design
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

/// A Super+number navigation target. Slot `0` is the first visible launcher
/// surface; `9` is the tenth (`Super+0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NavSlot(usize);

impl NavSlot {
    /// The zero-based surface index this slot selects.
    pub(crate) const fn index(self) -> usize {
        self.0
    }
}

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
    /// `XF86AudioPlay`.
    PlayPause,
    /// `XF86AudioPause`.
    Pause,
    /// `XF86AudioStop`.
    Stop,
    /// `XF86AudioNext`.
    Next,
    /// `XF86AudioPrev`.
    Previous,
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
            Self::PlayPause => "XF86AudioPlay",
            Self::Pause => "XF86AudioPause",
            Self::Stop => "XF86AudioStop",
            Self::Next => "XF86AudioNext",
            Self::Previous => "XF86AudioPrev",
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
        207 | 164 | 200 => HostKey::Media(MediaKey::PlayPause),
        201 => HostKey::Media(MediaKey::Pause),
        166 => HostKey::Media(MediaKey::Stop),
        163 => HostKey::Media(MediaKey::Next),
        165 => HostKey::Media(MediaKey::Previous),
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
        egui::Key::Space => "Super+Space",
        _ => return None,
    })
}

/// The stateful hotkey dispatcher: it carries only the leader latch (armed while a
/// Super key is held) and turns each frame's raw input into the matched typed
/// actions, applying lock 8. Pure + headless-testable — the shell owns the actual
/// seat / nav effects each action drives.
///
/// A clean Super **tap** (press then release with no leader chord used in between)
/// returns to Springboard/Spotlight; a Super **hold** used as a leader chord
/// (Super+Tab, Super+L, …) never does. The router disambiguates tap-vs-hold with
/// [`Self::leader_used`] and latches the tap for [`Self::take_super_tap`].
#[derive(Debug, Default)]
pub(crate) struct HotkeyRouter {
    /// Whether the leader (Super) is currently held — arms the leader chords.
    leader: bool,
    /// Whether a leader chord actually fired during the current Super hold — set
    /// when a named leader key resolves, cleared on the rising edge of a fresh
    /// Super press. Distinguishes a Super *hold* (a leader) from a clean *tap*
    /// (the Springboard tap action).
    leader_used: bool,
    /// Latched `true` on a clean Super-tap release; drained by
    /// [`Self::take_super_tap`]. A leader-chord hold never sets it.
    super_tap: bool,
    /// Latched Super+number navigation request. Drained by the shell after normal
    /// hotkey actions so the existing typed host-action table stays closed.
    nav_slot: Option<NavSlot>,
}

impl HotkeyRouter {
    /// Whether the leader is currently held (the reserved-chord state).
    #[cfg(test)]
    pub(crate) const fn leader_armed(&self) -> bool {
        self.leader
    }

    /// Drain the clean-Super-tap latch exactly once. A leader hold never sets it.
    pub(crate) fn take_super_tap(&mut self) -> bool {
        std::mem::take(&mut self.super_tap)
    }

    /// Drain a Super+number navigation request, if one fired this frame.
    pub(crate) fn take_nav_slot(&mut self) -> Option<NavSlot> {
        self.nav_slot.take()
    }

    /// Fold one forwarded host-key scan: a media key is host-first (always yields
    /// its action, lock 8); a leader transition updates the latch and yields
    /// nothing itself — but a clean Super **tap** (a release with no leader chord
    /// used) latches the Springboard tap action.
    fn on_host_key(&mut self, scan: HostScan) -> Option<HotkeyAction> {
        match decode_scan(scan)? {
            HostKey::Media(m) => Some(m.action()),
            HostKey::Leader(true) => {
                // Rising edge of a Super press: arm the leader and start a fresh
                // tap-vs-hold watch. Guard the reset to the rising edge so a
                // key-repeat press mid-hold can't re-arm the tap after a chord
                // already fired.
                if !self.leader {
                    self.leader_used = false;
                }
                self.leader = true;
                None
            }
            HostKey::Leader(false) => {
                // Release: a clean tap returns to the shell launcher; a hold used
                // as a leader just disarms.
                self.leader = false;
                if !self.leader_used {
                    self.super_tap = true;
                }
                None
            }
        }
    }

    /// Fold one egui key press: a leader-chord named key fires its action **only**
    /// while the leader is held (lock 8 — otherwise it reaches the focused guest).
    /// A firing chord also marks the current Super hold as *used* so its release
    /// is a hold, not a shell-launcher tap. A number key resolves to a
    /// nav slot through [`nav_slot_for`], which reads the press's Shift bit to
    /// pick the tier (REACH-2).
    fn on_egui_key(&mut self, press: KeyPress) -> Option<HotkeyAction> {
        if !self.leader {
            return None;
        }
        if let Some(slot) = nav_slot_for(press.key, press.shift) {
            self.leader_used = true;
            self.nav_slot = Some(slot);
            return None;
        }
        let action = leader_chord(press.key).and_then(action_for);
        if action.is_some() {
            self.leader_used = true;
        }
        action
    }

    /// The per-frame dispatch: drain the seat's forwarded host keys (media +
    /// leader), then fold this frame's egui key presses against the leader latch.
    /// Returns every matched typed action, in input order — the shell applies each
    /// to its seat / nav. Host keys are processed first so a same-frame
    /// `Super`+named-key chord sees the freshly-armed latch.
    pub(crate) fn dispatch(
        &mut self,
        host_keys: &[HostScan],
        egui_presses: &[KeyPress],
    ) -> Vec<HotkeyAction> {
        let mut actions = Vec::new();
        for scan in host_keys {
            if let Some(a) = self.on_host_key(*scan) {
                actions.push(a);
            }
        }
        for press in egui_presses {
            if let Some(a) = self.on_egui_key(*press) {
                actions.push(a);
            }
        }
        actions
    }
}

/// Map a leader-held number key (+ its Shift state) to the surface slot it
/// selects. Two tiers cover **all 20** `Surface::ALL` entries (REACH-2):
///
/// * plain **`Super`+`1`…`9`/`0`** → Springboard slots 0…=9
///   (`Super+0` = the tenth slot);
/// * **`Super`+`Shift`+`1`…`9`/`0`** → `Surface::ALL[10..=19]` — the ten surfaces
///   beyond the first ten (`Super+Shift+0` = the twentieth slot, `ALL[19]`, the
///   Communications hub that WL-FUNC-011 landed in the slot the prior 19-surface
///   set left open).
///
/// A slot past the last surface (only reachable if `Surface::ALL` shrinks below
/// 20) is handled bounds-safely: the [`NavSlot`] consumer (`apply_nav_slot`)
/// indexes `Surface::ALL` with `.get`, so an overshoot is a no-op, never a panic
/// or a wrap.
const fn nav_slot_for(key: egui::Key, shift: bool) -> Option<NavSlot> {
    let digit = match key {
        egui::Key::Num1 => 0,
        egui::Key::Num2 => 1,
        egui::Key::Num3 => 2,
        egui::Key::Num4 => 3,
        egui::Key::Num5 => 4,
        egui::Key::Num6 => 5,
        egui::Key::Num7 => 6,
        egui::Key::Num8 => 7,
        egui::Key::Num9 => 8,
        egui::Key::Num0 => 9,
        _ => return None,
    };
    // Shift shifts to the second tier: its ten-surface offset lands Num1..Num9 on
    // ALL[10..=18] and the shifted Num0 on ALL[19] (the twentieth surface).
    Some(NavSlot(if shift { digit + 10 } else { digit }))
}

/// One egui key **press** with the **Shift** state that came with it. Shift is
/// the only egui-side modifier the router reads — the Super leader arrives
/// host-first (evdev 125/126, [`decode_scan`]), so a chord is the leader latch
/// crossed with a `(key, shift)` press. Shift selects the **second** Super-number
/// nav tier (REACH-2): `Super+1..0` reaches `Surface::ALL[0..=9]`,
/// `Super+Shift+1..9/0` reaches `ALL[10..=19]`, so all 20 surfaces are reachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KeyPress {
    /// The pressed egui key.
    pub(crate) key: egui::Key,
    /// Whether Shift was held for this press.
    pub(crate) shift: bool,
}

/// The egui key **presses** in this frame's input (a press, not a release), the
/// leader-chord half of the dispatch input, each carrying its Shift bit (the nav
/// tier selector, REACH-2). Kept tiny so the shell's render can build it inline
/// from `ctx.input`.
pub(crate) fn egui_key_presses(events: &[egui::Event]) -> Vec<KeyPress> {
    events
        .iter()
        .filter_map(|e| match e {
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => Some(KeyPress {
                key: *key,
                shift: modifiers.shift,
            }),
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

    /// A plain (unshifted) egui key press.
    fn press(key: egui::Key) -> KeyPress {
        KeyPress { key, shift: false }
    }

    /// A Shift-held egui key press — the second Super-number nav tier (REACH-2).
    fn shift_press(key: egui::Key) -> KeyPress {
        KeyPress { key, shift: true }
    }

    #[test]
    fn every_media_chord_resolves_in_the_fixed_table_and_is_host_first() {
        for m in [
            MediaKey::VolumeUp,
            MediaKey::VolumeDown,
            MediaKey::Mute,
            MediaKey::PlayPause,
            MediaKey::Pause,
            MediaKey::Stop,
            MediaKey::Next,
            MediaKey::Previous,
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
        let acts = r.dispatch(&[scan(207, true)], &[]);
        assert_eq!(acts, vec![HotkeyAction::MediaPlayPause]);
        let acts = r.dispatch(&[scan(164, true)], &[]);
        assert_eq!(acts, vec![HotkeyAction::MediaPlayPause]);
        let acts = r.dispatch(&[scan(201, true)], &[]);
        assert_eq!(acts, vec![HotkeyAction::MediaPause]);
        let acts = r.dispatch(&[scan(166, true)], &[]);
        assert_eq!(acts, vec![HotkeyAction::MediaStop]);
        let acts = r.dispatch(&[scan(163, true)], &[]);
        assert_eq!(acts, vec![HotkeyAction::MediaNext]);
        let acts = r.dispatch(&[scan(165, true)], &[]);
        assert_eq!(acts, vec![HotkeyAction::MediaPrevious]);
    }

    #[test]
    fn a_named_key_reaches_the_guest_until_the_leader_is_held() {
        let mut r = HotkeyRouter::default();
        // Bare Tab: no action — it reaches the focused guest (lock 8).
        assert!(r.dispatch(&[], &[press(egui::Key::Tab)]).is_empty());
        assert!(!r.leader_armed());

        // Press the leader (Super, evdev 125), then Tab in the same frame → the
        // session-switch chord fires; the guest never sees the Tab.
        let acts = r.dispatch(&[scan(125, true)], &[press(egui::Key::Tab)]);
        assert_eq!(acts, vec![HotkeyAction::SessionSwitch]);
        assert!(r.leader_armed());

        // Still armed on a later frame: L → Lock, S → OpenSystem.
        assert_eq!(
            r.dispatch(&[], &[press(egui::Key::L)]),
            vec![HotkeyAction::Lock]
        );
        assert_eq!(
            r.dispatch(&[], &[press(egui::Key::S)]),
            vec![HotkeyAction::OpenSystem]
        );

        // Release the leader → the chord disarms; Tab reaches the guest again.
        assert!(r
            .dispatch(&[scan(125, false)], &[press(egui::Key::Tab)])
            .is_empty());
        assert!(!r.leader_armed());
    }

    #[test]
    fn the_full_leader_chord_set_maps_to_its_typed_actions() {
        let mut r = HotkeyRouter::default();
        let _ = r.dispatch(&[scan(126, true)], &[]); // right-Super arms too
        assert_eq!(
            r.dispatch(&[], &[press(egui::Key::Backtick)]),
            vec![HotkeyAction::MonitorFocusSwitch]
        );
        assert_eq!(
            r.dispatch(&[], &[press(egui::Key::Escape)]),
            vec![HotkeyAction::ReturnToChrome]
        );
        assert_eq!(
            r.dispatch(&[], &[press(egui::Key::Space)]),
            vec![HotkeyAction::OpenOmnibox]
        );
    }

    #[test]
    fn super_numbers_latch_springboard_navigation_slots_without_triggering_home() {
        let mut r = HotkeyRouter::default();

        let acts = r.dispatch(&[scan(125, true)], &[press(egui::Key::Num1)]);
        assert!(acts.is_empty(), "Super+1 is shell nav, not a host action");
        assert_eq!(r.take_nav_slot().map(NavSlot::index), Some(0));
        assert!(
            r.take_nav_slot().is_none(),
            "the nav slot latch drains exactly once"
        );
        let _ = r.dispatch(&[scan(125, false)], &[]);
        assert!(
            !r.take_super_tap(),
            "a Super+number hold is not a clean Super tap"
        );

        let _ = r.dispatch(&[scan(125, true)], &[press(egui::Key::Num0)]);
        assert_eq!(
            r.take_nav_slot().map(NavSlot::index),
            Some(9),
            "Super+0 maps to the tenth visible slot"
        );
    }

    #[test]
    fn a_clean_super_tap_latches_shell_home_but_a_leader_hold_does_not() {
        // A clean tap returns to shell home; a Super hold used as a leader never
        // does, so the two Super roles coexist.
        let mut r = HotkeyRouter::default();

        // Press then release Super with nothing in between → a clean tap → toggle.
        let _ = r.dispatch(&[scan(125, true)], &[]);
        assert!(!r.take_super_tap(), "no action until the tap completes");
        let _ = r.dispatch(&[scan(125, false)], &[]);
        assert!(r.take_super_tap(), "a clean Super tap latches shell home");
        assert!(
            !r.take_super_tap(),
            "the Super-tap latch drains exactly once"
        );

        // A Super *hold* used as a leader (Super+Tab) fires the chord and must NOT
        // trigger shell home on release.
        let acts = r.dispatch(&[scan(125, true)], &[press(egui::Key::Tab)]);
        assert_eq!(acts, vec![HotkeyAction::SessionSwitch]);
        let _ = r.dispatch(&[scan(125, false)], &[]);
        assert!(
            !r.take_super_tap(),
            "a leader-chord hold never triggers shell home"
        );

        // A fresh clean tap after a hold re-arms (the rising edge clears the
        // used-flag) and toggles again.
        let _ = r.dispatch(&[scan(125, true)], &[]);
        let _ = r.dispatch(&[scan(125, false)], &[]);
        assert!(r.take_super_tap(), "a fresh Super tap re-arms");

        // A same-frame press+release (a very quick tap) still toggles.
        let _ = r.dispatch(&[scan(125, true), scan(125, false)], &[]);
        assert!(
            r.take_super_tap(),
            "a same-frame Super tap latches shell home"
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
    fn egui_key_presses_keeps_presses_with_their_shift_bit_and_drops_the_rest() {
        let shifted = egui::Modifiers {
            shift: true,
            ..Default::default()
        };
        let events = vec![
            egui::Event::Key {
                key: egui::Key::L,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            },
            // A Shift-held press carries its shift bit through (REACH-2 tier 2).
            egui::Event::Key {
                key: egui::Key::Num1,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: shifted,
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
        assert_eq!(
            egui_key_presses(&events),
            vec![press(egui::Key::L), shift_press(egui::Key::Num1)]
        );
    }

    #[test]
    fn super_shift_numbers_reach_the_second_surface_tier() {
        // REACH-2 — Super+Shift+1..9 selects Surface::ALL[10..=18], the nine
        // surfaces the plain Super+1..0 tier can't reach.
        for (key, want) in [
            (egui::Key::Num1, 10),
            (egui::Key::Num2, 11),
            (egui::Key::Num3, 12),
            (egui::Key::Num4, 13),
            (egui::Key::Num5, 14),
            (egui::Key::Num6, 15),
            (egui::Key::Num7, 16),
            (egui::Key::Num8, 17),
            (egui::Key::Num9, 18),
        ] {
            let mut r = HotkeyRouter::default();
            let acts = r.dispatch(&[scan(125, true)], &[shift_press(key)]);
            assert!(
                acts.is_empty(),
                "Super+Shift+num is shell nav, not a host action"
            );
            assert_eq!(
                r.take_nav_slot().map(NavSlot::index),
                Some(want),
                "Super+Shift+{key:?} → Surface::ALL[{want}]"
            );
        }
    }

    #[test]
    fn every_surface_index_is_keyboard_reachable_across_both_super_number_tiers() {
        use std::collections::BTreeSet;

        // Sweep both tiers and collect every surface index a chord can select.
        let tier1 = [
            (egui::Key::Num1, false),
            (egui::Key::Num2, false),
            (egui::Key::Num3, false),
            (egui::Key::Num4, false),
            (egui::Key::Num5, false),
            (egui::Key::Num6, false),
            (egui::Key::Num7, false),
            (egui::Key::Num8, false),
            (egui::Key::Num9, false),
            (egui::Key::Num0, false),
        ];
        let tier2 = [
            (egui::Key::Num1, true),
            (egui::Key::Num2, true),
            (egui::Key::Num3, true),
            (egui::Key::Num4, true),
            (egui::Key::Num5, true),
            (egui::Key::Num6, true),
            (egui::Key::Num7, true),
            (egui::Key::Num8, true),
            (egui::Key::Num9, true),
            // Super+Shift+0 → slot 19 (an overshoot no-op after the WL-FUNC-011
            // Phase-2 17-surface cutover; slots 17..=19 resolve to no surface).
            (egui::Key::Num0, true),
        ];
        let mut reached = BTreeSet::new();
        for (key, shift) in tier1.into_iter().chain(tier2) {
            let mut r = HotkeyRouter::default();
            let kp = if shift { shift_press(key) } else { press(key) };
            let _ = r.dispatch(&[scan(125, true)], &[kp]);
            let slot = r
                .take_nav_slot()
                .expect("a Super-number chord latches a slot");
            reached.insert(slot.index());
        }
        // REACH-2 provides 20 chord slots across two Super-number tiers. After the
        // WL-FUNC-011 Phase-2 cutover 17 surfaces remain, occupying slots 0..=16, so
        // every surface index stays keyboard-reachable; the three overshoot chords
        // (slots 17..=19) latch a slot that resolves to no surface — a safe no-op
        // (see `a_slot_past_the_last_surface_is_a_safe_no_op`).
        let all: BTreeSet<usize> = (0..crate::surfaces::Surface::ALL.len()).collect();
        assert_eq!(all.len(), 17, "Springboard has 17 surface tiles");
        assert!(
            all.is_subset(&reached),
            "every Surface::ALL index has a Super-number chord"
        );
    }

    #[test]
    fn a_slot_past_the_last_surface_is_a_safe_no_op() {
        // REACH-2 provides 20 chord slots across two Super-number tiers. After the
        // WL-FUNC-011 Phase-2 cutover only 17 surfaces remain (slots 0..=16), so the
        // top tier-2 chords overshoot: Super+Shift+0 latches slot 19, which now
        // resolves to NO surface. Prove the chord still latches that slot and
        // consumes the hold (so releasing does not toggle the dock)…
        let mut r = HotkeyRouter::default();
        let _ = r.dispatch(&[scan(125, true)], &[shift_press(egui::Key::Num0)]);
        let slot = r.take_nav_slot().expect("Super+Shift+0 latches a slot");
        assert_eq!(slot.index(), 19, "Super+Shift+0 → slot 19");
        assert!(
            crate::surfaces::springboard_surface(slot.index()).is_none(),
            "slot 19 overshoots the 17-surface set — resolves to no surface",
        );
        // It consumed the hold, so releasing does not trigger shell home.
        let _ = r.dispatch(&[scan(125, false)], &[]);
        assert!(
            !r.take_super_tap(),
            "a Super+Shift+0 chord is a hold, not a clean Super tap"
        );
        // The consumer stays bounds-safe for any slot past the last surface: the
        // `apply_nav_slot` `.get` indexing yields nothing — a no-op, never a panic.
        let past_end = NavSlot(crate::surfaces::Surface::ALL.len());
        assert!(
            crate::surfaces::springboard_surface(past_end.index()).is_none(),
            "a slot past the last surface resolves to no surface",
        );
    }
}
