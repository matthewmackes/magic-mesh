//! FILEMGR-3 — archives: create / extract / browse-in-place.
//!
//! Packaging + unpacking are first-class Files verbs: compress a selection to a
//! chosen format, double-click to browse an archive's members without unpacking
//! it, and extract-here / extract-to. Everything runs through the injectable
//! [`FileOps`] seam (FILEMGR-1) — the engine reads member/archive bytes and
//! writes extracted files through the trait, never `std::fs` directly, so it is
//! render-agnostic (the surface is FILEMGR-8) and unit-tested against
//! [`crate::fileops::FakeFileOps`]. Long ops report [`Progress`] and honour an
//! [`OpControl`] exactly like the FILEMGR-2 op queue, so [`compress`]/[`extract`]
//! run on the same [`crate::opqueue::OpQueue`] and emit the same events.
//!
//! ## Formats (§9 — pure-Rust crates, never a shell-out to `zip`/`tar`)
//!
//! The dev farm is airgapped, so this is built strictly from crates already in
//! the workspace lockfile:
//!
//! * **tar** and **tar.gz** — the locked `tar` + `flate2` crates. Fully wired.
//! * **zip** — a self-contained reader/writer (STORE + DEFLATE) over `flate2`'s
//!   raw-deflate stream plus an inline CRC-32; no `zip` crate is in the lock, but
//!   none is needed. ZIP64 / encryption / multi-disk are out of scope (v1 lock:
//!   password-zip excluded). Fully wired.
//! * **tar.xz** and **tar.zst** — the `xz2`/`liblzma` and `zstd` codec crates
//!   are **not** in the airgapped lockfile, so these return an honest typed
//!   [`io::ErrorKind::Unsupported`] naming the missing crate (§7 — never a stub
//!   that fakes success). They light up the day the codec crate is vendored;
//!   the tar plumbing already handles them.
//!
//! Extraction guards against path traversal: a member whose path escapes the
//! destination (an absolute path or a `..` component) is refused, never written
//! outside the destination.

use crate::backend::OpId;
use crate::fileops::FileOps;
use crate::opqueue::{OpControl, OpOutcome, Progress};

use std::ffi::OsStr;
use std::io::{self, Cursor, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use flate2::read::{DeflateDecoder, GzDecoder};
use flate2::write::{DeflateEncoder, GzEncoder};
use flate2::Compression;
use tar::{Builder as TarBuilder, EntryType, Header as TarHeader};

// ═══════════════════════════════════════════════════════════════════════════
// Format model.
// ═══════════════════════════════════════════════════════════════════════════

/// A supported (or honestly-gated) archive container. `Zip`, `Tar`, `TarGz`
/// are fully wired; `TarXz`/`TarZst` are recognised (so the surface can offer +
/// detect them) but their codec crate is not in the airgapped lockfile, so the
/// engine returns a typed [`io::ErrorKind::Unsupported`] until it is vendored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    /// `.zip` — STORE + DEFLATE, self-contained (no `zip` crate needed).
    Zip,
    /// `.tar` — an uncompressed tarball.
    Tar,
    /// `.tar.gz` / `.tgz` — a gzip-compressed tarball (`flate2`).
    TarGz,
    /// `.tar.xz` / `.txz` — gated on the unlocked `xz2` (liblzma) crate.
    TarXz,
    /// `.tar.zst` / `.tzst` — gated on the unlocked `zstd` crate.
    TarZst,
}

impl ArchiveFormat {
    /// Guess the format from a filename's extension (case-insensitive, honouring
    /// the two-part `.tar.*` suffixes). `None` when nothing matches.
    #[must_use]
    pub fn from_path(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
        if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
            Some(Self::TarGz)
        } else if name.ends_with(".tar.xz") || name.ends_with(".txz") {
            Some(Self::TarXz)
        } else if name.ends_with(".tar.zst") || name.ends_with(".tzst") {
            Some(Self::TarZst)
        } else if name.ends_with(".tar") {
            Some(Self::Tar)
        } else if name.ends_with(".zip") {
            Some(Self::Zip)
        } else {
            None
        }
    }

    /// The canonical extension (no leading dot).
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::Tar => "tar",
            Self::TarGz => "tar.gz",
            Self::TarXz => "tar.xz",
            Self::TarZst => "tar.zst",
        }
    }

    /// Whether this build can actually read + write the format. `false` for the
    /// codec-gated `.tar.xz`/`.tar.zst` (see the module docs).
    #[must_use]
    pub fn is_supported(self) -> bool {
        matches!(self, Self::Zip | Self::Tar | Self::TarGz)
    }

    /// Every format the surface may offer for **creating** an archive today.
    #[must_use]
    pub fn supported() -> &'static [ArchiveFormat] {
        &[Self::Zip, Self::Tar, Self::TarGz]
    }
}

/// One member of an archive, as reported by [`browse`] — enough to render a
/// listing without unpacking anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveEntry {
    /// The member's path *within* the archive (directories have the trailing
    /// slash trimmed).
    pub path: PathBuf,
    /// Uncompressed size in bytes.
    pub size: u64,
    /// Whether the member is a directory.
    pub is_dir: bool,
}

/// The honest typed error for a codec-gated format.
fn unsupported(format: ArchiveFormat) -> io::Error {
    let crate_hint = match format {
        ArchiveFormat::TarXz => "`xz2` (liblzma)",
        ArchiveFormat::TarZst => "`zstd`",
        _ => "a codec",
    };
    io::Error::new(
        io::ErrorKind::Unsupported,
        format!(
            "{} archives need the {crate_hint} crate, which is not in the airgapped \
             workspace lockfile (FILEMGR-3)",
            format.extension()
        ),
    )
}

// ═══════════════════════════════════════════════════════════════════════════
// Progress bookkeeping (shares the FILEMGR-2 Progress type + OpControl).
// ═══════════════════════════════════════════════════════════════════════════

/// Running progress tallies for a compress/extract, emitting the shared
/// [`Progress`] snapshot so archive ops render identically to copy/move/delete.
struct Prog {
    op_id: OpId,
    start: Instant,
    files_total: u64,
    bytes_total: u64,
    files_done: u64,
    bytes_done: u64,
    files_skipped: u64,
}

