//! Native MIME detection — content magic-bytes + extension fallback (E11.6).
//!
//! The file manager needs a file's MIME type to pick an icon, a default handler
//! ([`crate::desktop`]), and a thumbnailer. This sniffs the leading bytes against
//! a built-in magic table (so a mislabelled/extension-less file is still typed),
//! and falls back to an extension map. A native subset of shared-mime-info — the
//! full `magic` database is a follow-up; this covers the common formats.

use std::io::Read;
use std::path::Path;

/// Detect a path's MIME type: content magic first (authoritative), then the
/// extension map, else `None`.
#[must_use]
pub fn detect(path: &Path) -> Option<&'static str> {
    from_content(path).or_else(|| from_extension(path))
}

/// Sniff the file's leading bytes against the magic table. Reads at most 264
/// bytes (enough for the tar `ustar` signature at offset 257). `None` when the
/// file can't be read or matches nothing.
#[must_use]
pub fn from_content(path: &Path) -> Option<&'static str> {
    let mut buf = [0u8; 264];
    let n = std::fs::File::open(path).ok()?.read(&mut buf).ok()?;
    from_bytes(&buf[..n])
}

/// Match a byte prefix against the magic table.
#[must_use]
pub fn from_bytes(b: &[u8]) -> Option<&'static str> {
    let starts = |sig: &[u8]| b.len() >= sig.len() && &b[..sig.len()] == sig;
    if starts(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some("image/png");
    }
    if starts(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if starts(b"GIF87a") || starts(b"GIF89a") {
        return Some("image/gif");
    }
    if starts(b"BM") {
        return Some("image/bmp");
    }
    // RIFF....WEBP
    if starts(b"RIFF") && b.len() >= 12 && &b[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if starts(b"%PDF-") {
        return Some("application/pdf");
    }
    if starts(&[0x1F, 0x8B]) {
        return Some("application/gzip");
    }
    // ZIP (and its container formats) — local-file/central/empty headers.
    if starts(&[0x50, 0x4B, 0x03, 0x04])
        || starts(&[0x50, 0x4B, 0x05, 0x06])
        || starts(&[0x50, 0x4B, 0x07, 0x08])
    {
        return Some("application/zip");
    }
    if starts(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C]) {
        return Some("application/x-7z-compressed");
    }
    if starts(&[0xFD, b'7', b'z', b'X', b'Z', 0x00]) {
        return Some("application/x-xz");
    }
    if starts(b"BZh") {
        return Some("application/x-bzip2");
    }
    if starts(&[0x7F, b'E', b'L', b'F']) {
        return Some("application/x-executable");
    }
    if starts(b"OggS") {
        return Some("audio/ogg");
    }
    if starts(b"fLaC") {
        return Some("audio/flac");
    }
    if starts(b"ID3") || starts(&[0xFF, 0xFB]) {
        return Some("audio/mpeg");
    }
    // tar: "ustar" magic lives at byte offset 257.
    if b.len() >= 262 && &b[257..262] == b"ustar" {
        return Some("application/x-tar");
    }
    None
}

/// Map a path's extension to a MIME type (lower-cased, no dot).
#[must_use]
pub fn from_extension(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "txt" | "text" | "log" | "md" => "text/plain",
        "html" | "htm" => "text/html",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "ogg" => "audio/ogg",
        "wav" => "audio/x-wav",
        "mp4" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "zip" => "application/zip",
        "tar" => "application/x-tar",
        "gz" | "tgz" => "application/gzip",
        "xz" => "application/x-xz",
        "bz2" => "application/x-bzip2",
        "7z" => "application/x-7z-compressed",
        "json" => "application/json",
        "xml" => "application/xml",
        "sh" => "application/x-shellscript",
        "rs" => "text/rust",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mde-files-mime-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn sniffs_common_magic_signatures() {
        assert_eq!(
            from_bytes(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0]),
            Some("image/png")
        );
        assert_eq!(from_bytes(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(from_bytes(b"GIF89a..."), Some("image/gif"));
        assert_eq!(from_bytes(b"%PDF-1.7"), Some("application/pdf"));
        assert_eq!(from_bytes(&[0x1F, 0x8B, 0x08]), Some("application/gzip"));
        assert_eq!(
            from_bytes(&[0x50, 0x4B, 0x03, 0x04]),
            Some("application/zip")
        );
        assert_eq!(from_bytes(b"OggS...."), Some("audio/ogg"));
        assert!(from_bytes(b"just some text").is_none());
    }

    #[test]
    fn tar_ustar_magic_at_offset_257() {
        let mut b = vec![0u8; 262];
        b[257..262].copy_from_slice(b"ustar");
        assert_eq!(from_bytes(&b), Some("application/x-tar"));
    }

    #[test]
    fn webp_needs_the_riff_and_webp_markers() {
        let mut b = Vec::from(*b"RIFF");
        b.extend_from_slice(&[0, 0, 0, 0]); // size
        b.extend_from_slice(b"WEBP");
        assert_eq!(from_bytes(&b), Some("image/webp"));
        // RIFF without WEBP (e.g. a WAV) is not webp
        let mut wav = Vec::from(*b"RIFF");
        wav.extend_from_slice(&[0, 0, 0, 0]);
        wav.extend_from_slice(b"WAVE");
        assert_eq!(from_bytes(&wav), None);
    }

    #[test]
    fn detect_prefers_content_over_extension() {
        let dir = scratch("detect");
        // a real PNG mislabelled as .txt — content wins.
        let f = dir.join("mislabelled.txt");
        std::fs::write(&f, [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]).unwrap();
        assert_eq!(detect(&f), Some("image/png"));

        // an unsniffable body falls back to the extension.
        let t = dir.join("notes.md");
        std::fs::write(&t, b"# hello\n").unwrap();
        assert_eq!(detect(&t), Some("text/plain"));

        // neither -> None
        let u = dir.join("blob.unknownext");
        std::fs::write(&u, b"\x01\x02 random").unwrap();
        assert_eq!(detect(&u), None);
    }

    #[test]
    fn extension_map_is_case_insensitive() {
        assert_eq!(from_extension(Path::new("A.PNG")), Some("image/png"));
        assert_eq!(from_extension(Path::new("x.FLAC")), Some("audio/flac"));
        assert_eq!(from_extension(Path::new("noext")), None);
    }
}
