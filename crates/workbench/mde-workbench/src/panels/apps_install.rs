//! Apps → Install panel — package-name input + curated MDE
//! suggestions over `pkexec dnf install`.
//!
//! CB-1.3 follow-up: replaces the v1.x
//! `mackes/workbench/apps/install.py` (curated CATALOG +
//! per-preset install list). v2.0.0 retires the preset
//! machinery the v1.x panel depended on; this Iced port
//! ships a simpler shape: a free-form package text input +
//! Install button, plus a curated "MDE recommendations"
//! grid baked into the binary (filter by Fedora-shipped vs
//! third-party at build time, no per-preset coupling).

use iced::widget::{column, container, row, scrollable, text, text_input};
use iced::{Element, Length, Padding, Task};
use mde_theme::Palette;
use tokio::process::Command;

use crate::controls::{variant_button, ButtonVariant};

/// Curated MDE-recommended packages. Surfaces a 1-click row
/// for each. Names are dnf-installable on Fedora workstation
/// out of the box; no third-party repo enable needed.
pub const RECOMMENDED: &[(&str, &str)] = &[
    ("firefox", "Default web browser"),
    ("thunderbird", "Mail + calendar client"),
    ("vlc", "Media player (any format)"),
    ("vim", "Modal text editor"),
    ("neovim", "Vim fork with async + Lua"),
    ("zsh", "Z shell (chsh -s /usr/bin/zsh to switch)"),
    ("tmux", "Terminal multiplexer"),
    ("git", "Version control"),
    ("gh", "GitHub CLI"),
    ("rsync", "File-transfer + backup utility"),
    ("htop", "Process viewer"),
    ("ncdu", "Disk-usage explorer"),
    ("ripgrep", "Fast grep replacement"),
    ("fd-find", "Fast find replacement"),
    ("podman", "Containers (Docker-compatible)"),
    ("flatpak", "Sandboxed app runtime"),
];

#[derive(Debug, Clone, Default)]
pub struct AppsInstallPanel {
    pub name_input: String,
    pub status: String,
    pub output: String,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    NameChanged(String),
    InstallClicked,
    QuickInstallClicked(String),
    Finished {
        name: String,
        success: bool,
        output: String,
    },
}

impl AppsInstallPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::NameChanged(v) => {
                self.name_input = v;
                Task::none()
            }
            Message::InstallClicked => {
                if self.busy {
                    return Task::none();
                }
                let name = self.name_input.trim().to_string();
                if let Err(msg) = validate_package_name(&name) {
                    self.status = msg;
                    return Task::none();
                }
                self.dispatch_install(name)
            }
            Message::QuickInstallClicked(name) => {
                if self.busy {
                    return Task::none();
                }
                self.dispatch_install(name)
            }
            Message::Finished {
                name,
                success,
                output,
            } => {
                self.busy = false;
                self.output = output;
                self.status = if success {
                    format!("Installed {name}.")
                } else {
                    format!("Installing {name} failed (see output).")
                };
                Task::none()
            }
        }
    }

    fn dispatch_install(&mut self, name: String) -> Task<crate::Message> {
        self.busy = true;
        self.status = format!("Installing {name} (polkit will prompt)…");
        self.output.clear();
        let name_for_task = name.clone();
        Task::perform(
            async move {
                let (success, output) = run_pkexec_dnf_install(&name_for_task).await;
                Message::Finished {
                    name: name_for_task,
                    success,
                    output,
                }
            },
            crate::Message::AppsInstall,
        )
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let name_input = text_input("Package name (e.g. ripgrep)", &self.name_input)
            .on_input(|v| crate::Message::AppsInstall(Message::NameChanged(v)));
        // UX-7.a — install routed through Primary (dominant CTA).
        let install_label = if self.busy {
            "Installing…"
        } else {
            "Install"
        };
        let install_btn = variant_button(
            install_label,
            ButtonVariant::Primary,
            (!self.busy).then(|| crate::Message::AppsInstall(Message::InstallClicked)),
            Palette::dark(),
        );

        let quick_rows = RECOMMENDED.iter().fold(column![], |col, (pkg, desc)| {
            let name = (*pkg).to_string();
            // UX-7.a — quick-install per recent-app routed through
            // Secondary (less prominent than the main install button).
            let btn = variant_button(
                "Install",
                ButtonVariant::Secondary,
                (!self.busy)
                    .then(|| crate::Message::AppsInstall(Message::QuickInstallClicked(name))),
                Palette::dark(),
            );
            col.push(
                row![
                    text(*pkg).width(Length::Fixed(140.0)),
                    text(*desc).width(Length::Fill),
                    btn,
                ]
                .spacing(12),
            )
        });

        column![
            text("Install apps").size(20),
            text(
                "Type a package name or pick one from the MDE \
                 recommendations. Installs run under pkexec dnf install.",
            )
            .size(13),
            row![name_input, install_btn].spacing(12),
            text("MDE recommendations").size(16),
            scrollable(container(quick_rows.spacing(4))).height(Length::Fixed(280.0)),
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

/// Validate a package name before shelling out. dnf accepts a
/// broader set than we surface, but we reject obvious shell-
/// injection vectors + empty input up-front so the user sees
/// the problem in the status row instead of a polkit prompt
/// for a request dnf would just error on.
pub fn validate_package_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Package name is required.".into());
    }
    if name.len() > 200 {
        return Err("Package name too long.".into());
    }
    let ok = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+'));
    if !ok {
        return Err(
            "Package name may only contain ASCII letters, digits, and -_.+ characters.".into(),
        );
    }
    Ok(())
}

