//! **Code folding** (EDITOR-12): tree-sitter-derived fold regions + the
//! per-buffer fold state the widget renders through.
//!
//! Folding reuses the *same* syntax [`Tree`](tree_sitter::Tree) the EDITOR-5
//! [`Highlighter`](crate::highlight::Highlighter) already parses — there is no
//! second parser. [`regions_from_tree`] walks that tree once and yields the
//! foldable [`FoldRegion`]s (functions, blocks, impls, structs, …): every named,
//! multi-line structural node, deduplicated to the outermost region per header
//! line so each line carries at most one fold chevron.
//!
//! [`Folds`] is the per-document fold *state*: which region headers are collapsed
//! and the resulting hidden-line intervals. The widget consumes those intervals
//! to skip hidden lines when it maps display rows to logical lines, so a folded
//! region genuinely disappears from the render (and reappears on unfold) rather
//! than being greyed out (§7 — a real behavior, not a decoration).
//!
//! The state is pure (no egui), so region derivation and the fold/unfold →
//! hidden-line math are unit-testable without a frame.

use std::collections::BTreeSet;
use std::ops::Range;

use tree_sitter::{Node, Tree};

/// A foldable region derived from the syntax tree.
///
/// The `header_line` carries the gutter chevron and stays visible when folded;
/// folding hides the lines `header_line + 1 ..= end_line` (the body plus the
/// closing delimiter), so the next visible line after a collapsed region is
/// `end_line + 1`. Lines are 0-based, matching tree-sitter's row coordinate and
/// the widget's logical-line index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FoldRegion {
    /// The line the fold opens on (stays visible; carries the chevron).
    pub header_line: usize,
    /// The last line the region covers (hidden when folded).
    pub end_line: usize,
}

impl FoldRegion {
    /// The half-open line range this region hides when it is folded — the body
    /// and closing line, never the header (`header_line + 1 .. end_line + 1`).
    #[must_use]
    pub const fn hidden(self) -> Range<usize> {
        self.header_line + 1..self.end_line + 1
    }
}

/// Whether a tree-sitter node kind names a multi-line **structural** construct
/// worth a fold chevron across the vendored grammars (rust / python / js / ts /
/// json / toml / markdown / bash).
///
/// A substring match keeps this grammar-agnostic: Rust's `function_item` /
/// `impl_item` / `struct_item` land via `_item` and their own words, its `block`
/// / `declaration_list` bodies via `block` / `declaration`, Python/JS blocks via
/// `block` / `body`, JSON/JS containers via `object` / `array`, TOML via `table`.
/// Over-inclusion is harmless: [`regions_from_tree`] keeps only the outermost
/// region per header line.
fn is_foldable_kind(kind: &str) -> bool {
    const NEEDLES: [&str; 18] = [
        "block",
        "body",
        "declaration",
        "_item",
        "impl",
        "struct",
        "enum",
        "trait",
        "function",
        "class",
        "array",
        "object",
        "table",
        "dictionary",
        "arguments",
        "parameters",
        "match",
        "closure",
    ];
    NEEDLES.iter().any(|n| kind.contains(n))
}

