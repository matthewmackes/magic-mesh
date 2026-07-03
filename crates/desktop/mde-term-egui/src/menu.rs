//! Selection context menu (TERM-15) — custom commands + built-in mesh actions.
//!
//! Right-clicking a selection composes the terminal with the mesh. Two families,
//! both pure folds + an injectable dispatch seam (the crate idiom the TERM-8
//! [`crate::remote::PtyBus`] / TERM-9 [`crate::smart::LaunchBus`] / TERM-12
//! [`crate::notify::NotifyBus`] seams already establish — a headless recorder in
//! tests, a live Bus/OS effect in production):
//!
//! * **User-defined custom commands** ([`CustomCommand`], Terminator parity): a
//!   menu `label` + a command `template` whose `{}`/`%s` placeholders are replaced
//!   by the selection. [`CustomCommand::argv`] is the pure substitution fold (the
//!   template → argv with the selection injected); [`CommandRunner`] dispatches
//!   the argv — [`OsCommandRunner`] spawns it detached in the pane's cwd.
//!
//! * **Built-in mesh actions**, each REUSING an existing surface-launch verb
//!   (§6 — never re-derived):
//!   - **send-selection-to-Chat** → the NOTIFY-CHAT [`mde_chat::MessageKind::Text`]
//!     on the existing [`ACTION_CHAT_SEND`] verb the mackesd `chat` worker drains
//!     (the same verb `mde-files-egui`'s `chat_bridge` posts a File offer on),
//!     via the [`ChatBus`] seam.
//!   - **open-path-in-Files** → [`crate::smart::LaunchRoute::Files`] on the
//!     existing [`crate::smart::OPEN_TOPIC`] surface-launch path (the widget's own
//!     [`crate::smart::LaunchBus`]).
//!   - **open-URL-in-mesh-browser** → [`crate::smart::LaunchRoute::Bookmarks`] on
//!     the same launch path.
//!   - **new-terminal-here** → the TERM-4/5 pane spawn ([`crate::splits`]),
//!     inheriting the current pane's cwd (the widget flags it, the split
//!     multiplexer drains the flag and splits with the inherited cwd).

use std::path::{Path, PathBuf};
use std::process::Command as OsCommand;

use serde::{Deserialize, Serialize};

use mde_chat::MessageKind;

/// The `action/chat/send` verb the mackesd `chat` worker drains.
///
/// (Its `ACTION_CHAT_SEND`.) Send-selection-to-Chat reuses it — a JSON boundary, a
/// local mirror of the worker's request shape, never a dep on `mackesd` (the same
/// discipline `mde-files-egui::chat_bridge` and [`crate::notify`] keep).
pub const ACTION_CHAT_SEND: &str = "action/chat/send";

/// The placeholder tokens a custom-command template substitutes the selection
/// for — `{}` (the common shell idiom) or `%s` (Terminator's spelling).
const PLACEHOLDERS: [&str; 2] = ["{}", "%s"];

/// A user-defined custom command (Terminator parity): a menu `label` + a command
/// `template` whose placeholder(s) are replaced by the current selection.
///
/// Serializable so it round-trips through the surface's config exactly as the
/// other TERM knobs do.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomCommand {
    /// The context-menu label (what the user reads).
    pub label: String,
    /// The command template, e.g. `xdg-open {}` or `grep -n %s log.txt`. Every
    /// `{}`/`%s` placeholder is replaced by the selection at dispatch.
    pub template: String,
}

impl CustomCommand {
    /// A command with `label` + `template`.
    #[must_use]
    pub fn new(label: impl Into<String>, template: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            template: template.into(),
        }
    }

    /// The argv for this command with `selection` substituted for every `{}`/`%s`
    /// placeholder — the pure substitution fold the acceptance tests.
    ///
    /// The template is whitespace-split into argv words, then each placeholder
    /// occurrence *within a word* is replaced by the selection. A bare `{}` word
    /// therefore becomes the selection as **one** argument even when the selection
    /// contains spaces (no word-splitting of the injected text), while an embedded
    /// `--file={}` substitutes inline. A template with no placeholder simply runs
    /// its fixed argv (the selection is still available to a mesh action).
    #[must_use]
    pub fn argv(&self, selection: &str) -> Vec<String> {
        self.template
            .split_whitespace()
            .map(|word| substitute(word, selection))
            .collect()
    }
}

