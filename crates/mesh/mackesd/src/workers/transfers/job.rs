//! TRANSFERS-1 — the `TransferJob` envelope + its state machine.
//!
//! One typed record carries every byte-move the mesh performs, whatever the
//! protocol: an id, a `source`, a `dest`, the routing [`Method`] (Q4 — the lane
//! that will execute it), the [`TransferPolicy`] knobs (Q12 bandwidth cap / Q15
//! verify), and the live [`TransferState`]. The lanes (TRANSFERS-2..6) keep their
//! tool's native semantics but all ride THIS envelope; the queue + ledger only ever
//! see a `TransferJob`.
//!
//! The state machine is the spine's contract: the legal transitions live here
//! ([`TransferState::can`]), so the queue, the CLI, and the future GUI all agree on
//! what a `pause`/`resume`/`cancel` may do to a job in a given state — no surface
//! invents its own rules (§9 one-state doctrine).

#![cfg(feature = "async-services")]

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use super::now_ms;

/// The protocol lane a job routes to (Q4).
///
/// Each variant is a distinct executor the TRANSFERS-2..6 lanes implement; TRANSFERS-1
/// carries the tag only (the injectable [`super::lane::LaneRunner`] seam gates
/// execution honestly until then).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    /// `sftp`/`ssh` to a foreign host (TRANSFERS lane: sftp).
    Sftp,
    /// `rsync --partial --bwlimit` mirror (TRANSFERS lane: rsync + the Q19 sync pairs).
    Rsync,
    /// `wget -c --limit-rate` HTTP download (TRANSFERS lane: http/wget).
    Http,
    /// A browser-enqueued download or scrape output handed to the queue (Q8/Q17).
    BrowserDownload,
    /// A node→node move staged through the mesh-share so Syncthing replicates (Q6).
    Node,
    /// A drop into the shared Navidrome music library dir (Q9).
    Music,
}

impl Method {
    /// Every method, in a stable order (help text + exhaustiveness tests).
    pub const ALL: [Self; 6] = [
        Self::Sftp,
        Self::Rsync,
        Self::Http,
        Self::BrowserDownload,
        Self::Node,
        Self::Music,
    ];

    /// The canonical lowercase token (matches the serde wire form).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sftp => "sftp",
            Self::Rsync => "rsync",
            Self::Http => "http",
            Self::BrowserDownload => "browser_download",
            Self::Node => "node",
            Self::Music => "music",
        }
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Parse a method from a CLI token — case-insensitive, `-` and `_` interchangeable
/// (`browser-download` == `browser_download`).
impl FromStr for Method {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let norm = s.trim().to_ascii_lowercase().replace('-', "_");
        Self::ALL
            .into_iter()
            .find(|m| m.as_str() == norm)
            .ok_or_else(|| {
                let known = Self::ALL
                    .iter()
                    .map(|m| m.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("unknown transfer method `{s}` (expected one of: {known})")
            })
    }
}

/// The per-job policy knobs (Q12 throttle + Q15 integrity). Both default off so a
/// bare job is the plain, unthrottled, unverified move.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferPolicy {
    /// Optional per-job bandwidth cap, passed to the tool verbatim (Q12 —
    /// `rsync --bwlimit` / `wget --limit-rate`). `None` is unthrottled. The lane
    /// validates the token against its tool; the spine only carries it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bwlimit: Option<String>,
    /// Verify integrity on completion (Q15 — size + checksum, a mismatch is a
    /// FAILURE not a silent pass). Off by default.
    #[serde(default)]
    pub verify: bool,
}

/// The live state of a job (the design's five-state machine).
///
/// There is no `Cancelled` state: a cancel REMOVES the job from the ledger (see
/// [`super::queue::TransferQueue::cancel`]), so a row is always one of these five.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferState {
    /// Accepted, waiting for a free slot (below the parallel cap, Q12).
    Queued,
    /// A lane is actively executing it (occupies one of the cap's slots).
    Running,
    /// Held by an operator `pause`; not eligible to run until `resume`d.
    Paused,
    /// Completed successfully (terminal).
    Done,
    /// Ended in an honest failure — the reason is on [`TransferJob::error`] (§7).
    Failed,
}

impl TransferState {
    /// The lowercase wire token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }

    /// A terminal state never transitions again on its own (Done / Failed).
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed)
    }

    /// Is `verb` a legal request against a job in this state? The single source of
    /// truth every surface consults so `pause`/`resume`/`start`/`complete` agree.
    #[must_use]
    pub const fn can(self, verb: Transition) -> bool {
        matches!(
            (self, verb),
            // The scheduler starts a Queued job (cap-gated by the queue);
            (Self::Queued, Transition::Start)
                // pause holds a Queued or Running job;
                | (Self::Queued | Self::Running, Transition::Pause)
                // resume re-arms a Paused job;
                | (Self::Paused, Transition::Resume)
                // a lane completes only a Running job.
                | (Self::Running, Transition::Complete)
        )
    }
}

impl fmt::Display for TransferState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A requested state transition — the verb the state machine gates. (`cancel` is
/// not here: it removes the job entirely rather than transitioning it, so it is
/// always legal.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    /// Queued → Running (the scheduler claims a slot).
    Start,
    /// Queued|Running → Paused (operator hold).
    Pause,
    /// Paused → Queued (operator re-arm).
    Resume,
    /// Running → Done|Failed (a lane reported its outcome).
    Complete,
}

