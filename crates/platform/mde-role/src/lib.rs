//! `mde-role` — the pinned deployment role (E1.1).
//!
//! `Lighthouse ⊂ Server ⊂ Workstation`, each a strict capability superset
//! (CLAUDE.md §1: Lighthouse relay ⊂ Server headless ⊂ Workstation desktop).
//! The role is chosen once at install time (the role chooser / `mde-role`)
//! and written to
//! [`default_role_path`] (`/var/lib/mde/role.toml`). Thereafter it can only be
//! **upgraded** to an equal-or-higher rank; a downgrade is refused and the file
//! is left byte-for-byte unchanged, so a box never silently loses the rank it
//! was deployed as.
//!
//! Every role-gated path (the `mackesd` worker subsets of E1.2, the role-gated
//! surface install, the systemd templates of E1.3) reads the role solely
//! through [`load`]. A **missing or malformed** file is a [`LoadError`], never
//! a default — callers fail closed (lowest privilege / refuse), they must not
//! assume `Workstation`.

#![forbid(unsafe_code)]

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// A deployment role. Each variant is a strict superset of the one below it;
/// [`Role::rank`] gives the total order the upgrade-only invariant compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    /// Relay-only mesh node — Nebula overlay + the `mackesd` control plane,
    /// no storage brick, no desktop. Rank 0. VPS-friendly.
    Lighthouse,
    /// Headless mesh peer — Lighthouse + a storage brick + fleet/monitoring
    /// workers. No desktop. Rank 1.
    Server,
    /// Full workstation — Server + the Cosmic desktop. Rank 2.
    Workstation,
}

impl Role {
    /// Capability rank; a higher number is a richer superset.
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::Lighthouse => 0,
            Self::Server => 1,
            Self::Workstation => 2,
        }
    }

    /// Lowercase canonical name — the `--profile=` argument and the value
    /// written to `role.toml`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Lighthouse => "lighthouse",
            Self::Server => "server",
            Self::Workstation => "workstation",
        }
    }

    /// All roles, lowest rank first.
    #[must_use]
    pub const fn all() -> [Self; 3] {
        [Self::Lighthouse, Self::Server, Self::Workstation]
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A `--profile=` argument or `role.toml` value that doesn't name a role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseRoleError(pub String);

impl fmt::Display for ParseRoleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown role: {} (choose lighthouse|server|workstation)",
            self.0
        )
    }
}

impl std::error::Error for ParseRoleError {}

impl FromStr for Role {
    type Err = ParseRoleError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // The governance names are canonical; the two `mde-installer` profile
        // spellings (`headless`/`full`) are accepted as aliases so a role
        // pinned via either vocabulary resolves (E1.4 bridges the installer).
        match s.trim().to_ascii_lowercase().as_str() {
            "lighthouse" => Ok(Self::Lighthouse),
            "server" | "headless" => Ok(Self::Server),
            "workstation" | "full" => Ok(Self::Workstation),
            other => Err(ParseRoleError(other.to_string())),
        }
    }
}

/// Canonical on-disk location of the pinned role.
///
/// Honors `MDE_ROLE_PATH` when set — so a test, a containerized
/// `mackesd serve`, or a non-root tool can redirect the role file off the
/// privileged `/var/lib/mde/` default without threading a path through
/// every `load()`/`pin()` caller. Unset → the canonical system path.
#[must_use]
pub fn default_role_path() -> PathBuf {
    if let Some(p) = std::env::var_os("MDE_ROLE_PATH") {
        return PathBuf::from(p);
    }
    PathBuf::from("/var/lib/mde/role.toml")
}

/// Why [`load`] / [`load_from`] couldn't yield a role. Callers treat **any**
/// variant as fail-closed (lowest privilege / refuse) — never a default.
#[derive(Debug)]
pub enum LoadError {
    /// No `role.toml` at the path — the box has not been role-pinned at install.
    NotPinned,
    /// The file exists but couldn't be read.
    Io(std::io::Error),
    /// The file exists but carries no parseable `role = "<name>"` value.
    Malformed(String),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotPinned => write!(
                f,
                "no deployment role pinned (set one at install via the role chooser)"
            ),
            Self::Io(e) => write!(f, "reading role.toml: {e}"),
            Self::Malformed(m) => write!(f, "malformed role.toml: {m}"),
        }
    }
}

impl std::error::Error for LoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

/// Read the pinned role from [`default_role_path`]. See [`load_from`].
pub fn load() -> Result<Role, LoadError> {
    load_from(&default_role_path())
}

/// Read the pinned role from `path`: [`LoadError::NotPinned`] when the file is
/// absent, [`LoadError::Malformed`] when it lacks a parseable `role` value.
/// Callers fail closed on either.
pub fn load_from(path: &Path) -> Result<Role, LoadError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(LoadError::NotPinned),
        Err(e) => return Err(LoadError::Io(e)),
    };
    parse_role_toml(&text)
        .ok_or_else(|| LoadError::Malformed(format!("no valid `role` value in {}", path.display())))
}

