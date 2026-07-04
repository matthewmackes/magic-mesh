//! QC-5 — real per-service secret sealing for the rendered Kolla config.
//!
//! QC-4 rendered every `OpenStack` service's connection string with a fixed
//! `SECRET_PLACEHOLDER` token — structurally complete, but no real credential.
//! QC-5 seals a real, per-service secret set on the mesh substrate and the
//! renderer ([`super::config_render`]) substitutes it in place of that token,
//! so a container starts against a genuine password (design Q21/24).
//!
//! ## Where the secrets live (design Q30 — one-state on the file substrate)
//!
//! The sealed set rides the Syncthing share as a TOML companion beside the
//! doctrine — `<workgroup_root>/cloud/secrets.toml`, mirroring QC-4's
//! `<workgroup_root>/cloud/doctrine.toml` idiom (always locally present, no
//! etcd round-trip on the read path). It is written `0600` (owner only) — a
//! real credential is readable only where the doctrine is enabled, never
//! world-readable on the share.
//!
//! ## The leader seals once; every other node reads (§7 — honest, not divergent)
//!
//! The single etcd `/mesh/leader` (Q15 — the same bit QC-4 folds off the
//! `/mesh/leader` lease) is the ONLY node that generates. On an absent file the
//! leader mints a fresh strong random set from the OS CSPRNG and writes it
//! atomically (tmp + rename) ONCE; every subsequent tick just reads it back. A
//! non-leader NEVER generates a divergent set — it reads the leader's file once
//! Syncthing propagates it, and until then the service stays honestly `Gated`
//! ("awaiting sealed secrets from leader"), never a blank password. Because the
//! values are *stored*, not re-derived, a given service always maps to the same
//! secret across nodes and ticks (the deterministic per-service key namespace
//! below indexes the same stored value everywhere).

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use rand::distributions::Alphanumeric;
use rand::rngs::OsRng;
use rand::Rng as _;
use serde::{Deserialize, Serialize};

/// Length of each generated secret. Alphanumeric (~5.95 bits/char) × 40 chars
/// ≈ 238 bits — well past any brute-force concern, and URL/INI-safe with no
/// escaping in the rendered connection strings.
const SECRET_LEN: usize = 40;

/// Every `OpenStack` service DB user Kolla connects as (design Q24 MVP set) — the
/// `[database] connection` password key namespace. The one-state doctrine seals
/// a distinct password per user.
const DB_USERS: &[&str] = &[
    "keystone",
    "glance",
    "placement",
    "nova",
    "nova_api",
    "neutron",
    "cinder",
];

/// Every Keystone service-user an API authenticates as (the
/// `[keystone_authtoken] password`, and the `[placement]` auth password Nova
/// carries). These are the machine service identities Keystone is bootstrapped
/// with — the mesh account being the *human* cloud account (Q21) is separate.
const SERVICE_USERS: &[&str] = &["glance", "placement", "nova", "neutron", "cinder"];

/// The single `RabbitMQ` `openstack` user password (Q16 — internal RPC, strictly
/// separate from mde-bus per Q67).
const RABBITMQ_KEY: &str = "rabbitmq_openstack";

/// The deterministic key for `user`'s DB password.
fn db_key(user: &str) -> String {
    format!("db_{user}")
}

/// The deterministic key for `user`'s Keystone service-user password.
fn service_user_key(user: &str) -> String {
    format!("svc_{user}")
}

/// The sealed per-service secret set.
///
/// Opaque + redacting: its [`fmt::Debug`] never prints a value, so a
/// `{ctx:?}` / `?secrets` reaching a log line can't leak a credential (§7 — no
/// secret in logs). Serialized as a `[secrets]` TOML table (deterministic
/// `BTreeMap` order) for the sealed companion.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Secrets {
    /// key → secret: `db_<user>`, `svc_<user>`, `rabbitmq_openstack`.
    secrets: BTreeMap<String, String>,
}

impl fmt::Debug for Secrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never print the values — the key count only (§7). Keys are structural
        // (`db_nova`, …), values are the credentials we must never surface.
        f.debug_struct("Secrets")
            .field("sealed_keys", &self.secrets.len())
            .field("values", &"<redacted>")
            .finish()
    }
}

