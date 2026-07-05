//! Nested split panes (TERM-4) — Terminator's split model over TERM-3 panes.
//!
//! The heart is a pure binary **split tree** (design lock Q4): every node is a
//! [`Pane::Leaf`] holding one shell session, or a [`Pane::Split`] cutting its
//! rectangle in two at a draggable `ratio`. Splitting to any depth, closing
//! (the parent split collapses to the sibling), and drag-rearranging (a leaf
//! detaches and re-splits at a drop target) are all tree operations with no
//! toolkit types in sight, so every one of them is unit-tested headless.
//!
//! [`SplitTerminal`] is the live multiplexer: a session registry
//! ([`SessionId`] → the TERM-3 [`TerminalWidget`], each owning its own
//! [`LocalPty`] shell) plus the tree, rendered by recursively laying the tree
//! out into the available rect. Interactions (Terminator's defaults, lock Q15;
//! rebinding is TERM-12):
//!
//! - **dividers** are thin `Sense::drag` strips between child rects; dragging
//!   one adjusts the split's `ratio`, clamped so no pane collapses below a
//!   minimum;
//! - **zoom** (`Ctrl+Shift+X`, lock Q8) maximizes the focused pane over the
//!   whole surface and restores it on the next toggle;
//! - **drag-rearrange** (lock Q8): hold **Alt** and drag a pane onto another;
//!   the drop half (nearest edge) picks the new split direction and side
//!   (Terminator drags its titlebars — this surface has none until TERM-12's
//!   titles, so the chord stands in);
//! - **splits**: `Ctrl+Shift+O` cuts horizontally (stacked), `Ctrl+Shift+E`
//!   vertically (side-by-side); the new shell takes focus;
//! - **close** (`Ctrl+Shift+W`) ends the focused session; a shell that exits
//!   on its own closes its pane the same way (Terminator behaviour) — the last
//!   pane closing empties the surface and the binary closes the window;
//! - **focus follows**: clicks focus a pane, `Alt+arrows` navigate
//!   geometrically, a split focuses the new pane, a close falls back to the
//!   collapsed sibling. The focused pane wears a hairline [`Style::ACCENT`]
//!   ring whenever more than one pane is up.
//! - **broadcast/grouped input** (TERM-6, design lock Q5): the focused pane's
//!   typing can fan out to every pane ([`Broadcast::All`]) or to the panes
//!   sharing its named group ([`Broadcast::Group`]); each fan-out byte is
//!   replayed through the target pane's own PTY-write path (§6 — the widget
//!   still owns encoding + the write). Panes in the live set wear a
//!   [`Style::WARN`] indicator border; the mode toggles by `Ctrl+Shift+A` /
//!   `Ctrl+Shift+G` or the on-surface chip, and panes are assigned to named
//!   groups from the per-pane badge.
//!
//! §4: every colour here is a `Style` token (dividers, focus ring, drop
//! previews, chips) — the terminal *content* palette stays [`crate::palette`]'s
//! documented carve-out inside the widget.

use std::collections::HashMap;
use std::io;
use std::time::{Duration, Instant};

use mde_egui::egui::{
    pos2, vec2, Align2, Context, CursorIcon, Event, FontId, Id, Key, Modifiers, Pos2, Rect,
    Response, Sense, Stroke, StrokeKind, Ui, UiBuilder, Vec2,
};
use mde_egui::Style;

use crate::appearance::Appearance;
use crate::bell::BellConfig;
use crate::layout::{cwd_of_pid, LayoutPane, LayoutTab, PaneSpec};
use crate::picker::RemoteTarget;
use crate::pty::{LocalPty, SpawnOptions};
use crate::remote::RemotePty;
use crate::widget::{chip, TerminalWidget};

/// Divider strip thickness in points — the visible gap between sibling panes.
const DIVIDER_PX: f32 = Style::SP_XS;

/// Extra grab slop on each side of a divider (the strip is thin; the hit area
/// overlaps the neighbouring panes slightly and, being registered after them,
/// wins the pointer).
const DIVIDER_HIT_SLOP: f32 = 2.0;

/// The smallest edge a pane may be squeezed to by a divider drag, in points.
const MIN_PANE_PX: f32 = 48.0;

/// The stored-ratio clamp: a ratio never leaves `[MIN_RATIO, 1 - MIN_RATIO]`,
/// so even a degenerate tree keeps both children representable.
pub const MIN_RATIO: f32 = 0.05;

/// How long the spawn-failure chip stays up.
const ERROR_TTL: Duration = Duration::from_secs(6);

/// The broadcast indicator border thickness — a [`Style::WARN`] ring on every
/// pane in the fan-out set, a touch heavier than the focus ring so it reads
/// first (nested just inside the ring, so a focused broadcasting pane shows
/// both cues).
const BROADCAST_BORDER_PX: f32 = 2.0;

/// The named groups the per-pane badge assigns into, in cycle order (ungrouped
/// → each in turn → ungrouped). A free-text group namer is a TERM-12 titlebar
/// refinement; this fixed ring is the reachable "named group" set today.
const GROUP_RING: [&str; 3] = ["A", "B", "C"];

/// A shell session's identity in the registry and the tree. Ids are handed
/// out once per spawn and never reused within a surface's lifetime.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SessionId(pub u64);

/// Which way a [`Pane::Split`] cuts its rectangle.
///
/// Named after the **cut**, exactly as Terminator names its actions: `H` is
/// "split horizontally" (a horizontal divider — children stacked above/below),
/// `V` is "split vertically" (a vertical divider — children side-by-side).
///
/// Serde-serializable so a saved layout (TERM-10) records the surface's own cut
/// direction rather than a parallel copy.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum SplitDir {
    /// A horizontal cut: child `a` above, child `b` below.
    H,
    /// A vertical cut: child `a` left, child `b` right.
    V,
}

/// A directional focus move (`Alt+arrows`), resolved geometrically against
/// the laid-out tree by [`navigate`].
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

/// A surface-level command decoded from the keyboard by [`consume_commands`]
/// and applied through [`SplitTerminal::apply`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Command {
    /// Split the focused pane (`Ctrl+Shift+O` = [`SplitDir::H`],
    /// `Ctrl+Shift+E` = [`SplitDir::V`]); the new shell takes focus.
    Split(SplitDir),
    /// Close the focused pane (`Ctrl+Shift+W`); its parent split collapses.
    Close,
    /// Maximize the focused pane over the surface, or restore the tiling
    /// (`Ctrl+Shift+X`, and `Ctrl+Shift+Z` for Terminator zoom muscle-memory —
    /// font-scaling zoom is out of TERM-4's scope).
    ToggleZoom,
    /// Move focus to the geometrically adjacent pane (`Alt+arrows`).
    Focus(NavDir),
    /// Toggle a broadcast routing mode on, or back off if already active
    /// (`Ctrl+Shift+A` = [`Broadcast::All`], `Ctrl+Shift+G` = [`Broadcast::Group`]).
    ToggleBroadcast(Broadcast),
}

/// Where typed input in the focused pane is routed — Terminator's broadcast
/// (design lock Q5): only the focused pane, every pane, or a named group.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Broadcast {
    /// Input reaches only the focused pane (the default).
    #[default]
    Off,
    /// Input fans out to every pane in the active split tree.
    All,
    /// Input fans out to every pane sharing the focused pane's named group.
    Group,
}

impl Broadcast {
    /// The next mode in the on-surface chip's cycle: `Off → All → Group → Off`.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Off => Self::All,
            Self::All => Self::Group,
            Self::Group => Self::Off,
        }
    }
}

// ── The split tree (pure — no toolkit, no PTY) ──────────────────────────────

/// Why a [`Pane::reparent`] was refused. The tree is untouched on every error.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReparentError {
    /// The moved leaf and the drop target are the same pane — a leaf cannot
    /// become its own sibling (the "into its own subtree" rejection).
    IntoOwnSubtree,
    /// The leaf to move is not in the tree.
    LeafNotFound,
    /// The drop target is not in the tree.
    TargetNotFound,
}

/// The outcome of detaching a leaf while consuming a subtree.
enum Detached {
    /// The whole subtree *was* the detached leaf — nothing remains.
    Gone,
    /// The leaf was found and removed; this is what remains.
    Kept(Pane),
    /// The leaf is not in this subtree; it is returned unchanged.
    NotFound(Pane),
}

/// The split tree: Terminator's pane model (design lock Q4).
///
/// Ratios are the fraction of the split's rectangle given to child `a`
/// (`0.5` = an even cut) and live between [`MIN_RATIO`] and `1 - MIN_RATIO`;
/// [`layout`] additionally clamps them so no pane renders below
/// [`MIN_PANE_PX`].
#[derive(Clone, PartialEq, Debug)]
pub enum Pane {
    /// One terminal session.
    Leaf(SessionId),
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
    /// A single-session tree.
    #[must_use]
    pub const fn leaf(id: SessionId) -> Self {
        Self::Leaf(id)
    }

    /// Whether `id`'s leaf is anywhere in this subtree.
    #[must_use]
    pub fn contains(&self, id: SessionId) -> bool {
        match self {
            Self::Leaf(leaf) => *leaf == id,
            Self::Split { a, b, .. } => a.contains(id) || b.contains(id),
        }
    }

    /// Every leaf in reading order (`a` before `b`, depth-first).
    #[must_use]
    pub fn leaves(&self) -> Vec<SessionId> {
        let mut out = Vec::new();
        self.collect_leaves(&mut out);
        out
    }

