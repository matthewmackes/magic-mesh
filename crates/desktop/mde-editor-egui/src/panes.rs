//! The **split-pane tree** (EDITOR-6) — the editor's tiling model.
//!
//! Mirrors `mde-term-egui`'s `splits.rs` pattern (§6: same design, specialised
//! to the editor's leaf identity rather than a shared session).
//!
//! The heart is a pure binary **split tree**: every node is a [`Pane::Leaf`]
//! holding one [`PaneId`] (a group of open document tabs, owned by the surface's
//! pane registry) or a [`Pane::Split`] cutting its rectangle in two at a
//! draggable `ratio`. Splitting to any depth, closing (the parent split
//! collapses to the sibling), resizing (divider drag), and geometric focus
//! navigation are all tree operations with no toolkit widgets in sight — only
//! egui's pure geometry types ([`Rect`]) — so every one is unit-tested headless.
//!
//! [`crate::panel::EditorSurface`] is the live surface: a pane registry
//! ([`PaneId`] → its open tabs) plus this tree, rendered by laying the tree out
//! into the panel's body rect. The heavy terminal-specific concerns of term's
//! multiplexer (PTY registry, broadcast, remote panes) have no editor analogue;
//! this module keeps only the pure tiling geometry that is genuinely shared.
//!
//! §4: the module carries no colours — only the shared spacing token for the
//! divider gap; the surface paints dividers / focus rings through `Style` tokens.

use mde_egui::egui::{pos2, Pos2, Rect};
use mde_egui::Style;

/// Divider strip thickness in points — the visible gap between sibling panes.
pub const DIVIDER_PX: f32 = Style::SP_XS;

/// The smallest edge a pane may be squeezed to by a divider drag, in points.
pub const MIN_PANE_PX: f32 = 96.0;

/// The stored-ratio clamp: a ratio never leaves `[MIN_RATIO, 1 - MIN_RATIO]`,
/// so even a degenerate tree keeps both children representable.
pub const MIN_RATIO: f32 = 0.05;

/// A group-of-tabs pane's identity in the surface registry and the split tree.
/// Ids are handed out once per pane and never reused within a surface.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct PaneId(pub u64);

/// Which way a [`Pane::Split`] cuts its rectangle.
///
/// Named after the **cut**, exactly as term's split model: `H` is a horizontal
/// divider (children stacked above/below), `V` is a vertical divider (children
/// side-by-side).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SplitDir {
    /// A horizontal cut: child `a` above, child `b` below.
    H,
    /// A vertical cut: child `a` left, child `b` right.
    V,
}

/// A directional focus move (`Alt+arrows`), resolved geometrically against the
/// laid-out tree by [`navigate`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NavDir {
    /// Toward the pane left of the focused one.
    Left,
    /// Toward the pane right of the focused one.
    Right,
    /// Toward the pane above the focused one.
    Up,
    /// Toward the pane below the focused one.
    Down,
}

// ── The split tree (pure — no toolkit) ──────────────────────────────────────

/// The outcome of detaching a leaf while consuming a subtree.
enum Detached {
    /// The whole subtree *was* the detached leaf — nothing remains.
    Gone,
    /// The leaf was found and removed; this is what remains.
    Kept(Pane),
    /// The leaf is not in this subtree; it is returned unchanged.
    NotFound(Pane),
}

/// The split tree: term's pane model, over the editor's [`PaneId`] leaves.
///
/// Ratios are the fraction of the split's rectangle given to child `a`
/// (`0.5` = an even cut) and live between [`MIN_RATIO`] and `1 - MIN_RATIO`;
/// [`layout`] additionally clamps them so no pane renders below [`MIN_PANE_PX`].
#[derive(Clone, PartialEq, Debug)]
pub enum Pane {
    /// One group-of-tabs pane.
    Leaf(PaneId),
    /// A rectangle cut in two, split to any depth.
    Split {
        /// Which way the cut runs.
        dir: SplitDir,
        /// Child `a`'s share of the rectangle.
        ratio: f32,
        /// The top (`H`) or left (`V`) child.
        a: Box<Self>,
        /// The bottom (`H`) or right (`V`) child.
        b: Box<Self>,
    },
}