impl Secrets {
    /// Mint a fresh strong random secret for every canonical key from the OS
    /// CSPRNG (`OsRng` → `getrandom`, the same source the mesh mints node keys
    /// and bearer tokens from). Complete by construction.
    pub(crate) fn generate() -> Self {
        let mut secrets = BTreeMap::new();
        for u in DB_USERS {
            secrets.insert(db_key(u), random_secret());
        }
        for u in SERVICE_USERS {
            secrets.insert(service_user_key(u), random_secret());
        }
        secrets.insert(RABBITMQ_KEY.to_string(), random_secret());
        Self { secrets }
    }

    /// Parse the sealed set from its TOML companion body.
    fn from_toml(body: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(body)
    }

    /// Render the sealed set to its TOML companion body (a `[secrets]` table)
    /// with a machine-managed provenance header.
    fn to_toml(&self) -> Result<String, toml::ser::Error> {
        let table = toml::to_string_pretty(self)?;
        Ok(format!(
            "# QC-5 sealed OpenStack service secrets — generated by the mackesd\n\
             # openstack worker on the etcd leader (Q15). Machine-managed; do not\n\
             # edit. Rotate by deleting this file on the leader to re-seal a fresh\n\
             # set; every node re-reads it over the Syncthing share.\n\
             {table}"
        ))
    }

    /// The first canonical key that is absent or empty — the renderer's
    /// completeness gate. `None` ⇒ a full, non-blank set (safe to substitute);
    /// `Some(key)` ⇒ gate the render (never substitute a blank credential, §7).
    #[must_use]
    pub fn first_missing(&self) -> Option<String> {
        for u in DB_USERS {
            let k = db_key(u);
            if self.secrets.get(&k).is_none_or(String::is_empty) {
                return Some(k);
            }
        }
        for u in SERVICE_USERS {
            let k = service_user_key(u);
            if self.secrets.get(&k).is_none_or(String::is_empty) {
                return Some(k);
            }
        }
        if self.secrets.get(RABBITMQ_KEY).is_none_or(String::is_empty) {
            return Some(RABBITMQ_KEY.to_string());
        }
        None
    }

    /// `user`'s sealed DB password (`[database] connection`). The renderer's
    /// [`Self::first_missing`] gate runs before any lookup, so a canonical user
    /// always resolves to a real, non-blank secret.
    #[must_use]
    pub fn db_password(&self, user: &str) -> &str {
        self.get(&db_key(user))
    }

    /// `user`'s sealed Keystone service-user password (`[keystone_authtoken]` /
    /// `[placement]`).
    #[must_use]
    pub fn service_user_password(&self, user: &str) -> &str {
        self.get(&service_user_key(user))
    }

    /// The sealed `RabbitMQ` `openstack` user password (the oslo.messaging
    /// transport URL + the broker's `default_pass`).
    #[must_use]
    pub fn rabbitmq_password(&self) -> &str {
        self.get(RABBITMQ_KEY)
    }

    /// Look a key up. Guarded by [`Self::first_missing`] at the render entry, so
    /// an empty fallback here is never rendered into a live config; it exists
    /// only so a caller that skips the gate degrades to a blank (which fails a
    /// login) rather than panicking a worker thread.
    fn get(&self, key: &str) -> &str {
        self.secrets.get(key).map_or("", String::as_str)
    }
}

/// One strong alphanumeric secret from the OS CSPRNG (uniform, unbiased — the
/// `Alphanumeric` distribution rejection-samples the 62-char alphabet).
fn random_secret() -> String {
    OsRng
        .sample_iter(Alphanumeric)
        .take(SECRET_LEN)
        .map(char::from)
        .collect()
}

/// What the renderer knows about the sealed secrets this tick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretView {
    /// Sealed secrets are present — the renderer substitutes real passwords.
    Sealed(Secrets),
    /// Not available this tick — the sharp reason (awaiting the leader /
    /// malformed / unreadable). Every desired service gates on it, never a
    /// blank or fabricated password (§7).
    Unsealed(String),
}

/// The sealed secrets' TOML companion on the Syncthing share (beside the
/// doctrine — QC-4's `<workgroup_root>/cloud/` idiom).
#[must_use]
pub fn secrets_toml_path(workgroup_root: &Path) -> PathBuf {
    workgroup_root.join("cloud").join("secrets.toml")
}

