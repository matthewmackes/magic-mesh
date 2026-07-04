//! The on-disk pairing store at `~/.config/mde/connect/`.
//!
//! The protocol crate deliberately owns no filesystem and no RSA keygen, so the
//! host provides both here. [`PairingStore`]:
//!
//! - generates (once) and persists this host's RSA-4096 identity key as
//!   `identity.pkcs8` (PKCS#8 DER, mode 0600) via [`crate::keygen`] — which the
//!   protocol crate can't (ring ships no RSA keygen) — and signs with the
//!   protocol's ring-backed [`PairingKeyPair`];
//! - persists the trusted-peer records as `devices.toml` (atomic write);
//! - implements the protocol's [`mde_kdc_proto::crypto::KeyStore`], delegating
//!   ephemeral AES session keys to an in-memory [`RingKeyStore`] (only the
//!   long-lived device records ever touch disk — never raw session keys).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use mde_kdc_proto::crypto::{KeyHandle, KeyStore, PairingKeyPair, RingKeyStore};
use serde::{Deserialize, Serialize};

use crate::error::HostError;

/// One trusted peer, as persisted in `devices.toml`. The peer's public key and
/// certificate fingerprint are added by the pairing handshake (a later
/// increment); this increment persists the identity + audit fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceRecord {
    /// The peer's `Announce.device_id`.
    pub device_id: String,
    /// The peer's last-seen friendly name (for the surface's device list).
    pub device_name: String,
    /// Unix-millisecond timestamp of when the peer was first paired (audit).
    pub paired_at_ms: i64,
    /// The peer's pinned TLS identity: the SHA-256 fingerprint of its self-signed
    /// cert ([`crate::tls::compute_fingerprint`]), recorded at first pair. Every
    /// later TLS handshake to this peer must present a cert with this fingerprint
    /// (the `PinnedFingerprintVerifier`); a mismatch is a key-change/MITM signal.
    /// `#[serde(default)]` so a `devices.toml` written before this field loads as
    /// an empty pin (which the transport treats as not-yet-pinned).
    #[serde(default)]
    pub fingerprint: String,
}

/// The `devices.toml` document root: a list of `[[device]]` tables.
#[derive(Debug, Default, Serialize, Deserialize)]
struct DeviceFile {
    #[serde(default)]
    device: Vec<DeviceRecord>,
}

/// A pairing learned from ANOTHER node's published roster (KDC-MESH-3, design #5).
///
/// The mesh replicated the trust record (device id + the pinned cert fingerprint
/// from the origin node's TOFU pairing) so THIS node recognizes the phone
/// **without re-running the pairing handshake itself**.
///
/// Held separately from the own-row `devices` map because the two have different
/// authority: `devices` is this node's OWN pairings — persisted to `devices.toml`
/// and republished on the substrate (own-row authority) — whereas a `MeshPairing`
/// is another node's row, reflected here for recognition only and **never
/// republished** (that would forge the origin + loop). It is not persisted; the
/// substrate (the Syncthing-replicated `kdc-phones/*.json` rosters) is the source
/// of truth, reconverged into [`PairingStore::replace_synced`] each shunt tick, so
/// recognition tracks the live mesh (a pairing that leaves the mesh stops being
/// recognized). A `MeshPairing` MUST carry a real `fingerprint`: an empty pin is
/// discovery, not trust — the honest gate lives in `replace_synced`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeshPairing {
    /// The phone's `Announce.device_id` (the shared, mesh-wide identity).
    pub device_id: String,
    /// The phone's friendly name as the origin node recorded it.
    pub device_name: String,
    /// The pinned SHA-256 cert fingerprint from the ORIGIN node's TOFU pairing.
    /// A synced node has never seen the phone's cert, so it trusts this pin and
    /// enforces it against the live handshake — no fake trust, the phone must
    /// still present the cert the origin node pinned.
    pub fingerprint: String,
    /// Unix-ms when the origin node first paired the phone (audit/attribution).
    pub paired_at_ms: i64,
    /// The mesh host that OWNS this pairing (ran the TOFU) — so a synced node's
    /// recognition is attributable in the audit log.
    pub origin_host: String,
}

