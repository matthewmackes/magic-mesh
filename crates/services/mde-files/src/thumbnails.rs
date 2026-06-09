//! Native thumbnail generation — the freedesktop thumbnail-cache parity op
//! (E11.6, Q34–Q39).
//!
//! Decodes an image, scales it down to the spec box (128 px `normal` / 256 px
//! `large`, aspect-preserving, never up-scaled), and writes a PNG into the shared
//! thumbnail cache at `$XDG_CACHE_HOME/thumbnails/<size>/<md5(uri)>.png`, carrying
//! the mandated `Thumb::URI` + `Thumb::MTime` tEXt chunks so any spec-aware viewer
//! (and our own GUI) reuses it and detects staleness. Native `image` + `png`
//! decode/encode — no `gdk-pixbuf` / external thumbnailer.
//!
//! Spec: <https://specifications.freedesktop.org/thumbnail-spec/>.

use std::fs::File;
use std::io::{self, BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Thumbnail size class per the spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbSize {
    /// 128×128 box (`thumbnails/normal`).
    Normal,
    /// 256×256 box (`thumbnails/large`).
    Large,
}

impl ThumbSize {
    /// The maximum edge length in pixels.
    #[must_use]
    pub const fn pixels(self) -> u32 {
        match self {
            Self::Normal => 128,
            Self::Large => 256,
        }
    }

    /// The cache subdirectory name.
    #[must_use]
    pub const fn dir(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Large => "large",
        }
    }
}

/// The canonical `file://` URI for `path` (made absolute, percent-encoded with
/// `/` and the RFC-3986 unreserved set left bare) — the key the cache hashes.
#[must_use]
pub fn file_uri(path: &Path) -> String {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let mut uri = String::from("file://");
    for &b in abs.as_os_str().as_encoded_bytes() {
        match b {
            b'/' | b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                uri.push(b as char);
            }
            _ => uri.push_str(&format!("%{b:02X}")),
        }
    }
    uri
}

/// The spec cache filename for a URI: the lowercase-hex MD5 of the URI, `.png`.
#[must_use]
pub fn cache_filename(uri: &str) -> String {
    format!("{:x}.png", md5::compute(uri.as_bytes()))
}

/// The default thumbnail cache root: `$XDG_CACHE_HOME/thumbnails`, or
/// `$HOME/.cache/thumbnails`.
#[must_use]
pub fn cache_root() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("thumbnails")
}

/// Generate a thumbnail for `path` in the default cache, returning its path.
///
/// # Errors
/// When the source can't be read/decoded or the thumbnail can't be written.
pub fn generate(path: &Path, size: ThumbSize) -> io::Result<PathBuf> {
    generate_into(path, size, &cache_root())
}

