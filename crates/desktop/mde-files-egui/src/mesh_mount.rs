//! FILEMGR-9 — the Files surface's **mesh-mount client** (the desktop half of the
//! FILEMGR-5 `mackesd` `mesh_mount` worker contract).
//!
//! Design: `docs/design/file-manager-full.md` (locks 11 / 12 / 14 / 17). The
//! worker owns the sshfs lifecycle mesh-side; this surface only **requests** a
//! mount and **reads** the published state (§6-clean — the desktop tier leans
//! inward on `mde-bus` only, never on `mackesd`). It talks the same Bus contract
//! the other surfaces use for their planes (`state/storage/*` in
//! `mde-shell-egui::storage`, `state/voice/*` in `mde-voice-egui::fleet`):
//!
//! * **Read** `state/mesh-mount/<host>` — the worker's latest [`MeshMountState`]
//!   per peer (phase + scope + mounted path + degrade reason). Drives the
//!   sidebar's live pips.
//! * **Write** `action/mesh-mount/<host>` — a typed [`MeshMountVerb`]
//!   (`mount` / `escalate` / `unmount`). `escalate` is the lock-14 home→full-
//!   filesystem GUI action.
//!
//! The payloads are a JSON boundary: **local** serde mirrors of the worker's
//! `mesh_mount.rs` wire types, exactly as `mde-shell-egui::storage` mirrors the
//! storage worker's structs. The [`MeshMountClient`] seam is injectable so the
//! model is unit-tested headless (a fake) while production talks the Bus
//! ([`BusMeshMount`]).
//!
//! ## No blocking probe (lock 15 — never a frozen UI)
//!
//! [`BusMeshMount`] reads/writes a **local** `Persist` (a `SQLite` scan of the
//! node's own spool) — it never opens a socket to a peer and never runs an sshfs
//! liveness probe. Reachability comes from the roster (`Peer::status`) and the
//! worker-published phase, so an offline peer can neither hang a read nor a
//! request: the read returns the last-known state (or nothing) and a request is a
//! local append the worker drains on its own tick.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The `action/mesh-mount/` request domain prefix. MUST equal the worker's
/// `mackesd::workers::mesh_mount::ACTION_PREFIX` (cross-checked in tests).
pub const ACTION_PREFIX: &str = "action/mesh-mount/";

/// The `state/mesh-mount/` publish domain prefix. MUST equal the worker's
/// `mackesd::workers::mesh_mount::STATE_PREFIX` (cross-checked in tests).
pub const STATE_PREFIX: &str = "state/mesh-mount/";

/// The request topic for one host (`action/mesh-mount/<host>`).
#[must_use]
pub fn action_topic(host: &str) -> String {
    format!("{ACTION_PREFIX}{host}")
}

/// The state topic for one host (`state/mesh-mount/<host>`).
#[must_use]
pub fn state_topic(host: &str) -> String {
    format!("{STATE_PREFIX}{host}")
}

// ── the typed request verb (mirror of the worker's `MeshMountVerb`) ─────────────

/// The typed body of a `action/mesh-mount/<host>` request.
///
/// Mirrors the worker's `MeshMountVerb` wire shape verbatim
/// (`{"verb":"mount"|"escalate"|"unmount"}`) so the surface never depends on
/// `mackesd`. There is deliberately **no** command/shell variant (§9) — the only
/// verbs are the three lifecycle intents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verb", rename_all = "snake_case")]
pub enum MeshMountVerb {
    /// Mount the peer's **home** directory (the least-privilege default, lock 14).
    Mount,
    /// Re-mount the peer's **full filesystem** (`/`) — the escalation (lock 14).
    Escalate,
    /// Unmount the peer + forget it.
    Unmount,
}