/// Extract the role from a `role.toml` body — the value of the first
/// `role = "<name>"` line, ignoring `#` comments and surrounding whitespace.
/// `None` when no such line names a known role.
fn parse_role_toml(text: &str) -> Option<Role> {
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("role") {
            if let Some(val) = rest.trim_start().strip_prefix('=') {
                let val = val.trim().trim_matches('"').trim();
                return val.parse::<Role>().ok();
            }
        }
    }
    None
}

/// What [`pin`] / [`pin_at`] did to the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinOutcome {
    /// First pin — `role.toml` didn't exist and was created.
    Pinned(Role),
    /// Re-pinned to a strictly higher rank.
    Upgraded {
        /// The previously pinned (lower) role.
        from: Role,
        /// The newly pinned (higher) role.
        to: Role,
    },
    /// Re-pinned to the same rank — idempotent (the file is rewritten
    /// with identical content).
    Unchanged(Role),
}

impl PinOutcome {
    /// The role now on disk.
    #[must_use]
    pub const fn role(self) -> Role {
        match self {
            Self::Pinned(r) | Self::Unchanged(r) => r,
            Self::Upgraded { to, .. } => to,
        }
    }
}

impl fmt::Display for PinOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let role = self.role();
        match self {
            Self::Pinned(_) => write!(f, "pinned role {role} (rank {})", role.rank()),
            Self::Unchanged(_) => {
                write!(f, "role already {role} (rank {}); unchanged", role.rank())
            }
            Self::Upgraded { from, to } => write!(
                f,
                "upgraded role {from} (rank {}) -> {to} (rank {})",
                from.rank(),
                to.rank()
            ),
        }
    }
}

/// Why [`pin`] / [`pin_at`] refused. On refusal the file is left untouched.
#[derive(Debug)]
pub enum PinError {
    /// The requested role is a lower rank than the pinned one — refused; the
    /// file is byte-for-byte unchanged.
    Downgrade {
        /// The pinned (higher) role kept in place.
        from: Role,
        /// The refused (lower) requested role.
        to: Role,
    },
    /// The existing `role.toml` is malformed — refuse to classify the
    /// transition (and so refuse to silently overwrite a corrupt pin). The
    /// file is left untouched; remove it to re-pin from scratch.
    MalformedExisting(String),
    /// Filesystem error while writing the new pin.
    Io(std::io::Error),
}

impl fmt::Display for PinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Downgrade { from, to } => write!(
                f,
                "downgrade blocked: pinned role is {from} (rank {}), refusing {to} (rank {}); role.toml unchanged",
                from.rank(),
                to.rank()
            ),
            Self::MalformedExisting(m) => write!(
                f,
                "existing role.toml is malformed ({m}); remove it to re-pin"
            ),
            Self::Io(e) => write!(f, "writing role.toml: {e}"),
        }
    }
}

impl std::error::Error for PinError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

/// Pin `role` at [`default_role_path`]. See [`pin_at`].
pub fn pin(role: Role) -> Result<PinOutcome, PinError> {
    pin_at(&default_role_path(), role)
}

/// Pin `role` at `path`, enforcing the upgrade-only invariant:
///
/// | current on disk      | result                                   |
/// |----------------------|------------------------------------------|
/// | absent               | write → [`PinOutcome::Pinned`]            |
/// | same rank            | rewrite (idempotent) → [`PinOutcome::Unchanged`] |
/// | strictly higher rank | write → [`PinOutcome::Upgraded`]          |
/// | strictly lower rank  | REFUSE, file untouched → [`PinError::Downgrade`] |
/// | malformed            | REFUSE, file untouched → [`PinError::MalformedExisting`] |
///
/// The write is atomic (temp file + rename) so a crash never leaves a
/// half-written pin; the refusal paths never open the file for writing, so the
/// downgrade case leaves it byte-for-byte identical.
pub fn pin_at(path: &Path, role: Role) -> Result<PinOutcome, PinError> {
    let outcome = match load_from(path) {
        Err(LoadError::NotPinned) => PinOutcome::Pinned(role),
        Err(LoadError::Malformed(m)) => return Err(PinError::MalformedExisting(m)),
        Err(LoadError::Io(e)) => return Err(PinError::Io(e)),
        Ok(current) => match role.rank().cmp(&current.rank()) {
            std::cmp::Ordering::Less => {
                return Err(PinError::Downgrade {
                    from: current,
                    to: role,
                })
            }
            std::cmp::Ordering::Equal => PinOutcome::Unchanged(role),
            std::cmp::Ordering::Greater => PinOutcome::Upgraded {
                from: current,
                to: role,
            },
        },
    };
    write_atomic(path, role).map_err(PinError::Io)?;
    Ok(outcome)
}

