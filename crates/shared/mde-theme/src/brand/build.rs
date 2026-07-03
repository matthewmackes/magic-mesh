//! `brand::build` — the compile-time build identity.
//!
//! The crate's `build.rs` stamps the version, git short-hash, UTC date and
//! release channel into `cargo:rustc-env` variables; this module reads them back
//! with `env!` and shapes them into the two lines the platform shows:
//!
//! * [`version_line`] → `12.0.0 "Quazar"` — the short brand line in the shell
//!   chrome and the boot-splash.
//! * [`full`] → `12.0.0 "Quazar" · <hash> · <date> · <channel>` — the complete
//!   build stamp for the About panel and `--version`.
//!
//! Both are single-sourced from [`info`] so the shell, `mde-shell-egui --version`
//! and `mackesd --version` can never print divergent build strings.

/// The stamped version — the workspace `CARGO_PKG_VERSION` at compile time.
const VERSION: &str = env!("MDE_BUILD_VERSION");
/// The stamped git short-hash, or the `nogit` sentinel for a git-less build.
const GIT_HASH: &str = env!("MDE_BUILD_GIT_HASH");
/// The stamped UTC build date (`YYYY-MM-DD`).
const BUILD_DATE: &str = env!("MDE_BUILD_DATE");
/// The stamped release channel (`dev` unless the packaging build overrides it).
const CHANNEL: &str = env!("MDE_BUILD_CHANNEL");

/// The platform codename for a semver major epoch — `12.x` → `"Quazar"`.
///
/// Unknown epochs return `""` so [`version_line`] degrades to a bare semver
/// rather than inventing a name. Keyed off the major so a `12.1`/`12.2` point
/// release stays "Quazar" without touching this map.
#[must_use]
pub const fn codename_for(major: u64) -> &'static str {
    match major {
        12 => "Quazar",
        _ => "",
    }
}

/// The immutable build identity baked into this binary.
///
/// Every field is a `'static` compile-time constant (`codename` is derived from
/// `version`), so [`info`] is allocation-free and the values match across every
/// binary built from the same tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildInfo {
    /// Semver version — the workspace `CARGO_PKG_VERSION` (`12.0.0`).
    pub version: &'static str,
    /// Codename for the version's major epoch (`"Quazar"`; `""` if unknown).
    pub codename: &'static str,
    /// Git short-hash at build time, or the `nogit` sentinel.
    pub git_hash: &'static str,
    /// UTC build date, `YYYY-MM-DD`.
    pub build_date: &'static str,
    /// Release channel (`dev` / `stable` / …).
    pub channel: &'static str,
}

/// Parse the semver major epoch from the stamped version (`"12.0.0"` → `12`).
fn major_epoch() -> u64 {
    VERSION
        .split('.')
        .next()
        .and_then(|part| part.parse::<u64>().ok())
        .unwrap_or(0)
}

/// The compile-time [`BuildInfo`] for this binary.
#[must_use]
pub fn info() -> BuildInfo {
    BuildInfo {
        version: VERSION,
        codename: codename_for(major_epoch()),
        git_hash: GIT_HASH,
        build_date: BUILD_DATE,
        channel: CHANNEL,
    }
}

/// The short brand line — semver plus the quoted codename — for the shell chrome
/// and the boot-splash: `12.0.0 "Quazar"`. The codename is omitted (bare semver)
/// when the epoch has no name.
#[must_use]
pub fn version_line() -> String {
    let info = info();
    if info.codename.is_empty() {
        info.version.to_owned()
    } else {
        format!("{} \"{}\"", info.version, info.codename)
    }
}

/// The complete build-identity line for the About panel and `--version`:
/// `12.0.0 "Quazar" · <hash> · <date> · <channel>`. Reuses [`version_line`] for
/// the leading brand line so the two never drift.
#[must_use]
pub fn full() -> String {
    let info = info();
    format!(
        "{} · {} · {} · {}",
        version_line(),
        info.git_hash,
        info.build_date,
        info.channel,
    )
}

/// [`full`] as a memoized `&'static str`.
///
/// For consumers that need a `'static` version string rather than an owned
/// `String` — notably clap's `#[command(version = …)]`, whose `Str` only
/// converts from `&'static str`. Reuses [`full`] (computed once on first call),
/// so there is still exactly one build-string source.
#[must_use]
pub fn full_static() -> &'static str {
    use std::sync::OnceLock;
    static FULL: OnceLock<String> = OnceLock::new();
    FULL.get_or_init(full).as_str()
}

#[cfg(test)]
mod tests {
    use super::{codename_for, full, full_static, info, version_line, VERSION};

    #[test]
    fn codename_maps_the_quazar_epoch_and_blanks_the_unknown() {
        assert_eq!(codename_for(12), "Quazar");
        assert!(codename_for(11).is_empty());
        assert!(codename_for(99).is_empty());
    }

    #[test]
    fn version_line_is_semver_plus_quoted_codename() {
        // The workspace is 12.0.0 → the "Quazar" epoch, so the shape is
        // `<semver> "Quazar"` (the chrome/splash line the design locks).
        assert_eq!(version_line(), format!("{VERSION} \"Quazar\""));
        assert!(version_line().starts_with(VERSION));
        assert!(version_line().contains("\"Quazar\""));
    }

    #[test]
    fn full_contains_version_hash_date_and_channel() {
        let line = full();
        let info = info();
        assert!(line.contains(info.version), "missing version: {line}");
        assert!(line.contains(info.git_hash), "missing git hash: {line}");
        assert!(line.contains(info.build_date), "missing date: {line}");
        assert!(line.contains(info.channel), "missing channel: {line}");
        // full() carries the whole brand line, codename included.
        assert!(line.contains("\"Quazar\""), "missing codename: {line}");
    }

    #[test]
    fn full_static_matches_full_and_is_stable() {
        assert_eq!(full_static(), full());
        // Memoized: the same pointer on every call.
        assert!(std::ptr::eq(full_static(), full_static()));
    }

    #[test]
    fn info_fields_are_all_populated() {
        let info = info();
        assert!(!info.version.is_empty());
        // May be the `nogit` sentinel on a git-less build, but never empty.
        assert!(!info.git_hash.is_empty());
        assert!(!info.build_date.is_empty());
        assert!(!info.channel.is_empty());
        // Codename is "Quazar" for the current 12.x epoch.
        assert_eq!(info.codename, "Quazar");
    }
}
