//! Network → Mesh Join panel — onboard this node to a mesh via a
//! single-use **join token** (the v2.5 Nebula flow).
//!
//! The v1.x passcode → signed-EnrollmentRequest → leader-pending-inbox flow was
//! superseded: there is no pending-approval step (the inbox in Phase 12.8.2 was
//! never built). A peer mints a join token with `mackesd enroll-token` (or the
//! Registration panel's Mint), hands it over, and this panel shells
//! `mackesd enroll --token-stdin` — which publishes a CSR and waits (~30 s) for a
//! lighthouse to sign the node in. The single-use bearer rides stdin, never argv
//! (EFF-21).

use cosmic::iced::widget::{column, container, row, scrollable, text, text_input};
use cosmic::iced::{Length, Padding, Task};
use tokio::process::Command;

use crate::controls::{variant_button, ButtonVariant};

#[derive(Debug, Clone, Default)]
pub struct MeshJoinPanel {
    pub token_input: String,
    pub name_input: String,
    pub output: String,
    pub status: String,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    TokenChanged(String),
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
            Message::TokenChanged(v) => {
                self.token_input = v;
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
                if let Err(msg) = validate_token(&self.token_input) {
                    self.status = msg;
                    return Task::none();
                }
                self.busy = true;
                self.status =
                    "Enrolling — publishing CSR, waiting for a lighthouse to sign (~30s)…".into();
                self.output.clear();
                let token = self.token_input.trim().to_string();
                let name = self.name_input.trim().to_string();
                Task::perform(
                    async move { Message::Finished(run_mackesd_enroll(&token, &name).await) },
                    crate::Message::MeshJoin,
                )
            }
            Message::Finished(result) => {
                self.busy = false;
                match result {
                    Ok(payload) => {
                        self.output = payload;
                        self.status = "Joined the mesh.".into();
                    }
                    Err(msg) => {
                        self.status = msg;
                    }
                }
                Task::none()
            }
        }
    }

    pub fn view(&self) -> cosmic::Element<'_, crate::Message> {
        let token_input = text_input(
            "join token: mesh:<id>@<host>:<port>#<bearer>",
            &self.token_input,
        )
        .on_input(|v| crate::Message::MeshJoin(Message::TokenChanged(v)));
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
                "Paste the single-use join token a peer minted for you with \
                 `mackesd enroll-token` (or the Registration panel's Mint). \
                 Enroll publishes this node's CSR and waits for a lighthouse to \
                 sign it in (~30s). The token rides stdin, never the command line.",
            )
            .size(13),
            row![text("Join token").width(Length::Fixed(180.0)), token_input,].spacing(12),
            row![text("Display name").width(Length::Fixed(180.0)), name_input,].spacing(12),
            row![enroll_btn, text(&self.status).size(13)].spacing(12),
            text("Enrollment result").size(14),
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

/// Validate a v2.5 join token of the shape `mesh:<id>@<host>:<port>#<bearer>`
/// (minted by `mackesd enroll-token`). Only the load-bearing shape is checked —
/// the lighthouse is the real authority — so an obvious typo surfaces before the
/// ~30 s sign wait rather than after it.
pub fn validate_token(input: &str) -> Result<(), String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Join token is required.".into());
    }
    if !trimmed.starts_with("mesh:") || !trimmed.contains('@') || !trimmed.contains('#') {
        return Err(
            "That doesn't look like a join token — expected `mesh:<id>@<host>:<port>#<bearer>`."
                .into(),
        );
    }
    Ok(())
}

/// The enroll argv — the token is deliberately NOT here. It travels via
/// `--token-stdin` so the single-use bearer never lands in `/proc/<pid>/cmdline`
/// or shell history (EFF-21).
#[must_use]
pub fn enroll_argv(name: &str) -> Vec<String> {
    let mut argv: Vec<String> = vec!["enroll".to_string(), "--token-stdin".to_string()];
    if !name.is_empty() {
        argv.push("--name".to_string());
        argv.push(name.to_string());
    }
    argv
}

/// Shell out to `mackesd enroll --token-stdin` (with an optional `--name <name>`
/// flag), piping the join token through stdin — never argv, which is
/// world-readable via `/proc/<pid>/cmdline` (EFF-21). Returns the stdout (the
/// "enrolled into mesh …" confirmation) on success.
pub async fn run_mackesd_enroll(token: &str, name: &str) -> Result<String, String> {
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
        let line = format!("{token}\n");
        if let Err(e) = stdin.write_all(line.as_bytes()).await {
            return Err(format!("writing token to mackesd stdin: {e}"));
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
    fn validate_token_accepts_a_well_formed_token() {
        assert!(validate_token("mesh:home@10.42.0.1:4242#deadbeefcafebabe").is_ok());
    }

    #[test]
    fn validate_token_rejects_empty() {
        assert!(validate_token("").is_err());
        assert!(validate_token("   ").is_err());
    }

    #[test]
    fn validate_token_rejects_malformed() {
        assert!(validate_token("not-a-token").is_err()); // no mesh: / @ / #
        assert!(validate_token("mesh:home").is_err()); // no @ or #
    }

    #[test]
    fn enroll_argv_never_carries_the_token() {
        // EFF-21: the single-use bearer rides stdin, never argv.
        let argv = enroll_argv("anvil");
        assert_eq!(argv[..2], ["enroll", "--token-stdin"]);
        assert!(!argv.contains(&"--token".to_string()));
        let bare = enroll_argv("");
        assert_eq!(bare, ["enroll", "--token-stdin"]);
    }

    #[test]
    fn token_changed_mutates_input() {
        let mut panel = MeshJoinPanel::new();
        let _ = panel.update(Message::TokenChanged("mesh:x@h:4242#tok".into()));
        assert_eq!(panel.token_input, "mesh:x@h:4242#tok");
    }

    #[test]
    fn name_changed_mutates_input() {
        let mut panel = MeshJoinPanel::new();
        let _ = panel.update(Message::NameChanged("anvil".into()));
        assert_eq!(panel.name_input, "anvil");
    }

    #[test]
    fn enroll_clicked_with_malformed_token_surfaces_validation() {
        let mut panel = MeshJoinPanel::new();
        panel.token_input = "not-a-token".into();
        let _ = panel.update(Message::EnrollClicked);
        assert!(panel.status.contains("join token"));
        assert!(!panel.busy);
    }

    #[test]
    fn enroll_clicked_with_empty_token_surfaces_validation() {
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
        panel.token_input = "mesh:x@h:4242#tok".into();
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
        assert!(panel.status.to_lowercase().contains("join"));
    }

    #[test]
    fn finished_err_clears_busy_and_records_error() {
        let mut panel = MeshJoinPanel::new();
        panel.busy = true;
        let _ = panel.update(Message::Finished(
            Err("token rejected by lighthouse".into()),
        ));
        assert!(!panel.busy);
        assert!(panel.status.contains("rejected"));
        assert!(panel.output.is_empty());
    }
}