/// Replace every placeholder token in one argv `word` with `selection`.
fn substitute(word: &str, selection: &str) -> String {
    let mut out = word.to_string();
    for ph in PLACEHOLDERS {
        if out.contains(ph) {
            out = out.replace(ph, selection);
        }
    }
    out
}

/// The dispatch seam for a custom command's argv — injectable so the substitution
/// fold is exercised headless (a recorder) while production spawns the real
/// process ([`OsCommandRunner`]).
pub trait CommandRunner: Send + Sync {
    /// Run `argv` (already substituted) in `cwd`. Best-effort: an empty argv or a
    /// spawn failure returns an operator-readable error; it never blocks and never
    /// panics.
    ///
    /// # Errors
    /// An empty argv, or whatever the OS refused when spawning the program.
    fn run(&self, argv: &[String], cwd: Option<&Path>) -> Result<(), String>;
}

/// The live runner — spawns a custom command as a detached OS process.
///
/// The child runs in the pane's cwd (Terminator's custom-command behaviour),
/// inheriting the surface's stdio, and runs independently; the terminal does not
/// wait on it.
#[derive(Debug, Default, Clone, Copy)]
pub struct OsCommandRunner;

impl CommandRunner for OsCommandRunner {
    fn run(&self, argv: &[String], cwd: Option<&Path>) -> Result<(), String> {
        let Some((prog, rest)) = argv.split_first() else {
            return Err("empty custom command — nothing to run".to_string());
        };
        let mut cmd = OsCommand::new(prog);
        cmd.args(rest);
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        cmd.spawn()
            .map(|_child| ())
            .map_err(|e| format!("could not run '{prog}': {e}"))
    }
}

/// The `action/chat/send` request body offering the selection to `to`'s
/// conversation — a local serde mirror of the worker's `SendRequest`: a 1:1
/// `peer` scope, the recipient contact (hostname = username), and the typed
/// NOTIFY-CHAT [`MessageKind::Text`] carrying the selection.
#[derive(Serialize)]
struct ChatSend<'a> {
    /// `"peer"` — a 1:1 conversation (the worker's `Scope::Peer`, `snake_case`).
    scope: &'a str,
    /// The recipient contact: a peer **host** (username = hostname).
    to: &'a str,
    /// The typed message body — a real [`MessageKind::Text`] the worker folds
    /// into a chat line (a `kind` wins over the worker's older `text` fallback).
    kind: MessageKind,
}

/// Build the `action/chat/send` body posting `text` (the selection) to `to`.
///
/// The message is a [`MessageKind::Text`] in `to`'s conversation — the pure,
/// headless core of [`BusChatClient::send`] (its exact wire shape is asserted
/// without a Bus).
///
/// # Errors
/// A serialization failure (never expected for this fixed shape).
pub fn chat_send_body(to: &str, text: &str) -> Result<String, String> {
    serde_json::to_string(&ChatSend {
        scope: "peer",
        to,
        kind: MessageKind::Text(text.to_string()),
    })
    .map_err(|e| format!("Couldn't encode the chat send: {e}"))
}

/// The Bus seam send-selection-to-Chat is dispatched over — publish the selection
/// as a chat message on the existing [`ACTION_CHAT_SEND`] verb.
///
/// Injectable so the action fold is unit-tested headless (a recorder) while
/// production talks the live Bus ([`BusChatClient`]).
pub trait ChatBus: Send + Sync {
    /// Send `text` (the selection) to the `to` contact's conversation.
    ///
    /// # Errors
    /// An operator-readable string when the append can't be written (e.g. no Bus
    /// dir on this node); it never blocks.
    fn send(&self, to: &str, text: &str) -> Result<(), String>;
}

/// The live Bus-backed chat client.
///
/// A synchronous local `Persist` append onto [`ACTION_CHAT_SEND`], the same
/// persist-first path the surface-launch and notify clients use. Degrades
/// honestly to an error when this node has no Bus dir.
#[derive(Debug, Clone)]
pub struct BusChatClient {
    bus_root: Option<PathBuf>,
}

