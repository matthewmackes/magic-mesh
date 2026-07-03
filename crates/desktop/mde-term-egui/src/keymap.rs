//! The rebindable keymap (TERM-12) — a config-driven action table over the
//! existing [`Command`] / [`TabCommand`] enums.
//!
//! Terminator's whole default chord set (split V/H, pane navigation, zoom, tab
//! new/next/prev/move, the broadcast toggles, and the TERM-12 pane actions)
//! resolves through **one** table ([`Keymap`]) rather than the hardcoded
//! `consume_key` ladders in `splits`/`tabs` — so every action is fully
//! rebindable from config. Each [`Chord`] parses from / prints to a
//! human-editable string (`"Ctrl+Shift+O"`), reusing egui's own
//! [`Key::from_name`] / [`Key::name`] so the config never leaks a discriminant.
//!
//! Glue (§6): the live decode still runs through egui's
//! [`InputState::consume_key`] exactly as the old ladders did — the table only
//! *drives* which chord maps to which action, and the winit `Ctrl+Shift+X →
//! Event::Cut` zoom wrinkle is preserved. Split-level actions fold to
//! [`Command`] and tab-level actions to [`TabCommand`]; the three TERM-12 pane
//! actions ([`Action::RenamePane`], the two watch toggles) have no legacy enum
//! and are dispatched directly by the surface.

use std::collections::BTreeMap;

use mde_egui::egui::{Context, Event, Key, Modifiers};

use crate::splits::{Broadcast, Command, NavDir, SplitDir};
use crate::tabs::TabCommand;

/// Every rebindable surface action. A superset of the split ([`Command`]) and
/// tab ([`TabCommand`]) commands plus the TERM-12 pane actions.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Action {
    /// Split the focused pane horizontally (a horizontal divider).
    SplitHorizontal,
    /// Split the focused pane vertically (a vertical divider).
    SplitVertical,
    /// Close the focused pane.
    ClosePane,
    /// Maximize the focused pane, or restore the tiling.
    ToggleZoom,
    /// Move focus to the pane left of the focused one.
    FocusLeft,
    /// Move focus to the pane right of the focused one.
    FocusRight,
    /// Move focus to the pane above the focused one.
    FocusUp,
    /// Move focus to the pane below the focused one.
    FocusDown,
    /// Toggle broadcasting typed input to every pane.
    BroadcastAll,
    /// Toggle broadcasting typed input to the focused pane's group.
    BroadcastGroup,
    /// Open a fresh tab and focus it.
    TabNew,
    /// Activate the next tab, wrapping.
    TabNext,
    /// Activate the previous tab, wrapping.
    TabPrev,
    /// Move the active tab one place left.
    TabMoveLeft,
    /// Move the active tab one place right.
    TabMoveRight,
    /// Toggle the remote "new terminal on → peer" picker.
    ToggleRemote,
    /// Toggle the saved-layouts overlay.
    ToggleLayouts,
    /// Toggle the appearance picker.
    ToggleAppearance,
    /// Begin renaming the focused pane's title (TERM-12).
    RenamePane,
    /// Toggle watch-for-activity on the focused pane (TERM-12).
    ToggleActivityWatch,
    /// Toggle watch-for-silence on the focused pane (TERM-12).
    ToggleSilenceWatch,
}

