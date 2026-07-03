//! MEDIA-7: the resume / watch-history store — per-item playback position, plus
//! continue-watching, recents, and most-played.
//!
//! Like the MEDIA-6 [`Playlist`](crate::Playlist) this is a **pure model**: a store
//! of [`ResumeEntry`]s keyed by the media path/URL, recording where each item was
//! last left, how many times it has been played, and how recently it was touched —
//! all serde-persisted, all unit-tested with no engine. The
//! [`Player`](crate::Player) embeds one ([`Player::resume_state`]) and drives it: it
//! resumes from the stored position on load and updates it on seek / stop, so the
//! store is runtime-reachable, not a paper structure.
//!
//! # Recency without a clock
//!
//! Ordering (recents, most-played, continue-watching) is by a monotonic `touch`
//! sequence the store hands out on every update — **not** wall-clock time. That keeps
//! the whole model deterministic + testable (no injected clock), while still meaning
//! "most recently interacted with". A surface that wants a human "played 3 days ago"
//! label layers a wall-clock timestamp itself.
//!
//! # Resume policy
//!
//! [`ResumeEntry::resume_position`] returns a position only when it is worth
//! resuming: past a small [`RESUME_MIN_SECS`] floor (don't bother for the first few
//! seconds), not flagged `completed`, and not within the [`RESUME_TAIL_SECS`] /
//! [`RESUME_TAIL_FRACTION`] tail of a known duration (a title watched to the end
//! starts over next time, the continue-watching convention).

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Below this many seconds in, there is nothing worth resuming — start from the top.
pub const RESUME_MIN_SECS: f64 = 5.0;
/// Within this many seconds of a known end, the item counts as finished.
pub const RESUME_TAIL_SECS: f64 = 10.0;
/// At/above this fraction of a known duration, the item counts as finished.
pub const RESUME_TAIL_FRACTION: f64 = 0.95;

/// The remembered state of one media item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResumeEntry {
    /// The last playback position in seconds.
    pub position_secs: f64,
    /// The media duration in seconds when known (used by the resume tail policy).
    pub duration_secs: Option<f64>,
    /// How many times playback of this item has been started.
    pub play_count: u32,
    /// The monotonic touch sequence of the most recent update — the recency order.
    pub last_touch: u64,
    /// Whether the item was watched to (near) its end.
    pub completed: bool,
}

impl ResumeEntry {
    /// The position playback should resume from, or [`None`] when it is not worth
    /// resuming (below the [`RESUME_MIN_SECS`] floor, already `completed`, or within
    /// the end tail of a known duration — see the [module docs](self)).
    #[must_use]
    pub fn resume_position(&self) -> Option<f64> {
        if self.completed || self.position_secs <= RESUME_MIN_SECS {
            return None;
        }
        if is_near_end(self.position_secs, self.duration_secs) {
            return None;
        }
        Some(self.position_secs)
    }
}

/// Whether `position` is within the end tail of a known `duration`.
fn is_near_end(position: f64, duration: Option<f64>) -> bool {
    match duration {
        Some(d) if d > 0.0 => {
            position >= d - RESUME_TAIL_SECS || position / d >= RESUME_TAIL_FRACTION
        }
        _ => false,
    }
}

/// The resume / watch-history store (MEDIA-7) — [`ResumeEntry`]s keyed by media path.
///
/// Serde-persisted ([`save`](Self::save) / [`load`](Self::load)), mirroring
/// [`Playlist`](crate::Playlist). The [`Player`](crate::Player) embeds one and drives
/// it; a surface reads [`continue_watching`](Self::continue_watching) /
/// [`recents`](Self::recents) / [`most_played`](Self::most_played) to fill its rows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ResumeState {
    /// The per-item entries, keyed by media path (a [`BTreeMap`] so serialization is
    /// deterministic).
    entries: BTreeMap<String, ResumeEntry>,
    /// The next touch sequence to hand out (monotonic across the store's lifetime).
    next_touch: u64,
}

