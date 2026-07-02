//! FILEMGR-1 — the `FileOps` backend core.
//!
//! The injectable POSIX-operation surface the Files app runs every mutation
//! through. `mde-files-egui` (the render+request surface) never touches
//! `std::fs` directly; it asks a `FileOps` to copy / move / delete / mkdir /
//! symlink / chmod / chown / … so the whole operation set is one testable,
//! swappable seam.
//!
//! Two impls ship:
//!   * [`LiveFileOps`] — the real filesystem, via `std::fs` + `std::os::unix::fs`.
//!     Every call is a typed syscall wrapper (§9 — no raw shell, no `Command`),
//!     and `unsafe_code` is forbidden crate-wide so these are all the safe std
//!     wrappers (`chown`, `symlink`, `set_times`, `PermissionsExt`).
//!   * [`FakeFileOps`] — an in-memory filesystem for unit tests. Faithful enough
//!     to exercise hardlink nlink-counting, symlink (no-follow) semantics, and a
//!     `chown`-without-privilege honest error, with zero disk I/O.
//!
//! Why not the `nix` crate (which the design sketch named)? It isn't in the
//! airgapped workspace lockfile, and modern `std::os::unix::fs` already exposes
//! the entire set — `chown` (1.73), `symlink`, `File::set_times`/`FileTimes`
//! (1.75), `PermissionsExt`, `hard_link` — as **safe** wrappers. Pulling `nix`
//! would add a fetch + an `unsafe`-carrying dep for no capability we lack.
//!
//! Errors are plain [`std::io::Error`]: they carry the real `errno`, so an
//! unprivileged `chown` surfaces as [`std::io::ErrorKind::PermissionDenied`]
//! (the honest typed error the lock demands — never a faked success).

use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Metadata snapshot returned by [`FileOps::metadata`] /
/// [`FileOps::symlink_metadata`]. A render-agnostic projection of the fields the
/// Properties dialog + the listing view actually read — no live `std::fs`
/// handle escapes the trait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStat {
    /// A directory.
    pub is_dir: bool,
    /// A regular file.
    pub is_file: bool,
    /// A symbolic link (only ever `true` from [`FileOps::symlink_metadata`],
    /// which does not follow the link).
    pub is_symlink: bool,
    /// Size in bytes (for a symlink, the length of the target path).
    pub len: u64,
    /// Unix permission bits (`st_mode & 0o7777`).
    pub mode: u32,
    /// Owning user id.
    pub uid: u32,
    /// Owning group id.
    pub gid: u32,
    /// Last-access time, when the platform reports it.
    pub accessed: Option<SystemTime>,
    /// Last-modification time, when the platform reports it.
    pub modified: Option<SystemTime>,
}

/// The injectable POSIX operation surface. Every method is a single logical
/// filesystem action; the recursive helpers ([`FileOps::copy`],
/// [`FileOps::remove`]) are provided in terms of the primitives so both impls
/// share the tree-walk.
pub trait FileOps {
    // ── inspection ──────────────────────────────────────────────────────────

    /// Stat following symlinks (the `stat(2)` shape).
    fn metadata(&self, path: &Path) -> io::Result<FileStat>;
    /// Stat **not** following symlinks (the `lstat(2)` shape).
    fn symlink_metadata(&self, path: &Path) -> io::Result<FileStat>;
    /// The absolute paths of a directory's immediate children (unordered).
    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>>;
    /// The target of a symbolic link.
    fn read_link(&self, path: &Path) -> io::Result<PathBuf>;

    // ── primitives ──────────────────────────────────────────────────────────

    /// Copy one **regular file** `src` → `dst`, returning the byte count. Callers
    /// wanting directory-recursion use [`FileOps::copy`].
    fn copy_file(&self, src: &Path, dst: &Path) -> io::Result<u64>;
    /// Rename / move (a single `rename(2)`; same-filesystem move is atomic).
    fn rename(&self, src: &Path, dst: &Path) -> io::Result<()>;
    /// Unlink a single file or symlink (never a directory).
    fn remove_file(&self, path: &Path) -> io::Result<()>;
    /// Remove a directory and everything under it.
    fn remove_dir_all(&self, path: &Path) -> io::Result<()>;
    /// Create a single directory; errors if the parent is missing or the path
    /// already exists.
    fn create_dir(&self, path: &Path) -> io::Result<()>;
    /// Create a directory and any missing parents.
    fn create_dir_all(&self, path: &Path) -> io::Result<()>;
    /// Create a new empty file; errors if it already exists (`O_CREAT|O_EXCL`).
    fn create_file(&self, path: &Path) -> io::Result<()>;
    /// Create a symbolic link `link` pointing at `target`.
    fn symlink(&self, target: &Path, link: &Path) -> io::Result<()>;
    /// Create a hard link `link` to the existing file `target`.
    fn hard_link(&self, target: &Path, link: &Path) -> io::Result<()>;
    /// `chmod` — set the permission bits (`mode & 0o7777`).
    fn set_permissions(&self, path: &Path, mode: u32) -> io::Result<()>;
    /// `chown` — set owner and/or group (`None` leaves that field unchanged).
    /// Returns [`io::ErrorKind::PermissionDenied`] when the caller lacks the
    /// privilege — an honest typed error, never a silent success (§7).
    fn chown(&self, path: &Path, uid: Option<u32>, gid: Option<u32>) -> io::Result<()>;
    /// Set the access + modification timestamps.
    fn set_times(&self, path: &Path, accessed: SystemTime, modified: SystemTime) -> io::Result<()>;

