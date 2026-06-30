//! `mde-role` — the pinned deployment role (E1.1).
//!
//! Two rank-ordered deployment roles — `Lighthouse` (the always-on relay /
//! control plane, no desktop) · `Workstation` (the full Quasar egui thin
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
/// relay / control plane) · **Workstation** (the full Quasar egui thin
/// client). "Headless" is not a role — a headless box is a `Workstation`
/// without a local display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    /// Relay + control plane — Nebula overlay, the `mackesd` control plane,
    /// the media server, and the CA/signer. Always-on, no desktop. Rank 0.
    /// VPS-friendly.
    Lighthouse,
    /// Full workstation — the Quasar stack: the egui-DRM shell + VDI + local
    /// KVM/cloud-hypervisor + Podman. A headless box is a `Workstation` with no
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

/// MEDIA-1 — a deployment **capability tag**: an orthogonal marker on top of the
/// [`Role`].
///
/// `AI_GOVERNANCE` §9 ("3 roles + capability tags"): the role is the install-time
/// identity; capabilities are orthogonal gating tags, NOT a 4th role. `Media`
/// marks a [`Role::Lighthouse`] as a **`Lighthouse_Media`** subclass: an
/// adequately-resourced lighthouse that hosts the Navidrome music service, so the
/// media container never lands on the tiny stock master
/// (`docs/design/media-lighthouse.md` lock #9). A non-lighthouse box carrying the
/// tag is a config error — [`Capability::applies_to`] refuses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    /// The media subclass — hosts the Navidrome / `music.mesh` service.
    /// Only valid on the [`Role::Lighthouse`] tier.
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

    /// Whether this capability is valid on `role`. `Media` is a **lighthouse
    /// subclass** (MEDIA-1) — only the [`Role::Lighthouse`] tier may carry it; a
    /// `Workstation` tagged media is rejected (the media class is a lighthouse
    /// subclass, never a peer/desktop capability).
    #[must_use]
    pub const fn applies_to(self, role: Role) -> bool {
        match self {
            Self::Media => matches!(role, Role::Lighthouse),
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

/// MEDIA-1 — a pinned deployment **class**: the [`Role`] plus its capability
/// tags.
///
/// This is what `role.toml` actually pins and what every gating decision (worker
/// tiers, the directory subclass marker) reads — the role alone answers "which
/// tier", the class answers "is this the `Lighthouse_Media` subclass". Keeping
/// the role and its tags together (rather than a 4th enum variant) is the §9
/// doctrine: a `Lighthouse_Media` box IS a Lighthouse (same rank, same relay
/// duties) that additionally carries [`Capability::Media`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleClass {
    /// The deployment role — the install-time identity / capability tier.
    pub role: Role,
    /// `true` when [`Capability::Media`] is set — the `Lighthouse_Media`
    /// subclass. Always `false` on a non-lighthouse role (the parser refuses an
    /// inapplicable tag).
    pub media: bool,
}

impl RoleClass {
    /// A plain role with no capability tags.
    #[must_use]
    pub const fn plain(role: Role) -> Self {
        Self { role, media: false }
    }

    /// Whether this box is the **`Lighthouse_Media`** subclass: the Lighthouse
    /// tier carrying [`Capability::Media`]. The single predicate every
    /// media-only gate (the Navidrome worker, `music.mesh` membership) asks.
    #[must_use]
    pub const fn is_media_lighthouse(&self) -> bool {
        matches!(self.role, Role::Lighthouse) && self.media
    }

    /// The class name surfaced in the snapshot / `mackesd role` output:
    /// `lighthouse_media` for the media subclass, else the plain role name.
    #[must_use]
    pub const fn class_str(&self) -> &'static str {
        if self.is_media_lighthouse() {
            "lighthouse_media"
        } else {
            self.role.as_str()
        }
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

/// MEDIA-1 — read the pinned role **plus its capability tags** from
/// [`default_role_path`].
///
/// See [`load_class_from`]. Use this (over [`load`]) where a gate needs to know
/// the `Lighthouse_Media` subclass, not just the tier.
///
/// # Errors
/// Same as [`load_from`]: [`LoadError::NotPinned`] / [`LoadError::Io`] /
/// [`LoadError::Malformed`].
pub fn load_class() -> Result<RoleClass, LoadError> {
    load_class_from(&default_role_path())
}

/// MEDIA-1 — read the role + capability tags from `path`.
///
/// The `media = true` capability is read off the same `role.toml`; an
/// inapplicable tag (e.g. `media` on a Server) is **dropped**, never promoted —
/// the role parse already succeeded, so a stray capability line never fails the
/// load (callers that only need the tier are unaffected). Same fail-closed
/// contract as [`load_from`].
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
    let media = parse_media_capability(&text) && Capability::Media.applies_to(role);
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

/// MEDIA-1 — pin a [`RoleClass`] (role + capability tags) at
/// [`default_role_path`]. See [`pin_class_at`].
///
/// # Errors
/// Same as [`pin_at`]: [`PinError::Downgrade`] / [`PinError::MalformedExisting`]
/// / [`PinError::Io`].
pub fn pin_class(class: &RoleClass) -> Result<PinOutcome, PinError> {
    pin_class_at(&default_role_path(), class)
}

/// MEDIA-1 — pin a [`RoleClass`] at `path`.
///
/// The same upgrade-only invariant on the role tier as [`pin_at`], additionally
/// persisting the `media` capability tag. The media tag is only written when
/// [`Capability::Media`] applies to the resolved role (a `Lighthouse_Media` pin
/// on a Server silently drops the tag — it is not a valid subclass), so
/// `role.toml` never records a contradictory class. The [`PinOutcome`] reports
/// the role transition exactly as [`pin_at`] (the capability tag is orthogonal to
/// the rank ordering — adding/clearing `media` on an already-pinned lighthouse is
/// a same-rank rewrite).
///
/// # Errors
/// [`PinError::Downgrade`] when `class.role` is a lower rank than the pinned one,
/// [`PinError::MalformedExisting`] over a corrupt pin, [`PinError::Io`] on a
/// write failure.
pub fn pin_class_at(path: &Path, class: &RoleClass) -> Result<PinOutcome, PinError> {
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

    // ── MEDIA-1: the media capability tag / Lighthouse_Media subclass ──

    #[test]
    fn media_capability_is_lighthouse_only() {
        // §9: media is a capability tag ON a lighthouse — never on Server/Workstation.
        assert!(Capability::Media.applies_to(Role::Lighthouse));
        assert!(!Capability::Media.applies_to(Role::Workstation));
        assert_eq!(Capability::Media.as_str(), "media");
        assert_eq!("media".parse(), Ok(Capability::Media));
        assert_eq!(Capability::all(), [Capability::Media]);
    }

    #[test]
    fn role_class_identifies_only_a_media_lighthouse() {
        // The media subclass = Lighthouse tier + the media tag.
        let media_lh = RoleClass {
            role: Role::Lighthouse,
            media: true,
        };
        assert!(media_lh.is_media_lighthouse());
        assert_eq!(media_lh.class_str(), "lighthouse_media");
        // A plain lighthouse is NOT the media subclass.
        assert!(!RoleClass::plain(Role::Lighthouse).is_media_lighthouse());
        assert_eq!(RoleClass::plain(Role::Lighthouse).class_str(), "lighthouse");
        // The 2 roles are intact and untagged by default.
        for r in Role::all() {
            assert!(!RoleClass::plain(r).is_media_lighthouse());
            assert_eq!(RoleClass::plain(r).class_str(), r.as_str());
        }
    }

    #[test]
    fn pin_class_persists_and_round_trips_the_media_tag() {
        let p = scratch("media-pin");
        let _ = std::fs::remove_file(&p);
        // Pin a Lighthouse_Media → role.toml carries `media = true`.
        let out = pin_class_at(
            &p,
            &RoleClass {
                role: Role::Lighthouse,
                media: true,
            },
        )
        .expect("pin media-lighthouse");
        assert_eq!(out, PinOutcome::Pinned(Role::Lighthouse));
        let class = load_class_from(&p).expect("reload class");
        assert!(class.is_media_lighthouse(), "the media tag round-trips");
        // The plain `load` still resolves the tier (the role line is unchanged).
        assert_eq!(load_from(&p).expect("tier"), Role::Lighthouse);
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