impl Pane {
    /// A single-pane tree.
    #[must_use]
    pub const fn leaf(id: PaneId) -> Self {
        Self::Leaf(id)
    }

    /// The number of leaves in this subtree.
    #[must_use]
    pub fn leaf_count(&self) -> usize {
        match self {
            Self::Leaf(_) => 1,
            Self::Split { a, b, .. } => a.leaf_count() + b.leaf_count(),
        }
    }

    /// Every leaf in reading order (`a` before `b`, depth-first).
    #[must_use]
    pub fn leaves(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.collect_leaves(&mut out);
        out
    }

    fn collect_leaves(&self, out: &mut Vec<PaneId>) {
        match self {
            Self::Leaf(id) => out.push(*id),
            Self::Split { a, b, .. } => {
                a.collect_leaves(out);
                b.collect_leaves(out);
            }
        }
    }

    /// The first leaf in reading order.
    #[must_use]
    pub fn first_leaf(&self) -> PaneId {
        match self {
            Self::Leaf(id) => *id,
            Self::Split { a, .. } => a.first_leaf(),
        }
    }

    /// Split the leaf `at` in `dir`, making `new` its `b`-slot sibling:
    /// `Leaf(at)` becomes `Split { dir, 0.5, Leaf(at), Leaf(new) }` so the new
    /// pane appears below (`H`) or to the right (`V`). Returns `false` (tree
    /// untouched) when `at` is not in the tree.
    pub fn split(&mut self, at: PaneId, dir: SplitDir, new: PaneId) -> bool {
        match self {
            Self::Leaf(id) if *id == at => {
                *self = Self::Split {
                    dir,
                    ratio: 0.5,
                    a: Box::new(Self::Leaf(at)),
                    b: Box::new(Self::Leaf(new)),
                };
                true
            }
            Self::Leaf(_) => false,
            Self::Split { a, b, .. } => a.split(at, dir, new) || b.split(at, dir, new),
        }
    }

    /// Close the leaf `at`: its parent split collapses to the sibling subtree.
    /// Returns the remaining tree (`None` when the root leaf itself closed) and
    /// whether the leaf was found.
    #[must_use]
    pub fn close(self, at: PaneId) -> (Option<Self>, bool) {
        match self.detach(at) {
            Detached::Gone => (None, true),
            Detached::Kept(rest) => (Some(rest), true),
            Detached::NotFound(rest) => (Some(rest), false),
        }
    }

    fn detach(self, at: PaneId) -> Detached {
        match self {
            Self::Leaf(id) if id == at => Detached::Gone,
            leaf @ Self::Leaf(_) => Detached::NotFound(leaf),
            Self::Split { dir, ratio, a, b } => match (*a).detach(at) {
                Detached::Gone => Detached::Kept(*b),
                Detached::Kept(a) => Detached::Kept(Self::Split {
                    dir,
                    ratio,
                    a: Box::new(a),
                    b,
                }),
                Detached::NotFound(a) => match (*b).detach(at) {
                    Detached::Gone => Detached::Kept(a),
                    Detached::Kept(b) => Detached::Kept(Self::Split {
                        dir,
                        ratio,
                        a: Box::new(a),
                        b: Box::new(b),
                    }),
                    Detached::NotFound(b) => Detached::NotFound(Self::Split {
                        dir,
                        ratio,
                        a: Box::new(a),
                        b: Box::new(b),
                    }),
                },
            },
        }
    }

    /// The `ratio` slot of the split addressed by `path` (as reported on a
    /// [`Divider`]); `None` when the path no longer names a split.
    pub fn ratio_mut(&mut self, path: NodePath) -> Option<&mut f32> {
        let mut node = self;
        let depth = 63_u32.saturating_sub(path.0.leading_zeros());
        for step in (0..depth).rev() {
            match node {
                Self::Leaf(_) => return None,
                Self::Split { a, b, .. } => {
                    node = if (path.0 >> step) & 1 == 1 { b } else { a };
                }
            }
        }
        match node {
            Self::Leaf(_) => None,
            Self::Split { ratio, .. } => Some(ratio),
        }
    }
}