    fn collect_leaves(&self, out: &mut Vec<SessionId>) {
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
    pub fn first_leaf(&self) -> SessionId {
        match self {
            Self::Leaf(id) => *id,
            Self::Split { a, .. } => a.first_leaf(),
        }
    }

    /// Split the leaf `at` in `dir`, making `new` its sibling: `Leaf(at)`
    /// becomes `Split { dir, 0.5, Leaf(at), Leaf(new) }`. Returns `false`
    /// (tree untouched) when `at` is not in the tree.
    pub fn split(&mut self, at: SessionId, dir: SplitDir, new: SessionId) -> bool {
        self.insert_beside(at, dir, false, Self::Leaf(new))
            .is_none()
    }

    /// Replace the leaf `target` with a split of itself and `pane`; `first`
    /// puts `pane` in the `a` slot (above/left), otherwise `b` (below/right).
    /// Returns `pane` back untouched when `target` is not in the tree.
    pub fn insert_beside(
        &mut self,
        target: SessionId,
        dir: SplitDir,
        first: bool,
        pane: Self,
    ) -> Option<Self> {
        match self {
            Self::Leaf(id) if *id == target => {
                let existing = std::mem::replace(self, Self::Leaf(target));
                let (a, b) = if first {
                    (pane, existing)
                } else {
                    (existing, pane)
                };
                *self = Self::Split {
                    dir,
                    ratio: 0.5,
                    a: Box::new(a),
                    b: Box::new(b),
                };
                None
            }
            Self::Leaf(_) => Some(pane),
            Self::Split { a, b, .. } => a
                .insert_beside(target, dir, first, pane)
                .and_then(|pane| b.insert_beside(target, dir, first, pane)),
        }
    }

    /// Replace the leaf `at` with an arbitrary subtree. Returns `with` back
    /// untouched when `at` is not in the tree.
    pub fn replace(&mut self, at: SessionId, with: Self) -> Option<Self> {
        match self {
            Self::Leaf(id) if *id == at => {
                *self = with;
                None
            }
            Self::Leaf(_) => Some(with),
            Self::Split { a, b, .. } => a.replace(at, with).and_then(|with| b.replace(at, with)),
        }
    }

    /// Close the leaf `at`: its parent split collapses to the sibling subtree.
    /// Returns the remaining tree (`None` when the root leaf itself closed)
    /// and whether the leaf was found.
    #[must_use]
    pub fn close(self, at: SessionId) -> (Option<Self>, bool) {
        match self.detach(at) {
            Detached::Gone => (None, true),
            Detached::Kept(rest) => (Some(rest), true),
            Detached::NotFound(rest) => (Some(rest), false),
        }
    }

    fn detach(self, at: SessionId) -> Detached {
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

    /// Drag-rearrange: detach the leaf `leaf` (its old parent collapses, as a
    /// close) and re-split it beside `target` in `dir` (`first` = the a slot).
    ///
    /// # Errors
    ///
    /// [`ReparentError`] when `leaf == target` (a leaf cannot land inside its
    /// own subtree) or either end is missing; the tree is untouched on error.
    pub fn reparent(
        &mut self,
        leaf: SessionId,
        target: SessionId,
        dir: SplitDir,
        first: bool,
    ) -> Result<(), ReparentError> {
        if leaf == target {
            return Err(ReparentError::IntoOwnSubtree);
        }
        if !self.contains(leaf) {
            return Err(ReparentError::LeafNotFound);
        }
        if !self.contains(target) {
            return Err(ReparentError::TargetNotFound);
        }
        // Checks passed: the detach finds `leaf`, and `target` survives it
        // (target ≠ leaf), so the rebuild below cannot fail.
        let tree = std::mem::replace(self, Self::Leaf(leaf));
        let (rest, _removed) = tree.close(leaf);
        let Some(mut rest) = rest else {
            // Unreachable: `target` is a *different* leaf, so the tree keeps
            // at least one leaf after the detach.
            return Err(ReparentError::TargetNotFound);
        };
        let unplaced = rest.insert_beside(target, dir, first, Self::Leaf(leaf));
        debug_assert!(unplaced.is_none(), "target was verified present");
        *self = rest;
        Ok(())
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

/// The leaf that takes focus when `leaf` closes: the first leaf (reading
/// order) of `leaf`'s sibling subtree — the pane that visually absorbs the
/// freed space. `None` when `leaf` is the root or missing.
#[must_use]
pub fn sibling_first_leaf(tree: &Pane, leaf: SessionId) -> Option<SessionId> {
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

/// A split node's address: the branch choices from the root, bit-encoded
/// under a sentinel bit (`a` = 0, `b` = 1).
///
/// Stable for as long as the tree shape is; trees deeper than 63 splits stop
/// being divider-addressable — unreachable in practice, since
/// [`MIN_PANE_PX`] caps real depth at `log2(screen / min pane)`.
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
    pub leaves: Vec<(SessionId, Rect)>,
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

/// The stored-ratio clamp: finite and inside `[MIN_RATIO, 1 - MIN_RATIO]`
/// (a non-finite ratio — e.g. from a degenerate drag division — resets to an
/// even cut).
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
    if inner <= 0.0 {
        return 0.5;
    }
    let lo = (min_px / inner).min(0.5);
    ratio.clamp(lo, 1.0 - lo)
}

/// The ratio a divider drag lands on: the pointer's position along the
/// split's axis, mapped into the span (divider width accounted, then
/// [`clamp_ratio`]-ed; [`layout`] applies the pixel minimum on top).
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

/// The pane focus lands on moving `dir` from `from`: the geometrically
/// adjacent leaf.
///
/// Nearest facing edge first, then the largest cross-axis overlap with
/// `from`, then the nearest cross-axis centre (reading order breaks exact
/// ties). `None` at the surface's edge or when `from` is missing.
#[must_use]
pub fn navigate(tree: &Pane, from: SessionId, dir: NavDir) -> Option<SessionId> {
    const EPS: f32 = 1e-4;
    let lay = layout_with(
        tree,
        Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
        0.0,
        0.0,
    );
    let from_rect = lay.leaves.iter().find(|(id, _)| *id == from)?.1;

    let mut best: Option<(f32, f32, f32, SessionId)> = None;
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
        let better = best.is_none_or(|(bg, bo, bc, _)| {
            key < (bg, bo, bc) // lexicographic; strict `<` keeps reading order on ties
        });
        if better {
            best = Some((key.0, key.1, key.2, *id));
        }
    }
    best.map(|(_, _, _, id)| id)
}

// ── Drag-rearrange drop zones (pure geometry) ───────────────────────────────

/// Where a pane drop at `pointer` lands within `rect`.
///
/// The nearest edge picks the split direction and side, and the returned
/// half-rect is the drop preview. Left/top land the moved pane in the `a`
/// slot (`first == true`).
#[must_use]
pub fn drop_zone(rect: Rect, pointer: Pos2) -> (SplitDir, bool, Rect) {
    let u = ((pointer.x - rect.min.x) / rect.width().max(1.0)).clamp(0.0, 1.0);
    let v = ((pointer.y - rect.min.y) / rect.height().max(1.0)).clamp(0.0, 1.0);
    let (left, right, top, bottom) = (u, 1.0 - u, v, 1.0 - v);
    let c = rect.center();
    if left <= right && left <= top && left <= bottom {
        (
            SplitDir::V,
            true,
            Rect::from_min_max(rect.min, pos2(c.x, rect.max.y)),
        )
    } else if right <= top && right <= bottom {
        (
            SplitDir::V,
            false,
            Rect::from_min_max(pos2(c.x, rect.min.y), rect.max),
        )
    } else if top <= bottom {
        (
            SplitDir::H,
            true,
            Rect::from_min_max(rect.min, pos2(rect.max.x, c.y)),
        )
    } else {
        (
            SplitDir::H,
            false,
            Rect::from_min_max(pos2(rect.min.x, c.y), rect.max),
        )
    }
}

// ── Keyboard: chords → commands ─────────────────────────────────────────────

/// Decode and **consume** this frame's split-surface chords (Terminator's
/// defaults, lock Q15) before any [`TerminalWidget`] clones the event stream —
/// consumed keys never reach a shell.
///
/// One backend wrinkle: winit folds `Ctrl(+Shift)+X` into [`Event::Cut`]
/// before a `Key` event ever exists, so the zoom chord additionally claims a
/// `Cut` arriving with Shift held (plain `Ctrl+X` stays the widget's CAN
/// byte); the bare-DRM backend delivers raw keys and takes the `Key::X` path.
#[must_use]
pub fn consume_commands(ctx: &Context) -> Vec<Command> {
    ctx.input_mut(|input| {
        let mut cmds = Vec::new();
        let cs = Modifiers::CTRL | Modifiers::SHIFT;
        if input.consume_key(cs, Key::O) {
            cmds.push(Command::Split(SplitDir::H));
        }
        if input.consume_key(cs, Key::E) {
            cmds.push(Command::Split(SplitDir::V));
        }
        if input.consume_key(cs, Key::W) {
            cmds.push(Command::Close);
        }
        if input.consume_key(cs, Key::X) || input.consume_key(cs, Key::Z) {
            cmds.push(Command::ToggleZoom);
        }
        // Broadcast toggles (Terminator-style): all-panes / same-group.
        if input.consume_key(cs, Key::A) {
            cmds.push(Command::ToggleBroadcast(Broadcast::All));
        }
        if input.consume_key(cs, Key::G) {
            cmds.push(Command::ToggleBroadcast(Broadcast::Group));
        }
        let shifted_ctrl =
            input.modifiers.shift && (input.modifiers.ctrl || input.modifiers.command);
        if shifted_ctrl {
            let before = input.events.len();
            input.events.retain(|event| !matches!(event, Event::Cut));
            if input.events.len() < before {
                cmds.push(Command::ToggleZoom);
            }
        }
        for (key, dir) in [
            (Key::ArrowLeft, NavDir::Left),
            (Key::ArrowRight, NavDir::Right),
            (Key::ArrowUp, NavDir::Up),
            (Key::ArrowDown, NavDir::Down),
        ] {
            if input.consume_key(Modifiers::ALT, key) {
                cmds.push(Command::Focus(dir));
            }
        }
        cmds
    })
}

// ── The live multiplexer ────────────────────────────────────────────────────

/// The split-pane terminal: the tree, the session registry, focus/zoom state,
/// and the frame renderer. See the module docs for the interaction model.
pub struct SplitTerminal {
    /// The split tree; `None` once every pane has closed (the surface's cue
    /// to close the window).
    tree: Option<Pane>,
    /// The session registry: every live leaf's widget (each owns its PTY).
    sessions: HashMap<SessionId, TerminalWidget>,
    /// Last frame's egui id per pane, for pre-focusing before widgets render.
    pane_ids: HashMap<SessionId, Id>,
    /// The tree-focused pane — the one holding the keyboard.
    focused: SessionId,
    /// The maximized pane, if any (lock Q8's zoom).
    zoomed: Option<SessionId>,
    /// The leaf currently being Alt-dragged toward a new home.
    drag: Option<SessionId>,
    /// Monotonic id source for [`SessionId`]s.
    next_id: u64,
    /// The spawn recipe every new pane's shell uses.
    spawn_opts: SpawnOptions,
    /// The last spawn failure, chip-displayed until [`ERROR_TTL`] passes.
    error: Option<(String, Instant)>,
    /// The current broadcast routing mode (TERM-6).
    broadcast: Broadcast,
    /// Named-group membership: a pane's leaf → its group label. A pane not in
    /// the map is ungrouped; an entry is dropped when its pane closes.
    groups: HashMap<SessionId, String>,
    /// The surface appearance (TERM-11): scheme + font size + cursor style,
    /// pushed into every pane each frame so a picker change reaches all shells.
    appearance: Appearance,
    /// The selection context menu (TERM-15): the user's custom commands + the
    /// Chat recipient, pushed into every pane each frame (like [`Self::appearance`])
    /// so a config change — and every freshly split pane — carries the same menu.
    menu: crate::menu::ContextMenu,
}

impl SplitTerminal {
    /// Open the multiplexer with one shell spawned from `spawn_opts` (the
    /// recipe every later split reuses).
    ///
    /// # Errors
    ///
    /// The first shell's spawn failure — whatever the OS refused
    /// ([`LocalPty::spawn`]). Later spawn failures (splits) surface as an
    /// in-pane error chip instead, since a session is already running.
    pub fn new(spawn_opts: SpawnOptions) -> io::Result<Self> {
        let mut this = Self::bare(spawn_opts);
        let first = this.spawn_session()?;
        this.tree = Some(Pane::leaf(first));
        this.focused = first;
        Ok(this)
    }

    /// Open the multiplexer with a **remote** mesh shell as its first pane
    /// (TERM-8), driven over the TERM-7 broker. Infallible: opening a remote pane
    /// only publishes the request, and a publish failure (no Bus) surfaces as the
    /// pane's honest `Failed` chip — never a spawn error. Later splits within this
    /// tab reuse `spawn_opts` for local shells, as elsewhere.
    #[must_use]
    pub fn from_remote(remote: crate::remote::RemotePty, spawn_opts: SpawnOptions) -> Self {
        let mut this = Self::bare(spawn_opts);
        let id = SessionId(this.next_id);
        this.next_id += 1;
        this.sessions.insert(id, TerminalWidget::new_remote(remote));
        this.tree = Some(Pane::leaf(id));
        this.focused = id;
        this
    }

    /// A registry/tree-less shell sharing the common field defaults.
    fn bare(spawn_opts: SpawnOptions) -> Self {
        Self {
            tree: None,
            sessions: HashMap::new(),
            pane_ids: HashMap::new(),
            focused: SessionId(0),
            zoomed: None,
            drag: None,
            next_id: 0,
            spawn_opts,
            error: None,
            broadcast: Broadcast::Off,
            groups: HashMap::new(),
            appearance: Appearance::default(),
            menu: crate::menu::ContextMenu::default(),
        }
    }

    /// Adopt the surface [`Appearance`] (TERM-11). The tabbed surface calls this
    /// on the active tab each frame before rendering; [`Self::show_panes`] then
    /// hands it to every live pane, so a scheme / font / cursor change reaches
    /// all shells at once.
    pub const fn set_appearance(&mut self, appearance: Appearance) {
        self.appearance = appearance;
    }

    /// Adopt the surface's selection context menu (TERM-15). The tabbed surface
    /// calls this on the active tab each frame; [`Self::show_panes`] then hands it
    /// to every live pane, so a config change — and every freshly split pane —
    /// carries the same custom commands + Chat recipient.
    pub fn set_context_menu(&mut self, menu: crate::menu::ContextMenu) {
        self.menu = menu;
    }

    /// `true` once every pane has closed — the surface should close with it.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.tree.is_none()
    }

    /// The number of live sessions in the registry.
    #[must_use]
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Pane `id`'s active content scheme (TERM-11). Test-only — lets the tabbed
    /// surface's tests assert the appearance actually reached the panes.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn pane_palette(&self, id: SessionId) -> Option<crate::palette::Palette> {
        self.sessions.get(&id).map(TerminalWidget::palette)
    }

    /// The tree-focused session.
    #[must_use]
    pub const fn focused_session(&self) -> SessionId {
        self.focused
    }

    /// The split tree, for reading the layout out (TERM-10 capture); `None` once
    /// every pane has closed.
    #[must_use]
    pub const fn tree(&self) -> Option<&Pane> {
        self.tree.as_ref()
    }

    // ── TERM-10: capture this tab into a saved layout, and rebuild one ──────────

    /// Capture this tab's live arrangement into a serializable [`LayoutTab`]: the
    /// split tree's exact shape (reusing [`Pane`] + [`SplitDir`]) plus each pane's
    /// relaunch spec. `None` when the tab is empty (nothing to save).
    #[must_use]
    pub fn capture_tab(&self, title: impl Into<String>) -> Option<LayoutTab> {
        let root = self.capture_pane(self.tree.as_ref()?);
        Some(LayoutTab {
            title: title.into(),
            root,
        })
    }

    /// Project the runtime [`Pane`] tree into a [`LayoutPane`] tree, resolving each
    /// leaf's [`SessionId`] into its relaunch [`PaneSpec`].
    fn capture_pane(&self, node: &Pane) -> LayoutPane {
        match node {
            Pane::Leaf(id) => LayoutPane::Leaf(self.capture_spec(*id)),
            Pane::Split { dir, ratio, a, b } => LayoutPane::Split {
                dir: *dir,
                ratio: *ratio,
                a: Box::new(self.capture_pane(a)),
                b: Box::new(self.capture_pane(b)),
            },
        }
    }

    /// The relaunch spec for one live leaf: a remote pane records its target node
    /// (peer + marker); a local pane records its live cwd (from `/proc`) and the
    /// tab's shell recipe. A vanished id degrades to a default local pane.
    fn capture_spec(&self, id: SessionId) -> PaneSpec {
        let Some(widget) = self.sessions.get(&id) else {
            return PaneSpec::default();
        };
        if let Some(target) = widget.remote_target() {
            return PaneSpec::remote(target);
        }
        let cwd = widget
            .local_pty()
            .and_then(|pty| cwd_of_pid(pty.child_pid()));
        PaneSpec::local(cwd, self.spawn_opts.shell.clone())
    }

    /// Rebuild a whole tab from a saved [`LayoutTab`]: recreate the split tree's
    /// shape, spawning a fresh local shell for each local pane (at its saved cwd +
    /// command) and reconnecting each remote pane through `make_remote` (the
    /// TERM-7 broker path). `base` is the fallback spawn recipe for the tab's
    /// local panes.
    ///
    /// # Errors
    /// The first local shell's spawn failure (whatever the OS refused) — the same
    /// contract as [`Self::new`]; remote panes never fail to *open* (an
    /// unreachable node surfaces its own honest chip).
    pub fn from_layout(
        tab: &LayoutTab,
        base: SpawnOptions,
        make_remote: &mut impl FnMut(&RemoteTarget) -> RemotePty,
    ) -> io::Result<Self> {
        let mut this = Self::bare(base);
        let tree = this.build_pane(&tab.root, make_remote)?;
        let first = tree.first_leaf();
        this.tree = Some(tree);
        this.focused = first;
        Ok(this)
    }

    /// Recursively rebuild one [`LayoutPane`] into a live [`Pane`], minting a fresh
    /// [`SessionId`] + widget per leaf and inserting it into the registry.
    fn build_pane(
        &mut self,
        node: &LayoutPane,
        make_remote: &mut impl FnMut(&RemoteTarget) -> RemotePty,
    ) -> io::Result<Pane> {
        match node {
            LayoutPane::Leaf(spec) => {
                let id = SessionId(self.next_id);
                self.next_id += 1;
                let widget = match &spec.target {
                    Some(target) => TerminalWidget::new_remote(make_remote(target)),
                    None => {
                        let opts = SpawnOptions {
                            cwd: spec.cwd.clone(),
                            shell: spec.command.clone(),
                            ..self.spawn_opts.clone()
                        };
                        TerminalWidget::new(LocalPty::spawn(opts)?)
                    }
                };
                self.sessions.insert(id, widget);
                Ok(Pane::Leaf(id))
            }
            LayoutPane::Split { dir, ratio, a, b } => {
                let a = self.build_pane(a, make_remote)?;
                let b = self.build_pane(b, make_remote)?;
                Ok(Pane::Split {
                    dir: *dir,
                    ratio: clamp_ratio(*ratio),
                    a: Box::new(a),
                    b: Box::new(b),
                })
            }
        }
    }

    /// Apply one keyboard [`Command`].
    pub fn apply(&mut self, cmd: Command) {
        match cmd {
            Command::Split(dir) => self.split_focused(dir),
            Command::Close => self.close_session(self.focused),
            Command::ToggleZoom => self.toggle_zoom(),
            Command::Focus(dir) => self.focus_dir(dir),
            Command::ToggleBroadcast(mode) => self.toggle_broadcast(mode),
        }
    }

    /// The focused pane's widget (TERM-12 pane actions target this).
    fn focused_widget_mut(&mut self) -> Option<&mut TerminalWidget> {
        self.sessions.get_mut(&self.focused)
    }

    /// Begin renaming the focused pane (the `RenamePane` action, TERM-12).
    pub fn begin_rename_focused(&mut self) {
        if let Some(w) = self.focused_widget_mut() {
            w.begin_rename();
        }
    }

    /// Toggle watch-for-activity on the focused pane (TERM-12).
    pub fn toggle_activity_watch_focused(&mut self) {
        if let Some(w) = self.focused_widget_mut() {
            w.toggle_activity_watch();
        }
    }

    /// Toggle watch-for-silence on the focused pane (TERM-12).
    pub fn toggle_silence_watch_focused(&mut self) {
        if let Some(w) = self.focused_widget_mut() {
            w.toggle_silence_watch();
        }
    }

    // ── TERM-MENUBAR-1 seams ────────────────────────────────────────────────
    // The top menu bar drives the focused pane's existing features (§6): Copy /
    // Find / Clear are the mouse twins of `Ctrl+Shift+C` / `Ctrl+Shift+F` /
    // `Ctrl+L`, and Bell is the appearance-style surface-wide push. Each mirrors
    // the `*_focused` idiom the TERM-12 pane actions above already established.

    /// The focused pane's widget, read-only — the menu bar reads the selection /
    /// search / bell state through it to gate + check its items.
    fn focused_widget(&self) -> Option<&TerminalWidget> {
        self.sessions.get(&self.focused)
    }

    /// Copy the focused pane's selection to the clipboard (Edit → Copy).
    pub fn copy_focused(&self, ctx: &Context) {
        if let Some(w) = self.focused_widget() {
            w.copy_selection_to_clipboard(ctx);
        }
    }

    /// Whether the focused pane has a non-empty selection — gates Edit → Copy so
    /// it greys out with nothing to copy (§7), never a silent no-op.
    #[must_use]
    pub fn focused_has_selection(&self) -> bool {
        self.focused_widget()
            .is_some_and(TerminalWidget::has_selection)
    }

    /// Toggle the focused pane's scrollback-search overlay (Edit → Find).
    pub fn toggle_search_focused(&mut self) {
        if let Some(w) = self.focused_widget_mut() {
            w.toggle_search();
        }
    }

    /// Whether the focused pane's search overlay is open (Edit → Find checkmark).
    #[must_use]
    pub fn focused_is_searching(&self) -> bool {
        self.focused_widget()
            .is_some_and(TerminalWidget::is_searching)
    }

    /// Clear the focused pane (Edit → Clear).
    pub fn clear_focused(&mut self) {
        if let Some(w) = self.focused_widget_mut() {
            w.clear_screen();
        }
    }

    /// The focused pane's bell config (Terminal → Bell checkmarks), or `None`
    /// when the tab is empty.
    #[must_use]
    pub fn focused_bell_config(&self) -> Option<BellConfig> {
        self.focused_widget().map(TerminalWidget::bell_config)
    }

    /// Set every pane's bell style at once (Terminal → Bell), so the choice
    /// reaches the whole tab like the surface appearance does (TERM-11).
    pub fn set_bell_config_all(&mut self, config: BellConfig) {
        for w in self.sessions.values_mut() {
            w.set_bell_config(config);
        }
    }

    /// The focused pane's shown title (TERM-12) — used by the tab strip to echo
    /// the active pane's label.
    #[must_use]
    pub fn focused_title(&self) -> Option<&str> {
        self.sessions
            .get(&self.focused)
            .map(TerminalWidget::title_text)
    }

    /// The current broadcast routing mode.
    #[must_use]
    pub const fn broadcast(&self) -> Broadcast {
        self.broadcast
    }

    /// Set the broadcast routing mode outright (the on-surface chip cycles
    /// through this).
    pub const fn set_broadcast(&mut self, mode: Broadcast) {
        self.broadcast = mode;
    }

    /// Toggle `mode` on, or back to [`Broadcast::Off`] when it is already
    /// active — the keybind semantics, so the same chord turns its mode off.
    pub fn toggle_broadcast(&mut self, mode: Broadcast) {
        self.broadcast = if self.broadcast == mode {
            Broadcast::Off
        } else {
            mode
        };
    }

    /// The named group a pane is labelled into, if any.
    #[must_use]
    pub fn group_of(&self, id: SessionId) -> Option<&str> {
        self.groups.get(&id).map(String::as_str)
    }

    /// Assign `id` to a named `group`, or clear its membership with `None`. An
    /// empty label also clears, so no unnameable group can be created.
    pub fn assign_group(&mut self, id: SessionId, group: Option<String>) {
        match group.filter(|g| !g.is_empty()) {
            Some(g) => {
                self.groups.insert(id, g);
            }
            None => {
                self.groups.remove(&id);
            }
        }
    }

    /// Cycle `id`'s label one step through [`GROUP_RING`]: ungrouped → the
    /// first named group → … → the last → ungrouped again (the badge action).
    pub fn cycle_group(&mut self, id: SessionId) {
        let next = self.group_of(id).map_or(Some(GROUP_RING[0]), |cur| {
            GROUP_RING
                .iter()
                .position(|g| *g == cur)
                .and_then(|i| GROUP_RING.get(i + 1).copied())
        });
        self.assign_group(id, next.map(str::to_owned));
    }

    /// Whether `id` is in the live broadcasting set: it wears the indicator
    /// and — unless it is the focused pane — receives the fan-out. In
    /// [`Broadcast::Group`] a pane broadcasts only when it shares the focused
    /// pane's group (so an ungrouped focus broadcasts to nobody).
    #[must_use]
    pub fn is_broadcasting(&self, id: SessionId) -> bool {
        match self.broadcast {
            Broadcast::Off => false,
            Broadcast::All => self.tree.as_ref().is_some_and(|t| t.contains(id)),
            Broadcast::Group => matches!(
                (self.groups.get(&self.focused), self.groups.get(&id)),
                (Some(focus_group), Some(id_group)) if focus_group == id_group
            ),
        }
    }

    /// The fan-out recipients of the focused pane's typing this frame: every
    /// pane in the broadcasting set **except** the focused one (which already
    /// got the keystrokes directly), in tree reading order. Empty when
    /// broadcast is off, or the selected group holds only the focused pane.
    fn broadcast_targets(&self) -> Vec<SessionId> {
        let Some(tree) = &self.tree else {
            return Vec::new();
        };
        tree.leaves()
            .into_iter()
            .filter(|id| *id != self.focused && self.is_broadcasting(*id))
            .collect()
    }

    /// Spawn a shell into the registry (not yet in the tree) from the tab's
    /// recipe.
    fn spawn_session(&mut self) -> io::Result<SessionId> {
        self.spawn_session_opts(self.spawn_opts.clone())
    }

    /// Spawn a shell into the registry from an explicit recipe (the cwd-inheriting
    /// new-terminal-here path reuses this with an overridden cwd).
    fn spawn_session_opts(&mut self, opts: SpawnOptions) -> io::Result<SessionId> {
        let pty = LocalPty::spawn(opts)?;
        let id = SessionId(self.next_id);
        self.next_id += 1;
        self.sessions.insert(id, TerminalWidget::new(pty));
        Ok(id)
    }

    /// Split the focused pane in `dir` with a fresh shell from the tab's recipe;
    /// the new pane takes focus (Terminator behaviour).
    fn split_focused(&mut self, dir: SplitDir) {
        self.split_at(self.focused, dir, self.spawn_opts.clone());
    }

    /// Split `at`'s pane in `dir` with a fresh shell spawned from `opts`; the new
    /// pane takes focus. A spawn failure leaves the tree as-is and raises the
    /// error chip. The one spawn+split path both the keyboard split and the
    /// TERM-15 new-terminal-here reuse.
    fn split_at(&mut self, at: SessionId, dir: SplitDir, opts: SpawnOptions) {
        if self.tree.is_none() {
            return;
        }
        let new = match self.spawn_session_opts(opts) {
            Ok(id) => id,
            Err(err) => {
                self.error = Some((format!("could not start a shell: {err}"), Instant::now()));
                return;
            }
        };
        let split_ok = self
            .tree
            .as_mut()
            .is_some_and(|tree| tree.split(at, dir, new));
        if split_ok {
            self.focused = new;
            self.zoomed = None;
        } else {
            // `at` was not in the tree (defensive) — release the freshly spawned
            // shell rather than leak it.
            self.sessions.remove(&new);
        }
    }

    /// A local pane's live cwd, read from `/proc/<pid>/cwd` — the same source
    /// [`Self::capture_spec`] uses. `None` for a remote pane or a gone pid.
    fn pane_cwd(&self, id: SessionId) -> Option<std::path::PathBuf> {
        self.sessions
            .get(&id)
            .and_then(TerminalWidget::local_pty)
            .and_then(|pty| cwd_of_pid(pty.child_pid()))
    }

    /// Drain each pane's pending TERM-15 **new-terminal-here** request and split
    /// it — a fresh shell beside the source pane inheriting its cwd (the TERM-4/5
    /// spawn reused). A vertical split (side-by-side), Terminator's default
    /// "new terminal" placement.
    fn drain_new_terminal_requests(&mut self, lay: &Layout) {
        let requests: Vec<SessionId> = lay
            .leaves
            .iter()
            .filter(|(sid, _)| {
                self.sessions
                    .get_mut(sid)
                    .is_some_and(TerminalWidget::take_new_terminal_here)
            })
            .map(|(sid, _)| *sid)
            .collect();
        for at in requests {
            let opts = SpawnOptions {
                cwd: self.pane_cwd(at),
                ..self.spawn_opts.clone()
            };
            self.split_at(at, SplitDir::V, opts);
        }
    }

    /// Close `id`'s pane: the tree collapses, the session drops (SIGHUP +
    /// child reap), and focus falls back to the collapsed sibling.
    fn close_session(&mut self, id: SessionId) {
        let Some(tree) = self.tree.take() else {
            return;
        };
        let fallback = sibling_first_leaf(&tree, id);
        let (rest, removed) = tree.close(id);
        self.tree = rest;
        if !removed {
            return;
        }
        drop(self.sessions.remove(&id));
        self.pane_ids.remove(&id);
        self.groups.remove(&id);
        if self.zoomed == Some(id) {
            self.zoomed = None;
        }
        if self.focused == id {
            if let Some(next) = fallback {
                self.focused = next;
            }
        }
    }

    /// Maximize the focused pane, or restore the tiling.
    fn toggle_zoom(&mut self) {
        if self.zoomed.take().is_some() {
            return;
        }
        let focused_live = self
            .tree
            .as_ref()
            .is_some_and(|tree| tree.contains(self.focused));
        if focused_live && self.sessions.len() > 1 {
            self.zoomed = Some(self.focused);
        }
    }

    /// Move focus geometrically; navigating restores a zoomed pane first
    /// (Terminator behaviour — the hidden neighbours become reachable again).
    fn focus_dir(&mut self, dir: NavDir) {
        self.zoomed = None;
        if let Some(tree) = &self.tree {
            if let Some(next) = navigate(tree, self.focused, dir) {
                self.focused = next;
            }
        }
    }

    /// Render one frame: reap exited shells, lay the tree out, mount every
    /// leaf's widget, run divider drags, Alt-drag rearrange, and the focus
    /// discipline.
    pub fn show(&mut self, ui: &mut Ui) {
        self.reap_ended();
        let full = ui.available_rect_before_wrap();
        if let Some(z) = self.zoomed {
            if !self.tree.as_ref().is_some_and(|tree| tree.contains(z)) {
                self.zoomed = None;
            }
        }
        let lay = match (&self.tree, self.zoomed) {
            (None, _) => return,
            (Some(_), Some(z)) => Layout {
                leaves: vec![(z, full)],
                dividers: Vec::new(),
            },
            (Some(tree), None) => layout(tree, full),
        };

        self.prefocus(ui);
        let responses = self.show_panes(ui, &lay);
        // TERM-15: any pane whose context menu chose "new terminal here" splits
        // now, inheriting that pane's cwd (the TERM-4/5 spawn reused).
        self.drain_new_terminal_requests(&lay);
        // Fan the focused pane's just-typed bytes to the broadcasting set,
        // using the focus that held the keyboard *this* frame (before the
        // click reconcile below can move it).
        self.fan_out_broadcast();
        self.show_dividers(ui, &lay);
        self.show_pane_drag(ui, &lay);
        self.paint_broadcast_indicators(ui, &lay);
        self.paint_focus_ring(ui, &lay);
        self.show_group_badges(ui, &lay);
        self.show_broadcast_control(ui, full);
        self.paint_chips(ui, full);
        self.reconcile_focus(&responses);
    }

    /// Replay the focused pane's just-typed bytes into every other pane of the
    /// broadcasting set, each through its own [`TerminalWidget::feed_broadcast`]
    /// → [`LocalPty`] write path (§6 glue — no PTY write is re-implemented, and
    /// the split-tree leaf enumeration is reused via [`Self::broadcast_targets`]).
    /// A no-op when broadcast is off, the set is a lone pane, or nothing typed.
    fn fan_out_broadcast(&mut self) {
        if self.broadcast == Broadcast::Off {
            return;
        }
        let targets = self.broadcast_targets();
        if targets.is_empty() {
            return;
        }
        let Some(bytes) = self
            .sessions
            .get_mut(&self.focused)
            .map(TerminalWidget::take_input_echo)
        else {
            return;
        };
        if bytes.is_empty() {
            return;
        }
        for id in targets {
            if let Some(widget) = self.sessions.get_mut(&id) {
                widget.feed_broadcast(&bytes);
            }
        }
    }

    /// The [`Style::WARN`] indicator border every pane in the live broadcasting
    /// set wears (§7 — the visible cue for which panes receive fan-out), nested
    /// just inside the [`Style::ACCENT`] focus ring so a focused broadcasting
    /// pane shows both.
    fn paint_broadcast_indicators(&self, ui: &Ui, lay: &Layout) {
        if self.broadcast == Broadcast::Off {
            return;
        }
        for (sid, rect) in &lay.leaves {
            if self.is_broadcasting(*sid) {
                ui.painter().rect_stroke(
                    rect.shrink(1.0),
                    0.0,
                    Stroke::new(BROADCAST_BORDER_PX, Style::WARN),
                    StrokeKind::Inside,
                );
            }
        }
    }

    /// The clickable broadcast-mode chip (bottom-left, only with more than one
    /// pane): shows the current routing and cycles `Off → All → Group → Off` on
    /// click — the UI half of the keybind toggle. WARN while fan-out is live,
    /// dim when off.
    fn show_broadcast_control(&mut self, ui: &Ui, full: Rect) {
        if self.sessions.len() <= 1 {
            return;
        }
        let label = match self.broadcast {
            Broadcast::Off => "cast: off".to_owned(),
            Broadcast::All => "cast: all".to_owned(),
            Broadcast::Group => self.group_of(self.focused).map_or_else(
                || "cast: grp \u{2014}".to_owned(),
                |g| format!("cast: grp {g}"),
            ),
        };
        let accent = if self.broadcast == Broadcast::Off {
            Style::TEXT_DIM
        } else {
            Style::WARN
        };
        let at = pos2(full.min.x + Style::SP_S, full.max.y - Style::SP_S);
        let resp = Self::chip_button(
            ui,
            ui.id().with("term-broadcast"),
            at,
            Align2::LEFT_BOTTOM,
            &label,
            accent,
        );
        if resp.clicked() {
            self.set_broadcast(self.broadcast.next());
        }
    }

    /// Per-pane group badges (bottom-right, only with more than one pane): a
    /// small clickable tag showing the pane's group label (or `+` when
    /// ungrouped) that cycles it through [`GROUP_RING`] — the UI to
    /// assign/label panes into a named broadcast group. WARN while the pane is
    /// in the live broadcasting set.
    fn show_group_badges(&mut self, ui: &Ui, lay: &Layout) {
        if self.sessions.len() <= 1 {
            return;
        }
        let mut to_cycle = None;
        for (sid, rect) in &lay.leaves {
            let label = self
                .group_of(*sid)
                .map_or_else(|| "+".to_owned(), str::to_owned);
            let color = if self.is_broadcasting(*sid) {
                Style::WARN
            } else {
                Style::TEXT_DIM
            };
            let at = pos2(rect.max.x - Style::SP_XS, rect.max.y - Style::SP_XS);
            let resp = Self::chip_button(
                ui,
                ui.id().with(("term-group", sid.0)),
                at,
                Align2::RIGHT_BOTTOM,
                &label,
                color,
            );
            if resp.clicked() {
                to_cycle = Some(*sid);
            }
        }
        if let Some(sid) = to_cycle {
            self.cycle_group(sid);
        }
    }

    /// A small interactive chip (the [`crate::widget::chip`] look, but hit-
    /// tested): a SURFACE plate + hairline that lights `accent` on hover, its
    /// `id`/rect anchored by `anchor` at `at`. Registered after the panes, so
    /// it claims the pointer within its rect (egui's later-interact rule) and a
    /// click on it never doubles as terminal input.
    fn chip_button(
        ui: &Ui,
        id: Id,
        at: Pos2,
        anchor: Align2,
        label: &str,
        accent: mde_egui::egui::Color32,
    ) -> Response {
        let font = FontId::monospace(Style::SMALL);
        let galley = ui
            .painter()
            .layout_no_wrap(label.to_owned(), font, Style::TEXT);
        let rect = anchor.anchor_size(at, galley.size() + Vec2::splat(2.0 * Style::SP_XS));
        let resp = ui
            .interact(rect, id, Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        let painter = ui.painter();
        painter.rect_filled(rect, Style::RADIUS, Style::SURFACE);
        painter.rect_stroke(
            rect,
            Style::RADIUS,
            Stroke::new(
                1.0,
                if resp.hovered() {
                    accent
                } else {
                    Style::BORDER
                },
            ),
            StrokeKind::Inside,
        );
        painter.galley(rect.min + Vec2::splat(Style::SP_XS), galley, accent);
        resp
    }

    /// Terminator's close-on-exit: a shell that ended closes its pane (the
    /// split collapses exactly as an explicit close would).
    fn reap_ended(&mut self) {
        let ended: Vec<SessionId> = self
            .sessions
            .iter()
            .filter(|(_, widget)| widget.is_output_closed())
            .map(|(id, _)| *id)
            .collect();
        for id in ended {
            self.close_session(id);
        }
    }

    /// Hand the keyboard to the tree-focused pane *before* widgets render, so
    /// the frame after a close/switch feeds no keys to the wrong shell. Uses
    /// last frame's widget ids; egui drops focus of despawned widgets, which
    /// is exactly the "none focused" case this fills.
    fn prefocus(&self, ui: &Ui) {
        if ui.memory(|m| m.focused().is_none()) {
            if let Some(id) = self.pane_ids.get(&self.focused) {
                ui.memory_mut(|m| m.request_focus(*id));
            }
        }
    }

    /// Mount every laid-out leaf's widget in its own child `Ui`.
    fn show_panes(&mut self, ui: &mut Ui, lay: &Layout) -> Vec<(SessionId, Response)> {
        let mut out = Vec::with_capacity(lay.leaves.len());
        for (sid, rect) in &lay.leaves {
            let Some(widget) = self.sessions.get_mut(sid) else {
                debug_assert!(false, "tree leaf {sid:?} missing from the session registry");
                continue;
            };
            // Push the surface appearance (scheme + font + cursor) into the pane
            // before it renders (TERM-11), plus the shared selection context menu
            // (TERM-15), so a config change reaches every live shell.
            widget.apply_appearance(&self.appearance);
            widget.apply_context_menu(&self.menu);
            let mut pane_ui = ui.new_child(
                UiBuilder::new()
                    .max_rect(*rect)
                    .id_salt(("term-pane", sid.0)),
            );
            out.push((*sid, widget.show(&mut pane_ui)));
        }
        out
    }

    /// Divider strips: drag adjusts the split ratio; hover/drag recolour the
    /// hairline through the `Style` tokens.
    fn show_dividers(&mut self, ui: &Ui, lay: &Layout) {
        for div in &lay.dividers {
            let (hit, icon, line_size) = match div.dir {
                SplitDir::V => (
                    div.rect.expand2(vec2(DIVIDER_HIT_SLOP, 0.0)),
                    CursorIcon::ResizeHorizontal,
                    vec2(1.0, div.rect.height()),
                ),
                SplitDir::H => (
                    div.rect.expand2(vec2(0.0, DIVIDER_HIT_SLOP)),
                    CursorIcon::ResizeVertical,
                    vec2(div.rect.width(), 1.0),
                ),
            };
            let resp = ui
                .interact(hit, ui.id().with(("splitter", div.path)), Sense::drag())
                .on_hover_cursor(icon);
            if resp.dragged() {
                if let (Some(pos), Some(tree)) = (resp.interact_pointer_pos(), self.tree.as_mut()) {
                    if let Some(ratio) = tree.ratio_mut(div.path) {
                        *ratio = pointer_ratio(div, pos);
                    }
                }
            }
            let color = if resp.dragged() {
                Style::ACCENT
            } else if resp.hovered() {
                Style::ACCENT_HI
            } else {
                Style::BORDER
            };
            ui.painter().rect_filled(
                Rect::from_center_size(div.rect.center(), line_size),
                0.0,
                color,
            );
        }
    }

    /// Alt-drag rearrange: an Alt-press on a pane picks it up; the pointer's
    /// drop half on another pane previews (token blend) and, on release,
    /// reparents the leaf there. Releasing anywhere else drops the drag.
    fn show_pane_drag(&mut self, ui: &Ui, lay: &Layout) {
        let alt = ui.input(|i| i.modifiers.alt);
        if alt || self.drag.is_some() {
            for (sid, rect) in &lay.leaves {
                let resp = ui
                    .interact(*rect, ui.id().with(("pane-move", sid.0)), Sense::drag())
                    .on_hover_cursor(CursorIcon::Grab);
                if resp.drag_started() {
                    self.drag = Some(*sid);
                    self.focused = *sid;
                }
            }
        }
        let Some(src) = self.drag else { return };
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        let (pointer, released) = ui.input(|i| {
            (
                i.pointer.latest_pos(),
                i.pointer.primary_released() || !i.pointer.any_down(),
            )
        });

        // The picked-up pane wears its own marker while in flight.
        if let Some((_, src_rect)) = lay.leaves.iter().find(|(sid, _)| *sid == src) {
            ui.painter().rect_stroke(
                *src_rect,
                0.0,
                Stroke::new(1.0, Style::ACCENT_HI),
                StrokeKind::Inside,
            );
        }

        let target = pointer.and_then(|p| {
            lay.leaves
                .iter()
                .find(|(sid, rect)| *sid != src && rect.contains(p))
                .map(|(sid, rect)| (*sid, *rect, p))
        });
        if let Some((tid, trect, p)) = target {
            let (dir, first, preview) = drop_zone(trect, p);
            ui.painter()
                .rect_filled(preview, Style::RADIUS, Style::ACCENT.gamma_multiply(0.25));
            ui.painter().rect_stroke(
                preview,
                Style::RADIUS,
                Stroke::new(1.0, Style::ACCENT),
                StrokeKind::Inside,
            );
            if released {
                if let Some(tree) = self.tree.as_mut() {
                    if tree.reparent(src, tid, dir, first).is_ok() {
                        self.focused = src;
                        self.zoomed = None;
                    }
                }
            }
        }
        if released {
            self.drag = None;
        }
    }

    /// The hairline focus ring — only once there is more than one pane to
    /// tell apart (a lone pane stays full-bleed, as in TERM-3).
    fn paint_focus_ring(&self, ui: &Ui, lay: &Layout) {
        if self.sessions.len() <= 1 {
            return;
        }
        if let Some((_, rect)) = lay.leaves.iter().find(|(sid, _)| *sid == self.focused) {
            ui.painter().rect_stroke(
                *rect,
                0.0,
                Stroke::new(1.0, Style::ACCENT),
                StrokeKind::Inside,
            );
        }
    }

    /// State chips: the zoom marker and the transient spawn-failure notice.
    fn paint_chips(&mut self, ui: &Ui, full: Rect) {
        if self.zoomed.is_some() {
            chip(
                ui.painter(),
                pos2(full.center().x, full.min.y + Style::SP_S),
                Align2::CENTER_TOP,
                "zoomed",
                Style::TEXT_DIM,
            );
        }
        if let Some((msg, since)) = &self.error {
            if since.elapsed() < ERROR_TTL {
                chip(
                    ui.painter(),
                    pos2(full.center().x, full.max.y - Style::SP_S),
                    Align2::CENTER_BOTTOM,
                    msg,
                    Style::DANGER,
                );
            } else {
                self.error = None;
            }
        }
    }

    /// Focus follows: a click/drag on a pane focuses it; otherwise exactly
    /// the tree-focused pane holds the egui keyboard. Also remembers each
    /// pane's widget id for next frame's [`Self::prefocus`].
    fn reconcile_focus(&mut self, responses: &[(SessionId, Response)]) {
        for (sid, resp) in responses {
            if resp.clicked() || resp.drag_started() {
                self.focused = *sid;
            }
        }
        if let Some((_, resp)) = responses.iter().find(|(sid, _)| *sid == self.focused) {
            if !resp.has_focus() {
                resp.request_focus();
            }
        }
        self.pane_ids = responses
            .iter()
            .map(|(sid, resp)| (*sid, resp.id))
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use mde_egui::egui::{self, PointerButton, RawInput, Vec2};

    use super::*;

    // ── tree fixtures ───────────────────────────────────────────────────────

    fn sid(n: u64) -> SessionId {
        SessionId(n)
    }

    fn l(n: u64) -> Pane {
        Pane::Leaf(sid(n))
    }

    fn s(dir: SplitDir, ratio: f32, a: Pane, b: Pane) -> Pane {
        Pane::Split {
            dir,
            ratio,
            a: Box::new(a),
            b: Box::new(b),
        }
    }

    /// The canonical 3-pane fixture: `1 | 2` (60/40) stacked over `3`.
    fn three() -> Pane {
        s(SplitDir::H, 0.5, s(SplitDir::V, 0.6, l(1), l(2)), l(3))
    }

    // ── split ───────────────────────────────────────────────────────────────

    #[test]
    fn split_replaces_the_leaf_with_a_half_ratio_split() {
        let mut tree = l(1);
        assert!(tree.split(sid(1), SplitDir::V, sid(2)));
        assert_eq!(tree, s(SplitDir::V, 0.5, l(1), l(2)));

        // Split the right child horizontally — nesting begins.
        assert!(tree.split(sid(2), SplitDir::H, sid(3)));
        assert_eq!(
            tree,
            s(SplitDir::V, 0.5, l(1), s(SplitDir::H, 0.5, l(2), l(3)))
        );
    }

    #[test]
    fn split_nests_to_arbitrary_depth() {
        let mut tree = l(0);
        for n in 1..=40 {
            let dir = if n % 2 == 0 { SplitDir::H } else { SplitDir::V };
            assert!(tree.split(sid(n - 1), dir, sid(n)), "split {n}");
        }
        let expect: Vec<SessionId> = (0..=40).map(sid).collect();
        assert_eq!(tree.leaves(), expect, "reading order holds at depth 40");
        assert!(tree.contains(sid(0)) && tree.contains(sid(40)));
        assert_eq!(tree.first_leaf(), sid(0));
    }

    #[test]
    fn split_of_a_missing_leaf_is_rejected() {
        let mut tree = three();
        let before = tree.clone();
        assert!(!tree.split(sid(99), SplitDir::V, sid(4)));
        assert_eq!(tree, before, "tree untouched");
    }

    // ── close ───────────────────────────────────────────────────────────────

    #[test]
    fn close_collapses_the_parent_split_to_the_sibling() {
        // Closing 1 collapses the inner V split to leaf 2.
        let (rest, removed) = three().close(sid(1));
        assert!(removed);
        assert_eq!(rest, Some(s(SplitDir::H, 0.5, l(2), l(3))));

        // Closing 3 collapses the root to the inner split.
        let (rest, removed) = three().close(sid(3));
        assert!(removed);
        assert_eq!(rest, Some(s(SplitDir::V, 0.6, l(1), l(2))));
    }

    #[test]
    fn close_of_the_root_leaf_empties_the_tree() {
        let (rest, removed) = l(7).close(sid(7));
        assert!(removed);
        assert_eq!(rest, None);
    }

    #[test]
    fn close_of_a_missing_leaf_leaves_the_tree_untouched() {
        let (rest, removed) = three().close(sid(99));
        assert!(!removed);
        assert_eq!(rest, Some(three()));
    }

    // ── reparent (drag-rearrange) ───────────────────────────────────────────

    #[test]
    fn reparent_moves_a_leaf_beside_the_target() {
        // Drag 1 onto 3's bottom half: detach 1 (inner split collapses to 2),
        // then re-split at 3 with 1 below.
        let mut tree = three();
        assert_eq!(tree.reparent(sid(1), sid(3), SplitDir::H, false), Ok(()));
        assert_eq!(
            tree,
            s(SplitDir::H, 0.5, l(2), s(SplitDir::H, 0.5, l(3), l(1)))
        );

        // `first` puts the moved leaf in the a (top/left) slot instead.
        let mut tree = three();
        assert_eq!(tree.reparent(sid(3), sid(2), SplitDir::V, true), Ok(()));
        assert_eq!(
            tree,
            s(SplitDir::V, 0.6, l(1), s(SplitDir::V, 0.5, l(3), l(2)))
        );
    }

    #[test]
    fn reparent_into_its_own_subtree_is_rejected() {
        let mut tree = three();
        let before = tree.clone();
        assert_eq!(
            tree.reparent(sid(2), sid(2), SplitDir::V, false),
            Err(ReparentError::IntoOwnSubtree)
        );
        assert_eq!(tree, before, "tree untouched on rejection");
    }

    #[test]
    fn reparent_of_missing_leaf_or_target_is_rejected() {
        let mut tree = three();
        let before = tree.clone();
        assert_eq!(
            tree.reparent(sid(99), sid(2), SplitDir::V, false),
            Err(ReparentError::LeafNotFound)
        );
        assert_eq!(
            tree.reparent(sid(2), sid(99), SplitDir::V, false),
            Err(ReparentError::TargetNotFound)
        );
        assert_eq!(tree, before, "tree untouched on both errors");
    }

    // ── replace / find ──────────────────────────────────────────────────────

    #[test]
    fn replace_swaps_a_leaf_for_a_subtree() {
        let mut tree = three();
        let sub = s(SplitDir::V, 0.5, l(8), l(9));
        assert!(tree.replace(sid(3), sub.clone()).is_none());
        assert_eq!(tree.leaves(), vec![sid(1), sid(2), sid(8), sid(9)]);

        // A missing target hands the subtree back untouched.
        assert_eq!(tree.replace(sid(99), sub.clone()), Some(sub));
    }

    // ── ratio clamps ────────────────────────────────────────────────────────

    #[test]
    fn ratio_clamps_hold() {
        assert!((clamp_ratio(0.5) - 0.5).abs() < f32::EPSILON);
        assert!((clamp_ratio(-3.0) - MIN_RATIO).abs() < f32::EPSILON);
        assert!((clamp_ratio(42.0) - (1.0 - MIN_RATIO)).abs() < f32::EPSILON);
        assert!((clamp_ratio(f32::NAN) - 0.5).abs() < f32::EPSILON);
        assert!((clamp_ratio(f32::INFINITY) - 0.5).abs() < f32::EPSILON);

        // The pixel minimum tightens further: 48px of a 400px span is 0.12.
        assert!((effective_ratio(0.01, 400.0, 48.0) - 0.12).abs() < 1e-6);
        assert!((effective_ratio(0.99, 400.0, 48.0) - 0.88).abs() < 1e-6);
        // A span too small for two minimum panes falls back to an even cut.
        assert!((effective_ratio(0.2, 40.0, 48.0) - 0.5).abs() < f32::EPSILON);
        assert!((effective_ratio(0.2, 0.0, 48.0) - 0.5).abs() < f32::EPSILON);
    }

    // ── layout ──────────────────────────────────────────────────────────────

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect::from_min_max(pos2(x0, y0), pos2(x1, y1))
    }

    fn approx(a: Rect, b: Rect) -> bool {
        (a.min - b.min).length() < 0.01 && (a.max - b.max).length() < 0.01
    }

    #[test]
    fn layout_tiles_the_rect_by_ratio_with_divider_gaps() {
        // V split at 0.25 of a 404-wide rect: inner 400 → a=100, divider 4.
        let tree = s(SplitDir::V, 0.25, l(1), l(2));
        let lay = layout(&tree, rect(0.0, 0.0, 404.0, 100.0));
        assert_eq!(lay.leaves.len(), 2);
        assert!(approx(lay.leaves[0].1, rect(0.0, 0.0, 100.0, 100.0)));
        assert!(approx(lay.leaves[1].1, rect(104.0, 0.0, 404.0, 100.0)));
        assert_eq!(lay.dividers.len(), 1);
        assert!(approx(lay.dividers[0].rect, rect(100.0, 0.0, 104.0, 100.0)));
        assert_eq!(lay.dividers[0].dir, SplitDir::V);
        assert_eq!(lay.dividers[0].path, NodePath::ROOT);

        // Nested: the fixture in an 804×504 rect. Root H at 0.5 → inner 500,
        // top 250; inner V at 0.6 → inner 800, left 480.
        let lay = layout(&three(), rect(0.0, 0.0, 804.0, 504.0));
        assert_eq!(
            lay.leaves.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![sid(1), sid(2), sid(3)]
        );
        assert!(approx(lay.leaves[0].1, rect(0.0, 0.0, 480.0, 250.0)));
        assert!(approx(lay.leaves[1].1, rect(484.0, 0.0, 804.0, 250.0)));
        assert!(approx(lay.leaves[2].1, rect(0.0, 254.0, 804.0, 504.0)));
        assert_eq!(lay.dividers.len(), 2);
        assert_eq!(lay.dividers[0].path, NodePath::ROOT);
        assert_eq!(lay.dividers[1].path, NodePath::ROOT.child_a());
    }

    #[test]
    fn layout_respects_the_min_pane_clamp() {
        // Ratio 0.01 would starve child a; the layout holds it at 48px.
        let tree = s(SplitDir::H, 0.01, l(1), l(2));
        let lay = layout(&tree, rect(0.0, 0.0, 100.0, 404.0));
        assert!((lay.leaves[0].1.height() - 48.0).abs() < 0.01);
    }

    #[test]
    fn ratio_mut_addresses_splits_by_path() {
        let mut tree = three();
        // The root split.
        *tree.ratio_mut(NodePath::ROOT).expect("root split") = 0.7;
        // The inner split sits on the root's `a` branch.
        let inner = NodePath::ROOT.child_a();
        *tree.ratio_mut(inner).expect("inner split") = 0.3;
        assert_eq!(
            tree,
            s(SplitDir::H, 0.7, s(SplitDir::V, 0.3, l(1), l(2)), l(3))
        );
        // Paths that walk into a leaf, or end on one, address nothing.
        assert!(tree.ratio_mut(NodePath::ROOT.child_b()).is_none());
        assert!(tree
            .ratio_mut(NodePath::ROOT.child_a().child_a().child_a())
            .is_none());
    }

    #[test]
    fn pointer_ratio_maps_the_drag_position_into_the_span() {
        let div = Divider {
            path: NodePath::ROOT,
            dir: SplitDir::V,
            rect: rect(100.0, 0.0, 104.0, 100.0),
            span: rect(0.0, 0.0, 404.0, 100.0),
        };
        // Pointer at x=202 → a-width 200 of inner 400 → ratio 0.5.
        assert!((pointer_ratio(&div, pos2(202.0, 50.0)) - 0.5).abs() < 1e-6);
        // Off both ends the stored clamp holds.
        assert!((pointer_ratio(&div, pos2(-500.0, 0.0)) - MIN_RATIO).abs() < f32::EPSILON);
        assert!((pointer_ratio(&div, pos2(9e3, 0.0)) - (1.0 - MIN_RATIO)).abs() < f32::EPSILON);
    }

    // ── navigation + focus fallback ─────────────────────────────────────────

    #[test]
    fn navigate_moves_focus_geometrically() {
        let tree = three(); // 1 | 2 on top (60/40), 3 across the bottom.
        assert_eq!(navigate(&tree, sid(1), NavDir::Right), Some(sid(2)));
        assert_eq!(navigate(&tree, sid(2), NavDir::Left), Some(sid(1)));
        assert_eq!(navigate(&tree, sid(1), NavDir::Down), Some(sid(3)));
        assert_eq!(navigate(&tree, sid(2), NavDir::Down), Some(sid(3)));
        // From 3 upward, 1 wins on the larger horizontal overlap (0.6 vs 0.4).
        assert_eq!(navigate(&tree, sid(3), NavDir::Up), Some(sid(1)));
        // Surface edges dead-end honestly.
        assert_eq!(navigate(&tree, sid(1), NavDir::Up), None);
        assert_eq!(navigate(&tree, sid(1), NavDir::Left), None);
        assert_eq!(navigate(&tree, sid(2), NavDir::Right), None);
        assert_eq!(navigate(&tree, sid(3), NavDir::Down), None);
        // A missing origin navigates nowhere.
        assert_eq!(navigate(&tree, sid(99), NavDir::Left), None);
    }

    #[test]
    fn sibling_first_leaf_finds_the_focus_fallback() {
        let tree = three();
        // 1's sibling is leaf 2; 2's is leaf 1.
        assert_eq!(sibling_first_leaf(&tree, sid(1)), Some(sid(2)));
        assert_eq!(sibling_first_leaf(&tree, sid(2)), Some(sid(1)));
        // 3's sibling is the whole inner split — its first leaf is 1.
        assert_eq!(sibling_first_leaf(&tree, sid(3)), Some(sid(1)));
        assert_eq!(sibling_first_leaf(&tree, sid(99)), None);
        assert_eq!(
            sibling_first_leaf(&l(1), sid(1)),
            None,
            "root has no sibling"
        );
    }

    // ── drop zones ──────────────────────────────────────────────────────────

    #[test]
    fn drop_zone_picks_the_nearest_edge() {
        let r = rect(0.0, 0.0, 100.0, 100.0);
        let (dir, first, preview) = drop_zone(r, pos2(10.0, 50.0));
        assert_eq!((dir, first), (SplitDir::V, true));
        assert!(approx(preview, rect(0.0, 0.0, 50.0, 100.0)));

        let (dir, first, preview) = drop_zone(r, pos2(90.0, 50.0));
        assert_eq!((dir, first), (SplitDir::V, false));
        assert!(approx(preview, rect(50.0, 0.0, 100.0, 100.0)));

        let (dir, first, preview) = drop_zone(r, pos2(50.0, 5.0));
        assert_eq!((dir, first), (SplitDir::H, true));
        assert!(approx(preview, rect(0.0, 0.0, 100.0, 50.0)));

        let (dir, first, preview) = drop_zone(r, pos2(50.0, 95.0));
        assert_eq!((dir, first), (SplitDir::H, false));
        assert!(approx(preview, rect(0.0, 50.0, 100.0, 100.0)));
    }

    // ── keyboard chords ─────────────────────────────────────────────────────

    fn key_event(key: Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn commands_consume_their_chords() {
        let cs = Modifiers::CTRL | Modifiers::SHIFT;
        let ctx = Context::default();
        let raw = RawInput {
            modifiers: cs,
            events: vec![
                key_event(Key::O, cs),
                key_event(Key::ArrowLeft, Modifiers::ALT),
                // winit's fold of Ctrl+Shift+X — arrives as Cut with Shift held.
                Event::Cut,
            ],
            ..RawInput::default()
        };
        let _ = ctx.run(raw, |ctx| {
            let cmds = consume_commands(ctx);
            assert_eq!(
                cmds,
                vec![
                    Command::Split(SplitDir::H),
                    Command::ToggleZoom,
                    Command::Focus(NavDir::Left),
                ]
            );
            // Everything claimed was consumed — nothing leaks to a shell.
            ctx.input(|i| assert!(i.events.is_empty(), "events consumed: {:?}", i.events));
        });

        // Plain Ctrl+O (no Shift) and an unshifted Cut stay the terminal's.
        let raw = RawInput {
            modifiers: Modifiers::CTRL,
            events: vec![key_event(Key::O, Modifiers::CTRL), Event::Cut],
            ..RawInput::default()
        };
        let _ = ctx.run(raw, |ctx| {
            assert!(consume_commands(ctx).is_empty());
            ctx.input(|i| assert_eq!(i.events.len(), 2, "both left for the widget"));
        });
    }

    // ── the live multiplexer, headless over real PTYs ───────────────────────

    fn sh_opts() -> SpawnOptions {
        SpawnOptions {
            shell: Some("/bin/sh".to_owned()),
            ..SpawnOptions::default()
        }
    }

    fn pid_exists(pid: u32) -> bool {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }

    /// One headless frame: 900×500 surface, the given events/modifiers.
    fn frame(ctx: &Context, term: &mut SplitTerminal, events: Vec<Event>, modifiers: Modifiers) {
        let raw = RawInput {
            screen_rect: Some(rect(0.0, 0.0, 900.0, 500.0)),
            modifiers,
            events,
            ..RawInput::default()
        };
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(Style::BG))
                .show(ctx, |ui| term.show(ui));
        });
    }

