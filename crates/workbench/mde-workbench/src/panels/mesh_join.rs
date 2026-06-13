//! Network → Mesh Join panel — one-button onboarding to a
//! mesh via the shared 16-character passcode.
//!
//! CB-1.8 partial: replaces the v1.x
//! `mackes/workbench/network/mesh_join.py` (which itself was
//! a thin GTK wrapper around the wizard's MeshJoinPage).
//! v2.0.0 routes joining through `mackesd enroll --passcode
//! <16-char>` (the same subcommand the CLI uses), so the
//! panel collapses to: passcode text input + Enroll button +
//! output area showing the enrollment-request JSON the leader
//! ingests.

use iced::widget::{column, container, row, scrollable, text, text_input};
use iced::{Element, Length, Padding, Task};
use tokio::process::Command;

use crate::controls::{variant_button, ButtonVariant};

/// Per the v12.10.1 lock: passcodes are URL-safe 16-character
/// strings. Anything shorter fails enrollment at the bus
/// surface; we surface the validation up-front so the user
/// doesn't get a polkit prompt for a request the leader
/// won't accept.
pub const PASSCODE_LEN: usize = 16;

#[derive(Debug, Clone, Default)]
pub struct MeshJoinPanel {
    pub passcode_input: String,
    pub name_input: String,
    pub output: String,
    pub status: String,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    PasscodeChanged(String),
    NameChanged(String),
    EnrollClicked,
    Finished(Result<String, String>),
}

impl MeshJoinPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            Message::PasscodeChanged(v) => {
                self.passcode_input = v;
                Task::none()
            }
            Message::NameChanged(v) => {
                self.name_input = v;
                Task::none()
            }
            Message::EnrollClicked => {
                if self.busy {
                    return Task::none();
                }
                let validation = validate_passcode(&self.passcode_input);
                if let Err(msg) = validation {
                    self.status = msg;
                    return Task::none();
                }
                self.busy = true;
                self.status = "Enrolling…".into();
                self.output.clear();
                let passcode = self.passcode_input.clone();
                let name = self.name_input.trim().to_string();
                Task::perform(
                    async move { Message::Finished(run_mackesd_enroll(&passcode, &name).await) },
                    crate::Message::MeshJoin,
                )
            }
            Message::Finished(result) => {
                self.busy = false;
                match result {
                    Ok(payload) => {
                        self.output = payload;
                        self.status = "Enrollment request emitted — feed it to the leader.".into();
                    }
                    Err(msg) => {
                        self.status = msg;
                    }
                }
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<'_, crate::Message> {
        let passcode_input = text_input("16-character shared passcode", &self.passcode_input)
            .on_input(|v| crate::Message::MeshJoin(Message::PasscodeChanged(v)));
        let name_input = text_input(
            "Optional display name (defaults to hostname)",
            &self.name_input,
        )
        .on_input(|v| crate::Message::MeshJoin(Message::NameChanged(v)));
        // UX-7.a — Enroll button routed through the shared
        // Primary variant; busy → label flips + disabled.
        let enroll_label = if self.busy { "Enrolling…" } else { "Enroll" };
        let enroll_btn = variant_button(
            enroll_label,
            ButtonVariant::Primary,
            (!self.busy).then(|| crate::Message::MeshJoin(Message::EnrollClicked)),
            crate::live_theme::palette(),
        );

        column![
            text("Join a mesh").size(20),
            text(
                "Paste the shared 16-character passcode another peer \
                 generated with `mackesd generate-passcode`. Optionally give \
                 this node a display name. Enroll emits a signed \
                 enrollment-request JSON the leader peer ingests.",
            )
            .size(13),
            row![text("Passcode").width(Length::Fixed(180.0)), passcode_input,].spacing(12),
            row![text("Display name").width(Length::Fixed(180.0)), name_input,].spacing(12),
            row![enroll_btn, text(&self.status).size(13)].spacing(12),
            text("Enrollment-request JSON").size(14),
            scrollable(
                container(text(&self.output).size(12))
                    .padding(Padding::new(12.0))
                    .width(Length::Fill),
            )
            .height(Length::Fixed(280.0)),
        ]
        .spacing(12)
        .width(Length::Fill)
        .into()
    }
}

/// Validate the passcode input matches the 16-character
/// URL-safe shape `mackesd` expects. Returns an Err with a
/// friendly message on any failure so the panel surfaces the
/// validation before shelling out.
pub fn validate_passcode(input: &str) -> Result<(), String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Passcode is required.".into());
    }
    if trimmed.len() != PASSCODE_LEN {
        return Err(format!(
            "Passcode must be exactly {PASSCODE_LEN} characters (got {}).",
            trimmed.len(),
        ));
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("Passcode must be URL-safe (ASCII alphanumeric + - / _).".into());
    }
    Ok(())
}

/// The enroll argv — the passcode is deliberately NOT here. It
/// travels via `--passcode-stdin` so it never lands in
/// `/proc/<pid>/cmdline` or shell history (EFF-21).
#[must_use]
pub fn enroll_argv(name: &str) -> Vec<String> {
    let mut argv: Vec<String> = vec!["enroll".to_string(), "--passcode-stdin".to_string()];
    if !name.is_empty() {
        argv.push("--name".to_string());
        argv.push(name.to_string());
    }
    argv
}