    // ── provided recursive helpers ──────────────────────────────────────────

    /// Copy a file, symlink, or directory (recursively) `src` → `dst`. A symlink
    /// is recreated as a symlink (its target is not dereferenced); a directory is
    /// created at `dst` and its children copied. Returns the total bytes of
    /// regular-file payload copied.
    fn copy(&self, src: &Path, dst: &Path) -> io::Result<u64> {
        let meta = self.symlink_metadata(src)?;
        if meta.is_symlink {
            let target = self.read_link(src)?;
            self.symlink(&target, dst)?;
            Ok(0)
        } else if meta.is_dir {
            self.create_dir(dst)?;
            let mut total = 0;
            for child in self.read_dir(src)? {
                let name = child.file_name().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "child has no file name")
                })?;
                total += self.copy(&child, &dst.join(name))?;
            }
            Ok(total)
        } else {
            self.copy_file(src, dst)
        }
    }

    /// Remove a path whatever its kind: a directory (recursively) or a
    /// file/symlink (unlinked, never followed).
    fn remove(&self, path: &Path) -> io::Result<()> {
        let meta = self.symlink_metadata(path)?;
        if meta.is_dir {
            self.remove_dir_all(path)
        } else {
            self.remove_file(path)
        }
    }

    /// Duplicate `path` to a fresh sibling ("`name copy.ext`", then
    /// "`name copy 2.ext`", …) that does not yet exist, returning the new path.
    /// The Files "Duplicate" verb.
    fn duplicate(&self, path: &Path) -> io::Result<PathBuf> {
        // Ensure the source exists before minting a name.
        self.symlink_metadata(path)?;
        let dst = free_duplicate_name(path, |candidate| self.symlink_metadata(candidate).is_ok());
        self.copy(path, &dst)?;
        Ok(dst)
    }
}

/// Pick the first "`<stem> copy<n>.<ext>`" sibling of `path` for which
/// `exists(candidate)` is `false`. `n` is empty for the first copy, then ` 2`,
/// ` 3`, … The extension and parent directory are preserved.
///
/// `pub(crate)` so the FILEMGR-2 conflict engine ([`crate::opqueue`]) mints the
/// same "Keep both" auto-rename the Duplicate verb uses — one naming convention
/// across the app (§6 reuse, not a re-implementation).
pub(crate) fn free_duplicate_name(path: &Path, exists: impl Fn(&Path) -> bool) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path.extension().map(|e| e.to_string_lossy().into_owned());
    let build = |suffix: String| -> PathBuf {
        let name = match &ext {
            Some(e) => format!("{stem} copy{suffix}.{e}"),
            None => format!("{stem} copy{suffix}"),
        };
        parent.join(name)
    };
    let first = build(String::new());
    if !exists(&first) {
        return first;
    }
    for n in 2..=u32::MAX {
        let candidate = build(format!(" {n}"));
        if !exists(&candidate) {
            return candidate;
        }
    }
    // Unreachable in practice (2^32 collisions); fall back to the first name.
    first
}

// ═══════════════════════════════════════════════════════════════════════════
// LiveFileOps — the real filesystem.
// ═══════════════════════════════════════════════════════════════════════════

/// The production [`FileOps`]: every call is a `std::fs` / `std::os::unix::fs`
/// syscall wrapper. Stateless, cheap to construct, `Copy`.
#[derive(Debug, Clone, Copy, Default)]
pub struct LiveFileOps;

impl LiveFileOps {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl FileOps for LiveFileOps {
    fn metadata(&self, path: &Path) -> io::Result<FileStat> {
        stat_from(&std::fs::metadata(path)?)
    }

    fn symlink_metadata(&self, path: &Path) -> io::Result<FileStat> {
        stat_from(&std::fs::symlink_metadata(path)?)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(path)? {
            out.push(entry?.path());
        }
        Ok(out)
    }

    fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        std::fs::read_link(path)
    }

