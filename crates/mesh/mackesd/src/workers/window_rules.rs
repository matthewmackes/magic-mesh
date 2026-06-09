//! Portal-53.a (v6.0, R12-Q14 — backend half) — window-rules TOML
//! store + mded worker.
//!
//! Operators write `~/.config/mde/window-rules.toml` to tell the
//! window manager what to do with new windows of a given app: float
//! them, mark them, fullscreen them on first open, send them to a
//! specific workspace, etc. This worker reads the TOML on startup,
//! re-reads on file mtime change, and applies each rule via
//! swayipc `for_window <criteria> <actions>` registrations.
//!
//! TOML schema:
//!
//! ```toml
//! [[rule]]
//! match = "Firefox"                 # app_id criterion
//! floating = true                   # `floating enable`
//! sticky = true                     # `sticky enable`
//! fullscreen_on_start = true        # `fullscreen enable`
//! border_width = 4                  # `border normal 4`
//! mark = "web"                      # `mark web`
//! assign_workspace = 3              # `move container to workspace number 3`
//! ```
//!
//! All fields except `match` are optional. Actions compose into a
//! semicolon-chained `for_window <criteria> <a1>; <a2>; ...`
//! string, fired once per rule.
//!
//! Sway-reload-survival: `for_window` registrations are
//! session-scoped + don't have a swayipc "unregister" op. The
//! worker re-fires every rule on every reload (sway's
//! `barconfig_update` event proxies the reload). Removing a rule
//! from the TOML takes effect on the next sway restart, not on
//! TOML edit alone — the worker logs this caveat in a warning the
//! first time it sees a rule disappear mid-session.
//!
//! `focus_policy` from the design body is deferred to Portal-53.b
//! (Hub modal) — sway's grammar for focus-policy varies enough
//! that the right shape needs UI-side coordination.

#![cfg(feature = "async-services")]

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use swayipc_async::Connection;

use super::{ShutdownToken, Worker};

// Portal-53.b.types-share (2026-05-27) — types moved to
// mackes-mesh-types so mde-portal's Hub right-click modal can
// consume them without a mackesd dep. Re-exported here so
// existing callers (`use crate::workers::window_rules::WindowRule;`)
// keep working without churn.
pub use mackes_mesh_types::window_rules::{WindowRule, WindowRulesFile};

const RECONNECT_BACKOFF: Duration = Duration::from_secs(3);
const MTIME_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Worker state — tracks the TOML mtime + the set of rules
/// already applied so dropping a rule mid-session logs a single
/// "rule dropped (sway restart required)" warning instead of
/// spamming on every poll.
pub struct WindowRulesWorker {
    last_mtime: Option<SystemTime>,
    applied_signatures: HashSet<String>,
}

impl WindowRulesWorker {
    /// Construct a fresh worker.
    #[must_use]
    pub fn new() -> Self {
        Self {
            last_mtime: None,
            applied_signatures: HashSet::new(),
        }
    }
}

impl Default for WindowRulesWorker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Worker for WindowRulesWorker {
    fn name(&self) -> &'static str {
        "window_rules"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            if shutdown.is_shutdown() {
                return Ok(());
            }
            let mut conn = match Connection::new().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::debug!(error = %e, "window_rules connect failed; backing off");
                    sleep_or_shutdown(RECONNECT_BACKOFF, &mut shutdown).await;
                    continue;
                }
            };
            // Initial load + apply.
            self.reload_and_apply(&mut conn).await;
            // mtime-poll loop. tokio::select! aborts on shutdown.
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.wait() => return Ok(()),
                    _ = tokio::time::sleep(MTIME_POLL_INTERVAL) => {
                        if self.has_mtime_changed() {
                            self.reload_and_apply(&mut conn).await;
                        }
                    }
                }
            }
        }
    }
}

impl WindowRulesWorker {
    fn has_mtime_changed(&self) -> bool {
        let Some(path) = default_rules_path() else {
            return false;
        };
        let Ok(meta) = std::fs::metadata(&path) else {
            // Missing file = no rules. last_mtime stays None.
            return self.last_mtime.is_some();
        };
        let Ok(mtime) = meta.modified() else {
            return false;
        };
        match self.last_mtime {
            None => true,
            Some(prev) => mtime != prev,
        }
    }