/// Shell out to `pkexec dnf install -y <name>`. Returns
/// `(success, combined_output)`.
pub async fn run_pkexec_dnf_install(name: &str) -> (bool, String) {
    let Ok(output) = Command::new("pkexec")
        .args(["dnf", "install", "-y", name])
        .output()
        .await
    else {
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
    fn recommended_list_is_non_empty() {
        assert!(!RECOMMENDED.is_empty());
        // Spot-check three canonical entries are present.
        let names: Vec<&str> = RECOMMENDED.iter().map(|(n, _)| *n).collect();
        assert!(names.contains(&"firefox"));
        assert!(names.contains(&"git"));
        assert!(names.contains(&"ripgrep"));
    }

    #[test]
    fn validate_package_name_accepts_typical_names() {
        assert!(validate_package_name("firefox").is_ok());
        assert!(validate_package_name("python3-pip").is_ok());
        assert!(validate_package_name("gcc-c++").is_ok());
        assert!(validate_package_name("rust1.78").is_ok());
    }

    #[test]
    fn validate_package_name_rejects_empty() {
        assert!(validate_package_name("").is_err());
    }

    #[test]
    fn validate_package_name_rejects_shell_metacharacters() {
        assert!(validate_package_name("pkg; rm -rf /").is_err());
        assert!(validate_package_name("pkg && evil").is_err());
        assert!(validate_package_name("pkg`echo`").is_err());
        assert!(validate_package_name("pkg|pipe").is_err());
        assert!(validate_package_name("pkg with space").is_err());
    }

    #[test]
    fn validate_package_name_rejects_overlong() {
        assert!(validate_package_name(&"x".repeat(201)).is_err());
    }

    #[test]
    fn install_clicked_with_invalid_name_surfaces_validation() {
        let mut panel = AppsInstallPanel::new();
        panel.name_input = "".into();
        let _ = panel.update(Message::InstallClicked);
        assert!(panel.status.contains("required"));
        assert!(!panel.busy);
    }

    #[test]
    fn install_clicked_with_metachars_surfaces_validation() {
        let mut panel = AppsInstallPanel::new();
        panel.name_input = "evil; rm".into();
        let _ = panel.update(Message::InstallClicked);
        assert!(panel.status.contains("ASCII"));
        assert!(!panel.busy);
    }

    #[test]
    fn install_clicked_while_busy_is_noop() {
        let mut panel = AppsInstallPanel::new();
        panel.busy = true;
        panel.name_input = "firefox".into();
        panel.status = "stale".into();
        let _ = panel.update(Message::InstallClicked);
        assert_eq!(panel.status, "stale");
    }

    #[test]
    fn quick_install_while_busy_is_noop() {
        let mut panel = AppsInstallPanel::new();
        panel.busy = true;
        panel.status = "stale".into();
        let _ = panel.update(Message::QuickInstallClicked("firefox".into()));
        assert_eq!(panel.status, "stale");
    }

    #[test]
    fn finished_success_clears_busy_and_records() {
        let mut panel = AppsInstallPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::Finished {
            name: "firefox".into(),
            success: true,
            output: "ok".into(),
        });
        assert!(!panel.busy);
        assert!(panel.status.contains("Installed firefox"));
        assert_eq!(panel.output, "ok");
    }

    #[test]
    fn finished_failure_records_failure_status() {
        let mut panel = AppsInstallPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::Finished {
            name: "ghost-pkg".into(),
            success: false,
            output: "No match for argument: ghost-pkg".into(),
        });
        assert!(!panel.busy);
        assert!(panel.status.contains("failed"));
        assert!(panel.output.contains("No match"));
    }

    #[test]
    fn name_changed_mutates_input() {
        let mut panel = AppsInstallPanel::new();
        let _ = panel.update(Message::NameChanged("ripgrep".into()));
        assert_eq!(panel.name_input, "ripgrep");
    }
}
