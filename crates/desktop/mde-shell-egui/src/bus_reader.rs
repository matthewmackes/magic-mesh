//! arch-11 — the shared Bus-reader seam.
//!
//! Before this, ~16 shell poll/render sites each hand-rolled the SAME two-step
//! "resolve the client `bus_root`, then `Persist::open` it fail-soft" prelude,
//! and several reader modules (e.g. `phones_hub`, `iac`) each carried a private
//! copy of the identical `Persist::open(root).ok()` helper. There was no single
//! seam — every reader re-derived the open. This module is that ONE seam: the
//! fail-soft readers borrow it instead of re-deriving how to open the store.
//!
//! **Why it opens per call rather than caching a `Connection`.** perf-3 already
//! made a repeat `Persist::open` of an already-initialized spool a cheap
//! bare-connection fast path, and opening per call keeps the BUS-INODE-ORPHAN-1
//! self-heal intact — a long-lived cached `Connection` would strand on a
//! recreated (new-inode) `index.sqlite` and silently stop seeing writes, the
//! "daemon not responding after long uptime" wedge. So the seam is the single
//! *source of the open + fail-soft policy*, behaviour-identical to the sites it
//! replaces, with room to grow read helpers behind the same API later.
//!
//! **Scope: fail-soft READERS only.** A missing/unopenable spool is the honest
//! off-mesh state (§7) — a silent `None`, no error. Publish (writer) sites keep
//! their own `Persist::open` because they need the `Result`'s error text to set
//! `last_error` / log a down lane; a fail-soft `Option` opener would swallow it.

use std::path::PathBuf;

use mde_bus::persist::{Persist, StoredMessage};

/// A cheap, cloneable handle over a resolved Bus spool path — the shared seam
/// the shell's fail-soft readers open through.
///
/// Holds only the `bus_root` (the same already-resolved `Option<PathBuf>` the
/// poller states carry, from [`mde_bus::client_data_dir`]); every read opens
/// through the perf-3 fast path.
#[derive(Debug, Clone, Default)]
pub(crate) struct BusReader {
    bus_root: Option<PathBuf>,
}

impl BusReader {
    /// Wrap an already-resolved bus root. The poller states keep their own
    /// resolved `bus_root` field and hand it here at the open point.
    pub(crate) fn new(bus_root: Option<PathBuf>) -> Self {
        Self { bus_root }
    }

    /// Resolve the desktop-client bus spool via the canonical GUI resolution
    /// ([`mde_bus::client_data_dir`]: `MDE_BUS_ROOT` → live system bus →
    /// per-HOME). For sites that resolve the path right at the open point.
    #[allow(dead_code)] // ctor kept available for sites migrated incrementally
    pub(crate) fn client() -> Self {
        Self::new(mde_bus::client_data_dir())
    }

    /// Open the store fail-soft: `None` when there is no configured spool OR the
    /// open fails — the honest off-mesh no-op (§7). This is the exact idiom the
    /// per-module `persist()` / `open_persist()` helpers had, now shared. Callers
    /// that need the raw handle (multi-topic folds, helpers taking `&Persist`)
    /// use this and then read off the returned [`Persist`] exactly as before.
    pub(crate) fn open(&self) -> Option<Persist> {
        Persist::open(self.bus_root.clone()?).ok()
    }

    /// The newest (latest-wins) message retained on `topic`, or `None` when there
    /// is no configured spool / it won't open / the topic carries no messages.
    ///
    /// This is the shared *latest-value* read the render path wants: the seam
    /// opens fail-soft (via [`open`](Self::open), keeping the BUS-INODE-ORPHAN-1
    /// self-heal — it opens per call, never caches a `Connection`) and then a
    /// single bounded [`Persist::read_latest`] index probe (perf-4:
    /// `ORDER BY ulid DESC LIMIT 1`, NOT a full-history `list_since(topic, None)`
    /// load discarded to its tail). It replaces the reader sites that hand-rolled
    /// "open, `list_since(topic, None)`, `.pop()`/`.next_back()` the newest,
    /// decode" — behaviour-identical to that `.pop()` (the newest row; `None`
    /// when the topic is empty), just without loading the retained backlog.
    pub(crate) fn latest(&self, topic: &str) -> Option<StoredMessage> {
        self.open()?.read_latest(topic).ok().flatten()
    }