/// The leaf that takes focus when `leaf` closes: the first leaf (reading order)
/// of `leaf`'s sibling subtree — the pane that visually absorbs the freed space.
/// `None` when `leaf` is the root or missing.
#[must_use]
pub fn sibling_first_leaf(tree: &Pane, leaf: PaneId) -> Option<PaneId> {
    match tree {
        Pane::Leaf(_) => None,
        Pane::Split { a, b, .. } => match (a.as_ref(), b.as_ref()) {
            (Pane::Leaf(id), _) if *id == leaf => Some(b.first_leaf()),
            (_, Pane::Leaf(id)) if *id == leaf => Some(a.first_leaf()),
            _ => sibling_first_leaf(a, leaf).or_else(|| sibling_first_leaf(b, leaf)),
        },
    }
}

// ── Layout: tree → rects (pure geometry) ────────────────────────────────────

/// A split node's address: the branch choices from the root, bit-encoded under
/// a sentinel bit (`a` = 0, `b` = 1). Stable for as long as the tree shape is.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NodePath(u64);

impl NodePath {
    /// The root split.
    pub const ROOT: Self = Self(1);

    /// The address of this split's `a` child.
    #[must_use]
    pub const fn child_a(self) -> Self {
        Self(self.0 << 1)
    }

    /// The address of this split's `b` child.
    #[must_use]
    pub const fn child_b(self) -> Self {
        Self(self.0 << 1 | 1)
    }
}

/// One draggable divider, as laid out this frame.
#[derive(Clone, Copy, Debug)]
pub struct Divider {
    /// The split it belongs to (feed to [`Pane::ratio_mut`]).
    pub path: NodePath,
    /// Which way the cut runs.
    pub dir: SplitDir,
    /// The divider strip itself.
    pub rect: Rect,
    /// The whole split rectangle (pointer → ratio maps within this).
    pub span: Rect,
}

/// The tree laid out into a rectangle: every leaf's rect plus every divider.
#[derive(Clone, Debug, Default)]
pub struct Layout {
    /// Leaf rects in reading order.
    pub leaves: Vec<(PaneId, Rect)>,
    /// Divider strips, parents before children.
    pub dividers: Vec<Divider>,
}

/// Lay the tree out into `rect` with the production divider gap and minimum
/// pane size.
#[must_use]
pub fn layout(pane: &Pane, rect: Rect) -> Layout {
    layout_with(pane, rect, DIVIDER_PX, MIN_PANE_PX)
}

/// [`layout`] with explicit divider/min-pane metrics (`0, 0` = the pure
/// ratio-only geometry [`navigate`] reasons over).
fn layout_with(pane: &Pane, rect: Rect, divider_px: f32, min_pane_px: f32) -> Layout {
    let mut out = Layout::default();
    collect_layout(
        pane,
        rect,
        divider_px,
        min_pane_px,
        NodePath::ROOT,
        &mut out,
    );
    out
}

fn collect_layout(
    pane: &Pane,
    rect: Rect,
    divider_px: f32,
    min_pane_px: f32,
    path: NodePath,
    out: &mut Layout,
) {
    match pane {
        Pane::Leaf(id) => out.leaves.push((*id, rect)),
        Pane::Split { dir, ratio, a, b } => {
            let (a_rect, div_rect, b_rect) =
                split_rects(rect, *dir, *ratio, divider_px, min_pane_px);
            out.dividers.push(Divider {
                path,
                dir: *dir,
                rect: div_rect,
                span: rect,
            });
            collect_layout(a, a_rect, divider_px, min_pane_px, path.child_a(), out);
            collect_layout(b, b_rect, divider_px, min_pane_px, path.child_b(), out);
        }
    }
}

