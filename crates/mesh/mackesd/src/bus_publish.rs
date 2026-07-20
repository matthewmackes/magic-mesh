//! In-process Bus publish (perf-10) — write directly through the local
//! `mde_bus` [`Persist`] store instead of fork+exec'ing the `mde-bus` CLI once
//! per message.
//!
//! **Why.** ~24 mackesd workers publish a tick summary to the *local* bus by
//! `std::process::Command::new("mde-bus").args(["publish", …])` +
//! [`crate::proc_reap::fire_and_reap`]. Every such publish paid for a whole new
//! process, a fresh SQLite open (schema exec + chmod + write-probe), AND a
//! dedicated 100 ms-poll reaper thread — for a write the daemon can do in-process
//! against the very same store the CLI opens. This module is the shared
//! in-process path: a long-lived worker opens one [`Persist`] handle and reuses
//! it across ticks, so a publish is a single `INSERT` + atomic file write with no
//! spawn, no reaper, no zombie.
//!
//! **Byte-identical to the CLI.** The `mde-bus publish <topic> --body-flag <json>`
//! form the workers shelled out resolves to
//! `Persist::write_full(topic, Priority::Default, None, Some(&body), &[], None)`
//! (default priority, no title, no action buttons, no `reply_to`), which is
//! exactly [`Persist::write`]. This helper serializes the payload with the same
//! compact `serde_json::to_string` the call sites passed to `--body-flag`, so the
//! stored row (topic, priority `"default"`, null title, JSON body, empty actions,
//! null `reply_to`) is identical to a CLI publish. Retention, audit emission, and
//! GFS replication all key off the stored row, so those are unchanged too.
//!
//! **Best-effort, matching CLI fire-and-forget.** A serialize failure or a
//! store-write error is logged at `debug` and swallowed — the caller
//! graceful-degrades exactly as it did when the `mde-bus` binary was absent on a
//! pre-RPM dev box (the old `fire_and_reap` swallowed the spawn error).

use mde_bus::hooks::config::Priority;
use mde_bus::persist::{Persist, StoredMessage};

/// The default bus root a CLI publish would use — honours `MDE_BUS_ROOT`
/// (the live fleet's shared-spool pin, `90-mde-bus.conf`) FIRST, then the
/// `~/.local/share/mde/bus` XDG fallback. This is [`mde_bus::default_data_dir`],
/// the SAME resolver the fork+exec'd `mde-bus` inherited via the environment —
/// using `dirs::data_dir()` directly (as a couple of older workers do) would
/// silently diverge whenever `MDE_BUS_ROOT` is set.
#[must_use]
pub fn default_bus_root() -> Option<std::path::PathBuf> {
    mde_bus::default_data_dir()
}

/// Publish `payload` (JSON-serialized) to `topic` in-process, byte-identical to
/// `mde-bus publish <topic> --body-flag <json>`. Returns the [`StoredMessage`]
/// on success, `None` on a swallowed serialize/write failure (best-effort).
///
/// Takes `&mut Persist` so it can [`Persist::reopen_if_index_changed`] before the
/// write: a fork+exec CLI opened a fresh handle on every call and thus always
/// followed a rotated `index.sqlite` inode (BUS-INODE-ORPHAN-1 / MUSIC-WEDGE-2).
/// A long-held in-process handle must re-follow that inode explicitly to keep the
/// same "always sees the live DB" property — the cheap stat is what preserves
/// behaviour-parity with the per-call open the CLI did.
pub fn publish_json<T: serde::Serialize>(
    persist: &mut Persist,
    topic: &str,
    payload: &T,
) -> Option<StoredMessage> {
    let body = match serde_json::to_string(payload) {
        Ok(b) => b,
        Err(e) => {
            // WL-RUN-002 — a serialize failure means the publish never
            // reaches the store; count it as a bus publish error too so
            // `mackesd_bus_publish_errors_total` captures every failed
            // publish, not just store-write failures.
            crate::metrics::record_bus_publish_error();
            tracing::debug!(
                target: "mackesd::bus_publish",
                topic,
                error = %e,
                "bus publish serialize failed",
            );
            return None;
        }
    };
    write_body(persist, topic, &body)
}

/// Publish an ALREADY-SERIALIZED `body` string to `topic` in-process,
/// byte-identical to `mde-bus publish <topic> --body-flag <body>`. Returns the
/// [`StoredMessage`] on success, `None` on a swallowed write failure
/// (best-effort).
///
/// This is the raw-string sibling of [`publish_json`]. Many workers hand-build
/// the JSON body up front (`serde_json::json!(…).to_string()`, a `format!`
/// template, or a `Record::body()` accessor) and shelled out passing that string
/// verbatim to `--body-flag`. Feeding such a string to [`publish_json`] would
/// serialize it a SECOND time (wrapping the whole document in quotes + escaping
/// every `"`), so those call sites must use this helper, which writes the string
/// through unchanged — exactly what the CLI's `--body-flag` did.
pub fn publish_body(persist: &mut Persist, topic: &str, body: &str) -> Option<StoredMessage> {
    write_body(persist, topic, body)
}