    /// The newest message body on `topic`, decoded from JSON into `T` — `None` on
    /// no spool / no message / an absent-or-undecodable body. The plain-`serde`
    /// twin of [`latest`](Self::latest) for the common
    /// "open, newest row, `serde_json::from_str`" mirror read (the same decode
    /// the shell's `state/*` latest-wins mirrors share); callers whose wire type
    /// needs bespoke validation (e.g. the browser status parsers) use
    /// [`latest`](Self::latest) and validate the body themselves.
    #[allow(dead_code)] // serde twin of `latest`; covered by the wire-contract fixtures
    pub(crate) fn latest_json<T: serde::de::DeserializeOwned>(&self, topic: &str) -> Option<T> {
        let body = self.latest(topic)?.body?;
        serde_json::from_str::<T>(&body).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_bus::hooks::config::Priority;

    #[test]
    fn open_is_none_without_a_spool() {
        // No configured bus root → the honest off-mesh None, never a panic.
        assert!(BusReader::new(None).open().is_none());
    }

    #[test]
    fn open_reads_back_what_was_written() {
        // A configured, openable spool yields a live handle whose reads match a
        // direct Persist::open — the seam is behaviour-identical to the prelude
        // it replaces.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        Persist::open(root.clone())
            .unwrap()
            .write("t/x", Priority::Default, None, Some("hi"))
            .unwrap();
        let reader = BusReader::new(Some(root));
        let persist = reader.open().expect("openable spool yields a handle");
        let msgs = persist.list_since("t/x", None).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].body.as_deref(), Some("hi"));
    }

    #[test]
    fn latest_is_none_without_a_spool_or_message() {
        // No spool → None (the honest off-mesh state); an openable-but-empty
        // topic → None, never a panic — the same fail-soft as `open`.
        assert!(BusReader::new(None).latest("t/x").is_none());
        let tmp = tempfile::tempdir().unwrap();
        let reader = BusReader::new(Some(tmp.path().to_path_buf()));
        assert!(reader.latest("t/x").is_none());
    }

    #[test]
    fn latest_returns_the_newest_row() {
        // `latest` is the newest-wins read: with several messages on a topic it
        // yields the most-recent body, matching the `list_since(topic, None)`
        // tail the reader sites used to `.pop()`.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let persist = Persist::open(root.clone()).unwrap();
        persist
            .write("state/x", Priority::Default, None, Some("old"))
            .unwrap();
        persist
            .write("state/x", Priority::Default, None, Some("new"))
            .unwrap();
        let reader = BusReader::new(Some(root));
        assert_eq!(
            reader.latest("state/x").and_then(|m| m.body).as_deref(),
            Some("new"),
        );
    }

    #[test]
    fn latest_json_decodes_the_newest_body() {
        // The serde twin: it decodes the newest body into `T`, returning None on
        // an undecodable body (never a panic) — the shared "current mirrored
        // value" read the `state/*` latest-wins mirrors want.
        #[derive(serde::Deserialize, PartialEq, Debug)]
        struct Mirror {
            n: u32,
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let persist = Persist::open(root.clone()).unwrap();
        persist
            .write("state/m", Priority::Default, None, Some(r#"{"n":1}"#))
            .unwrap();
        persist
            .write("state/m", Priority::Default, None, Some(r#"{"n":2}"#))
            .unwrap();
        let reader = BusReader::new(Some(root));
        assert_eq!(
            reader.latest_json::<Mirror>("state/m"),
            Some(Mirror { n: 2 })
        );
        // A wire-shape drift (garbage body) is a silent None, not a panic.
        persist
            .write("state/m", Priority::Default, None, Some("not json"))
            .unwrap();
        assert_eq!(reader.latest_json::<Mirror>("state/m"), None);
    }
}
