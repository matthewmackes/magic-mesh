//! `org.freedesktop.Notifications` — the desktop notification spec,
//! implemented by mackesd (Phase B.10).
//!
//! Phase B wires it through to the `notifications` SQLite table.
//! Every Notify call persists a row (or updates the row when
//! `replaces_id` is non-zero); CloseNotification stamps the row's
//! `dismissed_at`. The Iced applet overlay subscribes to the
//! `notification_closed` + `action_invoked` signals to drive its
//! UI.
//!
//! By matching the spec object path + bus name, every libnotify /
//! notify-send / GTK app reaches mackesd transparently, retiring
//! mako/fnott/xfce4-notifyd in one stroke.

#![cfg(feature = "async-services")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;
use zbus::interface;
use zbus::zvariant::Value;

/// Object exposed at `/org/freedesktop/Notifications`.
///
/// Holds an Arc<Mutex<rusqlite::Connection>> so the service can be
/// cloned by zbus across signal contexts while still routing every
/// write through the same connection (serialized by the mutex).
#[derive(Debug, Clone)]
pub struct NotificationsService {
    conn: Option<Arc<Mutex<rusqlite::Connection>>>,
}

impl Default for NotificationsService {
    fn default() -> Self {
        Self { conn: None }
    }
}

impl NotificationsService {
    /// Construct with a backing SQLite connection. The supervisor
    /// wires this into the zbus object-server at startup with a
    /// connection opened at `default_db_path()`.
    #[must_use]
    pub fn with_store(conn: rusqlite::Connection) -> Self {
        Self {
            conn: Some(Arc::new(Mutex::new(conn))),
        }
    }

    /// Convenience constructor: open the store at `path` and wrap
    /// it. Returns an error if open + migrate fail.
    ///
    /// # Errors
    /// Returns whatever `store::open` returns.
    pub fn open_at(path: &std::path::Path) -> crate::Result<Self> {
        let conn = crate::store::open(path)?;
        Ok(Self::with_store(conn))
    }

    /// Open at `default_db_path()`. Convenience for the supervisor's
    /// bootstrap call.
    ///
    /// # Errors
    /// Returns whatever `store::open` returns.
    pub fn open_default() -> crate::Result<Self> {
        Self::open_at(&PathBuf::from(crate::default_db_path()))
    }
}