    fn settle(ctx: &Context, term: &mut SplitTerminal, frames: usize) {
        for _ in 0..frames {
            frame(ctx, term, Vec::new(), Modifiers::NONE);
        }
    }

    fn test_ctx() -> Context {
        let ctx = Context::default();
        Style::install(&ctx);
        ctx
    }

    fn cols_of(term: &SplitTerminal, id: SessionId) -> usize {
        term.sessions[&id].with_terminal(crate::engine::Terminal::cols)
    }

    fn rows_of(term: &SplitTerminal, id: SessionId) -> usize {
        term.sessions[&id].with_terminal(crate::engine::Terminal::rows)
    }

    #[test]
    fn split_terminal_tiles_real_sessions() {
        let ctx = test_ctx();
        let mut term = SplitTerminal::new(sh_opts()).expect("first shell");
        let a = term.focused_session();
        term.apply(Command::Split(SplitDir::V));
        let b = term.focused_session();
        term.apply(Command::Split(SplitDir::H));
        let c = term.focused_session();
        assert_eq!(term.session_count(), 3, "three real shells in the registry");
        assert_ne!(a, b);
        assert_ne!(b, c);

        settle(&ctx, &mut term, 2);

        // Each pane's engine grid was sized from ITS rect (§7: the kernel-side
        // TIOCSWINSZ path runs through TERM-3's widget per pane): A fills the
        // left half full-height; B and C stack in the right half.
        assert_eq!(
            term.tree,
            Some(s(
                SplitDir::V,
                0.5,
                l(a.0),
                s(SplitDir::H, 0.5, l(b.0), l(c.0))
            ))
        );
        assert_eq!(cols_of(&term, a), cols_of(&term, b), "equal half-widths");
        assert!(
            rows_of(&term, b) < rows_of(&term, a),
            "B is half of A's height: {} vs {}",
            rows_of(&term, b),
            rows_of(&term, a)
        );
    }

