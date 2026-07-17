//! TMUX-FC-7 — the **5 mesh-styled layout presets**.
//!
//! Design: `docs/design/tmux-first-class.md` (#11/#13). Five curated tmux layouts
//! for the operator's everyday jobs, each a window of seeded-command panes in the
//! Quazar style — distinct from the stock tmux five ([`crate::tmux::StockLayout`]).
//!
//! Each preset is a built-in [`Blueprint`] (the same recipe a TMUX-FC-5 template
//! opens), so opening one builds a detached session and switches the control
//! client onto it through the exact FC-5 command sequence — the round-trip
//! discipline, reused, not re-implemented (§6). Being **built into the byte-
//! identical stack** they are the same on every node — the strongest form of the
//! design's "mesh-synced": no file to replicate, no drift between peers.
//!
//! The seeded commands are the real mesh tools (`meshctl status` /
//! `meshctl fleet status` / `meshctl logs --follow`, `openstack …`, `btop`,
//! `journalctl -f`, the AI CLIs). A preset is a **starting point**: a tool a given
//! node lacks simply prints "command not found" in its pane (honest, never a
//! crash), and the layout is the operator's to reshape.

use crate::blueprint::{Blueprint, BlueprintPane, BlueprintWindow};
use crate::splits::SplitDir;
use crate::tmux::StockLayout;

/// One of the five mesh-styled layout presets (TMUX-FC-7).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MeshPreset {
    /// `meshctl status` · fleet roll-up · mesh log follow · a control shell.
    MeshOps,
    /// `btop` · `journalctl -f` · disk usage · a shell (per-node health).
    NodeWatch,
    /// `OpenStack` instances · Heat stacks · service logs · a shell.
    Cloud,
    /// An editor · a build/test shell · a run/logs shell · git.
    DevBuild,
    /// The Claude CLI + the Codex CLI side by side + a work shell.
    AiCli,
}

impl MeshPreset {
    /// Every preset, in menu order.
    pub const ALL: [Self; 5] = [
        Self::MeshOps,
        Self::NodeWatch,
        Self::Cloud,
        Self::DevBuild,
        Self::AiCli,
    ];

    /// The menu / launch label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::MeshOps => "Mesh Ops",
            Self::NodeWatch => "Node Watch",
            Self::Cloud => "Cloud / OpenStack",
            Self::DevBuild => "Dev / Build",
            Self::AiCli => "AI CLI",
        }
    }

    /// The tmux session name a launch of this preset opens (topic-safe: no `.`/`:`).
    #[must_use]
    pub const fn session_name(self) -> &'static str {
        match self {
            Self::MeshOps => "mesh-ops",
            Self::NodeWatch => "node-watch",
            Self::Cloud => "cloud",
            Self::DevBuild => "dev-build",
            Self::AiCli => "ai-cli",
        }
    }

    /// The layout + seeded commands this preset builds.
    #[must_use]
    pub fn blueprint(self) -> Blueprint {
        match self {
            Self::MeshOps => one_window(
                "mesh",
                &[
                    Some("meshctl status"),
                    Some("meshctl fleet status"),
                    Some("meshctl logs --follow"),
                    None,
                ],
                StockLayout::Tiled,
            ),
            Self::NodeWatch => one_window(
                "watch",
                &[Some("btop"), Some("journalctl -f"), Some("df -h"), None],
                StockLayout::Tiled,
            ),
            Self::Cloud => one_window(
                "cloud",
                &[
                    Some("openstack server list"),
                    Some("openstack stack list"),
                    Some("journalctl -f"),
                    None,
                ],
                StockLayout::Tiled,
            ),
            Self::DevBuild => one_window(
                "dev",
                &[Some("${EDITOR:-vim} ."), None, None, Some("git status")],
                StockLayout::Tiled,
            ),
            // Claude + Codex side by side (+ a work shell) — even columns.
            Self::AiCli => one_window(
                "ai",
                &[Some("claude"), Some("codex"), None],
                StockLayout::EvenHorizontal,
            ),
        }
    }
}

/// Build a single-window blueprint: one window with a pane per `panes` entry
/// (`Some(cmd)` seeds a command, `None` is a bare shell), each splitting off
/// beside the previous and evened out by `layout`.
fn one_window(name: &str, panes: &[Option<&str>], layout: StockLayout) -> Blueprint {
    let panes = panes
        .iter()
        .map(|p| p.map_or_else(BlueprintPane::shell, BlueprintPane::cmd))
        .collect();
    Blueprint::new(vec![BlueprintWindow::new(
        name,
        panes,
        SplitDir::V,
        Some(layout),
    )])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_builds_a_non_empty_layout() {
        for preset in MeshPreset::ALL {
            let bp = preset.blueprint();
            assert_eq!(bp.windows.len(), 1, "{preset:?} is a one-window layout");
            let cmds = bp.commands(preset.session_name());
            assert!(!cmds.is_empty(), "{preset:?} built no commands");
            // Every preset opens its session detached then switches the client on.
            assert!(cmds[0].starts_with("new-session -d -s"));
            assert_eq!(
                cmds.last().map(String::as_str),
                Some(format!("switch-client -t '{}'", preset.session_name()).as_str())
            );
            // The session name is tmux-topic-safe (no reserved `.`/`:`).
            let name = preset.session_name();
            assert!(!name.contains('.') && !name.contains(':'), "{name}");
        }
    }

    #[test]
    fn mesh_ops_seeds_the_real_mesh_tools() {
        let cmds = MeshPreset::MeshOps.blueprint().commands("mesh-ops");
        let joined = cmds.join("\n");
        for tool in [
            "meshctl status",
            "meshctl fleet status",
            "meshctl logs --follow",
        ] {
            assert!(joined.contains(tool), "Mesh Ops is missing {tool:?}");
        }
        // Four panes → three splits in the one window.
        assert_eq!(
            cmds.iter()
                .filter(|c| c.starts_with("split-window"))
                .count(),
            3
        );
    }

    #[test]
    fn ai_cli_puts_claude_and_codex_side_by_side() {
        let bp = MeshPreset::AiCli.blueprint();
        assert_eq!(bp.pane_count(), 3);
        let cmds = bp.commands("ai-cli");
        let joined = cmds.join("\n");
        assert!(joined.contains("send-keys -t 'ai-cli:ai' -l 'claude'"));
        assert!(joined.contains("send-keys -t 'ai-cli:ai' -l 'codex'"));
        // Evened horizontally (side by side), not tiled.
        assert!(joined.contains("select-layout -t 'ai-cli:ai' even-horizontal"));
    }

    #[test]
    fn the_five_presets_have_distinct_labels_and_sessions() {
        let labels: Vec<&str> = MeshPreset::ALL.iter().map(|p| p.label()).collect();
        let sessions: Vec<&str> = MeshPreset::ALL.iter().map(|p| p.session_name()).collect();
        assert_eq!(labels.len(), 5);
        for i in 0..labels.len() {
            for j in (i + 1)..labels.len() {
                assert_ne!(labels[i], labels[j], "duplicate label");
                assert_ne!(sessions[i], sessions[j], "duplicate session");
            }
        }
    }
}