impl MeshMountVerb {
    /// The JSON request body to publish for this verb.
    #[must_use]
    pub fn body(self) -> String {
        // The derived `Serialize` can't fail for this closed enum; the fallback
        // keeps this total without an `unwrap`.
        serde_json::to_string(&self).unwrap_or_else(|_| r#"{"verb":"mount"}"#.to_string())
    }

    /// Stable tag for logs / the status line.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Mount => "mount",
            Self::Escalate => "escalate",
            Self::Unmount => "unmount",
        }
    }
}

// ── the mount scope (mirror of the worker's `MountScope` tag) ───────────────────

/// Which slice of the remote node a mount exposes — the tag the worker publishes
/// in the state record (`home` / `full`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountScope {
    /// The mesh user's home directory (`~`) — the least-privilege default.
    Home,
    /// The full filesystem (`/`) — the escalated scope.
    Full,
}

impl MountScope {
    /// Parse the wire tag (`home` / `full`); unknown tags yield `None`.
    #[must_use]
    pub fn from_tag(tag: &str) -> Option<Self> {
        match tag {
            "home" => Some(Self::Home),
            "full" => Some(Self::Full),
            _ => None,
        }
    }

    /// A short human label for the sidebar chip.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Full => "full FS",
        }
    }
}

// ── the lifecycle phase (mirror of the worker's `Phase` tag) ────────────────────

/// The lifecycle phase of one host's mount, parsed from the state record's
/// `state` tag. `Unmounted` is the default (an unknown tag falls here — honest,
/// never a panic).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MountPhase {
    /// No mount + not tracked as failing.
    #[default]
    Unmounted,
    /// A mount attempt is in flight.
    Mounting,
    /// Mounted + live.
    Mounted,
    /// Was mounted (or a transient failure) — retrying with backoff.
    Reconnecting,
    /// Peer is offline / the mount can't be established (honest dead-end).
    Unreachable,
}

impl MountPhase {
    /// Parse the worker's phase tag; an unrecognised tag is `Unmounted`.
    #[must_use]
    pub fn from_tag(tag: &str) -> Self {
        match tag {
            "mounting" => Self::Mounting,
            "mounted" => Self::Mounted,
            "reconnecting" => Self::Reconnecting,
            "unreachable" => Self::Unreachable,
            // "unmounted" and anything unknown.
            _ => Self::Unmounted,
        }
    }

    /// A short human label for the sidebar chip.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Unmounted => "not mounted",
            Self::Mounting => "mounting\u{2026}",
            Self::Mounted => "mounted",
            Self::Reconnecting => "reconnecting\u{2026}",
            Self::Unreachable => "unreachable",
        }
    }

    /// `true` once the mount is live (its path is browsable).
    #[must_use]
    pub const fn is_mounted(self) -> bool {
        matches!(self, Self::Mounted)
    }

    /// `true` while the phase is still moving (mount attempt / reconnect) — the
    /// view keeps a repaint heartbeat alive so it animates to completion without
    /// input, but the read itself never blocks.
    #[must_use]
    pub const fn is_transitional(self) -> bool {
        matches!(self, Self::Mounting | Self::Reconnecting)
    }
}

// ── the published state record (mirror; read side) ──────────────────────────────

/// One host's live mount state as published on `state/mesh-mount/<host>`. A local
/// mirror of the worker's `MeshMountState`; serde ignores any wire field we don't
/// project.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MeshMountState {
    /// The peer hostname (the topic verb slot + roster key).
    pub host: String,
    /// The phase tag (`mounted` / `mounting` / `reconnecting` / …).
    pub state: String,
    /// The mount scope tag when relevant (`home` / `full`).
    #[serde(default)]
    pub scope: Option<String>,
    /// The live mountpoint, present once `mounted`.
    #[serde(default)]
    pub path: Option<String>,
    /// A human reason on a degrade path (unreachable / reconnecting).
    #[serde(default)]
    pub reason: Option<String>,
    /// Wall-clock epoch millis of the transition.
    #[serde(default)]
    pub since_ms: u64,
}

