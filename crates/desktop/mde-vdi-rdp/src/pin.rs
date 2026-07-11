//! Trust-on-first-use (TOFU) TLS certificate pinning for the RDP transport
//! (vdi-vm-6).
//!
//! # Why not hard validation
//!
//! RDP hosts overwhelmingly present **self-signed** certificates (xrdp mints one
//! per box; a Windows host's default cert is untrusted), so a rejecting chain
//! validator would refuse essentially every real connection. The mesh link is
//! also already carried over the **mutually-authenticated Nebula overlay**, which
//! is the transport-trust floor. So [`crate::connect`] deliberately does **not**
//! chain-validate the server certificate — see the module doc there.
//!
//! # What this adds
//!
//! Hard validation being off does not mean cert *changes* have to be invisible.
//! This module pins the SHA-256 fingerprint of each host's TLS **public key** on
//! the **first** connect (trust-on-first-use) and compares it on every later
//! connect (RFC 7469-style key pinning: a MITM must present a different key, so a
//! key change is the MITM signal, while a benign same-key certificate renewal
//! does not false-alarm). A fingerprint that **changed** is the MITM signal; the
//! connect layer
//! logs it loudly and surfaces it to the shell, while — by default — still
//! letting the connection through (the Nebula floor stays the trust anchor and a
//! self-signed cert legitimately rotates when a VDI VM is rebuilt). A strict
//! `reject-on-change` mode is available behind [`strict_mode`] for operators who
//! want the change to hard-fail.
//!
//! # Testable seams
//!
//! The pure decision — [`pin_decision`] (first-use / unchanged / changed) and
//! [`pin_action`] (what the connect layer does with that outcome given the strict
//! flag) — is separated from the live TLS handshake and the on-disk store so the
//! security logic is unit-tested without a server (governance §7).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use sha2::{Digest, Sha256};

/// The SHA-256 fingerprint of a server's DER-encoded TLS credential (the connect
/// layer feeds it the server's public key — see [`crate::connect`]).
///
/// This is the same fingerprint shape as `mde-kdc-host` (SHA-256, rendered as
/// upper-case hex with `:` between bytes) so the two pin stores read the same way
/// to an operator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fingerprint([u8; 32]);

impl Fingerprint {
    /// Fingerprint DER-encoded bytes (the server's TLS public key at the connect
    /// call site) with SHA-256.
    #[must_use]
    pub fn from_der(der: &[u8]) -> Self {
        let digest = Sha256::digest(der);
        let mut bytes = [0_u8; 32];
        bytes.copy_from_slice(&digest);
        Self(bytes)
    }

    /// Render the fingerprint as upper-case, colon-separated hex.
    #[must_use]
    pub fn to_hex(&self) -> String {
        let mut out = String::with_capacity(32 * 3);
        for (i, b) in self.0.iter().enumerate() {
            if i > 0 {
                out.push(':');
            }
            // `write!` into a `String` is infallible; the discard is intentional.
            let _ = write!(out, "{b:02X}");
        }
        out
    }

    /// Parse a fingerprint from its [`Fingerprint::to_hex`] rendering. Returns
    /// [`None`] for anything that is not exactly 32 colon-separated hex bytes, so
    /// a corrupt pin-store line is skipped rather than trusted.
    #[must_use]
    pub fn from_hex(text: &str) -> Option<Self> {
        let mut bytes = [0_u8; 32];
        let mut count = 0_usize;
        for part in text.split(':') {
            if count >= 32 || part.len() != 2 {
                return None;
            }
            bytes[count] = u8::from_str_radix(part, 16).ok()?;
            count += 1;
        }
        (count == 32).then_some(Self(bytes))
    }

    /// A short, log-friendly prefix (first 4 bytes) for operator messages.
    #[must_use]
    pub fn short(&self) -> String {
        let full = self.to_hex();
        full.split(':').take(4).collect::<Vec<_>>().join(":")
    }
}

/// The outcome of comparing a freshly observed fingerprint against the stored pin
/// for a host. This is the pure security decision — no I/O, no policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinOutcome {
    /// No pin was stored for this host — trust-on-first-use: record and accept.
    FirstUse,
    /// The observed fingerprint matched the stored pin — silent accept.
    Match,
    /// The observed fingerprint **differs** from the stored pin — the MITM
    /// signal. What happens next depends on [`pin_action`].
    Changed {
        /// The previously pinned fingerprint.
        stored: Fingerprint,
        /// The fingerprint just presented by the host.
        current: Fingerprint,
    },
}