/// Resolve the sealed secrets for this tick (design: the leader seals once,
/// every other node reads).
///
/// - present + parseable → [`SecretView::Sealed`];
/// - present + unparseable → [`SecretView::Unsealed`] (malformed reason; §7 —
///   never a guessed/fabricated set);
/// - absent + `leader` → mint a fresh set from the OS CSPRNG, seal it `0600`
///   atomically (tmp + rename), return [`SecretView::Sealed`]; a write failure
///   → `Unsealed` (honest, retried next tick);
/// - absent + non-leader → [`SecretView::Unsealed`] ("awaiting sealed secrets
///   from leader") — the service gates until Syncthing propagates the file.
#[must_use]
pub fn load_or_seal(workgroup_root: &Path, leader: bool) -> SecretView {
    let path = secrets_toml_path(workgroup_root);
    match std::fs::read_to_string(&path) {
        Ok(body) => match Secrets::from_toml(&body) {
            Ok(secrets) => SecretView::Sealed(secrets),
            Err(e) => SecretView::Unsealed(format!(
                "the sealed secrets companion {} is malformed — {e}; refusing to \
                 render a fabricated credential (re-seal by deleting it on the leader)",
                path.display()
            )),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if leader {
                seal_fresh(&path)
            } else {
                SecretView::Unsealed(
                    "awaiting sealed secrets from leader — the etcd leader mints the \
                     OpenStack service secret set once, and this node reads it off the \
                     Syncthing share (not yet propagated here)"
                        .to_string(),
                )
            }
        }
        Err(e) => SecretView::Unsealed(format!(
            "the sealed secrets companion {} is unreadable — {e}",
            path.display()
        )),
    }
}

/// The leader's one-time seal: mint + atomically write the fresh set `0600`.
fn seal_fresh(path: &Path) -> SecretView {
    let secrets = Secrets::generate();
    let body = match secrets.to_toml() {
        Ok(b) => b,
        Err(e) => {
            return SecretView::Unsealed(format!("could not serialize a fresh secret set — {e}"))
        }
    };
    match write_sealed(path, &body) {
        Ok(()) => SecretView::Sealed(secrets),
        Err(e) => SecretView::Unsealed(format!(
            "the leader could not seal {} — {e}; will retry next tick",
            path.display()
        )),
    }
}

/// Atomic `0600` write (tmp + rename), creating `<workgroup_root>/cloud/`. The
/// tmp file is created `0600` so the secret is never briefly world-readable on
/// the share before the rename.
fn write_sealed(path: &Path, body: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    write_private(&tmp, body)?;
    std::fs::rename(&tmp, path)
}

#[cfg(unix)]
fn write_private(path: &Path, body: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    // `mode(..)` only applies at creation; enforce it even if a crashed tick
    // left a looser-permissioned tmp behind.
    f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    f.write_all(body.as_bytes())?;
    f.sync_all()
}

#[cfg(not(unix))]
fn write_private(path: &Path, body: &str) -> std::io::Result<()> {
    std::fs::write(path, body.as_bytes())
}