/// Write `role.toml` atomically: a sibling temp file + rename, creating the
/// parent directory (`/var/lib/mde`) if needed.
fn write_atomic(path: &Path, role: Role) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = format!(
        "# Magic Mesh deployment role — pinned at install by the role chooser.\n\
         # Upgrade-only: a lower rank is refused (E1.1).\n\
         # Rank: lighthouse 0  <  server 1  <  workstation 2.\n\
         role = \"{}\"\n",
        role.as_str()
    );
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique scratch path per test (no `tempfile` dep — keeps the crate
    /// zero-dependency). Caller removes it.
    fn scratch(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "mde-role-test-{}-{tag}-{:?}.toml",
            std::process::id(),
            std::thread::current().id()
        ))
    }

    #[test]
    fn rank_is_a_strict_total_order() {
        assert!(Role::Lighthouse.rank() < Role::Server.rank());
        assert!(Role::Server.rank() < Role::Workstation.rank());
        assert_eq!([0, 1, 2], Role::all().map(Role::rank));
    }

    #[test]
    fn parse_canonical_and_installer_aliases() {
        assert_eq!("lighthouse".parse(), Ok(Role::Lighthouse));
        assert_eq!("server".parse(), Ok(Role::Server));
        assert_eq!("workstation".parse(), Ok(Role::Workstation));
        // installer vocabulary aliases
        assert_eq!("headless".parse(), Ok(Role::Server));
        assert_eq!("full".parse(), Ok(Role::Workstation));
        // case-insensitive + trimmed
        assert_eq!("  WORKSTATION ".parse(), Ok(Role::Workstation));
        assert!("server-plus".parse::<Role>().is_err());
    }

    #[test]
    fn parse_role_toml_ignores_comments_and_quotes() {
        let body = "# header comment\n\nrole = \"server\"\n# trailing\n";
        assert_eq!(parse_role_toml(body), Some(Role::Server));
        assert_eq!(
            parse_role_toml("role=\"lighthouse\""),
            Some(Role::Lighthouse)
        );
        assert_eq!(parse_role_toml("# only comments\n"), None);
        assert_eq!(parse_role_toml("role = \"bogus\""), None);
    }

    #[test]
    fn load_from_absent_is_not_pinned() {
        let p = scratch("absent");
        let _ = std::fs::remove_file(&p);
        assert!(matches!(load_from(&p), Err(LoadError::NotPinned)));
    }

    #[test]
    fn load_from_malformed_is_malformed_not_a_default() {
        let p = scratch("malformed");
        std::fs::write(&p, "this is not a role file\n").unwrap();
        // Crucially: NOT Ok(Workstation) — a corrupt file fails closed.
        assert!(matches!(load_from(&p), Err(LoadError::Malformed(_))));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn pin_first_then_show_round_trips() {
        let p = scratch("first");
        let _ = std::fs::remove_file(&p);
        let out = pin_at(&p, Role::Server).expect("first pin");
        assert_eq!(out, PinOutcome::Pinned(Role::Server));
        assert_eq!(load_from(&p).expect("reload"), Role::Server);
        assert_eq!(load_from(&p).unwrap().rank(), 1);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn upgrade_is_allowed_same_rank_is_idempotent() {
        let p = scratch("upgrade");
        let _ = std::fs::remove_file(&p);
        pin_at(&p, Role::Lighthouse).expect("pin");
        // same rank → Unchanged
        assert_eq!(
            pin_at(&p, Role::Lighthouse).expect("same"),
            PinOutcome::Unchanged(Role::Lighthouse)
        );
        // higher rank → Upgraded
        assert_eq!(
            pin_at(&p, Role::Workstation).expect("upgrade"),
            PinOutcome::Upgraded {
                from: Role::Lighthouse,
                to: Role::Workstation
            }
        );
        assert_eq!(load_from(&p).unwrap(), Role::Workstation);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn downgrade_is_blocked_and_leaves_the_file_byte_for_byte_unchanged() {
        let p = scratch("downgrade");
        let _ = std::fs::remove_file(&p);
        pin_at(&p, Role::Workstation).expect("pin high");
        let before = std::fs::read(&p).expect("read before");
        let err = pin_at(&p, Role::Lighthouse).expect_err("downgrade must be refused");
        assert!(matches!(
            err,
            PinError::Downgrade {
                from: Role::Workstation,
                to: Role::Lighthouse
            }
        ));
        let after = std::fs::read(&p).expect("read after");
        assert_eq!(before, after, "role.toml must be byte-for-byte unchanged");
        assert_eq!(load_from(&p).unwrap(), Role::Workstation);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn pin_over_malformed_refuses_and_leaves_it_unchanged() {
        let p = scratch("pin-malformed");
        std::fs::write(&p, "garbage\n").unwrap();
        let before = std::fs::read(&p).unwrap();
        let err = pin_at(&p, Role::Workstation).expect_err("must refuse over malformed");
        assert!(matches!(err, PinError::MalformedExisting(_)));
        assert_eq!(std::fs::read(&p).unwrap(), before);
        let _ = std::fs::remove_file(&p);
    }
}