/// Cut `rect` into (child a, divider, child b) along `dir`.
fn split_rects(
    rect: Rect,
    dir: SplitDir,
    ratio: f32,
    divider_px: f32,
    min_pane_px: f32,
) -> (Rect, Rect, Rect) {
    match dir {
        SplitDir::V => {
            let inner = (rect.width() - divider_px).max(0.0);
            let cut = rect.min.x + inner * effective_ratio(ratio, inner, min_pane_px);
            (
                Rect::from_min_max(rect.min, pos2(cut, rect.max.y)),
                Rect::from_min_max(pos2(cut, rect.min.y), pos2(cut + divider_px, rect.max.y)),
                Rect::from_min_max(pos2(cut + divider_px, rect.min.y), rect.max),
            )
        }
        SplitDir::H => {
            let inner = (rect.height() - divider_px).max(0.0);
            let cut = rect.min.y + inner * effective_ratio(ratio, inner, min_pane_px);
            (
                Rect::from_min_max(rect.min, pos2(rect.max.x, cut)),
                Rect::from_min_max(pos2(rect.min.x, cut), pos2(rect.max.x, cut + divider_px)),
                Rect::from_min_max(pos2(rect.min.x, cut + divider_px), rect.max),
            )
        }
    }
}

/// The stored-ratio clamp: finite and inside `[MIN_RATIO, 1 - MIN_RATIO]` (a
/// non-finite ratio — e.g. a degenerate drag division — resets to an even cut).
#[must_use]
pub fn clamp_ratio(ratio: f32) -> f32 {
    if ratio.is_finite() {
        ratio.clamp(MIN_RATIO, 1.0 - MIN_RATIO)
    } else {
        0.5
    }
}

/// The ratio actually laid out: [`clamp_ratio`], then tightened so neither
/// child of an `inner`-point span drops below `min_px` (a span too small for
/// two minimum panes falls back to an even cut).
fn effective_ratio(ratio: f32, inner: f32, min_px: f32) -> f32 {
    let ratio = clamp_ratio(ratio);
    if inner < 2.0 * min_px {
        return 0.5;
    }
    let lo = (min_px / inner).min(0.5);
    ratio.clamp(lo, 1.0 - lo)
}

/// The ratio a divider drag lands on: the pointer's position along the split's
/// axis, mapped into the span (divider width accounted, then [`clamp_ratio`]-ed;
/// [`layout`] applies the pixel minimum on top).
#[must_use]
pub fn pointer_ratio(div: &Divider, pointer: Pos2) -> f32 {
    let (coord, min, span, divider_px) = match div.dir {
        SplitDir::V => (
            pointer.x,
            div.span.min.x,
            div.span.width(),
            div.rect.width(),
        ),
        SplitDir::H => (
            pointer.y,
            div.span.min.y,
            div.span.height(),
            div.rect.height(),
        ),
    };
    let inner = (span - divider_px).max(1.0);
    clamp_ratio(divider_px.mul_add(-0.5, coord - min) / inner)
}

// ── Directional focus navigation (pure geometry) ────────────────────────────