impl PinOutcome {
    /// Did the host's certificate change since it was pinned?
    #[must_use]
    pub const fn is_change(&self) -> bool {
        matches!(self, Self::Changed { .. })
    }
}

/// The pure pin decision: compare an optionally-stored pin against the current
/// fingerprint. Extracted so the security logic is testable without a live TLS
/// handshake (the vdi-vm-6 seam).
#[must_use]
pub fn pin_decision(stored: Option<&Fingerprint>, current: &Fingerprint) -> PinOutcome {
    match stored {
        None => PinOutcome::FirstUse,
        Some(pinned) if pinned == current => PinOutcome::Match,
        Some(pinned) => PinOutcome::Changed {
            stored: pinned.clone(),
            current: current.clone(),
        },
    }
}

/// What the connect layer should do with a [`PinOutcome`], given whether strict
/// reject-on-change is enabled.
///
/// Pure, so both the accept-and-warn and the strict-reject branches are
/// unit-tested without a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinAction {
    /// Proceed with the connection and do not surface anything (first-use or an
    /// unchanged cert). First-use additionally records the pin — see
    /// [`PinOutcome::FirstUse`].
    Proceed,
    /// Proceed, but the cert changed: surface the change to the operator and
    /// re-pin to the new fingerprint (adopt-after-warn — a legit self-signed
    /// rotation should not spam a warning on every reconnect).
    Warn {
        /// The previously pinned fingerprint.
        stored: Fingerprint,
        /// The newly presented (and now adopted) fingerprint.
        current: Fingerprint,
    },
    /// Strict mode + a changed cert: refuse the connection. The old pin is kept
    /// so the change stays detectable on the next attempt.
    Reject {
        /// The previously pinned fingerprint.
        stored: Fingerprint,
        /// The fingerprint the host just presented.
        current: Fingerprint,
    },
}

/// Map a pin outcome + the strict flag to the connect-layer action. Pure seam.
#[must_use]
pub fn pin_action(outcome: &PinOutcome, strict: bool) -> PinAction {
    match outcome {
        PinOutcome::FirstUse | PinOutcome::Match => PinAction::Proceed,
        PinOutcome::Changed { stored, current } if strict => PinAction::Reject {
            stored: stored.clone(),
            current: current.clone(),
        },
        PinOutcome::Changed { stored, current } => PinAction::Warn {
            stored: stored.clone(),
            current: current.clone(),
        },
    }
}