    #[test]
    fn new_terminal_here_splits_inheriting_the_pane_cwd() {
        // TERM-15: the context-menu "new terminal here" splits the pane with a
        // fresh shell that inherits its cwd (the TERM-4/5 spawn reused). Start a
        // shell in a known dir, flag the pane as the menu would, and render — one
        // frame drains the request and splits.
        let ctx = test_ctx();
        let mut term = SplitTerminal::new(SpawnOptions {
            cwd: Some(std::path::PathBuf::from("/tmp")),
            ..sh_opts()
        })
        .expect("first shell");
        let src = term.focused_session();
        settle(&ctx, &mut term, 2);
        assert_eq!(term.session_count(), 1, "one shell to start");

        term.sessions
            .get_mut(&src)
            .expect("source pane")
            .request_new_terminal_here();
        frame(&ctx, &mut term, Vec::new(), Modifiers::NONE);
        settle(&ctx, &mut term, 2);

        assert_eq!(term.session_count(), 2, "a new pane split in");
        let new = term.focused_session();
        assert_ne!(new, src, "the new pane took focus");
        // The new shell inherited the source pane's cwd, read live from /proc
        // (§7: the real spawn, not a stored flag).
        let src_cwd = term.pane_cwd(src);
        assert!(src_cwd.is_some(), "the source pane's cwd is readable");
        assert_eq!(term.pane_cwd(new), src_cwd, "new pane inherited the cwd");
    }