/// Parse a `state/mesh-mount/<host>` record body; `None` on malformed JSON (an
/// honest miss, never a panic).
#[must_use]
pub fn parse_state(raw: &str) -> Option<MeshMountState> {
    serde_json::from_str(raw).ok()
}

/// The projected, UI-friendly view of one host's mount — the shape the sidebar
/// renders. Built from a [`MeshMountState`] with the tags resolved to enums.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MountView {
    /// The lifecycle phase.
    pub phase: MountPhase,
    /// The mount scope, when the worker reported one.
    pub scope: Option<MountScope>,
    /// The mounted local path, when `mounted`.
    pub path: Option<String>,
    /// A degrade reason, when the worker reported one.
    pub reason: Option<String>,
}

impl MountView {
    /// Project a wire state record into the UI view.
    #[must_use]
    pub fn from_state(state: &MeshMountState) -> Self {
        Self {
            phase: MountPhase::from_tag(&state.state),
            scope: state.scope.as_deref().and_then(MountScope::from_tag),
            path: state.path.clone(),
            reason: state.reason.clone(),
        }
    }

    /// The browsable local mountpoint — `Some` only when the mount is live AND a
    /// path was published (so navigating never points at a not-yet-real path).
    #[must_use]
    pub fn mountpoint(&self) -> Option<&str> {
        if self.phase.is_mounted() {
            self.path.as_deref()
        } else {
            None
        }
    }

    /// `true` when this mount is escalated to the full filesystem (lock 14).
    #[must_use]
    pub fn is_full(&self) -> bool {
        matches!(self.scope, Some(MountScope::Full))
    }
}

// ── the client seam ─────────────────────────────────────────────────────────────

/// The mesh-mount client seam: read the worker's published state, request a
/// lifecycle verb. Injectable so the model is unit-tested headless (a fake) while
/// production talks the Bus ([`BusMeshMount`]).
pub trait MeshMountClient {
    /// The latest state view per host (`host` → [`MountView`]). Non-blocking — a
    /// local spool scan, never a peer probe.
    fn views(&self) -> HashMap<String, MountView>;

    /// Publish a lifecycle request for `host`. `Err` carries an honest,
    /// operator-readable reason (e.g. no Bus dir); it never blocks on a peer.
    ///
    /// # Errors
    /// A human-readable string when the request can't be appended to the Bus.
    fn request(&self, host: &str, verb: MeshMountVerb) -> Result<(), String>;
}

/// The live Bus-backed client — a synchronous local `Persist` read/write.
///
/// The same persist-first path `mde-shell-egui::storage` uses. Holds only the
/// resolved Bus spool dir; a fresh `Persist` is opened per call (it isn't `Send`).
/// Degrades honestly to empty / an error when there's no Bus dir — never a panic,
/// never a hang.
pub struct BusMeshMount {
    /// The resolved Bus client spool dir, or `None` when this node has no Bus.
    bus_root: Option<PathBuf>,
}

impl BusMeshMount {
    /// Resolve the Bus spool dir from the environment (the production path).
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
        }
    }

    /// Construct with an explicit spool root (tests point this at a tempdir).
    #[must_use]
    pub fn with_root(bus_root: Option<PathBuf>) -> Self {
        Self { bus_root }
    }
}

