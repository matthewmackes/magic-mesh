//! `mde-role` — the pinned deployment role (E1.1).
//!
//! Two rank-ordered deployment roles — `Lighthouse` (the always-on relay /
//! control plane, no desktop) · `Workstation` (the full Construct egui thin
//! client). Rank gives the upgrade-only order. "Headless" is not a role: a
//! headless box is a `Workstation` without a local display.
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

/// A deployment role — the install-time identity, rank-ordered for the
/// upgrade-only invariant ([`Role::rank`]): **Lighthouse** (the always-on
/// relay / control plane) · **Workstation** (the full Construct egui thin
/// client). "Headless" is not a role — a headless box is a `Workstation`
/// without a local display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    /// Thin relay + control plane — Nebula overlay, `mackesd`, etcd, and the
    /// CA/signer. Media and file-sharing duties stay on non-lighthouse hosts.
    /// Always-on, no desktop. Rank 0. VPS-friendly.
    Lighthouse,
    /// Full workstation — the Construct stack: the egui-DRM shell + VDI +
    /// libvirt/QEMU-KVM + Podman. A headless box is a `Workstation` with no
    /// local display. Rank 1. *(The retired XCP-NG/Server role folded in here;
    /// the legacy `xcpng`/`server`/`headless` slugs stay accepted aliases.)*
    Workstation,
}

impl Role {
    /// Capability rank; a higher number is a higher deployment tier (the
    /// upgrade-only invariant refuses a downgrade).
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::Lighthouse => 0,
            Self::Workstation => 1,
        }
    }

    /// Lowercase canonical name — the `--profile=` argument and the value
    /// written to `role.toml`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Lighthouse => "lighthouse",
            Self::Workstation => "workstation",
        }
    }

    /// All roles, lowest rank first.
    #[must_use]
    pub const fn all() -> [Self; 2] {
        [Self::Lighthouse, Self::Workstation]
    }
}

/// Retired media capability marker kept only so older `role.toml` files remain
/// parseable. New lighthouse pins must stay thin; this marker is never accepted
/// or activated. Media and file-sharing duties belong on non-lighthouse hosts.
///
/// Historical design notes described `Media` as a lighthouse subclass. That
/// subclass is retired; the marker remains only for legacy decoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    /// Legacy media marker. It is rejected by [`Capability::applies_to`].
    Media,
}

impl Capability {
    /// Canonical lowercase name — the `role.toml` capability key and the
    /// `--capability=` argument.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Media => "media",
        }
    }

    /// Every capability tag (one today).
    #[must_use]
    pub const fn all() -> [Self; 1] {
        [Self::Media]
    }

    /// Whether this capability is valid on `role`. The legacy media marker is
    /// retired and therefore valid on no role.
    #[must_use]
    pub const fn applies_to(self, _role: Role) -> bool {
        match self {
            Self::Media => false,
        }
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Capability {
    type Err = ParseRoleError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "media" => Ok(Self::Media),
            other => Err(ParseRoleError(other.to_string())),
        }
    }
}

/// A pinned deployment class: the [`Role`] plus legacy capability state. New
/// lighthouses always use the plain class; media state is rejected or cleared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleClass {
    /// The deployment role — the install-time identity / capability tier.
    pub role: Role,
    /// Legacy media state. Runtime loaders always clear it and pinning rejects
    /// it, so a live lighthouse is always the thin plain class.
    pub media: bool,
}

impl RoleClass {
    /// A plain role with no capability tags.
    #[must_use]
    pub const fn plain(role: Role) -> Self {
        Self { role, media: false }
    }

    /// Whether this box is a supported media lighthouse. Always `false`: the
    /// media/file-sharing lighthouse subclass is retired.
    #[must_use]
    pub const fn is_media_lighthouse(&self) -> bool {
        false
    }

    /// The class name surfaced in snapshots. Retired media state never changes
    /// the plain role name.
    #[must_use]
    pub const fn class_str(&self) -> &'static str {
        self.role.as_str()
    }
}

impl fmt::Display for RoleClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.class_str())
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
            "unknown role: {} (choose lighthouse|workstation)",
            self.0
        )
    }
}

