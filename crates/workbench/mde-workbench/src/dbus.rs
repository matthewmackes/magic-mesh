//! Workbench `focus(slug)` control surface — single-instance
//! deep-link router (DBUS-3 — migrated to Bus).
//!
//! CB-1.13 lock: "`mde --focus <slug>` opens the running workbench
//! at the named panel, or launches one if none. Replaces the 1.x
//! WM_CLASS-based single-instance hack."
//!
//! Per the Q96 Bus-canonical lock (EPIC-RETIRE-DBUS), the `focus`
//! verb is served on the Bus at [`ACTION_TOPIC`]
//! (`action/shell/workbench-focus`) instead of the retired
//! `dev.mackes.MDE.Shell.Workbench.Focus` D-Bus method. Because
//! `focus` is interactive (a human clicks a taskbar entry and waits
//! for the window to raise), the responder + caller use the 40 ms
//! [`mde_bus::rpc::INTERACTIVE_POLL_INTERVAL`] (finding #1).
//!
//! The single-instance NAME [`crate::single_instance::BUS_NAME`]
//! (`dev.mackes.MDE.Workbench`) is still owned on D-Bus — name
//! ownership is inherently a D-Bus/socket primitive, the documented
//! exception per EPIC-RETIRE-DBUS finding #3. Only the method moved.

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::{reply_topic, request, INTERACTIVE_POLL_INTERVAL};

/// Bus action topic the `focus(slug)` hand-off publishes to. The
/// slug travels in the message body (empty body = "raise only").
pub const ACTION_TOPIC: &str = "action/shell/workbench-focus";

/// E0.3.1.a — read-side timeout for the Nebula status Bus probes.
/// Matches the wizard preview page's 2 s budget.
const NEBULA_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// E0.3.1.a — synchronous Bus client for mackesd's Nebula status
/// read verbs (`action/nebula/{status,self-node,list-peers}`),
/// replacing the panels' `dbus-send` / D-Bus reads of the
/// (dual-served, retiring) `dev.mackes.MDE.Nebula.Status`
/// interface. Publishes one request + blocks for the reply body on
/// a private current-thread runtime (`Persist`/rusqlite isn't
/// `Send`, so it can't ride a shared multi-thread executor).
///
/// MUST be called OUTSIDE an async runtime (it builds + drives its
/// own) — callers on the iced executor wrap it in
/// `tokio::task::spawn_blocking`. Returns `None` on no Bus
/// data-dir / persist error / timeout / no-responder; callers map
/// that to their daemon-down rendering.
#[must_use]
pub fn nebula_request(verb: &str) -> Option<String> {
    action_request(&format!("action/nebula/{verb}"), NEBULA_PROBE_TIMEOUT)
}

/// As [`nebula_request`] but with an explicit timeout. WRITE verbs
/// like `regen-certs` shell `nebula-cert` subprocesses and can run
/// for seconds, so they need more headroom than the 2 s read budget.
#[must_use]
pub fn nebula_request_with_timeout(verb: &str, timeout: Duration) -> Option<String> {
    action_request(&format!("action/nebula/{verb}"), timeout)
}

/// E0.3.x — synchronous Bus action/reply client: publish one request
/// to `topic` + block for the reply body on a private current-thread
/// runtime (`Persist`/rusqlite isn't `Send`, so it can't ride a
/// shared multi-thread executor). Generalizes the per-service
/// helpers (e.g. [`nebula_request`], the Shell liveness probe).
///
/// MUST be called OUTSIDE an async runtime — callers on the iced
/// executor wrap it in `tokio::task::spawn_blocking`. Returns `None`
/// on no Bus data-dir / persist error / timeout / no-responder.
#[must_use]
pub fn action_request(topic: &str, timeout: Duration) -> Option<String> {
    action_request_with_body(topic, None, timeout)
}

/// As [`action_request`] but carries a request `body` — the verb's
/// argument (e.g. a settings key for `action/settings/get`, or a
/// `{"key","value_json"}` object for `action/settings/set`). Same
/// current-thread-runtime contract as [`action_request`]: MUST be
/// called OUTSIDE an async runtime; callers on the iced executor
/// wrap it in `tokio::task::spawn_blocking`.
#[must_use]
pub fn action_request_with_body(
    topic: &str,
    body: Option<&str>,
    timeout: Duration,
) -> Option<String> {
    let bus_dir = mde_bus::client_data_dir()?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    rt.block_on(async {
        let persist = Persist::open(bus_dir).ok()?;
        match request(&persist, topic, Priority::Default, None, body, timeout).await {
            Ok(reply) => reply.body,
            Err(_) => None,
        }
    })
}

