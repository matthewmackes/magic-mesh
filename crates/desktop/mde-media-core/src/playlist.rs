//! MEDIA-6: the typed playlist / queue model — the load-bearing, unit-tested core.
//!
//! Where MEDIA-3/4/5 fold config to mpv, the playlist is a **pure model**: an
//! ordered list of [`PlaylistItem`]s with a current cursor, a [`RepeatMode`], and a
//! deterministic seedable **shuffle**. It owns *what plays next* — enqueue /
//! dequeue / reorder, `next`/`prev` transitions, repeat wrap, and shuffle order —
//! all testable with no engine at all. The [`Player`](crate::Player) embeds one and
//! drives the engine from it (auto-advancing the queue on end-of-file), so the
//! model is runtime-reachable, not a paper structure.
//!
//! Shuffle is a seeded Fisher–Yates permutation ([`Playlist::shuffle`]), so the
//! same seed always yields the same play order — the "deterministic so it's
//! testable" acceptance. The current item is tracked by its index into the authored
//! [`items`](Playlist::items), so it survives a shuffle toggle, an enqueue, a
//! reorder, and a save/load round-trip unchanged.
//!
//! Save/load is [`serde`] (JSON) — [`Playlist::save`] / [`Playlist::load`] persist
//! the queue to a path, and the type round-trips through any serde format.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// A `SplitMix64` step — a tiny, dependency-free PRNG for the seeded shuffle.
///
/// Deterministic given the seed, so the shuffle order is reproducible + testable.
/// This is *not* cryptographic (playlist ordering has no security bearing).
const fn next_u64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A seeded Fisher–Yates permutation of `0..n` — the shuffle play order.
///
/// Deterministic for a given `seed` (same seed → same permutation), and always a
/// full permutation (every index present exactly once).
fn shuffled_order(n: usize, seed: u64) -> Vec<usize> {
    let mut order: Vec<usize> = (0..n).collect();
    let mut state = seed;
    let mut i = n;
    while i > 1 {
        i -= 1;
        let bound = (i as u64) + 1;
        let j = usize::try_from(next_u64(&mut state) % bound).unwrap_or(0);
        order.swap(i, j);
    }
    order
}

/// Whether, and how, the queue repeats at its ends — mpv's `loop-playlist` model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RepeatMode {
    /// Play to the end and stop — `next` past the last item yields nothing.
    #[default]
    Off,
    /// Repeat the current item forever — `next`/`prev` stay on it.
    One,
    /// Loop the whole queue — `next` past the last item wraps to the first.
    All,
}

/// One entry in a [`Playlist`] — a media URL/path with an optional display title.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaylistItem {
    /// The media URL or path handed to [`Player::load`](crate::Player::load).
    pub url: String,
    /// A human title for the surface, if any (else the surface derives one from
    /// the URL).
    pub title: Option<String>,
}

impl PlaylistItem {
    /// An item at `url` with no explicit title.
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            title: None,
        }
    }

    /// An item at `url` with a display `title`.
    #[must_use]
    pub fn titled(url: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            title: Some(title.into()),
        }
    }
}

/// The typed playlist / queue for the [`Player`](crate::Player).
///
/// Ordered [`items`](Self::items) plus a current cursor (an index into `items`), a
/// [`RepeatMode`], and an optional shuffle seed. All transitions are pure + tested;
/// the [`Player`](crate::Player) drives the engine from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Playlist {
    /// The authored item order (enqueue appends, reorder moves within this).
    items: Vec<PlaylistItem>,
    /// The current item's index into `items`, or [`None`] when nothing is current.
    current: Option<usize>,
    /// How the queue repeats at its ends.
    repeat: RepeatMode,
    /// The shuffle seed when shuffled, or [`None`] for authored order. Kept (not the
    /// derived permutation) so save/load stays minimal + the order recomputes
    /// deterministically.
    shuffle: Option<u64>,
}

