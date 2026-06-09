//! AIR-2 (v6.1) — the playback queue.
//!
//! The queue is an ordered list of Airsonic song-ids plus a cursor at
//! the currently-playing track. It persists to
//! `~/.local/share/mde/music-queue.json` so it survives a restart and so
//! the daemon + GUI share one source of truth (the daemon plays from it;
//! the GUI's "Add to Queue" / "Play Next" + the maxi-player queue tab
//! mutate it). The state transitions are pure functions, fully
//! unit-testable; `mde-musicd queue {add,add-next,list,clear,next,prev}`
//! is the reachable entry point, and the playback daemon (AIR-2.b) reads
//! the same file.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// An ordered playback queue with a current-track cursor.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Queue {
    /// Song-ids in play order.
    #[serde(default)]
    pub songs: Vec<String>,
    /// Index of the current track. Meaningless when `songs` is empty;
    /// clamped into range by the accessors.
    #[serde(default)]
    pub current: usize,
}

impl Queue {
    /// Append a track to the end.
    pub fn enqueue(&mut self, song_id: impl Into<String>) {
        self.songs.push(song_id.into());
    }

    /// Insert a track immediately after the current one ("Play Next").
    /// On an empty queue it just appends.
    pub fn enqueue_after_current(&mut self, song_id: impl Into<String>) {
        if self.songs.is_empty() {
            self.songs.push(song_id.into());
        } else {
            let at = (self.current + 1).min(self.songs.len());
            self.songs.insert(at, song_id.into());
        }
    }

    /// Empty the queue + reset the cursor.
    pub fn clear(&mut self) {
        self.songs.clear();
        self.current = 0;
    }

    /// The current song-id, if any (clamps a stale cursor).
    #[must_use]
    pub fn current(&self) -> Option<&str> {
        self.songs
            .get(self.current.min(self.songs.len().saturating_sub(1)))
            .map(String::as_str)
            .filter(|_| !self.songs.is_empty())
    }

    /// Advance to the next track, returning it. `None` at the end (the
    /// cursor stays on the last track).
    pub fn next(&mut self) -> Option<&str> {
        if self.current + 1 < self.songs.len() {
            self.current += 1;
            self.current()
        } else {
            None
        }
    }

    /// Step back to the previous track, returning it. `None` at the
    /// start (the cursor stays on the first track).
    pub fn prev(&mut self) -> Option<&str> {
        if self.current > 0 && !self.songs.is_empty() {
            self.current -= 1;
            self.current()
        } else {
            None
        }
    }

    /// Number of queued tracks.
    #[must_use]
    pub fn len(&self) -> usize {
        self.songs.len()
    }

    /// Whether the queue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.songs.is_empty()
    }
}

/// The queue file path: `$HOME/.local/share/mde/music-queue.json`.
#[must_use]
pub fn queue_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    Path::new(&home).join(".local/share/mde/music-queue.json")
}

/// Read the queue from `path` (empty queue when absent/malformed — the
/// queue is rebuildable, never a hard error).
#[must_use]
pub fn read_from(path: &Path) -> Queue {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Write the queue to `path`, creating the parent dir.
///
/// # Errors
/// IO / serialization failures.
pub fn write_to(path: &Path, queue: &Queue) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(queue)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn enqueue_and_current() {
        let mut q = Queue::default();
        assert!(q.current().is_none());
        q.enqueue("a");
        q.enqueue("b");
        assert_eq!(q.current(), Some("a"));
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn next_and_prev_walk_with_end_clamping() {
        let mut q = Queue::default();
        q.enqueue("a");
        q.enqueue("b");
        q.enqueue("c");
        assert_eq!(q.next(), Some("b"));
        assert_eq!(q.next(), Some("c"));
        assert_eq!(q.next(), None); // at end, cursor stays
        assert_eq!(q.current(), Some("c"));
        assert_eq!(q.prev(), Some("b"));
        assert_eq!(q.prev(), Some("a"));
        assert_eq!(q.prev(), None); // at start
        assert_eq!(q.current(), Some("a"));
    }

    #[test]
    fn enqueue_after_current_inserts_next() {
        let mut q = Queue::default();
        q.enqueue("a");
        q.enqueue("b");
        // current = a (0); play-next inserts between a and b.
        q.enqueue_after_current("x");
        assert_eq!(q.songs, vec!["a", "x", "b"]);
        assert_eq!(q.next(), Some("x"));
        // On empty, it just appends.
        let mut e = Queue::default();
        e.enqueue_after_current("z");
        assert_eq!(e.songs, vec!["z"]);
    }

    #[test]
    fn clear_resets() {
        let mut q = Queue::default();
        q.enqueue("a");
        q.next();
        q.clear();
        assert!(q.is_empty());
        assert_eq!(q.current, 0);
        assert!(q.current().is_none());
    }

    #[test]
    fn current_clamps_stale_cursor() {
        let q = Queue {
            songs: vec!["a".into()],
            current: 9,
        };
        assert_eq!(q.current(), Some("a"));
    }

    #[test]
    fn persist_round_trips() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("sub").join("music-queue.json");
        let mut q = Queue::default();
        q.enqueue("a");
        q.enqueue("b");
        q.next();
        write_to(&p, &q).unwrap();
        assert_eq!(read_from(&p), q);
    }

    #[test]
    fn read_absent_is_empty() {
        let dir = tempdir().unwrap();
        assert_eq!(read_from(&dir.path().join("none.json")), Queue::default());
    }
}