/// The host pairing store: this host's identity keypair, the persisted trusted
/// peers, and an in-memory store of live AES session keys.
pub struct PairingStore {
    dir: PathBuf,
    keypair: PairingKeyPair,
    /// This host's RSA public key as PKCS#1 `RSAPublicKey` DER — the form
    /// [`mde_kdc_proto::crypto::verify_signature`] expects.
    public_key_der: Vec<u8>,
    /// The trusted-peer records. Interior-mutable (E2.3) so a single
    /// `Arc<PairingStore>` is shared, without an outer `Mutex`, between the
    /// read-only LAN transport (identity + pin-verify) and the operator
    /// pairing surface (`pair`/`unpair` via `&self`) that mutates it — the
    /// canonical "one authoritative store" mackesd owns.
    devices: Mutex<HashMap<String, DeviceRecord>>,
    /// KDC-MESH-3 (design #5) — mesh-replicated pairings from OTHER nodes, keyed
    /// by device id. NOT persisted and NOT republished (own-row authority: only
    /// `devices` is authoritative here); rebuilt from the substrate each shunt
    /// tick via [`replace_synced`]. Checked by [`is_paired`] / [`get`] so every
    /// node that has synced the shared pairing recognizes the phone; a node that
    /// hasn't synced yet simply has no entry → honest gate (it never fakes trust).
    ///
    /// [`replace_synced`]: Self::replace_synced
    /// [`is_paired`]: Self::is_paired
    /// [`get`]: Self::get
    synced: Mutex<HashMap<String, MeshPairing>>,
    sessions: RingKeyStore,
}

impl PairingStore {
    /// The conventional store directory, `$XDG_CONFIG_HOME/mde/connect`
    /// (falling back to `$HOME/.config/mde/connect`).
    pub fn default_dir() -> Result<PathBuf, HostError> {
        // Per the XDG spec, an empty $XDG_CONFIG_HOME is treated as unset, so
        // filter the empty string out before it shadows the $HOME fallback (else
        // a set-but-empty var yields a relative `mde/connect` under the CWD).
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .ok_or(HostError::NoConfigDir)?;
        Ok(base.join("mde").join("connect"))
    }

    /// Open (or first-time create) the store under `dir`. Generates
    /// `identity.pkcs8` (RSA-4096, via [`crate::keygen::generate_pkcs8`]) if
    /// absent, else loads it through [`PairingKeyPair::from_pkcs8`]; reads
    /// `devices.toml`, tolerating a missing or garbage file by starting empty.
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, HostError> {
        use std::os::unix::fs::PermissionsExt;
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        // The store dir holds the long-lived identity key; keep it owner-only.
        // Harmless to re-apply on every open (the key file is created 0600
        // regardless, so this is defense-in-depth on the containing dir).
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;

        let key_path = dir.join("identity.pkcs8");
        let pkcs8 = if key_path.exists() {
            std::fs::read(&key_path)?
        } else {
            // §3 max-crypto: the long-lived identity key is RSA-4096, single-sourced
            // through `keygen` (the same generator `issue_identity_cert` binds the
            // cert to) so the key and cert are one identity at the pinned size.
            let der =
                crate::keygen::generate_pkcs8().map_err(|e| HostError::Keygen(e.to_string()))?;
            write_private(&key_path, &der)?;
            der
        };
        let keypair = PairingKeyPair::from_pkcs8(&pkcs8)?;
        let public_key_der = public_key_pkcs1_from_pkcs8(&pkcs8)?;
        let devices = read_devices(&dir);

        let store = Self {
            dir,
            keypair,
            public_key_der,
            devices: Mutex::new(devices),
            synced: Mutex::new(HashMap::new()),
            sessions: RingKeyStore::new(),
        };
        // SEC-8 (Q34) — restore the sealed session keys so live links
        // survive a daemon restart instead of forcing a re-pair.
        // Fails closed to empty (wrong master / tampered / absent).
        let master_path = store.dir.join("session-master.key");
        if let Ok(master) = crate::session_persist::load_or_create_master(&master_path) {
            let restored =
                crate::session_persist::load_sessions(&store.dir.join("sessions.enc"), &master);
            for (device_id, raw) in &restored {
                let handle = store.sessions.install_session_key(raw);
                tracing::debug!(device = %device_id, ?handle, "SEC-8: session restored");
            }
            if !restored.is_empty() {
                tracing::info!(
                    count = restored.len(),
                    "SEC-8: restored sealed KDC sessions across restart"
                );
            }
        }
        Ok(store)
    }