    fn copy_file(&self, src: &Path, dst: &Path) -> io::Result<u64> {
        std::fs::copy(src, dst)
    }

    fn rename(&self, src: &Path, dst: &Path) -> io::Result<()> {
        std::fs::rename(src, dst)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_file(path)
    }

    fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_dir_all(path)
    }

    fn create_dir(&self, path: &Path) -> io::Result<()> {
        std::fs::create_dir(path)
    }

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        std::fs::create_dir_all(path)
    }

    fn create_file(&self, path: &Path) -> io::Result<()> {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map(|_| ())
    }

    fn symlink(&self, target: &Path, link: &Path) -> io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    fn hard_link(&self, target: &Path, link: &Path) -> io::Result<()> {
        std::fs::hard_link(target, link)
    }

    fn set_permissions(&self, path: &Path, mode: u32) -> io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
    }

    fn chown(&self, path: &Path, uid: Option<u32>, gid: Option<u32>) -> io::Result<()> {
        // Safe std wrapper (stable since 1.73). EPERM → PermissionDenied, which
        // is the honest typed error the lock requires for an unprivileged chown.
        std::os::unix::fs::chown(path, uid, gid)
    }

    fn set_times(&self, path: &Path, accessed: SystemTime, modified: SystemTime) -> io::Result<()> {
        // Open read-only (works for files and directories on Linux); futimens
        // via `File::set_times` needs only the fd + ownership, not write access.
        let file = std::fs::File::open(path)?;
        let times = std::fs::FileTimes::new()
            .set_accessed(accessed)
            .set_modified(modified);
        file.set_times(times)
    }
}