impl Action {
    /// Every action, in a stable order (for config round-trips + a settings UI).
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[
            Self::SplitHorizontal,
            Self::SplitVertical,
            Self::ClosePane,
            Self::ToggleZoom,
            Self::FocusLeft,
            Self::FocusRight,
            Self::FocusUp,
            Self::FocusDown,
            Self::BroadcastAll,
            Self::BroadcastGroup,
            Self::TabNew,
            Self::TabNext,
            Self::TabPrev,
            Self::TabMoveLeft,
            Self::TabMoveRight,
            Self::ToggleRemote,
            Self::ToggleLayouts,
            Self::ToggleAppearance,
            Self::RenamePane,
            Self::ToggleActivityWatch,
            Self::ToggleSilenceWatch,
        ]
    }

    /// The stable config name for this action.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::SplitHorizontal => "split_horizontal",
            Self::SplitVertical => "split_vertical",
            Self::ClosePane => "close_pane",
            Self::ToggleZoom => "toggle_zoom",
            Self::FocusLeft => "focus_left",
            Self::FocusRight => "focus_right",
            Self::FocusUp => "focus_up",
            Self::FocusDown => "focus_down",
            Self::BroadcastAll => "broadcast_all",
            Self::BroadcastGroup => "broadcast_group",
            Self::TabNew => "tab_new",
            Self::TabNext => "tab_next",
            Self::TabPrev => "tab_prev",
            Self::TabMoveLeft => "tab_move_left",
            Self::TabMoveRight => "tab_move_right",
            Self::ToggleRemote => "toggle_remote",
            Self::ToggleLayouts => "toggle_layouts",
            Self::ToggleAppearance => "toggle_appearance",
            Self::RenamePane => "rename_pane",
            Self::ToggleActivityWatch => "toggle_activity_watch",
            Self::ToggleSilenceWatch => "toggle_silence_watch",
        }
    }

    /// Resolve a config name back to its action.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::all().iter().copied().find(|a| a.name() == name)
    }

    /// The split-level [`Command`] this action folds to, if any.
    #[must_use]
    pub const fn as_command(self) -> Option<Command> {
        Some(match self {
            Self::SplitHorizontal => Command::Split(SplitDir::H),
            Self::SplitVertical => Command::Split(SplitDir::V),
            Self::ClosePane => Command::Close,
            Self::ToggleZoom => Command::ToggleZoom,
            Self::FocusLeft => Command::Focus(NavDir::Left),
            Self::FocusRight => Command::Focus(NavDir::Right),
            Self::FocusUp => Command::Focus(NavDir::Up),
            Self::FocusDown => Command::Focus(NavDir::Down),
            Self::BroadcastAll => Command::ToggleBroadcast(Broadcast::All),
            Self::BroadcastGroup => Command::ToggleBroadcast(Broadcast::Group),
            _ => return None,
        })
    }

    /// The tab-level [`TabCommand`] this action folds to, if any.
    #[must_use]
    pub const fn as_tab_command(self) -> Option<TabCommand> {
        Some(match self {
            Self::TabNew => TabCommand::New,
            Self::TabNext => TabCommand::Next,
            Self::TabPrev => TabCommand::Prev,
            Self::TabMoveLeft => TabCommand::MoveLeft,
            Self::TabMoveRight => TabCommand::MoveRight,
            Self::ToggleRemote => TabCommand::ToggleRemote,
            Self::ToggleLayouts => TabCommand::ToggleLayouts,
            Self::ToggleAppearance => TabCommand::ToggleAppearance,
            _ => return None,
        })
    }
}

/// A key chord: a logical [`Key`] plus its modifier flags. Serializes to a
/// human-editable string via [`Chord::parse`] / [`Chord::to_string`].
// A chord genuinely is a key plus the four independent modifier flags; the
// bool-per-modifier shape is what keeps parse/Display + `Hash` trivial.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Chord {
    /// The logical key.
    pub key: Key,
    /// Ctrl held.
    pub ctrl: bool,
    /// Shift held.
    pub shift: bool,
    /// Alt / Option held.
    pub alt: bool,
    /// Cmd / Super / Meta held.
    pub command: bool,
}

impl Chord {
    /// A bare-key chord (no modifiers).
    #[must_use]
    pub const fn key(key: Key) -> Self {
        Self {
            key,
            ctrl: false,
            shift: false,
            alt: false,
            command: false,
        }
    }

    /// This chord with Ctrl added.
    #[must_use]
    pub const fn ctrl(mut self) -> Self {
        self.ctrl = true;
        self
    }

    /// This chord with Shift added.
    #[must_use]
    pub const fn shift(mut self) -> Self {
        self.shift = true;
        self
    }

    /// This chord with Alt added.
    #[must_use]
    pub const fn alt(mut self) -> Self {
        self.alt = true;
        self
    }

    /// The egui [`Modifiers`] this chord matches against (never `mac_cmd`; the
    /// `command` flag already unifies Ctrl-on-mac).
    #[must_use]
    pub const fn modifiers(self) -> Modifiers {
        Modifiers {
            alt: self.alt,
            ctrl: self.ctrl,
            shift: self.shift,
            mac_cmd: false,
            command: self.command,
        }
    }

    /// How many modifiers this chord carries — used to consume the *most
    /// specific* chords first (egui's `consume_key` matches modifier subsets, so
    /// a `Ctrl` pattern would otherwise steal a `Ctrl+Shift` event).
    #[must_use]
    pub fn specificity(self) -> u32 {
        u32::from(self.ctrl) + u32::from(self.shift) + u32::from(self.alt) + u32::from(self.command)
    }