    /// SEC-8 — install a fresh session key AND persist the sealed map
    /// (the SEC-4 handshake's completion hook): the device's link
    /// survives the next restart.
    ///
    /// # Errors
    /// Master-key / seal IO failures (the in-memory install still
    /// happened — the link works until restart).
    pub fn install_and_persist_session(
        &self,
        device_id: &str,
        raw_key: &[u8],
    ) -> Result<KeyHandle, HostError> {
        let handle = self.sessions.install_session_key(raw_key);
        let master =
            crate::session_persist::load_or_create_master(&self.dir.join("session-master.key"))?;
        let path = self.dir.join("sessions.enc");
        let mut map = crate::session_persist::load_sessions(&path, &master);
        map.insert(device_id.to_string(), raw_key.to_vec());
        crate::session_persist::save_sessions(&path, &master, &map)?;
        Ok(handle)
    }

    /// This host's RSA public key (PKCS#1 `RSAPublicKey` DER), to advertise
    /// during pairing and to feed to `verify_signature`.
    #[must_use]
    pub fn public_key_der(&self) -> Vec<u8> {
        self.public_key_der.clone()
    }

    /// This host's identity private key as PKCS#8 DER. The LAN transport needs it
    /// to build its inbound TLS `ServerConfig` (it must present a cert + key) and
    /// to issue its self-signed identity cert. In-process only — same trust domain
    /// as the on-disk `identity.pkcs8` (0600) this returns a copy of; never sent on
    /// the wire or logged.
    #[must_use]
    pub fn identity_pkcs8(&self) -> &[u8] {
        self.keypair.pkcs8_bytes()
    }

    /// Sign a handshake challenge with this host's identity key
    /// (RSA-PKCS1-v1_5 / SHA-256).
    pub fn sign_challenge(&self, message: &[u8]) -> Result<Vec<u8>, HostError> {
        Ok(self.keypair.sign(message)?)
    }