/// Walk `node` (pre-order) accumulating the largest foldable span per header
/// line into `best` (`header_line → max end_line`). The tree root is skipped
/// (folding the whole file has no chevron home) via the `is_root` flag.
fn collect(node: Node, is_root: bool, best: &mut Vec<(usize, usize)>) {
    if !is_root && node.is_named() {
        let start = node.start_position().row;
        let end = node.end_position().row;
        if end > start && is_foldable_kind(node.kind()) {
            best.push((start, end));
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect(child, false, best);
    }
}

/// The foldable regions of `tree`, one per header line (the outermost multi-line
/// structural node starting there), sorted ascending by header line.
///
/// Pure over the already-parsed tree — no re-parse, no rope needed (line numbers
/// come straight off the node positions). The dedup-to-outermost keeps the gutter
/// to at most one chevron per line while still exposing nested regions on their
/// own header lines (fold the `impl`, or fold a method inside it).
#[must_use]
pub fn regions_from_tree(tree: &Tree) -> Vec<FoldRegion> {
    let mut spans: Vec<(usize, usize)> = Vec::new();
    collect(tree.root_node(), true, &mut spans);
    // Keep the largest end per header line (the outermost region there).
    spans.sort_unstable();
    let mut regions: Vec<FoldRegion> = Vec::new();
    for (header_line, end_line) in spans {
        if let Some(last) = regions.last_mut() {
            if last.header_line == header_line {
                last.end_line = last.end_line.max(end_line);
                continue;
            }
        }
        regions.push(FoldRegion {
            header_line,
            end_line,
        });
    }
    regions
}

/// The per-document fold state (EDITOR-12).
///
/// Holds the derived [`FoldRegion`]s (cached, keyed by the buffer revision they
/// were parsed at), which region headers are currently collapsed, and the merged
/// hidden-line intervals that fall out of that. The widget reads
/// [`hidden`](Self::hidden) / the display-row mapping to skip folded lines.
///
/// Fold state is **per-buffer**: each open document owns one `Folds` (a split's
/// duplicated buffer gets its own). Folds are keyed by absolute header line; an
/// edit that shifts a header's line number re-derives the regions and drops a
/// fold whose header no longer starts a region — an honest degradation, never a
/// stale ghost fold.
#[derive(Default)]
pub struct Folds {
    /// The foldable regions of the current tree, sorted by header line.
    regions: Vec<FoldRegion>,
    /// The header lines whose regions are currently collapsed.
    folded: BTreeSet<usize>,
    /// The merged, sorted, disjoint hidden-line intervals derived from
    /// `folded ∩ regions` — the render skip list.
    hidden: Vec<Range<usize>>,
    /// The buffer revision `regions` was derived at, or `None` before the first
    /// refresh (so the first [`needs_refresh`](Self::needs_refresh) is `true`).
    rev: Option<u64>,
    /// A monotonic generation bumped on every region or fold-state change — the
    /// widget's wrap-map cache compares it to know when to rebuild.
    generation: u64,
}

impl Folds {
    /// Whether the cached regions predate buffer `revision` (so the caller should
    /// re-derive them from the current tree).
    #[must_use]
    pub fn needs_refresh(&self, revision: u64) -> bool {
        self.rev != Some(revision)
    }

    /// Replace the cached regions with a fresh derivation at buffer `revision`,
    /// pruning any collapsed header that no longer starts a region, and recompute
    /// the hidden intervals. Passing an empty `regions` (a plain-text buffer with
    /// no grammar) clears folding honestly.
    pub fn update_regions(&mut self, regions: Vec<FoldRegion>, revision: u64) {
        let headers: BTreeSet<usize> = regions.iter().map(|r| r.header_line).collect();
        self.folded.retain(|h| headers.contains(h));
        self.regions = regions;
        self.rev = Some(revision);
        self.recompute_hidden();
    }

    /// The foldable regions of the current tree.
    #[must_use]
    pub fn regions(&self) -> &[FoldRegion] {
        &self.regions
    }

    /// The region whose header is exactly `line`, if any — the gutter-chevron
    /// lookup (a chevron shows only on a real header line).
    #[must_use]
    pub fn region_at_header(&self, line: usize) -> Option<FoldRegion> {
        self.regions.iter().find(|r| r.header_line == line).copied()
    }

    /// Whether the region headed by `line` is currently collapsed.
    #[must_use]
    pub fn is_folded(&self, line: usize) -> bool {
        self.folded.contains(&line)
    }

    /// Whether any fold is currently collapsed (a cheap gate for the fast path).
    #[must_use]
    pub fn any_folded(&self) -> bool {
        !self.hidden.is_empty()
    }

    /// The merged, sorted, disjoint hidden-line intervals (the render skip list).
    #[must_use]
    pub fn hidden(&self) -> &[Range<usize>] {
        &self.hidden
    }

    /// The cache generation — bumped on every region/fold change so a downstream
    /// cache (the wrap map) can detect staleness.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether logical `line` is inside a collapsed region (and so not painted).
    #[must_use]
    pub fn is_line_hidden(&self, line: usize) -> bool {
        // `hidden` is sorted + disjoint; find the last interval starting at/below
        // `line` and test containment.
        let idx = self.hidden.partition_point(|iv| iv.start <= line);
        idx > 0 && self.hidden[idx - 1].end > line
    }

    /// The total number of currently hidden lines.
    #[must_use]
    pub fn hidden_count(&self) -> usize {
        self.hidden.iter().map(|iv| iv.end - iv.start).sum()
    }

    /// The number of *visible* logical lines for a buffer of `total_lines` — the
    /// unwrapped display-row count.
    #[must_use]
    pub fn visible_line_count(&self, total_lines: usize) -> usize {
        total_lines.saturating_sub(self.hidden_count())
    }

    /// The logical line shown on unwrapped display row `display_row` (skipping the
    /// hidden lines above it). Intervals are disjoint + sorted, so one ascending
    /// pass over the (few) collapsed regions resolves it in O(folds).
    #[must_use]
    pub fn line_of_display_row(&self, display_row: usize) -> usize {
        let mut line = display_row;
        for iv in &self.hidden {
            if iv.start <= line {
                line += iv.end - iv.start;
            } else {
                break;
            }
        }
        line
    }

    /// The unwrapped display row a logical `line` renders on. A line that is
    /// itself hidden collapses onto its region's header row (so a stray caret
    /// stays visible).
    #[must_use]
    pub fn display_row_of_line(&self, line: usize) -> usize {
        let mut before = 0;
        for iv in &self.hidden {
            if line >= iv.end {
                before += iv.end - iv.start;
            } else if line >= iv.start {
                // Inside a collapsed region → show at the header line (start - 1).
                return iv.start.saturating_sub(1) - before;
            } else {
                break;
            }
        }
        line - before
    }

    /// The header line of the collapsed region hiding `line`, or `None` when the
    /// line is visible — used to pull a caret out of a region as it folds.
    #[must_use]
    pub fn header_of_hidden_line(&self, line: usize) -> Option<usize> {
        let idx = self.hidden.partition_point(|iv| iv.start <= line);
        (idx > 0 && self.hidden[idx - 1].end > line)
            .then(|| self.hidden[idx - 1].start.saturating_sub(1))
    }

    /// The region to fold given a caret on `line`: the region opening exactly on
    /// `line`, else the innermost (largest-header) region containing it.
    #[must_use]
    pub fn foldable_at(&self, line: usize) -> Option<FoldRegion> {
        if let Some(region) = self.region_at_header(line) {
            return Some(region);
        }
        self.regions
            .iter()
            .filter(|r| r.header_line <= line && line <= r.end_line)
            .max_by_key(|r| r.header_line)
            .copied()
    }

    /// The collapsed region to unfold given a caret on `line`: the region headed
    /// by `line` if it is folded, else the innermost folded region covering it.
    #[must_use]
    pub fn unfoldable_at(&self, line: usize) -> Option<usize> {
        if self.is_folded(line) {
            return Some(line);
        }
        self.regions
            .iter()
            .filter(|r| self.folded.contains(&r.header_line))
            .filter(|r| r.header_line <= line && line <= r.end_line)
            .max_by_key(|r| r.header_line)
            .map(|r| r.header_line)
    }

    /// Collapse the region headed by `header` (a no-op if it is not a region
    /// header or is already folded). Returns whether the state changed.
    pub fn fold(&mut self, header: usize) -> bool {
        if self.region_at_header(header).is_none() || self.folded.contains(&header) {
            return false;
        }
        self.folded.insert(header);
        self.recompute_hidden();
        true
    }

    /// Expand the region headed by `header`. Returns whether the state changed.
    pub fn unfold(&mut self, header: usize) -> bool {
        if !self.folded.remove(&header) {
            return false;
        }
        self.recompute_hidden();
        true
    }

    /// Toggle the region headed by `header` (the gutter chevron seam). Returns
    /// whether the state changed (`false` if `header` is not a region header).
    pub fn toggle(&mut self, header: usize) -> bool {
        if self.folded.contains(&header) {
            self.unfold(header)
        } else {
            self.fold(header)
        }
    }

    /// Rebuild the merged hidden-line intervals from `folded ∩ regions` and bump
    /// the cache generation.
    fn recompute_hidden(&mut self) {
        let mut ivs: Vec<Range<usize>> = self
            .regions
            .iter()
            .filter(|r| self.folded.contains(&r.header_line))
            .map(|r| r.hidden())
            .collect();
        ivs.sort_by_key(|iv| iv.start);
        let mut merged: Vec<Range<usize>> = Vec::with_capacity(ivs.len());
        for iv in ivs {
            if let Some(last) = merged.last_mut() {
                if iv.start <= last.end {
                    last.end = last.end.max(iv.end);
                    continue;
                }
            }
            merged.push(iv);
        }
        self.hidden = merged;
        self.generation = self.generation.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::{FoldRegion, Folds};
    use crate::buffer::Buffer;
    use crate::highlight::{Highlighter, Language};

    /// A small but real Rust sample with an impl block, a method, and a struct —
    /// parsed through the same [`Highlighter`] the editor uses.
    const SAMPLE: &str = "\
struct Point {
    x: i32,
    y: i32,
}

impl Point {
    fn sum(&self) -> i32 {
        self.x + self.y
    }
}
";

    /// Parse `SAMPLE` and derive its fold regions off the live tree.
    fn sample_regions() -> Vec<FoldRegion> {
        let mut buf = Buffer::from_text(SAMPLE);
        let mut hl = Highlighter::new(Language::Rust).expect("rust grammar");
        hl.sync(&mut buf);
        hl.fold_regions(buf.rope())
    }

    #[test]
    fn regions_cover_the_struct_impl_and_method() {
        let regions = sample_regions();
        // struct Point { … } opens on line 0 and closes on line 3.
        assert!(
            regions
                .iter()
                .any(|r| r.header_line == 0 && r.end_line == 3),
            "the struct is foldable (0..=3): {regions:?}"
        );
        // impl Point { … } opens on line 5, closes on line 9.
        assert!(
            regions
                .iter()
                .any(|r| r.header_line == 5 && r.end_line == 9),
            "the impl is foldable (5..=9): {regions:?}"
        );
        // fn sum(&self) … opens on line 6, closes on line 8.
        assert!(
            regions
                .iter()
                .any(|r| r.header_line == 6 && r.end_line == 8),
            "the method is foldable (6..=8): {regions:?}"
        );
    }

    #[test]
    fn regions_are_one_per_header_line_and_sorted() {
        let regions = sample_regions();
        for pair in regions.windows(2) {
            assert!(
                pair[0].header_line < pair[1].header_line,
                "sorted + one region per header line: {regions:?}"
            );
        }
    }

    #[test]
    fn plain_text_has_no_regions() {
        // No grammar → no highlighter → no fold regions (honest, §7).
        assert!(Highlighter::for_path(std::path::Path::new("notes.txt")).is_none());
    }

    #[test]
    fn folding_hides_the_body_and_unfolding_restores_it() {
        let regions = sample_regions();
        let mut folds = Folds::default();
        folds.update_regions(regions, 1);
        assert!(!folds.any_folded(), "nothing folded yet");
        assert_eq!(folds.hidden_count(), 0);

        // Fold the impl block (header line 5, body 6..=9).
        assert!(folds.fold(5), "folding a real header changes state");
        assert!(folds.is_folded(5));
        for line in 6..=9 {
            assert!(
                folds.is_line_hidden(line),
                "line {line} is hidden by the fold"
            );
        }
        assert!(!folds.is_line_hidden(5), "the header stays visible");
        assert!(!folds.is_line_hidden(0), "the struct above is untouched");
        assert_eq!(folds.hidden_count(), 4, "lines 6,7,8,9 hidden");

        // Unfold restores every line.
        assert!(folds.unfold(5), "unfolding changes state");
        assert_eq!(folds.hidden_count(), 0);
        for line in 0..=9 {
            assert!(!folds.is_line_hidden(line), "line {line} restored");
        }
    }

    #[test]
    fn display_rows_skip_hidden_lines_both_ways() {
        let regions = sample_regions();
        let mut folds = Folds::default();
        folds.update_regions(regions, 1);
        // 11 lines total (0..=10, trailing newline yields an empty line 10).
        folds.fold(5); // hide 6,7,8,9
                       // Visible logical lines in order: 0,1,2,3,4,5,10.
        let visible: Vec<usize> = (0..folds.visible_line_count(11))
            .map(|d| folds.line_of_display_row(d))
            .collect();
        assert_eq!(visible, vec![0, 1, 2, 3, 4, 5, 10]);
        // The inverse agrees for a visible line and never lands on a hidden one.
        assert_eq!(folds.display_row_of_line(5), 5);
        assert_eq!(
            folds.display_row_of_line(10),
            6,
            "line 10 is the 7th visible row"
        );
        // A line inside the fold collapses onto the header's display row.
        assert_eq!(folds.display_row_of_line(7), folds.display_row_of_line(5));
    }

    #[test]
    fn nested_folds_merge_into_disjoint_intervals() {
        let regions = sample_regions();
        let mut folds = Folds::default();
        folds.update_regions(regions, 1);
        // Fold both the impl (6..=9) and the method inside it (7..=8): the method's
        // hidden range is a subset, so the merged skip list stays one interval.
        folds.fold(5);
        folds.fold(6);
        assert_eq!(folds.hidden().len(), 1, "overlapping folds merge");
        assert_eq!(folds.hidden_count(), 4);
    }

    #[test]
    fn a_reparse_dropping_a_header_prunes_its_fold() {
        let regions = sample_regions();
        let mut folds = Folds::default();
        folds.update_regions(regions, 1);
        folds.fold(5);
        assert!(folds.is_folded(5));
        // A later revision whose regions no longer include header 5 drops the fold
        // (no stale ghost) — honest degradation.
        folds.update_regions(
            vec![FoldRegion {
                header_line: 0,
                end_line: 3,
            }],
            2,
        );
        assert!(!folds.is_folded(5), "the pruned header is no longer folded");
        assert_eq!(folds.hidden_count(), 0);
    }

    #[test]
    fn foldable_and_unfoldable_lookups_pick_the_enclosing_region() {
        let regions = sample_regions();
        let mut folds = Folds::default();
        folds.update_regions(regions, 1);
        // A caret on the impl body (line 7) folds the innermost region (the method
        // at header 6), not the outer impl.
        assert_eq!(folds.foldable_at(7).map(|r| r.header_line), Some(6));
        // On a header line, that header's own region is chosen.
        assert_eq!(folds.foldable_at(5).map(|r| r.header_line), Some(5));
        // With the impl folded, a caret on its header unfolds it.
        folds.fold(5);
        assert_eq!(folds.unfoldable_at(5), Some(5));
    }
}
