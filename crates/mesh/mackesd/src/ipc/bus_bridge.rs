//! BUS-4.4 — bridge `org.freedesktop.Notifications.Notify` to
//! `mde-bus publish` on the `fdo/<app>` topic namespace.
//!
//! Every successful Notify call from this peer also publishes
//! the same notification through the Mackes Bus so:
//!
//! 1. The Bus surface dispatcher (BUS-2.1) sees the message
//!    and lights up tray + badge / status strip / Theater
//!    depending on the urgency hint.
//! 2. The per-peer SQLite index (BUS-1.4) carries the message
//!    for tail / history queries.
//! 3. The JSONL audit log (BUS-7.1) records who / when / what
//!    topic / what priority / which ULID.
//! 4. The cross-peer GFS file tree carries the notification to
//!    every other peer (which is the BUS-4.2 retirement of the
//!    legacy `notification_relay` cross-peer mirror).
//!
//! **Bridge architecture.** The mackesd process invokes
//! `mde-bus publish ... --no-broker` as a child process. Fire-
//! and-forget — the Notify call returns to the FDO client
//! immediately while the Bus publish runs in the background.
//! `--no-broker` means "persist + audit only, skip the outbound
//! ntfy POST" so the bridge works even when the broker isn't
//! up yet (pre-enrollment peer). The persistence layer + audit
//! log are reached regardless.
//!
//! Why shell-out and not a library dep? Adding `mde-bus` as a
//! direct dep would pull `axum + reqwest + rusqlite + ulid +
//! tera` into mackesd. The shell-out is one process spawn per
//! Notify call (~1 ms overhead, well below the typical
//! FDO-client expectation) and keeps mackesd's binary lean.

/// Sanitize a free-form FDO app name into a Bus-topic-safe
/// path component. Lowercased; non-alphanumeric chars become
/// hyphens; leading/trailing hyphens stripped; empty → "unknown".
///
/// Examples:
///
/// - `"Slack"` → `"slack"`
/// - `"Org.Gnome.Calendar"` → `"org-gnome-calendar"`
/// - `"   "` → `"unknown"`
#[must_use]
pub fn sanitize_app_name(app: &str) -> String {
    let s: String = app
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = s.trim_matches('-').to_string();
    // Collapse runs of `-` to single hyphens for readability.
    let mut out = String::with_capacity(trimmed.len());
    let mut prev_hyphen = false;
    for c in trimmed.chars() {
        if c == '-' {
            if !prev_hyphen {
                out.push(c);
            }
            prev_hyphen = true;
        } else {
            out.push(c);
            prev_hyphen = false;
        }
    }
    if out.is_empty() {
        "unknown".to_string()
    } else {
        out
    }
}

/// Map an FDO urgency hint (0 = low, 1 = normal, 2 = critical)
/// to a Bus priority string. Per the design-doc §6 surface lock:
///
/// - urgency 0 → `min` (silent log; the FDO client signalled
///   "background information only")
/// - urgency 1 → `default` (tray + dock badge)
/// - urgency 2 → `urgent` (Theater takeover + wallpaper +
///   phone push — FDO `critical` is "user action required")
///
/// `high` priority is never produced by the FDO bridge —
/// the FDO spec only defines three urgency levels and `high`
/// (status-strip + sound, persistent until ack) is a Bus
/// concept that publishers opt into deliberately.
#[must_use]
pub fn priority_for_urgency(urgency: u8) -> &'static str {
    match urgency {
        0 => "min",
        2 => "urgent",
        _ => "default",
    }
}

/// Compute the full topic string for an FDO notification.
#[must_use]
pub fn topic_for_app(app: &str) -> String {
    format!("fdo/{}", sanitize_app_name(app))
}

/// Fire the `mde-bus publish ...` shell-out for this
/// notification. Detaches from the caller so the FDO Notify
/// return path isn't blocked on the publish.
///
/// Best-effort: missing `mde-bus` binary, non-zero exit, etc.
/// all log to tracing and don't surface as errors. The FDO
/// client never sees the bridge failure — by spec the Notify
/// return value is the notification id, not a success flag.
pub fn publish_to_bus_async(app: &str, summary: &str, body: &str, urgency: u8) {
    let topic = topic_for_app(app);
    let priority_str = priority_for_urgency(urgency);
    let summary = summary.to_string();
    let body = body.to_string();
    tokio::spawn(async move {
        let result = tokio::process::Command::new("mde-bus")
            .args([
                "publish",
                &topic,
                "--priority",
                priority_str,
                "--title",
                &summary,
                "--body-flag",
                &body,
                // Persist + audit reach without needing the
                // broker — important pre-enrollment so we
                // still record FDO traffic.
                "--no-broker",
            ])
            .output()
            .await;
        match result {
            Ok(out) if out.status.success() => {}
            Ok(out) => {
                tracing::debug!(
                    target: "mackesd::bus_bridge",
                    topic = %topic,
                    exit_code = ?out.status.code(),
                    "mde-bus publish exited non-zero (FDO notification still delivered)"
                );
            }
            Err(e) => {
                tracing::debug!(
                    target: "mackesd::bus_bridge",
                    error = %e,
                    "mde-bus binary not invocable (likely pre-RPM-install dev env)"
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_lowercases_and_hyphenates() {
        assert_eq!(sanitize_app_name("Slack"), "slack");
        assert_eq!(sanitize_app_name("Firefox"), "firefox");
        assert_eq!(
            sanitize_app_name("Org.Gnome.Calendar"),
            "org-gnome-calendar"
        );
        assert_eq!(
            sanitize_app_name("Visual Studio Code"),
            "visual-studio-code"
        );
    }

    #[test]
    fn sanitize_strips_leading_trailing_hyphens() {
        assert_eq!(sanitize_app_name(".dotfile"), "dotfile");
        assert_eq!(sanitize_app_name("__weird__"), "weird");
        assert_eq!(sanitize_app_name(" -- spaced -- "), "spaced");
    }

    #[test]
    fn sanitize_collapses_hyphen_runs() {
        assert_eq!(sanitize_app_name("a   b"), "a-b");
        assert_eq!(sanitize_app_name("a...b"), "a-b");
        assert_eq!(sanitize_app_name("a.-.b"), "a-b");
    }

    #[test]
    fn sanitize_falls_back_to_unknown_on_empty() {
        assert_eq!(sanitize_app_name(""), "unknown");
        assert_eq!(sanitize_app_name("   "), "unknown");
        assert_eq!(sanitize_app_name("..."), "unknown");
    }

    #[test]
    fn topic_for_app_prefixes_fdo() {
        assert_eq!(topic_for_app("Slack"), "fdo/slack");
        assert_eq!(
            topic_for_app("Org.Gnome.Calendar"),
            "fdo/org-gnome-calendar"
        );
        assert_eq!(topic_for_app(""), "fdo/unknown");
    }

    #[test]
    fn priority_for_urgency_maps_three_levels() {
        assert_eq!(priority_for_urgency(0), "min");
        assert_eq!(priority_for_urgency(1), "default");
        assert_eq!(priority_for_urgency(2), "urgent");
        // Unknown urgency hint → default (safe).
        assert_eq!(priority_for_urgency(255), "default");
        assert_eq!(priority_for_urgency(3), "default");
    }
}