    /// Parse a chord string such as `"Ctrl+Shift+O"` or `"Alt+Left"`. Modifier
    /// tokens are case-insensitive; the final token is the key (any spelling
    /// [`Key::from_name`] accepts).
    ///
    /// # Errors
    /// A human-readable message when a token isn't a known modifier or the final
    /// token isn't a known key.
    pub fn parse(s: &str) -> Result<Self, String> {
        let parts: Vec<&str> = s
            .split('+')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect();
        let Some((key_str, mods)) = parts.split_last() else {
            return Err(format!("empty key chord: {s:?}"));
        };
        let key = Key::from_name(key_str)
            .or_else(|| Key::from_name(&key_str.to_ascii_uppercase()))
            .ok_or_else(|| format!("unknown key {key_str:?} in chord {s:?}"))?;
        let mut chord = Self {
            key,
            ctrl: false,
            shift: false,
            alt: false,
            command: false,
        };
        for part in mods {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => chord.ctrl = true,
                "shift" => chord.shift = true,
                "alt" | "option" => chord.alt = true,
                "cmd" | "command" | "super" | "meta" | "win" => chord.command = true,
                other => return Err(format!("unknown modifier {other:?} in chord {s:?}")),
            }
        }
        Ok(chord)
    }
}

impl std::fmt::Display for Chord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.ctrl {
            write!(f, "Ctrl+")?;
        }
        if self.alt {
            write!(f, "Alt+")?;
        }
        if self.shift {
            write!(f, "Shift+")?;
        }
        if self.command {
            write!(f, "Cmd+")?;
        }
        write!(f, "{}", self.key.name())
    }
}

/// The rebindable action table: a set of `(chord → action)` bindings.
#[derive(Clone, Debug)]
pub struct Keymap {
    bindings: Vec<(Chord, Action)>,
}

impl Default for Keymap {
    /// Terminator's default chord set (design lock Q15) — the exact chords the
    /// hardcoded `splits`/`tabs` ladders used, now one editable table.
    fn default() -> Self {
        use Action as A;
        let c = |k: Key| Chord::key(k).ctrl();
        let cs = |k: Key| Chord::key(k).ctrl().shift();
        let alt = |k: Key| Chord::key(k).alt();
        Self {
            bindings: vec![
                (cs(Key::O), A::SplitHorizontal),
                (cs(Key::E), A::SplitVertical),
                (cs(Key::W), A::ClosePane),
                (cs(Key::X), A::ToggleZoom),
                // Terminator zoom muscle-memory: Ctrl+Shift+Z also zooms.
                (cs(Key::Z), A::ToggleZoom),
                (alt(Key::ArrowLeft), A::FocusLeft),
                (alt(Key::ArrowRight), A::FocusRight),
                (alt(Key::ArrowUp), A::FocusUp),
                (alt(Key::ArrowDown), A::FocusDown),
                (cs(Key::A), A::BroadcastAll),
                (cs(Key::G), A::BroadcastGroup),
                (cs(Key::T), A::TabNew),
                (c(Key::PageDown), A::TabNext),
                (c(Key::PageUp), A::TabPrev),
                (cs(Key::PageUp), A::TabMoveLeft),
                (cs(Key::PageDown), A::TabMoveRight),
                (cs(Key::R), A::ToggleRemote),
                (cs(Key::L), A::ToggleLayouts),
                (cs(Key::P), A::ToggleAppearance),
                (cs(Key::I), A::RenamePane),
                (cs(Key::U), A::ToggleActivityWatch),
                (cs(Key::Y), A::ToggleSilenceWatch),
            ],
        }
    }
}

impl Keymap {
    /// The action this exact chord maps to (all modifiers equal), if any.
    #[must_use]
    pub fn resolve(&self, chord: &Chord) -> Option<Action> {
        self.bindings
            .iter()
            .find(|(c, _)| c == chord)
            .map(|(_, a)| *a)
    }

    /// The chord currently bound to `action`, if any (the first, when an action
    /// carries more than one default chord).
    #[must_use]
    pub fn binding_for(&self, action: Action) -> Option<Chord> {
        self.bindings
            .iter()
            .find(|(_, a)| *a == action)
            .map(|(c, _)| *c)
    }

    /// Rebind `action` to `chord`: every existing binding of `action` is dropped,
    /// any other action already on `chord` is displaced, and the new binding is
    /// installed. So the old chord stops resolving and `chord` resolves only to
    /// `action`.
    pub fn rebind(&mut self, action: Action, chord: Chord) {
        self.bindings.retain(|(c, a)| *a != action && *c != chord);
        self.bindings.push((chord, action));
    }

