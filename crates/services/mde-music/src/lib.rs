//! `mde-music` — native Airsonic music GUI for MDE (AIR-* / v6.1).
//!
//! AIR-10/11 slice: the pure [`hub`] card model + [`nav`] breadcrumb/
//! navigation stack that the Iced shell (`main.rs`) renders. The live
//! grids behind each card + the playback transport arrive over the
//! `mde-musicd` data path (AIR-10.b / AIR-2).

pub mod album;
pub mod color;
pub mod hub;
pub mod library;
pub mod nav;
pub mod nowplaying;
pub mod prefs;
pub mod search;
