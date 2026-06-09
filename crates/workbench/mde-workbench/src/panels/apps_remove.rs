//! Apps → Remove panel — one-click bloat removal.
//!
//! CB-1.3 follow-up: replaces the v1.x
//! `mackes/workbench/apps/remove.py` (per-preset bloat list).
//! v2.0.0 retires the xfconf-preset machinery; the
//! v2.0.0 bloat list is baked into the binary as
//! [`BLOAT`] (8 categories × ~30 packages), removable
//! one-by-one or in bulk via `pkexec dnf remove`.
//!
//! The list mirrors the Q15 "single combined bloat list"
//! lock — GNOME-on-XFCE apps + LibreOffice + XFCE extras
//! (asunder/parole/pragha/xfburn/transmission-gtk/
//! claws-mail/pidgin) merged into one removable set.

use std::collections::HashSet;

use iced::widget::{checkbox, column, container, row, scrollable, text};
use iced::{Element, Length, Padding, Task};
use mde_theme::Palette;
use tokio::process::Command;

use crate::controls::{variant_button, ButtonVariant};

/// v2.0.0 bloat list — packages that are commonly preinstalled
/// on Fedora workstation but redundant under MDE (sway +
/// MDE Workbench replace several built-in GNOME / XFCE pieces).
pub const BLOAT: &[(&str, &str)] = &[
    (
        "libreoffice-core",
        "LibreOffice suite (use Flatpak / portable if needed)",
    ),
    ("libreoffice-writer", "LibreOffice Writer"),
    ("libreoffice-calc", "LibreOffice Calc"),
    ("libreoffice-impress", "LibreOffice Impress"),
    ("gnome-tour", "GNOME first-run tour (irrelevant under MDE)"),
    ("gnome-photos", "GNOME Photos (use any image viewer)"),
    ("gnome-tetravex", "GNOME Tetravex game"),
    ("gnome-weather", "GNOME Weather (clock-applet covers it)"),
    ("rhythmbox", "GNOME music player (vlc/mpv cover it)"),
    ("totem", "GNOME video player (vlc covers it)"),
    ("cheese", "GNOME webcam tool"),
    (
        "simple-scan",
        "GNOME scanner tool (gscan2pdf or skanlite alt)",
    ),
    ("evince", "GNOME PDF viewer (zathura/mupdf alt)"),
    ("gnome-contacts", "GNOME contacts (KDE Connect has its own)"),
    ("gnome-maps", "GNOME maps"),
    ("gnome-calendar", "GNOME calendar (Thunderbird covers it)"),
    ("gnome-clocks", "GNOME clocks"),
    ("gnome-calculator", "GNOME calculator (terminal `bc` alt)"),
    ("gnome-disk-utility", "GNOME disks"),
    ("gnome-characters", "GNOME character map"),
    ("gnome-font-viewer", "GNOME font viewer"),
    ("gnome-logs", "GNOME logs viewer (journalctl alt)"),
    ("gnome-screenshot", "GNOME screenshot (grim+slurp alt)"),
    ("gnome-system-monitor", "GNOME system monitor (htop alt)"),
    (
        "gnome-terminal",
        "GNOME terminal (foot/wezterm/alacritty alt)",
    ),
    ("gnome-text-editor", "GNOME text editor (vim/neovim alt)"),
    ("xfburn", "XFCE CD/DVD burner"),
    ("parole", "XFCE media player"),
    ("asunder", "XFCE CD ripper"),
    ("transmission-gtk", "BitTorrent GUI client"),
    ("claws-mail", "Claws Mail"),
    ("pidgin", "Pidgin IM"),
];

#[derive(Debug, Clone, Default)]
pub struct AppsRemovePanel {
    /// Packages currently ticked for removal.
    pub selected: HashSet<String>,
    pub status: String,
    pub output: String,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Toggled {
        name: String,
        checked: bool,
    },
    SelectAllClicked,
    DeselectAllClicked,
    RemoveSelectedClicked,
    Finished {
        count: usize,
        success: bool,
        output: String,
    },
}

