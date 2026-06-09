//! Native local file operations — copy / move / mkdir / remove (E11.6, Q34–Q39).
//!
//! The general-FM verbs the manager invokes on local paths, native `std::fs` (no
//! `cp`/`mv`/`rm` shell-out). `move_path` is the canonical rename-with-EXDEV-fallback
//! used by both the trash ([`crate::trash`]) and the Move verb; `copy_recursive`
//! deep-copies a tree. These are the local-FS primitives; mesh/peer moves go over
//! the Bus backend, not here.

use std::io;
use std::path::Path;

/// Move `from` to `to`, falling back to copy+remove when `rename` fails across
/// filesystems (`EXDEV` — source and destination on different mounts).
///
/// # Errors
/// When neither the rename nor the copy+remove fallback can complete.
pub fn move_path(from: &Path, to: &Path) -> io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            if from.is_dir() {
                copy_recursive(from, to)?;
                std::fs::remove_dir_all(from)?;
            } else {
                std::fs::copy(from, to)?;
                std::fs::remove_file(from)?;
            }
            Ok(())
        }
    }
}

/// Copy `from` to `to`. A directory is copied recursively (its whole tree); a
/// regular file is copied with its bytes. Creates parent directories as needed.
///
/// # Errors
/// When a read/write/create step fails.
pub fn copy(from: &Path, to: &Path) -> io::Result<()> {
    if from.is_dir() {
        copy_recursive(from, to)
    } else {
        if let Some(parent) = to.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        std::fs::copy(from, to).map(|_| ())
    }
}

/// Recursively copy directory `from` into a new directory `to`.
///
/// # Errors
/// When a directory can't be created or a file can't be copied.
pub fn copy_recursive(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if src.is_dir() {
            copy_recursive(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

/// Create directory `path` and any missing parents (idempotent — an existing
/// directory is not an error).
///
/// # Errors
/// When the directory can't be created (e.g. a parent is a file).
pub fn make_dir(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)
}

/// Permanently remove `path` — a file, an empty dir, or a whole directory tree.
/// This is the hard delete; the reversible delete is [`crate::trash`].
///
/// # Errors
/// When the path doesn't exist or can't be removed.
pub fn remove(path: &Path) -> io::Result<()> {
    let md = std::fs::symlink_metadata(path)?;
    if md.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mde-files-fileops-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn copy_file_creates_missing_parents() {
        let dir = scratch("copyfile");
        let src = dir.join("src.txt");
        std::fs::write(&src, b"payload").unwrap();
        let dst = dir.join("new/sub/dst.txt");
        copy(&src, &dst).unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), b"payload");
        assert!(src.exists(), "copy leaves the source in place");
    }

    #[test]
    fn copy_recursive_clones_a_tree() {
        let dir = scratch("copytree");
        let src = dir.join("tree");
        std::fs::create_dir_all(src.join("a/b")).unwrap();
        std::fs::write(src.join("top.txt"), b"t").unwrap();
        std::fs::write(src.join("a/b/deep.txt"), b"d").unwrap();
        let dst = dir.join("clone");
        copy(&src, &dst).unwrap();
        assert_eq!(std::fs::read(dst.join("top.txt")).unwrap(), b"t");
        assert_eq!(std::fs::read(dst.join("a/b/deep.txt")).unwrap(), b"d");
        assert!(src.join("top.txt").exists(), "original tree untouched");
    }

    #[test]
    fn move_path_relocates_a_file() {
        let dir = scratch("move");
        let src = dir.join("a.txt");
        std::fs::write(&src, b"x").unwrap();
        let dst = dir.join("b.txt");
        move_path(&src, &dst).unwrap();
        assert!(!src.exists());
        assert_eq!(std::fs::read(&dst).unwrap(), b"x");
    }

    #[test]
    fn make_dir_is_idempotent() {
        let dir = scratch("mkdir");
        let nested = dir.join("x/y/z");
        make_dir(&nested).unwrap();
        assert!(nested.is_dir());
        // calling again is fine
        make_dir(&nested).unwrap();
    }

    #[test]
    fn remove_handles_files_dirs_and_trees() {
        let dir = scratch("remove");
        let file = dir.join("f.txt");
        std::fs::write(&file, b"x").unwrap();
        remove(&file).unwrap();
        assert!(!file.exists());

        let tree = dir.join("t");
        std::fs::create_dir_all(tree.join("sub")).unwrap();
        std::fs::write(tree.join("sub/inner"), b"y").unwrap();
        remove(&tree).unwrap();
        assert!(!tree.exists());
    }

    #[test]
    fn remove_missing_path_errors() {
        let dir = scratch("rmmissing");
        assert!(remove(&dir.join("ghost")).is_err());
    }
}
