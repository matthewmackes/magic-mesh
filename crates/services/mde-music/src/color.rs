//! AIR-16 (v6.1) — per-album dominant colour + contrast text.
//!
//! The album page fetches its cover art over the Bus (`get-cover-art`,
//! base64), decodes + downscales it, and runs a quantized-histogram
//! dominant-colour pass (a median-cut approximation that skips near-black /
//! near-white so the cover's accent wins over borders). The page header
//! then tints to that colour with a WCAG-contrast text colour; any failure
//! falls back to Indigo. [`dominant_color`] + [`contrast_text`] are pure +
//! unit-tested; the decode + Bus fetch are I/O.

use std::collections::HashMap;

use base64::Engine;

use crate::album::{req, with_bus};

/// MUSIC-ARTGATE — bound concurrent cover-art bus round-trips. The album/folder
/// grid fans out ONE `fetch_cover_art` per visible item; a 200+-item folder
/// would otherwise fire 200+ simultaneous `spawn_blocking` tasks, each opening a
/// SQLite handle on the shared `/run/mde-bus` index and waiting up to 5 s on the
/// single-threaded daemon — exhausting the blocking pool + stampeding the bus,
/// which froze the window while browsing (live 2026-06-17). Acquiring a permit
/// before each round-trip caps in-flight fetches; the rest await cheaply (no
/// blocking thread held) and complete as permits free.
static ART_GATE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

/// The fallback accent when extraction fails / can't meet contrast —
/// the canonical MDE indigo accent, single-sourced from the design
/// palette (`mde_theme::Palette::dark().accent`) so there is no
/// hardcoded hex here (E5.3 / §2.1).
#[must_use]
pub fn accent_rgb() -> (u8, u8, u8) {
    let a = mde_theme::Palette::dark().accent;
    (a.r, a.g, a.b)
}

/// Dominant colour of an interleaved RGBA buffer via a 4-bit/channel
/// quantized histogram (a median-cut approximation): the most-populous
/// bucket's average, skipping transparent + near-black + near-white pixels
/// so the cover's accent wins. `None` when every pixel is skipped/empty.
#[must_use]
pub fn dominant_color(rgba: &[u8]) -> Option<(u8, u8, u8)> {
    let mut buckets: HashMap<u16, (u64, u64, u64, u64)> = HashMap::new();
    for px in rgba.chunks_exact(4) {
        if px[3] < 128 {
            continue; // transparent
        }
        let max = px[0].max(px[1]).max(px[2]);
        let min = px[0].min(px[1]).min(px[2]);
        if max < 24 || min > 232 {
            continue; // near-black / near-white — skip so the accent wins
        }
        let key = ((u16::from(px[0]) >> 4) << 8)
            | ((u16::from(px[1]) >> 4) << 4)
            | (u16::from(px[2]) >> 4);
        let e = buckets.entry(key).or_insert((0, 0, 0, 0));
        e.0 += u64::from(px[0]);
        e.1 += u64::from(px[1]);
        e.2 += u64::from(px[2]);
        e.3 += 1;
    }
    let best = buckets.values().max_by_key(|v| v.3)?;
    let n = best.3.max(1);
    Some((
        u8::try_from(best.0 / n).unwrap_or(0),
        u8::try_from(best.1 / n).unwrap_or(0),
        u8::try_from(best.2 / n).unwrap_or(0),
    ))
}

/// A WCAG-ish contrast text colour for a background: white on a dark bg,
/// charcoal on a light bg (luminance via the Rec. 601 weights).
#[must_use]
pub fn contrast_text(bg: (u8, u8, u8)) -> (u8, u8, u8) {
    let lum = (u32::from(bg.0) * 299 + u32::from(bg.1) * 587 + u32::from(bg.2) * 114) / 1000;
    if lum < 140 {
        (255, 255, 255)
    } else {
        (40, 40, 40)
    }
}

/// Decode `bytes` (JPEG/PNG), downscale to a ~64×64 thumbnail, and extract
/// the dominant colour + its contrast text colour. `None` on a decode
/// failure or an all-skipped image (the caller then uses [`accent_rgb`]).
#[must_use]
pub fn extract(bytes: &[u8]) -> Option<((u8, u8, u8), (u8, u8, u8))> {
    let img = image::load_from_memory(bytes).ok()?;
    let thumb = img.thumbnail(64, 64).to_rgba8();
    let dom = dominant_color(thumb.as_raw())?;
    Some((dom, contrast_text(dom)))
}