impl BusChatClient {
    /// Resolve the Bus spool dir from the environment (the production path).
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
        }
    }

    /// Construct with an explicit spool root (tests point this at a tempdir).
    #[must_use]
    pub const fn with_root(bus_root: Option<PathBuf>) -> Self {
        Self { bus_root }
    }
}

impl ChatBus for BusChatClient {
    fn send(&self, to: &str, text: &str) -> Result<(), String> {
        let Some(root) = self.bus_root.as_ref() else {
            return Err(
                "No mesh Bus directory — can't send the selection to Chat on this node."
                    .to_string(),
            );
        };
        let body = chat_send_body(to, text)?;
        mde_bus::persist::Persist::open(root.clone())
            .and_then(|p| {
                p.write(
                    ACTION_CHAT_SEND,
                    mde_bus::hooks::config::Priority::Default,
                    None,
                    Some(&body),
                )
            })
            .map(|_| ())
            .map_err(|e| format!("Couldn't publish the chat send: {e}"))
    }
}

/// The per-surface context-menu config: the user's custom commands + the default
/// Chat recipient for send-selection-to-Chat.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextMenu {
    /// The user-defined custom commands, shown at the top of the menu (Terminator
    /// parity). Empty by default — the four built-in mesh actions always show.
    #[serde(default)]
    pub commands: Vec<CustomCommand>,
    /// The contact send-selection-to-Chat posts to (a peer host; username =
    /// hostname). Defaults to this node — a scratch/self conversation that is
    /// always reachable offline.
    #[serde(default = "crate::layout::local_node")]
    pub chat_recipient: String,
}

impl Default for ContextMenu {
    fn default() -> Self {
        Self {
            commands: Vec::new(),
            chat_recipient: crate::layout::local_node(),
        }
    }
}

impl ContextMenu {
    /// A config with `commands` and the default (self) Chat recipient.
    #[must_use]
    pub fn with_commands(commands: Vec<CustomCommand>) -> Self {
        Self {
            commands,
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // ── custom-command substitution (the template → argv fold) ───────────────

    #[test]
    fn argv_substitutes_a_bare_placeholder_as_one_argument() {
        // `{}` and `%s` both inject the selection; a bare-placeholder word stays
        // ONE argv element even when the selection has spaces.
        let cmd = CustomCommand::new("open", "xdg-open {}");
        assert_eq!(cmd.argv("a b c"), vec!["xdg-open", "a b c"]);
        let cmd = CustomCommand::new("open", "xdg-open %s");
        assert_eq!(cmd.argv("/etc/hosts"), vec!["xdg-open", "/etc/hosts"]);
    }

    #[test]
    fn argv_substitutes_an_embedded_placeholder_inline() {
        let cmd = CustomCommand::new("grep", "grep -n --file={} log.txt");
        assert_eq!(
            cmd.argv("needle"),
            vec!["grep", "-n", "--file=needle", "log.txt"]
        );
    }

    #[test]
    fn argv_with_no_placeholder_is_the_fixed_argv() {
        let cmd = CustomCommand::new("date", "date -u");
        assert_eq!(cmd.argv("ignored selection"), vec!["date", "-u"]);
    }

    #[test]
    fn argv_replaces_every_occurrence() {
        let cmd = CustomCommand::new("echo twice", "echo {} {}");
        assert_eq!(cmd.argv("x"), vec!["echo", "x", "x"]);
    }

    // ── the command runner seam ──────────────────────────────────────────────

    #[derive(Default)]
    struct RecordingRunner {
        runs: Mutex<Vec<(Vec<String>, Option<PathBuf>)>>,
    }
    impl CommandRunner for RecordingRunner {
        fn run(&self, argv: &[String], cwd: Option<&Path>) -> Result<(), String> {
            self.runs
                .lock()
                .expect("lock")
                .push((argv.to_vec(), cwd.map(Path::to_path_buf)));
            Ok(())
        }
    }

    #[test]
    fn a_runner_receives_the_substituted_argv_and_cwd() {
        let runner = RecordingRunner::default();
        let cmd = CustomCommand::new("open", "xdg-open {}");
        let cwd = PathBuf::from("/srv/work");
        runner
            .run(&cmd.argv("report.pdf"), Some(&cwd))
            .expect("record");
        let runs = runner.runs.lock().expect("lock").clone();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].0, vec!["xdg-open", "report.pdf"]);
        assert_eq!(runs[0].1.as_deref(), Some(Path::new("/srv/work")));
    }