impl ResumeState {
    /// An empty store.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            next_touch: 0,
        }
    }

    // ── accessors ────────────────────────────────────────────────────────────

    /// The number of remembered items.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is remembered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The remembered entry for `key`, if any.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&ResumeEntry> {
        self.entries.get(key)
    }

    /// The position playback of `key` should resume from, or [`None`] — the
    /// [`ResumeEntry::resume_position`] of its entry (or `None` when unknown).
    #[must_use]
    pub fn resume_position(&self, key: &str) -> Option<f64> {
        self.entries.get(key).and_then(ResumeEntry::resume_position)
    }

    // ── updates ──────────────────────────────────────────────────────────────

    /// Hand out the next monotonic touch sequence.
    const fn touch(&mut self) -> u64 {
        self.next_touch += 1;
        self.next_touch
    }

    /// Mutably borrow (creating a fresh zeroed) the entry for `key`.
    fn entry_mut(&mut self, key: &str) -> &mut ResumeEntry {
        self.entries
            .entry(key.to_owned())
            .or_insert_with(|| ResumeEntry {
                position_secs: 0.0,
                duration_secs: None,
                play_count: 0,
                last_touch: 0,
                completed: false,
            })
    }

    /// Record that playback of `key` has **started**: bump its play count (feeding
    /// most-played) and mark it the most recently touched. Position is untouched, so a
    /// resume that was pending still applies.
    pub fn mark_started(&mut self, key: &str) {
        let touch = self.touch();
        let entry = self.entry_mut(key);
        entry.play_count = entry.play_count.saturating_add(1);
        entry.last_touch = touch;
    }

    /// Record the current playback `position` of `key` (with the media `duration`
    /// when known) and mark it most recently touched. Crossing into the end tail sets
    /// `completed`; stepping back out of it clears `completed` again.
    pub fn record_position(&mut self, key: &str, position: f64, duration: Option<f64>) {
        let touch = self.touch();
        let completed = is_near_end(position, duration);
        let entry = self.entry_mut(key);
        entry.position_secs = position.max(0.0);
        if duration.is_some() {
            entry.duration_secs = duration;
        }
        entry.completed = completed;
        entry.last_touch = touch;
    }

    /// Mark `key` watched to its end (position → `duration`, `completed`), so it will
    /// not offer a resume next time. Called when a title reaches its natural EOF.
    pub fn mark_completed(&mut self, key: &str, duration: Option<f64>) {
        let touch = self.touch();
        let entry = self.entry_mut(key);
        if let Some(d) = duration {
            entry.position_secs = d;
            entry.duration_secs = Some(d);
        }
        entry.completed = true;
        entry.last_touch = touch;
    }

    /// Forget `key` entirely, returning its entry if present.
    pub fn forget(&mut self, key: &str) -> Option<ResumeEntry> {
        self.entries.remove(key)
    }

    // ── history folds ────────────────────────────────────────────────────────

    /// The `limit` most recently touched items, most-recent first — the "recents"
    /// row. A pure fold over the store.
    #[must_use]
    pub fn recents(&self, limit: usize) -> Vec<&str> {
        let mut keys: Vec<(&String, &ResumeEntry)> = self.entries.iter().collect();
        keys.sort_by(|a, b| {
            b.1.last_touch
                .cmp(&a.1.last_touch)
                .then_with(|| a.0.cmp(b.0))
        });
        keys.into_iter()
            .take(limit)
            .map(|(k, _)| k.as_str())
            .collect()
    }

    /// The `limit` most-played items, most-played first (recency breaks ties) — the
    /// "most-played" row. A pure fold over the store.
    #[must_use]
    pub fn most_played(&self, limit: usize) -> Vec<&str> {
        let mut keys: Vec<(&String, &ResumeEntry)> = self.entries.iter().collect();
        keys.sort_by(|a, b| {
            b.1.play_count
                .cmp(&a.1.play_count)
                .then_with(|| b.1.last_touch.cmp(&a.1.last_touch))
                .then_with(|| a.0.cmp(b.0))
        });
        keys.into_iter()
            .take(limit)
            .map(|(k, _)| k.as_str())
            .collect()
    }

    /// The `limit` in-progress items (a real resume position, not yet completed),
    /// most-recently-touched first — the "continue watching" row.
    #[must_use]
    pub fn continue_watching(&self, limit: usize) -> Vec<&str> {
        let mut keys: Vec<(&String, &ResumeEntry)> = self
            .entries
            .iter()
            .filter(|(_, e)| e.resume_position().is_some())
            .collect();
        keys.sort_by(|a, b| {
            b.1.last_touch
                .cmp(&a.1.last_touch)
                .then_with(|| a.0.cmp(b.0))
        });
        keys.into_iter()
            .take(limit)
            .map(|(k, _)| k.as_str())
            .collect()
    }

    // ── persistence ──────────────────────────────────────────────────────────

    /// Save the store to `path` as pretty JSON.
    ///
    /// # Errors
    /// Returns the [`std::io::Error`] if the file cannot be written.
    pub fn save(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, json)
    }

    /// Load a store from a JSON `path` written by [`save`](Self::save).
    ///
    /// # Errors
    /// Returns the [`std::io::Error`] if the file cannot be read, or a mapped error
    /// if its contents are not a valid serialized [`ResumeState`].
    pub fn load(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let json = std::fs::read_to_string(path)?;
        serde_json::from_str(&json).map_err(std::io::Error::other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── resume policy ────────────────────────────────────────────────────────

    #[test]
    fn resume_position_respects_floor_tail_and_completed() {
        let mut rs = ResumeState::new();

        // Below the floor → no resume.
        rs.record_position("a", 2.0, Some(100.0));
        assert_eq!(rs.resume_position("a"), None);

        // Mid-title → resume there.
        rs.record_position("a", 40.0, Some(100.0));
        assert_eq!(rs.resume_position("a"), Some(40.0));

        // Within the end tail (>=95% or within 10s of the end) → finished, no resume.
        rs.record_position("a", 97.0, Some(100.0));
        assert_eq!(rs.resume_position("a"), None);
        assert!(rs.get("a").expect("a").completed);

        // Stepping back out of the tail clears completion + resumes again.
        rs.record_position("a", 30.0, Some(100.0));
        assert_eq!(rs.resume_position("a"), Some(30.0));
        assert!(!rs.get("a").expect("a").completed);

        // Unknown media → no resume.
        assert_eq!(rs.resume_position("missing"), None);
    }

    #[test]
    fn mark_completed_blocks_resume() {
        let mut rs = ResumeState::new();
        rs.record_position("m", 40.0, Some(100.0));
        assert_eq!(rs.resume_position("m"), Some(40.0));
        rs.mark_completed("m", Some(100.0));
        assert_eq!(rs.resume_position("m"), None);
        let e = rs.get("m").expect("m");
        assert!(e.completed);
        assert!((e.position_secs - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn record_position_without_duration_still_resumes() {
        let mut rs = ResumeState::new();
        // A live stream / unknown-duration item: no tail policy, just the floor.
        rs.record_position("live", 30.0, None);
        assert_eq!(rs.resume_position("live"), Some(30.0));
    }

    // ── counters + recency ───────────────────────────────────────────────────

    #[test]
    fn mark_started_counts_plays_and_keeps_position() {
        let mut rs = ResumeState::new();
        rs.record_position("x", 25.0, Some(100.0));
        rs.mark_started("x");
        rs.mark_started("x");
        let e = rs.get("x").expect("x");
        assert_eq!(e.play_count, 2);
        // Starting a play does not disturb the resume position.
        assert!((e.position_secs - 25.0).abs() < f64::EPSILON);
        assert_eq!(rs.resume_position("x"), Some(25.0));
    }

    #[test]
    fn recents_are_most_recently_touched_first() {
        let mut rs = ResumeState::new();
        rs.mark_started("a");
        rs.mark_started("b");
        rs.mark_started("c");
        // Touch "a" again → it becomes the most recent.
        rs.record_position("a", 10.0, Some(100.0));
        assert_eq!(rs.recents(10), vec!["a", "c", "b"]);
        // Limit is honoured.
        assert_eq!(rs.recents(2), vec!["a", "c"]);
    }

    #[test]
    fn most_played_orders_by_count_then_recency() {
        let mut rs = ResumeState::new();
        rs.mark_started("a"); // count 1
        rs.mark_started("b"); // count 1
        rs.mark_started("b"); // count 2
        rs.mark_started("c"); // count 1
                              // b(2) first; among the count-1 items the most recently touched (c) leads a.
        assert_eq!(rs.most_played(10), vec!["b", "c", "a"]);
    }

    #[test]
    fn continue_watching_lists_only_resumable_items() {
        let mut rs = ResumeState::new();
        rs.record_position("mid", 40.0, Some(100.0)); // resumable
        rs.record_position("done", 99.0, Some(100.0)); // finished → excluded
        rs.record_position("early", 1.0, Some(100.0)); // below floor → excluded
        rs.record_position("mid2", 55.0, Some(200.0)); // resumable, more recent
        assert_eq!(rs.continue_watching(10), vec!["mid2", "mid"]);
    }

    // ── persistence ──────────────────────────────────────────────────────────

    #[test]
    fn save_and_load_round_trip_through_a_file() {
        let mut rs = ResumeState::new();
        rs.record_position("a", 40.0, Some(100.0));
        rs.mark_started("a");
        rs.record_position("b", 12.0, Some(300.0));

        let mut path = std::env::temp_dir();
        path.push(format!(
            "mde-media-resume-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));

        rs.save(&path).expect("save");
        let back = ResumeState::load(&path).expect("load");
        let _ = std::fs::remove_file(&path);

        assert_eq!(rs, back);
        assert_eq!(back.resume_position("a"), Some(40.0));
        assert_eq!(back.recents(10), rs.recents(10));
    }
}
