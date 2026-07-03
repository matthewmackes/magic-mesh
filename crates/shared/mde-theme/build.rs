//! QBRAND-1 — compile-time build-identity stamp for [`mde_theme::brand::build`].
//!
//! Emits four `cargo:rustc-env` variables the `brand::build` module reads back
//! with `env!`:
//!
//! * `MDE_BUILD_VERSION` — the crate (workspace) `CARGO_PKG_VERSION` (`12.0.0`).
//! * `MDE_BUILD_GIT_HASH` — `git rev-parse --short HEAD`, or the sentinel
//!   `nogit` when git is absent (a release tarball / shallow export).
//! * `MDE_BUILD_DATE` — UTC calendar date `YYYY-MM-DD`; from `SOURCE_DATE_EPOCH`
//!   when set (reproducible builds), else the current build time.
//! * `MDE_BUILD_CHANNEL` — the release channel from `MDE_CHANNEL`, default `dev`.
//!
//! It never panics on a missing git / env — every lookup degrades to a sentinel
//! so an offline, git-less packaging build still stamps a valid identity line.

use std::process::Command;

fn main() {
    // Version — single-sourced from Cargo (the workspace `version.workspace`).
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_owned());
    println!("cargo:rustc-env=MDE_BUILD_VERSION={version}");

    // Short git hash — best-effort. No git (release tarball) → the `nogit`
    // sentinel rather than a build failure.
    let git_hash = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "nogit".to_owned());
    println!("cargo:rustc-env=MDE_BUILD_GIT_HASH={git_hash}");

    // Build date (UTC). Reproducible builds pin `SOURCE_DATE_EPOCH`; otherwise
    // stamp the wall-clock build time.
    let epoch = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .unwrap_or_else(now_unix);
    println!("cargo:rustc-env=MDE_BUILD_DATE={}", utc_date(epoch));

    // Release channel — `dev` unless the packaging build sets `MDE_CHANNEL`.
    let channel = std::env::var("MDE_CHANNEL")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "dev".to_owned());
    println!("cargo:rustc-env=MDE_BUILD_CHANNEL={channel}");

    // Re-stamp when HEAD moves (so the hash tracks new commits) or the
    // reproducibility / channel envs change. In a git worktree HEAD lives
    // outside `.git/`, so ask git for its real path; absent git we skip it and
    // fall back to Cargo's default package-dir change scan.
    if let Some(head) = git(&["rev-parse", "--git-path", "HEAD"]) {
        println!("cargo:rerun-if-changed={head}");
    }
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    println!("cargo:rerun-if-env-changed=MDE_CHANNEL");
}

/// Run `git <args>` and return the trimmed stdout, or `None` when git is
/// missing, errors, or prints nothing (so the caller can substitute a sentinel).
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Current Unix time (seconds since the epoch), or `0` if the clock is before
/// the epoch (never, in practice — the fallback keeps the routine total).
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

/// Format Unix seconds as a UTC `YYYY-MM-DD` calendar date.
///
/// Howard Hinnant's public-domain civil-from-days algorithm — a dependency-free
/// (no `chrono`) conversion valid for the whole proleptic Gregorian range, so the
/// airgapped farm build needs no extra crate for the date stamp.
fn utc_date(epoch: i64) -> String {
    let days = epoch.div_euclid(86_400);
    let shifted = days + 719_468; // shift the epoch so the era starts 0000-03-01
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let doe = shifted - era * 146_097; // day-of-era      [0, 146_096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // year-of-era [0, 399]
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day-of-year (Mar-based)  [0, 365]
    let mp = (5 * doy + 2) / 153; // month index from March  [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if month <= 2 { year + 1 } else { year };
    format!("{year:04}-{month:02}-{day:02}")
}
