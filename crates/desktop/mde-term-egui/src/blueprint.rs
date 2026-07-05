//! **Session blueprint** — the shared layout + seeded-command recipe a
//! TMUX-FC-5 template ("project") or a TMUX-FC-7 mesh preset opens.
//!
//! Design: `docs/design/tmux-first-class.md` (#5 templates, #11/#13 presets).
//! Both features want the same thing — "seed a layout + commands" — so the
//! recipe and the command emission live once here (§6 — glue, one home), and the
//! two features differ only in where the [`Blueprint`] comes from: a template is
//! a user-authored, persisted blueprint ([`crate::tmux_store`]); a preset is a
//! built-in one ([`crate::presets_tmux`]).
//!
//! ## The build, deterministically
//!
//! [`Blueprint::commands`] turns a recipe into the exact `tmux` command lines the
//! control client writes. It builds a **detached** session first
//! (`new-session -d`), seeds each window and pane, and only then `switch-client`s
//! the control client onto it — so the client sees one clean `%session-changed`
//! after the whole layout exists, and the `%`-events reconcile
//! [`crate::tmux::TmuxModel`] the same round-trip way every other op does (never
//! a direct tree edit).
//!
//! Panes are addressed by their window's **name** (`session:window`), and each
//! pane's command is seeded into the window's *active* pane the instant it is
//! created (right after its `split-window`), so the recipe never depends on
//! tmux pane ids (assigned + reported asynchronously) or on a particular
//! `base-index`/`pane-base-index` — it is a fully deterministic, unit-tested
//! string sequence.

use serde::{Deserialize, Serialize};

use crate::splits::SplitDir;
use crate::tmux::{commands, StockLayout};

/// One pane in a blueprint window: an optional command line seeded into it (the
/// pane runs it on open; `None` leaves a bare shell).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlueprintPane {
    /// The command typed + entered when the pane opens (`None` = a bare shell).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

impl BlueprintPane {
    /// A pane seeded with `command`.
    #[must_use]
    pub fn cmd(command: impl Into<String>) -> Self {
        Self {
            command: Some(command.into()),
        }
    }

    /// A bare-shell pane (no seeded command).
    #[must_use]
    pub const fn shell() -> Self {
        Self { command: None }
    }
}

/// One window in a blueprint: a name, its panes (at least one), how each extra
/// pane splits off the previous, and an optional final tidy layout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlueprintWindow {
    /// The window name (its `session:window` target + tab-strip label).
    pub name: String,
    /// The panes, in creation order (the first is the window's original pane).
    pub panes: Vec<BlueprintPane>,
    /// How each extra pane splits off: a native [`SplitDir::V`] (beside) is tmux
    /// `-h`, [`SplitDir::H`] (stacked) tmux `-v`.
    pub split: SplitDir,
    /// A final `select-layout` to even the panes out (`None` keeps tmux's
    /// as-split arrangement).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<StockLayout>,
}

impl BlueprintWindow {
    /// A window named `name` with `panes`, splitting `split`, tidied by `layout`.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        panes: Vec<BlueprintPane>,
        split: SplitDir,
        layout: Option<StockLayout>,
    ) -> Self {
        Self {
            name: name.into(),
            panes,
            split,
            layout,
        }
    }
}

/// A whole session recipe — the windows to build, in strip order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blueprint {
    /// The windows to open (at least one; the first seeds the `new-session`).
    pub windows: Vec<BlueprintWindow>,
}

impl Blueprint {
    /// A blueprint of `windows`.
    #[must_use]
    pub const fn new(windows: Vec<BlueprintWindow>) -> Self {
        Self { windows }
    }

    /// The total pane count across every window (a template editor's live tally).
    #[must_use]
    pub fn pane_count(&self) -> usize {
        self.windows.iter().map(|w| w.panes.len()).sum()
    }