impl Playlist {
    /// An empty queue: no items, nothing current, repeat off, no shuffle.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            items: Vec::new(),
            current: None,
            repeat: RepeatMode::Off,
            shuffle: None,
        }
    }

    /// A queue of `items` with the first item current (or nothing current when
    /// empty), repeat off, no shuffle.
    #[must_use]
    pub fn from_items(items: Vec<PlaylistItem>) -> Self {
        let current = (!items.is_empty()).then_some(0);
        Self {
            items,
            current,
            repeat: RepeatMode::Off,
            shuffle: None,
        }
    }

    // ── accessors ────────────────────────────────────────────────────────────

    /// The authored items, in enqueue order.
    #[must_use]
    pub fn items(&self) -> &[PlaylistItem] {
        &self.items
    }

    /// The number of items in the queue.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the queue has no items.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The current item, if any.
    #[must_use]
    pub fn current(&self) -> Option<&PlaylistItem> {
        self.current.and_then(|i| self.items.get(i))
    }

    /// The current item's index into [`items`](Self::items), if any.
    #[must_use]
    pub const fn current_index(&self) -> Option<usize> {
        self.current
    }

    /// The active repeat mode.
    #[must_use]
    pub const fn repeat(&self) -> RepeatMode {
        self.repeat
    }

    /// Whether the queue is currently shuffled.
    #[must_use]
    pub const fn is_shuffled(&self) -> bool {
        self.shuffle.is_some()
    }

    /// The active shuffle seed, if shuffled.
    #[must_use]
    pub const fn shuffle_seed(&self) -> Option<u64> {
        self.shuffle
    }

    // ── mutation ─────────────────────────────────────────────────────────────

    /// Append `item` to the end of the queue (enqueue). If the queue was empty,
    /// the new item becomes current.
    pub fn push(&mut self, item: PlaylistItem) {
        self.items.push(item);
        if self.current.is_none() && self.items.len() == 1 {
            self.current = Some(0);
        }
    }

    /// Remove and return the item at `index` (dequeue), keeping the cursor pointing
    /// at the same logical position. Returns [`None`] if `index` is out of range.
    ///
    /// If the removed item was current, the cursor holds its slot (now the item that
    /// shifted up, clamped to the last item) so playback continues in place; the
    /// queue going empty clears the cursor.
    pub fn remove(&mut self, index: usize) -> Option<PlaylistItem> {
        if index >= self.items.len() {
            return None;
        }
        let removed = self.items.remove(index);
        self.current = match self.current {
            _ if self.items.is_empty() => None,
            Some(cur) if index < cur => Some(cur - 1),
            Some(cur) if index == cur => Some(cur.min(self.items.len() - 1)),
            other => other,
        };
        Some(removed)
    }

    /// Move the item at `from` to index `to`, shifting the rest (reorder). The
    /// current item follows its content. Returns `false` (a no-op) if either index
    /// is out of range.
    pub fn reorder(&mut self, from: usize, to: usize) -> bool {
        let len = self.items.len();
        if from >= len || to >= len {
            return false;
        }
        if from == to {
            return true;
        }
        let item = self.items.remove(from);
        self.items.insert(to, item);
        if let Some(cur) = self.current {
            self.current = Some(remap_index(cur, from, to));
        }
        true
    }

    /// Set the current item to `index`. Returns `false` if out of range.
    pub fn select(&mut self, index: usize) -> bool {
        if index < self.items.len() {
            self.current = Some(index);
            true
        } else {
            false
        }
    }

    /// Clear the queue entirely (items + cursor); repeat/shuffle are preserved.
    pub fn clear(&mut self) {
        self.items.clear();
        self.current = None;
    }

    /// Set the [`RepeatMode`].
    pub const fn set_repeat(&mut self, mode: RepeatMode) {
        self.repeat = mode;
    }

    /// Shuffle the play order with `seed` (deterministic — the same seed always
    /// yields the same order). The current item is unchanged (only the traversal
    /// order changes), so playback continues from where it is.
    pub const fn shuffle(&mut self, seed: u64) {
        self.shuffle = Some(seed);
    }

    /// Restore the authored (un-shuffled) play order. The current item is unchanged.
    pub const fn unshuffle(&mut self) {
        self.shuffle = None;
    }

    // ── transitions ──────────────────────────────────────────────────────────

    /// Advance to and return the next item per the [`RepeatMode`] + shuffle order,
    /// updating the cursor. Returns [`None`] (leaving the cursor put) when there is
    /// no next item — an empty queue, or the end of a [`RepeatMode::Off`] queue.
    pub fn next_item(&mut self) -> Option<&PlaylistItem> {
        let idx = self.step(true)?;
        self.current = Some(idx);
        self.items.get(idx)
    }

    /// Step back to and return the previous item per the [`RepeatMode`] + shuffle
    /// order, updating the cursor. Returns [`None`] (leaving the cursor put) when
    /// there is no previous item.
    pub fn prev_item(&mut self) -> Option<&PlaylistItem> {
        let idx = self.step(false)?;
        self.current = Some(idx);
        self.items.get(idx)
    }

    /// Compute the item index a `forward`/backward step lands on, without mutating.
    ///
    /// Returns the item index (into [`items`](Self::items)) or [`None`] at a hard
    /// end (a [`RepeatMode::Off`] boundary, or an empty queue).
    fn step(&self, forward: bool) -> Option<usize> {
        if self.items.is_empty() {
            return None;
        }
        let order = self.order();
        let len = order.len();
        // Repeat-one stays on the current item (or starts the queue if none).
        if self.repeat == RepeatMode::One {
            return Some(self.current.unwrap_or(order[0]));
        }
        let wrap = self.repeat == RepeatMode::All;
        let cur_pos = self
            .current
            .and_then(|c| order.iter().position(|&i| i == c));
        let next_pos = match cur_pos {
            Some(p) if forward => {
                if p + 1 < len {
                    Some(p + 1)
                } else if wrap {
                    Some(0)
                } else {
                    None
                }
            }
            Some(p) => {
                if p > 0 {
                    Some(p - 1)
                } else if wrap {
                    Some(len - 1)
                } else {
                    None
                }
            }
            // Nothing current yet — a step starts at the appropriate end.
            None => Some(if forward { 0 } else { len - 1 }),
        };
        next_pos.map(|p| order[p])
    }

    /// The play order: identity when un-shuffled, else the seeded permutation.
    fn order(&self) -> Vec<usize> {
        self.shuffle.map_or_else(
            || (0..self.items.len()).collect(),
            |seed| shuffled_order(self.items.len(), seed),
        )
    }

    // ── persistence ──────────────────────────────────────────────────────────

    /// Save the queue to `path` as pretty JSON (save playlists).
    ///
    /// # Errors
    /// Returns the [`std::io::Error`] if the file cannot be written (serialization
    /// of a [`Playlist`] cannot itself fail, but is mapped into an I/O error for a
    /// single return type).
    pub fn save(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, json)
    }

    /// Load a queue from a JSON `path` written by [`save`](Self::save) (load
    /// playlists).
    ///
    /// # Errors
    /// Returns the [`std::io::Error`] if the file cannot be read, or a mapped error
    /// if its contents are not a valid serialized [`Playlist`].
    pub fn load(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let json = std::fs::read_to_string(path)?;
        serde_json::from_str(&json).map_err(std::io::Error::other)
    }
}

