//! MESHFS-1.2 (v5.0.0) — LizardFS data-dir headroom pre-flight check.
//!
//! Pure-function `check(data_dir, xdg_dirs)` walks the listed
//! XDG directories, sums their on-disk byte sizes, queries the
//! filesystem free space at `data_dir`, and reports whether
//! the LizardFS data partition has enough headroom for the
//! mesh-storage volume to safely accept replication overhead
//! (target: ≥ 1.5× the sum of XDG content).
//!
//! Operator-invokable via `mackesd preflight-meshfs-headroom`
//! (added in this commit). The Workbench "Mesh Storage" panel
//! (MESHFS-13.1) will surface the same report as a banner once
//! it lands; the CLI lets operators run the check from a
//! terminal today even before the panel ships.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Default LizardFS data parent directory (per the MESHFS design
/// lock — `/var/lib/mde/meshfs/`). `check()` takes the data dir
/// as an arg so tests + dev rigs use a tempdir; production
/// callers pass this constant.
pub const DEFAULT_MESHFS_DATA_DIR: &str = "/var/lib/mde/meshfs";

/// Headroom multiplier — the worklist locks 1.5× of the sum of
/// XDG content as the WARN threshold. Below this, the pre-flight
/// reports WARN; at or above, OK.
pub const HEADROOM_MULTIPLIER: f64 = 1.5;

/// The five XDG dirs that the mesh-storage FUSE mount owns once
/// MESHFS-3.3 lands. The pre-flight sums the bytes currently
/// sitting in these locations on the local peer.
pub fn default_xdg_dirs(home: &Path) -> Vec<PathBuf> {
    ["Documents", "Pictures", "Music", "Videos", "Downloads"]
        .iter()
        .map(|name| home.join(name))
        .collect()
}

/// Verdict of one pre-flight check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    /// Data dir has enough headroom — install / upgrade is safe.
    Ok,
    /// Data dir headroom is below `1.5 × xdg_bytes`. The
    /// operator can still install; mesh-storage will accept
    /// writes until the quota pinches.
    Warn,
    /// Data dir doesn't exist at all (lizardfs-server not
    /// installed yet, or the data mountpoint is missing).
    /// Treated as Warn semantically — operator should fix before
    /// mesh-storage bootstrap.
    NoDataDir,
}

/// Headroom report — what the CLI prints + what the panel banner
/// reads from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeadroomReport {
    /// OK / Warn / NoDataDir.
    pub verdict: Verdict,
    /// Data directory examined.
    pub data_dir: PathBuf,
    /// Free bytes at the data-dir filesystem. `0` when
    /// `verdict == NoDataDir`.
    pub data_free_bytes: u64,
    /// Per-XDG-dir size in bytes (in order of the input
    /// `xdg_dirs` arg). Missing dirs contribute 0.
    pub xdg_sizes: Vec<(PathBuf, u64)>,
    /// Sum of `xdg_sizes`.
    pub xdg_total_bytes: u64,
    /// Required free bytes given the 1.5× multiplier.
    pub required_bytes: u64,
}

impl HeadroomReport {
    /// One-line, operator-friendly summary suitable for CLI
    /// output. The full JSON shape goes to Workbench / scripted
    /// consumers via serde.
    #[must_use]
    pub fn summary(&self) -> String {
        match self.verdict {
            Verdict::NoDataDir => format!(
                "[NoDataDir] {dir} missing — install lizardfs-server first",
                dir = self.data_dir.display()
            ),
            Verdict::Ok => format!(
                "[OK] data-dir free {free_mb} MB ≥ required {req_mb} MB (XDG {xdg_mb} MB × 1.5)",
                free_mb = self.data_free_bytes / 1_048_576,
                req_mb = self.required_bytes / 1_048_576,
                xdg_mb = self.xdg_total_bytes / 1_048_576,
            ),
            Verdict::Warn => format!(
                "[WARN] data-dir free {free_mb} MB < required {req_mb} MB (XDG {xdg_mb} MB × 1.5) — \
                 mesh-storage will accept writes but quota will pinch",
                free_mb = self.data_free_bytes / 1_048_576,
                req_mb = self.required_bytes / 1_048_576,
                xdg_mb = self.xdg_total_bytes / 1_048_576,
            ),
        }
    }
}

/// Run the pre-flight check.
///
/// `data_dir` is the LizardFS data parent
/// (`/var/lib/mde/meshfs` by default). `xdg_dirs` is the list
/// of directories whose content will move into mesh-storage —
/// pass [`default_xdg_dirs($HOME)`].
///
/// Pure-ish: walks the filesystem (so not strictly pure) but
/// returns a fully-self-described [`HeadroomReport`] with no
/// side effects beyond stat calls.
#[must_use]
pub fn check(data_dir: &Path, xdg_dirs: &[PathBuf]) -> HeadroomReport {
    if !data_dir.exists() {
        return HeadroomReport {
            verdict: Verdict::NoDataDir,
            data_dir: data_dir.to_path_buf(),
            data_free_bytes: 0,
            xdg_sizes: xdg_dirs
                .iter()
                .map(|d| (d.clone(), dir_size_bytes(d)))
                .collect(),
            xdg_total_bytes: xdg_dirs.iter().map(|d| dir_size_bytes(d)).sum(),
            required_bytes: 0,
        };
    }
    let xdg_sizes: Vec<(PathBuf, u64)> = xdg_dirs
        .iter()
        .map(|d| (d.clone(), dir_size_bytes(d)))
        .collect();
    let xdg_total: u64 = xdg_sizes.iter().map(|(_, n)| *n).sum();
    let required = (xdg_total as f64 * HEADROOM_MULTIPLIER) as u64;
    let free = filesystem_free_bytes(data_dir).unwrap_or(0);
    let verdict = if free >= required {
        Verdict::Ok
    } else {
        Verdict::Warn
    };
    HeadroomReport {
        verdict,
        data_dir: data_dir.to_path_buf(),
        data_free_bytes: free,
        xdg_sizes,
        xdg_total_bytes: xdg_total,
        required_bytes: required,
    }
}