/// Parse the strict-mode flag from a raw env value.
///
/// Truthy = `1`/`true`/`yes`/`on` (case-insensitive, trimmed). Anything else —
/// including unset — is off, so the non-breaking TOFU posture is the default.
/// Pure so it is tested without env.
#[must_use]
pub fn strict_from_raw(raw: Option<&str>) -> bool {
    matches!(
        raw.map(|s| s.trim().to_ascii_lowercase()).as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

/// Whether strict reject-on-change pinning is enabled (`MDE_RDP_STRICT_PIN`).
///
/// Default **off** — a changed cert is surfaced but not rejected, preserving the
/// Nebula-authenticated trust floor and self-signed rotations.
#[must_use]
pub fn strict_mode() -> bool {
    strict_from_raw(std::env::var("MDE_RDP_STRICT_PIN").ok().as_deref())
}

/// The pin-store key for one RDP endpoint (`host:port`).
#[must_use]
pub fn host_key(host: &str, port: u16) -> String {
    format!("{host}:{port}")
}

/// A per-host TLS fingerprint pin store: an in-memory `host:port → fingerprint`
/// map with an optional line-oriented backing file (`known_hosts`-style).
///
/// Persistence is best-effort — a store with no path (or an unwritable one) still
/// gives full process-lifetime change detection; only cross-restart memory is
/// lost. Load never fails on a corrupt line, it skips it.
#[derive(Debug, Default)]
pub struct PinStore {
    pins: BTreeMap<String, Fingerprint>,
    path: Option<PathBuf>,
}

impl PinStore {
    /// An empty, memory-only store (no persistence). Useful in tests.
    #[must_use]
    pub const fn in_memory() -> Self {
        Self {
            pins: BTreeMap::new(),
            path: None,
        }
    }

    /// Load the store from `path`, creating an empty (but path-backed) store if
    /// the file does not exist yet. Corrupt lines are skipped, never fatal.
    #[must_use]
    pub fn load(path: PathBuf) -> Self {
        let mut pins = BTreeMap::new();
        if let Ok(text) = std::fs::read_to_string(&path) {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let mut it = line.split_whitespace();
                if let (Some(host), Some(hex)) = (it.next(), it.next()) {
                    if let Some(fp) = Fingerprint::from_hex(hex) {
                        pins.insert(host.to_owned(), fp);
                    }
                }
            }
        }
        Self {
            pins,
            path: Some(path),
        }
    }

    /// The pure pin decision for `host_key` against `current`, without mutating
    /// the store (that is [`PinStore::record`]'s job).
    #[must_use]
    pub fn decision(&self, host_key: &str, current: &Fingerprint) -> PinOutcome {
        pin_decision(self.pins.get(host_key), current)
    }

    /// Record (or overwrite) the pin for `host_key` and persist the store if it is
    /// path-backed. A persistence failure is logged, never propagated — the
    /// in-memory pin still stands for the rest of the process.
    pub fn record(&mut self, host_key: String, fingerprint: Fingerprint) {
        self.pins.insert(host_key, fingerprint);
        if let Err(e) = self.save() {
            tracing::warn!(error = %e, "rdp cert pin store could not be persisted (in-memory pin still active)");
        }
    }

    /// The pinned fingerprint for `host_key`, if any.
    #[must_use]
    pub fn get(&self, host_key: &str) -> Option<&Fingerprint> {
        self.pins.get(host_key)
    }

    /// Serialise the store to its backing file (no-op for a memory-only store).
    ///
    /// # Errors
    /// [`std::io::Error`] if the parent directory cannot be created or the file
    /// cannot be written.
    fn save(&self) -> std::io::Result<()> {
        let Some(path) = self.path.as_deref() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut body = String::new();
        body.push_str("# mde RDP TLS pin store (trust-on-first-use, vdi-vm-6)\n");
        body.push_str("# <host:port> <sha256-der-fingerprint>\n");
        for (host, fp) in &self.pins {
            let _ = writeln!(body, "{host} {}", fp.to_hex());
        }
        write_private(path, body.as_bytes())
    }
}

/// Write `bytes` to `path`, restricting the file to the owner where the platform
/// supports it (the pins are not secret, but a known-hosts file is owner-scoped).
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// The default backing path for the pin store.
///
/// `$MDE_RDP_PIN_STORE` if set, else `$XDG_STATE_HOME/mde/rdp-known-hosts`, else
/// `$HOME/.local/state/mde/rdp-known-hosts`. [`None`] (memory-only) when no home
/// can be resolved.
#[must_use]
pub fn default_store_path() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("MDE_RDP_PIN_STORE") {
        if !explicit.is_empty() {
            return Some(PathBuf::from(explicit));
        }
    }
    let base = std::env::var_os("XDG_STATE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|v| !v.is_empty())
                .map(|home| PathBuf::from(home).join(".local").join("state"))
        })?;
    Some(base.join("mde").join("rdp-known-hosts"))
}

/// The process-global pin store, loaded once from [`default_store_path`].
///
/// Shared (and serialised) across all live RDP workers so concurrent connects to
/// different hosts do not clobber each other's persisted pins.
pub fn global_store() -> &'static Mutex<PinStore> {
    static STORE: OnceLock<Mutex<PinStore>> = OnceLock::new();
    STORE.get_or_init(|| {
        let store = default_store_path().map_or_else(PinStore::in_memory, PinStore::load);
        Mutex::new(store)
    })
}