/// Fetch the raw cover-art bytes over the Bus (`action/music/get-cover-art`,
/// the coverArt id in the body; the daemon base64s them into its reply).
/// The caller renders the image (iced) + runs [`extract`] for the dominant
/// colour off the same bytes (one fetch). Empty `Vec` when there's no art.
///
/// # Errors
/// Bus-store / request / timeout failures.
pub async fn fetch_cover_art(cover_id: String) -> Result<Vec<u8>, String> {
    // MUSIC-RESPONSIVE-3 — serve from the persistent on-disk thumbnail cache
    // first: a cache hit skips the bus round-trip (and its ART_GATE permit)
    // entirely, so a relaunch paints grid art immediately instead of re-fetching
    // every cover. Only a miss hits the daemon, and the result is written back.
    if let Some(path) = art_cache_path(&cover_id) {
        if let Ok(bytes) = std::fs::read(&path) {
            if !bytes.is_empty() {
                return Ok(bytes);
            }
        }
    }
    // MUSIC-ARTGATE — hold a permit for the whole round-trip so no more than
    // ART_GATE permits of cover-art fetches hit the bus/daemon at once. The
    // permit drops when `_permit` goes out of scope at the end of the fn.
    let _permit = ART_GATE
        .acquire()
        .await
        .map_err(|e| format!("art gate closed: {e}"))?;
    let id_for_fetch = cover_id.clone();
    let bytes = with_bus(move |p, rt| {
        let reply = req(p, rt, "action/music/get-cover-art", Some(&id_for_fetch))?;
        let v: serde_json::Value = serde_json::from_str(&reply).map_err(|e| e.to_string())?;
        let b64 = v
            .get("result")
            .and_then(|r| r.get("art"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        Ok(base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap_or_default())
    })
    .await?;
    // Populate the cache on a successful, non-empty fetch (best-effort).
    if !bytes.is_empty() {
        if let Some(path) = art_cache_path(&cover_id) {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&path, &bytes);
        }
    }
    Ok(bytes)
}

/// MUSIC-RESPONSIVE-3 — on-disk path for a cached cover thumbnail
/// (`$XDG_CACHE_HOME|~/.cache/mde-music/art/<sanitized coverArt>`). The id is
/// sanitized to a safe filename (Airsonic ids like `al-123` pass through; any
/// other char → `_`). `None` if no cache home / an empty id.
fn art_cache_path(cover_id: &str) -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))?;
    let safe: String = cover_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if safe.is_empty() {
        return None;
    }
    Some(base.join("mde-music").join("art").join(safe))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba(px: &[(u8, u8, u8)]) -> Vec<u8> {
        px.iter().flat_map(|&(r, g, b)| [r, g, b, 255]).collect()
    }

    #[test]
    fn dominant_picks_the_accent_skipping_black_and_white() {
        // Black + white borders (skipped) + a red accent → red wins.
        let mut buf = rgba(&[(0, 0, 0), (0, 0, 0), (255, 255, 255), (255, 255, 255)]);
        buf.extend(rgba(&[(200, 30, 30), (205, 35, 35), (195, 25, 25)]));
        let d = dominant_color(&buf).unwrap();
        assert!(
            d.0 > 150 && d.1 < 90 && d.2 < 90,
            "expected red-ish, got {d:?}"
        );
    }

    #[test]
    fn dominant_is_none_when_all_skipped() {
        // Only near-black + near-white → nothing survives the skip.
        assert!(dominant_color(&rgba(&[(0, 0, 0), (255, 255, 255), (8, 8, 8)])).is_none());
        assert!(dominant_color(&[]).is_none());
    }

    #[test]
    fn contrast_is_white_on_dark_charcoal_on_light() {
        assert_eq!(contrast_text((20, 20, 20)), (255, 255, 255));
        assert_eq!(contrast_text((240, 240, 240)), (40, 40, 40));
        // Indigo is dark-ish → white text.
        assert_eq!(contrast_text(accent_rgb()), (255, 255, 255));
    }
}