    async fn reload_and_apply(&mut self, conn: &mut Connection) {
        let Some(path) = default_rules_path() else {
            return;
        };
        let file = match read_rules_file(&path) {
            Ok(f) => f,
            Err(e) => {
                tracing::debug!(path = %path.display(), error = %e, "window_rules load failed; skipping reload");
                return;
            }
        };
        // Update mtime stamp first so a parse failure doesn't
        // hot-loop on the same broken file.
        self.last_mtime = std::fs::metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok());
        let mut next_sigs: HashSet<String> = HashSet::new();
        for rule in &file.rules {
            let Some(cmd) = command_for_rule(rule) else {
                continue;
            };
            next_sigs.insert(rule_signature(rule));
            match conn.run_command(&cmd).await {
                Ok(_) => {
                    tracing::debug!(rule = %rule.match_app_id, "window_rules applied");
                }
                Err(e) => {
                    tracing::warn!(rule = %rule.match_app_id, error = %e, "window_rules apply failed");
                }
            }
        }
        // Warn once per rule that dropped from the TOML — sway
        // can't unregister a for_window; operator needs a sway
        // restart to clear the dropped rule.
        for sig in self.applied_signatures.difference(&next_sigs) {
            tracing::warn!(rule_signature = %sig, "window_rules rule dropped; existing for_window registrations linger until sway restart");
        }
        self.applied_signatures = next_sigs;
    }
}

async fn sleep_or_shutdown(dur: Duration, shutdown: &mut ShutdownToken) {
    tokio::select! {
        _ = shutdown.wait() => {}
        _ = tokio::time::sleep(dur) => {}
    }
}

// ── Pure helpers ────────────────────────────────────────────────────────

/// Default path for the rules TOML:
/// `<XDG_CONFIG_HOME>/mde/window-rules.toml`.
///
/// Worker-side `Option<PathBuf>` shim around the moved
/// `mackes_mesh_types::window_rules::default_rules_path()`. Kept
/// at this signature so existing callers in the worker loop
/// (`if let Some(p) = default_rules_path() { … }`) compile
/// unchanged. The result-side path-resolution failure variant in
/// the mesh-types crate maps to `None` here — the worker handles
/// missing-XDG by skipping the apply tick, never by an Err.
#[must_use]
pub fn default_rules_path() -> Option<PathBuf> {
    mackes_mesh_types::window_rules::default_rules_path().ok()
}

/// Read + parse the rules file. Missing file returns an empty
/// `WindowRulesFile` (first-boot / no-rules path).
///
/// Worker-side shim that forwards to
/// `WindowRulesFile::load_from`. Kept as a free function (rather
/// than callers using the method directly) so the `ReadError`
/// surface stays stable for the worker loop's existing match
/// arms; the moved type's `RulesError` covers more variants
/// (Serialize + PathResolution) that the worker doesn't see on
/// the read path.
pub fn read_rules_file(path: &Path) -> Result<WindowRulesFile, ReadError> {
    use mackes_mesh_types::window_rules::RulesError;
    WindowRulesFile::load_from(path).map_err(|e| match e {
        RulesError::Io(e) => ReadError::Io(e),
        RulesError::Parse(e) => ReadError::Parse(e),
        // Serialize + PathResolution don't fire on a load — the
        // load path's only failure modes are Io + Parse. Map
        // defensively to Io with an explanatory wrapper to keep
        // the match exhaustive.
        other => ReadError::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("unexpected error on read: {other}"),
        )),
    })
}

/// Error surface for `read_rules_file`. Worker-side wrapper around
/// the read-path variants of `mackes_mesh_types::window_rules::RulesError`
/// so the loop's match arms don't need to handle the write-path
/// variants.
#[derive(Debug)]
pub enum ReadError {
    /// Filesystem I/O failure.
    Io(std::io::Error),
    /// TOML parse failure.
    Parse(toml::de::Error),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Parse(e) => write!(f, "toml parse: {e}"),
        }
    }
}

impl std::error::Error for ReadError {}

/// Build the swayipc `for_window <criteria> <actions>` command
/// string for a rule. Returns `None` when the rule has no actions
/// (only `match` set) — applying it would be a no-op.
///
/// Embedded double-quotes in the match key + mark name are
/// backslash-escaped so quirky app_ids round-trip.
#[must_use]
pub fn command_for_rule(rule: &WindowRule) -> Option<String> {
    let actions = actions_for_rule(rule);
    if actions.is_empty() {
        return None;
    }
    let criterion = criterion_for_match(&rule.match_app_id);
    Some(format!(
        "for_window {criterion} {action}",
        action = actions.join(", ")
    ))
}