    /// Lock the device map, recovering the guard if a prior holder panicked
    /// (the map is plain data — a panic leaves no broken invariant).
    fn devices(&self) -> MutexGuard<'_, HashMap<String, DeviceRecord>> {
        self.devices.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Lock the mesh-synced pairing map (design #5), panic-recovering like
    /// [`devices`](Self::devices) — plain data, no invariant to break.
    fn synced(&self) -> MutexGuard<'_, HashMap<String, MeshPairing>> {
        self.synced.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether `device_id` is a trusted peer — either LOCALLY paired (own-row,
    /// persisted in `devices.toml`) OR recognized via a mesh-synced pairing
    /// (design #5: another node paired it and the trust record replicated here).
    /// Drives `PluginContext.paired`, the outbound-verb gate, and the pair re-ack,
    /// so every node that has synced the shared pairing recognizes the phone
    /// without re-pairing. A node that hasn't synced simply returns `false` here
    /// (the honest gate — it never fakes trust).
    #[must_use]
    pub fn is_paired(&self, device_id: &str) -> bool {
        self.devices().contains_key(device_id) || self.synced().contains_key(device_id)
    }

    /// Look up a trusted peer's record (cloned out of the lock). An own-row
    /// pairing wins (own-row authority); otherwise a mesh-synced pairing (design
    /// #5) is surfaced as a [`DeviceRecord`] carrying the **origin node's pinned
    /// fingerprint**, so the transport enforces the same cert pin a synced node
    /// never saw at TOFU. `None` when the device is neither locally paired nor
    /// synced (honest gate).
    #[must_use]
    pub fn get(&self, device_id: &str) -> Option<DeviceRecord> {
        if let Some(rec) = self.devices().get(device_id).cloned() {
            return Some(rec);
        }
        self.synced().get(device_id).map(|p| DeviceRecord {
            device_id: p.device_id.clone(),
            device_name: p.device_name.clone(),
            paired_at_ms: p.paired_at_ms,
            fingerprint: p.fingerprint.clone(),
        })
    }

    /// Whether `device_id` is LOCALLY paired (own-row authority) — the set THIS
    /// node persists in `devices.toml` and republishes on the substrate.
    /// [`is_paired`](Self::is_paired) additionally recognizes mesh-synced
    /// pairings; this is the narrower "did WE pair it" question the mesh-shunt
    /// uses to decide what to republish (own rows only, never a synced one).
    #[must_use]
    pub fn is_locally_paired(&self, device_id: &str) -> bool {
        self.devices().contains_key(device_id)
    }

    /// Whether `device_id` is recognized via a mesh-synced pairing (design #5) —
    /// i.e. another node owns the pairing and it replicated here. Distinct from
    /// [`is_locally_paired`](Self::is_locally_paired); both together are
    /// [`is_paired`](Self::is_paired).
    #[must_use]
    pub fn is_synced(&self, device_id: &str) -> bool {
        self.synced().contains_key(device_id)
    }

    /// The mesh-synced pairing for a device, if any — for attribution/audit
    /// (which mesh host vouches for this recognized phone).
    #[must_use]
    pub fn synced_pairing(&self, device_id: &str) -> Option<MeshPairing> {
        self.synced().get(device_id).cloned()
    }

    /// KDC-MESH-3 (design #5) — replace the WHOLE mesh-synced pairing set with the
    /// merged view the shunt tick collected from every neighbor's published
    /// roster. Replace (not merge) so recognition CONVERGES with the live mesh: a
    /// pairing that left the substrate stops being recognized on the next tick.
    ///
    /// Honest gate: a pairing carrying an empty `fingerprint` is a name/discovery
    /// relay, NOT trust — it is dropped here, so an unpinned relay never makes a
    /// node fake trust. Own-row authority is the caller's contract: it passes only
    /// OTHER nodes' pairings (a neighbor's roster reader already skips our own
    /// file), and [`get`](Self::get) prefers a local pairing over a synced one, so
    /// a device we paired ourselves is always served from our own authoritative row.
    pub fn replace_synced(&self, pairings: Vec<MeshPairing>) {
        let mut map = self.synced();
        map.clear();
        for p in pairings {
            if p.fingerprint.is_empty() {
                continue; // no pin ⇒ discovery, not trust (honest gate)
            }
            map.insert(p.device_id.clone(), p);
        }
    }

    /// Every trusted peer, for enumeration — e.g. a host surfacing the paired-device
    /// roster (the daemon's published roster). Iteration order is unspecified.
    #[must_use]
    pub fn records(&self) -> Vec<DeviceRecord> {
        self.devices().values().cloned().collect()
    }

    /// Number of trusted peers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.devices().len()
    }

    /// Whether the store has no trusted peers.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.devices().is_empty()
    }

    /// Trust a peer and persist the store (atomic write of `devices.toml`).
    /// Interior-mutable (`&self`) so a shared `Arc<PairingStore>` can pair
    /// without an outer lock (E2.3).
    pub fn pair(&self, record: DeviceRecord) -> Result<(), HostError> {
        let snapshot = {
            let mut devices = self.devices();
            devices.insert(record.device_id.clone(), record);
            devices.values().cloned().collect::<Vec<_>>()
        };
        self.write_devices(&snapshot)
    }

