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

    /// MUSIC-RFX-1 — move the track at `from` to index `to` (clamped), keeping the
    /// cursor on the same *playing* track. `false` if `from` is out of range.
    pub fn move_track(&mut self, from: usize, to: usize) -> bool {
        if from >= self.songs.len() {
            return false;
        }
        let to = to.min(self.songs.len() - 1);
        if from == to {
            return true;
        }
        let el = self.songs.remove(from);
        self.songs.insert(to, el);
        // Cursor: if the moved track WAS current, it follows to `to`; else adjust
        // for the removal-then-insertion around the cursor.
        if self.current == from {
            self.current = to;
        } else {
            let mut c = self.current;
            if from < c {
                c -= 1;
            }
            if to <= c {
                c += 1;
            }
            self.current = c;
        }
        true
    }

    /// MUSIC-RFX-1 — remove the track at `idx`, keeping the cursor sensible
    /// (shifts down if `idx` was before it; clamps if it removed the last/current).
    /// `false` if out of range.
    pub fn remove(&mut self, idx: usize) -> bool {
        if idx >= self.songs.len() {
            return false;
        }
        self.songs.remove(idx);
        if idx < self.current {
            self.current -= 1;
        }
        self.current = self.current.min(self.songs.len().saturating_sub(1));
        true
    }

    /// MUSIC-RFX-1 — remove a set of indices (multi-select). De-duped + removed
    /// high-to-low so earlier removals don't shift later ones. Returns the count
    /// actually removed.
    pub fn remove_many(&mut self, idxs: &[usize]) -> usize {
        let mut v: Vec<usize> = idxs
            .iter()
            .copied()
            .filter(|i| *i < self.songs.len())
            .collect();
        v.sort_unstable();
        v.dedup();
        let mut removed = 0;
        for idx in v.into_iter().rev() {
            if self.remove(idx) {
                removed += 1;
            }
        }
        removed
    }

    /// MUSIC-RFX-1 — move the track at `idx` to immediately after the current one
    /// ("Play next"). The cursor stays on the current track. `false` if `idx` is
    /// out of range or is already the current track.
    pub fn move_to_next(&mut self, idx: usize) -> bool {
        if idx >= self.songs.len() || idx == self.current {
            return false;
        }
        let el = self.songs.remove(idx);
        if idx < self.current {
            self.current -= 1;
        }
        let at = (self.current + 1).min(self.songs.len());
        self.songs.insert(at, el);
        true
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

    fn q(songs: &[&str], current: usize) -> Queue {
        Queue {
            songs: songs.iter().map(|s| (*s).to_string()).collect(),
            current,
        }
    }

    #[test]
    fn move_track_keeps_cursor_on_playing_song() {
        // current = c (idx 2). Move a (0) to end → cursor still on c.
        let mut x = q(&["a", "b", "c", "d"], 2);
        assert!(x.move_track(0, 3));
        assert_eq!(x.songs, ["b", "c", "d", "a"]);
        assert_eq!(x.current(), Some("c"));
        // Moving the CURRENT track follows it.
        let mut y = q(&["a", "b", "c"], 1);
        assert!(y.move_track(1, 2));
        assert_eq!(y.songs, ["a", "c", "b"]);
        assert_eq!(y.current(), Some("b"));
        assert!(!y.move_track(9, 0)); // out of range
    }

    #[test]
    fn remove_and_remove_many_adjust_cursor() {
        let mut x = q(&["a", "b", "c", "d"], 2); // on c
        assert!(x.remove(0)); // remove a (before cursor)
        assert_eq!(x.songs, ["b", "c", "d"]);
        assert_eq!(x.current(), Some("c")); // cursor shifted down, still c
                                            // remove the current → cursor lands on the next.
        let mut y = q(&["a", "b", "c"], 1); // on b
        assert!(y.remove(1));
        assert_eq!(y.current(), Some("c"));
        // multi-select removal (high+low) keeps the survivor cursor valid.
        let mut z = q(&["a", "b", "c", "d", "e"], 4); // on e
        assert_eq!(z.remove_many(&[0, 2, 0]), 2); // a + c (dedup)
        assert_eq!(z.songs, ["b", "d", "e"]);
        assert_eq!(z.current(), Some("e"));
    }

    #[test]
    fn move_to_next_inserts_after_current() {
        let mut x = q(&["a", "b", "c", "d"], 0); // on a
        assert!(x.move_to_next(3)); // play d next
        assert_eq!(x.songs, ["a", "d", "b", "c"]);
        assert_eq!(x.current(), Some("a")); // cursor unchanged
        assert!(!x.move_to_next(0)); // can't play-next the current
    }

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