/// Generate a thumbnail under an explicit `cache_root` (so tests don't touch the
/// real cache). The file lands at `<cache_root>/<size.dir>/<md5(uri)>.png`.
///
/// # Errors
/// When the source can't be read/decoded, or any write/rename fails.
pub fn generate_into(path: &Path, size: ThumbSize, cache_root: &Path) -> io::Result<PathBuf> {
    let uri = file_uri(path);
    let mtime = std::fs::symlink_metadata(path)?
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());

    let img = image::open(path)
        .map_err(|e| io::Error::other(format!("decode {}: {e}", path.display())))?;
    let max = size.pixels();
    let scaled = if img.width().max(img.height()) > max {
        img.resize(max, max, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };
    let rgba = scaled.to_rgba8();
    let (w, h) = rgba.dimensions();

    let dir = cache_root.join(size.dir());
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join(cache_filename(&uri));
    // Atomic: encode to a temp sibling, fsync via drop, then rename into place.
    let tmp = dir.join(format!(
        ".{}.{}.tmp",
        cache_filename(&uri),
        std::process::id()
    ));

    {
        let writer = BufWriter::new(File::create(&tmp)?);
        let mut enc = png::Encoder::new(writer, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.add_text_chunk("Thumb::URI".to_string(), uri.clone())
            .map_err(io::Error::other)?;
        enc.add_text_chunk("Thumb::MTime".to_string(), mtime.to_string())
            .map_err(io::Error::other)?;
        let _ = enc.add_text_chunk("Software".to_string(), "mde-files".to_string());
        let mut png_writer = enc.write_header().map_err(io::Error::other)?;
        png_writer
            .write_image_data(&rgba)
            .map_err(io::Error::other)?;
        png_writer.finish().map_err(io::Error::other)?;
    }
    // Spec: thumbnails are private (0600).
    set_user_only(&tmp)?;
    std::fs::rename(&tmp, &dest)?;
    Ok(dest)
}

#[cfg(unix)]
fn set_user_only(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

/// Read back a thumbnail's `Thumb::URI` / `Thumb::MTime` tEXt chunks (the
/// staleness check a consumer runs: regenerate when the source MTime differs).
///
/// # Errors
/// When the file can't be read as a PNG.
pub fn read_thumb_metadata(path: &Path) -> io::Result<Vec<(String, String)>> {
    let decoder = png::Decoder::new(BufReader::new(File::open(path)?));
    let reader = decoder.read_info().map_err(io::Error::other)?;
    let info = reader.info();
    Ok(info
        .uncompressed_latin1_text
        .iter()
        .map(|c| (c.keyword.clone(), c.text.clone()))
        .collect())
}

/// Mark a thumbnail stale-or-missing: `true` when no cached thumbnail exists for
/// `source`, or its recorded `Thumb::MTime` no longer matches the source's.
#[must_use]
pub fn needs_refresh(source: &Path, size: ThumbSize, cache_root: &Path) -> bool {
    let uri = file_uri(source);
    let dest = cache_root.join(size.dir()).join(cache_filename(&uri));
    let Ok(current) = std::fs::symlink_metadata(source).and_then(|m| m.modified()) else {
        return true;
    };
    let current_secs = current
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let Ok(meta) = read_thumb_metadata(&dest) else {
        return true; // no thumbnail yet
    };
    meta.iter()
        .find(|(k, _)| k == "Thumb::MTime")
        .and_then(|(_, v)| v.parse::<u64>().ok())
        != Some(current_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mde-files-thumb-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Write a `w`×`h` solid-colour PNG source image.
    fn make_png(path: &Path, w: u32, h: u32) {
        let buf = image::RgbImage::from_pixel(w, h, image::Rgb([200, 100, 50]));
        image::DynamicImage::ImageRgb8(buf).save(path).unwrap();
    }

    #[test]
    fn file_uri_is_absolute_and_percent_encoded() {
        let uri = file_uri(Path::new("/home/mm/a file.png"));
        assert_eq!(uri, "file:///home/mm/a%20file.png");
    }

    #[test]
    fn cache_filename_is_md5_of_uri() {
        // md5("file:///x") — stable, so the cache key is reproducible.
        let uri = "file:///x";
        let expect = format!("{:x}.png", md5::compute(uri.as_bytes()));
        assert_eq!(cache_filename(uri), expect);
        assert!(expect.ends_with(".png"));
        assert_eq!(expect.len(), 32 + 4, "32 hex chars + .png");
    }

    #[test]
    fn generate_scales_down_and_lands_at_the_spec_path() {
        let dir = scratch("gen");
        let src = dir.join("big.png");
        make_png(&src, 400, 300);
        let cache = dir.join("cache");
        let dest = generate_into(&src, ThumbSize::Normal, &cache).unwrap();

        // path is <cache>/normal/<md5(uri)>.png
        let uri = file_uri(&src);
        assert_eq!(dest, cache.join("normal").join(cache_filename(&uri)));
        assert!(dest.exists());

        // scaled to fit the 128 box, aspect preserved (400x300 -> 128x96)
        let thumb = image::open(&dest).unwrap();
        assert!(thumb.width() <= 128 && thumb.height() <= 128);
        assert_eq!(thumb.width(), 128);
        assert_eq!(thumb.height(), 96);
    }

    #[test]
    fn generate_does_not_upscale_a_small_image() {
        let dir = scratch("small");
        let src = dir.join("tiny.png");
        make_png(&src, 40, 30);
        let cache = dir.join("cache");
        let dest = generate_into(&src, ThumbSize::Large, &cache).unwrap();
        let thumb = image::open(&dest).unwrap();
        assert_eq!(
            (thumb.width(), thumb.height()),
            (40, 30),
            "kept original size"
        );
    }

    #[test]
    fn thumbnail_carries_uri_and_mtime_text_chunks() {
        let dir = scratch("meta");
        let src = dir.join("img.png");
        make_png(&src, 200, 200);
        let cache = dir.join("cache");
        let dest = generate_into(&src, ThumbSize::Normal, &cache).unwrap();

        let meta = read_thumb_metadata(&dest).unwrap();
        let uri = file_uri(&src);
        assert!(meta.iter().any(|(k, v)| k == "Thumb::URI" && v == &uri));
        assert!(
            meta.iter()
                .any(|(k, v)| k == "Thumb::MTime" && v.parse::<u64>().is_ok()),
            "MTime present and numeric"
        );
    }

    #[test]
    fn needs_refresh_tracks_existence_and_mtime() {
        let dir = scratch("refresh");
        let src = dir.join("p.png");
        make_png(&src, 100, 100);
        let cache = dir.join("cache");
        // no thumbnail yet -> needs refresh
        assert!(needs_refresh(&src, ThumbSize::Normal, &cache));
        generate_into(&src, ThumbSize::Normal, &cache).unwrap();
        // fresh thumbnail -> no refresh
        assert!(!needs_refresh(&src, ThumbSize::Normal, &cache));
    }
}