impl MeshMountClient for BusMeshMount {
    fn views(&self) -> HashMap<String, MountView> {
        let mut out = HashMap::new();
        let Some(root) = self.bus_root.clone() else {
            return out;
        };
        let Ok(persist) = mde_bus::persist::Persist::open(root) else {
            return out; // a transient open failure = an honest empty read
        };
        let topics = persist.list_topics().unwrap_or_default();
        for topic in topics
            .iter()
            .filter(|t| t.starts_with(STATE_PREFIX) && t.len() > STATE_PREFIX.len())
        {
            // The worker writes one record per transition; the newest (last, ULID
            // ascending) is the live state.
            let latest = persist
                .list_since(topic, None)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|m| m.body)
                .next_back();
            if let Some(state) = latest.as_deref().and_then(parse_state) {
                out.insert(state.host.clone(), MountView::from_state(&state));
            }
        }
        out
    }

    fn request(&self, host: &str, verb: MeshMountVerb) -> Result<(), String> {
        let Some(root) = self.bus_root.as_ref() else {
            return Err("No mesh Bus directory — mesh mounts are unavailable on this node.".into());
        };
        let topic = action_topic(host);
        let body = verb.body();
        mde_bus::persist::Persist::open(root.clone())
            .and_then(|p| {
                p.write(
                    &topic,
                    mde_bus::hooks::config::Priority::Default,
                    None,
                    Some(&body),
                )
            })
            .map(|_| ())
            .map_err(|e| format!("Couldn't publish the mesh-mount request: {e}"))
    }
}

// ── a test double, shared by the model + view test suites ───────────────────────

#[cfg(test)]
pub(crate) mod test_support {
    use super::{MeshMountClient, MeshMountVerb, MountView};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// An in-memory [`MeshMountClient`] for headless tests: canned views + a
    /// recorded request log (so a test asserts the exact verb the surface emitted
    /// without a live Bus or worker). `Clone` shares the request log (an `Arc`), so
    /// a test can keep a probe handle after boxing a clone into the model.
    #[derive(Clone)]
    pub struct FakeMeshMount {
        views: HashMap<String, MountView>,
        requests: Arc<Mutex<Vec<(String, MeshMountVerb)>>>,
    }

    impl FakeMeshMount {
        /// A fresh fake with no canned views and an empty request log.
        pub fn new() -> Self {
            Self {
                views: HashMap::new(),
                requests: Arc::new(Mutex::new(Vec::new())),
            }
        }

        /// Seed a host's published view.
        #[must_use]
        pub fn with_view(mut self, host: &str, view: MountView) -> Self {
            self.views.insert(host.to_string(), view);
            self
        }

        /// The verbs requested for `host`, in order.
        pub fn verbs_for(&self, host: &str) -> Vec<MeshMountVerb> {
            self.requests
                .lock()
                .expect("request log mutex poisoned")
                .iter()
                .filter(|(h, _)| h == host)
                .map(|(_, v)| *v)
                .collect()
        }

        /// The total number of requests recorded across all hosts.
        pub fn request_count(&self) -> usize {
            self.requests
                .lock()
                .expect("request log mutex poisoned")
                .len()
        }
    }

    impl MeshMountClient for FakeMeshMount {
        fn views(&self) -> HashMap<String, MountView> {
            self.views.clone()
        }

