//! The notification Bus seam (TERM-12).
//!
//! Publishes a desktop notice for a pane's activity / silence watcher or its
//! bell. This mirrors the TERM-9 [`crate::smart::LaunchBus`] / TERM-8
//! [`crate::remote::PtyBus`] pattern one-for-one: an injectable trait
//! ([`NotifyBus`]) so the watcher/bell fold is unit-tested headless (a recorder),
//! and a live client ([`BusNotifyClient`]) that does a synchronous local
//! `Persist` append — the same persist-first path the surface-launch client
//! uses. We do **not** hand-roll a notifier (§6): the notice rides the shell's
//! existing toast lane [`TOAST_TOPIC`] (`event/toast/show`), the exact JSON
//! boundary `mde-shell-egui`'s toast bridge already subscribes and renders, so a
//! terminal watcher/bell surfaces as a real desktop chyron with no new plumbing.

use std::path::PathBuf;

use serde::Serialize;

/// The shell's toast lane — the flat Bus topic any node/surface raises a chyron
/// on.
///
/// Mirrored from `mde-shell-egui`'s `toast_bridge`; MUST match the shell's
/// subscription, so a terminal notice renders as a real desktop toast.
pub const TOAST_TOPIC: &str = "event/toast/show";

/// The category flag chip a terminal notice wears in the toast band.
const TERM_FLAG: &str = "TERM";

/// How prominent a notice is — mapped to the toast lane's lowercase `severity`
/// contract (the wire never leaks an enum discriminant).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum NoticeLevel {
    /// Informational — a watch-for-activity / watch-for-silence fold.
    #[default]
    Info,
    /// Worth noticing — a bell, by default.
    Warning,
}

impl NoticeLevel {
    /// The lowercase wire token the toast lane decodes.
    #[must_use]
    pub const fn wire(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
        }
    }
}

/// A terminal notice to publish — the typed input to the [`NotifyBus`] seam.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TermNotice {
    /// The severity (drives the toast's colour + dwell).
    pub level: NoticeLevel,
    /// The originating mesh host (mesh identity), for the toast's source label.
    pub source_host: String,
    /// The single-line headline shown in the toast band.
    pub headline: String,
}

impl TermNotice {
    /// A notice at `level` from `host` reading `headline`.
    #[must_use]
    pub fn new(level: NoticeLevel, host: impl Into<String>, headline: impl Into<String>) -> Self {
        Self {
            level,
            source_host: host.into(),
            headline: headline.into(),
        }
    }
}

/// The `event/toast/show` wire body — a local serde mirror of the shell's toast
/// message (§6: mirror the JSON boundary, don't depend on the consumer's crate,
/// exactly as [`crate::smart::LaunchRequest`] mirrors the dock's open request).
/// `action_*` are omitted (both-or-neither, and a terminal notice carries no
/// click-through), which the consumer decodes as absent.
#[derive(Serialize)]
struct ToastWire<'a> {
    severity: &'a str,
    source_host: &'a str,
    flag: &'a str,
    headline: &'a str,
}

/// The Bus seam a terminal notice is dispatched over — publish a typed toast the
/// shell's notify hub raises.
///
/// Injectable so the watcher/bell fold is unit-tested headless (a recorder)
/// while production talks the live Bus ([`BusNotifyClient`]).
pub trait NotifyBus: Send + Sync {
    /// Publish `notice` on [`TOAST_TOPIC`].
    ///
    /// # Errors
    /// An operator-readable string when the append can't be written (e.g. no
    /// Bus dir on this node); it never blocks.
    fn notify(&self, notice: &TermNotice) -> Result<(), String>;
}

/// The live Bus-backed notifier — a synchronous local `Persist` append, the same
/// persist-first path the surface-launch client uses. Degrades honestly to an
/// error when this node has no Bus dir.
#[derive(Debug, Clone)]
pub struct BusNotifyClient {
    bus_root: Option<PathBuf>,
}

impl BusNotifyClient {
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

impl NotifyBus for BusNotifyClient {
    fn notify(&self, notice: &TermNotice) -> Result<(), String> {
        let Some(root) = self.bus_root.as_ref() else {
            return Err(
                "No mesh Bus directory — can't raise a terminal notice on this node.".to_string(),
            );
        };
        let body = serde_json::to_string(&ToastWire {
            severity: notice.level.wire(),
            source_host: &notice.source_host,
            flag: TERM_FLAG,
            headline: &notice.headline,
        })
        .map_err(|e| format!("Couldn't encode the terminal notice: {e}"))?;
        mde_bus::persist::Persist::open(root.clone())
            .and_then(|p| {
                p.write(
                    TOAST_TOPIC,
                    mde_bus::hooks::config::Priority::Default,
                    None,
                    Some(&body),
                )
            })
            .map(|_| ())
            .map_err(|e| format!("Couldn't publish the terminal notice: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_maps_to_the_wire_severity_contract() {
        assert_eq!(NoticeLevel::Info.wire(), "info");
        assert_eq!(NoticeLevel::Warning.wire(), "warning");
    }

    #[test]
    fn a_client_without_a_bus_dir_errors_rather_than_panics() {
        let client = BusNotifyClient::with_root(None);
        assert!(client
            .notify(&TermNotice::new(NoticeLevel::Info, "oak", "activity"))
            .is_err());
    }

    #[test]
    fn a_published_notice_lands_on_the_toast_lane_with_the_term_flag() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        BusNotifyClient::with_root(Some(root.clone()))
            .notify(&TermNotice::new(
                NoticeLevel::Warning,
                "oak",
                "bell in bash",
            ))
            .expect("publish");

        let persist = mde_bus::persist::Persist::open(root).expect("open");
        let msgs = persist.list_since(TOAST_TOPIC, None).expect("list");
        assert_eq!(msgs.len(), 1, "one notice on the toast lane");
        let body = msgs[0].body.as_deref().expect("body");
        // Decode as the loose shape the shell's toast bridge accepts.
        let v: serde_json::Value = serde_json::from_str(body).expect("json");
        assert_eq!(v["severity"], "warning");
        assert_eq!(v["source_host"], "oak");
        assert_eq!(v["flag"], TERM_FLAG);
        assert_eq!(v["headline"], "bell in bash");
    }
}