    /// Apply a [`KeymapConfig`] over the defaults — each entry rebinds its action.
    /// Unparsable entries are collected and returned (the rest still apply), so a
    /// typo in one binding never wipes the map.
    pub fn apply_config(&mut self, config: &KeymapConfig) -> Vec<String> {
        let mut errors = Vec::new();
        for (name, chord_str) in &config.0 {
            let Some(action) = Action::from_name(name) else {
                errors.push(format!("unknown action {name:?} in keymap config"));
                continue;
            };
            match Chord::parse(chord_str) {
                Ok(chord) => self.rebind(action, chord),
                Err(e) => errors.push(e),
            }
        }
        errors
    }

    /// The current bindings as a [`KeymapConfig`] (one chord per action — the
    /// first, for an action with several default chords).
    #[must_use]
    pub fn to_config(&self) -> KeymapConfig {
        let mut map = BTreeMap::new();
        for action in Action::all() {
            if let Some(chord) = self.binding_for(*action) {
                map.insert(action.name().to_owned(), chord.to_string());
            }
        }
        KeymapConfig(map)
    }

    /// Decode and **consume** this frame's chords into their actions, before any
    /// pane widget clones the event stream — a consumed chord never reaches a
    /// shell. Bindings are consumed most-specific-first (see
    /// [`Chord::specificity`]).
    ///
    /// The winit `Ctrl(+Shift)+X → Event::Cut` fold is preserved: when a `Cut`
    /// arrives with Shift held and [`Action::ToggleZoom`] is on a Ctrl+Shift
    /// chord, it is claimed as a zoom (the bare-DRM backend takes the `Key::X`
    /// path instead).
    #[must_use]
    pub fn consume(&self, ctx: &Context) -> Vec<Action> {
        // Most-specific chords first so a subset pattern can't steal a superset
        // event (egui's `consume_key` matches modifier subsets).
        let mut order: Vec<&(Chord, Action)> = self.bindings.iter().collect();
        order.sort_by(|a, b| b.0.specificity().cmp(&a.0.specificity()));

        let zoom_is_ctrl_shift = self
            .binding_for(Action::ToggleZoom)
            .is_some_and(|c| c.ctrl && c.shift);

        ctx.input_mut(|input| {
            let mut actions = Vec::new();
            for (chord, action) in order {
                if input.consume_key(chord.modifiers(), chord.key) {
                    actions.push(*action);
                }
            }
            // The Cut wrinkle (see doc): only when Ctrl+Shift zoom is bound.
            if zoom_is_ctrl_shift {
                let shifted_ctrl =
                    input.modifiers.shift && (input.modifiers.ctrl || input.modifiers.command);
                if shifted_ctrl {
                    let before = input.events.len();
                    input.events.retain(|event| !matches!(event, Event::Cut));
                    if input.events.len() < before {
                        actions.push(Action::ToggleZoom);
                    }
                }
            }
            actions
        })
    }
}

/// A serializable keymap override — a map of action name → chord string, applied
/// over [`Keymap::default`]. Human-editable, and the shell can persist it in the
/// surface's config.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KeymapConfig(pub BTreeMap<String, String>);

impl KeymapConfig {
    /// An empty override (the defaults stand).
    #[must_use]
    pub const fn new() -> Self {
        Self(BTreeMap::new())
    }