    /// Untrust a peer and persist the store. No-op for an unknown id.
    pub fn unpair(&self, device_id: &str) -> Result<(), HostError> {
        let snapshot = {
            let mut devices = self.devices();
            devices.remove(device_id);
            devices.values().cloned().collect::<Vec<_>>()
        };
        self.write_devices(&snapshot)
    }

    /// Atomically write a peer snapshot to `devices.toml`. The snapshot is
    /// built under the device lock and passed here by reference so the disk
    /// write happens *after* the lock is released (no I/O under the lock).
    fn write_devices(&self, devices: &[DeviceRecord]) -> Result<(), HostError> {
        let file = DeviceFile {
            device: devices.to_vec(),
        };
        let text = toml::to_string_pretty(&file)?;
        let path = self.dir.join("devices.toml");
        let tmp = self.dir.join("devices.toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
}

/// The store fronts the protocol's session-key store so the wire layer can hold
/// it as `Box<dyn KeyStore>`. Only ephemeral session keys flow through here;
/// they live in memory and are zeroized on drop — never persisted.
impl KeyStore for PairingStore {
    fn session_key(&self, handle: KeyHandle) -> Option<Vec<u8>> {
        self.sessions.session_key(handle)
    }

    fn install_session_key(&self, raw_key: &[u8]) -> KeyHandle {
        self.sessions.install_session_key(raw_key)
    }

    fn forget(&self, handle: KeyHandle) {
        self.sessions.forget(handle);
    }
}

/// Derive the PKCS#1 `RSAPublicKey` DER (what `verify_signature` wants) from a
/// PKCS#8 private key.
fn public_key_pkcs1_from_pkcs8(pkcs8: &[u8]) -> Result<Vec<u8>, HostError> {
    use rsa::pkcs1::EncodeRsaPublicKey;
    use rsa::pkcs8::DecodePrivateKey;
    let key =
        rsa::RsaPrivateKey::from_pkcs8_der(pkcs8).map_err(|e| HostError::Keygen(e.to_string()))?;
    let der = key
        .to_public_key()
        .to_pkcs1_der()
        .map_err(|e| HostError::Keygen(e.to_string()))?;
    Ok(der.as_bytes().to_vec())
}

/// Create a private-key file at mode 0600, applied atomically *at creation* so
/// the key bytes are never momentarily group/world-readable (the mode-after-write
/// idiom leaves a 0644 window under the usual umask). `create_new` (O_CREAT |
/// O_EXCL) additionally refuses to follow a pre-planted symlink, so the key can't
/// be redirected outside the store dir — `open()` has already confirmed the path
/// does not exist, so the exclusivity is free.
fn write_private(path: &Path, der: &[u8]) -> Result<(), HostError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(der)?;
    Ok(())
}

/// Read `devices.toml` into a map; a missing or unparseable file yields an empty
/// store (never an error — the daemon must always start).
fn read_devices(dir: &Path) -> HashMap<String, DeviceRecord> {
    let Ok(text) = std::fs::read_to_string(dir.join("devices.toml")) else {
        return HashMap::new();
    };
    let file: DeviceFile = toml::from_str(&text).unwrap_or_default();
    file.device
        .into_iter()
        .map(|d| (d.device_id.clone(), d))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_kdc_proto::crypto::verify_signature;

    fn rec(id: &str) -> DeviceRecord {
        DeviceRecord {
            device_id: id.into(),
            device_name: "Phone".into(),
            paired_at_ms: 1,
            fingerprint: "AB:CD:EF".into(),
        }
    }

    #[test]
    fn open_creates_then_reloads_identity_key() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!tmp.path().join("identity.pkcs8").exists());
        let s1 = PairingStore::open(tmp.path()).unwrap();
        assert!(tmp.path().join("identity.pkcs8").exists());
        let pub1 = s1.public_key_der();
        assert!(!pub1.is_empty());
        // Reopen loads the SAME persisted key (no regeneration).
        let s2 = PairingStore::open(tmp.path()).unwrap();
        assert_eq!(s2.public_key_der(), pub1);
    }

    #[test]
    fn identity_key_and_dir_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("connect");
        PairingStore::open(&dir).unwrap();
        // The private identity key must be 0600 — and created that way, not
        // chmod'd after a 0644 write (no group/world-readable window).
        let key_mode = std::fs::metadata(dir.join("identity.pkcs8"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            key_mode, 0o600,
            "identity.pkcs8 must be 0600, got {key_mode:o}"
        );
        // The store dir holding it is owner-only too.
        let dir_mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "store dir must be 0700, got {dir_mode:o}");
    }

    #[test]
    fn sign_then_verify_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let s = PairingStore::open(tmp.path()).unwrap();
        let msg = b"handshake-challenge";
        let sig = s.sign_challenge(msg).unwrap();
        // End-to-end proof of the rsa-keygen -> ring-sign -> ring-verify interop.
        verify_signature(&s.public_key_der(), msg, &sig).unwrap();
    }

    #[test]
    fn pair_persists_across_reopen() {
        let tmp = tempfile::tempdir().unwrap();
        {
            let s = PairingStore::open(tmp.path()).unwrap();
            s.pair(rec("dev-1")).unwrap();
            assert!(s.is_paired("dev-1"));
            assert_eq!(s.len(), 1);
        }
        let s2 = PairingStore::open(tmp.path()).unwrap();
        assert!(s2.is_paired("dev-1"));
        assert_eq!(s2.get("dev-1").unwrap().device_name, "Phone");
        // The pinned fingerprint round-trips through devices.toml.
        assert_eq!(s2.get("dev-1").unwrap().fingerprint, "AB:CD:EF");
    }

    #[test]
    fn devices_toml_without_fingerprint_loads_with_empty_pin() {
        // Back-compat: a devices.toml written before the fingerprint field (e.g. a
        // store from increment 1) must still load, with an empty (not-yet-pinned)
        // fingerprint rather than a deserialize error.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("devices.toml"),
            "[[device]]\ndevice_id = \"old-1\"\ndevice_name = \"Legacy\"\npaired_at_ms = 42\n",
        )
        .unwrap();
        let s = PairingStore::open(tmp.path()).unwrap();
        let rec = s.get("old-1").expect("legacy record loads");
        assert_eq!(rec.device_name, "Legacy");
        assert_eq!(rec.fingerprint, "", "missing fingerprint defaults to empty");
    }

    #[test]
    fn unpair_persists_removal() {
        let tmp = tempfile::tempdir().unwrap();
        {
            let s = PairingStore::open(tmp.path()).unwrap();
            s.pair(rec("dev-1")).unwrap();
            s.unpair("dev-1").unwrap();
        }
        assert!(!PairingStore::open(tmp.path()).unwrap().is_paired("dev-1"));
    }

    #[test]
    fn records_enumerates_every_paired_peer() {
        let tmp = tempfile::tempdir().unwrap();
        let s = PairingStore::open(tmp.path()).unwrap();
        s.pair(rec("dev-1")).unwrap();
        s.pair(rec("dev-2")).unwrap();
        let recs = s.records();
        let mut ids: Vec<&str> = recs.iter().map(|d| d.device_id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, ["dev-1", "dev-2"]);
        assert!(PairingStore::open(tmp.path()).unwrap().records().len() == 2);
    }

    #[test]
    fn garbage_devices_file_loads_empty() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("devices.toml"), "not valid toml { [[[").unwrap();
        let s = PairingStore::open(tmp.path()).unwrap();
        assert!(s.is_empty());
    }

    // ── KDC-MESH-3 (#5): mesh-wide pairing replication ──────────────────────

    fn mesh_pairing(id: &str, fp: &str) -> MeshPairing {
        MeshPairing {
            device_id: id.into(),
            device_name: "Pixel".into(),
            fingerprint: fp.into(),
            paired_at_ms: 42,
            origin_host: "eagle".into(),
        }
    }

    #[test]
    fn mesh_synced_pairing_is_recognized_without_local_pairing() {
        let tmp = tempfile::tempdir().unwrap();
        let s = PairingStore::open(tmp.path()).unwrap();
        // Unsynced: the honest gate — the phone is NOT recognized, no fake trust.
        assert!(!s.is_paired("phone-x"));
        assert!(!s.is_synced("phone-x"));
        assert!(s.get("phone-x").is_none());

        // A fixture peer's shared pairing replicates in over the substrate.
        s.replace_synced(vec![mesh_pairing("phone-x", "AA:BB:CC")]);

        // Now recognized mesh-wide WITHOUT a local devices.toml entry (design #5).
        assert!(s.is_paired("phone-x"), "a synced pairing is recognized");
        assert!(s.is_synced("phone-x"));
        assert!(
            !s.is_locally_paired("phone-x"),
            "recognized via the mesh, not an own-row pairing"
        );
        // get() surfaces the ORIGIN node's pin so the transport enforces the same
        // cert the synced node never saw at TOFU (real trust, not a bypass).
        assert_eq!(s.get("phone-x").unwrap().fingerprint, "AA:BB:CC");
        assert_eq!(
            s.synced_pairing("phone-x").unwrap().origin_host,
            "eagle",
            "recognition is attributable to the vouching host"
        );
        // Own-row authority: a synced pairing is NEVER in the republish set.
        assert!(
            s.records().is_empty(),
            "synced pairings are not republished (own-row authority)"
        );

        // Convergence: an empty replace drops recognition (the pairing left the mesh).
        s.replace_synced(vec![]);
        assert!(!s.is_paired("phone-x"), "recognition converges away with the mesh");
    }

    #[test]
    fn unpinned_relay_is_not_trusted_honest_gate() {
        // A relayed row carrying no fingerprint is discovery, not trust — it must
        // never make a node recognize the phone (design #5 honest gate).
        let tmp = tempfile::tempdir().unwrap();
        let s = PairingStore::open(tmp.path()).unwrap();
        s.replace_synced(vec![mesh_pairing("phone-nopin", "")]);
        assert!(!s.is_paired("phone-nopin"), "no pin ⇒ not trusted");
        assert!(!s.is_synced("phone-nopin"));
        assert!(s.get("phone-nopin").is_none());
    }

    #[test]
    fn local_pairing_takes_precedence_over_synced() {
        // Own-row authority: when the same device is both locally paired and
        // synced, get() serves OUR authoritative record, not the mesh copy.
        let tmp = tempfile::tempdir().unwrap();
        let s = PairingStore::open(tmp.path()).unwrap();
        s.pair(rec("dev-1")).unwrap(); // fingerprint "AB:CD:EF"
        s.replace_synced(vec![mesh_pairing("dev-1", "99:99:99")]);
        assert!(s.is_locally_paired("dev-1"));
        assert!(s.is_synced("dev-1"));
        assert_eq!(
            s.get("dev-1").unwrap().fingerprint,
            "AB:CD:EF",
            "the own-row pin wins over the synced one"
        );
    }

    #[test]
    fn session_keys_delegate_to_ring_store() {
        let tmp = tempfile::tempdir().unwrap();
        let s = PairingStore::open(tmp.path()).unwrap();
        let h = s.install_session_key(&[7_u8; 32]);
        assert_eq!(s.session_key(h).as_deref(), Some(&[7_u8; 32][..]));
        s.forget(h);
        assert!(s.session_key(h).is_none());
    }
}