fn stat_from(meta: &std::fs::Metadata) -> io::Result<FileStat> {
    use std::os::unix::fs::MetadataExt;
    Ok(FileStat {
        is_dir: meta.is_dir(),
        is_file: meta.is_file(),
        is_symlink: meta.file_type().is_symlink(),
        len: meta.len(),
        mode: meta.mode() & 0o7777,
        uid: meta.uid(),
        gid: meta.gid(),
        accessed: meta.accessed().ok(),
        modified: meta.modified().ok(),
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// FakeFileOps — an in-memory filesystem for tests.
// ═══════════════════════════════════════════════════════════════════════════

use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Debug, Clone)]
enum FakeKind {
    File(Vec<u8>),
    Dir,
    Symlink(PathBuf),
}

#[derive(Debug, Clone)]
struct FakeInode {
    kind: FakeKind,
    mode: u32,
    uid: u32,
    gid: u32,
    accessed: SystemTime,
    modified: SystemTime,
    nlink: u32,
}

#[derive(Debug)]
struct FakeState {
    /// Inode arena, keyed by inode number. Hard links are two paths → one ino.
    inodes: HashMap<u64, FakeInode>,
    /// Directory structure, flattened: absolute path → inode number.
    paths: HashMap<PathBuf, u64>,
    next_ino: u64,
    /// The uid the fake caller runs as (what a `chown` to "self" is a no-op for).
    current_uid: u32,
    /// When `false`, a `chown` that changes the owner to another uid returns
    /// `PermissionDenied` — the unprivileged-user path, driven deterministically.
    privileged: bool,
}

/// In-memory [`FileOps`] for unit tests. Construct with [`FakeFileOps::new`]
/// (an unprivileged caller, uid 1000) or [`FakeFileOps::privileged`] (a caller
/// that may `chown` freely). A root directory `/` exists on construction.
#[derive(Debug)]
pub struct FakeFileOps {
    state: RefCell<FakeState>,
}

impl Default for FakeFileOps {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeFileOps {
    /// An unprivileged fake FS (uid 1000): a `chown` to a different uid errors,
    /// mirroring a non-root process.
    #[must_use]
    pub fn new() -> Self {
        Self::with(1000, false)
    }

    /// A privileged fake FS (uid 0, `CAP_CHOWN`): every `chown` succeeds.
    #[must_use]
    pub fn privileged() -> Self {
        Self::with(0, true)
    }

    fn with(current_uid: u32, privileged: bool) -> Self {
        let now = SystemTime::now();
        let mut inodes = HashMap::new();
        inodes.insert(
            1,
            FakeInode {
                kind: FakeKind::Dir,
                mode: 0o755,
                uid: current_uid,
                gid: current_uid,
                accessed: now,
                modified: now,
                nlink: 1,
            },
        );
        let mut paths = HashMap::new();
        paths.insert(PathBuf::from("/"), 1);
        Self {
            state: RefCell::new(FakeState {
                inodes,
                paths,
                next_ino: 2,
                current_uid,
                privileged,
            }),
        }
    }

    // ── test-support helpers (not part of the trait) ────────────────────────

    /// Seed a regular file with `contents`, creating it if absent. Parent must
    /// exist. Test convenience — the trait's `create_file` makes empty files.
    pub fn seed_file(&self, path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<()> {
        let path = path.as_ref().to_path_buf();
        let mut st = self.state.borrow_mut();
        st.require_parent_dir(&path)?;
        let bytes = contents.as_ref().to_vec();
        if let Some(&ino) = st.paths.get(&path) {
            match st.inodes.get_mut(&ino).map(|n| &mut n.kind) {
                Some(FakeKind::File(data)) => {
                    *data = bytes;
                    Ok(())
                }
                _ => Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "path exists and is not a file",
                )),
            }
        } else {
            st.insert_node(&path, FakeKind::File(bytes), 0o644);
            Ok(())
        }
    }

    /// Read a regular file's bytes back (test assertion helper).
    pub fn read(&self, path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
        let st = self.state.borrow();
        let ino = st.follow(path.as_ref())?;
        match &st.inodes[&ino].kind {
            FakeKind::File(data) => Ok(data.clone()),
            _ => Err(io::Error::new(io::ErrorKind::InvalidInput, "not a file")),
        }
    }

    /// `true` when a path exists (no symlink following — an `lstat` existence).
    pub fn exists(&self, path: impl AsRef<Path>) -> bool {
        self.state.borrow().paths.contains_key(path.as_ref())
    }

    /// The hard-link count of the inode a path points at (test helper).
    pub fn nlink(&self, path: impl AsRef<Path>) -> io::Result<u32> {
        let st = self.state.borrow();
        let ino = st.lookup(path.as_ref())?;
        Ok(st.inodes[&ino].nlink)
    }
}

impl FakeState {
    fn lookup(&self, path: &Path) -> io::Result<u64> {
        self.paths
            .get(path)
            .copied()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }

    /// Resolve a path to its final inode, following symlinks (bounded).
    fn follow(&self, path: &Path) -> io::Result<u64> {
        let mut cur = path.to_path_buf();
        for _ in 0..40 {
            let ino = self.lookup(&cur)?;
            match &self.inodes[&ino].kind {
                FakeKind::Symlink(target) => cur = target.clone(),
                _ => return Ok(ino),
            }
        }
        Err(io::Error::other("too many levels of symbolic links"))
    }

    fn require_parent_dir(&self, path: &Path) -> io::Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
        let ino = self.lookup(parent)?;
        match self.inodes[&ino].kind {
            FakeKind::Dir => Ok(()),
            _ => Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "parent is not a directory",
            )),
        }
    }

    fn insert_node(&mut self, path: &Path, kind: FakeKind, mode: u32) -> u64 {
        let ino = self.next_ino;
        self.next_ino += 1;
        let now = SystemTime::now();
        self.inodes.insert(
            ino,
            FakeInode {
                kind,
                mode,
                uid: self.current_uid,
                gid: self.current_uid,
                accessed: now,
                modified: now,
                nlink: 1,
            },
        );
        self.paths.insert(path.to_path_buf(), ino);
        ino
    }

    fn stat_of(&self, ino: u64) -> FileStat {
        let node = &self.inodes[&ino];
        let (is_dir, is_file, is_symlink, len) = match &node.kind {
            FakeKind::File(data) => (false, true, false, data.len() as u64),
            FakeKind::Dir => (true, false, false, 0),
            FakeKind::Symlink(target) => (false, false, true, target.as_os_str().len() as u64),
        };
        FileStat {
            is_dir,
            is_file,
            is_symlink,
            len,
            mode: node.mode,
            uid: node.uid,
            gid: node.gid,
            accessed: Some(node.accessed),
            modified: Some(node.modified),
        }
    }

    /// Drop one path→inode mapping, freeing the inode when its last link goes.
    fn unlink_path(&mut self, path: &Path) {
        if let Some(ino) = self.paths.remove(path) {
            if let Some(node) = self.inodes.get_mut(&ino) {
                node.nlink = node.nlink.saturating_sub(1);
                if node.nlink == 0 {
                    self.inodes.remove(&ino);
                }
            }
        }
    }
}

impl FileOps for FakeFileOps {
    fn metadata(&self, path: &Path) -> io::Result<FileStat> {
        let st = self.state.borrow();
        let ino = st.follow(path)?;
        Ok(st.stat_of(ino))
    }