/// Build the `[app_id="<match>"]` criterion string for a rule.
/// The match key is JSON-string-escape-ish for `"` + `\` so
/// quirky names round-trip.
#[must_use]
pub fn criterion_for_match(match_app_id: &str) -> String {
    let escaped = match_app_id.replace('\\', "\\\\").replace('"', "\\\"");
    format!(r#"[app_id="{escaped}"]"#)
}

/// Build the per-rule action list. Empty list = no-op rule.
#[must_use]
pub fn actions_for_rule(rule: &WindowRule) -> Vec<String> {
    let mut out = Vec::new();
    if rule.floating == Some(true) {
        out.push("floating enable".to_string());
    } else if rule.floating == Some(false) {
        out.push("floating disable".to_string());
    }
    if rule.sticky == Some(true) {
        out.push("sticky enable".to_string());
    } else if rule.sticky == Some(false) {
        out.push("sticky disable".to_string());
    }
    if rule.fullscreen_on_start == Some(true) {
        out.push("fullscreen enable".to_string());
    } else if rule.fullscreen_on_start == Some(false) {
        out.push("fullscreen disable".to_string());
    }
    if let Some(bw) = rule.border_width {
        out.push(format!("border normal {bw}"));
    }
    if let Some(mark) = &rule.mark {
        let escaped = mark.replace('\\', "\\\\").replace('"', "\\\"");
        out.push(format!("mark \"{escaped}\""));
    }
    if let Some(ws) = rule.assign_workspace {
        out.push(format!("move container to workspace number {ws}"));
    }
    out
}

/// Stable signature for a rule — `<match>|<action-list-joined>`.
/// Used to detect rule drops between mtime polls.
#[must_use]
pub fn rule_signature(rule: &WindowRule) -> String {
    let actions = actions_for_rule(rule);
    format!("{}|{}", rule.match_app_id, actions.join(";"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_firefox_float() -> WindowRule {
        WindowRule {
            match_app_id: "Firefox".to_string(),
            floating: Some(true),
            sticky: None,
            fullscreen_on_start: None,
            border_width: None,
            mark: None,
            assign_workspace: None,
        }
    }

    /// Bench acceptance mirror: rule `match = "Firefox"` +
    /// `floating = true` produces
    /// `for_window [app_id="Firefox"] floating enable`.
    #[test]
    fn float_rule_canonical_command() {
        let rule = sample_firefox_float();
        assert_eq!(
            command_for_rule(&rule).unwrap(),
            r#"for_window [app_id="Firefox"] floating enable"#
        );
    }

    /// Rule with only `match` set + no actions → command_for_rule
    /// returns None (no-op).
    #[test]
    fn no_op_rule_returns_none() {
        let rule = WindowRule {
            match_app_id: "foot".to_string(),
            ..Default::default()
        };
        assert!(command_for_rule(&rule).is_none());
    }

    /// All actions composed in a single rule → comma-joined.
    #[test]
    fn full_rule_chains_all_actions() {
        let rule = WindowRule {
            match_app_id: "helix".to_string(),
            floating: Some(true),
            sticky: Some(true),
            fullscreen_on_start: Some(true),
            border_width: Some(4),
            mark: Some("editor".to_string()),
            assign_workspace: Some(3),
        };
        let cmd = command_for_rule(&rule).unwrap();
        assert_eq!(
            cmd,
            r#"for_window [app_id="helix"] floating enable, sticky enable, fullscreen enable, border normal 4, mark "editor", move container to workspace number 3"#
        );
    }

    /// `floating = false` produces `floating disable`. Locks the
    /// negative-form contract.
    #[test]
    fn explicit_false_disables() {
        let rule = WindowRule {
            match_app_id: "firefox".to_string(),
            floating: Some(false),
            ..Default::default()
        };
        assert_eq!(
            command_for_rule(&rule).unwrap(),
            r#"for_window [app_id="firefox"] floating disable"#
        );
    }

    /// Quotes + backslashes in the match key get escaped.
    #[test]
    fn criterion_escapes_quotes_and_backslashes() {
        assert_eq!(criterion_for_match("Firefox"), r#"[app_id="Firefox"]"#);
        assert_eq!(
            criterion_for_match(r#"weird"name"#),
            r#"[app_id="weird\"name"]"#
        );
        assert_eq!(
            criterion_for_match(r"path\with\slashes"),
            r#"[app_id="path\\with\\slashes"]"#
        );
    }

    /// Mark names with quotes + backslashes get escaped inside the
    /// quoted string the swayipc grammar expects.
    #[test]
    fn mark_action_escapes_quotes_and_backslashes() {
        let rule = WindowRule {
            match_app_id: "foot".to_string(),
            mark: Some(r#"quirky"mark"#.to_string()),
            ..Default::default()
        };
        assert_eq!(
            command_for_rule(&rule).unwrap(),
            r#"for_window [app_id="foot"] mark "quirky\"mark""#
        );
    }

    /// TOML round-trip: parse the canonical example body from the
    /// module docstring + serialize back. Catches schema drift.
    #[test]
    fn toml_parse_canonical_example() {
        let toml_src = r#"
schema_version = 1

[[rule]]
match = "Firefox"
floating = true

[[rule]]
match = "helix"
floating = true
sticky = true
mark = "editor"
assign_workspace = 3
"#;
        let parsed: WindowRulesFile = toml::from_str(toml_src).unwrap();
        assert_eq!(parsed.schema_version, 1);
        assert_eq!(parsed.rules.len(), 2);
        assert_eq!(parsed.rules[0].match_app_id, "Firefox");
        assert_eq!(parsed.rules[0].floating, Some(true));
        assert_eq!(parsed.rules[1].match_app_id, "helix");
        assert_eq!(parsed.rules[1].mark.as_deref(), Some("editor"));
        assert_eq!(parsed.rules[1].assign_workspace, Some(3));
    }

    /// Empty rules file (only `schema_version` set) parses to an
    /// empty rules vec.
    #[test]
    fn empty_rules_file_parses() {
        let parsed: WindowRulesFile = toml::from_str("schema_version = 1\n").unwrap();
        assert!(parsed.rules.is_empty());
    }

    /// Missing schema_version defaults to 1 — forward-compat for
    /// hand-written TOMLs that omit the field.
    #[test]
    fn missing_schema_version_defaults_to_one() {
        let toml_src = r#"
[[rule]]
match = "foot"
floating = true
"#;
        let parsed: WindowRulesFile = toml::from_str(toml_src).unwrap();
        assert_eq!(parsed.schema_version, 1);
    }

    /// `read_rules_file` returns the empty default on missing
    /// file — first-boot path.
    #[test]
    fn read_rules_file_missing_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nope/window-rules.toml");
        let parsed = read_rules_file(&path).unwrap();
        assert!(parsed.rules.is_empty());
        assert_eq!(parsed.schema_version, 1);
    }

    /// `read_rules_file` round-trips a written file.
    #[test]
    fn read_rules_file_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("window-rules.toml");
        std::fs::write(
            &path,
            r#"
schema_version = 1

[[rule]]
match = "foot"
floating = true
"#,
        )
        .unwrap();
        let parsed = read_rules_file(&path).unwrap();
        assert_eq!(parsed.rules.len(), 1);
        assert_eq!(parsed.rules[0].match_app_id, "foot");
        assert_eq!(parsed.rules[0].floating, Some(true));
    }

    /// `rule_signature` stays stable for the same rule + differs
    /// when fields change. Used to detect rule drops between
    /// polls.
    #[test]
    fn rule_signature_stable_and_distinguishing() {
        let a = sample_firefox_float();
        let b = sample_firefox_float();
        assert_eq!(rule_signature(&a), rule_signature(&b));
        let mut c = sample_firefox_float();
        c.floating = Some(false);
        assert_ne!(rule_signature(&a), rule_signature(&c));
        let mut d = sample_firefox_float();
        d.match_app_id = "chromium".to_string();
        assert_ne!(rule_signature(&a), rule_signature(&d));
    }

    /// Invalid TOML surfaces as `ReadError::Parse`.
    #[test]
    fn invalid_toml_surfaces_parse_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("broken.toml");
        std::fs::write(&path, "this is = not valid =[").unwrap();
        let err = read_rules_file(&path).unwrap_err();
        assert!(matches!(err, ReadError::Parse(_)));
    }
}