    #[test]
    fn set_appearance_reaches_every_pane_on_the_next_frame() {
        use crate::appearance::Appearance;
        use crate::palette::Palette;
        use crate::presets::Preset;

        // TERM-11: a scheme chosen in the picker (set on the surface) must reach
        // every live pane's renderer. Two panes, both default; set Nord, render a
        // frame, and both panes now carry the Nord palette (§7 — the real render
        // path applied it, not a stored flag).
        let ctx = test_ctx();
        let mut term = SplitTerminal::new(sh_opts()).expect("first shell");
        term.apply(Command::Split(SplitDir::V));
        settle(&ctx, &mut term, 1);
        let ids: Vec<SessionId> = term.sessions.keys().copied().collect();
        assert_eq!(ids.len(), 2);
        for id in &ids {
            assert_eq!(term.sessions[id].palette(), Palette::from_tokens());
        }

        let nord = Preset::Nord.palette();
        term.set_appearance(Appearance {
            palette: nord,
            ..Appearance::default()
        });
        settle(&ctx, &mut term, 1);
        for id in &ids {
            assert_eq!(
                term.sessions[id].palette(),
                nord,
                "the surface pushed the scheme into every pane"
            );
        }
    }

    #[test]
    fn divider_drag_adjusts_the_ratio() {
        let ctx = test_ctx();
        let mut term = SplitTerminal::new(sh_opts()).expect("first shell");
        term.apply(Command::Split(SplitDir::V));
        settle(&ctx, &mut term, 2);

        // The divider strip's centre, from the same pure layout `show` uses.
        let lay = layout(
            term.tree.as_ref().expect("tree"),
            rect(0.0, 0.0, 900.0, 500.0),
        );
        let grab = lay.dividers[0].rect.center();

        // Press on the divider, drag 120px right, release.
        frame(
            &ctx,
            &mut term,
            vec![
                Event::PointerMoved(grab),
                Event::PointerButton {
                    pos: grab,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: Modifiers::NONE,
                },
            ],
            Modifiers::NONE,
        );
        let dragged_to = grab + Vec2::new(120.0, 0.0);
        frame(
            &ctx,
            &mut term,
            vec![Event::PointerMoved(dragged_to)],
            Modifiers::NONE,
        );
        frame(
            &ctx,
            &mut term,
            vec![Event::PointerButton {
                pos: dragged_to,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            }],
            Modifiers::NONE,
        );

        let Some(Pane::Split { ratio, .. }) = term.tree else {
            unreachable!("the root is a split after one split command");
        };
        assert!(
            ratio > 0.55 && ratio < 0.75,
            "ratio followed the 120px drag: {ratio}"
        );
    }