impl AppsRemovePanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::Toggled { name, checked } => {
                if checked {
                    self.selected.insert(name);
                } else {
                    self.selected.remove(&name);
                }
                Task::none()
            }
            Message::SelectAllClicked => {
                self.selected = BLOAT.iter().map(|(n, _)| (*n).to_string()).collect();
                Task::none()
            }
            Message::DeselectAllClicked => {
                self.selected.clear();
                Task::none()
            }
            Message::RemoveSelectedClicked => {
                if self.busy {
                    return Task::none();
                }
                if self.selected.is_empty() {
                    self.status = "Pick at least one package to remove.".into();
                    return Task::none();
                }
                let names: Vec<String> = self.selected.iter().cloned().collect();
                let count = names.len();
                self.busy = true;
                self.status = format!("Removing {count} package(s) (polkit will prompt)…");
                self.output.clear();
                Task::perform(
                    async move {
                        let (success, output) = run_pkexec_dnf_remove(&names).await;
                        Message::Finished {
                            count,
                            success,
                            output,
                        }
                    },
                    crate::Message::AppsRemove,
                )
            }
            Message::Finished {
                count,
                success,
                output,
            } => {
                self.busy = false;
                self.output = output;
                self.status = if success {
                    self.selected.clear();
                    format!("Removed {count} package(s).")
                } else {
                    format!("Removing {count} package(s) failed (see output).")
                };
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let busy = self.busy;
        // UX-7.a — bulk select toggles → Ghost (low emphasis).
        let palette = Palette::dark();
        let select_all = variant_button(
            "Select all",
            ButtonVariant::Ghost,
            (!busy).then(|| crate::Message::AppsRemove(Message::SelectAllClicked)),
            palette,
        );
        let deselect_all = variant_button(
            "Deselect all",
            ButtonVariant::Ghost,
            (!busy).then(|| crate::Message::AppsRemove(Message::DeselectAllClicked)),
            palette,
        );
        // UX-7.a — bulk-remove → Primary (dominant destructive
        // action on this panel; label conveys count when not busy).
        let remove_label = if busy {
            "Removing…".to_string()
        } else {
            format!("Remove selected ({})", self.selected.len())
        };
        let remove_btn = variant_button(
            remove_label,
            ButtonVariant::Primary,
            (!busy).then(|| crate::Message::AppsRemove(Message::RemoveSelectedClicked)),
            palette,
        );

        let rows = BLOAT.iter().fold(column![], |col, (pkg, desc)| {
            let name = (*pkg).to_string();
            let checked = self.selected.contains(*pkg);
            let cb = checkbox(checked).label(*pkg).on_toggle(move |c| {
                crate::Message::AppsRemove(Message::Toggled {
                    name: name.clone(),
                    checked: c,
                })
            });
            col.push(
                row![
                    cb.width(Length::Fixed(240.0)),
                    text(*desc).width(Length::Fill)
                ]
                .spacing(12),
            )
        });

        column![
            text("Remove bloat").size(20),
            text(
                "MDE's curated removal list. Tick what you don't need + click \
                 Remove selected — runs as one `pkexec dnf remove` invocation.",
            )
            .size(13),
            row![select_all, deselect_all, remove_btn].spacing(8),
            scrollable(container(rows.spacing(4))).height(Length::Fixed(360.0)),
            text("Output").size(14),
            scrollable(
                container(text(&self.output).size(12))
                    .padding(Padding::new(12.0))
                    .width(Length::Fill),
            )
            .height(Length::Fixed(200.0)),
            text(&self.status).size(13),
        ]
        .spacing(12)
        .width(Length::Fill)
        .into()
    }
}

/// Shell out to `pkexec dnf remove -y <name>...`. Returns
/// `(success, combined_output)`.
pub async fn run_pkexec_dnf_remove(names: &[String]) -> (bool, String) {
    let mut argv: Vec<String> = vec!["dnf".into(), "remove".into(), "-y".into()];
    argv.extend(names.iter().cloned());
    let Ok(output) = Command::new("pkexec").args(&argv).output().await else {
        return (false, "pkexec not on PATH".into());
    };
    let mut combined = String::from_utf8(output.stdout).unwrap_or_default();
    let stderr = String::from_utf8(output.stderr).unwrap_or_default();
    if !stderr.is_empty() {
        if !combined.is_empty() && !combined.ends_with('\n') {
            combined.push('\n');
        }
        combined.push_str(&stderr);
    }
    (output.status.success(), combined)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bloat_list_has_canonical_entries() {
        assert!(BLOAT.len() >= 30);
        let names: Vec<&str> = BLOAT.iter().map(|(n, _)| *n).collect();
        // Spot-check key entries from the Q15 lock.
        assert!(names.contains(&"libreoffice-core"));
        assert!(names.contains(&"gnome-tour"));
        assert!(names.contains(&"transmission-gtk"));
        assert!(names.contains(&"pidgin"));
    }

    #[test]
    fn toggled_adds_and_removes_from_selection() {
        let mut panel = AppsRemovePanel::new();
        let _ = panel.update(Message::Toggled {
            name: "firefox".into(),
            checked: true,
        });
        assert!(panel.selected.contains("firefox"));
        let _ = panel.update(Message::Toggled {
            name: "firefox".into(),
            checked: false,
        });
        assert!(!panel.selected.contains("firefox"));
    }

    #[test]
    fn select_all_ticks_every_bloat_entry() {
        let mut panel = AppsRemovePanel::new();
        let _ = panel.update(Message::SelectAllClicked);
        assert_eq!(panel.selected.len(), BLOAT.len());
    }

    #[test]
    fn deselect_all_clears_selection() {
        let mut panel = AppsRemovePanel::new();
        panel.selected.insert("firefox".into());
        let _ = panel.update(Message::DeselectAllClicked);
        assert!(panel.selected.is_empty());
    }

    #[test]
    fn remove_selected_empty_surfaces_validation() {
        let mut panel = AppsRemovePanel::new();
        let _ = panel.update(Message::RemoveSelectedClicked);
        assert!(panel.status.contains("at least one"));
        assert!(!panel.busy);
    }

    #[test]
    fn remove_selected_while_busy_is_noop() {
        let mut panel = AppsRemovePanel::new();
        panel.busy = true;
        panel.status = "stale".into();
        panel.selected.insert("firefox".into());
        let _ = panel.update(Message::RemoveSelectedClicked);
        assert_eq!(panel.status, "stale");
    }

    #[test]
    fn finished_success_clears_selection_and_records_count() {
        let mut panel = AppsRemovePanel::new();
        panel.busy = true;
        panel.selected.insert("firefox".into());
        panel.selected.insert("vim".into());
        let _ = panel.update(Message::Finished {
            count: 2,
            success: true,
            output: "ok".into(),
        });
        assert!(!panel.busy);
        assert!(panel.selected.is_empty());
        assert!(panel.status.contains("Removed 2"));
    }

    #[test]
    fn finished_failure_preserves_selection() {
        let mut panel = AppsRemovePanel::new();
        panel.busy = true;
        panel.selected.insert("firefox".into());
        let _ = panel.update(Message::Finished {
            count: 1,
            success: false,
            output: "Error: dependency held".into(),
        });
        assert!(panel.status.contains("failed"));
        assert!(panel.selected.contains("firefox"));
        assert!(panel.output.contains("dependency"));
    }
}