/// Shared tail of [`publish_json`] / [`publish_body`]: follow a rotated index,
/// then write the row with the CLI-default envelope (default priority, no
/// title/actions/reply), swallowing a write error at `debug` (best-effort).
fn write_body(persist: &mut Persist, topic: &str, body: &str) -> Option<StoredMessage> {
    // Follow a rotated index (retention recreate / BOOT-REC-3 unlink) — a no-op
    // fast stat when nothing changed.
    persist.reopen_if_index_changed();
    match persist.write(topic, Priority::Default, None, Some(body)) {
        Ok(msg) => Some(msg),
        Err(e) => {
            // WL-RUN-002 — the single chokepoint every in-process bus
            // publish (`publish_json` / `publish_body`) funnels through.
            // A swallowed write failure still bumps the process-wide
            // `mackesd_bus_publish_errors_total` counter so the
            // best-effort degrade stays observable in the scrape.
            crate::metrics::record_bus_publish_error();
            tracing::debug!(
                target: "mackesd::bus_publish",
                topic,
                error = %e,
                "bus publish write failed",
            );
            None
        }
    }
}

/// Open a long-lived [`Persist`] handle at `bus_root` for the in-process publish
/// path, best-effort. Returns `None` (with a `debug` note) when the root is
/// absent (pre-RPM dev box / tests pass `None`) or the open fails — the caller
/// then graceful-degrades exactly as the old fork+exec path did when `mde-bus`
/// was missing (the publish becomes a swallowed no-op).
#[must_use]
pub fn open_bus(bus_root: Option<std::path::PathBuf>) -> Option<Persist> {
    let root = bus_root?;
    match Persist::open(root) {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::debug!(
                target: "mackesd::bus_publish",
                error = %e,
                "bus open failed; in-process publish disabled this run",
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
    struct Sample {
        host: String,
        n: u32,
    }

    /// The in-process publish stores the SAME row a CLI publish would: same
    /// topic, same compact-JSON body, default priority, no title/actions/reply.
    #[test]
    fn publish_json_writes_the_cli_equivalent_row() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let payload = Sample {
            host: "node-a".to_string(),
            n: 7,
        };

        let mut persist = Persist::open(root.clone()).unwrap();
        let stored =
            publish_json(&mut persist, "event/kvm/services", &payload).expect("in-process publish");

        // The body the CLI would carry in `--body-flag` is the exact same
        // compact serialization.
        let cli_body = serde_json::to_string(&payload).unwrap();
        assert_eq!(stored.body.as_deref(), Some(cli_body.as_str()));
        assert_eq!(stored.topic, "event/kvm/services");
        assert_eq!(stored.priority, "default");
        assert!(stored.title.is_none());
        assert!(stored.actions.is_empty());
        assert!(stored.reply_to.is_none());

        // Read the row back through a FRESH handle (as any bus consumer does)
        // and confirm exactly one message with the identical body landed.
        let reader = Persist::open(root).unwrap();
        let rows = reader.list_since("event/kvm/services", None).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].body.as_deref(), Some(cli_body.as_str()));
        // And it round-trips back to the original typed payload.
        let back: Sample = serde_json::from_str(rows[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(back, payload);
    }

    /// Two publishes on one long-lived handle both land (proves the reused
    /// handle keeps writing — the fork+exec path opened a new handle each time).
    #[test]
    fn reused_handle_publishes_repeatedly() {
        let tmp = tempfile::tempdir().unwrap();
        let mut persist = Persist::open(tmp.path().to_path_buf()).unwrap();
        publish_json(
            &mut persist,
            "event/kvm/services",
            &Sample {
                host: "a".into(),
                n: 1,
            },
        )
        .expect("first");
        publish_json(
            &mut persist,
            "event/kvm/services",
            &Sample {
                host: "a".into(),
                n: 2,
            },
        )
        .expect("second");
        let rows = persist.list_since("event/kvm/services", None).unwrap();
        assert_eq!(rows.len(), 2);
    }

    /// [`publish_body`] writes an ALREADY-SERIALIZED JSON string through
    /// unchanged — byte-identical to `--body-flag <body>`, NOT re-serialized.
    /// This is the property the string-body call sites (dc_* records,
    /// selinux/firewall alerts, clipboard) rely on.
    #[test]
    fn publish_body_writes_string_verbatim_not_reserialized() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        // The exact JSON a `format!`/`json!().to_string()` call site built.
        let body = r#"{"host":"node-a","alert":true,"count":3}"#;

        let mut persist = Persist::open(root.clone()).unwrap();
        let stored =
            publish_body(&mut persist, "event/firewall/node-a", body).expect("in-process publish");

        // Stored body is the string VERBATIM — a `publish_json` on the same
        // string would instead store `"{\"host\":\"node-a\",…}"` (quoted +
        // escaped), which would NOT equal `body`.
        assert_eq!(stored.body.as_deref(), Some(body));
        assert_ne!(
            stored.body.as_deref(),
            Some(serde_json::to_string(body).unwrap().as_str()),
            "publish_body must NOT double-encode the string",
        );
        assert_eq!(stored.topic, "event/firewall/node-a");
        assert_eq!(stored.priority, "default");
        assert!(stored.title.is_none());
        assert!(stored.actions.is_empty());
        assert!(stored.reply_to.is_none());

        // Read back through a fresh handle: exactly one row, identical body.
        let reader = Persist::open(root).unwrap();
        let rows = reader.list_since("event/firewall/node-a", None).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].body.as_deref(), Some(body));
    }

    /// [`open_bus`] returns `None` for a `None` root (test / pre-RPM parity) and
    /// `Some` for a real root, so a caller's publish graceful-degrades to a
    /// swallowed no-op exactly as the old fork+exec did when `mde-bus` was
    /// absent.
    #[test]
    fn open_bus_none_root_disables_publish() {
        assert!(open_bus(None).is_none());
        let tmp = tempfile::tempdir().unwrap();
        assert!(open_bus(Some(tmp.path().to_path_buf())).is_some());
    }
}