    /// Set `action`'s chord in this override.
    #[must_use]
    pub fn with(mut self, action: Action, chord: &str) -> Self {
        self.0.insert(action.name().to_owned(), chord.to_owned());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chord_round_trips_through_its_string() {
        // These are already in `Key::name` canonical form (arrows print as
        // "Left"/"Right"/…), so the printed form is byte-identical.
        for s in ["Ctrl+Shift+O", "Alt+Left", "Ctrl+PageDown", "Ctrl+Shift+I"] {
            let chord = Chord::parse(s).expect("parse");
            assert_eq!(chord.to_string(), s, "round-trip {s}");
            assert_eq!(
                Chord::parse(&chord.to_string()).expect("reparse"),
                chord,
                "re-parsing the printed form is stable"
            );
        }
        // Aliases parse but normalize to the canonical printed form.
        assert_eq!(
            Chord::parse("Alt+ArrowLeft").expect("alias").to_string(),
            "Alt+Left"
        );
    }

    #[test]
    fn chord_parse_is_modifier_case_insensitive() {
        let a = Chord::parse("ctrl+shift+o").expect("lower");
        let b = Chord::parse("Ctrl+Shift+O").expect("mixed");
        assert_eq!(a, b);
        assert!(a.ctrl && a.shift && !a.alt && a.key == Key::O);
    }

    #[test]
    fn chord_parse_rejects_a_bad_token() {
        assert!(Chord::parse("Hyper+O").is_err());
        assert!(Chord::parse("Ctrl+Shift+Nope").is_err());
        assert!(Chord::parse("").is_err());
    }

    #[test]
    fn default_map_resolves_terminator_chords_to_actions() {
        let km = Keymap::default();
        assert_eq!(
            km.resolve(&Chord::parse("Ctrl+Shift+O").unwrap()),
            Some(Action::SplitHorizontal)
        );
        assert_eq!(
            km.resolve(&Chord::parse("Ctrl+Shift+E").unwrap()),
            Some(Action::SplitVertical)
        );
        assert_eq!(
            km.resolve(&Chord::parse("Alt+ArrowRight").unwrap()),
            Some(Action::FocusRight)
        );
        assert_eq!(
            km.resolve(&Chord::parse("Ctrl+PageDown").unwrap()),
            Some(Action::TabNext)
        );
        assert_eq!(
            km.resolve(&Chord::parse("Ctrl+Shift+PageDown").unwrap()),
            Some(Action::TabMoveRight)
        );
    }

    #[test]
    fn actions_fold_to_the_existing_command_enums() {
        assert_eq!(
            Action::SplitVertical.as_command(),
            Some(Command::Split(SplitDir::V))
        );
        assert_eq!(
            Action::BroadcastAll.as_command(),
            Some(Command::ToggleBroadcast(Broadcast::All))
        );
        assert_eq!(Action::TabNew.as_tab_command(), Some(TabCommand::New));
        // A pane-only TERM-12 action folds to neither legacy enum.
        assert_eq!(Action::RenamePane.as_command(), None);
        assert_eq!(Action::RenamePane.as_tab_command(), None);
    }

    #[test]
    fn a_rebind_moves_an_action_and_frees_its_old_chord() {
        let mut km = Keymap::default();
        let old = Chord::parse("Ctrl+Shift+O").unwrap();
        let new = Chord::parse("Ctrl+Alt+H").unwrap();
        assert_eq!(km.resolve(&old), Some(Action::SplitHorizontal));

        km.rebind(Action::SplitHorizontal, new);

        // The new chord resolves; the old one no longer maps to this action.
        assert_eq!(km.resolve(&new), Some(Action::SplitHorizontal));
        assert_eq!(km.resolve(&old), None);
    }

    #[test]
    fn rebinding_a_chord_displaces_its_previous_action() {
        let mut km = Keymap::default();
        let close = Chord::parse("Ctrl+Shift+W").unwrap();
        // Point the close chord at a split instead.
        km.rebind(Action::SplitVertical, close);
        assert_eq!(km.resolve(&close), Some(Action::SplitVertical));
        // ClosePane lost its only chord — it now resolves nowhere.
        assert_eq!(km.binding_for(Action::ClosePane), None);
    }

    #[test]
    fn a_config_override_rebinds_over_the_defaults() {
        let config = KeymapConfig::new()
            .with(Action::SplitHorizontal, "Ctrl+Alt+H")
            .with(Action::ToggleActivityWatch, "Ctrl+Shift+M");
        let mut km = Keymap::default();
        let errors = km.apply_config(&config);
        assert!(errors.is_empty(), "clean config applies: {errors:?}");

        assert_eq!(
            km.resolve(&Chord::parse("Ctrl+Alt+H").unwrap()),
            Some(Action::SplitHorizontal)
        );
        assert_eq!(km.resolve(&Chord::parse("Ctrl+Shift+O").unwrap()), None);
        assert_eq!(
            km.resolve(&Chord::parse("Ctrl+Shift+M").unwrap()),
            Some(Action::ToggleActivityWatch)
        );
    }

    #[test]
    fn a_bad_config_entry_is_reported_without_wiping_the_map() {
        let mut config = KeymapConfig::new().with(Action::SplitVertical, "Ctrl+Alt+V");
        config
            .0
            .insert("not_an_action".to_owned(), "Ctrl+Z".to_owned());
        config
            .0
            .insert(Action::ClosePane.name().to_owned(), "Ctrl+Bogus".to_owned());

        let mut km = Keymap::default();
        let errors = km.apply_config(&config);
        assert_eq!(errors.len(), 2, "the unknown action + the bad chord");
        // The good entry still landed.
        assert_eq!(
            km.resolve(&Chord::parse("Ctrl+Alt+V").unwrap()),
            Some(Action::SplitVertical)
        );
    }
}