/// Recursive byte-size walk. Symlinks are NOT followed (we want
/// the local on-disk footprint). Per-entry errors are silently
/// skipped — pre-flight is best-effort.
fn dir_size_bytes(dir: &Path) -> u64 {
    let mut total = 0u64;
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return 0,
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_file() {
            total += meta.len();
        } else if meta.is_dir() {
            total += dir_size_bytes(&entry.path());
        }
    }
    total
}

/// Query the filesystem free space for the partition that
/// contains `path`. Shells `df -B1 --output=avail <path>`
/// (GNU coreutils, available on every Fedora install) and
/// parses the second line. Returns `None` on shellout failure or
/// unparseable output — the workspace forbids `unsafe_code`, so
/// direct `statvfs` via libc isn't on the table.
fn filesystem_free_bytes(path: &Path) -> Option<u64> {
    let out = std::process::Command::new("df")
        .args(["-B1", "--output=avail"])
        .arg(path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = std::str::from_utf8(&out.stdout).ok()?;
    text.lines().nth(1)?.trim().parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_data_dir_returns_no_data_dir_verdict() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let missing = tmp.path().join("does-not-exist");
        let report = check(&missing, &[]);
        assert_eq!(report.verdict, Verdict::NoDataDir);
        assert_eq!(report.data_free_bytes, 0);
        assert_eq!(report.required_bytes, 0);
    }

    #[test]
    fn empty_xdg_dirs_yields_zero_required_bytes() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let report = check(tmp.path(), &[]);
        assert_eq!(report.xdg_total_bytes, 0);
        assert_eq!(report.required_bytes, 0);
        assert_eq!(report.verdict, Verdict::Ok);
    }

    #[test]
    fn xdg_sizes_aggregate_file_bytes() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let docs = tmp.path().join("Documents");
        std::fs::create_dir_all(&docs).expect("mkdir");
        std::fs::write(docs.join("a.txt"), vec![0u8; 1024]).expect("write a");
        std::fs::write(docs.join("b.txt"), vec![0u8; 2048]).expect("write b");
        let nested = docs.join("nested");
        std::fs::create_dir(&nested).expect("mkdir nested");
        std::fs::write(nested.join("c.txt"), vec![0u8; 512]).expect("write c");

        let report = check(tmp.path(), &[docs.clone()]);
        assert_eq!(report.xdg_sizes.len(), 1);
        assert_eq!(report.xdg_sizes[0].1, 1024 + 2048 + 512);
        assert_eq!(report.xdg_total_bytes, 3584);
        assert_eq!(report.required_bytes, (3584.0 * 1.5) as u64);
    }

    #[test]
    fn missing_xdg_dirs_contribute_zero_and_dont_panic() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let report = check(
            tmp.path(),
            &[tmp.path().join("Documents"), tmp.path().join("Music")],
        );
        assert_eq!(report.xdg_total_bytes, 0);
        for (_, n) in &report.xdg_sizes {
            assert_eq!(*n, 0);
        }
    }

    #[test]
    fn default_xdg_dirs_returns_five_known_names() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dirs = default_xdg_dirs(tmp.path());
        assert_eq!(dirs.len(), 5);
        let names: Vec<_> = dirs
            .iter()
            .map(|d| d.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"Documents".to_owned()));
        assert!(names.contains(&"Pictures".to_owned()));
        assert!(names.contains(&"Music".to_owned()));
        assert!(names.contains(&"Videos".to_owned()));
        assert!(names.contains(&"Downloads".to_owned()));
    }

    #[test]
    fn summary_lines_format_per_verdict_shape() {
        let no_data_dir = HeadroomReport {
            verdict: Verdict::NoDataDir,
            data_dir: PathBuf::from("/missing"),
            data_free_bytes: 0,
            xdg_sizes: vec![],
            xdg_total_bytes: 0,
            required_bytes: 0,
        };
        assert!(no_data_dir.summary().starts_with("[NoDataDir]"));

        let ok = HeadroomReport {
            verdict: Verdict::Ok,
            data_dir: PathBuf::from("/var/lib/mde/meshfs"),
            data_free_bytes: 10 * 1_073_741_824,
            xdg_sizes: vec![],
            xdg_total_bytes: 1_073_741_824,
            required_bytes: (1_073_741_824.0 * 1.5) as u64,
        };
        assert!(ok.summary().starts_with("[OK]"));

        let warn = HeadroomReport {
            verdict: Verdict::Warn,
            data_dir: PathBuf::from("/var/lib/mde/meshfs"),
            data_free_bytes: 1_073_741_824,
            xdg_sizes: vec![],
            xdg_total_bytes: 10 * 1_073_741_824,
            required_bytes: 15 * 1_073_741_824,
        };
        assert!(warn.summary().starts_with("[WARN]"));
        assert!(warn.summary().contains("quota will pinch"));
    }

    #[test]
    fn report_json_round_trips() {
        let report = HeadroomReport {
            verdict: Verdict::Ok,
            data_dir: PathBuf::from("/var/lib/mde/meshfs"),
            data_free_bytes: 10 * 1_073_741_824,
            xdg_sizes: vec![(PathBuf::from("/home/u/Documents"), 1024)],
            xdg_total_bytes: 1024,
            required_bytes: 1536,
        };
        let json = serde_json::to_string(&report).expect("encode");
        let back: HeadroomReport = serde_json::from_str(&json).expect("decode");
        assert_eq!(report, back);
    }
}