/// Lock the global store, recovering from a poisoned mutex.
///
/// A panic in another worker while holding the lock must not wedge every later
/// connect — the pin map is plain data, so the recovered guard is safe to use.
/// Avoids `unwrap`.
pub fn lock_global() -> std::sync::MutexGuard<'static, PinStore> {
    global_store()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::{
        host_key, pin_action, pin_decision, strict_from_raw, Fingerprint, PinAction, PinOutcome,
        PinStore,
    };

    fn fp(seed: &[u8]) -> Fingerprint {
        Fingerprint::from_der(seed)
    }

    #[test]
    fn fingerprint_is_deterministic_and_input_sensitive() {
        assert_eq!(fp(b"cert-a"), fp(b"cert-a"));
        assert_ne!(fp(b"cert-a"), fp(b"cert-b"));
    }

    #[test]
    fn fingerprint_hex_round_trips() {
        let f = fp(b"round-trip");
        let hex = f.to_hex();
        // 32 bytes → 32 hex pairs joined by 31 colons.
        assert_eq!(hex.len(), 32 * 2 + 31);
        assert_eq!(Fingerprint::from_hex(&hex), Some(f));
        assert_eq!(Fingerprint::from_hex("not-a-fingerprint"), None);
        assert_eq!(Fingerprint::from_hex("AA:BB"), None, "too short");
    }

    #[test]
    fn pin_decision_covers_first_use_match_and_change() {
        let a = fp(b"host-cert");
        let b = fp(b"rotated-cert");
        assert_eq!(pin_decision(None, &a), PinOutcome::FirstUse);
        assert_eq!(pin_decision(Some(&a), &a), PinOutcome::Match);
        assert_eq!(
            pin_decision(Some(&a), &b),
            PinOutcome::Changed {
                stored: a,
                current: b,
            }
        );
    }

    #[test]
    fn pin_action_warns_by_default_but_rejects_in_strict_mode() {
        let a = fp(b"old");
        let b = fp(b"new");
        // First-use and match always proceed silently, strict or not.
        assert_eq!(pin_action(&PinOutcome::FirstUse, false), PinAction::Proceed);
        assert_eq!(pin_action(&PinOutcome::FirstUse, true), PinAction::Proceed);
        assert_eq!(pin_action(&PinOutcome::Match, true), PinAction::Proceed);

        let changed = PinOutcome::Changed {
            stored: a.clone(),
            current: b.clone(),
        };
        // Default (non-strict): a changed cert is surfaced, not rejected.
        assert_eq!(
            pin_action(&changed, false),
            PinAction::Warn {
                stored: a.clone(),
                current: b.clone(),
            }
        );
        // Strict: a changed cert is rejected.
        assert_eq!(
            pin_action(&changed, true),
            PinAction::Reject {
                stored: a,
                current: b,
            }
        );
    }

    #[test]
    fn strict_flag_parses_truthy_values_only() {
        for on in ["1", "true", "TRUE", "Yes", " on "] {
            assert!(strict_from_raw(Some(on)), "{on:?} should enable strict");
        }
        for off in ["0", "false", "", "no", "off", "maybe"] {
            assert!(!strict_from_raw(Some(off)), "{off:?} should not");
        }
        assert!(!strict_from_raw(None), "unset defaults to off");
    }

    #[test]
    fn store_first_connect_pins_then_reconnect_is_silent() {
        let mut store = PinStore::in_memory();
        let key = host_key("10.42.0.9", 3389);
        let cert = fp(b"xrdp-self-signed");
        // First connect: no pin → FirstUse → record.
        assert_eq!(store.decision(&key, &cert), PinOutcome::FirstUse);
        store.record(key.clone(), cert.clone());
        // Reconnect, same cert → silent Match.
        assert_eq!(store.decision(&key, &cert), PinOutcome::Match);
    }

    #[test]
    fn store_flags_a_changed_certificate() {
        let mut store = PinStore::in_memory();
        let key = host_key("10.42.0.9", 3389);
        let original = fp(b"original-cert");
        let attacker = fp(b"attacker-cert");
        store.record(key.clone(), original.clone());
        assert_eq!(
            store.decision(&key, &attacker),
            PinOutcome::Changed {
                stored: original,
                current: attacker,
            }
        );
    }

    #[test]
    fn store_persists_across_a_reload() {
        // A unique temp path so parallel tests never collide.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "mde-rdp-pin-persist-{}-{nanos}.known",
            std::process::id()
        ));
        let key = host_key("desktop.mesh", 3389);
        let cert = fp(b"persisted-cert");

        let mut store = PinStore::load(path.clone());
        assert_eq!(store.decision(&key, &cert), PinOutcome::FirstUse);
        store.record(key.clone(), cert.clone());

        // A fresh store loaded from the same file remembers the pin.
        let reloaded = PinStore::load(path.clone());
        assert_eq!(reloaded.get(&key), Some(&cert));
        assert_eq!(reloaded.decision(&key, &cert), PinOutcome::Match);

        let _ = std::fs::remove_file(&path);
    }
}
