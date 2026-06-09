//! Phase 1.9 — `Layout::Grid` rendering primitives.
//!
//! Pure-data helpers the Iced view consults when `MdeFiles.layout
//! == Layout::Grid` to lay out the current file list as a tile
//! grid. The actual Iced widget tree (containers + spaces +
//! aligned columns + the mime-icon image) lives with the view-
//! functions; this module ships the math:
//!
//!   * [`columns_for_width`] — how many tile columns fit at the
//!     given container width.
//!   * [`tile_size`] — locked square tile size (120 × 120 px per
//!     the design spec).
//!   * [`tile_layout`] — given `(container_width, num_files)`
//!     returns `TileLayout { columns, rows, total_height }`.
//!   * [`TileMetadata`] — pure-data tile descriptor (filename,
//!     mime, origin pill text, glyph index). The view binds this
//!     to its widget tree without re-deriving the strings.
//!
//! Locked: 120 px tile + 16 px gutter (matches the prototype's
//! "small grid" layout from `Artifact-Manager.html`). Switching
//! to a different tile size is a one-line change to
//! [`TILE_SIZE_PX`] + [`TILE_GUTTER_PX`].

use crate::model::{FileRow, Mime};

/// Locked tile side length (square). Per the prototype.
pub const TILE_SIZE_PX: u16 = 120;

/// Locked gutter between tiles.
pub const TILE_GUTTER_PX: u16 = 16;

/// Locked grid edge padding.
pub const GRID_EDGE_PADDING_PX: u16 = 24;

/// Compute how many tile columns fit at the given container width.
/// Always returns at least 1 so the view never produces zero
/// columns (which would cause a divide-by-zero in the grid
/// layout).
#[must_use]
pub fn columns_for_width(container_width_px: u16) -> u16 {
    let usable = container_width_px.saturating_sub(2 * GRID_EDGE_PADDING_PX);
    // Each column takes TILE_SIZE_PX of width + one gutter on its
    // right (the last column's "right gutter" is the edge padding).
    let per_col = TILE_SIZE_PX as u32 + TILE_GUTTER_PX as u32;
    if per_col == 0 {
        return 1;
    }
    // The first column doesn't need a gutter on its left (edge
    // padding covers that); subsequent columns each cost per_col.
    let cols = (usable as u32 + TILE_GUTTER_PX as u32) / per_col;
    cols.max(1) as u16
}

/// Tile layout for a given container + file count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileLayout {
    /// Tile columns (≥ 1).
    pub columns: u16,
    /// Tile rows (≥ 1 when there are files; 0 when there are
    /// none).
    pub rows: u16,
    /// Pixel height the grid will occupy when rendered.
    pub total_height_px: u16,
}

/// Compute the tile layout for `num_files` tiles in a container
/// of `container_width_px`.
#[must_use]
pub fn tile_layout(container_width_px: u16, num_files: usize) -> TileLayout {
    let columns = columns_for_width(container_width_px);
    if num_files == 0 {
        return TileLayout {
            columns,
            rows: 0,
            total_height_px: 0,
        };
    }
    let rows_u32 = (num_files as u32).div_ceil(columns as u32);
    let rows: u16 = rows_u32.min(u16::MAX as u32) as u16;
    let total_height_px: u32 = 2 * GRID_EDGE_PADDING_PX as u32
        + rows as u32 * TILE_SIZE_PX as u32
        + (rows as u32).saturating_sub(1) * TILE_GUTTER_PX as u32;
    TileLayout {
        columns,
        rows,
        total_height_px: total_height_px.min(u16::MAX as u32) as u16,
    }
}

/// Per-tile metadata. View widget consumers bind these strings
/// directly without re-deriving them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TileMetadata {
    /// Filename (top label).
    pub name: String,
    /// Origin pill text — set to the peer name when the file
    /// came from a peer, `None` when local.
    pub origin: Option<String>,
    /// Mime category for the metadata-icon lookup.
    pub mime: Mime,
    /// Human-friendly size + age subtitle ("12 MB · 3 min ago").
    pub subtitle: String,
}

impl TileMetadata {
    /// Build a tile descriptor from a `FileRow`.
    #[must_use]
    pub fn from_row(row: &FileRow) -> Self {
        Self {
            name: row.name.to_string(),
            origin: row.origin().map(str::to_string),
            mime: row.mime,
            subtitle: format!("{} · {}", row.size, row.age),
        }
    }
}