#[cfg(test)]
impl Secrets {
    /// Test-only: a sealed set with `key` dropped, to exercise a caller's
    /// completeness gate (the renderer's `first_missing` check).
    pub(crate) fn dropping_for_test(mut self, key: &str) -> Self {
        self.secrets.remove(key);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_seals_every_canonical_key_non_blank() {
        let s = Secrets::generate();
        // Complete by construction — the render gate sees nothing missing.
        assert!(s.first_missing().is_none(), "generated set is complete");
        for u in DB_USERS {
            assert_eq!(s.db_password(u).len(), SECRET_LEN, "db_{u}");
        }
        for u in SERVICE_USERS {
            assert_eq!(s.service_user_password(u).len(), SECRET_LEN, "svc_{u}");
        }
        assert_eq!(s.rabbitmq_password().len(), SECRET_LEN);
    }

    #[test]
    fn generated_secrets_are_alphanumeric_and_distinct() {
        let s = Secrets::generate();
        // URL/INI-safe: no escaping needed in a connection string.
        assert!(s
            .db_password("nova")
            .chars()
            .all(|c| c.is_ascii_alphanumeric()));
        // Distinct per service (independent draws) — a leaked nova password is
        // not the cinder password.
        assert_ne!(s.db_password("nova"), s.db_password("cinder"));
        assert_ne!(s.db_password("nova"), s.service_user_password("nova"));
        assert_ne!(s.rabbitmq_password(), s.db_password("keystone"));
    }

    #[test]
    fn leader_seals_once_and_a_non_leader_reads_the_same_set() {
        let root = tempfile::tempdir().unwrap();
        // The leader mints + seals.
        let SecretView::Sealed(sealed) = load_or_seal(root.path(), true) else {
            unreachable!("the leader must seal a fresh set");
        };
        // The file landed 0600 on the share.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let meta = std::fs::metadata(secrets_toml_path(root.path())).unwrap();
            assert_eq!(meta.permissions().mode() & 0o777, 0o600, "sealed 0600");
        }
        // A second leader tick does NOT regenerate — it reads the same set back.
        let SecretView::Sealed(again) = load_or_seal(root.path(), true) else {
            unreachable!("a present file reads back Sealed");
        };
        assert_eq!(
            again.db_password("nova"),
            sealed.db_password("nova"),
            "the leader seals ONCE"
        );
        // A non-leader on the same share reads the identical secrets (§7 — same
        // input → same password, never a divergent generation).
        let SecretView::Sealed(non_leader) = load_or_seal(root.path(), false) else {
            unreachable!("a non-leader reads the present file");
        };
        assert_eq!(non_leader.db_password("nova"), sealed.db_password("nova"));
        assert_eq!(non_leader.rabbitmq_password(), sealed.rabbitmq_password());
    }

    #[test]
    fn a_non_leader_without_a_file_awaits_the_leader() {
        let root = tempfile::tempdir().unwrap();
        let SecretView::Unsealed(reason) = load_or_seal(root.path(), false) else {
            unreachable!("a non-leader must NOT generate");
        };
        assert!(
            reason.contains("awaiting sealed secrets from leader"),
            "{reason}"
        );
        // ...and it wrote nothing (never a divergent seal).
        assert!(!secrets_toml_path(root.path()).exists());
    }

    #[test]
    fn a_malformed_companion_gates_rather_than_fabricating() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("cloud")).unwrap();
        std::fs::write(secrets_toml_path(root.path()), "this = = not toml").unwrap();
        let SecretView::Unsealed(reason) = load_or_seal(root.path(), true) else {
            unreachable!("a malformed file must gate, even for the leader");
        };
        assert!(reason.contains("malformed"), "{reason}");
        assert!(reason.contains("secrets.toml"), "{reason}");
    }

    #[test]
    fn an_incomplete_set_is_reported_missing() {
        // A hand-edited file that parsed but dropped a key → first_missing names
        // it → the renderer gates rather than substituting a blank.
        let mut s = Secrets::generate();
        s.secrets.remove("db_nova");
        assert_eq!(s.first_missing().as_deref(), Some("db_nova"));
        // An empty value counts as missing too.
        let mut s2 = Secrets::generate();
        s2.secrets
            .insert("rabbitmq_openstack".to_string(), String::new());
        assert_eq!(s2.first_missing().as_deref(), Some("rabbitmq_openstack"));
    }

    #[test]
    fn toml_round_trips_and_carries_the_provenance_header() {
        let s = Secrets::generate();
        let body = s.to_toml().unwrap();
        assert!(body.contains("QC-5 sealed"), "provenance header present");
        assert!(body.contains("[secrets]"), "a [secrets] table");
        let back = Secrets::from_toml(&body).unwrap();
        assert_eq!(back, s, "round-trips through the companion format");
    }

    #[test]
    fn debug_never_prints_a_secret_value() {
        let s = Secrets::generate();
        let dbg = format!("{s:?}");
        assert!(dbg.contains("<redacted>"), "{dbg}");
        // No sealed value leaks into the Debug output.
        assert!(!dbg.contains(s.db_password("nova")), "leaked db password");
        assert!(
            !dbg.contains(s.rabbitmq_password()),
            "leaked rabbit password"
        );
        // The SecretView wrapping it is redacted too.
        let view = SecretView::Sealed(s.clone());
        let vdbg = format!("{view:?}");
        assert!(!vdbg.contains(s.db_password("nova")), "{vdbg}");
    }
}