/// Fire-and-forget Bus publish: enqueue one request to `topic` with
/// `body` and return WITHOUT awaiting a reply. For best-effort
/// propagation pushes — e.g. `RemoteBackend`'s settings write-through
/// to mackesd's Settings responder, where the change rides onward via
/// the mesh settings sync and the caller never consumes the reply.
/// Synchronous + cheap (a single `Persist` write), so an absent
/// responder costs one db write, not a timeout. Returns `true` on a
/// successful enqueue.
pub fn action_publish(topic: &str, body: &str) -> bool {
    let Some(bus_dir) = mde_bus::client_data_dir() else {
        return false;
    };
    let Ok(persist) = Persist::open(bus_dir) else {
        return false;
    };
    mde_bus::rpc::publish_request(&persist, topic, Priority::Default, None, Some(body)).is_ok()
}

/// Normalise a focus-request body into the slug to submit. Trims
/// surrounding whitespace; a missing or whitespace-only body means
/// "raise the window, no view change" (the 1.x taskbar contract) →
/// empty string. Pure + testable.
#[must_use]
pub fn slug_from_body(body: Option<&str>) -> String {
    body.map(str::trim).unwrap_or("").to_string()
}

/// Run the focus Bus responder on the current thread, building a
/// local current-thread tokio runtime (none needed for the effect —
/// [`PendingFocus::submit`] is sync — but the structure mirrors the
/// other Bus responders and leaves room for async effects). Loops
/// until `should_stop()` returns true. `Persist` (rusqlite) isn't
/// `Send`, so this runs off the Iced/tokio main executor on its own
/// thread (see `mde-workbench` main).
pub fn serve_bus<F: Fn() -> bool>(persist: &Persist, should_stop: F) {
    let mut cursor: Option<String> = None;
    while !should_stop() {
        poll_once(persist, &mut cursor);
        std::thread::sleep(INTERACTIVE_POLL_INTERVAL);
    }
}

/// One poll sweep across [`ACTION_TOPIC`] (split out so a test can
/// drive it without the sleep loop). Each new request submits its
/// slug into [`PendingFocus`] and acknowledges on `reply/<ulid>`.
pub fn poll_once(persist: &Persist, cursor: &mut Option<String>) {
    let msgs = match persist.list_since(ACTION_TOPIC, cursor.as_deref()) {
        Ok(m) => m,
        Err(_) => return,
    };
    for msg in msgs {
        *cursor = Some(msg.ulid.clone());
        PendingFocus::submit(slug_from_body(msg.body.as_deref()));
        let _ = persist.write(&reply_topic(&msg.ulid), Priority::Default, None, Some("ok"));
    }
}

/// Cross-task focus channel — the zbus handler writes; the Iced
/// subscription reads on a 200 ms tick (cheap given Focus is a
/// user-action surface, not real-time data).
///
/// A `Mutex<Option<String>>` over a poll loop is deliberately
/// simpler than a tokio `mpsc::UnboundedReceiver` shipped through
/// a `OnceLock<Mutex<_>>` — Focus requests coalesce naturally
/// (only the latest slug matters), and the poll-tick is the same
/// rate `iced::time::every` already runs subscriptions at.
pub struct PendingFocus;

static PENDING: OnceLock<Mutex<Option<String>>> = OnceLock::new();

