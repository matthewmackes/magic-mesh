//! QBRAND-1 — compile-time build-identity stamp for [`mde_theme::brand::build`].
//!
//! Emits four `cargo:rustc-env` variables the `brand::build` module reads back
//! with `env!`:
//!
//! * `MDE_BUILD_VERSION` — the crate (workspace) `CARGO_PKG_VERSION`.
//! * `MDE_BUILD_GIT_HASH` — the exact immutable source revision from a governed
//!   build receipt, or an explicit `non-promotable-*` marker for developer builds.
//! * `MDE_BUILD_DATE` — UTC calendar date `YYYY-MM-DD`; from `SOURCE_DATE_EPOCH`
//!   when set (reproducible builds), else the current build time.
//! * `MDE_BUILD_CHANNEL` — the release channel from `MDE_CHANNEL`, default `dev`.
//!
//! A build declaring `MCNF_BUILD_PROMOTABLE=1` fails closed unless its receipt is
//! exact and, when Git metadata is present, matches a clean checkout at HEAD.

use std::process::Command;

fn main() {
    // Version — single-sourced from Cargo (the workspace `version.workspace`).
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_owned());
    println!("cargo:rustc-env=MDE_BUILD_VERSION={version}");

    println!(
        "cargo:rustc-env=MDE_BUILD_GIT_HASH={}",
        source_revision_stamp()
    );

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
    println!("cargo:rerun-if-env-changed=MCNF_BUILD_SOURCE_REVISION");
    println!("cargo:rerun-if-env-changed=MCNF_BUILD_PROMOTABLE");
}

fn source_revision_stamp() -> String {
    let promotable = std::env::var("MCNF_BUILD_PROMOTABLE").as_deref() == Ok("1");
    let receipt = std::env::var("MCNF_BUILD_SOURCE_REVISION")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());

    if promotable {
        let revision = receipt
            .unwrap_or_else(|| panic!("promotable build requires MCNF_BUILD_SOURCE_REVISION"));
        assert!(
            exact_revision(&revision),
            "promotable source revision must be an exact lowercase 40- or 64-hex Git object ID"
        );
        if let Some(head) = git(&["rev-parse", "--verify", "HEAD^{commit}"]) {
            assert_eq!(
                head, revision,
                "promotable source receipt does not match checkout HEAD"
            );
            let status = git_allow_empty(&["status", "--porcelain=v1", "--untracked-files=normal"])
                .expect("promotable build could not determine checkout cleanliness");
            assert!(status.is_empty(), "promotable build checkout is dirty");
        }
        return revision;
    }

    match git(&["rev-parse", "--verify", "HEAD^{commit}"]) {
        Some(head) if exact_revision(&head) => {
            match git_allow_empty(&["status", "--porcelain=v1", "--untracked-files=normal"]) {
                Some(status) if status.is_empty() => format!("non-promotable-unreceipted-{head}"),
                Some(_) => format!("non-promotable-dirty-{head}"),
                None => "non-promotable-unresolved".to_owned(),
            }
        }
        _ => "non-promotable-unresolved".to_owned(),
    }
}

fn exact_revision(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Run `git <args>` and return the trimmed stdout, or `None` when git is
/// missing, errors, or prints nothing (so the caller can substitute a sentinel).
fn git(args: &[&str]) -> Option<String> {
    git_allow_empty(args).filter(|text| !text.is_empty())
}

fn git_allow_empty(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    Some(text)
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