    #[test]
    fn zoom_maximizes_and_restores() {
        let ctx = test_ctx();
        let mut term = SplitTerminal::new(sh_opts()).expect("first shell");
        term.apply(Command::Split(SplitDir::V));
        let b = term.focused_session();
        settle(&ctx, &mut term, 2);
        let half_cols = cols_of(&term, b);

        term.apply(Command::ToggleZoom);
        assert_eq!(term.zoomed, Some(b));
        settle(&ctx, &mut term, 2);
        let full_cols = cols_of(&term, b);
        assert!(
            full_cols > half_cols + 20,
            "zoomed pane spans the surface: {half_cols} -> {full_cols}"
        );

        term.apply(Command::ToggleZoom);
        assert_eq!(term.zoomed, None);
        settle(&ctx, &mut term, 2);
        assert_eq!(cols_of(&term, b), half_cols, "restored to its tile");
    }

    #[test]
    fn close_focused_collapses_reaps_and_refocuses() {
        let ctx = test_ctx();
        let mut term = SplitTerminal::new(sh_opts()).expect("first shell");
        let a = term.focused_session();
        term.apply(Command::Split(SplitDir::V));
        let b = term.focused_session();
        settle(&ctx, &mut term, 2);
        let pid_b = term.sessions[&b].local_pty().expect("local").child_pid();

        term.apply(Command::Close);
        assert_eq!(term.session_count(), 1);
        assert!(!pid_exists(pid_b), "closed pane's shell reaped");
        assert_eq!(term.focused_session(), a, "focus fell back to the sibling");
        assert!(!term.is_empty());

        // Closing the last pane empties the surface.
        let pid_a = term.sessions[&a].local_pty().expect("local").child_pid();
        term.apply(Command::Close);
        assert!(term.is_empty());
        assert_eq!(term.session_count(), 0);
        assert!(!pid_exists(pid_a));
    }