impl PendingFocus {
    fn slot() -> &'static Mutex<Option<String>> {
        PENDING.get_or_init(|| Mutex::new(None))
    }

    /// Write the latest focus request — overwriting any earlier
    /// unread slug (latest-wins coalescing). Returns `true`
    /// when the write happened (always true today; the Result
    /// shape leaves room for future rate-limiting).
    pub fn submit(slug: String) -> bool {
        if let Ok(mut guard) = Self::slot().lock() {
            *guard = Some(slug);
            true
        } else {
            false
        }
    }

    /// Take whatever pending slug is in the slot, leaving
    /// `None`. The Iced subscription calls this each tick.
    pub fn drain() -> Option<String> {
        Self::slot().lock().ok().and_then(|mut g| g.take())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serializes every test that touches the `PendingFocus`
    /// global. Tests in this module hold a single shared
    /// `[u8; 0]` value-less guard for their full body so
    /// concurrent runs (the default `cargo test` topology)
    /// observe sequential `submit` / `drain` interleavings.
    /// Without this guard the six `pending_focus_*` and
    /// `focus_handler_*` tests race on the process-wide slot
    /// and `cargo test` fails intermittently
    /// (OV-test-flake-1).
    static FOCUS_LOCK: Mutex<()> = Mutex::new(());

    /// Acquire the focus-test lock. Recovers from poisoning so
    /// a panicking earlier test doesn't block the rest of the
    /// suite — every test calls `reset_pending()` immediately
    /// after to scrub state.
    fn lock_focus() -> std::sync::MutexGuard<'static, ()> {
        FOCUS_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Drop the process-wide slot between tests so they don't
    /// observe each other's writes. Safe because we hold the
    /// global mutex for the swap.
    fn reset_pending() {
        if let Some(slot) = PENDING.get() {
            if let Ok(mut guard) = slot.lock() {
                *guard = None;
            }
        }
    }

    #[test]
    fn action_topic_is_under_shell_namespace() {
        assert_eq!(ACTION_TOPIC, "action/shell/workbench-focus");
        assert!(ACTION_TOPIC.starts_with("action/shell/"));
    }

    #[test]
    fn slug_from_body_trims_and_normalises_empty() {
        assert_eq!(slug_from_body(Some("network.mesh_ssh")), "network.mesh_ssh");
        assert_eq!(
            slug_from_body(Some("  network.mesh_ssh  ")),
            "network.mesh_ssh"
        );
        assert_eq!(slug_from_body(Some("   ")), "");
        assert_eq!(slug_from_body(None), "");
    }

    #[test]
    fn pending_focus_drain_returns_none_on_empty_slot() {
        let _guard = lock_focus();
        reset_pending();
        assert_eq!(PendingFocus::drain(), None);
    }

    #[test]
    fn pending_focus_round_trip_through_submit_and_drain() {
        let _guard = lock_focus();
        reset_pending();
        assert!(PendingFocus::submit("network.mesh_ssh".into()));
        assert_eq!(PendingFocus::drain(), Some("network.mesh_ssh".into()));
        assert_eq!(PendingFocus::drain(), None, "drain should clear the slot");
    }

    #[test]
    fn pending_focus_coalesces_to_latest_submit() {
        let _guard = lock_focus();
        reset_pending();
        PendingFocus::submit("apps".into());
        PendingFocus::submit("network".into());
        PendingFocus::submit("help".into());
        // Only the latest survives — Focus is a user-action
        // hand-off, not an event queue.
        assert_eq!(PendingFocus::drain(), Some("help".into()));
    }

    fn persist() -> (tempfile::TempDir, Persist) {
        let tmp = tempfile::tempdir().unwrap();
        let p = Persist::open(tmp.path().to_path_buf()).unwrap();
        (tmp, p)
    }

    #[test]
    fn poll_once_submits_slug_and_replies_ok() {
        let _guard = lock_focus();
        reset_pending();
        let (_tmp, p) = persist();
        let msg = p
            .write(ACTION_TOPIC, Priority::Default, None, Some("look_and_feel"))
            .unwrap();
        let mut cursor = None;
        poll_once(&p, &mut cursor);
        // Slug landed in the pending slot.
        assert_eq!(PendingFocus::drain(), Some("look_and_feel".to_string()));
        // Cursor advanced + an `ok` reply landed on reply/<ulid>.
        assert_eq!(cursor.as_deref(), Some(msg.ulid.as_str()));
        let replies = p.list_since(&reply_topic(&msg.ulid), None).unwrap();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].body.as_deref(), Some("ok"));
    }

    #[test]
    fn poll_once_normalises_whitespace_only_body_to_empty() {
        let _guard = lock_focus();
        reset_pending();
        let (_tmp, p) = persist();
        p.write(ACTION_TOPIC, Priority::Default, None, Some("   "))
            .unwrap();
        let mut cursor = None;
        poll_once(&p, &mut cursor);
        // Whitespace-only is "raise only" — empty slug.
        assert_eq!(PendingFocus::drain(), Some(String::new()));
    }

    #[test]
    fn poll_once_is_idempotent_via_cursor() {
        let _guard = lock_focus();
        reset_pending();
        let (_tmp, p) = persist();
        p.write(ACTION_TOPIC, Priority::Default, None, Some("apps"))
            .unwrap();
        let mut cursor = None;
        poll_once(&p, &mut cursor);
        assert_eq!(PendingFocus::drain(), Some("apps".to_string()));
        // A second sweep with no new requests submits nothing.
        poll_once(&p, &mut cursor);
        assert_eq!(PendingFocus::drain(), None);
    }
}