impl Prog {
    fn new(op_id: OpId, files_total: u64, bytes_total: u64) -> Self {
        Self {
            op_id,
            start: Instant::now(),
            files_total,
            bytes_total,
            files_done: 0,
            bytes_done: 0,
            files_skipped: 0,
        }
    }

    fn snapshot(&self, current: Option<PathBuf>) -> Progress {
        Progress {
            op_id: self.op_id,
            files_total: self.files_total,
            files_done: self.files_done,
            files_skipped: self.files_skipped,
            bytes_total: self.bytes_total,
            bytes_done: self.bytes_done,
            bytes_skipped: 0,
            current,
            elapsed: self.start.elapsed(),
        }
    }

    fn emit(&self, sink: &mut dyn FnMut(Progress), current: Option<PathBuf>) {
        sink(self.snapshot(current));
    }

    fn advance(&mut self, sink: &mut dyn FnMut(Progress), current: PathBuf, bytes: u64) {
        self.files_done += 1;
        self.bytes_done += bytes;
        self.emit(sink, Some(current));
    }

    fn skip(&mut self, sink: &mut dyn FnMut(Progress), current: PathBuf) {
        self.files_skipped += 1;
        self.emit(sink, Some(current));
    }
}

fn fresh_outcome(op_id: OpId) -> OpOutcome {
    OpOutcome {
        op_id,
        cancelled: false,
        items_completed: 0,
        items_skipped: 0,
        files_done: 0,
        bytes_done: 0,
        error: None,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Member collection (compress input).
// ═══════════════════════════════════════════════════════════════════════════

/// What a source path contributes to the archive.
enum MemberKind {
    File(Vec<u8>),
    Dir,
    Symlink(PathBuf),
}

struct Member {
    /// Path stored inside the archive (relative to the compress `base_dir`).
    rel: PathBuf,
    kind: MemberKind,
}

impl Member {
    fn bytes(&self) -> u64 {
        match &self.kind {
            MemberKind::File(data) => data.len() as u64,
            MemberKind::Dir | MemberKind::Symlink(_) => 0,
        }
    }
}

/// Walk every `item` (recursing directories) into a flat member list, naming
/// each member relative to `base_dir` (the folder the selection lives in, so
/// archiving `/home/u/proj` yields `proj`, `proj/a.txt`, …).
fn collect_members(
    ops: &dyn FileOps,
    items: &[PathBuf],
    base_dir: &Path,
) -> io::Result<Vec<Member>> {
    let mut out = Vec::new();
    for item in items {
        collect_one(ops, item, base_dir, &mut out)?;
    }
    Ok(out)
}

fn collect_one(
    ops: &dyn FileOps,
    path: &Path,
    base_dir: &Path,
    out: &mut Vec<Member>,
) -> io::Result<()> {
    let rel = path
        .strip_prefix(base_dir)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| {
            path.file_name()
                .map(PathBuf::from)
                .unwrap_or_else(|| path.to_path_buf())
        });
    if rel.as_os_str().is_empty() {
        return Ok(());
    }
    let meta = ops.symlink_metadata(path)?;
    if meta.is_symlink {
        let target = ops.read_link(path)?;
        out.push(Member {
            rel,
            kind: MemberKind::Symlink(target),
        });
    } else if meta.is_dir {
        out.push(Member {
            rel,
            kind: MemberKind::Dir,
        });
        let mut children = ops.read_dir(path)?;
        children.sort();
        for child in children {
            collect_one(ops, &child, base_dir, out)?;
        }
    } else {
        let data = ops.read_file(path)?;
        out.push(Member {
            rel,
            kind: MemberKind::File(data),
        });
    }
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// Public API: create / extract / browse.
// ═══════════════════════════════════════════════════════════════════════════

/// Compress `items` into a new archive at `archive` in the chosen `format`.
/// Members are named relative to `base_dir`. Emits [`Progress`] per member and
/// stops at a cancel checkpoint between members — because the archive is built
/// in memory and only written on success, a cancel leaves **no** half-archive.
///
/// This is the worker body the [`crate::opqueue::OpQueue`] runs for
/// [`crate::opqueue::OpKind::Compress`]; call it directly for a synchronous
/// compress. A codec-gated `format` returns an outcome carrying the honest
/// typed error, never a stub.
// Every argument is load-bearing: the FS seam, the four op parameters, and the
// three queue-context handles (control/op_id/sink) that make it ride the same
// progress path as [`crate::opqueue::execute`]. Bundling them would only hide
// the shape the queue already threads.
#[allow(clippy::too_many_arguments)]
pub fn compress(
    ops: &dyn FileOps,
    items: &[PathBuf],
    base_dir: &Path,
    archive: &Path,
    format: ArchiveFormat,
    control: &OpControl,
    op_id: OpId,
    sink: &mut dyn FnMut(Progress),
) -> OpOutcome {
    let mut outcome = fresh_outcome(op_id);
    if !format.is_supported() {
        outcome.error = Some(unsupported(format).to_string());
        return outcome;
    }
    let members = match collect_members(ops, items, base_dir) {
        Ok(m) => m,
        Err(err) => {
            outcome.error = Some(format!("read selection: {err}"));
            return outcome;
        }
    };
    let files_total = members.len() as u64;
    let bytes_total = members.iter().map(Member::bytes).sum();
    let mut prog = Prog::new(op_id, files_total, bytes_total);
    prog.emit(sink, None);

    let built = match format {
        ArchiveFormat::Tar => build_tar(&members, false, control, &mut prog, sink),
        ArchiveFormat::TarGz => build_tar(&members, true, control, &mut prog, sink),
        ArchiveFormat::Zip => build_zip(&members, control, &mut prog, sink),
        ArchiveFormat::TarXz | ArchiveFormat::TarZst => Err(unsupported(format)),
    };
    match built {
        Ok(Some(bytes)) => match ops.write_file(archive, &bytes) {
            Ok(()) => outcome.items_completed = items.len() as u64,
            Err(err) => outcome.error = Some(format!("write {}: {err}", archive.display())),
        },
        // Cancelled mid-build: nothing was written, so the archive never exists.
        Ok(None) => outcome.cancelled = true,
        Err(err) => outcome.error = Some(format!("build {}: {err}", archive.display())),
    }
    outcome.files_done = prog.files_done;
    outcome.bytes_done = prog.bytes_done;
    prog.emit(sink, None);
    outcome
}

/// Extract every member of `archive` into `dest_dir` (created if absent). This
/// is both "extract here" (pass the current folder) and "extract to" (pass the
/// chosen folder). Emits [`Progress`] per member; a member whose path would
/// escape `dest_dir` is refused (counted as skipped). Cancellable between
/// members (already-written members stay — extraction is additive, honestly
/// reported).
///
/// The worker body for [`crate::opqueue::OpKind::Extract`].
pub fn extract(
    ops: &dyn FileOps,
    archive: &Path,
    dest_dir: &Path,
    control: &OpControl,
    op_id: OpId,
    sink: &mut dyn FnMut(Progress),
) -> OpOutcome {
    let mut outcome = fresh_outcome(op_id);
    let bytes = match ops.read_file(archive) {
        Ok(b) => b,
        Err(err) => {
            outcome.error = Some(format!("read {}: {err}", archive.display()));
            return outcome;
        }
    };
    let format = ArchiveFormat::from_path(archive).unwrap_or_else(|| sniff_magic(&bytes));
    if !format.is_supported() {
        outcome.error = Some(unsupported(format).to_string());
        return outcome;
    }
    if let Err(err) = ops.create_dir_all(dest_dir) {
        outcome.error = Some(format!("create {}: {err}", dest_dir.display()));
        return outcome;
    }

    let (files_total, bytes_total) = match browse(ops, archive) {
        Ok(entries) => (
            entries.len() as u64,
            entries.iter().filter(|e| !e.is_dir).map(|e| e.size).sum(),
        ),
        Err(_) => (0, 0),
    };
    let mut prog = Prog::new(op_id, files_total, bytes_total);
    prog.emit(sink, None);

    let result = match format {
        ArchiveFormat::Zip => extract_zip(
            ops,
            &bytes,
            dest_dir,
            control,
            &mut prog,
            sink,
            &mut outcome,
        ),
        ArchiveFormat::Tar | ArchiveFormat::TarGz => extract_tar(
            ops,
            &bytes,
            dest_dir,
            control,
            &mut prog,
            sink,
            &mut outcome,
        ),
        ArchiveFormat::TarXz | ArchiveFormat::TarZst => Err(unsupported(format)),
    };
    match result {
        Ok(()) if !outcome.cancelled => outcome.items_completed = 1,
        Ok(()) => {}
        Err(err) => outcome.error = Some(format!("extract {}: {err}", archive.display())),
    }
    outcome.files_done = prog.files_done;
    outcome.bytes_done = prog.bytes_done;
    prog.emit(sink, None);
    outcome
}

/// List an archive's members without extracting anything — the double-click
/// "browse in place" verb. Cheap: reads the archive bytes once and parses the
/// index (the tar stream / the zip central directory).
pub fn browse(ops: &dyn FileOps, archive: &Path) -> io::Result<Vec<ArchiveEntry>> {
    let bytes = ops.read_file(archive)?;
    let format = ArchiveFormat::from_path(archive).unwrap_or_else(|| sniff_magic(&bytes));
    if !format.is_supported() {
        return Err(unsupported(format));
    }
    match format {
        ArchiveFormat::Zip => Ok(zip_central(&bytes)?
            .into_iter()
            .map(|e| ArchiveEntry {
                path: zip_path(&e.name),
                size: u64::from(e.uncomp_size),
                is_dir: e.is_dir,
            })
            .collect()),
        ArchiveFormat::Tar | ArchiveFormat::TarGz => browse_tar(&bytes),
        ArchiveFormat::TarXz | ArchiveFormat::TarZst => Err(unsupported(format)),
    }
}

/// Detect a format from leading magic bytes when the extension is missing or
/// lies. Defaults to plain `Tar` (a headerless tarball has no magic).
fn sniff_magic(bytes: &[u8]) -> ArchiveFormat {
    if bytes.starts_with(&[0x50, 0x4b, 0x03, 0x04]) || bytes.starts_with(&[0x50, 0x4b, 0x05, 0x06])
    {
        ArchiveFormat::Zip
    } else if bytes.starts_with(&[0x1f, 0x8b]) {
        ArchiveFormat::TarGz
    } else if bytes.starts_with(&[0xfd, 0x37, 0x7a, 0x58, 0x5a, 0x00]) {
        ArchiveFormat::TarXz
    } else if bytes.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        ArchiveFormat::TarZst
    } else {
        ArchiveFormat::Tar
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Path-traversal guard + parent-dir helper (shared by extract).
// ═══════════════════════════════════════════════════════════════════════════

/// Join a member's in-archive path under `dest`, refusing any escape: an
/// absolute path or a `..` component returns `None` (the member is skipped and
/// never written outside `dest`).
fn safe_join(dest: &Path, rel: &Path) -> Option<PathBuf> {
    let mut out = dest.to_path_buf();
    for comp in rel.components() {
        match comp {
            Component::Normal(c) => out.push(c),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    if out == dest {
        return None;
    }
    Some(out)
}

fn ensure_parent(ops: &dyn FileOps, target: &Path) -> io::Result<()> {
    if let Some(parent) = target.parent() {
        if parent.as_os_str().is_empty() {
            return Ok(());
        }
        if ops.symlink_metadata(parent).is_err() {
            ops.create_dir_all(parent)?;
        }
    }
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// tar / tar.gz (the `tar` + `flate2` crates).
// ═══════════════════════════════════════════════════════════════════════════

fn build_tar(
    members: &[Member],
    gzip: bool,
    control: &OpControl,
    prog: &mut Prog,
    sink: &mut dyn FnMut(Progress),
) -> io::Result<Option<Vec<u8>>> {
    if gzip {
        let mut builder = TarBuilder::new(GzEncoder::new(Vec::new(), Compression::default()));
        if append_members(&mut builder, members, control, prog, sink)?.is_none() {
            return Ok(None);
        }
        Ok(Some(builder.into_inner()?.finish()?))
    } else {
        let mut builder = TarBuilder::new(Vec::new());
        if append_members(&mut builder, members, control, prog, sink)?.is_none() {
            return Ok(None);
        }
        Ok(Some(builder.into_inner()?))
    }
}

/// Append every member to a tar builder, emitting progress and honouring
/// cancel between members. `Ok(None)` = cancelled before finishing.
fn append_members<W: Write>(
    builder: &mut TarBuilder<W>,
    members: &[Member],
    control: &OpControl,
    prog: &mut Prog,
    sink: &mut dyn FnMut(Progress),
) -> io::Result<Option<()>> {
    for m in members {
        if !control.proceed() {
            return Ok(None);
        }
        match &m.kind {
            MemberKind::File(data) => {
                let mut h = TarHeader::new_gnu();
                h.set_size(data.len() as u64);
                h.set_mode(0o644);
                h.set_mtime(0);
                h.set_entry_type(EntryType::Regular);
                h.set_cksum();
                builder.append_data(&mut h, &m.rel, data.as_slice())?;
                prog.advance(sink, m.rel.clone(), data.len() as u64);
            }
            MemberKind::Dir => {
                let mut h = TarHeader::new_gnu();
                h.set_size(0);
                h.set_mode(0o755);
                h.set_mtime(0);
                h.set_entry_type(EntryType::Directory);
                h.set_cksum();
                builder.append_data(&mut h, &m.rel, io::empty())?;
                prog.advance(sink, m.rel.clone(), 0);
            }
            MemberKind::Symlink(target) => {
                let mut h = TarHeader::new_gnu();
                h.set_size(0);
                h.set_mode(0o777);
                h.set_mtime(0);
                h.set_entry_type(EntryType::Symlink);
                h.set_link_name(target)?;
                h.set_cksum();
                builder.append_data(&mut h, &m.rel, io::empty())?;
                prog.advance(sink, m.rel.clone(), 0);
            }
        }
    }
    Ok(Some(()))
}

/// A tar byte stream, transparently gunzipped when the bytes begin with the
/// gzip magic — so a mislabelled `.tar` that is really gzip still reads.
fn tar_reader(bytes: &[u8]) -> Box<dyn Read + '_> {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        Box::new(GzDecoder::new(Cursor::new(bytes)))
    } else {
        Box::new(Cursor::new(bytes))
    }
}

fn browse_tar(bytes: &[u8]) -> io::Result<Vec<ArchiveEntry>> {
    let mut archive = tar::Archive::new(tar_reader(bytes));
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

fn extract_tar(
    ops: &dyn FileOps,
    bytes: &[u8],
    dest: &Path,
    control: &OpControl,
    prog: &mut Prog,
    sink: &mut dyn FnMut(Progress),
    outcome: &mut OpOutcome,
) -> io::Result<()> {
    let mut archive = tar::Archive::new(tar_reader(bytes));
    for entry in archive.entries()? {
        if !control.proceed() {
            outcome.cancelled = true;
            return Ok(());
        }
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let Some(target) = safe_join(dest, &path) else {
            prog.skip(sink, path);
            continue;
        };
        let etype = entry.header().entry_type();
        if etype.is_dir() {
            ops.create_dir_all(&target)?;
            prog.advance(sink, path, 0);
        } else if etype.is_symlink() {
            let link = entry
                .link_name()?
                .map(|c| c.into_owned())
                .unwrap_or_default();
            ensure_parent(ops, &target)?;
            let _ = ops.remove_file(&target);
            ops.symlink(&link, &target)?;
            prog.advance(sink, path, 0);
        } else if etype.is_hard_link() {
            let link = entry
                .link_name()?
                .map(|c| c.into_owned())
                .unwrap_or_default();
            match safe_join(dest, &link) {
                Some(src) => {
                    ensure_parent(ops, &target)?;
                    let _ = ops.remove_file(&target);
                    ops.hard_link(&src, &target)?;
                    prog.advance(sink, path, 0);
                }
                None => prog.skip(sink, path),
            }
        } else if etype.is_file() {
            let mut data = Vec::new();
            entry.read_to_end(&mut data)?;
            ensure_parent(ops, &target)?;
            ops.write_file(&target, &data)?;
            let n = data.len() as u64;
            prog.advance(sink, path, n);
        } else {
            // fifo / device / other special members are skipped honestly.
            prog.skip(sink, path);
        }
    }
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// zip — a self-contained STORE + DEFLATE reader/writer (no `zip` crate).
// ═══════════════════════════════════════════════════════════════════════════

const ZIP_LOCAL_SIG: [u8; 4] = [0x50, 0x4b, 0x03, 0x04];
const ZIP_CENTRAL_SIG: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];
const ZIP_EOCD_SIG: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
const ZIP_METHOD_STORE: u16 = 0;
const ZIP_METHOD_DEFLATE: u16 = 8;
/// DOS timestamp for 1980-01-01 00:00 (date `0x0021`, time `0x0000`) — a fixed
/// value keeps archive bytes deterministic (v1 does not preserve mtimes).
const ZIP_DOS_DATE: u16 = 0x0021;
const ZIP_DOS_TIME: u16 = 0x0000;

fn wr_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn wr_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn rd_u16(b: &[u8], at: usize) -> io::Result<u16> {
    b.get(at..at + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or_else(truncated)
}

fn rd_u32(b: &[u8], at: usize) -> io::Result<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(truncated)
}

fn truncated() -> io::Error {
    io::Error::new(io::ErrorKind::UnexpectedEof, "archive truncated")
}

fn corrupt(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_owned())
}

/// The in-archive name of a member, as zip bytes (forward-slash separated on
/// unix, trailing slash for a directory).
fn zip_name_bytes(rel: &Path, is_dir: bool) -> Vec<u8> {
    let mut v = rel.as_os_str().as_bytes().to_vec();
    if is_dir && v.last() != Some(&b'/') {
        v.push(b'/');
    }
    v
}

fn zip_path(name: &[u8]) -> PathBuf {
    let trimmed = name.strip_suffix(b"/").unwrap_or(name);
    PathBuf::from(OsStr::from_bytes(trimmed))
}

fn deflate(data: &[u8]) -> io::Result<Vec<u8>> {
    let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
    enc.write_all(data)?;
    enc.finish()
}

fn inflate(data: &[u8]) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    DeflateDecoder::new(data).read_to_end(&mut out)?;
    Ok(out)
}

/// CRC-32 (IEEE 802.3, the zip/gzip polynomial), computed inline so no CRC
/// crate is needed on the airgapped farm. Bitwise + reflected; the standard
/// `"123456789"` check value is `0xCBF4_3926` (unit-tested).
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// One prepared member ready to write into the zip (compressed once, up front).
struct ZipPrepared {
    name: Vec<u8>,
    method: u16,
    crc: u32,
    data: Vec<u8>,
    uncomp_size: u32,
    ext_attrs: u32,
    /// Progress accounting: the member's source path + uncompressed byte count.
    rel: PathBuf,
    src_bytes: u64,
}

fn prepare_zip_member(m: &Member) -> io::Result<ZipPrepared> {
    match &m.kind {
        MemberKind::Dir => Ok(ZipPrepared {
            name: zip_name_bytes(&m.rel, true),
            method: ZIP_METHOD_STORE,
            crc: 0,
            data: Vec::new(),
            uncomp_size: 0,
            ext_attrs: (0o040_755u32 << 16) | 0x10, // S_IFDIR | 0755, DOS dir bit
            rel: m.rel.clone(),
            src_bytes: 0,
        }),
        MemberKind::Symlink(target) => {
            let bytes = target.as_os_str().as_bytes().to_vec();
            let len = u32::try_from(bytes.len()).map_err(|_| corrupt("symlink target too long"))?;
            Ok(ZipPrepared {
                name: zip_name_bytes(&m.rel, false),
                method: ZIP_METHOD_STORE,
                crc: crc32(&bytes),
                data: bytes,
                uncomp_size: len,
                ext_attrs: 0o120_777u32 << 16, // S_IFLNK | 0777
                rel: m.rel.clone(),
                src_bytes: 0,
            })
        }
        MemberKind::File(raw) => {
            let uncomp_size = u32::try_from(raw.len())
                .map_err(|_| corrupt("zip member exceeds 4 GiB (no ZIP64)"))?;
            let crc = crc32(raw);
            let deflated = deflate(raw)?;
            // Only keep the compressed form when it actually shrank the data.
            let (method, data) = if deflated.len() < raw.len() {
                (ZIP_METHOD_DEFLATE, deflated)
            } else {
                (ZIP_METHOD_STORE, raw.clone())
            };
            Ok(ZipPrepared {
                name: zip_name_bytes(&m.rel, false),
                method,
                crc,
                data,
                uncomp_size,
                ext_attrs: 0o100_644u32 << 16, // S_IFREG | 0644
                rel: m.rel.clone(),
                src_bytes: raw.len() as u64,
            })
        }
    }
}

fn build_zip(
    members: &[Member],
    control: &OpControl,
    prog: &mut Prog,
    sink: &mut dyn FnMut(Progress),
) -> io::Result<Option<Vec<u8>>> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    let mut count: u16 = 0;

    for m in members {
        if !control.proceed() {
            return Ok(None);
        }
        let p = prepare_zip_member(m)?;
        let comp_size = u32::try_from(p.data.len())
            .map_err(|_| corrupt("compressed zip member exceeds 4 GiB (no ZIP64)"))?;
        let local_offset = u32::try_from(out.len())
            .map_err(|_| corrupt("zip archive exceeds 4 GiB (no ZIP64)"))?;
        let name_len =
            u16::try_from(p.name.len()).map_err(|_| corrupt("zip member name too long"))?;

        // Local file header.
        out.extend_from_slice(&ZIP_LOCAL_SIG);
        wr_u16(&mut out, 20); // version needed to extract (2.0 = DEFLATE)
        wr_u16(&mut out, 0); // general-purpose flags (none; sizes are known)
        wr_u16(&mut out, p.method);
        wr_u16(&mut out, ZIP_DOS_TIME);
        wr_u16(&mut out, ZIP_DOS_DATE);
        wr_u32(&mut out, p.crc);
        wr_u32(&mut out, comp_size);
        wr_u32(&mut out, p.uncomp_size);
        wr_u16(&mut out, name_len);
        wr_u16(&mut out, 0); // extra length
        out.extend_from_slice(&p.name);
        out.extend_from_slice(&p.data);

        // Central-directory header (the authoritative index).
        central.extend_from_slice(&ZIP_CENTRAL_SIG);
        wr_u16(&mut central, 0x031E); // version made by: unix (3), 3.0
        wr_u16(&mut central, 20);
        wr_u16(&mut central, 0);
        wr_u16(&mut central, p.method);
        wr_u16(&mut central, ZIP_DOS_TIME);
        wr_u16(&mut central, ZIP_DOS_DATE);
        wr_u32(&mut central, p.crc);
        wr_u32(&mut central, comp_size);
        wr_u32(&mut central, p.uncomp_size);
        wr_u16(&mut central, name_len);
        wr_u16(&mut central, 0); // extra length
        wr_u16(&mut central, 0); // comment length
        wr_u16(&mut central, 0); // disk number start
        wr_u16(&mut central, 0); // internal attributes
        wr_u32(&mut central, p.ext_attrs);
        wr_u32(&mut central, local_offset);
        central.extend_from_slice(&p.name);

        count = count
            .checked_add(1)
            .ok_or_else(|| corrupt("too many zip members"))?;
        prog.advance(sink, p.rel, p.src_bytes);
    }

    let cd_offset =
        u32::try_from(out.len()).map_err(|_| corrupt("zip archive exceeds 4 GiB (no ZIP64)"))?;
    let cd_size =
        u32::try_from(central.len()).map_err(|_| corrupt("zip directory exceeds 4 GiB"))?;
    out.extend_from_slice(&central);

    // End-of-central-directory record.
    out.extend_from_slice(&ZIP_EOCD_SIG);
    wr_u16(&mut out, 0); // this disk number
    wr_u16(&mut out, 0); // disk with central directory
    wr_u16(&mut out, count); // entries on this disk
    wr_u16(&mut out, count); // total entries
    wr_u32(&mut out, cd_size);
    wr_u32(&mut out, cd_offset);
    wr_u16(&mut out, 0); // comment length

    Ok(Some(out))
}

/// A parsed central-directory record.
struct ZipCentral {
    name: Vec<u8>,
    method: u16,
    crc: u32,
    comp_size: u32,
    uncomp_size: u32,
    local_offset: u32,
    is_dir: bool,
    is_symlink: bool,
}

/// Locate the end-of-central-directory record by scanning backwards (the record
/// is at the very end when there is no archive comment, but a comment may push
/// it up to 64 KiB earlier).
fn find_eocd(bytes: &[u8]) -> io::Result<usize> {
    if bytes.len() < 22 {
        return Err(truncated());
    }
    let last = bytes.len() - 22;
    let earliest = bytes.len().saturating_sub(22 + 0xFFFF);
    for pos in (earliest..=last).rev() {
        if bytes[pos..pos + 4] == ZIP_EOCD_SIG {
            return Ok(pos);
        }
    }
    Err(corrupt("no zip end-of-central-directory record"))
}

fn zip_central(bytes: &[u8]) -> io::Result<Vec<ZipCentral>> {
    let eocd = find_eocd(bytes)?;
    let total = rd_u16(bytes, eocd + 10)?;
    let mut pos = rd_u32(bytes, eocd + 16)? as usize;
    let mut out = Vec::with_capacity(total as usize);
    for _ in 0..total {
        if bytes.get(pos..pos + 4) != Some(&ZIP_CENTRAL_SIG[..]) {
            return Err(corrupt("bad zip central-directory header"));
        }
        let method = rd_u16(bytes, pos + 10)?;
        let crc = rd_u32(bytes, pos + 16)?;
        let comp_size = rd_u32(bytes, pos + 20)?;
        let uncomp_size = rd_u32(bytes, pos + 24)?;
        let name_len = rd_u16(bytes, pos + 28)? as usize;
        let extra_len = rd_u16(bytes, pos + 30)? as usize;
        let comment_len = rd_u16(bytes, pos + 32)? as usize;
        let ext_attrs = rd_u32(bytes, pos + 38)?;
        let local_offset = rd_u32(bytes, pos + 42)?;
        let name = bytes
            .get(pos + 46..pos + 46 + name_len)
            .ok_or_else(truncated)?
            .to_vec();
        let unix_mode = ext_attrs >> 16;
        let is_dir = name.last() == Some(&b'/') || (unix_mode & 0o170_000) == 0o040_000;
        let is_symlink = (unix_mode & 0o170_000) == 0o120_000;
        out.push(ZipCentral {
            name,
            method,
            crc,
            comp_size,
            uncomp_size,
            local_offset,
            is_dir,
            is_symlink,
        });
        pos += 46 + name_len + extra_len + comment_len;
    }
    Ok(out)
}

/// Read + decompress one member's data from its local header, verifying the CRC.
fn zip_member_data(bytes: &[u8], e: &ZipCentral) -> io::Result<Vec<u8>> {
    if e.is_dir {
        return Ok(Vec::new());
    }
    let lo = e.local_offset as usize;
    if bytes.get(lo..lo + 4) != Some(&ZIP_LOCAL_SIG[..]) {
        return Err(corrupt("bad zip local file header"));
    }
    let name_len = rd_u16(bytes, lo + 26)? as usize;
    let extra_len = rd_u16(bytes, lo + 28)? as usize;
    let start = lo + 30 + name_len + extra_len;
    let raw = bytes
        .get(start..start + e.comp_size as usize)
        .ok_or_else(truncated)?;
    let data = match e.method {
        ZIP_METHOD_STORE => raw.to_vec(),
        ZIP_METHOD_DEFLATE => inflate(raw)?,
        other => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("unsupported zip compression method {other}"),
            ))
        }
    };
    if crc32(&data) != e.crc {
        return Err(corrupt("zip entry CRC mismatch"));
    }
    Ok(data)
}