/// Remap an index when the item at `from` is moved to `to` (used to keep the
/// cursor on its content across a [`Playlist::reorder`]).
const fn remap_index(index: usize, from: usize, to: usize) -> usize {
    if index == from {
        to
    } else if from < to {
        // Items in `(from, to]` shift one toward the front.
        if index > from && index <= to {
            index - 1
        } else {
            index
        }
    } else {
        // from > to: items in `[to, from)` shift one toward the back.
        if index >= to && index < from {
            index + 1
        } else {
            index
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(urls: &[&str]) -> Vec<PlaylistItem> {
        urls.iter().map(|u| PlaylistItem::new(*u)).collect()
    }

    fn urls(pl: &Playlist) -> Vec<&str> {
        pl.items().iter().map(|i| i.url.as_str()).collect()
    }

    // ── construction + accessors ─────────────────────────────────────────────

    #[test]
    fn empty_playlist_has_no_current() {
        let mut pl = Playlist::new();
        assert!(pl.is_empty());
        assert_eq!(pl.len(), 0);
        assert_eq!(pl.current(), None);
        assert_eq!(pl.current_index(), None);
        assert_eq!(pl.next_item(), None);
        assert_eq!(pl.prev_item(), None);
        assert_eq!(pl.repeat(), RepeatMode::Off);
        assert!(!pl.is_shuffled());
    }

    #[test]
    fn from_items_selects_first() {
        let pl = Playlist::from_items(items(&["a", "b", "c"]));
        assert_eq!(pl.len(), 3);
        assert_eq!(pl.current_index(), Some(0));
        assert_eq!(pl.current().map(|i| i.url.as_str()), Some("a"));
    }

    // ── enqueue / dequeue / reorder ──────────────────────────────────────────

    #[test]
    fn push_appends_and_first_push_selects() {
        let mut pl = Playlist::new();
        pl.push(PlaylistItem::new("a"));
        assert_eq!(pl.current_index(), Some(0));
        pl.push(PlaylistItem::titled("b", "Song B"));
        // Enqueue appends; current stays on the first.
        assert_eq!(urls(&pl), vec!["a", "b"]);
        assert_eq!(pl.current_index(), Some(0));
        assert_eq!(pl.items()[1].title.as_deref(), Some("Song B"));
    }

    #[test]
    fn remove_before_current_shifts_cursor_down() {
        let mut pl = Playlist::from_items(items(&["a", "b", "c"]));
        pl.select(2); // current = "c"
        let removed = pl.remove(0).expect("remove");
        assert_eq!(removed.url, "a");
        assert_eq!(urls(&pl), vec!["b", "c"]);
        // "c" is still current, now at index 1.
        assert_eq!(pl.current().map(|i| i.url.as_str()), Some("c"));
        assert_eq!(pl.current_index(), Some(1));
    }

    #[test]
    fn remove_current_holds_the_slot() {
        let mut pl = Playlist::from_items(items(&["a", "b", "c"]));
        pl.select(1); // current = "b"
        pl.remove(1).expect("remove current");
        // The slot now holds "c" (the item that shifted up).
        assert_eq!(urls(&pl), vec!["a", "c"]);
        assert_eq!(pl.current().map(|i| i.url.as_str()), Some("c"));
    }

    #[test]
    fn remove_last_current_clamps_to_new_last() {
        let mut pl = Playlist::from_items(items(&["a", "b", "c"]));
        pl.select(2); // current = last "c"
        pl.remove(2).expect("remove last");
        assert_eq!(urls(&pl), vec!["a", "b"]);
        // Clamped back onto the new last item.
        assert_eq!(pl.current_index(), Some(1));
    }

    #[test]
    fn remove_out_of_range_is_none() {
        let mut pl = Playlist::from_items(items(&["a"]));
        assert_eq!(pl.remove(9), None);
        assert_eq!(pl.len(), 1);
    }

    #[test]
    fn remove_to_empty_clears_cursor() {
        let mut pl = Playlist::from_items(items(&["a"]));
        pl.remove(0).expect("remove only");
        assert!(pl.is_empty());
        assert_eq!(pl.current_index(), None);
    }

    #[test]
    fn reorder_moves_item_and_cursor_follows() {
        let mut pl = Playlist::from_items(items(&["a", "b", "c", "d"]));
        pl.select(1); // current = "b"
        assert!(pl.reorder(1, 3)); // move "b" to the end
        assert_eq!(urls(&pl), vec!["a", "c", "d", "b"]);
        // The cursor followed "b" to index 3.
        assert_eq!(pl.current().map(|i| i.url.as_str()), Some("b"));
        assert_eq!(pl.current_index(), Some(3));
    }

    #[test]
    fn reorder_backwards_remaps_untouched_cursor() {
        let mut pl = Playlist::from_items(items(&["a", "b", "c", "d"]));
        pl.select(0); // current = "a"
        assert!(pl.reorder(3, 1)); // move "d" between "a" and "b"
        assert_eq!(urls(&pl), vec!["a", "d", "b", "c"]);
        // "a" stayed put at index 0.
        assert_eq!(pl.current().map(|i| i.url.as_str()), Some("a"));
    }

    #[test]
    fn reorder_out_of_range_is_noop() {
        let mut pl = Playlist::from_items(items(&["a", "b"]));
        assert!(!pl.reorder(0, 9));
        assert_eq!(urls(&pl), vec!["a", "b"]);
    }

    // ── next / prev + repeat ─────────────────────────────────────────────────

    #[test]
    fn next_advances_then_stops_at_end_when_repeat_off() {
        let mut pl = Playlist::from_items(items(&["a", "b", "c"]));
        assert_eq!(pl.next_item().map(|i| i.url.as_str()), Some("b"));
        assert_eq!(pl.next_item().map(|i| i.url.as_str()), Some("c"));
        // Past the last item → None, cursor stays on the last.
        assert_eq!(pl.next_item(), None);
        assert_eq!(pl.current().map(|i| i.url.as_str()), Some("c"));
    }

    #[test]
    fn prev_steps_back_then_stops_at_start_when_repeat_off() {
        let mut pl = Playlist::from_items(items(&["a", "b", "c"]));
        pl.select(2); // "c"
        assert_eq!(pl.prev_item().map(|i| i.url.as_str()), Some("b"));
        assert_eq!(pl.prev_item().map(|i| i.url.as_str()), Some("a"));
        assert_eq!(pl.prev_item(), None);
        assert_eq!(pl.current().map(|i| i.url.as_str()), Some("a"));
    }

    #[test]
    fn repeat_all_wraps_forward_and_backward() {
        let mut pl = Playlist::from_items(items(&["a", "b", "c"]));
        pl.set_repeat(RepeatMode::All);
        pl.select(2); // "c"
                      // Forward past the end wraps to the first.
        assert_eq!(pl.next_item().map(|i| i.url.as_str()), Some("a"));
        // Backward past the start wraps to the last.
        assert_eq!(pl.prev_item().map(|i| i.url.as_str()), Some("c"));
    }

    #[test]
    fn repeat_one_stays_on_current() {
        let mut pl = Playlist::from_items(items(&["a", "b", "c"]));
        pl.set_repeat(RepeatMode::One);
        pl.select(1); // "b"
        assert_eq!(pl.next_item().map(|i| i.url.as_str()), Some("b"));
        assert_eq!(pl.next_item().map(|i| i.url.as_str()), Some("b"));
        assert_eq!(pl.prev_item().map(|i| i.url.as_str()), Some("b"));
    }

    // ── shuffle ──────────────────────────────────────────────────────────────

    /// Walk the whole queue from a clean start via `next`, collecting the visited
    /// item urls in play order.
    fn play_order(pl: &mut Playlist) -> Vec<String> {
        // Start before the first item so the first `next` yields the head.
        pl.current = None;
        let mut seen = Vec::new();
        while let Some(item) = pl.next_item() {
            let url = item.url.clone();
            if seen.contains(&url) {
                break; // guard against a wrap (repeat all)
            }
            seen.push(url);
        }
        seen
    }

    #[test]
    fn shuffle_is_deterministic_for_a_seed() {
        let mut a = Playlist::from_items(items(&["1", "2", "3", "4", "5", "6"]));
        let mut b = Playlist::from_items(items(&["1", "2", "3", "4", "5", "6"]));
        a.shuffle(0xABCD);
        b.shuffle(0xABCD);
        assert!(a.is_shuffled());
        // Same seed → identical play order.
        assert_eq!(play_order(&mut a), play_order(&mut b));
    }

    #[test]
    fn shuffle_visits_every_item_once() {
        let mut pl = Playlist::from_items(items(&["1", "2", "3", "4", "5", "6", "7"]));
        pl.shuffle(42);
        let mut order = play_order(&mut pl);
        assert_eq!(order.len(), 7, "every item is visited exactly once");
        order.sort();
        assert_eq!(order, vec!["1", "2", "3", "4", "5", "6", "7"]);
    }

    #[test]
    fn different_seeds_generally_differ_and_unshuffle_restores_authored() {
        let mut pl = Playlist::from_items(items(&["1", "2", "3", "4", "5", "6", "7", "8"]));
        pl.shuffle(1);
        let s1 = play_order(&mut pl);
        pl.shuffle(2);
        let s2 = play_order(&mut pl);
        assert_ne!(s1, s2, "distinct seeds give distinct orders for this list");
        pl.unshuffle();
        assert!(!pl.is_shuffled());
        assert_eq!(
            play_order(&mut pl),
            vec!["1", "2", "3", "4", "5", "6", "7", "8"]
        );
    }

    #[test]
    fn shuffle_toggle_keeps_current_item() {
        let mut pl = Playlist::from_items(items(&["a", "b", "c", "d"]));
        pl.select(2); // current = "c"
        pl.shuffle(7);
        // The traversal order changed but "c" is still current.
        assert_eq!(pl.current().map(|i| i.url.as_str()), Some("c"));
        pl.unshuffle();
        assert_eq!(pl.current().map(|i| i.url.as_str()), Some("c"));
        assert_eq!(pl.current_index(), Some(2));
    }

    // ── persistence ──────────────────────────────────────────────────────────

    #[test]
    fn round_trips_through_serde_json_string() {
        let mut pl = Playlist::from_items(vec![
            PlaylistItem::titled("a.mkv", "Alpha"),
            PlaylistItem::new("b.mkv"),
        ]);
        pl.set_repeat(RepeatMode::All);
        pl.shuffle(99);
        pl.select(1);

        let json = serde_json::to_string(&pl).expect("serialize");
        let back: Playlist = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(pl, back);
        assert_eq!(back.repeat(), RepeatMode::All);
        assert_eq!(back.shuffle_seed(), Some(99));
        assert_eq!(back.current_index(), Some(1));
    }

    #[test]
    fn save_and_load_round_trip_through_a_file() {
        let mut pl = Playlist::from_items(items(&["one", "two", "three"]));
        pl.set_repeat(RepeatMode::One);
        pl.shuffle(5);
        pl.select(2);

        // A uniquely named temp file so parallel tests never collide.
        let mut path = std::env::temp_dir();
        path.push(format!(
            "mde-media-playlist-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));

        pl.save(&path).expect("save");
        let back = Playlist::load(&path).expect("load");
        let _ = std::fs::remove_file(&path);

        assert_eq!(pl, back);
        // The deterministic shuffle order survives the round-trip.
        let (mut a, mut b) = (pl, back);
        assert_eq!(play_order(&mut a), play_order(&mut b));
    }
}