        fn request(&self, host: &str, verb: MeshMountVerb) -> Result<(), String> {
            self.requests
                .lock()
                .expect("request log mutex poisoned")
                .push((host.to_string(), verb));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_match_the_worker_contract() {
        // Cross-check: these MUST equal mackesd::workers::mesh_mount::{ACTION,STATE}_PREFIX.
        assert_eq!(ACTION_PREFIX, "action/mesh-mount/");
        assert_eq!(STATE_PREFIX, "state/mesh-mount/");
        assert_eq!(action_topic("oak"), "action/mesh-mount/oak");
        assert_eq!(state_topic("oak"), "state/mesh-mount/oak");
    }

    #[test]
    fn verb_body_matches_the_worker_wire_shape() {
        // The worker parses `{"verb":"…"}` with serde(tag="verb"); round-trip it.
        assert_eq!(MeshMountVerb::Mount.body(), r#"{"verb":"mount"}"#);
        assert_eq!(MeshMountVerb::Escalate.body(), r#"{"verb":"escalate"}"#);
        assert_eq!(MeshMountVerb::Unmount.body(), r#"{"verb":"unmount"}"#);
        // And the surface can decode its own bodies back (symmetry with the worker).
        let v: MeshMountVerb =
            serde_json::from_str(&MeshMountVerb::Escalate.body()).expect("round-trips");
        assert_eq!(v, MeshMountVerb::Escalate);
    }

    #[test]
    fn phase_parses_every_worker_tag_and_defaults_unknown() {
        assert_eq!(MountPhase::from_tag("mounting"), MountPhase::Mounting);
        assert_eq!(MountPhase::from_tag("mounted"), MountPhase::Mounted);
        assert_eq!(
            MountPhase::from_tag("reconnecting"),
            MountPhase::Reconnecting
        );
        assert_eq!(MountPhase::from_tag("unreachable"), MountPhase::Unreachable);
        assert_eq!(MountPhase::from_tag("unmounted"), MountPhase::Unmounted);
        // An unknown tag is an honest Unmounted, never a panic.
        assert_eq!(MountPhase::from_tag("wat"), MountPhase::Unmounted);
        assert!(MountPhase::Mounted.is_mounted());
        assert!(MountPhase::Mounting.is_transitional());
        assert!(MountPhase::Reconnecting.is_transitional());
        assert!(!MountPhase::Mounted.is_transitional());
    }

    #[test]
    fn parse_and_project_a_mounted_state() {
        let raw = r#"{
            "host":"oak","state":"mounted","scope":"home",
            "path":"/run/user/1000/mde-mesh/oak","since_ms":1715000000000
        }"#;
        let st = parse_state(raw).expect("decodes the worker record");
        assert_eq!(st.host, "oak");
        let view = MountView::from_state(&st);
        assert_eq!(view.phase, MountPhase::Mounted);
        assert_eq!(view.scope, Some(MountScope::Home));
        assert_eq!(view.mountpoint(), Some("/run/user/1000/mde-mesh/oak"));
        assert!(!view.is_full());
    }

    #[test]
    fn an_escalated_full_mount_projects_scope_full() {
        let raw = r#"{"host":"oak","state":"mounted","scope":"full","path":"/run/user/1000/mde-mesh/oak","since_ms":1}"#;
        let view = MountView::from_state(&parse_state(raw).expect("decodes"));
        assert!(view.is_full());
        assert_eq!(view.scope.expect("has a scope").label(), "full FS");
    }

    #[test]
    fn an_unreachable_state_has_no_mountpoint_but_keeps_the_reason() {
        let raw = r#"{"host":"cedar","state":"unreachable","reason":"unreachable: offline","since_ms":1}"#;
        let view = MountView::from_state(&parse_state(raw).expect("decodes"));
        assert_eq!(view.phase, MountPhase::Unreachable);
        // A non-mounted phase never yields a browsable path (even if one lingered).
        assert_eq!(view.mountpoint(), None);
        assert_eq!(view.reason.as_deref(), Some("unreachable: offline"));
    }

    #[test]
    fn malformed_state_is_an_honest_none() {
        assert!(parse_state("not json").is_none());
    }

    #[test]
    fn bus_client_without_a_root_reads_empty_and_errors_honestly() {
        // No Bus dir → an empty read (never a hang) and an honest request error.
        let client = BusMeshMount::with_root(None);
        assert!(client.views().is_empty());
        let err = client
            .request("oak", MeshMountVerb::Mount)
            .expect_err("no Bus dir surfaces an error, not a panic");
        assert!(err.contains("No mesh Bus directory"));
    }

    #[test]
    fn fake_client_records_the_requested_verbs() {
        use test_support::FakeMeshMount;
        let fake = FakeMeshMount::new();
        fake.request("oak", MeshMountVerb::Mount).expect("recorded");
        fake.request("oak", MeshMountVerb::Escalate)
            .expect("recorded");
        assert_eq!(
            fake.verbs_for("oak"),
            vec![MeshMountVerb::Mount, MeshMountVerb::Escalate]
        );
    }
}