    #[test]
    fn exited_shell_auto_closes_its_pane() {
        let ctx = test_ctx();
        let mut term = SplitTerminal::new(sh_opts()).expect("first shell");
        let a = term.focused_session();
        term.apply(Command::Split(SplitDir::V));
        let b = term.focused_session();
        settle(&ctx, &mut term, 2);

        // The user types `exit` in pane B; the shell ends on its own.
        term.sessions[&b]
            .local_pty()
            .expect("local")
            .send_input(b"exit\n")
            .expect("queue exit");
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !term.sessions[&b].is_output_closed() {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for the shell to exit"
            );
            std::thread::sleep(Duration::from_millis(25));
        }

        // The next frame reaps the pane: the split collapses to A.
        settle(&ctx, &mut term, 1);
        assert_eq!(term.session_count(), 1);
        assert_eq!(term.tree, Some(l(a.0)));
        assert_eq!(term.focused_session(), a);
    }

    #[test]
    fn alt_drag_reparents_a_leaf() {
        let ctx = test_ctx();
        let mut term = SplitTerminal::new(sh_opts()).expect("first shell");
        let a = term.focused_session();
        term.apply(Command::Split(SplitDir::V));
        let b = term.focused_session();
        term.apply(Command::Split(SplitDir::H));
        let c = term.focused_session();
        settle(&ctx, &mut term, 2);

        // Tree: A | (B over C). Alt-drag A onto C's bottom half.
        let lay = layout(
            term.tree.as_ref().expect("tree"),
            rect(0.0, 0.0, 900.0, 500.0),
        );
        let a_center = lay.leaves[0].1.center();
        let c_rect = lay.leaves[2].1;
        assert_eq!(lay.leaves[0].0, a);
        assert_eq!(lay.leaves[2].0, c);
        let c_bottom = pos2(c_rect.center().x, c_rect.max.y - 5.0);

        // Hover with Alt (registers the move overlays), press, drag, release.
        frame(
            &ctx,
            &mut term,
            vec![Event::PointerMoved(a_center)],
            Modifiers::ALT,
        );
        frame(
            &ctx,
            &mut term,
            vec![Event::PointerButton {
                pos: a_center,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::ALT,
            }],
            Modifiers::ALT,
        );
        frame(
            &ctx,
            &mut term,
            vec![Event::PointerMoved(c_bottom)],
            Modifiers::ALT,
        );
        assert_eq!(term.drag, Some(a), "the pane is in flight");
        frame(
            &ctx,
            &mut term,
            vec![Event::PointerButton {
                pos: c_bottom,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::ALT,
            }],
            Modifiers::ALT,
        );

        // A detached (its old split collapsed) and re-split below C.
        assert_eq!(
            term.tree,
            Some(s(
                SplitDir::H,
                0.5,
                l(b.0),
                s(SplitDir::H, 0.5, l(c.0), l(a.0))
            ))
        );
        assert_eq!(term.drag, None);
        assert_eq!(term.focused_session(), a, "the moved pane keeps focus");
        assert_eq!(term.session_count(), 3, "no session was lost in the move");
    }

    // ── broadcast / grouped input (TERM-6) ──────────────────────────────────

    /// A pane's shell output (scrollback + viewport) joined into one string.
    fn pane_text(term: &SplitTerminal, id: SessionId) -> String {
        term.sessions[&id].with_terminal(|t| {
            let full = t.full();
            (0..full.rows())
                .map(|r| full.line_text(r))
                .collect::<Vec<_>>()
                .join("\n")
        })
    }

    /// Poll a pane's shell output until `needle` appears. The PTY pumps are
    /// asynchronous, so tests wait on observed bytes — never a bare sleep.
    fn wait_for_text(term: &SplitTerminal, id: SessionId, needle: &str) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !pane_text(term, id).contains(needle) {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for {needle:?} in pane {id:?}"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// A three-pane split `a | (b / c)` over real `/bin/sh` PTYs, with `a`
    /// focused (both in the tree and, after settling, holding egui's keyboard).
    fn three_panes(ctx: &Context) -> (SplitTerminal, SessionId, SessionId, SessionId) {
        let mut term = SplitTerminal::new(sh_opts()).expect("first shell");
        let a = term.focused_session();
        term.apply(Command::Split(SplitDir::V));
        let b = term.focused_session();
        term.apply(Command::Split(SplitDir::H));
        let c = term.focused_session();
        assert_eq!(term.session_count(), 3);
        term.focused = a;
        settle(ctx, &mut term, 3);
        (term, a, b, c)
    }

    #[test]
    fn broadcast_next_cycles_off_all_group() {
        assert_eq!(Broadcast::Off.next(), Broadcast::All);
        assert_eq!(Broadcast::All.next(), Broadcast::Group);
        assert_eq!(Broadcast::Group.next(), Broadcast::Off);
    }

    #[test]
    fn broadcast_targets_track_mode_group_and_close() {
        let ctx = test_ctx();
        let (mut term, a, b, c) = three_panes(&ctx);

        // Off (default): nobody broadcasts, no fan-out targets.
        assert_eq!(term.broadcast(), Broadcast::Off);
        assert!(term.broadcast_targets().is_empty());
        assert!(!term.is_broadcasting(a));

        // All: the other two panes (the focused one already got the keystrokes).
        term.set_broadcast(Broadcast::All);
        assert_eq!(term.broadcast_targets(), vec![b, c]);
        assert!(term.is_broadcasting(a) && term.is_broadcasting(b) && term.is_broadcasting(c));

        // Named group: an ungrouped focus routes nowhere; once a+b share "A",
        // it targets b (and never the ungrouped c).
        term.set_broadcast(Broadcast::Group);
        assert!(term.broadcast_targets().is_empty(), "focus is ungrouped");
        term.assign_group(a, Some("A".to_owned()));
        term.assign_group(b, Some("A".to_owned()));
        assert_eq!(term.broadcast_targets(), vec![b]);
        assert!(term.is_broadcasting(a) && term.is_broadcasting(b));
        assert!(!term.is_broadcasting(c), "c is not in group A");

        // A membership change re-targets: move b to "B" and a is alone in "A".
        term.assign_group(b, Some("B".to_owned()));
        assert!(term.broadcast_targets().is_empty(), "b left a's group");
        term.assign_group(b, Some("A".to_owned()));
        assert_eq!(term.broadcast_targets(), vec![b]);

        // A closed pane drops from the set and clears its membership.
        term.close_session(b);
        assert!(term.group_of(b).is_none(), "closed pane's group cleared");
        assert!(
            term.broadcast_targets().is_empty(),
            "the group lost its only peer"
        );
        term.set_broadcast(Broadcast::All);
        assert_eq!(
            term.broadcast_targets(),
            vec![c],
            "closed b is gone from All too"
        );
    }

    #[test]
    fn broadcast_all_fans_typed_input_to_every_real_pty() {
        let ctx = test_ctx();
        let (mut term, a, b, c) = three_panes(&ctx);
        term.set_broadcast(Broadcast::All);

        // Type a quoted command into the focused pane: a real frame drives the
        // widget's own input path, and the multiplexer fans the captured bytes
        // out to b and c through their PTYs.
        frame(
            &ctx,
            &mut term,
            vec![
                Event::Text("echo bx-'all'".to_owned()),
                key_event(Key::Enter, Modifiers::NONE),
            ],
            Modifiers::NONE,
        );

        // Every shell ran it — the fan-out reached all three real PTYs. The
        // quoted tail keeps the echoed *input* line from matching "bx-all".
        for id in [a, b, c] {
            wait_for_text(&term, id, "bx-all");
        }
    }

    #[test]
    fn broadcast_group_fans_only_to_the_named_group() {
        let ctx = test_ctx();
        let (mut term, a, b, c) = three_panes(&ctx);
        term.assign_group(a, Some("net".to_owned()));
        term.assign_group(c, Some("net".to_owned()));
        // b is left ungrouped.
        term.set_broadcast(Broadcast::Group);

        // Model routing: only c is a fan-out target of the focused a.
        assert_eq!(term.broadcast_targets(), vec![c]);
        assert!(!term.is_broadcasting(b));

        frame(
            &ctx,
            &mut term,
            vec![
                Event::Text("echo bx-'grp'".to_owned()),
                key_event(Key::Enter, Modifiers::NONE),
            ],
            Modifiers::NONE,
        );

        // Both grouped panes ran it: a typed it, c received the fan-out. The
        // ungrouped b's exclusion is proven above at the model level (a
        // timing-based negative over a real shell would be flaky).
        wait_for_text(&term, a, "bx-grp");
        wait_for_text(&term, c, "bx-grp");
    }

    #[test]
    fn broadcast_chords_decode_and_toggle_the_mode() {
        // Decode: Ctrl+Shift+A / Ctrl+Shift+G map to the broadcast toggles and
        // are consumed, so they never leak to a shell.
        let cs = Modifiers::CTRL | Modifiers::SHIFT;
        let ctx = Context::default();
        let raw = RawInput {
            modifiers: cs,
            events: vec![key_event(Key::A, cs), key_event(Key::G, cs)],
            ..RawInput::default()
        };
        let _ = ctx.run(raw, |ctx| {
            assert_eq!(
                consume_commands(ctx),
                vec![
                    Command::ToggleBroadcast(Broadcast::All),
                    Command::ToggleBroadcast(Broadcast::Group),
                ]
            );
            ctx.input(|i| assert!(i.events.is_empty(), "chords consumed"));
        });

        // Toggle semantics through apply: the same chord turns its mode off.
        let mut term = SplitTerminal::new(sh_opts()).expect("first shell");
        term.apply(Command::ToggleBroadcast(Broadcast::All));
        assert_eq!(term.broadcast(), Broadcast::All);
        term.apply(Command::ToggleBroadcast(Broadcast::Group));
        assert_eq!(
            term.broadcast(),
            Broadcast::Group,
            "the other chord switches modes"
        );
        term.apply(Command::ToggleBroadcast(Broadcast::Group));
        assert_eq!(
            term.broadcast(),
            Broadcast::Off,
            "the same chord toggles off"
        );
    }

    #[test]
    fn cycle_group_walks_the_named_ring_and_assign_clears() {
        let mut term = SplitTerminal::new(sh_opts()).expect("first shell");
        let a = term.focused_session();
        assert_eq!(term.group_of(a), None);
        term.cycle_group(a);
        assert_eq!(term.group_of(a), Some("A"));
        term.cycle_group(a);
        assert_eq!(term.group_of(a), Some("B"));
        term.cycle_group(a);
        assert_eq!(term.group_of(a), Some("C"));
        term.cycle_group(a);
        assert_eq!(
            term.group_of(a),
            None,
            "past the last group wraps to ungrouped"
        );

        // assign_group sets an explicit label and clears on None / an empty one.
        term.assign_group(a, Some("ops".to_owned()));
        assert_eq!(term.group_of(a), Some("ops"));
        term.assign_group(a, None);
        assert_eq!(term.group_of(a), None);
        term.assign_group(a, Some("ops".to_owned()));
        term.assign_group(a, Some(String::new()));
        assert_eq!(
            term.group_of(a),
            None,
            "an empty label clears rather than names"
        );
    }

    // ── remote pane (TERM-8) rendered through the shared grid ────────────────

    #[test]
    fn a_remote_first_pane_streams_broker_output_into_the_grid() {
        use std::sync::Arc;
        use std::time::Duration;

        use base64::engine::general_purpose::STANDARD as B64;
        use base64::Engine as _;

        use crate::remote::test_support::FakeBus;
        use crate::remote::RemotePty;

        let ctx = test_ctx();
        // A remote session on a fake broker, polling every frame (ZERO throttle).
        let bus = FakeBus::new();
        let remote = RemotePty::open(Arc::new(bus.clone()), "oak", "oak", 80, 24)
            .with_poll_interval(Duration::ZERO);
        let id = remote.session_id().to_string();
        // The broker opens the session and streams a base64 output chunk.
        let open_rec =
            r#"{"id":"ID","peer":"oak","phase":"open","seq":1,"since_ms":0}"#.replace("ID", &id);
        let out_rec = format!(
            r#"{{"id":"{id}","peer":"oak","phase":"open","seq":2,"data":"{}","since_ms":1}}"#,
            B64.encode("remote-online")
        );
        bus.push_state(&id, &open_rec);
        bus.push_state(&id, &out_rec);

        // Mount it as a split's first pane and drive frames — the SAME TERM-3
        // widget + engine renders it (§6, no second emulator).
        let mut term = SplitTerminal::from_remote(remote, sh_opts());
        let pane = term.focused_session();
        settle(&ctx, &mut term, 2);

        let text = term.sessions[&pane].with_terminal(|t| {
            let full = t.full();
            (0..full.rows())
                .map(|r| full.line_text(r))
                .collect::<Vec<_>>()
                .join("\n")
        });
        assert!(
            text.contains("remote-online"),
            "the broker's base64 output decoded into the reused engine grid: {text:?}"
        );
        // The open verb went to the broker; the pane never faked a shell (§7).
        assert_eq!(bus.verb_count("open"), 1);
        assert!(
            !term.is_empty(),
            "a live remote pane keeps the surface open"
        );
    }

    // ── TERM-10: rebuild a tab from a saved layout ──────────────────────────────

    #[test]
    fn from_layout_rebuilds_a_nested_local_tree_and_recaptures_identically() {
        use crate::layout::{LayoutPane, LayoutTab, PaneSpec};

        // A three-pane tree: an H-split of one pane over a V-split of two panes.
        let leaf = || LayoutPane::leaf(PaneSpec::local(None, Some("/bin/sh".into())));
        let tab = LayoutTab {
            title: "1".into(),
            root: LayoutPane::Split {
                dir: SplitDir::H,
                ratio: 0.5,
                a: Box::new(leaf()),
                b: Box::new(LayoutPane::Split {
                    dir: SplitDir::V,
                    ratio: 0.5,
                    a: Box::new(leaf()),
                    b: Box::new(leaf()),
                }),
            },
        };
        // No remote panes here — the reconnect closure must never fire.
        let mut make_remote =
            |_: &RemoteTarget| -> RemotePty { unreachable!("this layout has no remote panes") };
        let term = SplitTerminal::from_layout(&tab, sh_opts(), &mut make_remote).expect("rebuild");

        // Every leaf became a real local shell, and the tree kept its shape.
        assert_eq!(term.session_count(), 3, "three local shells rebuilt");
        assert!(matches!(
            term.tree(),
            Some(Pane::Split {
                dir: SplitDir::H,
                ..
            })
        ));

        // Round-trip: capturing the rebuilt surface reproduces the same shape.
        let recaptured = term.capture_tab("1").expect("capture the rebuilt tree");
        assert_eq!(recaptured.root.pane_count(), 3);
        assert!(matches!(
            recaptured.root,
            LayoutPane::Split {
                dir: SplitDir::H,
                ..
            }
        ));
    }
}
