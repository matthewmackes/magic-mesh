//! Native archive handling — the list/extract parity op (E11.6, Q34–Q39).
//!
//! Reads `.tar` and gzip-compressed `.tar.gz`/`.tgz` archives natively (the
//! `tar` + `flate2` crates), so the file manager can preview an archive's
//! contents and extract it without shelling out to `tar(1)`. Gzip is detected by
//! magic bytes (`1f 8b`), not the extension, so a mislabelled archive still
//! reads. Extraction uses the `tar` crate's path-traversal guard — an entry
//! whose path escapes the destination is skipped, never written outside it.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;

/// One archive member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveEntry {
    /// Path of the member *within* the archive.
    pub path: PathBuf,
    /// Uncompressed size in bytes.
    pub size: u64,
    /// Whether the member is a directory.
    pub is_dir: bool,
}

/// Open `path` as a tar byte stream, transparently gunzipping when the file
/// begins with the gzip magic (`1f 8b`).
fn open_tar_stream(path: &Path) -> io::Result<Box<dyn Read>> {
    let mut file = File::open(path)?;
    let mut magic = [0u8; 2];
    let is_gzip = matches!(file.read(&mut magic), Ok(2)) && magic == [0x1f, 0x8b];
    file.seek(SeekFrom::Start(0))?;
    if is_gzip {
        Ok(Box::new(GzDecoder::new(file)))
    } else {
        Ok(Box::new(file))
    }
}

/// List an archive's members without extracting anything.
///
/// # Errors
/// When the file can't be opened or the tar stream is malformed.
pub fn list(path: &Path) -> io::Result<Vec<ArchiveEntry>> {
    let mut archive = tar::Archive::new(open_tar_stream(path)?);
    let mut out = Vec::new();
    for entry in archive.entries()? {
        let entry = entry?;
        let header = entry.header();
        out.push(ArchiveEntry {
            path: entry.path()?.into_owned(),
            size: header.size().unwrap_or(0),
            is_dir: header.entry_type().is_dir(),
        });
    }
    Ok(out)
}

/// Extract every member into `dest` (created if absent). Returns the number of
/// members written; members whose path would escape `dest` (a `../…` or absolute
/// path) are skipped by the `tar` crate's guard and not counted.
///
/// # Errors
/// When the archive can't be read or a member can't be written.
pub fn extract(path: &Path, dest: &Path) -> io::Result<usize> {
    std::fs::create_dir_all(dest)?;
    let mut archive = tar::Archive::new(open_tar_stream(path)?);
    let mut written = 0;
    for entry in archive.entries()? {
        let mut entry = entry?;
        // unpack_in returns false when the entry was refused for pointing outside
        // `dest` — the path-traversal guard.
        if entry.unpack_in(dest)? {
            written += 1;
        }
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mde-files-archive-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Build a `.tar` with two files + a subdir at `path`.
    fn build_tar(path: &Path) {
        let file = File::create(path).unwrap();
        let mut b = tar::Builder::new(file);
        append_file(&mut b, "top.txt", b"top-level");
        append_file(&mut b, "dir/inner.txt", b"nested-content");
        b.finish().unwrap();
    }

    /// Build the same tree gzip-compressed at `path`.
    fn build_tar_gz(path: &Path) {
        let file = File::create(path).unwrap();
        let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut b = tar::Builder::new(enc);
        append_file(&mut b, "top.txt", b"top-level");
        append_file(&mut b, "dir/inner.txt", b"nested-content");
        b.into_inner().unwrap().finish().unwrap();
    }

    fn append_file<W: Write>(b: &mut tar::Builder<W>, name: &str, data: &[u8]) {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        b.append_data(&mut header, name, data).unwrap();
    }

    #[test]
    fn lists_a_plain_tar() {
        let dir = scratch("list-tar");
        let arc = dir.join("a.tar");
        build_tar(&arc);
        let entries = list(&arc).unwrap();
        let names: Vec<String> = entries
            .iter()
            .map(|e| e.path.display().to_string())
            .collect();
        assert!(names.contains(&"top.txt".to_string()));
        assert!(names.contains(&"dir/inner.txt".to_string()));
        let top = entries
            .iter()
            .find(|e| e.path == Path::new("top.txt"))
            .unwrap();
        assert_eq!(top.size, 9, "size of \"top-level\"");
        assert!(!top.is_dir);
    }

    #[test]
    fn lists_a_gzip_tar_by_magic_not_extension() {
        let dir = scratch("list-gz");
        // deliberately a .tar name even though the bytes are gzipped
        let arc = dir.join("mislabelled.tar");
        build_tar_gz(&arc);
        let entries = list(&arc).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|e| e.path == Path::new("dir/inner.txt")));
    }

    #[test]
    fn extracts_contents_to_a_destination() {
        let dir = scratch("extract");
        let arc = dir.join("a.tar.gz");
        build_tar_gz(&arc);
        let dest = dir.join("out");
        let n = extract(&arc, &dest).unwrap();
        assert!(n >= 2);
        assert_eq!(std::fs::read(dest.join("top.txt")).unwrap(), b"top-level");
        assert_eq!(
            std::fs::read(dest.join("dir/inner.txt")).unwrap(),
            b"nested-content"
        );
    }

    /// Hand-write a single-entry USTAR archive with an arbitrary member name —
    /// `tar::Builder` refuses to *create* a `..` path, so a malicious archive
    /// must be assembled at the byte level to exercise the *extract* guard.
    fn raw_tar(path: &Path, name: &str, data: &[u8]) {
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[100..108].copy_from_slice(b"0000644\0"); // mode
        h[108..116].copy_from_slice(b"0000000\0"); // uid
        h[116..124].copy_from_slice(b"0000000\0"); // gid
        h[124..136].copy_from_slice(format!("{:011o}\0", data.len()).as_bytes()); // size
        h[136..148].copy_from_slice(b"00000000000\0"); // mtime
        h[156] = b'0'; // typeflag: regular file
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        for b in &mut h[148..156] {
            *b = b' '; // checksum field spaces before summing
        }
        let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
        h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());

        let mut out = h.to_vec();
        out.extend_from_slice(data);
        out.resize(out.len() + (512 - data.len() % 512) % 512, 0); // pad data block
        out.resize(out.len() + 1024, 0); // two zero blocks = end marker
        std::fs::write(path, out).unwrap();
    }

    #[test]
    fn extract_refuses_path_traversal() {
        let dir = scratch("traversal");
        let arc = dir.join("evil.tar");
        raw_tar(&arc, "../escaped.txt", b"pwned");
        let dest = dir.join("out");
        // the escape is refused, so nothing was unpacked...
        let written = extract(&arc, &dest).unwrap();
        assert_eq!(written, 0, "the traversal entry is skipped");
        // ...and nothing was written to the parent dir.
        assert!(
            !dir.join("escaped.txt").exists(),
            "path-traversal entry must not write outside dest"
        );
    }

    #[test]
    fn list_missing_archive_errors() {
        let dir = scratch("missing");
        assert!(list(&dir.join("nope.tar")).is_err());
    }
}