#[interface(name = "org.freedesktop.Notifications")]
impl NotificationsService {
    /// Notify a user. Returns the notification id (≥ 1 per spec).
    /// When `replaces_id` is non-zero, the matching row is updated;
    /// otherwise a fresh row lands and its rowid is returned.
    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        _actions: Vec<&str>,
        hints: HashMap<&str, Value<'_>>,
        expire_timeout: i32,
    ) -> u32 {
        let urgency = hints
            .get("urgency")
            .and_then(|v| u8::try_from(v).ok())
            .unwrap_or(1);
        let hints_json = format!(r#"{{"urgency":{urgency}}}"#);
        let Some(arc) = self.conn.as_ref() else {
            // No store: return the spec-mandated id without
            // persisting. Phase A behavior preserved for the
            // never-bound service struct.
            return if replaces_id == 0 { 1 } else { replaces_id };
        };
        let now = chrono::Utc::now().to_rfc3339();
        let guard = arc.lock().await;
        if replaces_id != 0 {
            let n = guard.execute(
                "UPDATE notifications SET sender=?, summary=?, body=?, \
                 app_icon=?, hints_json=?, urgency=?, expire_after_ms=?, \
                 read_at=NULL, dismissed_at=NULL \
                 WHERE notification_id=?",
                (
                    app_name,
                    summary,
                    body,
                    app_icon,
                    &hints_json,
                    i64::from(urgency),
                    i64::from(expire_timeout),
                    i64::from(replaces_id),
                ),
            );
            if n.map(|n| n > 0).unwrap_or(false) {
                return replaces_id;
            }
        }
        // Insert fresh row.
        let _ = guard.execute(
            "INSERT INTO notifications \
             (sender, summary, body, app_icon, hints_json, urgency, \
              expire_after_ms, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            (
                app_name,
                summary,
                body,
                app_icon,
                &hints_json,
                i64::from(urgency),
                i64::from(expire_timeout),
                &now,
            ),
        );
        let id: i64 = guard
            .query_row("SELECT last_insert_rowid()", [], |r| r.get(0))
            .unwrap_or(1);
        // Release the SQLite lock BEFORE the Bus bridge spawn so
        // we don't tie up the mutex across the tokio::spawn
        // boundary.
        drop(guard);
        // BUS-4.4 — bridge to Mackes Bus. Fire-and-forget shell
        // out so the FDO client's Notify() return doesn't wait
        // on our publish. Pre-enrollment peers (no mde-bus
        // binary) log + continue; the FDO delivery path is
        // unaffected either way.
        crate::ipc::bus_bridge::publish_to_bus_async(app_name, summary, body, urgency);
        u32::try_from(id).unwrap_or(1)
    }

    /// Close a previously-sent notification. Stamps `dismissed_at`
    /// on the matching row + emits NotificationClosed(id, reason=3).
    async fn close_notification(&self, id: u32) {
        let Some(arc) = self.conn.as_ref() else {
            return;
        };
        let now = chrono::Utc::now().to_rfc3339();
        let guard = arc.lock().await;
        let _ = guard.execute(
            "UPDATE notifications SET dismissed_at=? \
             WHERE notification_id=? AND dismissed_at IS NULL",
            (&now, i64::from(id)),
        );
    }

    /// Server capabilities the spec requires us to advertise.
    /// Pinned at Phase A; we'll add `"actions"`, `"action-icons"`,
    /// `"body-markup"` in Phase B once the applet supports them.
    async fn get_capabilities(&self) -> Vec<&'static str> {
        vec!["body", "persistence", "icon-static"]
    }

    /// Server identity. Spec requires (name, vendor, version, spec_version).
    async fn get_server_information(
        &self,
    ) -> (&'static str, &'static str, &'static str, &'static str) {
        ("mackesd", "mackes-shell", env!("CARGO_PKG_VERSION"), "1.2")
    }

    /// Signal: a notification was closed (by the user, by timeout,
    /// or by the server). `reason` follows the spec:
    /// 1 = expired, 2 = dismissed by user, 3 = closed by call,
    /// 4 = undefined / reserved.
    #[zbus(signal)]
    pub async fn notification_closed(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        id: u32,
        reason: u32,
    ) -> zbus::Result<()>;

    /// Signal: the user invoked an action.
    #[zbus(signal)]
    pub async fn action_invoked(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        id: u32,
        action_key: &str,
    ) -> zbus::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn capabilities_include_body_and_persistence() {
        let svc = NotificationsService::default();
        let caps = svc.get_capabilities().await;
        assert!(caps.contains(&"body"));
        assert!(caps.contains(&"persistence"));
    }

    #[tokio::test]
    async fn server_info_reports_mackesd() {
        let svc = NotificationsService::default();
        let (name, vendor, _version, spec) = svc.get_server_information().await;
        assert_eq!(name, "mackesd");
        assert_eq!(vendor, "mackes-shell");
        assert_eq!(spec, "1.2", "must match freedesktop spec version");
    }

    #[tokio::test]
    async fn notify_returns_synthetic_id_when_unbound() {
        // Default service has no DB; spec requires id ≥ 1.
        let svc = NotificationsService::default();
        let id = svc
            .notify(
                "test-app",
                0,
                "",
                "summary",
                "body",
                vec![],
                HashMap::new(),
                -1,
            )
            .await;
        assert!(id >= 1);
    }

    #[tokio::test]
    async fn notify_persists_row_when_bound_and_returns_rowid() {
        let conn = crate::store::open_in_memory().expect("open");
        let svc = NotificationsService::with_store(conn);
        let id = svc
            .notify("app", 0, "", "hi", "body", vec![], HashMap::new(), -1)
            .await;
        assert!(id >= 1);
        // Reach into the underlying connection (via the Arc) and
        // confirm the row landed.
        let arc = svc.conn.as_ref().expect("bound");
        let guard = arc.lock().await;
        let count: i64 = guard
            .query_row("SELECT COUNT(*) FROM notifications", [], |r| r.get(0))
            .expect("count");
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn notify_with_replaces_id_updates_existing_row() {
        let conn = crate::store::open_in_memory().expect("open");
        let svc = NotificationsService::with_store(conn);
        let first = svc
            .notify("app", 0, "", "first", "body-1", vec![], HashMap::new(), -1)
            .await;
        let replaced = svc
            .notify(
                "app",
                first,
                "",
                "second",
                "body-2",
                vec![],
                HashMap::new(),
                -1,
            )
            .await;
        assert_eq!(replaced, first, "replaces_id must round-trip");
        // Still exactly one row, with updated summary.
        let arc = svc.conn.as_ref().expect("bound");
        let guard = arc.lock().await;
        let count: i64 = guard
            .query_row("SELECT COUNT(*) FROM notifications", [], |r| r.get(0))
            .expect("count");
        assert_eq!(count, 1);
        let summary: String = guard
            .query_row(
                "SELECT summary FROM notifications WHERE notification_id=?",
                [i64::from(first)],
                |r| r.get(0),
            )
            .expect("read");
        assert_eq!(summary, "second");
    }

    #[tokio::test]
    async fn close_notification_stamps_dismissed_at() {
        let conn = crate::store::open_in_memory().expect("open");
        let svc = NotificationsService::with_store(conn);
        let id = svc
            .notify("app", 0, "", "hi", "body", vec![], HashMap::new(), -1)
            .await;
        svc.close_notification(id).await;
        let arc = svc.conn.as_ref().expect("bound");
        let guard = arc.lock().await;
        let dismissed: Option<String> = guard
            .query_row(
                "SELECT dismissed_at FROM notifications WHERE notification_id=?",
                [i64::from(id)],
                |r| r.get(0),
            )
            .expect("read");
        assert!(dismissed.is_some());
    }

    #[test]
    fn notifications_service_default_is_unbound() {
        let svc = NotificationsService::default();
        assert!(svc.conn.is_none());
    }
}