/// Build a flat `Vec<TileMetadata>` for `rows`. Mirror of
/// `filter_rows` from the search module — pure conversion, view
/// binds it as the grid's row source.
#[must_use]
pub fn tile_metadata_for(rows: &[FileRow]) -> Vec<TileMetadata> {
    rows.iter().map(TileMetadata::from_row).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &'static str) -> FileRow {
        FileRow::local(name, Mime::Doc, "12 MB", "3 min ago")
    }

    #[test]
    fn columns_for_narrow_container_is_1() {
        assert_eq!(columns_for_width(0), 1);
        assert_eq!(columns_for_width(100), 1);
        assert_eq!(columns_for_width(120), 1);
        assert_eq!(columns_for_width(160), 1);
    }

    #[test]
    fn columns_for_wide_container_grows_in_tiles() {
        // 24 padding + 120 tile + 24 padding = 168 → 1 column.
        // 24 + 120 + 16 + 120 + 24 = 304 → 2 columns.
        // 24 + 120 + 16 + 120 + 16 + 120 + 24 = 440 → 3 columns.
        assert!(columns_for_width(304) >= 2);
        assert!(columns_for_width(440) >= 3);
        assert!(columns_for_width(800) >= 5);
    }

    #[test]
    fn tile_layout_zero_files_zero_rows() {
        let l = tile_layout(800, 0);
        assert!(l.columns >= 1);
        assert_eq!(l.rows, 0);
        assert_eq!(l.total_height_px, 0);
    }

    #[test]
    fn tile_layout_partial_row_rounds_up() {
        // 5 tiles in a 3-column grid = 2 rows.
        let cols = columns_for_width(440);
        let l = tile_layout(440, 5);
        assert_eq!(l.columns, cols);
        assert_eq!(l.rows, 2);
    }

    #[test]
    fn tile_layout_total_height_includes_padding_and_gutters() {
        // 2 rows × 120 tile + 1 gutter × 16 + 2 × 24 padding = 304.
        let l = tile_layout(800, 10);
        let expected = 2 * GRID_EDGE_PADDING_PX
            + l.rows * TILE_SIZE_PX
            + (l.rows.saturating_sub(1)) * TILE_GUTTER_PX;
        assert_eq!(l.total_height_px, expected);
    }

    #[test]
    fn tile_metadata_from_row_carries_origin_when_present() {
        let r = row("a.doc").with_mesh("pine.mesh");
        let t = TileMetadata::from_row(&r);
        assert_eq!(t.name, "a.doc");
        assert_eq!(t.origin.as_deref(), Some("pine.mesh"));
        assert_eq!(t.mime, Mime::Doc);
        assert!(t.subtitle.contains("12 MB"));
        assert!(t.subtitle.contains("3 min ago"));
    }

    #[test]
    fn tile_metadata_for_returns_one_tile_per_row() {
        let rows = vec![row("a"), row("b"), row("c")];
        let tiles = tile_metadata_for(&rows);
        assert_eq!(tiles.len(), 3);
        assert_eq!(tiles[1].name, "b");
    }

    #[test]
    fn locked_constants_match_design_spec() {
        // Lock-check — changing these is a wire-format break for
        // the screenshot regression tests + the design spec.
        assert_eq!(TILE_SIZE_PX, 120);
        assert_eq!(TILE_GUTTER_PX, 16);
        assert_eq!(GRID_EDGE_PADDING_PX, 24);
    }

    #[test]
    fn columns_is_at_least_1_for_pathological_inputs() {
        // u16::MAX-px container, etc.
        assert!(columns_for_width(u16::MAX) >= 1);
        // 24 + ... container width less than padding.
        assert!(columns_for_width(48) >= 1);
    }

    #[test]
    fn tile_layout_handles_single_file_one_row() {
        let l = tile_layout(800, 1);
        assert_eq!(l.rows, 1);
        // One row → no gutter between rows → just padding + tile.
        let expected_h = 2 * GRID_EDGE_PADDING_PX + TILE_SIZE_PX;
        assert_eq!(l.total_height_px, expected_h);
    }
}