impl std::error::Error for ParseRoleError {}

impl FromStr for Role {
    type Err = ParseRoleError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // The canonical 2-role names plus back-compat aliases. The retired
        // XCP-NG/Server role and the `mde-installer` `headless`/`full` spellings
        // all fold into `Workstation`, so a box pinned under any pre-2-role
        // vocabulary still resolves (a former server/headless box is now a
        // Workstation without a local display).
        match s.trim().to_ascii_lowercase().as_str() {
            "lighthouse" => Ok(Self::Lighthouse),
            "workstation" | "full" | "xcpng" | "xcp-ng" | "server" | "headless" => {
                Ok(Self::Workstation)
            }
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

/// Read the pinned role plus legacy capability state from [`default_role_path`].
///
/// `load_class` remains for callers migrating from the retired subclass.
///
/// # Errors
/// Same as [`load_from`]: [`LoadError::NotPinned`] / [`LoadError::Io`] /
/// [`LoadError::Malformed`].
pub fn load_class() -> Result<RoleClass, LoadError> {
    load_class_from(&default_role_path())
}

/// Read the role from `path`; a legacy `media = true` line is always dropped.
///
/// # Errors
/// [`LoadError::NotPinned`] when the file is absent, [`LoadError::Io`] on a read
/// error, [`LoadError::Malformed`] when no parseable `role` value is present.
pub fn load_class_from(path: &Path) -> Result<RoleClass, LoadError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(LoadError::NotPinned),
        Err(e) => return Err(LoadError::Io(e)),
    };
    let role = parse_role_toml(&text).ok_or_else(|| {
        LoadError::Malformed(format!("no valid `role` value in {}", path.display()))
    })?;
    // Thin-lighthouse policy: a legacy `media = true` marker must never revive
    // the retired Lighthouse_Media subclass after a daemon restart. Keep the
    // field parse-compatible for old files, but fail closed to the plain role.
    let _legacy_media = parse_media_capability(&text);
    let media = false;
    Ok(RoleClass { role, media })
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

/// MEDIA-1 — read the `media = true` capability tag from a `role.toml` body.
/// `true` only when an explicit `media` key parses to a truthy TOML boolean;
/// any absent / `false` / unparseable value is `false` (capabilities are
/// opt-in, fail-off). Tolerates `media = true`, `media="true"`, surrounding
/// whitespace, and `#` comments, mirroring [`parse_role_toml`].
fn parse_media_capability(text: &str) -> bool {
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("media") {
            if let Some(val) = rest.trim_start().strip_prefix('=') {
                let val = val.trim().trim_matches('"').trim();
                return val.eq_ignore_ascii_case("true");
            }
        }
    }
    false
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
    /// The retired media/file-sharing lighthouse capability was requested.
    /// Lighthouses are permanently thin control-plane nodes; the file is left
    /// untouched so a failed promotion cannot partially change the role.
    UnsupportedLighthouseCapability,
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
            Self::UnsupportedLighthouseCapability => write!(
                f,
                "media/file-sharing lighthouse capability is retired; lighthouses are thin control-plane nodes"
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

/// MEDIA-1 — pin a [`RoleClass`] (role + capability tags) at
/// [`default_role_path`]. See [`pin_class_at`].
///
/// # Errors
/// Same as [`pin_at`]: [`PinError::Downgrade`] / [`PinError::MalformedExisting`]
/// / [`PinError::Io`].
pub fn pin_class(class: &RoleClass) -> Result<PinOutcome, PinError> {
    pin_class_at(&default_role_path(), class)
}

/// Pin a [`RoleClass`] at `path`. Legacy media state is rejected for a
/// lighthouse and never persisted for any role.
///
/// # Errors
/// [`PinError::Downgrade`] when `class.role` is a lower rank than the pinned one,
/// [`PinError::MalformedExisting`] over a corrupt pin, [`PinError::Io`] on a
/// write failure.
pub fn pin_class_at(path: &Path, class: &RoleClass) -> Result<PinOutcome, PinError> {
    if class.media && matches!(class.role, Role::Lighthouse) {
        return Err(PinError::UnsupportedLighthouseCapability);
    }
    let outcome = match load_from(path) {
        Err(LoadError::NotPinned) => PinOutcome::Pinned(class.role),
        Err(LoadError::Malformed(m)) => return Err(PinError::MalformedExisting(m)),
        Err(LoadError::Io(e)) => return Err(PinError::Io(e)),
        Ok(current) => match class.role.rank().cmp(&current.rank()) {
            std::cmp::Ordering::Less => {
                return Err(PinError::Downgrade {
                    from: current,
                    to: class.role,
                })
            }
            std::cmp::Ordering::Equal => PinOutcome::Unchanged(class.role),
            std::cmp::Ordering::Greater => PinOutcome::Upgraded {
                from: current,
                to: class.role,
            },
        },
    };
    let media = class.media && Capability::Media.applies_to(class.role);
    write_atomic_class(path, class.role, media).map_err(PinError::Io)?;
    Ok(outcome)
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
/// parent directory (`/var/lib/mde`) if needed. No capability tags — see
/// [`write_atomic_class`].
fn write_atomic(path: &Path, role: Role) -> std::io::Result<()> {
    write_atomic_class(path, role, false)
}

/// MEDIA-1 — write `role.toml` atomically with the optional `media` capability
/// tag (`media = true` only when `media` is set — an off capability writes no
/// line, keeping a plain pin byte-for-byte identical to [`write_atomic`]).
fn write_atomic_class(path: &Path, role: Role, media: bool) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut body = format!(
        "# MCNF deployment role — pinned at install by the role chooser.\n\
         # Upgrade-only: a lower rank is refused (E1.1).\n\
         # Rank: lighthouse 0  <  workstation 1.\n\
         role = \"{}\"\n",
        role.as_str()
    );
    if media {
        body.push_str(
            "# MEDIA-1 capability tag: this lighthouse is the Lighthouse_Media subclass\n\
             # (hosts the Navidrome music service). Orthogonal to the role tier (§9).\n\
             media = true\n",
        );
    }
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
        assert!(Role::Lighthouse.rank() < Role::Workstation.rank());
        assert_eq!([0, 1], Role::all().map(Role::rank));
    }

    #[test]
    fn parse_canonical_and_legacy_aliases() {
        assert_eq!("lighthouse".parse(), Ok(Role::Lighthouse));
        assert_eq!("workstation".parse(), Ok(Role::Workstation));
        // The retired XCP-NG/Server role + installer spellings all fold into
        // Workstation (a former server/headless box is now a Workstation).
        assert_eq!("xcpng".parse(), Ok(Role::Workstation));
        assert_eq!("xcp-ng".parse(), Ok(Role::Workstation));
        assert_eq!("server".parse(), Ok(Role::Workstation));
        assert_eq!("headless".parse(), Ok(Role::Workstation));
        assert_eq!("full".parse(), Ok(Role::Workstation));
        // case-insensitive + trimmed
        assert_eq!("  WORKSTATION ".parse(), Ok(Role::Workstation));
        assert!("server-plus".parse::<Role>().is_err());
    }

    #[test]
    fn parse_role_toml_ignores_comments_and_quotes() {
        let body = "# header comment\n\nrole = \"server\"\n# trailing\n";
        assert_eq!(parse_role_toml(body), Some(Role::Workstation));
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
        let out = pin_at(&p, Role::Workstation).expect("first pin");
        assert_eq!(out, PinOutcome::Pinned(Role::Workstation));
        assert_eq!(load_from(&p).expect("reload"), Role::Workstation);
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

    // ── Retired media capability compatibility ──

    #[test]
    fn media_capability_is_retired_everywhere() {
        assert!(!Capability::Media.applies_to(Role::Lighthouse));
        assert!(!Capability::Media.applies_to(Role::Workstation));
        assert_eq!(Capability::Media.as_str(), "media");
        assert_eq!("media".parse(), Ok(Capability::Media));
        assert_eq!(Capability::all(), [Capability::Media]);
    }

    #[test]
    fn role_class_never_identifies_a_media_lighthouse() {
        let media_lh = RoleClass {
            role: Role::Lighthouse,
            media: true,
        };
        assert!(!media_lh.is_media_lighthouse());
        assert_eq!(media_lh.class_str(), "lighthouse");
        assert!(!RoleClass::plain(Role::Lighthouse).is_media_lighthouse());
        assert_eq!(RoleClass::plain(Role::Lighthouse).class_str(), "lighthouse");
        // The 2 roles are intact and untagged by default.
        for r in Role::all() {
            assert!(!RoleClass::plain(r).is_media_lighthouse());
            assert_eq!(RoleClass::plain(r).class_str(), r.as_str());
        }
    }

    #[test]
    fn pin_class_rejects_the_retired_media_lighthouse_capability() {
        let p = scratch("media-pin");
        let _ = std::fs::remove_file(&p);
        let err = pin_class_at(
            &p,
            &RoleClass {
                role: Role::Lighthouse,
                media: true,
            },
        )
        .expect_err("media lighthouse promotion must be refused");
        assert!(matches!(err, PinError::UnsupportedLighthouseCapability));
        assert!(!p.exists(), "refusal must not create role state");

        // A legacy hand-written marker is tolerated for parsing but demoted to
        // a plain lighthouse, so restart cannot revive the retired service.
        std::fs::write(&p, "role = \"lighthouse\"\nmedia = true\n").unwrap();
        let class = load_class_from(&p).expect("legacy class remains readable");
        assert!(!class.media);
        assert_eq!(class.class_str(), "lighthouse");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn plain_pin_carries_no_media_tag_and_is_unchanged_byte_for_byte() {
        // A class pin with media=false must produce the SAME bytes as the
        // legacy plain `pin_at` — the capability is opt-in, fail-off.
        let plain = scratch("plain-class");
        let legacy = scratch("plain-legacy");
        let _ = std::fs::remove_file(&plain);
        let _ = std::fs::remove_file(&legacy);
        pin_class_at(&plain, &RoleClass::plain(Role::Lighthouse)).expect("class pin");
        pin_at(&legacy, Role::Lighthouse).expect("legacy pin");
        assert_eq!(
            std::fs::read(&plain).unwrap(),
            std::fs::read(&legacy).unwrap(),
            "media=false writes no extra line"
        );
        assert!(!load_class_from(&plain).unwrap().is_media_lighthouse());
        let _ = std::fs::remove_file(&plain);
        let _ = std::fs::remove_file(&legacy);
    }

    #[test]
    fn media_tag_on_a_non_lighthouse_is_dropped_not_promoted() {
        // A media tag is meaningless on a Workstation — pinning one drops it, and
        // reading it back is a plain Workstation (never a phantom media-peer).
        let p = scratch("media-on-workstation");
        let _ = std::fs::remove_file(&p);
        pin_class_at(
            &p,
            &RoleClass {
                role: Role::Workstation,
                media: true,
            },
        )
        .expect("pin workstation");
        let class = load_class_from(&p).expect("reload");
        assert_eq!(class.role, Role::Workstation);
        assert!(!class.media, "media tag dropped on a non-lighthouse role");
        assert!(!class.is_media_lighthouse());
        // Belt-and-suspenders: even a hand-edited `media=true` under a non-lighthouse
        // role (the legacy `server` slug now resolves to Workstation) is ignored.
        std::fs::write(&p, "role = \"server\"\nmedia = true\n").unwrap();
        assert!(!load_class_from(&p).unwrap().media);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn parse_media_capability_is_opt_in_fail_off() {
        assert!(parse_media_capability(
            "role=\"lighthouse\"\nmedia = true\n"
        ));
        assert!(parse_media_capability("media=\"true\"\n"));
        // The key is the canonical lowercase `media` (as the writer emits, same
        // as the `role` key); the VALUE is case-insensitive.
        assert!(parse_media_capability("# c\nmedia = True\n"));
        assert!(!parse_media_capability("role=\"lighthouse\"\n"));
        assert!(!parse_media_capability("media = false\n"));
        assert!(!parse_media_capability("# media = true (commented)\n"));
        assert!(!parse_media_capability("media = bogus\n"));
    }
}