    /// The exact `tmux` command lines that build this blueprint as session
    /// `session` and switch the control client onto it.
    ///
    /// Empty when the blueprint has no window (nothing to build — an honest
    /// no-op, never a half-formed session). A window with no pane still opens
    /// (its original pane), so `panes` is effectively "at least one".
    #[must_use]
    pub fn commands(&self, session: &str) -> Vec<String> {
        if self.windows.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (wi, window) in self.windows.iter().enumerate() {
            // The first window rides the `new-session -d`; the rest are added.
            if wi == 0 {
                out.push(commands::new_session_detached(session, &window.name));
            } else {
                out.push(commands::new_window_named(session, &window.name));
            }
            let target = format!("{session}:{}", window.name);
            for (pi, pane) in window.panes.iter().enumerate() {
                // The first pane is the window's original; the rest split off.
                if pi > 0 {
                    out.push(commands::split_window_in(&target, window.split));
                }
                // The command seeds the pane just created (the active one), so
                // no pane index is ever needed.
                if let Some(cmd) = pane.command.as_deref().map(str::trim) {
                    if !cmd.is_empty() {
                        out.push(commands::send_text(&target, cmd));
                        out.push(commands::send_enter(&target));
                    }
                }
            }
            if let Some(layout) = window.layout {
                out.push(commands::select_layout_in(&target, layout));
            }
        }
        // Focus the first window, then attach the control client to the whole
        // freshly built session (one clean `%session-changed`).
        let first = format!("{session}:{}", self.windows[0].name);
        out.push(commands::select_window_target(&first));
        out.push(commands::attach_session(session));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-window blueprint: an "ops" window of a shell beside a log follow,
    /// and a lone "shell" window — the shape both a template and a preset build.
    fn sample() -> Blueprint {
        Blueprint::new(vec![
            BlueprintWindow::new(
                "ops",
                vec![
                    BlueprintPane::cmd("meshctl status"),
                    BlueprintPane::cmd("journalctl -f"),
                ],
                SplitDir::V,
                Some(StockLayout::EvenHorizontal),
            ),
            BlueprintWindow::new("shell", vec![BlueprintPane::shell()], SplitDir::H, None),
        ])
    }

    #[test]
    fn commands_build_a_detached_session_then_switch_onto_it() {
        let cmds = sample().commands("mesh-ops");
        assert_eq!(
            cmds,
            vec![
                "new-session -d -s 'mesh-ops' -n 'ops'".to_owned(),
                "send-keys -t 'mesh-ops:ops' -l 'meshctl status'".to_owned(),
                "send-keys -t 'mesh-ops:ops' Enter".to_owned(),
                "split-window -t 'mesh-ops:ops' -h".to_owned(),
                "send-keys -t 'mesh-ops:ops' -l 'journalctl -f'".to_owned(),
                "send-keys -t 'mesh-ops:ops' Enter".to_owned(),
                "select-layout -t 'mesh-ops:ops' even-horizontal".to_owned(),
                "new-window -t 'mesh-ops' -n 'shell'".to_owned(),
                "select-window -t 'mesh-ops:ops'".to_owned(),
                "switch-client -t 'mesh-ops'".to_owned(),
            ]
        );
    }

    #[test]
    fn a_bare_shell_pane_seeds_no_command() {
        let bp = Blueprint::new(vec![BlueprintWindow::new(
            "w",
            vec![BlueprintPane::shell()],
            SplitDir::H,
            None,
        )]);
        let cmds = bp.commands("s");
        // Just the create, the focus, and the attach — no send-keys.
        assert_eq!(
            cmds,
            vec![
                "new-session -d -s 's' -n 'w'".to_owned(),
                "select-window -t 's:w'".to_owned(),
                "switch-client -t 's'".to_owned(),
            ]
        );
    }

    #[test]
    fn an_empty_blueprint_emits_nothing() {
        assert!(Blueprint::new(Vec::new()).commands("s").is_empty());
    }

    #[test]
    fn blueprint_serde_round_trips() {
        let bp = sample();
        let json = serde_json::to_string_pretty(&bp).expect("serialize");
        let back: Blueprint = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(bp, back);
        assert_eq!(back.pane_count(), 3);
    }

    #[test]
    fn a_whitespace_only_command_is_treated_as_a_bare_shell() {
        let bp = Blueprint::new(vec![BlueprintWindow::new(
            "w",
            vec![BlueprintPane::cmd("   ")],
            SplitDir::H,
            None,
        )]);
        let cmds = bp.commands("s");
        assert!(
            !cmds.iter().any(|c| c.contains("send-keys")),
            "a blank command seeds nothing: {cmds:?}"
        );
    }
}