fn extract_zip(
    ops: &dyn FileOps,
    bytes: &[u8],
    dest: &Path,
    control: &OpControl,
    prog: &mut Prog,
    sink: &mut dyn FnMut(Progress),
    outcome: &mut OpOutcome,
) -> io::Result<()> {
    let entries = zip_central(bytes)?;
    for e in entries {
        if !control.proceed() {
            outcome.cancelled = true;
            return Ok(());
        }
        let rel = zip_path(&e.name);
        let Some(target) = safe_join(dest, &rel) else {
            prog.skip(sink, rel);
            continue;
        };
        if e.is_dir {
            ops.create_dir_all(&target)?;
            prog.advance(sink, rel, 0);
        } else if e.is_symlink {
            let data = zip_member_data(bytes, &e)?;
            let link_target = PathBuf::from(OsStr::from_bytes(&data));
            ensure_parent(ops, &target)?;
            let _ = ops.remove_file(&target);
            ops.symlink(&link_target, &target)?;
            prog.advance(sink, rel, 0);
        } else {
            let data = zip_member_data(bytes, &e)?;
            ensure_parent(ops, &target)?;
            ops.write_file(&target, &data)?;
            let n = data.len() as u64;
            prog.advance(sink, rel, n);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fileops::{FakeFileOps, LiveFileOps};

    // ── fixtures ─────────────────────────────────────────────────────────────

    /// A fake FS seeded with a `/src/proj` tree + an empty `/out` for archives
    /// and `/dest` for extraction.
    fn scratch() -> FakeFileOps {
        let fs = FakeFileOps::new();
        fs.create_dir(Path::new("/src")).expect("mkdir /src");
        fs.create_dir(Path::new("/out")).expect("mkdir /out");
        fs.create_dir(Path::new("/dest")).expect("mkdir /dest");
        fs.create_dir(Path::new("/src/proj")).expect("mkdir proj");
        fs.create_dir(Path::new("/src/proj/sub"))
            .expect("mkdir sub");
        fs.seed_file("/src/proj/top.txt", b"top-level")
            .expect("seed");
        fs.seed_file("/src/proj/sub/inner.txt", b"nested-content")
            .expect("seed");
        fs
    }

    fn noop_control() -> OpControl {
        OpControl::new()
    }

    /// Run a compress + collect the progress snapshots.
    fn do_compress(
        fs: &FakeFileOps,
        archive: &str,
        format: ArchiveFormat,
    ) -> (OpOutcome, Vec<Progress>) {
        let mut seen = Vec::new();
        let outcome = {
            let mut sink = |p: Progress| seen.push(p);
            compress(
                fs,
                &[PathBuf::from("/src/proj")],
                Path::new("/src"),
                Path::new(archive),
                format,
                &noop_control(),
                1,
                &mut sink,
            )
        };
        (outcome, seen)
    }

    fn do_extract(fs: &FakeFileOps, archive: &str, dest: &str) -> OpOutcome {
        let mut sink = |_p: Progress| {};
        extract(
            fs,
            Path::new(archive),
            Path::new(dest),
            &noop_control(),
            2,
            &mut sink,
        )
    }

    /// Full create → browse → extract round-trip for one format.
    fn roundtrip(format: ArchiveFormat, archive: &str) {
        let fs = scratch();
        let (outcome, seen) = do_compress(&fs, archive, format);
        assert!(
            outcome.error.is_none() && !outcome.cancelled,
            "{format:?} compress failed: {:?}",
            outcome.error
        );
        assert_eq!(outcome.items_completed, 1, "{format:?}: one item archived");
        assert!(fs.exists(archive), "{format:?}: archive written");
        // Members: proj, proj/sub, proj/top.txt, proj/sub/inner.txt = 4.
        assert_eq!(outcome.files_done, 4, "{format:?}: 4 members");
        // Progress reached the totals and never went backwards.
        let last = seen.last().expect("progress emitted");
        assert_eq!(last.files_total, 4);
        assert!(seen.windows(2).all(|w| w[1].files_done >= w[0].files_done));

        // Browse without extracting.
        let entries = browse(&fs, Path::new(archive)).expect("browse");
        let names: Vec<String> = entries
            .iter()
            .map(|e| e.path.display().to_string())
            .collect();
        assert!(
            names.iter().any(|n| n == "proj/top.txt"),
            "{format:?}: browse lists proj/top.txt, got {names:?}"
        );
        assert!(
            names.iter().any(|n| n == "proj/sub/inner.txt"),
            "{format:?}: browse lists nested file"
        );
        let top = entries
            .iter()
            .find(|e| e.path == Path::new("proj/top.txt"))
            .expect("top entry");
        assert_eq!(
            top.size, 9,
            "{format:?}: uncompressed size of \"top-level\""
        );
        assert!(!top.is_dir);

        // Extract to a fresh destination and verify contents byte-for-byte.
        let out = do_extract(&fs, archive, "/dest");
        assert!(
            out.error.is_none() && !out.cancelled,
            "{format:?} extract failed: {:?}",
            out.error
        );
        assert_eq!(out.items_completed, 1);
        assert_eq!(
            fs.read("/dest/proj/top.txt").expect("read top"),
            b"top-level",
            "{format:?}: extracted top.txt"
        );
        assert_eq!(
            fs.read("/dest/proj/sub/inner.txt").expect("read inner"),
            b"nested-content",
            "{format:?}: extracted nested file"
        );
    }

    #[test]
    fn zip_create_browse_extract_roundtrip() {
        roundtrip(ArchiveFormat::Zip, "/out/proj.zip");
    }

    #[test]
    fn tar_create_browse_extract_roundtrip() {
        roundtrip(ArchiveFormat::Tar, "/out/proj.tar");
    }

    #[test]
    fn targz_create_browse_extract_roundtrip() {
        roundtrip(ArchiveFormat::TarGz, "/out/proj.tar.gz");
    }

    // ── format detection ─────────────────────────────────────────────────────

    #[test]
    fn format_from_extension() {
        let cases = [
            ("a.zip", Some(ArchiveFormat::Zip)),
            ("a.tar", Some(ArchiveFormat::Tar)),
            ("a.tar.gz", Some(ArchiveFormat::TarGz)),
            ("a.TGZ", Some(ArchiveFormat::TarGz)),
            ("a.tar.xz", Some(ArchiveFormat::TarXz)),
            ("a.tzst", Some(ArchiveFormat::TarZst)),
            ("a.txt", None),
            ("noext", None),
        ];
        for (name, want) in cases {
            assert_eq!(ArchiveFormat::from_path(Path::new(name)), want, "{name}");
        }
    }

    #[test]
    fn only_zip_tar_targz_are_supported() {
        assert!(ArchiveFormat::Zip.is_supported());
        assert!(ArchiveFormat::Tar.is_supported());
        assert!(ArchiveFormat::TarGz.is_supported());
        assert!(!ArchiveFormat::TarXz.is_supported());
        assert!(!ArchiveFormat::TarZst.is_supported());
        assert_eq!(ArchiveFormat::supported().len(), 3);
    }

    // ── the CRC-32 primitive against the canonical check vector ──────────────

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    // ── zip container is spec-conformant (interop without a reference crate) ──

    #[test]
    fn zip_bytes_carry_the_spec_signatures_and_stored_payload() {
        let fs = scratch();
        // A tiny, incompressible payload → stored verbatim in the zip bytes.
        fs.seed_file("/src/proj/raw.bin", b"\x00\x01\x02\x03")
            .expect("seed");
        let (outcome, _) = do_compress(&fs, "/out/proj.zip", ArchiveFormat::Zip);
        assert!(outcome.error.is_none());
        let bytes = fs.read("/out/proj.zip").expect("read zip");
        assert!(
            bytes.starts_with(&ZIP_LOCAL_SIG),
            "starts with a local file header"
        );
        // The EOCD lives at the very end (no archive comment).
        assert_eq!(&bytes[bytes.len() - 22..bytes.len() - 18], &ZIP_EOCD_SIG);
        // A STORED member's raw bytes appear verbatim somewhere in the archive.
        assert!(
            bytes.windows(4).any(|w| w == b"\x00\x01\x02\x03"),
            "stored payload present verbatim"
        );
    }

    #[test]
    fn zip_deflates_compressible_data() {
        let fs = scratch();
        let big = vec![b'A'; 4096];
        fs.seed_file("/src/proj/big.txt", &big).expect("seed");
        let (outcome, _) = do_compress(&fs, "/out/proj.zip", ArchiveFormat::Zip);
        assert!(outcome.error.is_none());
        let zip = fs.read("/out/proj.zip").expect("read");
        assert!(
            (zip.len() as u64) < 4096,
            "highly-compressible data deflated the archive below the raw size ({} bytes)",
            zip.len()
        );
        // And it still round-trips exactly.
        let out = do_extract(&fs, "/out/proj.zip", "/dest");
        assert!(out.error.is_none());
        assert_eq!(fs.read("/dest/proj/big.txt").expect("read"), big);
    }

    // ── path-traversal guard ─────────────────────────────────────────────────

    /// Hand-write a single-entry USTAR archive with an arbitrary member name —
    /// `tar::Builder` refuses to *create* a `..` path, so a hostile archive must
    /// be assembled at the byte level to exercise the *extract* guard.
    fn raw_tar_bytes(name: &str, data: &[u8]) -> Vec<u8> {
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[100..108].copy_from_slice(b"0000644\0");
        h[108..116].copy_from_slice(b"0000000\0");
        h[116..124].copy_from_slice(b"0000000\0");
        h[124..136].copy_from_slice(format!("{:011o}\0", data.len()).as_bytes());
        h[136..148].copy_from_slice(b"00000000000\0");
        h[156] = b'0';
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        for b in &mut h[148..156] {
            *b = b' ';
        }
        let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
        h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        let mut out = h.to_vec();
        out.extend_from_slice(data);
        out.resize(out.len() + (512 - data.len() % 512) % 512, 0);
        out.resize(out.len() + 1024, 0);
        out
    }

    #[test]
    fn extract_refuses_path_traversal() {
        let fs = scratch();
        let evil = raw_tar_bytes("../escaped.txt", b"pwned");
        fs.write_file(Path::new("/out/evil.tar"), &evil)
            .expect("seed evil tar");
        let out = do_extract(&fs, "/out/evil.tar", "/dest");
        // The op completes (no hard error) but the escaping member was refused.
        assert!(out.error.is_none(), "extract itself did not error");
        assert!(
            !fs.exists("/escaped.txt"),
            "the traversal member was NOT written outside dest"
        );
        assert!(!fs.exists("/dest/escaped.txt"));
    }

    #[test]
    fn safe_join_rejects_escapes_and_accepts_normal_paths() {
        let dest = Path::new("/dest");
        assert_eq!(
            safe_join(dest, Path::new("a/b.txt")),
            Some(PathBuf::from("/dest/a/b.txt"))
        );
        assert_eq!(safe_join(dest, Path::new("../x")), None);
        assert_eq!(safe_join(dest, Path::new("/abs")), None);
        assert_eq!(safe_join(dest, Path::new("a/../../x")), None);
        assert_eq!(safe_join(dest, Path::new("")), None);
    }

    // ── honest gating of the codec-less formats ──────────────────────────────

    #[test]
    fn gated_formats_error_honestly_not_stub() {
        let fs = scratch();
        // Compress to .tar.xz → typed error naming the missing crate.
        let (outcome, _) = do_compress(&fs, "/out/proj.tar.xz", ArchiveFormat::TarXz);
        let msg = outcome.error.expect("tar.xz compress must error");
        assert!(msg.contains("xz2"), "names the xz2 crate: {msg}");
        assert!(!fs.exists("/out/proj.tar.xz"), "no archive was written");

        let (zst, _) = do_compress(&fs, "/out/proj.tar.zst", ArchiveFormat::TarZst);
        let zmsg = zst.error.expect("tar.zst compress must error");
        assert!(zmsg.contains("zstd"), "names the zstd crate: {zmsg}");

        // Browse of a gated format is a typed Unsupported error.
        fs.seed_file("/out/x.tar.zst", b"not-a-real-zst")
            .expect("seed");
        let err = browse(&fs, Path::new("/out/x.tar.zst")).expect_err("browse gated");
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
    }

    // ── cancel leaves no half-archive ────────────────────────────────────────

    #[test]
    fn cancel_during_compress_writes_no_archive() {
        let fs = scratch();
        let control = OpControl::new();
        let trigger = control.clone();
        let mut sink = |p: Progress| {
            // Cancel after the first member is appended.
            if p.files_done == 1 {
                trigger.cancel();
            }
        };
        let outcome = compress(
            &fs,
            &[PathBuf::from("/src/proj")],
            Path::new("/src"),
            Path::new("/out/proj.zip"),
            ArchiveFormat::Zip,
            &control,
            9,
            &mut sink,
        );
        assert!(outcome.cancelled, "reported cancelled");
        assert_eq!(outcome.items_completed, 0, "nothing claimed complete");
        assert!(
            !fs.exists("/out/proj.zip"),
            "a cancelled compress leaves no half-archive"
        );
    }

    // ── symlink members survive a tar round-trip ─────────────────────────────

    #[test]
    fn tar_preserves_a_symlink_member() {
        let fs = scratch();
        fs.symlink(Path::new("top.txt"), Path::new("/src/proj/link"))
            .expect("symlink");
        let (outcome, _) = do_compress(&fs, "/out/proj.tar", ArchiveFormat::Tar);
        assert!(outcome.error.is_none());
        let out = do_extract(&fs, "/out/proj.tar", "/dest");
        assert!(out.error.is_none(), "extract: {:?}", out.error);
        let st = fs
            .symlink_metadata(Path::new("/dest/proj/link"))
            .expect("stat link");
        assert!(st.is_symlink, "extracted member is a symlink");
        assert_eq!(
            fs.read_link(Path::new("/dest/proj/link"))
                .expect("readlink"),
            PathBuf::from("top.txt")
        );
    }

    // ── the same round-trip on a real filesystem (LiveFileOps) ───────────────

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "mde-files-archive-{}-{}-{:?}",
                tag,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&base).expect("create temp dir");
            Self(base)
        }
        fn path(&self, rel: &str) -> PathBuf {
            self.0.join(rel)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn live_zip_roundtrip_on_a_real_fs() {
        let ops = LiveFileOps::new();
        let d = TempDir::new("zip");
        std::fs::create_dir_all(d.path("src/proj/sub")).expect("mkdir tree");
        std::fs::write(d.path("src/proj/top.txt"), b"top-level").expect("write");
        std::fs::write(d.path("src/proj/sub/inner.txt"), b"nested").expect("write");
        std::fs::create_dir_all(d.path("out")).expect("mkdir out");

        let archive = d.path("out/proj.zip");
        let mut sink = |_p: Progress| {};
        let outcome = compress(
            &ops,
            &[d.path("src/proj")],
            &d.path("src"),
            &archive,
            ArchiveFormat::Zip,
            &noop_control(),
            1,
            &mut sink,
        );
        assert!(outcome.error.is_none(), "compress: {:?}", outcome.error);
        assert!(archive.exists());

        let entries = browse(&ops, &archive).expect("browse");
        assert!(entries.iter().any(|e| e.path == Path::new("proj/top.txt")));

        let dest = d.path("dest");
        let out = extract(&ops, &archive, &dest, &noop_control(), 2, &mut |_p| {});
        assert!(out.error.is_none(), "extract: {:?}", out.error);
        assert_eq!(
            std::fs::read(dest.join("proj/top.txt")).expect("read"),
            b"top-level"
        );
        assert_eq!(
            std::fs::read(dest.join("proj/sub/inner.txt")).expect("read"),
            b"nested"
        );
    }
}