    fn symlink_metadata(&self, path: &Path) -> io::Result<FileStat> {
        let st = self.state.borrow();
        let ino = st.lookup(path)?;
        Ok(st.stat_of(ino))
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        let st = self.state.borrow();
        let ino = st.follow(path)?;
        if !matches!(st.inodes[&ino].kind, FakeKind::Dir) {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "not a directory",
            ));
        }
        let mut out = Vec::new();
        for child in st.paths.keys() {
            if child.parent() == Some(path) && child.as_path() != path {
                out.push(child.clone());
            }
        }
        Ok(out)
    }

    fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        let st = self.state.borrow();
        let ino = st.lookup(path)?;
        match &st.inodes[&ino].kind {
            FakeKind::Symlink(target) => Ok(target.clone()),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a symbolic link",
            )),
        }
    }

    fn copy_file(&self, src: &Path, dst: &Path) -> io::Result<u64> {
        let mut st = self.state.borrow_mut();
        let ino = st.follow(src)?;
        let (bytes, mode) = match &st.inodes[&ino].kind {
            FakeKind::File(data) => (data.clone(), st.inodes[&ino].mode),
            FakeKind::Dir => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "copy_file on a directory",
                ))
            }
            FakeKind::Symlink(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "copy_file followed a dangling symlink",
                ))
            }
        };
        st.require_parent_dir(dst)?;
        let len = bytes.len() as u64;
        if let Some(&d) = st.paths.get(dst) {
            // Overwrite an existing regular file in place.
            if let Some(FakeKind::File(_)) = st.inodes.get(&d).map(|n| &n.kind) {
                st.unlink_path(dst);
            }
        }
        st.insert_node(dst, FakeKind::File(bytes), mode);
        Ok(len)
    }

    fn rename(&self, src: &Path, dst: &Path) -> io::Result<()> {
        let mut st = self.state.borrow_mut();
        st.lookup(src)?;
        st.require_parent_dir(dst)?;
        // Re-key `src` and, if it is a directory, every descendant path.
        let moves: Vec<(PathBuf, PathBuf)> = st
            .paths
            .keys()
            .filter_map(|p| {
                if p == src {
                    Some((p.clone(), dst.to_path_buf()))
                } else if p.starts_with(src) {
                    let rel = p.strip_prefix(src).ok()?;
                    Some((p.clone(), dst.join(rel)))
                } else {
                    None
                }
            })
            .collect();
        // Clear any existing destination subtree first (overwrite semantics).
        let clears: Vec<PathBuf> = st
            .paths
            .keys()
            .filter(|p| p.as_path() == dst || p.starts_with(dst))
            .cloned()
            .collect();
        for p in clears {
            st.unlink_path(&p);
        }
        for (from, to) in moves {
            if let Some(ino) = st.paths.remove(&from) {
                st.paths.insert(to, ino);
            }
        }
        Ok(())
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        let mut st = self.state.borrow_mut();
        let ino = st.lookup(path)?;
        if matches!(st.inodes[&ino].kind, FakeKind::Dir) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "remove_file on a directory",
            ));
        }
        st.unlink_path(path);
        Ok(())
    }

    fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
        let mut st = self.state.borrow_mut();
        let ino = st.lookup(path)?;
        if !matches!(st.inodes[&ino].kind, FakeKind::Dir) {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "remove_dir_all on a non-directory",
            ));
        }
        let victims: Vec<PathBuf> = st
            .paths
            .keys()
            .filter(|p| p.as_path() == path || p.starts_with(path))
            .cloned()
            .collect();
        for p in victims {
            st.unlink_path(&p);
        }
        Ok(())
    }

    fn create_dir(&self, path: &Path) -> io::Result<()> {
        let mut st = self.state.borrow_mut();
        if st.paths.contains_key(path) {
            return Err(io::Error::from(io::ErrorKind::AlreadyExists));
        }
        st.require_parent_dir(path)?;
        st.insert_node(path, FakeKind::Dir, 0o755);
        Ok(())
    }

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        let mut st = self.state.borrow_mut();
        // Build the ancestor chain root→leaf, creating each missing directory.
        let mut ancestors: Vec<&Path> = path.ancestors().collect();
        ancestors.reverse();
        for dir in ancestors {
            if dir.as_os_str().is_empty() {
                continue;
            }
            match st.paths.get(dir) {
                Some(&ino) => {
                    if !matches!(st.inodes[&ino].kind, FakeKind::Dir) {
                        return Err(io::Error::new(
                            io::ErrorKind::AlreadyExists,
                            "path component is not a directory",
                        ));
                    }
                }
                None => {
                    st.insert_node(dir, FakeKind::Dir, 0o755);
                }
            }
        }
        Ok(())
    }

    fn create_file(&self, path: &Path) -> io::Result<()> {
        let mut st = self.state.borrow_mut();
        if st.paths.contains_key(path) {
            return Err(io::Error::from(io::ErrorKind::AlreadyExists));
        }
        st.require_parent_dir(path)?;
        st.insert_node(path, FakeKind::File(Vec::new()), 0o644);
        Ok(())
    }

    fn symlink(&self, target: &Path, link: &Path) -> io::Result<()> {
        let mut st = self.state.borrow_mut();
        if st.paths.contains_key(link) {
            return Err(io::Error::from(io::ErrorKind::AlreadyExists));
        }
        st.require_parent_dir(link)?;
        st.insert_node(link, FakeKind::Symlink(target.to_path_buf()), 0o777);
        Ok(())
    }

    fn hard_link(&self, target: &Path, link: &Path) -> io::Result<()> {
        let mut st = self.state.borrow_mut();
        let ino = st.lookup(target)?;
        if matches!(st.inodes[&ino].kind, FakeKind::Dir) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "hard link to a directory",
            ));
        }
        if st.paths.contains_key(link) {
            return Err(io::Error::from(io::ErrorKind::AlreadyExists));
        }
        st.require_parent_dir(link)?;
        st.paths.insert(link.to_path_buf(), ino);
        if let Some(node) = st.inodes.get_mut(&ino) {
            node.nlink += 1;
        }
        Ok(())
    }

    fn set_permissions(&self, path: &Path, mode: u32) -> io::Result<()> {
        let mut st = self.state.borrow_mut();
        let ino = st.follow(path)?;
        if let Some(node) = st.inodes.get_mut(&ino) {
            node.mode = mode & 0o7777;
        }
        Ok(())
    }

    fn chown(&self, path: &Path, uid: Option<u32>, gid: Option<u32>) -> io::Result<()> {
        let mut st = self.state.borrow_mut();
        let ino = st.follow(path)?;
        // Honest privilege check: an unprivileged caller may not give a file
        // away to another user. `None` (leave unchanged) and a no-op self-chown
        // are always allowed — this mirrors `chown(2)` EPERM.
        if !st.privileged {
            if let Some(new_uid) = uid {
                if new_uid != st.current_uid {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "chown: not permitted (needs CAP_CHOWN)",
                    ));
                }
            }
        }
        if let Some(node) = st.inodes.get_mut(&ino) {
            if let Some(u) = uid {
                node.uid = u;
            }
            if let Some(g) = gid {
                node.gid = g;
            }
        }
        Ok(())
    }

    fn set_times(&self, path: &Path, accessed: SystemTime, modified: SystemTime) -> io::Result<()> {
        let mut st = self.state.borrow_mut();
        let ino = st.follow(path)?;
        if let Some(node) = st.inodes.get_mut(&ino) {
            node.accessed = accessed;
            node.modified = modified;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // ── free_duplicate_name -------------------------------------------------

    #[test]
    fn duplicate_name_preserves_extension_and_escalates() {
        let base = Path::new("/d/report.txt");
        let mut taken = vec![PathBuf::from("/d/report.txt")];
        let n1 = free_duplicate_name(base, |p| taken.contains(&p.to_path_buf()));
        assert_eq!(n1, PathBuf::from("/d/report copy.txt"));
        taken.push(n1);
        let n2 = free_duplicate_name(base, |p| taken.contains(&p.to_path_buf()));
        assert_eq!(n2, PathBuf::from("/d/report copy 2.txt"));
    }

    #[test]
    fn duplicate_name_handles_no_extension() {
        let n = free_duplicate_name(Path::new("/d/README"), |_| false);
        assert_eq!(n, PathBuf::from("/d/README copy"));
    }

    // ── FakeFileOps: the whole operation set in-memory ---------------------

    fn scratch() -> FakeFileOps {
        let fs = FakeFileOps::new();
        fs.create_dir(Path::new("/work")).expect("mkdir /work");
        fs
    }

    #[test]
    fn fake_create_file_and_stat() {
        let fs = scratch();
        fs.create_file(Path::new("/work/a.txt")).expect("create");
        let st = fs.metadata(Path::new("/work/a.txt")).expect("stat");
        assert!(st.is_file);
        assert_eq!(st.len, 0);
        assert!(fs.create_file(Path::new("/work/a.txt")).is_err(), "O_EXCL");
    }

    #[test]
    fn fake_copy_move_delete_roundtrip() {
        let fs = scratch();
        fs.seed_file("/work/src.bin", b"payload").expect("seed");
        let n = fs
            .copy(Path::new("/work/src.bin"), Path::new("/work/dst.bin"))
            .expect("copy");
        assert_eq!(n, 7);
        assert_eq!(fs.read("/work/dst.bin").expect("read"), b"payload");
        fs.rename(Path::new("/work/dst.bin"), Path::new("/work/moved.bin"))
            .expect("rename");
        assert!(!fs.exists("/work/dst.bin"));
        assert!(fs.exists("/work/moved.bin"));
        fs.remove(Path::new("/work/moved.bin")).expect("remove");
        assert!(!fs.exists("/work/moved.bin"));
    }

    #[test]
    fn fake_recursive_copy_and_remove_dir() {
        let fs = scratch();
        fs.create_dir(Path::new("/work/tree")).expect("mkdir");
        fs.create_dir(Path::new("/work/tree/sub")).expect("mkdir");
        fs.seed_file("/work/tree/sub/leaf.txt", b"x").expect("seed");
        fs.copy(Path::new("/work/tree"), Path::new("/work/tree2"))
            .expect("recursive copy");
        assert_eq!(fs.read("/work/tree2/sub/leaf.txt").expect("read"), b"x");
        fs.remove(Path::new("/work/tree")).expect("rm -r");
        assert!(!fs.exists("/work/tree/sub/leaf.txt"));
        assert!(fs.exists("/work/tree2/sub/leaf.txt"));
    }

    #[test]
    fn fake_symlink_is_not_followed_by_lstat() {
        let fs = scratch();
        fs.seed_file("/work/real.txt", b"hi").expect("seed");
        fs.symlink(Path::new("/work/real.txt"), Path::new("/work/link"))
            .expect("symlink");
        let l = fs.symlink_metadata(Path::new("/work/link")).expect("lstat");
        assert!(l.is_symlink);
        let m = fs.metadata(Path::new("/work/link")).expect("stat follows");
        assert!(m.is_file);
        assert_eq!(
            fs.read_link(Path::new("/work/link")).expect("readlink"),
            PathBuf::from("/work/real.txt")
        );
    }

    #[test]
    fn fake_hardlink_shares_inode_and_bumps_nlink() {
        let fs = scratch();
        fs.seed_file("/work/orig", b"data").expect("seed");
        fs.hard_link(Path::new("/work/orig"), Path::new("/work/hl"))
            .expect("hardlink");
        assert_eq!(fs.nlink("/work/orig").expect("nlink"), 2);
        assert_eq!(fs.read("/work/hl").expect("read"), b"data");
        // Unlinking one name leaves the other + drops nlink back to 1.
        fs.remove(Path::new("/work/orig")).expect("unlink");
        assert_eq!(fs.nlink("/work/hl").expect("nlink"), 1);
        assert_eq!(fs.read("/work/hl").expect("read"), b"data");
    }

    #[test]
    fn fake_duplicate_picks_a_free_name() {
        let fs = scratch();
        fs.seed_file("/work/note.md", b"n").expect("seed");
        let d1 = fs.duplicate(Path::new("/work/note.md")).expect("dup");
        assert_eq!(d1, PathBuf::from("/work/note copy.md"));
        let d2 = fs.duplicate(Path::new("/work/note.md")).expect("dup2");
        assert_eq!(d2, PathBuf::from("/work/note copy 2.md"));
    }

    #[test]
    fn fake_chmod_updates_mode_bits() {
        let fs = scratch();
        fs.seed_file("/work/s.sh", b"#!/bin/sh").expect("seed");
        fs.set_permissions(Path::new("/work/s.sh"), 0o755)
            .expect("chmod");
        assert_eq!(
            fs.metadata(Path::new("/work/s.sh")).expect("stat").mode,
            0o755
        );
    }

    #[test]
    fn fake_chown_honest_errors_without_privilege() {
        let fs = FakeFileOps::new(); // unprivileged, uid 1000
        fs.seed_file("/f", b"x").expect("seed");
        // Giving the file to another uid is not permitted for a normal user.
        let err = fs
            .chown(Path::new("/f"), Some(0), None)
            .expect_err("must be denied");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        // A no-op self-chown and a gid-only change are allowed.
        fs.chown(Path::new("/f"), Some(1000), Some(1000))
            .expect("self chown ok");
        assert_eq!(fs.metadata(Path::new("/f")).expect("stat").gid, 1000);
    }

    #[test]
    fn fake_chown_succeeds_when_privileged() {
        let fs = FakeFileOps::privileged();
        fs.seed_file("/f", b"x").expect("seed");
        fs.chown(Path::new("/f"), Some(42), Some(7))
            .expect("root chown");
        let st = fs.metadata(Path::new("/f")).expect("stat");
        assert_eq!((st.uid, st.gid), (42, 7));
    }

    #[test]
    fn fake_set_times_records_timestamps() {
        let fs = scratch();
        fs.seed_file("/work/t", b"x").expect("seed");
        let when = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        fs.set_times(Path::new("/work/t"), when, when)
            .expect("utimes");
        let st = fs.metadata(Path::new("/work/t")).expect("stat");
        assert_eq!(st.modified, Some(when));
        assert_eq!(st.accessed, Some(when));
    }

    #[test]
    fn fake_read_dir_lists_immediate_children_only() {
        let fs = scratch();
        fs.create_file(Path::new("/work/a")).expect("f");
        fs.create_dir(Path::new("/work/d")).expect("d");
        fs.create_file(Path::new("/work/d/nested")).expect("f");
        let mut kids = fs.read_dir(Path::new("/work")).expect("readdir");
        kids.sort();
        assert_eq!(
            kids,
            vec![PathBuf::from("/work/a"), PathBuf::from("/work/d")]
        );
    }

    #[test]
    fn fake_missing_path_is_not_found() {
        let fs = scratch();
        let err = fs.metadata(Path::new("/work/ghost")).expect_err("missing");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    // ── LiveFileOps: the same set against a real temp directory ------------

    /// A unique temp dir for a live test, cleaned up at the end.
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "mde-files-fileops-{}-{}-{:?}",
                tag,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
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
    fn live_copy_move_delete_on_a_real_fs() {
        let ops = LiveFileOps::new();
        let d = TempDir::new("cmd");
        let src = d.path("src.txt");
        std::fs::write(&src, b"hello world").expect("write");
        let dst = d.path("dst.txt");
        let n = ops.copy(&src, &dst).expect("copy");
        assert_eq!(n, 11);
        assert_eq!(std::fs::read(&dst).expect("read"), b"hello world");
        let moved = d.path("moved.txt");
        ops.rename(&dst, &moved).expect("rename");
        assert!(!dst.exists() && moved.exists());
        ops.remove(&moved).expect("remove");
        assert!(!moved.exists());
    }

    #[test]
    fn live_mkdir_newfile_symlink_hardlink() {
        let ops = LiveFileOps::new();
        let d = TempDir::new("mk");
        let sub = d.path("a/b/c");
        ops.create_dir_all(&sub).expect("mkdir -p");
        assert!(ops.metadata(&sub).expect("stat").is_dir);

        let f = d.path("a/new.txt");
        ops.create_file(&f).expect("create");
        assert!(ops.create_file(&f).is_err(), "O_EXCL on second create");

        let link = d.path("a/link.txt");
        ops.symlink(&f, &link).expect("symlink");
        assert!(ops.symlink_metadata(&link).expect("lstat").is_symlink);
        assert!(ops.metadata(&link).expect("stat").is_file);
        assert_eq!(ops.read_link(&link).expect("readlink"), f);

        let hl = d.path("a/hard.txt");
        ops.hard_link(&f, &hl).expect("hardlink");
        assert!(ops.metadata(&hl).expect("stat").is_file);
    }

    #[test]
    fn live_chmod_reads_back_the_mode() {
        let ops = LiveFileOps::new();
        let d = TempDir::new("chmod");
        let f = d.path("s.sh");
        std::fs::write(&f, b"#!/bin/sh\n").expect("write");
        ops.set_permissions(&f, 0o750).expect("chmod");
        assert_eq!(ops.metadata(&f).expect("stat").mode, 0o750);
    }

    #[test]
    fn live_chown_to_self_is_ok_and_stat_has_owner() {
        let ops = LiveFileOps::new();
        let d = TempDir::new("chown");
        let f = d.path("owned");
        std::fs::write(&f, b"x").expect("write");
        let st = ops.metadata(&f).expect("stat");
        // chown to the file's current owner is always permitted (a no-op),
        // even for an unprivileged caller — proves the wrapper reaches chown(2)
        // without needing privilege in the test env.
        ops.chown(&f, Some(st.uid), Some(st.gid))
            .expect("self-chown must succeed");
    }

    #[test]
    fn live_set_times_roundtrips() {
        let ops = LiveFileOps::new();
        let d = TempDir::new("times");
        let f = d.path("t");
        std::fs::write(&f, b"x").expect("write");
        let when = SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000);
        ops.set_times(&f, when, when).expect("set_times");
        let got = ops.metadata(&f).expect("stat").modified.expect("mtime");
        // Filesystems can round timestamps; allow a small slop.
        let delta = got
            .duration_since(when)
            .or_else(|_| when.duration_since(got))
            .unwrap_or_default();
        assert!(
            delta < Duration::from_secs(2),
            "mtime not applied: {delta:?}"
        );
    }

    #[test]
    fn live_duplicate_makes_a_copy_sibling() {
        let ops = LiveFileOps::new();
        let d = TempDir::new("dup");
        let f = d.path("photo.png");
        std::fs::write(&f, b"PNG").expect("write");
        let dup = ops.duplicate(&f).expect("duplicate");
        assert_eq!(
            dup.file_name().and_then(|s| s.to_str()),
            Some("photo copy.png")
        );
        assert_eq!(std::fs::read(&dup).expect("read"), b"PNG");
    }
}