    #[test]
    fn os_runner_rejects_an_empty_argv_rather_than_panicking() {
        assert!(OsCommandRunner.run(&[], None).is_err());
    }

    #[test]
    fn os_runner_spawns_a_real_process_in_the_given_cwd() {
        // A real spawn (`true` exits 0) proves the production path runs — no mock.
        let argv = vec!["true".to_string()];
        OsCommandRunner
            .run(&argv, Some(Path::new("/")))
            .expect("spawn /bin/true");
    }

    // ── send-selection-to-Chat (the NOTIFY-CHAT verb reuse) ──────────────────

    #[test]
    fn chat_body_is_a_peer_scoped_text_message_kind() {
        let body = chat_send_body("eagle", "cargo test failed on line 42").expect("encode");
        let v: serde_json::Value = serde_json::from_str(&body).expect("json");
        assert_eq!(v["scope"], "peer");
        assert_eq!(v["to"], "eagle");
        // The worker reads `kind` as a real mde-chat MessageKind (snake_case-tagged).
        assert_eq!(v["kind"]["text"], "cargo test failed on line 42");
    }

    #[test]
    fn chat_body_round_trips_into_a_text_message_kind() {
        // Prove it's the REAL mde-chat text kind, not a hand-rolled shape.
        let body = chat_send_body("nyc3", "hello mesh").expect("encode");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let kind: MessageKind = serde_json::from_value(v["kind"].clone()).expect("a MessageKind");
        assert_eq!(kind, MessageKind::Text("hello mesh".to_string()));
    }

    #[derive(Default)]
    struct RecordingChat {
        sends: Mutex<Vec<(String, String)>>,
    }
    impl ChatBus for RecordingChat {
        fn send(&self, to: &str, text: &str) -> Result<(), String> {
            self.sends
                .lock()
                .expect("lock")
                .push((to.to_string(), text.to_string()));
            Ok(())
        }
    }

    #[test]
    fn a_chat_bus_receives_the_selection_send() {
        let chat = RecordingChat::default();
        chat.send("fra1", "the selected line").expect("record");
        assert_eq!(
            chat.sends.lock().expect("lock").as_slice(),
            &[("fra1".to_string(), "the selected line".to_string())]
        );
    }

    #[test]
    fn bus_chat_client_publishes_a_real_send_request() {
        // The live client round-trips through a real Bus spool (tempdir) — the
        // exact persist-first path the notify + launch clients use.
        let dir = std::env::temp_dir().join(format!("mde-term-chat-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("mkdir");
        BusChatClient::with_root(Some(dir.clone()))
            .send("eagle", "shipit")
            .expect("publish");

        let persist = mde_bus::persist::Persist::open(dir.clone()).expect("open persist");
        let msgs = persist.list_since(ACTION_CHAT_SEND, None).expect("list");
        assert_eq!(msgs.len(), 1, "one send landed on the chat verb");
        let body = msgs[0].body.as_deref().expect("body");
        let v: serde_json::Value = serde_json::from_str(body).expect("json");
        assert_eq!(v["scope"], "peer");
        assert_eq!(v["to"], "eagle");
        assert_eq!(v["kind"]["text"], "shipit");

        // A no-Bus node degrades to an honest error, never a panic.
        assert!(BusChatClient::with_root(None).send("eagle", "x").is_err());

        std::fs::remove_dir_all(&dir).ok();
    }

    // ── config ───────────────────────────────────────────────────────────────

    #[test]
    fn context_menu_defaults_to_no_commands_and_the_local_recipient() {
        let menu = ContextMenu::default();
        assert!(menu.commands.is_empty());
        assert_eq!(menu.chat_recipient, crate::layout::local_node());
    }

    #[test]
    fn context_menu_with_commands_keeps_the_default_recipient() {
        let menu = ContextMenu::with_commands(vec![CustomCommand::new("open", "xdg-open {}")]);
        assert_eq!(menu.commands.len(), 1);
        assert_eq!(menu.chat_recipient, crate::layout::local_node());
    }
}