/// One typed transfer — the envelope every lane, the queue, and the ledger share.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferJob {
    /// Stable job id (client-minted at submit, like the DEVMGR-8 request id).
    pub id: String,
    /// Where the bytes come from (a path, a URL, a `host:path`, a peer — lane-parsed).
    pub source: String,
    /// Where the bytes land (a path, a peer, the Music Library, a `host:path`).
    pub dest: String,
    /// The lane that will execute it (Q4).
    pub method: Method,
    /// The Q12/Q15 policy knobs.
    #[serde(default)]
    pub policy: TransferPolicy,
    /// The live state (the five-state machine).
    pub state: TransferState,
    /// The honest failure reason when `state == Failed` (§7 — never fabricated,
    /// `None` in every other state).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Live progress percent (0..=100) when a lane can report one; `None` until a
    /// lane parses real progress — the spine never fabricates a percentage (the
    /// design's progress-parsing risk note).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<u8>,
    /// Wall-clock ms when the job was submitted (the FIFO order key).
    pub created_ms: u64,
    /// Wall-clock ms of the last state change.
    pub updated_ms: u64,
}

impl TransferJob {
    /// Mint a fresh Queued job (client side — the CLI + GUI both call this so the id
    /// scheme is single-sourced). The id is `<created_ms>-<rand>` (unique across
    /// processes within a millisecond, mirroring the DEVMGR-8 request id).
    #[must_use]
    pub fn new(
        source: impl Into<String>,
        dest: impl Into<String>,
        method: Method,
        policy: TransferPolicy,
    ) -> Self {
        let now = now_ms();
        Self {
            id: mint_id(now),
            source: source.into(),
            dest: dest.into(),
            method,
            policy,
            state: TransferState::Queued,
            error: None,
            progress: None,
            created_ms: now,
            updated_ms: now,
        }
    }

    /// Move to `state`, stamping `updated_ms`. Clears `error` unless moving to
    /// `Failed` (a re-queue drops a stale reason).
    pub(super) fn set_state(&mut self, state: TransferState) {
        self.state = state;
        self.updated_ms = now_ms();
        if state != TransferState::Failed {
            self.error = None;
        }
    }

    /// Move to `Failed` with an honest reason (§7).
    pub(super) fn fail(&mut self, error: impl Into<String>) {
        self.error = Some(error.into());
        self.set_state(TransferState::Failed);
    }
}

/// `<created_ms>-<seq>-<rand hex>` — the sort key that makes the queue deterministic
/// FIFO. The `created_ms` prefix orders by submit time; the process-monotonic `seq`
/// breaks a same-millisecond tie in submission order (so a burst of submits keeps
/// FIFO); the random suffix keeps ids unique across the CLI and daemon processes that
/// both mint them. Sorting the ledger by `(created_ms, id)` therefore yields a
/// stable, submission-ordered queue.
fn mint_id(now: u64) -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let suffix: u32 = rand::random();
    format!("{now:013}-{seq:012x}-{suffix:08x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_round_trips_through_serde_and_str() {
        for m in Method::ALL {
            let json = serde_json::to_string(&m).unwrap();
            let back: Method = serde_json::from_str(&json).unwrap();
            assert_eq!(m, back);
            // The CLI token parses back to the same variant (dash form too).
            assert_eq!(Method::from_str(m.as_str()).unwrap(), m);
            assert_eq!(
                Method::from_str(&m.as_str().replace('_', "-").to_uppercase()).unwrap(),
                m
            );
        }
        assert!(Method::from_str("torrent").is_err());
    }

    #[test]
    fn new_job_is_queued_with_a_minted_id() {
        let j = TransferJob::new("/a", "peer:b", Method::Rsync, TransferPolicy::default());
        assert_eq!(j.state, TransferState::Queued);
        assert!(j.error.is_none());
        assert!(j.progress.is_none());
        assert!(j.id.contains('-'), "id is <ms>-<rand>: {}", j.id);
        assert_eq!(j.created_ms, j.updated_ms);
    }

    #[test]
    fn state_machine_gates_the_legal_transitions() {
        use TransferState::{Done, Failed, Paused, Queued, Running};
        use Transition::{Complete, Pause, Resume, Start};
        // Start: only from Queued.
        assert!(Queued.can(Start));
        assert!(!Running.can(Start) && !Paused.can(Start) && !Done.can(Start));
        // Pause: Queued or Running.
        assert!(Queued.can(Pause) && Running.can(Pause));
        assert!(!Paused.can(Pause) && !Done.can(Pause) && !Failed.can(Pause));
        // Resume: only from Paused.
        assert!(Paused.can(Resume));
        assert!(!Queued.can(Resume) && !Running.can(Resume));
        // Complete: only a Running job.
        assert!(Running.can(Complete));
        assert!(!Queued.can(Complete) && !Paused.can(Complete));
        // Terminality.
        assert!(Done.is_terminal() && Failed.is_terminal());
        assert!(!Queued.is_terminal() && !Running.is_terminal() && !Paused.is_terminal());
    }

    #[test]
    fn fail_records_an_honest_reason_and_clears_on_requeue() {
        let mut j = TransferJob::new("/a", "/b", Method::Http, TransferPolicy::default());
        j.fail("the http lane is not yet wired");
        assert_eq!(j.state, TransferState::Failed);
        assert_eq!(j.error.as_deref(), Some("the http lane is not yet wired"));
        // Re-queueing drops the stale reason (§7 — no lingering fake failure).
        j.set_state(TransferState::Queued);
        assert!(j.error.is_none());
    }

    #[test]
    fn policy_serializes_lean() {
        // A default policy omits the optional fields on the wire.
        let json = serde_json::to_string(&TransferPolicy::default()).unwrap();
        assert!(
            !json.contains("bwlimit"),
            "unthrottled omits bwlimit: {json}"
        );
        assert!(json.contains("\"verify\":false"));
        let throttled = TransferPolicy {
            bwlimit: Some("2m".into()),
            verify: true,
        };
        let json = serde_json::to_string(&throttled).unwrap();
        assert!(json.contains("\"bwlimit\":\"2m\""));
    }
}