/// The pane focus lands on moving `dir` from `from`: the geometrically adjacent leaf.
///
/// Nearest facing edge first, then the largest cross-axis overlap with `from`,
/// then the nearest cross-axis centre (reading order breaks exact ties). `None`
/// at the surface's edge or when `from` is missing.
#[must_use]
pub fn navigate(tree: &Pane, from: PaneId, dir: NavDir) -> Option<PaneId> {
    const EPS: f32 = 1e-4;
    let lay = layout_with(
        tree,
        Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
        0.0,
        0.0,
    );
    let from_rect = lay.leaves.iter().find(|(id, _)| *id == from)?.1;

    let mut best: Option<(f32, f32, f32, PaneId)> = None;
    for (id, r) in &lay.leaves {
        if *id == from {
            continue;
        }
        let (beyond, gap) = match dir {
            NavDir::Right => (r.min.x >= from_rect.max.x - EPS, r.min.x - from_rect.max.x),
            NavDir::Left => (r.max.x <= from_rect.min.x + EPS, from_rect.min.x - r.max.x),
            NavDir::Down => (r.min.y >= from_rect.max.y - EPS, r.min.y - from_rect.max.y),
            NavDir::Up => (r.max.y <= from_rect.min.y + EPS, from_rect.min.y - r.max.y),
        };
        if !beyond {
            continue;
        }
        let (overlap, cross_dist) = match dir {
            NavDir::Left | NavDir::Right => (
                (r.max.y.min(from_rect.max.y) - r.min.y.max(from_rect.min.y)).max(0.0),
                (r.center().y - from_rect.center().y).abs(),
            ),
            NavDir::Up | NavDir::Down => (
                (r.max.x.min(from_rect.max.x) - r.min.x.max(from_rect.min.x)).max(0.0),
                (r.center().x - from_rect.center().x).abs(),
            ),
        };
        let key = (gap, -overlap, cross_dist);
        let better = best.is_none_or(|(bg, bo, bc, _)| key < (bg, bo, bc));
        if better {
            best = Some((key.0, key.1, key.2, *id));
        }
    }
    best.map(|(_, _, _, id)| id)
}

#[cfg(test)]
mod tests {
    use super::{
        clamp_ratio, layout, navigate, pointer_ratio, sibling_first_leaf, NavDir, NodePath, Pane,
        PaneId, SplitDir, MIN_RATIO,
    };
    use mde_egui::egui::{pos2, Rect};

    fn id(n: u64) -> PaneId {
        PaneId(n)
    }

    #[test]
    fn splitting_a_leaf_makes_the_new_pane_its_sibling() {
        let mut tree = Pane::leaf(id(1));
        assert!(tree.split(id(1), SplitDir::V, id(2)), "the target leaf split");
        assert_eq!(tree.leaves(), vec![id(1), id(2)], "both panes are present");
        assert_eq!(tree.leaf_count(), 2);
        // The new pane lands in the b-slot (right of a V-cut).
        assert!(
            matches!(&tree, Pane::Split { dir: SplitDir::V, a, b, .. }
                if **a == Pane::Leaf(id(1)) && **b == Pane::Leaf(id(2))),
            "the split keeps the original pane in slot a and the new one in slot b"
        );
    }

    #[test]
    fn splitting_a_missing_leaf_leaves_the_tree_untouched() {
        let mut tree = Pane::leaf(id(1));
        assert!(!tree.split(id(9), SplitDir::H, id(2)), "no such leaf");
        assert_eq!(tree, Pane::leaf(id(1)));
    }

    #[test]
    fn nested_splits_reach_any_depth() {
        let mut tree = Pane::leaf(id(1));
        tree.split(id(1), SplitDir::V, id(2));
        tree.split(id(2), SplitDir::H, id(3));
        assert_eq!(tree.leaves(), vec![id(1), id(2), id(3)]);
        assert_eq!(tree.leaf_count(), 3);
    }

    #[test]
    fn closing_a_leaf_collapses_its_parent_to_the_sibling() {
        let mut tree = Pane::leaf(id(1));
        tree.split(id(1), SplitDir::V, id(2));
        let (rest, found) = tree.close(id(2));
        assert!(found, "the leaf was in the tree");
        assert_eq!(rest, Some(Pane::leaf(id(1))), "the split collapsed to id 1");
    }

    #[test]
    fn closing_the_root_leaf_empties_the_tree() {
        let tree = Pane::leaf(id(1));
        let (rest, found) = tree.close(id(1));
        assert!(found);
        assert_eq!(rest, None, "no panes remain");
    }

    #[test]
    fn closing_a_missing_leaf_reports_not_found() {
        let tree = Pane::leaf(id(1));
        let (rest, found) = tree.clone().close(id(9));
        assert!(!found);
        assert_eq!(rest, Some(tree));
    }

    #[test]
    fn sibling_first_leaf_absorbs_the_freed_space() {
        let mut tree = Pane::leaf(id(1));
        tree.split(id(1), SplitDir::V, id(2));
        assert_eq!(sibling_first_leaf(&tree, id(1)), Some(id(2)));
        assert_eq!(sibling_first_leaf(&tree, id(2)), Some(id(1)));
        assert_eq!(sibling_first_leaf(&Pane::leaf(id(1)), id(1)), None);
    }