/// Shell out to `mackesd enroll --passcode-stdin` (with an
/// optional `--name <name>` flag), piping the passcode through
/// stdin — never argv, which is world-readable via
/// `/proc/<pid>/cmdline` (EFF-21). Returns the stdout
/// (enrollment-request JSON) on success.
pub async fn run_mackesd_enroll(passcode: &str, name: &str) -> Result<String, String> {
    let argv = enroll_argv(name);
    let mut child = match Command::new("mackesd")
        .args(&argv)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => {
            return Err("`mackesd` not on PATH — install MackesWorkstation or check $PATH.".into())
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let line = format!("{passcode}\n");
        if let Err(e) = stdin.write_all(line.as_bytes()).await {
            return Err(format!("writing passcode to mackesd stdin: {e}"));
        }
        // Drop closes the pipe so the daemon's read_line returns.
    }
    let Ok(output) = child.wait_with_output().await else {
        return Err("`mackesd enroll` did not complete.".into());
    };
    if output.status.success() {
        Ok(String::from_utf8(output.stdout)
            .unwrap_or_default()
            .trim()
            .to_string())
    } else {
        let stderr = String::from_utf8(output.stderr).unwrap_or_default();
        Err(format!("mackesd enroll failed: {}", stderr.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_passcode_accepts_16_char_urlsafe() {
        assert!(validate_passcode("abcdef0123456789").is_ok());
        assert!(validate_passcode("AbCdEf0123-_4567").is_ok());
    }

    #[test]
    fn validate_passcode_rejects_empty() {
        assert!(validate_passcode("").is_err());
        assert!(validate_passcode("   ").is_err());
    }

    #[test]
    fn validate_passcode_rejects_wrong_length() {
        assert!(validate_passcode("short").is_err());
        assert!(validate_passcode(&"x".repeat(17)).is_err());
    }

    #[test]
    fn validate_passcode_rejects_non_urlsafe_chars() {
        assert!(validate_passcode("abcdef0123456789!").is_err()); // 17 chars
        assert!(validate_passcode("abcdef 123456789").is_err()); // space
        assert!(validate_passcode("abcdef+/23456789").is_err()); // + and /
    }

    #[test]
    fn enroll_argv_never_carries_the_passcode() {
        // EFF-21 / AUD6-1: the secret rides stdin, never argv.
        let argv = enroll_argv("anvil");
        assert_eq!(argv[..2], ["enroll", "--passcode-stdin"]);
        assert!(!argv.contains(&"--passcode".to_string()));
        let bare = enroll_argv("");
        assert_eq!(bare, ["enroll", "--passcode-stdin"]);
    }

    #[test]
    fn passcode_changed_mutates_input() {
        let mut panel = MeshJoinPanel::new();
        let _ = panel.update(Message::PasscodeChanged("abcdef".into()));
        assert_eq!(panel.passcode_input, "abcdef");
    }

    #[test]
    fn name_changed_mutates_input() {
        let mut panel = MeshJoinPanel::new();
        let _ = panel.update(Message::NameChanged("anvil".into()));
        assert_eq!(panel.name_input, "anvil");
    }

    #[test]
    fn enroll_clicked_with_invalid_passcode_surfaces_validation() {
        let mut panel = MeshJoinPanel::new();
        panel.passcode_input = "tooshort".into();
        let _ = panel.update(Message::EnrollClicked);
        assert!(panel.status.contains("Passcode"));
        assert!(!panel.busy);
    }

    #[test]
    fn enroll_clicked_with_empty_passcode_surfaces_validation() {
        let mut panel = MeshJoinPanel::new();
        let _ = panel.update(Message::EnrollClicked);
        assert!(panel.status.contains("required"));
        assert!(!panel.busy);
    }

    #[test]
    fn enroll_clicked_while_busy_is_noop() {
        let mut panel = MeshJoinPanel::new();
        panel.busy = true;
        panel.status = "stale".into();
        panel.passcode_input = "abcdef0123456789".into();
        let _ = panel.update(Message::EnrollClicked);
        assert_eq!(panel.status, "stale");
    }

    #[test]
    fn finished_ok_clears_busy_and_records_payload() {
        let mut panel = MeshJoinPanel::new();
        panel.busy = true;
        let payload = "{\"request\":\"signed-blob\"}".to_string();
        let _ = panel.update(Message::Finished(Ok(payload.clone())));
        assert!(!panel.busy);
        assert_eq!(panel.output, payload);
        assert!(panel.status.to_lowercase().contains("enroll"));
    }

    #[test]
    fn finished_err_clears_busy_and_records_error() {
        let mut panel = MeshJoinPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::Finished(Err("passcode rejected by leader".into())));
        assert!(!panel.busy);
        assert!(panel.status.contains("rejected"));
        assert!(panel.output.is_empty());
    }
}
