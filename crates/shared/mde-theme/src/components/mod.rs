//! Reusable layout primitives — the canonical data shapes every
//! Iced panel pulls from. UX-6 introduces this module so panel
//! authors have one place to look for "what does a polished
//! empty-state / card / status badge actually look like."
//!
//! `mde-theme` keeps the data forms (struct definitions, tier
//! constants). The Iced widget builders live in the
//! consumer-side `crates/mde-workbench/src/panel_chrome.rs` so
//! the toolkit dep doesn't leak into this crate.

pub mod empty_state;
pub mod object_card;

pub use empty_state::{
    EmptyState, BODY_CTA_GAP, EMPTY_ICON_SIZE, HEADING_BODY_GAP, VERTICAL_PADDING,
};
pub use object_card::{
    CardSize, CardState, IconPlacement, ObjectCard, CARD_CORNER_RADIUS, CARD_DISABLED_OPACITY,
    CARD_FOCUS_OUTLINE_OFFSET, CARD_FOCUS_OUTLINE_WIDTH, CARD_GRID_GAP, CARD_HOVER_OVERLAY_ALPHA,
    CARD_PADDING, CARD_PRESS_RIPPLE_ALPHA, CARD_PRESS_RIPPLE_DURATION_MS,
    CARD_SELECTED_BORDER_WIDTH, CARD_SELECTED_OVERLAY_ALPHA, CARD_SHADOW_DEFAULT_ALPHA,
    CARD_SHADOW_DEFAULT_BLUR, CARD_SHADOW_DEFAULT_OFFSET_Y, CARD_SHADOW_HOVER_ALPHA,
    CARD_SHADOW_HOVER_BLUR, CARD_SHADOW_HOVER_OFFSET_Y, CARD_SHADOW_PRESSED_ALPHA,
    CARD_SHADOW_PRESSED_BLUR, CARD_SHADOW_PRESSED_OFFSET_Y, CARD_SUBTITLE_SIZE, CARD_TITLE_SIZE,
};