    #[test]
    fn layout_splits_the_rect_by_the_ratio() {
        let mut tree = Pane::leaf(id(1));
        tree.split(id(1), SplitDir::V, id(2));
        let rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(1000.0, 400.0));
        let lay = layout(&tree, rect);
        assert_eq!(lay.leaves.len(), 2);
        assert_eq!(lay.dividers.len(), 1);
        // A 0.5 V-cut of a 1000-wide rect puts the cut near the middle.
        let (_, a_rect) = lay.leaves[0];
        let (_, b_rect) = lay.leaves[1];
        assert!(a_rect.max.x < b_rect.min.x, "a is left of b");
        assert!((a_rect.width() - b_rect.width()).abs() < 8.0, "roughly even");
    }

    #[test]
    fn ratio_mut_addresses_the_root_split() {
        let mut tree = Pane::leaf(id(1));
        tree.split(id(1), SplitDir::V, id(2));
        *tree.ratio_mut(NodePath::ROOT).expect("root split") = 0.7;
        assert!(
            (*tree.ratio_mut(NodePath::ROOT).expect("root split") - 0.7).abs() < f32::EPSILON,
            "the root split's ratio was updated in place",
        );
        // A leaf child has no ratio.
        assert!(tree.ratio_mut(NodePath::ROOT.child_a()).is_none());
    }

    #[test]
    fn pointer_ratio_maps_the_drag_into_the_span() {
        let mut tree = Pane::leaf(id(1));
        tree.split(id(1), SplitDir::V, id(2));
        let rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(400.0, 100.0));
        let lay = layout(&tree, rect);
        let div = lay.dividers[0];
        // Dragging to the horizontal centre lands near a 0.5 ratio.
        assert!((pointer_ratio(&div, pos2(200.0, 50.0)) - 0.5).abs() < 0.05);
        // Off the ends clamps to the stored-ratio bounds.
        assert!((pointer_ratio(&div, pos2(-500.0, 0.0)) - MIN_RATIO).abs() < f32::EPSILON);
        assert!((pointer_ratio(&div, pos2(9e3, 0.0)) - (1.0 - MIN_RATIO)).abs() < f32::EPSILON);
    }

    #[test]
    fn clamp_ratio_keeps_ratios_representable() {
        assert!((clamp_ratio(0.5) - 0.5).abs() < f32::EPSILON);
        assert!((clamp_ratio(0.0) - MIN_RATIO).abs() < f32::EPSILON);
        assert!((clamp_ratio(1.0) - (1.0 - MIN_RATIO)).abs() < f32::EPSILON);
        assert!((clamp_ratio(f32::NAN) - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn navigate_moves_focus_geometrically() {
        // A 2x2 grid: split V (1|2), then split each column H.
        let mut tree = Pane::leaf(id(1));
        tree.split(id(1), SplitDir::V, id(2)); // 1 | 2
        tree.split(id(1), SplitDir::H, id(3)); // 1 over 3, beside 2
        tree.split(id(2), SplitDir::H, id(4)); // 2 over 4
        // Layout: top row [1,2], bottom row [3,4].
        assert_eq!(navigate(&tree, id(1), NavDir::Right), Some(id(2)));
        assert_eq!(navigate(&tree, id(1), NavDir::Down), Some(id(3)));
        assert_eq!(navigate(&tree, id(4), NavDir::Left), Some(id(3)));
        assert_eq!(navigate(&tree, id(4), NavDir::Up), Some(id(2)));
        // At the edge there is nowhere to go.
        assert_eq!(navigate(&tree, id(1), NavDir::Left), None);
        assert_eq!(navigate(&tree, id(1), NavDir::Up), None);
        // A missing leaf navigates nowhere.
        assert_eq!(navigate(&tree, id(9), NavDir::Right), None);
    }
}
