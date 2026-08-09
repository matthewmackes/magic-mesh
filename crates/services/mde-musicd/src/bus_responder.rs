//! AIR-2 (v6.1) — Bus-native control surface for the music daemon.
//!
//! Per the Q96 Bus-canonical lock (EPIC-RETIRE-DBUS), the daemon's
//! MDE-internal control is **Bus**, not a new `dev.mackes.MDE.Music`
//! D-Bus interface. The GUI (and `mde-bus publish`) send requests on
//! `action/music/<verb>`; the responder applies them to the shared
//! [`Queue`] and writes the result to `reply/<request-ulid>`. (MPRIS
//! `org.mpris.MediaPlayer2` — FDO-standard — stays D-Bus for media-key /
//! lock-screen interop; that + the play flow are AIR-2.c, gated on the
//! AIR-5 audio engine.)
//!
//! The verb dispatch ([`dispatch_queue_action`]) is a pure function over
//! the [`Queue`], fully unit-testable; [`serve`] is the thin poll loop
//! (the standard mackesd Bus-responder shape) that drives it off the
//! Bus persistence store.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mackes_mesh_types::clock::{
    clock_audio_status_topic, ClockAudioRef, ClockAudioRequestV1, ClockMusicKind,
    CLOCK_AUDIO_ACTION_TOPIC,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::airsonic::Client;
use crate::clock_audio::{ClockAudioAuthority, ClockAudioEffects};
use crate::creds;
use crate::domain::{
    build_shelves, dedup_catalog, normalized_identity, ordered_variants, BookmarkItem, CatalogItem,
    ContentKind, ContentRef, DownloadRecord, LibraryCollection, MusicActionRequestV1,
    MusicActionResultV1, MusicStorageSnapshot, MusicWorkspaceSnapshotV1, PlaybackSnapshot,
    QueueEntry, SearchPage, ServerCapabilities, SourceVariant, MAX_BOOKMARKS, MAX_COLLECTION_ITEMS,
    MAX_LIBRARY_OFFSET, MAX_LIBRARY_PAGE_SIZE, MAX_PLAYLIST_FIELD_BYTES, MAX_SEARCH_ITEMS,
    MAX_SOURCE_RECORDS, MUSIC_CONTRACT_VERSION,
};
use crate::engine::{Engine, PlaybackTrack, SourceCodec};
use crate::seat_audio::{SeatAudioAuthority, SeatDuckGeneration};

/// Shared `ipc/action_auth` wire contract for the music responder.
///
/// `mde-musicd` is a user-service crate and intentionally does not depend on
/// the root `mackesd` binary crate. Keep this verifier byte-compatible with
/// `mackesd::ipc::action_auth`: schema v1, the canonical request digest, the
/// v2 armed-token format, a 30-second maximum lifetime, and a durable
/// host-local nonce claim. Missing credentials fail closed.
mod music_action_auth {
    use std::path::{Path, PathBuf};

    use ed25519_dalek::VerifyingKey;
    use mackes_mesh_types::music_auth::{self, MusicAuthContext};
    use serde_json::Value;

    pub(super) const SCHEMA_VERSION: u64 = 1;
    const MAX_TTL_MS: i64 = 30_000;
    const NONCE_MIN_LEN: usize = 8;
    const CREDENTIAL_NAME: &str = "cloud-arm-key";
    const MUSIC_AUTH_NONCE_DIRECTORY: &str = "music-auth-nonces";
    const PUBLIC_KEY_ENV: &str = "MDE_MUSIC_ACTION_PUBLIC_KEY";

    pub(super) fn production_auth_root() -> PathBuf {
        crate::state::data_dir().join(MUSIC_AUTH_NONCE_DIRECTORY)
    }

    #[derive(Debug, Clone)]
    struct ArmedToken {
        nonce: String,
        expires_at_ms: i64,
        verb: String,
        node: String,
        target: String,
        request_sha256: String,
        signature: String,
    }

    impl ArmedToken {
        fn parse(raw: &str) -> Option<Self> {
            let parts: Vec<&str> = raw.trim().split('|').collect();
            if parts.len() != 8 || parts[0] != "v2" {
                return None;
            }
            Some(Self {
                nonce: parts[1].to_string(),
                expires_at_ms: parts[2].parse().ok()?,
                verb: parts[3].to_string(),
                node: parts[4].to_string(),
                target: parts[5].to_string(),
                request_sha256: parts[6].to_string(),
                signature: parts[7].to_string(),
            })
        }

        fn signing_payload(&self) -> String {
            format!(
                "v2|{}|{}|{}|{}|{}|{}",
                self.nonce,
                self.expires_at_ms,
                self.verb,
                self.node,
                self.target,
                self.request_sha256
            )
        }
    }

    pub(super) struct Authorizer {
        key: Option<Vec<u8>>,
        public_key: Option<VerifyingKey>,
        auth_root: PathBuf,
        test_now_ms: Option<i64>,
    }

    impl std::fmt::Debug for Authorizer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Authorizer")
                .field("auth_root", &self.auth_root)
                .field("has_key", &self.key.is_some())
                .field("has_public_key", &self.public_key.is_some())
                .finish_non_exhaustive()
        }
    }

    impl Authorizer {
        pub(super) fn production() -> Self {
            let key = load_production_key().ok();
            let public_key = load_production_public_key().ok();
            if key.is_none() && public_key.is_none() {
                tracing::error!(
                    target: "mde_musicd::action_auth",
                    "music mutation authorization unavailable; mutations are disabled"
                );
            }
            Self {
                key,
                public_key,
                // mde-musicd is deliberately a user service. Its replay ledger
                // must therefore live below the same user-owned durable state
                // root as its catalog/queue, not below root-only mackesd state.
                // The asymmetric public key still decides authorization; this
                // directory only records already-consumed nonces.
                auth_root: production_auth_root(),
                test_now_ms: None,
            }
        }

        #[cfg(test)]
        pub(super) fn for_test(key: &[u8], auth_root: PathBuf, now_ms: i64) -> Self {
            Self {
                key: Some(key.to_vec()),
                public_key: None,
                auth_root,
                test_now_ms: Some(now_ms),
            }
        }

        #[cfg(test)]
        pub(super) fn for_test_music(
            public_key: VerifyingKey,
            auth_root: PathBuf,
            now_ms: i64,
        ) -> Self {
            Self {
                key: None,
                public_key: Some(public_key),
                auth_root,
                test_now_ms: Some(now_ms),
            }
        }

        fn now_ms(&self) -> i64 {
            self.test_now_ms
                .unwrap_or_else(|| i64::try_from(crate::state::now_ms()).unwrap_or(i64::MAX))
        }

        pub(super) fn authorize(
            &self,
            body: &str,
            verb: &str,
            node: &str,
            target: &str,
        ) -> Result<(), String> {
            if body.len() > 64 * 1024 {
                return Err("request body exceeds the 64 KiB cap".to_string());
            }
            let envelope: Value = serde_json::from_str(body)
                .map_err(|_| "request body is not a JSON object".to_string())?;
            let object = envelope
                .as_object()
                .ok_or_else(|| "request body is not a JSON object".to_string())?;
            if object.get("schema_version").and_then(Value::as_u64) != Some(SCHEMA_VERSION) {
                return Err(format!(
                    "privileged action requires schema_version {SCHEMA_VERSION}"
                ));
            }
            if object.contains_key("music_auth") {
                return self.authorize_music(body, verb, node, target);
            }
            let raw_token = object
                .get("armed_token")
                .and_then(Value::as_str)
                .filter(|token| !token.trim().is_empty())
                .ok_or_else(|| "no armed token supplied".to_string())?;
            let key = self
                .key
                .as_deref()
                .ok_or_else(|| "music arming credential is unavailable".to_string())?;
            let token = ArmedToken::parse(raw_token)
                .ok_or_else(|| "armed token is malformed".to_string())?;
            if token.nonce.len() < NONCE_MIN_LEN {
                return Err("armed token is malformed".to_string());
            }
            if token.verb != verb || token.node != node || token.target != target {
                return Err("armed token does not authorize this verb/node/target".to_string());
            }
            let request_sha256 = request_digest(body)?;
            if token.request_sha256 != request_sha256 {
                return Err("armed token does not authorize this request body".to_string());
            }
            let now_ms = self.now_ms();
            if now_ms >= token.expires_at_ms {
                return Err("armed token has expired".to_string());
            }
            if token.expires_at_ms > now_ms.saturating_add(MAX_TTL_MS) {
                return Err("armed token exceeds the 30-second lifetime".to_string());
            }
            let expected = hmac_sha256_hex(key, token.signing_payload().as_bytes());
            if !constant_time_eq(expected.as_bytes(), token.signature.as_bytes()) {
                return Err("armed token signature did not verify".to_string());
            }
            match claim_nonce(&self.auth_root, &token.nonce, token.expires_at_ms, now_ms)? {
                true => Ok(()),
                false => Err("armed token was already used".to_string()),
            }
        }

        fn authorize_music(
            &self,
            body: &str,
            verb: &str,
            node: &str,
            target: &str,
        ) -> Result<(), String> {
            let public_key = self
                .public_key
                .clone()
                .or_else(|| load_production_public_key().ok())
                .ok_or_else(|| "music action public key is unavailable".to_string())?;
            let token = music_auth::verify_request(
                body,
                MusicAuthContext { verb, node, target },
                &public_key,
            )?;
            if token.nonce.len() < NONCE_MIN_LEN {
                return Err("music_auth nonce is too short".to_string());
            }
            let now_ms = self.now_ms();
            if now_ms >= token.expires_at_ms {
                return Err("music_auth token has expired".to_string());
            }
            if token.expires_at_ms > now_ms.saturating_add(music_auth::MUSIC_AUTH_MAX_TTL_MS) {
                return Err("music_auth token exceeds the 30-second lifetime".to_string());
            }
            match claim_nonce(&self.auth_root, &token.nonce, token.expires_at_ms, now_ms)? {
                true => Ok(()),
                false => Err("music_auth token was already used".to_string()),
            }
        }
    }

    fn load_production_key() -> Result<Vec<u8>, String> {
        let directory = std::env::var_os("CREDENTIALS_DIRECTORY")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or_else(|| "systemd action credential is unavailable".to_string())?;
        let raw = std::fs::read(directory.join(CREDENTIAL_NAME))
            .map_err(|error| format!("read systemd action credential: {error}"))?;
        let text = std::str::from_utf8(&raw)
            .map_err(|_| "cloud arming credential is not UTF-8".to_string())?
            .trim();
        let key = decode_hex(text).ok_or_else(|| {
            "cloud arming credential must encode exactly 32 hexadecimal bytes".to_string()
        })?;
        if key.len() != 32 {
            return Err("cloud arming credential must encode exactly 32 bytes".to_string());
        }
        Ok(key)
    }

    fn load_production_public_key() -> Result<VerifyingKey, String> {
        let path = std::env::var_os(PUBLIC_KEY_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(music_auth::MUSIC_AUTH_PUBLIC_KEY_PATH));
        if !path.is_absolute() {
            return Err("music action public key path must be absolute".to_string());
        }
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| format!("inspect music action public key: {error}"))?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err("music action public key is not a regular file".to_string());
        }
        if metadata.len() > 4096 {
            return Err("music action public key exceeds the 4 KiB cap".to_string());
        }
        let raw = std::fs::read(&path)
            .map_err(|error| format!("read music action public key: {error}"))?;
        let text = std::str::from_utf8(&raw)
            .map_err(|_| "music action public key is not UTF-8".to_string())?
            .trim();
        let bytes = decode_hex(text)
            .filter(|bytes| bytes.len() == 32)
            .ok_or_else(|| {
                "music action public key must encode 32 hexadecimal bytes".to_string()
            })?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| "music action public key must encode 32 bytes".to_string())?;
        VerifyingKey::from_bytes(&bytes)
            .map_err(|error| format!("parse music action public key: {error}"))
    }

    fn request_digest(raw: &str) -> Result<String, String> {
        let mut value: Value =
            serde_json::from_str(raw).map_err(|_| "request body is not valid JSON".to_string())?;
        value
            .as_object_mut()
            .ok_or_else(|| "request body JSON root is not an object".to_string())?
            .remove("armed_token");
        let mut canonical = String::new();
        write_canonical_json(&value, &mut canonical)?;
        Ok(hex_encode(&sha256(canonical.as_bytes())))
    }

    fn write_canonical_json(value: &Value, output: &mut String) -> Result<(), String> {
        match value {
            Value::Null => output.push_str("null"),
            Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
            Value::Number(value) => output.push_str(&value.to_string()),
            Value::String(value) => output.push_str(
                &serde_json::to_string(value)
                    .map_err(|_| "request string cannot serialize".to_string())?,
            ),
            Value::Array(values) => {
                output.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        output.push(',');
                    }
                    write_canonical_json(value, output)?;
                }
                output.push(']');
            }
            Value::Object(values) => {
                let mut entries: Vec<_> = values.iter().collect();
                entries.sort_by(|(left, _), (right, _)| left.cmp(right));
                output.push('{');
                for (index, (key, value)) in entries.into_iter().enumerate() {
                    if index > 0 {
                        output.push(',');
                    }
                    output.push_str(
                        &serde_json::to_string(key)
                            .map_err(|_| "request key cannot serialize".to_string())?,
                    );
                    output.push(':');
                    write_canonical_json(value, output)?;
                }
                output.push('}');
            }
        }
        Ok(())
    }

    fn claim_nonce(
        root: &Path,
        nonce: &str,
        expires_at_ms: i64,
        now_ms: i64,
    ) -> Result<bool, String> {
        use std::io::Write as _;
        use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

        let dir = root.join("spent-nonces");
        std::fs::create_dir_all(&dir)
            .map_err(|error| format!("create armed-token replay store: {error}"))?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("secure armed-token replay store: {error}"))?;
        for entry in std::fs::read_dir(&dir)
            .map_err(|error| format!("read armed-token replay store: {error}"))?
        {
            let Ok(entry) = entry else { continue };
            if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
                continue;
            }
            let expired = std::fs::read_to_string(entry.path())
                .ok()
                .and_then(|value| value.trim().parse::<i64>().ok())
                .is_some_and(|expiry| expiry <= now_ms);
            if expired {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        let path = dir.join(hex_encode(&sha256(nonce.as_bytes())));
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
            Err(error) => return Err(format!("claim armed-token nonce: {error}")),
        };
        file.write_all(expires_at_ms.to_string().as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("persist armed-token nonce: {error}"))?;
        std::fs::File::open(&dir)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("sync armed-token replay store: {error}"))?;
        Ok(true)
    }

    fn hmac_sha256_hex(key: &[u8], payload: &[u8]) -> String {
        let mut padded = [0_u8; 64];
        if key.len() > 64 {
            padded[..32].copy_from_slice(&sha256(key));
        } else {
            padded[..key.len()].copy_from_slice(key);
        }
        let mut inner = Vec::with_capacity(64 + payload.len());
        let mut outer = Vec::with_capacity(64 + 32);
        for byte in &padded {
            inner.push(*byte ^ 0x36);
            outer.push(*byte ^ 0x5c);
        }
        inner.extend_from_slice(payload);
        outer.extend_from_slice(&sha256(&inner));
        hex_encode(&sha256(&outer))
    }

    fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
        if left.len() != right.len() {
            return false;
        }
        let mut difference = 0_u8;
        for (left, right) in left.iter().zip(right) {
            difference |= left ^ right;
        }
        difference == 0
    }

    fn decode_hex(text: &str) -> Option<Vec<u8>> {
        if text.len() % 2 != 0 {
            return None;
        }
        let bytes = text.as_bytes();
        let mut decoded = Vec::with_capacity(bytes.len() / 2);
        for pair in bytes.chunks_exact(2) {
            let high = hex_value(pair[0])?;
            let low = hex_value(pair[1])?;
            decoded.push((high << 4) | low);
        }
        Some(decoded)
    }

    fn hex_value(value: u8) -> Option<u8> {
        match value {
            b'0'..=b'9' => Some(value - b'0'),
            b'a'..=b'f' => Some(value - b'a' + 10),
            b'A'..=b'F' => Some(value - b'A' + 10),
            _ => None,
        }
    }

    pub(super) fn hex_encode(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut output = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            output.push(HEX[(byte >> 4) as usize] as char);
            output.push(HEX[(byte & 0x0f) as usize] as char);
        }
        output
    }

    pub(super) fn sha256(input: &[u8]) -> [u8; 32] {
        const K: [u32; 64] = [
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
            0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
            0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
            0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
            0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
            0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
            0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
            0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
            0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
            0xc67178f2,
        ];
        let mut message = input.to_vec();
        let bit_len = (message.len() as u64).saturating_mul(8);
        message.push(0x80);
        while message.len() % 64 != 56 {
            message.push(0);
        }
        message.extend_from_slice(&bit_len.to_be_bytes());
        let mut state = [
            0x6a09e667_u32,
            0xbb67ae85,
            0x3c6ef372,
            0xa54ff53a,
            0x510e527f,
            0x9b05688c,
            0x1f83d9ab,
            0x5be0cd19,
        ];
        for chunk in message.chunks_exact(64) {
            let mut words = [0_u32; 64];
            for (index, word) in words[..16].iter_mut().enumerate() {
                let offset = index * 4;
                *word = u32::from_be_bytes([
                    chunk[offset],
                    chunk[offset + 1],
                    chunk[offset + 2],
                    chunk[offset + 3],
                ]);
            }
            for index in 16..64 {
                let s0 = words[index - 15].rotate_right(7)
                    ^ words[index - 15].rotate_right(18)
                    ^ (words[index - 15] >> 3);
                let s1 = words[index - 2].rotate_right(17)
                    ^ words[index - 2].rotate_right(19)
                    ^ (words[index - 2] >> 10);
                words[index] = words[index - 16]
                    .wrapping_add(s0)
                    .wrapping_add(words[index - 7])
                    .wrapping_add(s1);
            }
            let mut working = state;
            for index in 0..64 {
                let s1 = working[4].rotate_right(6)
                    ^ working[4].rotate_right(11)
                    ^ working[4].rotate_right(25);
                let choose = (working[4] & working[5]) ^ ((!working[4]) & working[6]);
                let temp1 = working[7]
                    .wrapping_add(s1)
                    .wrapping_add(choose)
                    .wrapping_add(K[index])
                    .wrapping_add(words[index]);
                let s0 = working[0].rotate_right(2)
                    ^ working[0].rotate_right(13)
                    ^ working[0].rotate_right(22);
                let majority = (working[0] & working[1])
                    ^ (working[0] & working[2])
                    ^ (working[1] & working[2]);
                let temp2 = s0.wrapping_add(majority);
                working[7] = working[6];
                working[6] = working[5];
                working[5] = working[4];
                working[4] = working[3].wrapping_add(temp1);
                working[3] = working[2];
                working[2] = working[1];
                working[1] = working[0];
                working[0] = temp1.wrapping_add(temp2);
            }
            for (slot, value) in state.iter_mut().zip(working) {
                *slot = slot.wrapping_add(value);
            }
        }
        let mut output = [0_u8; 32];
        for (index, word) in state.iter().enumerate() {
            output[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        output
    }

    #[cfg(test)]
    pub(super) fn request_digest_for_test(raw: &str) -> String {
        request_digest(raw).expect("test body is canonical JSON")
    }

    #[cfg(test)]
    pub(super) fn sign_for_test(key: &[u8], payload: &str) -> String {
        hmac_sha256_hex(key, payload.as_bytes())
    }
}

/// MUSIC-RESPONSIVE-7 — short-TTL cache of the Internet-radio station list so
/// only the first `list-radio` open pays the upstream round-trip (the saved
/// station list rarely changes). 5-minute TTL; in-process (per daemon).
static RADIO_CACHE: std::sync::Mutex<Option<(Instant, serde_json::Value)>> =
    std::sync::Mutex::new(None);
const RADIO_CACHE_TTL: Duration = Duration::from_secs(300);
use crate::queue::{self, Queue};
use crate::state::{self, MusicState};

/// Poll cadence for the action topics.
pub const POLL_INTERVAL: Duration = Duration::from_millis(500);
/// Browse requests are user-facing but not transport-critical.  Keep their
/// Bus scans at a slower bounded cadence so an idle daemon does not prepare a
/// query for every browse topic twice per second on every seat.
pub const BROWSE_POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Credential files are operator-edited configuration, not a transport
/// signal.  Re-reading and validating them every control sweep amplifies
/// filesystem and JSON work across all seats, especially when no credentials
/// exist yet.  A short bounded refresh keeps reconnect discovery responsive.
pub const CREDS_REFRESH_INTERVAL: Duration = Duration::from_secs(2);
/// Maximum deterministic startup offset for the responder sweep.  Every seat
/// used to enter the fixed 500 ms loop at service start, making all music
/// daemons wake together and amplify the same Bus/provider work across a
/// workstation fleet.  A host-derived phase preserves the cadence and
/// response bound while spreading those wakes over a small, bounded window.
pub const MAX_INITIAL_POLL_PHASE: Duration = Duration::from_millis(251);
/// Maximum retained messages admitted for one Music topic per poll.
///
/// The cursor advances only through this page so a delayed daemon drains a
/// retained action backlog over bounded sweeps instead of materializing the
/// whole history in one process queue.
const MAX_MESSAGES_PER_POLL: usize = 64;
/// Complete retained Music workspace read-model topic.
pub const WORKSPACE_STATE_TOPIC: &str = "state/music/workspace";
/// Durable monotonic revision for the retained workspace snapshot. The UI
/// deliberately ignores snapshots at or below its last observed revision, so
/// resetting this value on daemon restart would make a healthy workspace look
/// permanently stale until the process happened to catch up.
const WORKSPACE_REVISION_FILE: &str = "music-workspace-revision";
/// Typed mutation topic for the complete Music workspace contract.
pub const WORKSPACE_ACTION_VERB: &str = "workspace";
const WORKSPACE_LEDGER_SCHEMA_VERSION: u16 = 1;
const MAX_WORKSPACE_LEDGER_RECORDS: usize = 1024;
const WORKSPACE_LEDGER_FILE: &str = "music-workspace-action-ledger.json";
const DOWNLOAD_STORE_SCHEMA_VERSION: u16 = 1;
const DOWNLOAD_STORE_FILE: &str = "music-downloads-v1.json";
const CATALOG_STORE_SCHEMA_VERSION: u16 = 1;
const CATALOG_STORE_FILE: &str = "music-catalog-v1.json";
const MAX_CATALOG_ITEMS: usize = MAX_COLLECTION_ITEMS;
/// Stable Clock-facing identity for catalog-owned aliases. Provider locators
/// remain below this boundary and are resolved only by mde-musicd.
const CLOCK_CATALOG_SOURCE_ID: &str = "mde-musicd:catalog";
/// Stable Clock-facing source identity for daemon-admitted local alarm files.
/// Clock sees only the map key; this source's filesystem locator is never
/// serialized into a Clock request or result.
const CLOCK_LOCAL_FILE_SOURCE_ID: &str = "mde-musicd:local-alarm";
const CLOCK_LOCAL_FILE_DIRECTORY: &str = "clock-alarm-audio";
const MAX_CLOCK_LOCAL_FILES: usize = 32;
const MAX_CLOCK_LOCAL_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// NPR's durable News Now podcast/feed identity.
const NPR_NEWS_NOW_PRESET_ID: &str = "500005";
/// News Now is hourly. A retained resolution older than two publication
/// windows is not honest enough to ring and therefore falls back immediately.
const NPR_NEWS_NOW_MAX_AGE_MS: u64 = 2 * 60 * 60 * 1_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClockCatalogPreset {
    content: ContentRef,
    admitted_at_utc_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClockLocalFileAdmission {
    /// Path relative to the daemon-owned Clock audio directory.
    relative_path: String,
    byte_len: u64,
    modified_at_utc_ms: u64,
    sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CatalogPage {
    offset: usize,
    size: usize,
    has_more: bool,
    items: Vec<CatalogItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CatalogStoreFile {
    schema_version: u16,
    /// Current plural source representation.  `source` is retained only so
    /// older catalog files can be read and migrated in memory.
    #[serde(default)]
    sources: Vec<ServerCapabilities>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<ServerCapabilities>,
    albums: Vec<CatalogItem>,
    artists: Vec<CatalogItem>,
    #[serde(default)]
    podcasts: Vec<CatalogItem>,
    #[serde(default)]
    radio: Vec<CatalogItem>,
    #[serde(default)]
    episodes: Vec<CatalogItem>,
    songs: Vec<CatalogItem>,
    starred: Vec<CatalogItem>,
    recent: Vec<CatalogItem>,
    frequent: Vec<CatalogItem>,
    #[serde(default)]
    bookmarks: Vec<BookmarkItem>,
    search: Option<SearchPage>,
    /// Last provider page for each browse collection. The full catalog cache
    /// remains separately bounded; this page is what makes catalogs larger
    /// than the retained cache navigable without publishing every row.
    #[serde(default)]
    pages: BTreeMap<String, CatalogPage>,
    /// Bounded catalog-owned aliases for Clock. Values are provider-qualified
    /// identities, never URLs, paths, commands, credentials, or queue entries.
    #[serde(default)]
    clock_presets: BTreeMap<String, ClockCatalogPreset>,
    /// Fail-closed local-file registry. Keys are stable Clock catalog IDs;
    /// values remain private to mde-musicd and are never projected to Clock.
    #[serde(default)]
    clock_local_files: BTreeMap<String, ClockLocalFileAdmission>,
}

impl Default for CatalogStoreFile {
    fn default() -> Self {
        Self {
            schema_version: CATALOG_STORE_SCHEMA_VERSION,
            sources: Vec::new(),
            source: None,
            albums: Vec::new(),
            artists: Vec::new(),
            podcasts: Vec::new(),
            radio: Vec::new(),
            episodes: Vec::new(),
            songs: Vec::new(),
            starred: Vec::new(),
            recent: Vec::new(),
            frequent: Vec::new(),
            bookmarks: Vec::new(),
            search: None,
            pages: BTreeMap::new(),
            clock_presets: BTreeMap::new(),
            clock_local_files: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceLedgerRecord {
    request_id: String,
    result: MusicActionResultV1,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceLedgerFile {
    schema_version: u16,
    records: Vec<WorkspaceLedgerRecord>,
}

#[derive(Debug, Clone, Default)]
struct WorkspaceActionLedger {
    records: Vec<WorkspaceLedgerRecord>,
}

impl WorkspaceActionLedger {
    fn contains(&self, request_id: &str) -> bool {
        self.records
            .iter()
            .any(|record| record.request_id == request_id)
    }

    fn reserve(&mut self, request_id: &str, revision: u64) -> bool {
        if self.contains(request_id) {
            return false;
        }
        if self.records.len() == MAX_WORKSPACE_LEDGER_RECORDS {
            self.records.remove(0);
        }
        self.records.push(WorkspaceLedgerRecord {
            request_id: request_id.to_string(),
            result: typed_result(request_id, false, revision, Some("in_flight")),
        });
        true
    }

    fn finish(&mut self, request_id: &str, result: MusicActionResultV1) {
        if let Some(record) = self
            .records
            .iter_mut()
            .find(|record| record.request_id == request_id)
        {
            record.result = result;
        }
    }

    fn release(&mut self, request_id: &str) {
        self.records
            .retain(|record| record.request_id != request_id);
    }
}

fn workspace_ledger_path(dir: &Path) -> PathBuf {
    dir.join(WORKSPACE_LEDGER_FILE)
}

fn workspace_revision_path(dir: &Path) -> PathBuf {
    dir.join(WORKSPACE_REVISION_FILE)
}

/// Load the last published workspace revision. An absent record is the clean
/// first-start state; malformed or unreadable state is an error because
/// publishing a reset revision would strand existing clients behind a false
/// stale-result guard.
fn load_workspace_revision(dir: &Path) -> std::io::Result<u64> {
    let path = workspace_revision_path(dir);
    let Ok(raw) = std::fs::read_to_string(&path) else {
        if !path.exists() {
            return Ok(0);
        }
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "music workspace revision could not be read",
        ));
    };
    raw.trim().parse::<u64>().map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "music workspace revision is not an unsigned integer",
        )
    })
}

/// Persist the next revision before its snapshot is exposed on the Bus. This
/// keeps a crash between the two writes safe: a later daemon restart advances
/// again instead of reusing a revision already visible to a client.
fn persist_workspace_revision(dir: &Path, revision: u64) -> std::io::Result<()> {
    if revision == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "workspace revision must be non-zero",
        ));
    }
    std::fs::create_dir_all(dir)?;
    persist_json_atomic(&workspace_revision_path(dir), &revision)
}

/// Persist one daemon-owned JSON record without exposing a partially-written
/// state file or sharing a fixed temporary pathname between writers/restarts.
/// The retained Music stores contain credentials-adjacent metadata and queue
/// identities, so newly-created files are owner-readable on Unix.
fn persist_json_atomic<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    let body = serde_json::to_vec_pretty(value)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let temporary = path.with_extension(format!("json.tmp-{}-{nonce}", std::process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let result = (|| {
        let mut file = options.open(&temporary)?;
        file.write_all(&body)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)?;
        if let Some(parent) = path.parent() {
            OpenOptions::new().read(true).open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn load_workspace_ledger(dir: &Path) -> std::io::Result<WorkspaceActionLedger> {
    let path = workspace_ledger_path(dir);
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Ok(WorkspaceActionLedger::default());
    };
    let file: WorkspaceLedgerFile = serde_json::from_str(&raw).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("music action ledger: {error}"),
        )
    })?;
    let mut request_ids = HashSet::with_capacity(file.records.len());
    let invalid = file.schema_version != WORKSPACE_LEDGER_SCHEMA_VERSION
        || file.records.len() > MAX_WORKSPACE_LEDGER_RECORDS
        || file.records.iter().any(|record| {
            record.request_id.is_empty()
                || record.request_id.len() > crate::domain::MAX_REQUEST_ID_BYTES
                || record.request_id.chars().any(char::is_control)
                || record.result.request_id != record.request_id
                || !request_ids.insert(record.request_id.clone())
        });
    if invalid {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "music action ledger has an unsupported schema or bound violation",
        ));
    }
    Ok(WorkspaceActionLedger {
        records: file.records,
    })
}

fn persist_workspace_ledger(dir: &Path, ledger: &WorkspaceActionLedger) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = workspace_ledger_path(dir);
    persist_json_atomic(
        &path,
        &WorkspaceLedgerFile {
            schema_version: WORKSPACE_LEDGER_SCHEMA_VERSION,
            records: ledger.records.clone(),
        },
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DownloadStoreFile {
    schema_version: u16,
    records: Vec<DownloadRecord>,
}

fn downloads_path(dir: &Path) -> PathBuf {
    dir.join(DOWNLOAD_STORE_FILE)
}

fn valid_download_record(record: &DownloadRecord) -> bool {
    !record.content.source_id.trim().is_empty()
        && !record.content.remote_id.trim().is_empty()
        && matches!(
            record.content.kind,
            ContentKind::Music
                | ContentKind::Episode
                | ContentKind::Chapter
                | ContentKind::Audiobook
        )
        && matches!(
            record.state.as_str(),
            "queued" | "downloading" | "ready" | "failed" | "cancelled"
        )
        && record.state.len() <= MAX_PLAYLIST_FIELD_BYTES
        && record.error_code.as_ref().is_none_or(|error| {
            error.len() <= MAX_PLAYLIST_FIELD_BYTES && !error.chars().any(char::is_control)
        })
}

fn load_downloads(dir: &Path) -> std::io::Result<Vec<DownloadRecord>> {
    let path = downloads_path(dir);
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Ok(Vec::new());
    };
    let file: DownloadStoreFile = serde_json::from_str(&raw).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("music download store: {error}"),
        )
    })?;
    if file.schema_version != DOWNLOAD_STORE_SCHEMA_VERSION
        || file.records.len() > crate::domain::MAX_QUEUE_ITEMS
        || file
            .records
            .iter()
            .any(|record| !valid_download_record(record))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "music download store has an unsupported schema or bound violation",
        ));
    }
    Ok(file.records)
}

fn persist_downloads(dir: &Path, records: &[DownloadRecord]) -> std::io::Result<()> {
    if records.len() > crate::domain::MAX_QUEUE_ITEMS
        || records.iter().any(|r| !valid_download_record(r))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "music download store exceeds its contract bounds",
        ));
    }
    std::fs::create_dir_all(dir)?;
    let path = downloads_path(dir);
    persist_json_atomic(
        &path,
        &DownloadStoreFile {
            schema_version: DOWNLOAD_STORE_SCHEMA_VERSION,
            records: records.to_vec(),
        },
    )
}

/// Convert in-flight downloads left by a crashed daemon into an honest,
/// retryable failure before the next workspace snapshot is published. The
/// cache writer only installs a complete file after the provider response has
/// finished, so a durable `downloading` record cannot safely claim that its
/// bytes are resumable.
fn recover_interrupted_downloads(dir: &Path) -> std::io::Result<bool> {
    let mut records = load_downloads(dir)?;
    let mut changed = false;
    for record in &mut records {
        if record.state == "downloading" {
            record.state = "failed".to_string();
            record.bytes = 0;
            record.total_bytes = None;
            record.pinned = false;
            record.error_code = Some("download_interrupted".to_string());
            changed = true;
        }
    }
    if changed {
        persist_downloads(dir, &records)?;
    }
    Ok(changed)
}

fn catalog_path(dir: &Path) -> PathBuf {
    dir.join(CATALOG_STORE_FILE)
}

fn valid_catalog_store_item(item: &CatalogItem) -> bool {
    !item.id.trim().is_empty()
        && item.variants.len() <= crate::domain::MAX_SOURCE_VARIANTS
        && item.variants.iter().all(|variant| {
            !variant.content.source_id.trim().is_empty()
                && !variant.content.remote_id.trim().is_empty()
        })
}

fn valid_catalog_page(page: &CatalogPage) -> bool {
    page.size > 0
        && page.size <= MAX_LIBRARY_PAGE_SIZE
        && page.offset <= MAX_LIBRARY_OFFSET
        && page.items.len() <= MAX_COLLECTION_ITEMS
        && page.items.len() <= page.size
        && page.items.iter().all(valid_catalog_store_item)
}

fn valid_catalog_store_bookmark(bookmark: &BookmarkItem) -> bool {
    !bookmark.content.source_id.trim().is_empty()
        && !bookmark.content.remote_id.trim().is_empty()
        && !bookmark.title.trim().is_empty()
        && bookmark
            .duration_ms
            .is_none_or(|duration| bookmark.position_ms <= duration)
}

fn valid_catalog_source(source: &ServerCapabilities) -> bool {
    !source.source_id.trim().is_empty()
        && source.source_id.len() <= MAX_PLAYLIST_FIELD_BYTES
        && source.api_profile.len() <= MAX_PLAYLIST_FIELD_BYTES
        && source.features.len() <= 64
        && source.features.iter().all(|feature| {
            !feature.is_empty()
                && feature.len() <= MAX_PLAYLIST_FIELD_BYTES
                && !feature.chars().any(char::is_control)
        })
}

fn valid_clock_catalog_presets(presets: &BTreeMap<String, ClockCatalogPreset>) -> bool {
    presets.len() <= 8
        && presets.iter().all(|(identity, preset)| {
            identity == NPR_NEWS_NOW_PRESET_ID
                && preset.content.kind == ContentKind::Episode
                && !preset.content.source_id.trim().is_empty()
                && preset.content.source_id.len() <= MAX_PLAYLIST_FIELD_BYTES
                && !preset.content.remote_id.trim().is_empty()
                && preset.content.remote_id.len() <= MAX_PLAYLIST_FIELD_BYTES
        })
}

fn valid_clock_local_id(identity: &str) -> bool {
    !identity.is_empty()
        && identity.len() <= mackes_mesh_types::clock::MAX_CLOCK_AUDIO_ID_BYTES
        && identity
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn supported_clock_local_suffix(path: &Path) -> bool {
    path.extension()
        .and_then(|suffix| suffix.to_str())
        .is_some_and(|suffix| {
            matches!(
                suffix.to_ascii_lowercase().as_str(),
                "flac" | "mp3" | "ogg" | "oga" | "aac" | "m4a" | "wav" | "wave" | "opus"
            )
        })
}

fn valid_clock_local_admissions(admissions: &BTreeMap<String, ClockLocalFileAdmission>) -> bool {
    admissions.len() <= MAX_CLOCK_LOCAL_FILES
        && admissions.iter().all(|(identity, admission)| {
            let relative = Path::new(&admission.relative_path);
            valid_clock_local_id(identity)
                && !admission.relative_path.is_empty()
                && admission.relative_path.len() <= MAX_PLAYLIST_FIELD_BYTES
                && !relative.is_absolute()
                && relative
                    .components()
                    .all(|component| matches!(component, Component::Normal(_)))
                && supported_clock_local_suffix(relative)
                && admission.byte_len > 0
                && admission.byte_len <= MAX_CLOCK_LOCAL_FILE_BYTES
                && admission.modified_at_utc_ms > 0
                && admission.sha256.len() == 64
                && admission
                    .sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())
        })
}

fn clock_file_sha256(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    if bytes.is_empty()
        || u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_CLOCK_LOCAL_FILE_BYTES
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Clock local audio size changed outside the admitted bound",
        ));
    }
    Ok(music_action_auth::hex_encode(&music_action_auth::sha256(
        &bytes,
    )))
}

fn modified_at_utc_ms(metadata: &std::fs::Metadata) -> std::io::Result<u64> {
    let modified = metadata.modified()?;
    let duration = modified.duration_since(UNIX_EPOCH).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Clock local audio predates the Unix epoch",
        )
    })?;
    u64::try_from(duration.as_millis()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Clock local audio timestamp exceeds the contract bound",
        )
    })
}

/// Admit one regular audio file already placed beneath mde-musicd's private
/// Clock directory. Arbitrary paths, symlinks, empty/oversized files, unknown
/// codecs, and unstable identities fail closed.
pub fn admit_clock_local_file(
    data_dir: &Path,
    identity: &str,
    candidate: &Path,
) -> std::io::Result<ClockAudioRef> {
    if !valid_clock_local_id(identity) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid Clock local audio identity",
        ));
    }
    let root = data_dir.join(CLOCK_LOCAL_FILE_DIRECTORY);
    let canonical_root = std::fs::canonicalize(&root)?;
    let link_metadata = std::fs::symlink_metadata(candidate)?;
    if link_metadata.file_type().is_symlink() || !link_metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Clock local audio must be a regular non-symlink file",
        ));
    }
    let canonical_candidate = std::fs::canonicalize(candidate)?;
    let relative = canonical_candidate
        .strip_prefix(&canonical_root)
        .map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Clock local audio is outside the daemon-owned directory",
            )
        })?;
    if !supported_clock_local_suffix(relative) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "unsupported Clock local audio codec",
        ));
    }
    let metadata = std::fs::metadata(&canonical_candidate)?;
    if metadata.len() == 0 || metadata.len() > MAX_CLOCK_LOCAL_FILE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Clock local audio size is outside the admitted bound",
        ));
    }
    let relative_path = relative.to_string_lossy().into_owned();
    let admission = ClockLocalFileAdmission {
        relative_path,
        byte_len: metadata.len(),
        modified_at_utc_ms: modified_at_utc_ms(&metadata)?,
        sha256: clock_file_sha256(&canonical_candidate)?,
    };
    let mut catalog = load_catalog(data_dir)?;
    if catalog.clock_local_files.len() == MAX_CLOCK_LOCAL_FILES
        && !catalog.clock_local_files.contains_key(identity)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Clock local audio registry is full",
        ));
    }
    catalog
        .clock_local_files
        .insert(identity.to_owned(), admission);
    persist_catalog(data_dir, &catalog)?;
    Ok(ClockAudioRef::Music {
        source_id: CLOCK_LOCAL_FILE_SOURCE_ID.to_owned(),
        remote_id: identity.to_owned(),
        content_kind: ClockMusicKind::Track,
        fallback_tone_id: "alarm_classic".to_owned(),
    })
}

fn set_catalog_variants_reachable(file: &mut CatalogStoreFile, source_id: &str, reachable: bool) {
    let update_item = |item: &mut CatalogItem| {
        for variant in &mut item.variants {
            if variant.content.source_id == source_id {
                variant.reachable = reachable;
            }
        }
    };
    for items in [
        &mut file.albums,
        &mut file.artists,
        &mut file.podcasts,
        &mut file.radio,
        &mut file.episodes,
        &mut file.songs,
        &mut file.starred,
        &mut file.recent,
        &mut file.frequent,
    ] {
        for item in items {
            update_item(item);
        }
    }
    for page in file.pages.values_mut() {
        for item in &mut page.items {
            update_item(item);
        }
    }
    if let Some(page) = &mut file.search {
        for items in page.groups.values_mut() {
            for item in items {
                update_item(item);
            }
        }
    }
}

fn load_catalog(dir: &Path) -> std::io::Result<CatalogStoreFile> {
    let path = catalog_path(dir);
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Ok(CatalogStoreFile::default());
    };
    let mut file: CatalogStoreFile = serde_json::from_str(&raw).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("music catalog store: {error}"),
        )
    })?;
    if file.sources.is_empty() {
        if let Some(source) = file.source.take() {
            file.sources.push(source);
        }
    }
    if file.sources.len() > creds::MAX_CONFIGURED_SOURCES
        || file
            .sources
            .iter()
            .any(|source| !valid_catalog_source(source))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "music catalog store has an unsupported source bound or identity",
        ));
    }
    let collections = [
        &file.albums,
        &file.artists,
        &file.podcasts,
        &file.radio,
        &file.episodes,
        &file.songs,
        &file.starred,
        &file.recent,
        &file.frequent,
    ];
    if file.schema_version != CATALOG_STORE_SCHEMA_VERSION
        || collections.iter().any(|items| {
            items.len() > MAX_CATALOG_ITEMS || items.iter().any(|i| !valid_catalog_store_item(i))
        })
        || file.bookmarks.len() > MAX_BOOKMARKS
        || file
            .bookmarks
            .iter()
            .any(|bookmark| !valid_catalog_store_bookmark(bookmark))
        || file.search.as_ref().is_some_and(|page| {
            page.groups.values().map(Vec::len).sum::<usize>() > MAX_SEARCH_ITEMS
                || page
                    .groups
                    .values()
                    .flat_map(|items| items.iter())
                    .any(|item| !valid_catalog_store_item(item))
        })
        || file.pages.len() > MAX_SOURCE_RECORDS
        || file
            .pages
            .iter()
            .any(|(key, page)| key.trim().is_empty() || !valid_catalog_page(page))
        || !valid_clock_catalog_presets(&file.clock_presets)
        || !valid_clock_local_admissions(&file.clock_local_files)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "music catalog store has an unsupported schema or bound violation",
        ));
    }
    Ok(file)
}

fn persist_catalog(dir: &Path, file: &CatalogStoreFile) -> std::io::Result<()> {
    let collections = [
        &file.albums,
        &file.artists,
        &file.podcasts,
        &file.radio,
        &file.episodes,
        &file.songs,
        &file.starred,
        &file.recent,
        &file.frequent,
    ];
    if file.schema_version != CATALOG_STORE_SCHEMA_VERSION
        || file.sources.len() > creds::MAX_CONFIGURED_SOURCES
        || file
            .sources
            .iter()
            .any(|source| !valid_catalog_source(source))
        || collections.iter().any(|items| {
            items.len() > MAX_CATALOG_ITEMS || items.iter().any(|i| !valid_catalog_store_item(i))
        })
        || file.bookmarks.len() > MAX_BOOKMARKS
        || file
            .bookmarks
            .iter()
            .any(|bookmark| !valid_catalog_store_bookmark(bookmark))
        || file.pages.len() > MAX_SOURCE_RECORDS
        || file
            .pages
            .iter()
            .any(|(key, page)| key.trim().is_empty() || !valid_catalog_page(page))
        || !valid_clock_catalog_presets(&file.clock_presets)
        || !valid_clock_local_admissions(&file.clock_local_files)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "music catalog store exceeds its contract bounds",
        ));
    }
    std::fs::create_dir_all(dir)?;
    let path = catalog_path(dir);
    persist_json_atomic(&path, file)
}

fn catalog_source<'a>(
    file: &'a mut CatalogStoreFile,
    client: &Client,
) -> Option<&'a mut ServerCapabilities> {
    let source_id = catalog_source_id(client);
    if let Some(index) = file
        .sources
        .iter()
        .position(|source| source.source_id == source_id)
    {
        return Some(&mut file.sources[index]);
    }
    if file.sources.len() == creds::MAX_CONFIGURED_SOURCES {
        // The configured source list is bounded.  Keep the existing source
        // rows stable; the caller still records its content variants using
        // the source id, but does not let metadata grow without bound.
        return None;
    }
    file.sources.push(ServerCapabilities {
        source_id,
        api_profile: format!("subsonic/{}", client.api_version()),
        reachable: true,
        authentication_required: false,
        features: BTreeSet::new(),
    });
    file.sources.last_mut()
}

fn catalog_source_id(client: &Client) -> String {
    let base = client.base_url().trim().trim_end_matches('/');
    let suffix: String = base
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_PLAYLIST_FIELD_BYTES.saturating_sub(9))
        .collect();
    format!("airsonic:{suffix}")
}

fn catalog_variant(source_id: &str, remote_id: &str, kind: ContentKind) -> Option<SourceVariant> {
    Some(SourceVariant {
        content: ContentRef::new(source_id, remote_id, kind)?,
        cached: false,
        reachable: true,
        operator_priority: 0,
        latency_ms: None,
    })
}

fn album_catalog_item(
    source_id: &str,
    album: &crate::airsonic::Album,
    starred: bool,
) -> Option<CatalogItem> {
    Some(CatalogItem {
        id: normalized_identity(ContentKind::Album, &album.name, &album.artist, "", None),
        kind: ContentKind::Album,
        title: album.name.clone(),
        creator: album.artist.clone(),
        parent_title: String::new(),
        duration_ms: None,
        artwork_ref: (!album.cover_art.is_empty()).then(|| album.cover_art.clone()),
        starred,
        cached: false,
        variants: vec![catalog_variant(source_id, &album.id, ContentKind::Album)?],
    })
}

fn artist_catalog_item(source_id: &str, artist: &crate::airsonic::Artist) -> Option<CatalogItem> {
    Some(CatalogItem {
        id: normalized_identity(ContentKind::Artist, &artist.name, "", "", None),
        kind: ContentKind::Artist,
        title: artist.name.clone(),
        creator: artist.name.clone(),
        parent_title: String::new(),
        duration_ms: None,
        artwork_ref: None,
        starred: false,
        cached: false,
        variants: vec![catalog_variant(source_id, &artist.id, ContentKind::Artist)?],
    })
}

fn podcast_catalog_item(
    source_id: &str,
    channel: &crate::airsonic::PodcastChannel,
) -> Option<CatalogItem> {
    Some(CatalogItem {
        id: normalized_identity(ContentKind::Podcast, &channel.title, "", "", None),
        kind: ContentKind::Podcast,
        title: channel.title.clone(),
        creator: String::new(),
        parent_title: String::new(),
        duration_ms: None,
        artwork_ref: (!channel.artwork_ref.is_empty()).then(|| channel.artwork_ref.clone()),
        starred: false,
        cached: false,
        variants: vec![catalog_variant(
            source_id,
            &channel.id,
            ContentKind::Podcast,
        )?],
    })
}

fn radio_catalog_item(
    source_id: &str,
    station: &crate::airsonic::RadioStation,
) -> Option<CatalogItem> {
    Some(CatalogItem {
        id: normalized_identity(ContentKind::Radio, &station.name, "", "", None),
        kind: ContentKind::Radio,
        title: station.name.clone(),
        creator: "Internet radio".to_string(),
        parent_title: String::new(),
        duration_ms: None,
        artwork_ref: (!station.artwork_ref.is_empty()).then(|| station.artwork_ref.clone()),
        starred: false,
        cached: false,
        // The engine's existing Airsonic stream seam treats an http(s) remote
        // id as a direct stream URL. Keep the provider station id in the
        // display identity, but retain the URL in the playable variant.
        variants: vec![catalog_variant(
            source_id,
            &station.stream_url,
            ContentKind::Radio,
        )?],
    })
}

fn podcast_episode_catalog_item(
    source_id: &str,
    episode: &crate::airsonic::PodcastEpisode,
    parent_title: &str,
) -> Option<CatalogItem> {
    Some(CatalogItem {
        id: normalized_identity(ContentKind::Episode, &episode.title, "", parent_title, None),
        kind: ContentKind::Episode,
        title: episode.title.clone(),
        creator: String::new(),
        parent_title: parent_title.to_string(),
        duration_ms: None,
        artwork_ref: (!episode.artwork_ref.is_empty()).then(|| episode.artwork_ref.clone()),
        starred: false,
        cached: false,
        variants: vec![catalog_variant(
            source_id,
            &episode.id,
            ContentKind::Episode,
        )?],
    })
}

fn song_catalog_item(source_id: &str, song: &crate::airsonic::Song) -> Option<CatalogItem> {
    Some(CatalogItem {
        id: normalized_identity(
            ContentKind::Music,
            &song.title,
            &song.artist,
            &song.album,
            (song.duration > 0).then(|| u64::from(song.duration) * 1_000),
        ),
        kind: ContentKind::Music,
        title: song.title.clone(),
        creator: song.artist.clone(),
        parent_title: song.album.clone(),
        duration_ms: (song.duration > 0).then(|| u64::from(song.duration) * 1_000),
        artwork_ref: (!song.cover_art.is_empty()).then(|| song.cover_art.clone()),
        starred: false,
        cached: false,
        variants: vec![catalog_variant(source_id, &song.id, ContentKind::Music)?],
    })
}

fn bookmark_content_kind(kind: &str) -> Option<ContentKind> {
    match kind {
        "music" | "song" | "track" => Some(ContentKind::Music),
        "episode" | "podcast" => Some(ContentKind::Episode),
        "chapter" => Some(ContentKind::Chapter),
        "audiobook" | "book" => Some(ContentKind::Audiobook),
        _ => None,
    }
}

fn bookmark_item(source_id: &str, bookmark: &crate::airsonic::Bookmark) -> Option<BookmarkItem> {
    let kind = bookmark_content_kind(&bookmark.kind)?;
    Some(BookmarkItem {
        content: ContentRef::new(source_id, &bookmark.id, kind)?,
        title: bookmark.title.clone(),
        creator: bookmark.creator.clone(),
        parent_title: bookmark.parent_title.clone(),
        position_ms: bookmark.position_ms,
        duration_ms: bookmark.duration_ms,
        artwork_ref: bookmark.artwork_ref.clone(),
    })
}

fn decode_catalog_rows<T: serde::de::DeserializeOwned>(result: &Value, key: &str) -> Vec<T> {
    result
        .get(key)
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| serde_json::from_value(row.clone()).ok())
                .take(MAX_CATALOG_ITEMS)
                .collect()
        })
        .unwrap_or_default()
}

fn merge_catalog_items(slot: &mut Vec<CatalogItem>, incoming: Vec<CatalogItem>) {
    let existing = std::mem::take(slot);
    *slot = dedup_catalog(existing.into_iter().chain(incoming))
        .into_iter()
        .take(MAX_CATALOG_ITEMS)
        .collect();
}

fn merge_bookmarks(slot: &mut Vec<BookmarkItem>, incoming: Vec<BookmarkItem>) {
    let mut merged = std::mem::take(slot);
    for bookmark in incoming {
        if let Some(existing) = merged
            .iter_mut()
            .find(|existing| existing.content == bookmark.content)
        {
            *existing = bookmark;
        } else {
            merged.push(bookmark);
        }
    }
    merged.truncate(MAX_BOOKMARKS);
    *slot = merged;
}

/// Replace a provider cover-art token with the daemon-local cache path after a
/// successful `get-cover-art` browse request. The provider token remains the
/// request key until this point; the retained path is what lets an embedded
/// surface render without credentials or a second network authority.
fn retain_cover_art_path(file: &mut CatalogStoreFile, cover_id: &str, path: &str) {
    if !path.starts_with('/')
        || path.len() > MAX_PLAYLIST_FIELD_BYTES
        || path.chars().any(char::is_control)
    {
        return;
    }
    let update = |items: &mut [CatalogItem]| {
        for item in items {
            if item.artwork_ref.as_deref() == Some(cover_id) {
                item.artwork_ref = Some(path.to_owned());
            }
        }
    };
    for items in [
        &mut file.albums,
        &mut file.artists,
        &mut file.podcasts,
        &mut file.radio,
        &mut file.episodes,
        &mut file.songs,
        &mut file.starred,
        &mut file.recent,
        &mut file.frequent,
    ] {
        update(items);
    }
    for page in file.pages.values_mut() {
        update(&mut page.items);
    }
    if let Some(search) = &mut file.search {
        for items in search.groups.values_mut() {
            update(items);
        }
    }
}

fn record_catalog_response(dir: &Path, client: &Client, verb: &str, body: &str, reply: &str) {
    let Ok(envelope) = serde_json::from_str::<Value>(reply) else {
        record_catalog_failure(dir, client);
        return;
    };
    if envelope.get("ok") != Some(&Value::Bool(true)) {
        let authentication_required = envelope
            .get("error")
            .and_then(|error| error.get("code"))
            .and_then(Value::as_i64)
            .is_some_and(|code| matches!(code, 40 | 41));
        record_catalog_failure_with_auth(dir, client, Some(authentication_required));
        return;
    }
    let Some(result) = envelope.get("result") else {
        return;
    };
    let source_id = catalog_source_id(client);
    let mut file = match load_catalog(dir) {
        Ok(file) => file,
        Err(error) => {
            tracing::warn!(%error, "music catalog store refused before update");
            return;
        }
    };
    let Some(source) = catalog_source(&mut file, client) else {
        tracing::warn!(source_id = %source_id, "music catalog source bound reached");
        return;
    };
    source.source_id = source_id.clone();
    source.api_profile = format!("subsonic/{}", client.api_version());
    source.reachable = true;
    source.authentication_required = false;
    source.features.insert(verb.to_string());
    set_catalog_variants_reachable(&mut file, &source_id, true);
    if matches!(verb, "albums-by-artist" | "get-artist") {
        // A detail response must be visible to the artist detail route even
        // when the last library page did not contain that artist's albums.
        // The bounded aggregate cache remains the detail fallback; the next
        // explicit library browse restores the paged list projection.
        file.pages.remove("albums");
    }

    match verb {
        "list-albums" | "albums-by-genre" | "albums-by-artist" | "get-artist" | "get-album" => {
            let starred = false;
            merge_catalog_items(
                &mut file.albums,
                decode_catalog_rows::<crate::airsonic::Album>(result, "albums")
                    .iter()
                    .filter_map(|album| album_catalog_item(&source_id, album, starred))
                    .collect(),
            );
            if let Some(album) = result.get("album").and_then(|value| {
                serde_json::from_value::<crate::airsonic::Album>(value.clone()).ok()
            }) {
                merge_catalog_items(
                    &mut file.albums,
                    album_catalog_item(&source_id, &album, false)
                        .into_iter()
                        .collect(),
                );
            }
            merge_catalog_items(
                &mut file.songs,
                decode_catalog_rows::<crate::airsonic::Song>(result, "songs")
                    .iter()
                    .filter_map(|song| song_catalog_item(&source_id, song))
                    .collect(),
            );
        }
        "list-starred" => {
            let items: Vec<CatalogItem> =
                decode_catalog_rows::<crate::airsonic::Album>(result, "albums")
                    .iter()
                    .filter_map(|album| album_catalog_item(&source_id, album, true))
                    .collect();
            merge_catalog_items(&mut file.starred, items.clone());
            merge_catalog_items(&mut file.albums, items);
        }
        "list-recents" => merge_catalog_items(
            &mut file.recent,
            decode_catalog_rows::<crate::airsonic::Album>(result, "albums")
                .iter()
                .filter_map(|album| album_catalog_item(&source_id, album, false))
                .collect(),
        ),
        "list-frequent" => merge_catalog_items(
            &mut file.frequent,
            decode_catalog_rows::<crate::airsonic::Album>(result, "albums")
                .iter()
                .filter_map(|album| album_catalog_item(&source_id, album, false))
                .collect(),
        ),
        "list-bookmarks" => merge_bookmarks(
            &mut file.bookmarks,
            decode_catalog_rows::<crate::airsonic::Bookmark>(result, "bookmarks")
                .iter()
                .filter_map(|bookmark| bookmark_item(&source_id, bookmark))
                .collect(),
        ),
        "list-artists" => merge_catalog_items(
            &mut file.artists,
            decode_catalog_rows::<crate::airsonic::Artist>(result, "artists")
                .iter()
                .filter_map(|artist| artist_catalog_item(&source_id, artist))
                .collect(),
        ),
        "list-podcasts" => merge_catalog_items(
            &mut file.podcasts,
            decode_catalog_rows::<crate::airsonic::PodcastChannel>(result, "podcasts")
                .iter()
                .filter_map(|channel| podcast_catalog_item(&source_id, channel))
                .collect(),
        ),
        "list-radio" => merge_catalog_items(
            &mut file.radio,
            decode_catalog_rows::<crate::airsonic::RadioStation>(result, "radio")
                .iter()
                .filter_map(|station| radio_catalog_item(&source_id, station))
                .collect(),
        ),
        "podcast-episodes" => {
            let channel_id = serde_json::from_str::<Value>(body)
                .ok()
                .and_then(|value| value.get("id").and_then(Value::as_str).map(str::to_owned))
                .unwrap_or_default();
            let parent_title = file
                .podcasts
                .iter()
                .find(|podcast| {
                    podcast.variants.iter().any(|variant| {
                        variant.content.source_id == source_id
                            && variant.content.remote_id == channel_id
                    })
                })
                .map(|podcast| podcast.title.as_str())
                .unwrap_or_default();
            // The provider's typed podcast response is newest-first. Capture
            // that ordering before the general catalog's deterministic merge
            // sorts display rows by normalized identity.
            let admitted =
                decode_catalog_rows::<crate::airsonic::PodcastEpisode>(result, "episodes")
                    .iter()
                    .filter_map(|episode| {
                        podcast_episode_catalog_item(&source_id, episode, parent_title)
                    })
                    .collect::<Vec<_>>();
            if channel_id == NPR_NEWS_NOW_PRESET_ID {
                let newest =
                    admitted
                        .first()
                        .and_then(|item| item.variants.first())
                        .map(|variant| ClockCatalogPreset {
                            content: variant.content.clone(),
                            admitted_at_utc_ms: state::now_ms(),
                        });
                match newest {
                    Some(preset) => {
                        file.clock_presets
                            .insert(NPR_NEWS_NOW_PRESET_ID.to_string(), preset);
                    }
                    None => {
                        file.clock_presets.remove(NPR_NEWS_NOW_PRESET_ID);
                    }
                }
            }
            merge_catalog_items(&mut file.episodes, admitted);
        }
        "get-song" => {
            if let Some(song) = result.get("song").and_then(|value| {
                serde_json::from_value::<crate::airsonic::Song>(value.clone()).ok()
            }) {
                merge_catalog_items(
                    &mut file.songs,
                    song_catalog_item(&source_id, &song).into_iter().collect(),
                );
            }
        }
        "search" => {
            let mut groups = std::collections::BTreeMap::new();
            let albums = decode_catalog_rows::<crate::airsonic::Album>(result, "albums")
                .iter()
                .filter_map(|album| album_catalog_item(&source_id, album, false))
                .collect::<Vec<_>>();
            let artists = decode_catalog_rows::<crate::airsonic::Artist>(result, "artists")
                .iter()
                .filter_map(|artist| artist_catalog_item(&source_id, artist))
                .collect::<Vec<_>>();
            let songs = decode_catalog_rows::<crate::airsonic::Song>(result, "songs")
                .iter()
                .filter_map(|song| song_catalog_item(&source_id, song))
                .collect::<Vec<_>>();
            if !artists.is_empty() {
                groups.insert(ContentKind::Artist, artists.clone());
                merge_catalog_items(&mut file.artists, artists);
            }
            if !albums.is_empty() {
                groups.insert(ContentKind::Album, albums.clone());
                merge_catalog_items(&mut file.albums, albums);
            }
            if !songs.is_empty() {
                groups.insert(ContentKind::Music, songs.clone());
                merge_catalog_items(&mut file.songs, songs);
            }
            let query = search_query_from(body);
            let generation = file
                .search
                .as_ref()
                .map_or(1, |page| page.generation.saturating_add(1));
            let mut page = SearchPage {
                generation,
                query,
                groups,
                has_more: false,
            };
            if let Some(previous) = file.search.take() {
                if previous.query == page.query {
                    for (kind, items) in previous.groups {
                        let slot = page.groups.entry(kind).or_default();
                        merge_catalog_items(slot, items);
                    }
                }
            }
            file.search = Some(page);
        }
        "get-cover-art" => {
            let cover_id = song_id_from(body).unwrap_or_default();
            if let Some(path) = result.get("path").and_then(Value::as_str) {
                retain_cover_art_path(&mut file, &cover_id, path);
            }
        }
        _ => {}
    }
    record_browse_page(&mut file, &source_id, verb, body, result);
    if let Err(error) = persist_catalog(dir, &file) {
        tracing::warn!(%error, "music catalog store update failed");
    }
}

fn record_catalog_failure(dir: &Path, client: &Client) {
    record_catalog_failure_with_auth(dir, client, None);
}

fn record_catalog_failure_with_auth(
    dir: &Path,
    client: &Client,
    authentication_required: Option<bool>,
) {
    let source_id = catalog_source_id(client);
    let mut file = match load_catalog(dir) {
        Ok(file) => file,
        Err(error) => {
            tracing::warn!(%error, "music catalog store refused before failure update");
            return;
        }
    };
    if let Some(source) = catalog_source(&mut file, client) {
        source.source_id = source_id.clone();
        source.api_profile = format!("subsonic/{}", client.api_version());
        source.reachable = false;
        if let Some(authentication_required) = authentication_required {
            source.authentication_required = authentication_required;
        }
        set_catalog_variants_reachable(&mut file, &source_id, false);
        if let Err(error) = persist_catalog(dir, &file) {
            tracing::warn!(%error, "music catalog source failure update failed");
        }
    }
}

fn download_identity(request: &MusicActionRequestV1) -> Result<&ContentRef, &'static str> {
    let content = request.content.as_ref().ok_or("missing_content")?;
    if !matches!(
        content.kind,
        ContentKind::Music | ContentKind::Episode | ContentKind::Chapter | ContentKind::Audiobook
    ) {
        return Err("unsupported_source");
    }
    Ok(content)
}

/// Resolve a managed download against the same retained source admission used
/// by typed playback/progress. A configured provider is not enough: a
/// non-legacy identity must still be present in the daemon catalog and match
/// the selected provider's stable source id.
fn admitted_download_client<'a>(
    request: &MusicActionRequestV1,
    clients: &'a [&Client],
    data_dir: &Path,
) -> Result<&'a Client, &'static str> {
    let content = download_identity(request)?;
    if content.source_id == "legacy" {
        return clients.first().copied().ok_or("source_unavailable");
    }
    let catalog = load_catalog(data_dir).unwrap_or_default();
    if !catalog_contains_variant(&catalog, content) {
        return Err("unsupported_source");
    }
    clients
        .iter()
        .copied()
        .find(|candidate| catalog_source_id(candidate) == content.source_id)
        .ok_or("source_unavailable")
}

fn replace_download_record(
    records: &mut Vec<DownloadRecord>,
    content: &ContentRef,
    state: &str,
    bytes: u64,
    total_bytes: Option<u64>,
    error_code: Option<&str>,
) {
    let pinned = records
        .iter()
        .find(|record| record.content == *content)
        .is_some_and(|record| record.pinned);
    records.retain(|record| record.content != *content);
    records.push(DownloadRecord {
        content: content.clone(),
        state: state.to_string(),
        bytes,
        total_bytes,
        pinned,
        error_code: error_code.map(str::to_string),
    });
}

fn cache_cap_bytes() -> u64 {
    std::env::var("MDE_MUSIC_CACHE_CAP_BYTES")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(crate::cache::DEFAULT_CAP_BYTES)
}

fn reconcile_evicted_downloads(records: &mut Vec<DownloadRecord>, evicted: &[String]) {
    for record in records.iter_mut().filter(|record| {
        record.state == "ready" && evicted.iter().any(|id| id == &record.content.remote_id)
    }) {
        record.state = "failed".to_string();
        record.bytes = 0;
        record.total_bytes = None;
        record.pinned = false;
        record.error_code = Some("cache_evicted".to_string());
    }
}

fn download_to_cache(
    request: &MusicActionRequestV1,
    client: &Client,
    rt: &tokio::runtime::Runtime,
    data_dir: &Path,
    cache_dir: &Path,
) -> Result<(), &'static str> {
    let content = download_identity(request)?;
    let mut records = load_downloads(data_dir).map_err(|_| "download_store_unavailable")?;
    // Publish an observable in-flight state before contacting the provider;
    // a restart or workspace snapshot must not make a slow download look idle.
    replace_download_record(&mut records, content, "downloading", 0, None, None);
    persist_downloads(data_dir, &records).map_err(|_| "download_store_unavailable")?;

    const PROGRESS_INTERVAL_BYTES: u64 = 256 * 1024;
    let mut last_progress = 0_u64;
    let bytes = match rt.block_on(client.get_stream_bytes_with_progress(
        &content.remote_id,
        |received, total| {
            // Persist progress at a bounded cadence so a large response does
            // not turn every transport chunk into a journal/fsync storm. The
            // final chunk is always recorded before the ready transition.
            let final_chunk = total.is_some_and(|size| received >= size);
            if !final_chunk && received.saturating_sub(last_progress) < PROGRESS_INTERVAL_BYTES {
                return;
            }
            last_progress = received;
            replace_download_record(&mut records, content, "downloading", received, total, None);
            if let Err(error) = persist_downloads(data_dir, &records) {
                tracing::debug!(%error, received, "music download progress persistence deferred");
            }
        },
    )) {
        Ok(bytes) => bytes,
        Err(_) => {
            replace_download_record(
                &mut records,
                content,
                "failed",
                0,
                None,
                Some("download_failed"),
            );
            let _ = persist_downloads(data_dir, &records);
            return Err("download_failed");
        }
    };
    if bytes.is_empty() {
        replace_download_record(
            &mut records,
            content,
            "failed",
            0,
            Some(0),
            Some("download_empty"),
        );
        let _ = persist_downloads(data_dir, &records);
        return Err("download_empty");
    }
    let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if crate::cache::write_cached_track(
        cache_dir,
        &content.remote_id,
        "audio",
        &bytes,
        crate::cache::now_ms(),
        records
            .iter()
            .find(|record| record.content == *content)
            .is_some_and(|record| record.pinned),
    )
    .is_err()
    {
        replace_download_record(
            &mut records,
            content,
            "failed",
            0,
            Some(size),
            Some("download_persist_failed"),
        );
        let _ = persist_downloads(data_dir, &records);
        return Err("download_persist_failed");
    }
    let evicted = crate::cache::run_gc(cache_dir, cache_cap_bytes())
        .map_err(|_| "download_cache_gc_failed")?;
    replace_download_record(&mut records, content, "ready", size, Some(size), None);
    reconcile_evicted_downloads(&mut records, &evicted);
    if evicted.iter().any(|id| id == &content.remote_id) {
        replace_download_record(
            &mut records,
            content,
            "failed",
            0,
            None,
            Some("cache_evicted"),
        );
        return persist_downloads(data_dir, &records).map_err(|_| "download_store_unavailable");
    }
    persist_downloads(data_dir, &records).map_err(|_| "download_store_unavailable")
}

fn cancel_download(request: &MusicActionRequestV1, data_dir: &Path) -> Result<(), &'static str> {
    let content = download_identity(request)?;
    let mut records = load_downloads(data_dir).map_err(|_| "download_store_unavailable")?;
    let Some(record) = records.iter_mut().find(|record| record.content == *content) else {
        return Err("download_not_found");
    };
    record.state = "cancelled".to_string();
    record.error_code = None;
    persist_downloads(data_dir, &records).map_err(|_| "download_store_unavailable")
}

fn remove_download(
    request: &MusicActionRequestV1,
    data_dir: &Path,
    cache_dir: &Path,
) -> Result<(), &'static str> {
    let content = download_identity(request)?;
    let mut records = load_downloads(data_dir).map_err(|_| "download_store_unavailable")?;
    let before = records.len();
    records.retain(|record| record.content != *content);
    if before == records.len() {
        return Err("download_not_found");
    }
    crate::cache::remove_cached_track(cache_dir, &content.remote_id)
        .map_err(|_| "download_remove_failed")?;
    persist_downloads(data_dir, &records).map_err(|_| "download_store_unavailable")
}

fn set_download_pinned(
    request: &MusicActionRequestV1,
    data_dir: &Path,
    cache_dir: &Path,
    pinned: bool,
) -> Result<(), &'static str> {
    let content = download_identity(request)?;
    let mut records = load_downloads(data_dir).map_err(|_| "download_store_unavailable")?;
    let record = records
        .iter_mut()
        .find(|record| record.content == *content)
        .ok_or("download_not_found")?;
    record.pinned = pinned;

    let mut index = crate::cache::read_index(cache_dir);
    if index.entries.contains_key(&content.remote_id) {
        index.set_starred(&content.remote_id, pinned);
        crate::cache::write_index(cache_dir, &index).map_err(|_| "download_pin_persist_failed")?;
    }
    persist_downloads(data_dir, &records).map_err(|_| "download_store_unavailable")
}

fn typed_result(
    request_id: &str,
    accepted: bool,
    revision: u64,
    error_code: Option<&str>,
) -> MusicActionResultV1 {
    MusicActionResultV1 {
        schema_version: MUSIC_CONTRACT_VERSION,
        request_id: request_id.to_string(),
        accepted,
        revision,
        error_code: error_code.map(str::to_string),
    }
}

/// The queue-control verbs served on `action/music/<verb>` (synchronous
/// — they only touch the local queue file).
pub const ACTION_VERBS: [&str; 10] = [
    "enqueue",
    "enqueue-after",
    "clear",
    "next",
    "prev",
    "get-queue",
    // MUSIC-RFX-1 — queue management.
    "queue-move",
    "queue-remove",
    "queue-remove-many",
    "queue-move-to-next",
];

/// The library-browse verbs served on `action/music/<verb>`
/// (asynchronous — each proxies an Airsonic REST call).
pub const BROWSE_VERBS: [&str; 25] = [
    "list-albums",
    "list-artists",
    "search",
    "get-album",
    "list-genres",
    "albums-by-genre",
    "albums-by-artist",
    "get-artist",
    "get-song",
    "get-cover-art",
    "list-podcasts",
    "list-radio",
    "podcast-episodes",
    "list-bookmarks",
    "list-recents",
    "list-playlists",
    "get-playlist",
    "get-lyrics",
    // MUSIC-RFX-3 — playlist write verbs (proxy the Subsonic create/update/delete
    // endpoints; a re-query of list-playlists reflects the change).
    "playlist-create",
    "playlist-update",
    "playlist-delete",
    // MUSIC-RFX-6b — reorder a playlist in place (preserves its id).
    "playlist-reorder",
    // MUSIC-HOME-1 — the Music Home page's server-stats snapshot.
    "library-stats",
    // MUSIC-HOME-3 — Home discovery strips: most-played + starred albums.
    "list-frequent",
    "list-starred",
];

/// The transport verbs served on `action/music/<verb>` (AIR-2.d — drive
/// the AIR-5 playback engine).
pub const TRANSPORT_VERBS: [&str; 7] = [
    "play",
    "pause",
    "resume",
    "stop",
    "set-volume",
    "get-state",
    // MUSIC-RFX-2 — scrub within the current (finite) track.
    "seek",
];

/// AIR-15.b.5 — peer-roster + take-over verbs. They read/write the AIR-8
/// state files (`music-state-by-peer/`, handoff intents) and need neither
/// the engine nor the airsonic client.
pub const PEER_VERBS: [&str; 2] = ["peer-states", "take-over"];
/// Queue-independent Clock alert handoff verb.
pub const CLOCK_AUDIO_VERB: &str = "clock-audio";

/// Last-seen control cursors at the instant a renderer fails. A replacement
/// renderer may continue playback only if no queue or transport request was
/// observed in the meantime. This deliberately includes refused controls: an
/// operator pressing Stop while audio is unavailable must never be surprised
/// by an automatic restart.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ControlMarker(Vec<(String, String)>);

fn control_marker(cursors: &HashMap<String, String>) -> ControlMarker {
    let mut topics = ACTION_VERBS
        .iter()
        .copied()
        .filter(|verb| *verb != "get-queue")
        .chain(
            TRANSPORT_VERBS
                .iter()
                .copied()
                .filter(|verb| *verb != "get-state"),
        )
        .map(|verb| format!("action/music/{verb}"))
        .collect::<Vec<_>>();
    topics.push(format!("action/music/{WORKSPACE_ACTION_VERB}"));
    topics.sort_unstable();
    ControlMarker(
        topics
            .into_iter()
            .map(|topic| {
                let cursor = cursors.get(&topic).cloned().unwrap_or_default();
                (topic, cursor)
            })
            .collect(),
    )
}

#[derive(Debug, Clone)]
struct InterruptedPlayback {
    generation: u64,
    queue: Queue,
    position_ms: u64,
    controls: ControlMarker,
}

#[derive(Debug, Default)]
struct RendererRecovery {
    generation: u64,
    pending: Option<InterruptedPlayback>,
}

impl RendererRecovery {
    fn advance_generation(&mut self) -> Option<u64> {
        self.generation = self.generation.checked_add(1)?;
        Some(self.generation)
    }

    fn capture(&mut self, queue: Queue, position_ms: Option<u64>, controls: ControlMarker) {
        self.pending = None;
        let Some(generation) = self.advance_generation() else {
            return;
        };
        if position_ms.is_some() && queue.current().is_some() {
            self.pending = Some(InterruptedPlayback {
                generation,
                queue,
                position_ms: position_ms.unwrap_or_default(),
                controls,
            });
        }
    }

    fn invalidate_if_controls_changed(&mut self, controls: &ControlMarker) {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| &pending.controls != controls)
        {
            self.pending = None;
            let _ = self.advance_generation();
        }
    }

    fn resumable(&self, queue: &Queue, engine_idle: bool) -> Option<&InterruptedPlayback> {
        self.pending.as_ref().filter(|pending| {
            engine_idle && pending.generation == self.generation && pending.queue == *queue
        })
    }

    fn complete(&mut self, generation: u64) {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.generation == generation)
        {
            self.pending = None;
            let _ = self.advance_generation();
        }
    }
}

/// Authoritative-state write cadence while playing (AIR-8's 5 s heartbeat,
/// so a stale owner frees the mesh after `STATE_STALE_MS`).
pub const STATE_WRITE_INTERVAL: Duration = Duration::from_secs(5);
/// Maximum host-derived phase for the first active-playback heartbeat.  The
/// heartbeat remains five seconds apart after that first write, but seats that
/// start playback together do not all hit the Bus and state file at once.
pub const MAX_STATE_WRITE_PHASE: Duration = Duration::from_secs(2);

/// Resolve one queued provider id through the retained source variants. A
/// source-aware catalog identity is preferred using the same cache/reachability
/// ordering as playback; legacy queues remain visible when the catalog has not
/// projected a matching row yet.
fn workspace_content_ref(catalog: &CatalogStoreFile, remote_id: &str) -> ContentRef {
    let fallback = || {
        ContentRef::new("legacy", remote_id, ContentKind::Music)
            .expect("queue ids are validated before entering the workspace projection")
    };
    let Some(item) = catalog
        .songs
        .iter()
        .chain(catalog.episodes.iter())
        .chain(catalog.radio.iter())
        .find(|item| {
            matches!(
                item.kind,
                ContentKind::Music
                    | ContentKind::Episode
                    | ContentKind::Chapter
                    | ContentKind::Audiobook
                    | ContentKind::Radio
            ) && item
                .variants
                .iter()
                .any(|variant| variant.content.remote_id == remote_id)
        })
    else {
        if let Some(bookmark) = catalog
            .bookmarks
            .iter()
            .find(|bookmark| bookmark.content.remote_id == remote_id)
        {
            return bookmark.content.clone();
        }
        return fallback();
    };
    ordered_variants(&item.variants)
        .into_iter()
        .next()
        .or_else(|| item.variants.first())
        .map_or_else(fallback, |variant| variant.content.clone())
}

/// Build the complete typed workspace snapshot consumed by a surface. No
/// playable cached entry is discarded during migration; queues without a
/// matching retained catalog row keep their explicit `legacy` identity.
#[must_use]
pub fn workspace_snapshot(
    queue: &Queue,
    engine: Option<&Engine>,
    revision: u64,
) -> MusicWorkspaceSnapshotV1 {
    workspace_snapshot_from_dirs(
        queue,
        engine,
        revision,
        &state::data_dir(),
        &state::coordination_dir(),
    )
}

#[cfg(test)]
fn workspace_snapshot_from_dir(
    queue: &Queue,
    engine: Option<&Engine>,
    revision: u64,
    data_dir: &Path,
) -> MusicWorkspaceSnapshotV1 {
    workspace_snapshot_from_dirs(queue, engine, revision, data_dir, data_dir)
}

fn workspace_snapshot_from_dirs(
    queue: &Queue,
    engine: Option<&Engine>,
    revision: u64,
    data_dir: &Path,
    coordination_dir: &Path,
) -> MusicWorkspaceSnapshotV1 {
    let queue_revision = queue::revision(queue);
    let catalog = load_catalog(data_dir).unwrap_or_default();
    let current = queue
        .current()
        .map(|remote_id| workspace_content_ref(&catalog, remote_id));
    let current_position = engine.map_or(0, |value| value.position_ms());
    let playing = engine.is_some_and(|value| value.is_playing());
    let (shuffle, repeat) = crate::mpris::workspace_playback_policy(data_dir);
    let duration_ms = None;
    let queue_entries = queue
        .songs
        .iter()
        .enumerate()
        .map(|(index, remote_id)| QueueEntry {
            id: format!("legacy-{index}"),
            content: workspace_content_ref(&catalog, remote_id),
            title: remote_id.clone(),
        })
        .collect();
    let shelves = build_shelves(&catalog.albums, &catalog.starred, &catalog.recent);
    let mut collections = Vec::new();
    for (key, title, kind, items) in [
        ("albums", "Albums", ContentKind::Album, &catalog.albums),
        ("artists", "Artists", ContentKind::Artist, &catalog.artists),
        (
            "podcasts",
            "Podcasts",
            ContentKind::Podcast,
            &catalog.podcasts,
        ),
        ("radio", "Radio", ContentKind::Radio, &catalog.radio),
        (
            "episodes",
            "Episodes",
            ContentKind::Episode,
            &catalog.episodes,
        ),
        ("songs", "Songs", ContentKind::Music, &catalog.songs),
    ] {
        let page = catalog.pages.get(key);
        let projected_items = page.map_or(items.as_slice(), |page| page.items.as_slice());
        if !projected_items.is_empty() || page.is_some() {
            collections.push(LibraryCollection {
                key: key.to_string(),
                title: title.to_string(),
                kind,
                items: projected_items.to_vec(),
                mutable: false,
                offset: page.map_or(0, |page| page.offset),
                page_size: page.map_or(0, |page| page.size),
                has_more: page.is_some_and(|page| page.has_more),
            });
        }
    }
    let any_source_reachable = catalog.sources.iter().any(|source| source.reachable);
    let sources = catalog.sources;
    MusicWorkspaceSnapshotV1 {
        schema_version: MUSIC_CONTRACT_VERSION,
        revision,
        shelves,
        bookmarks: catalog.bookmarks,
        collections,
        search: catalog.search,
        playback: PlaybackSnapshot {
            current,
            playing,
            position_ms: current_position,
            duration_ms,
            volume_milli: engine.map_or(1000, |value| {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let volume = (value.volume().clamp(0.0, 1.0) * 1000.0) as u16;
                volume
            }),
            shuffle,
            repeat: repeat.to_string(),
            queue_revision,
            seekable: engine.is_some_and(|value| value.is_seekable()),
        },
        queue: queue_entries,
        downloads: load_downloads(&state::data_dir()).unwrap_or_default(),
        storage: MusicStorageSnapshot {
            used_bytes: crate::cache::read_index(&crate::cache::cache_dir()).total_bytes(),
            cap_bytes: cache_cap_bytes(),
        },
        targets: playback_targets(engine.is_some(), coordination_dir),
        sources,
        any_source_reachable,
    }
}

/// Publish one complete workspace snapshot over the retained state lane.
pub fn publish_workspace_snapshot(
    persist: &Persist,
    queue_path: &Path,
    engine: Option<&Engine>,
    revision: u64,
) {
    let snapshot = workspace_snapshot_from_dirs(
        &queue::read_from(queue_path),
        engine,
        revision,
        &state::data_dir(),
        &state::coordination_dir(),
    );
    if snapshot.validate().is_ok() {
        if let Ok(body) = serde_json::to_string(&snapshot) {
            let _ = persist.write(WORKSPACE_STATE_TOPIC, Priority::Default, None, Some(&body));
        }
    }
}

/// Compare two workspace projections without treating the monotonic revision
/// as content.  Revisions exist to order real updates; incrementing one for an
/// unchanged idle projection only creates synchronized JSON/index writes on
/// every seat and does not help a reader converge.
#[must_use]
fn workspace_snapshot_content_eq(
    left: &MusicWorkspaceSnapshotV1,
    right: &MusicWorkspaceSnapshotV1,
) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    left.revision = 0;
    right.revision = 0;
    left == right
}

/// Result of dispatching one action: the JSON reply + whether the queue
/// changed (and so must be persisted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dispatch {
    /// JSON written to `reply/<request-ulid>`.
    pub reply_json: String,
    /// Whether the queue changed and must be persisted.
    pub mutated: bool,
}

/// Extract a song-id from a request body: either a bare string or
/// `{"song_id": "..."}`.
#[must_use]
fn song_id_from(body: &str) -> Option<String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        // Accept both `song_id` and `id` (the Hub's get-song / the GUI's
        // browse-by-id callers use `id`; older callers use `song_id`). Reading
        // only `song_id` made `{"id":"14119"}` fall through to the raw-body
        // branch below, which passed the WHOLE JSON string as the Airsonic id →
        // HTTP 400 → the Notification Hub showed "Unknown Track" for every song.
        for key in ["song_id", "id"] {
            if let Some(s) = v.get(key).and_then(serde_json::Value::as_str) {
                return Some(s.to_string());
            }
        }
        if let Some(s) = v.as_str() {
            return Some(s.to_string());
        }
        // A JSON object we didn't recognise — don't feed it raw to Airsonic.
        if v.is_object() {
            return None;
        }
    }
    // Fall back to the raw body as the id (a bare, unquoted id string).
    Some(trimmed.trim_matches('"').to_string())
}

/// Extract a bounded search query from either `{"query":"..."}` or the
/// legacy bare-string request shape.
fn search_query_from(body: &str) -> String {
    let query = serde_json::from_str::<Value>(body.trim())
        .ok()
        .and_then(|value| {
            value
                .get("query")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .or_else(|| song_id_from(body))
        .unwrap_or_default();
    query
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_PLAYLIST_FIELD_BYTES)
        .collect()
}

fn queue_reply(q: &Queue, mutated: bool) -> Dispatch {
    Dispatch {
        reply_json: json!({
            "ok": true,
            "len": q.len(),
            "current": q.current(),
            "songs": q.songs,
        })
        .to_string(),
        mutated,
    }
}

fn error_reply(message: &str) -> Dispatch {
    Dispatch {
        reply_json: json!({ "ok": false, "error": message }).to_string(),
        mutated: false,
    }
}

/// Dispatch a peer verb against the AIR-8 state `dir`. `peer-states`
/// returns every peer's last snapshot (the Peers-tab roster);
/// `take-over` posts a handoff intent asking `<body>` (a host, or empty
/// to claim an idle mesh) to yield, via AIR-8 `post_takeover`.
#[must_use]
pub fn dispatch_peer(verb: &str, body: &str, dir: &Path) -> String {
    match verb {
        "peer-states" => {
            json!({ "ok": true, "result": { "peers": state::read_all_peer_states(dir) } })
                .to_string()
        }
        "take-over" => {
            let to = body.trim().trim_matches('"').to_string();
            let to_peer = if to.is_empty() { None } else { Some(to) };
            match state::post_takeover(dir, &state::local_host(), to_peer, state::now_ms()) {
                Ok(intent) => json!({ "ok": true, "intent_id": intent.intent_id }).to_string(),
                Err(e) => json!({ "ok": false, "error": e.to_string() }).to_string(),
            }
        }
        other => json!({ "ok": false, "error": format!("unknown peer verb: {other}") }).to_string(),
    }
}

/// Apply one `action/music/<verb>` request to `q`, returning the reply.
#[must_use]
pub fn dispatch_queue_action(verb: &str, body: &str, q: &mut Queue) -> Dispatch {
    match verb {
        "enqueue" => match song_id_from(body) {
            Some(id) => {
                q.enqueue(id);
                queue_reply(q, true)
            }
            None => error_reply("enqueue: missing song_id"),
        },
        "enqueue-after" => match song_id_from(body) {
            Some(id) => {
                q.enqueue_after_current(id);
                queue_reply(q, true)
            }
            None => error_reply("enqueue-after: missing song_id"),
        },
        "clear" => {
            q.clear();
            queue_reply(q, true)
        }
        "next" => {
            q.next();
            queue_reply(q, true)
        }
        "prev" => {
            q.prev();
            queue_reply(q, true)
        }
        "get-queue" => queue_reply(q, false),
        // MUSIC-RFX-1 — queue management. Indices come from the JSON body.
        "queue-move" => {
            let v: serde_json::Value = serde_json::from_str(body).unwrap_or(json!({}));
            match (
                v.get("from").and_then(serde_json::Value::as_u64),
                v.get("to").and_then(serde_json::Value::as_u64),
            ) {
                (Some(f), Some(t)) => {
                    let ok = q.move_track(f as usize, t as usize);
                    queue_reply(q, ok)
                }
                _ => error_reply("queue-move: need {from,to}"),
            }
        }
        "queue-remove" => match index_from(body) {
            Some(i) => {
                let ok = q.remove(i);
                queue_reply(q, ok)
            }
            None => error_reply("queue-remove: need {index}"),
        },
        "queue-remove-many" => {
            let v: serde_json::Value = serde_json::from_str(body).unwrap_or(json!({}));
            let idxs: Vec<usize> = v
                .get("indices")
                .and_then(|a| a.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_u64().map(|n| n as usize))
                        .collect()
                })
                .unwrap_or_default();
            let removed = q.remove_many(&idxs);
            queue_reply(q, removed > 0)
        }
        "queue-move-to-next" => match index_from(body) {
            Some(i) => {
                let ok = q.move_to_next(i);
                queue_reply(q, ok)
            }
            None => error_reply("queue-move-to-next: need {index}"),
        },
        other => error_reply(&format!("unknown verb: {other}")),
    }
}

/// Extract a `Vec<String>` from a JSON object field that's an array of strings
/// (e.g. `song_ids`, `add`, `remove_indices`). Numbers are stringified so a
/// caller can send `remove_indices: [0,2]` as numbers or strings. Missing /
/// non-array → empty.
fn str_array(v: &serde_json::Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    x.as_str()
                        .map(str::to_string)
                        .or_else(|| x.as_u64().map(|n| n.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// MUSIC-RESPONSIVE-4 — the LOCAL (per-node), always-readable cover-art cache
/// dir: `<music-cache>/artwork/`. Distinct from the communal Syncthing mesh
/// artwork dir (`crate::cache::artwork_dir`), which can be down — this one lives
/// under the daemon's own `$HOME/.local/share` so the path returned to the GUI is
/// always openable on the node that served the RPC.
#[must_use]
fn local_artwork_dir() -> PathBuf {
    crate::cache::cache_dir().join("artwork")
}

/// MUSIC-RESPONSIVE-4 — write `bytes` to the local cover-art cache and return the
/// absolute file path (creating the dir + writing via a temp-then-rename so a
/// concurrent reader never sees a half-written image). The id is sanitized to a
/// single safe filename via [`crate::cache::artwork_filename`] (Subsonic ids are
/// never trusted as paths). `None` on any IO failure (no dir, read-only, race).
#[must_use]
fn materialize_local_artwork(cover_id: &str, bytes: &[u8]) -> Option<PathBuf> {
    if bytes.is_empty() {
        return None;
    }
    let dir = local_artwork_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let name = crate::cache::artwork_filename(cover_id);
    let path = dir.join(&name);
    // Already present + non-empty (a prior pull) → reuse it, no rewrite.
    if std::fs::metadata(&path)
        .map(|m| m.len() > 0)
        .unwrap_or(false)
    {
        return Some(path);
    }
    let tmp = dir.join(format!(".{name}.tmp"));
    std::fs::write(&tmp, bytes).ok()?;
    std::fs::rename(&tmp, &path).ok()?;
    Some(path)
}

/// MUSIC-RESPONSIVE-4 — build the `get-cover-art` reply that carries a file PATH
/// instead of base64 bytes. Materializes `bytes` into the local cache and returns
/// `{ "path": "<abs>", "bytes": <n> }`. On a materialize failure (e.g. a
/// read-only cache) it falls back to the legacy base64 `{ "art": … }` shape so
/// art never silently disappears — but the steady-state reply is a short path, so
/// the Bus spool no longer grows with cover bytes. Infallible — a materialize
/// failure degrades to the legacy base64 shape rather than erroring.
#[must_use]
fn cover_art_path_reply(cover_id: &str, bytes: &[u8]) -> serde_json::Value {
    if let Some(path) = materialize_local_artwork(cover_id, bytes) {
        json!({ "path": path.to_string_lossy(), "bytes": bytes.len() })
    } else {
        use base64::Engine;
        json!({ "art": base64::engine::general_purpose::STANDARD.encode(bytes) })
    }
}

/// Parse a queue index from a request body: `{"index":N}` or a bare number.
fn index_from(body: &str) -> Option<usize> {
    let v: serde_json::Value = serde_json::from_str(body.trim()).ok()?;
    v.get("index")
        .and_then(serde_json::Value::as_u64)
        .or_else(|| v.as_u64())
        .map(|n| n as usize)
}

#[derive(Debug, Clone, Copy)]
struct BrowsePageRequest {
    offset: usize,
    size: usize,
}

fn browse_page_request(body: &str) -> BrowsePageRequest {
    let value = serde_json::from_str::<Value>(body).unwrap_or_default();
    let offset = value
        .get("offset")
        .and_then(Value::as_u64)
        .map_or(0, |value| (value as usize).min(MAX_LIBRARY_OFFSET));
    let size = value
        .get("size")
        .and_then(Value::as_u64)
        .map_or(100, |value| {
            (value as usize).clamp(1, MAX_LIBRARY_PAGE_SIZE)
        });
    BrowsePageRequest { offset, size }
}

fn page_slice<T>(items: Vec<T>, request: BrowsePageRequest) -> (Vec<T>, usize, bool) {
    let total = items.len();
    let start = request.offset.min(total);
    let end = start.saturating_add(request.size).min(total);
    let page = items.into_iter().skip(start).take(end - start).collect();
    (page, start, end < total)
}

fn browse_collection_key(verb: &str) -> Option<&'static str> {
    match verb {
        "list-albums" => Some("albums"),
        "list-artists" => Some("artists"),
        "list-podcasts" => Some("podcasts"),
        "list-radio" => Some("radio"),
        _ => None,
    }
}

fn record_browse_page(
    file: &mut CatalogStoreFile,
    source_id: &str,
    verb: &str,
    body: &str,
    result: &Value,
) {
    let Some(key) = browse_collection_key(verb) else {
        return;
    };
    let items = match verb {
        "list-albums" => decode_catalog_rows::<crate::airsonic::Album>(result, "albums")
            .iter()
            .filter_map(|album| album_catalog_item(source_id, album, false))
            .collect::<Vec<_>>(),
        "list-artists" => decode_catalog_rows::<crate::airsonic::Artist>(result, "artists")
            .iter()
            .filter_map(|artist| artist_catalog_item(source_id, artist))
            .collect::<Vec<_>>(),
        "list-podcasts" => {
            decode_catalog_rows::<crate::airsonic::PodcastChannel>(result, "podcasts")
                .iter()
                .filter_map(|channel| podcast_catalog_item(source_id, channel))
                .collect::<Vec<_>>()
        }
        "list-radio" => decode_catalog_rows::<crate::airsonic::RadioStation>(result, "radio")
            .iter()
            .filter_map(|station| radio_catalog_item(source_id, station))
            .collect::<Vec<_>>(),
        _ => return,
    };
    let request = browse_page_request(body);
    let offset = result
        .get("offset")
        .and_then(Value::as_u64)
        .map_or(request.offset, |value| {
            (value as usize).min(MAX_LIBRARY_OFFSET)
        });
    let size = result
        .get("size")
        .and_then(Value::as_u64)
        .map_or(request.size, |value| {
            (value as usize).clamp(1, MAX_LIBRARY_PAGE_SIZE)
        });
    let has_more = result
        .get("has_more")
        .and_then(Value::as_bool)
        .unwrap_or(items.len() == size);
    let page = CatalogPage {
        offset,
        size,
        has_more,
        items,
    };
    if let Some(previous) = file.pages.get_mut(key) {
        if previous.offset == page.offset {
            let merged = dedup_catalog(
                previous
                    .items
                    .drain(..)
                    .chain(page.items)
                    .collect::<Vec<_>>(),
            );
            previous.items = merged.into_iter().take(MAX_COLLECTION_ITEMS).collect();
            previous.size = previous
                .size
                .max(page.size)
                .max(previous.items.len())
                .min(MAX_LIBRARY_PAGE_SIZE);
            previous.has_more |= page.has_more;
            return;
        }
    }
    file.pages.insert(key.to_string(), page);
}

/// Reply JSON for a library-browse verb. Proxies the Airsonic REST call
/// via the shared [`Client`]; missing creds / server errors become an
/// `{ok:false,error}` reply rather than a panic. I/O, so not pure — the
/// URL-building + parse logic it leans on is unit-tested in [`crate::airsonic`].
fn dispatch_browse(
    verb: &str,
    body: &str,
    client: &Client,
    rt: &tokio::runtime::Runtime,
) -> String {
    let result: Result<serde_json::Value, String> = rt.block_on(async {
        match verb {
            "list-albums" => {
                let request = browse_page_request(body);
                client
                    .get_album_list2_page("newest", request.size as u32, request.offset as u32)
                    .await
                    .map(|page| {
                        let has_more = page.albums.len() == page.page_size as usize;
                        json!({
                            "albums": page.albums,
                            "offset": page.offset,
                            "size": page.page_size,
                            "has_more": has_more,
                        })
                    })
                    .map_err(|e| e.to_string())
            }
            "list-artists" => {
                let request = browse_page_request(body);
                client
                    .get_artists()
                    .await
                    .map(|artists| {
                        let (artists, offset, has_more) = page_slice(artists, request);
                        json!({
                            "artists": artists,
                            "offset": offset,
                            "size": request.size,
                            "has_more": has_more,
                        })
                    })
                    .map_err(|e| e.to_string())
            }
            "search" => {
                let query = search_query_from(body);
                client
                    .search3(&query)
                    .await
                    .map(|r| json!({ "artists": r.artists, "albums": r.albums, "songs": r.songs }))
                    .map_err(|e| e.to_string())
            }
            "get-album" => {
                let id = song_id_from(body).unwrap_or_default();
                client
                    .get_album(&id)
                    .await
                    .map(|a| json!({ "album": a.album, "songs": a.songs }))
                    .map_err(|e| e.to_string())
            }
            "list-genres" => client
                .get_genres()
                .await
                .map(|g| json!({ "genres": g }))
                .map_err(|e| e.to_string()),
            // MUSIC-HOME-1 — the Home page's library snapshot (counts + scan +
            // server identity). Infallible (best-effort per sub-call); a down
            // server yields `reachable:false`.
            "library-stats" => Ok(json!({ "stats": client.library_stats().await })),
            "get-song" => {
                let id = song_id_from(body).unwrap_or_default();
                client
                    .get_song(&id)
                    .await
                    .map(|s| json!({ "song": s }))
                    .map_err(|e| e.to_string())
            }
            "albums-by-genre" => {
                let genre = song_id_from(body).unwrap_or_default();
                client
                    .get_albums_by_genre(&genre, 200)
                    .await
                    .map(|a| json!({ "albums": a }))
                    .map_err(|e| e.to_string())
            }
            // Artist browse — one artist's albums (the dead "click an artist"
            // path now loads its next layer).
            "albums-by-artist" | "get-artist" => {
                let id = song_id_from(body).unwrap_or_default();
                client
                    .get_artist(&id)
                    .await
                    .map(|a| json!({ "albums": a }))
                    .map_err(|e| e.to_string())
            }
            "get-cover-art" => {
                let id = song_id_from(body).unwrap_or_default();
                // MUSIC-RESPONSIVE-4 — serve cover art by file PATH, not
                // base64-over-bus. The base64 blob used to ride the reply onto
                // `reply/<ulid>` in the Bus persistence store, so every cover the
                // GUI grid requested grew the spool (the EFF-47 ephemeral reaper
                // only bounded it after the fact). Now the daemon materializes the
                // image into a LOCAL, always-readable cache file (NOT the Syncthing
                // mesh share — that addresses the deferral's "regress when the
                // share is down" concern) and returns just its path; the GUI opens
                // the file directly. The reply now carries a short path string, so
                // the Bus spool no longer grows with art bytes.
                //
                // MUSIC-ART-SYNC still applies: a communal mesh-cache hit (art any
                // node already pulled) is reused without an Airsonic round-trip,
                // and a fresh Airsonic pull is written THROUGH to the mesh cache
                // for every other node. Either way the bytes are also mirrored to
                // the local cache so the returned path is valid on this node even
                // when the mesh mount is unreachable.
                if let Some(bytes) = crate::cache::read_shared_artwork(&id) {
                    Ok(cover_art_path_reply(&id, &bytes))
                } else {
                    client
                        .get_cover_art_bytes(&id)
                        .await
                        .map(|bytes| {
                            crate::cache::write_shared_artwork(&id, &bytes);
                            cover_art_path_reply(&id, &bytes)
                        })
                        .map_err(|e| e.to_string())
                }
            }
            "list-podcasts" => {
                let request = browse_page_request(body);
                client
                    .get_podcast_channels()
                    .await
                    .map(|podcasts| {
                        let (podcasts, offset, has_more) = page_slice(podcasts, request);
                        json!({
                            "podcasts": podcasts,
                            "offset": offset,
                            "size": request.size,
                            "has_more": has_more,
                        })
                    })
                    .map_err(|e| e.to_string())
            }
            // SVC-3 — the Radio hub card: the server's saved stations.
            // MUSIC-RESPONSIVE-7 — serve a fresh cached list (no upstream call);
            // only the first open (or a stale cache) hits the server.
            "list-radio" => {
                let cached = RADIO_CACHE.lock().ok().and_then(|g| {
                    g.as_ref()
                        .filter(|(at, _)| at.elapsed() < RADIO_CACHE_TTL)
                        .map(|(_, v)| v.clone())
                });
                let request = browse_page_request(body);
                if let Some(v) = cached {
                    let stations = v
                        .get("radio")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    let (stations, offset, has_more) = page_slice(stations, request);
                    Ok(json!({
                        "radio": stations,
                        "offset": offset,
                        "size": request.size,
                        "has_more": has_more,
                    }))
                } else {
                    match client.get_internet_radio_stations().await {
                        Ok(r) => {
                            let v = json!({ "radio": r });
                            if let Ok(mut g) = RADIO_CACHE.lock() {
                                *g = Some((Instant::now(), v.clone()));
                            }
                            let stations = v
                                .get("radio")
                                .and_then(Value::as_array)
                                .cloned()
                                .unwrap_or_default();
                            let (stations, offset, has_more) = page_slice(stations, request);
                            Ok(json!({
                                "radio": stations,
                                "offset": offset,
                                "size": request.size,
                                "has_more": has_more,
                            }))
                        }
                        Err(e) => Err(e.to_string()),
                    }
                }
            }
            "podcast-episodes" => {
                let id = song_id_from(body).unwrap_or_default();
                client
                    .get_podcast_episodes(&id)
                    .await
                    .map(|e| json!({ "episodes": e }))
                    .map_err(|e| e.to_string())
            }
            "list-bookmarks" => client
                .get_bookmarks()
                .await
                .map(|bookmarks| json!({ "bookmarks": bookmarks }))
                .map_err(|e| e.to_string()),
            // AIR-4.b — Recents hub card: recently-added albums (reuses
            // getAlbumList2 with type=recent).
            "list-recents" => client
                .get_album_list2("recent", 100)
                .await
                .map(|a| json!({ "albums": a }))
                .map_err(|e| e.to_string()),
            // MUSIC-HOME-3 — most-played (getAlbumList2 frequent) + starred.
            "list-frequent" => client
                .get_album_list2("frequent", 24)
                .await
                .map(|a| json!({ "albums": a }))
                .map_err(|e| e.to_string()),
            "list-starred" => client
                .get_starred2()
                .await
                .map(|a| json!({ "albums": a }))
                .map_err(|e| e.to_string()),
            // AIR-4.b — Playlists hub card: the playlist roster, then a
            // single playlist's songs (the GUI enqueues these to play it).
            "list-playlists" => client
                .get_playlists()
                .await
                .map(|p| json!({ "playlists": p }))
                .map_err(|e| e.to_string()),
            "get-playlist" => {
                let id = song_id_from(body).unwrap_or_default();
                client
                    .get_playlist(&id)
                    .await
                    .map(|s| json!({ "songs": s }))
                    .map_err(|e| e.to_string())
            }
            "get-lyrics" => {
                let id = song_id_from(body).unwrap_or_default();
                client
                    .get_lyrics_by_song_id(&id)
                    .await
                    .map(|lines| json!({ "lyrics": lines }))
                    .map_err(|e| e.to_string())
            }
            // MUSIC-RFX-3 — playlist write verbs. Body is a JSON object:
            //   playlist-create {"name":..,"song_ids":[..]?}
            //   playlist-update {"id":..,"name":..?,"add":[..]?,"remove_indices":[..]?}
            //   playlist-delete {"id":..} | "<id>"
            "playlist-create" => {
                let v: serde_json::Value = serde_json::from_str(body.trim()).unwrap_or(json!({}));
                let name = v.get("name").and_then(serde_json::Value::as_str);
                match name {
                    Some(name) if !name.is_empty() => {
                        let song_ids = str_array(&v, "song_ids");
                        client
                            .create_playlist(name, &song_ids)
                            .await
                            .map(|()| json!({ "created": name }))
                            .map_err(|e| e.to_string())
                    }
                    _ => Err("playlist-create: need {name}".to_string()),
                }
            }
            "playlist-update" => {
                let v: serde_json::Value = serde_json::from_str(body.trim()).unwrap_or(json!({}));
                match v.get("id").and_then(serde_json::Value::as_str) {
                    Some(id) if !id.is_empty() => {
                        let name = v
                            .get("name")
                            .and_then(serde_json::Value::as_str)
                            .filter(|s| !s.is_empty());
                        let add = str_array(&v, "add");
                        let remove = str_array(&v, "remove_indices");
                        client
                            .update_playlist(id, name, &add, &remove)
                            .await
                            .map(|()| json!({ "updated": id }))
                            .map_err(|e| e.to_string())
                    }
                    _ => Err("playlist-update: need {id}".to_string()),
                }
            }
            "playlist-delete" => {
                let id = song_id_from(body).unwrap_or_default();
                if id.is_empty() {
                    Err("playlist-delete: need {id}".to_string())
                } else {
                    client
                        .delete_playlist(&id)
                        .await
                        .map(|()| json!({ "deleted": id }))
                        .map_err(|e| e.to_string())
                }
            }
            // MUSIC-RFX-6b — reorder a playlist in place. Body:
            //   playlist-reorder {"id":..,"order":[song_id,…]}
            // `order` is the full track set rearranged; the daemon re-applies it
            // via one updatePlaylist (remove-all + re-add) so the id survives.
            "playlist-reorder" => {
                let v: serde_json::Value = serde_json::from_str(body.trim()).unwrap_or(json!({}));
                match v.get("id").and_then(serde_json::Value::as_str) {
                    Some(id) if !id.is_empty() => {
                        let order = str_array(&v, "order");
                        if order.is_empty() {
                            Err("playlist-reorder: need {order:[song_id,…]}".to_string())
                        } else {
                            client
                                .reorder_playlist(id, &order)
                                .await
                                .map(|()| json!({ "reordered": id, "len": order.len() }))
                                .map_err(|e| e.to_string())
                        }
                    }
                    _ => Err("playlist-reorder: need {id}".to_string()),
                }
            }
            other => Err(format!("unknown browse verb: {other}")),
        }
    });
    match result {
        Ok(v) => json!({ "ok": true, "result": v }).to_string(),
        Err(e) => json!({ "ok": false, "error": e }).to_string(),
    }
}

/// Closed mutation contexts for the music action surface. The verb is exact;
/// the target is a stable capability scope, so a queue token cannot authorize
/// transport, playlist, or peer-takeover effects.
fn music_mutation_scope(verb: &str) -> Option<&'static str> {
    match verb {
        "enqueue" | "enqueue-after" | "clear" | "next" | "prev" | "queue-move" | "queue-remove"
        | "queue-remove-many" | "queue-move-to-next" => Some("queue"),
        "playlist-create" | "playlist-update" | "playlist-delete" | "playlist-reorder" => {
            Some("playlists")
        }
        WORKSPACE_ACTION_VERB => Some("workspace"),
        "play" | "pause" | "resume" | "stop" | "set-volume" | "seek" => Some("transport"),
        "take-over" => Some("peer-takeover"),
        // `get-queue`, browse, `get-state`, and `peer-states` are reads.
        _ => None,
    }
}

fn music_action_scope(verb: &str, body: &str) -> Option<&'static str> {
    if verb == WORKSPACE_ACTION_VERB
        && serde_json::from_str::<Value>(body)
            .ok()
            .is_some_and(|value| value.get("action").and_then(Value::as_str) == Some("transfer"))
    {
        return Some("peer-takeover");
    }
    music_mutation_scope(verb)
}

fn authorize_music_mutation(
    authorizer: &music_action_auth::Authorizer,
    verb: &str,
    body: &str,
) -> Result<(), String> {
    let Some(target) = music_action_scope(verb, body) else {
        return Ok(());
    };
    let auth_verb = format!("music-{verb}");
    let node = state::local_host();
    authorizer.authorize(body, &auth_verb, &node, target)
}

fn unauthorized_reply(verb: &str, error: &str) -> String {
    json!({
        "ok": false,
        "error": format!("{verb}: authorization refused: {error}")
    })
    .to_string()
}

fn request_id_from_body(body: &str) -> String {
    let Some(request_id) = serde_json::from_str::<Value>(body).ok().and_then(|value| {
        value
            .get("request_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
    }) else {
        return "invalid-request".to_string();
    };
    if request_id.is_empty()
        || request_id.len() > crate::domain::MAX_REQUEST_ID_BYTES
        || request_id.chars().any(char::is_control)
    {
        "invalid-request".to_string()
    } else {
        request_id
    }
}

fn typed_reply(result: &MusicActionResultV1) -> String {
    // All fields are bounded scalar values, so serialization cannot fail in
    // practice. Keep the fallback typed and redacted if that ever changes.
    serde_json::to_string(result).unwrap_or_else(|_| {
        r#"{"schema_version":1,"request_id":"invalid-request","accepted":false,"revision":0,"error_code":"reply_failed"}"#
            .to_string()
    })
}

fn typed_transport_error(reply: &str) -> &'static str {
    let Ok(value) = serde_json::from_str::<Value>(reply) else {
        return "transport_rejected";
    };
    let Some(error) = value.get("error").and_then(Value::as_str) else {
        return "transport_rejected";
    };
    if error.contains("no audio output") {
        "audio_unavailable"
    } else if error.contains("Airsonic") {
        "source_unavailable"
    } else if error.contains("queue is empty") {
        "queue_empty"
    } else if error == "not_seekable" {
        "not_seekable"
    } else if error == "nothing_to_resume" {
        "nothing_to_resume"
    } else {
        "transport_rejected"
    }
}

fn accepted_json(reply: &str) -> Result<bool, &'static str> {
    let value = serde_json::from_str::<Value>(reply).map_err(|_| "transport_rejected")?;
    if value.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(true)
    } else {
        Err(typed_transport_error(reply))
    }
}

#[cfg(test)]
fn apply_workspace_action(
    request: &MusicActionRequestV1,
    queue: &mut Queue,
    engine: Option<&Engine>,
    client: Option<&Client>,
    rt: &tokio::runtime::Runtime,
    data_dir: &Path,
) -> Result<bool, &'static str> {
    let clients = client.into_iter().collect::<Vec<_>>();
    apply_workspace_action_with_clients(request, queue, engine, &clients, rt, data_dir)
}

/// Apply one typed workspace action against the bounded set of admitted
/// sources.  The compatibility wrapper above keeps the focused single-client
/// callers intact, while the production responder passes the complete source
/// set so a selected catalog variant can be played without bypassing the
/// daemon-owned queue and engine authorities.
#[cfg(test)]
fn apply_workspace_action_with_clients(
    request: &MusicActionRequestV1,
    queue: &mut Queue,
    engine: Option<&Engine>,
    clients: &[&Client],
    rt: &tokio::runtime::Runtime,
    data_dir: &Path,
) -> Result<bool, &'static str> {
    apply_workspace_action_with_clients_and_coordination(
        request, queue, engine, clients, rt, data_dir, data_dir,
    )
}

fn apply_workspace_action_with_clients_and_coordination(
    request: &MusicActionRequestV1,
    queue: &mut Queue,
    engine: Option<&Engine>,
    clients: &[&Client],
    rt: &tokio::runtime::Runtime,
    data_dir: &Path,
    coordination_dir: &Path,
) -> Result<bool, &'static str> {
    let client = clients.first().copied();
    match request.action.as_str() {
        "play" => {
            if let Some(content) = request.content.as_ref() {
                if !matches!(
                    content.kind,
                    ContentKind::Music
                        | ContentKind::Episode
                        | ContentKind::Chapter
                        | ContentKind::Audiobook
                        | ContentKind::Radio
                ) {
                    return Err("unsupported_content");
                }
                if content.source_id != "legacy" {
                    let catalog = load_catalog(data_dir).unwrap_or_default();
                    let typed_resume = matches!(
                        content.kind,
                        ContentKind::Episode
                            | ContentKind::Chapter
                            | ContentKind::Audiobook
                            | ContentKind::Radio
                    );
                    let mut playback_queue = queue.clone();
                    let queue_changed =
                        if playback_queue.current() == Some(content.remote_id.as_str()) {
                            false
                        } else if typed_resume {
                            playback_queue.select_or_enqueue(content.remote_id.clone())
                        } else {
                            return Err("content_not_current");
                        };
                    let upcoming = selected_source_upcoming_candidates(
                        &playback_queue,
                        content,
                        clients,
                        &catalog,
                        &crate::cache::cache_dir(),
                    )?;
                    let engine = engine.ok_or("audio_unavailable")?;
                    let position_ms = typed_play_start_position(content, request.position_ms)?;
                    if !engine.play_from_candidates_at(
                        upcoming,
                        playback_queue.current,
                        position_ms,
                    ) {
                        return Err("audio_unavailable");
                    }
                    *queue = playback_queue;
                    write_playback_state(true, content.remote_id.as_str(), position_ms);
                    return Ok(queue_changed);
                }
                if queue.current() != Some(content.remote_id.as_str()) {
                    return Err("content_not_current");
                }
            }
            accepted_json(&apply_transport_with_clients(
                "play", "", engine, clients, queue,
            ))
        }
        "pause" | "resume" | "stop" => {
            if matches!(request.action.as_str(), "pause" | "stop") {
                finalize_progress_for_transport(
                    queue,
                    request.content.as_ref(),
                    engine,
                    clients,
                    rt,
                    data_dir,
                    request.action.as_str(),
                );
            }
            accepted_json(&apply_transport_with_clients(
                request.action.as_str(),
                "",
                engine,
                clients,
                queue,
            ))
        }
        "seek" => {
            let position_ms = request.position_ms.ok_or("missing_position")?;
            let body = json!({ "position_ms": position_ms }).to_string();
            accepted_json(&apply_transport_with_clients(
                "seek", &body, engine, clients, queue,
            ))
        }
        "set_volume" => {
            let volume_milli = request.volume_milli.ok_or("missing_volume")?;
            let body = json!({ "volume": f32::from(volume_milli) / 1000.0 }).to_string();
            accepted_json(&apply_transport_with_clients(
                "set-volume",
                &body,
                engine,
                clients,
                queue,
            ))
        }
        "star" | "unstar" => {
            let content = request.content.as_ref().ok_or("missing_content")?;
            if !matches!(
                content.kind,
                ContentKind::Music | ContentKind::Album | ContentKind::Artist
            ) {
                return Err("unsupported_source");
            }
            let client = if content.source_id == "legacy" {
                client.ok_or("source_unavailable")?
            } else {
                let catalog = load_catalog(data_dir).unwrap_or_default();
                if !catalog_contains_variant(&catalog, content) {
                    return Err("unsupported_source");
                }
                clients
                    .iter()
                    .copied()
                    .find(|candidate| catalog_source_id(candidate) == content.source_id)
                    .ok_or("source_unavailable")?
            };
            let result = if request.action == "star" {
                rt.block_on(client.star(&content.remote_id))
            } else {
                rt.block_on(client.unstar(&content.remote_id))
            };
            result.map(|()| false).map_err(|_| "curation_failed")
        }
        "scrobble" => {
            let content = request.content.as_ref().ok_or("missing_content")?;
            let client = admitted_progress_client(content, clients, data_dir)?;
            let position_ms = request.position_ms.ok_or("missing_position")?;
            rt.block_on(client.scrobble(&content.remote_id, position_ms))
                .map(|()| false)
                .map_err(|_| "progress_write_failed")
        }
        "bookmark" | "bookmark_delete" => {
            let content = request.content.as_ref().ok_or("missing_content")?;
            if !matches!(
                content.kind,
                ContentKind::Episode | ContentKind::Chapter | ContentKind::Audiobook
            ) {
                return Err("unsupported_source");
            }
            let client = admitted_progress_client(content, clients, data_dir)?;
            let result = if request.action == "bookmark" {
                rt.block_on(client.create_bookmark(
                    &content.remote_id,
                    request.position_ms.ok_or("missing_position")?,
                ))
            } else {
                rt.block_on(client.delete_bookmark(&content.remote_id))
            };
            result.map(|()| false).map_err(|_| "bookmark_write_failed")
        }
        "shuffle" => {
            crate::mpris::set_workspace_shuffle(data_dir, request.shuffle.ok_or("missing_shuffle")?)
                .map(|()| false)
        }
        "repeat" => crate::mpris::set_workspace_repeat(
            data_dir,
            request.repeat.as_deref().ok_or("missing_repeat")?,
        )
        .map(|()| false),
        "next" => {
            let dispatch = dispatch_queue_action("next", "", queue);
            accepted_json(&dispatch.reply_json).map(|_| dispatch.mutated)
        }
        "previous" => {
            let dispatch = dispatch_queue_action("prev", "", queue);
            accepted_json(&dispatch.reply_json).map(|_| dispatch.mutated)
        }
        "queue_clear" => {
            let dispatch = dispatch_queue_action("clear", "", queue);
            accepted_json(&dispatch.reply_json).map(|_| dispatch.mutated)
        }
        "queue_move" => {
            let from = request.queue_index.ok_or("missing_queue_index")?;
            let to = request
                .target_queue_index
                .ok_or("missing_target_queue_index")?;
            let body = json!({ "from": from, "to": to }).to_string();
            let dispatch = dispatch_queue_action("queue-move", &body, queue);
            accepted_json(&dispatch.reply_json).map(|_| dispatch.mutated)
        }
        "queue_remove" => {
            let index = request.queue_index.ok_or("missing_queue_index")?;
            let body = json!({ "index": index }).to_string();
            let dispatch = dispatch_queue_action("queue-remove", &body, queue);
            accepted_json(&dispatch.reply_json).map(|_| dispatch.mutated)
        }
        "playlist_create" => {
            let client = client.ok_or("source_unavailable")?;
            let name = request
                .playlist_name
                .as_deref()
                .ok_or("missing_playlist_name")?;
            rt.block_on(client.create_playlist(name, &request.playlist_song_ids))
                .map(|()| false)
                .map_err(|_| "playlist_mutation_failed")
        }
        "playlist_update" => {
            let playlist = request.playlist.as_ref().ok_or("missing_playlist")?;
            let client = admitted_playlist_client(request, clients, data_dir)?;
            let remove_indices = request
                .playlist_remove_indices
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>();
            rt.block_on(client.update_playlist(
                &playlist.remote_id,
                request.playlist_name.as_deref(),
                &request.playlist_song_ids,
                &remove_indices,
            ))
            .map(|()| false)
            .map_err(|_| "playlist_mutation_failed")
        }
        "playlist_delete" => {
            let playlist = request.playlist.as_ref().ok_or("missing_playlist")?;
            let client = admitted_playlist_client(request, clients, data_dir)?;
            rt.block_on(client.delete_playlist(&playlist.remote_id))
                .map(|()| false)
                .map_err(|_| "playlist_mutation_failed")
        }
        "playlist_reorder" => {
            let playlist = request.playlist.as_ref().ok_or("missing_playlist")?;
            let client = admitted_playlist_client(request, clients, data_dir)?;
            rt.block_on(client.reorder_playlist(&playlist.remote_id, &request.playlist_song_ids))
                .map(|()| false)
                .map_err(|_| "playlist_mutation_failed")
        }
        "download" => {
            let client = admitted_download_client(request, clients, data_dir)?;
            download_to_cache(request, client, rt, data_dir, &crate::cache::cache_dir())
                .map(|()| false)
        }
        "cancel_download" => cancel_download(request, data_dir).map(|()| false),
        "remove_download" => {
            remove_download(request, data_dir, &crate::cache::cache_dir()).map(|()| false)
        }
        "pin_download" => {
            set_download_pinned(request, data_dir, &crate::cache::cache_dir(), true).map(|()| false)
        }
        "unpin_download" => {
            set_download_pinned(request, data_dir, &crate::cache::cache_dir(), false)
                .map(|()| false)
        }
        "transfer" => {
            let target_peer = request
                .target_peer
                .as_deref()
                .ok_or("missing_target_peer")?;
            let local_peer = state::local_host();
            if target_peer == local_peer {
                return Err("target_is_local_peer");
            }
            let target_state = state::read_all_peer_states(coordination_dir)
                .into_iter()
                .find(|peer| peer.peer == target_peer)
                .ok_or("target_not_admitted")?;
            if state::now_ms().saturating_sub(target_state.updated_ms) > state::STATE_STALE_MS {
                return Err("target_stale");
            }
            if target_state.playing {
                return Err("target_busy");
            }
            let Some(engine) = engine else {
                return Err("audio_unavailable");
            };
            if !engine.is_active() {
                return Err("playback_not_active");
            }
            if state::read_all_peer_states(coordination_dir)
                .into_iter()
                .any(|peer| {
                    peer.peer != local_peer
                        && peer.playing
                        && state::now_ms().saturating_sub(peer.updated_ms) <= state::STATE_STALE_MS
                })
            {
                return Err("playback_owned_elsewhere");
            }
            state::post_takeover(
                coordination_dir,
                target_peer,
                Some(local_peer),
                state::now_ms(),
            )
            .map(|_| false)
            .map_err(|_| "handoff_persist_failed")
        }
        _ => Err("unknown_action"),
    }
}

fn admitted_playlist_client<'a>(
    request: &MusicActionRequestV1,
    clients: &'a [&Client],
    data_dir: &Path,
) -> Result<&'a Client, &'static str> {
    let playlist = request.playlist.as_ref().ok_or("missing_playlist")?;
    if playlist.kind != ContentKind::Playlist {
        return Err("unsupported_source");
    }
    if playlist.source_id == "legacy" {
        return clients.first().copied().ok_or("source_unavailable");
    }
    let catalog = load_catalog(data_dir).unwrap_or_default();
    if !catalog_contains_variant(&catalog, playlist) {
        return Err("unsupported_source");
    }
    clients
        .iter()
        .copied()
        .find(|candidate| catalog_source_id(candidate) == playlist.source_id)
        .ok_or("source_unavailable")
}

/// Resolve the provider for one playable retained identity before writing
/// final playback progress.  The source choice follows the same admission
/// rule as explicit `scrobble`: legacy queues use the primary client, while a
/// non-legacy identity must still be present in the bounded catalog and match
/// an admitted client.  This prevents pause/stop/close from turning a
/// source-less queue id into a write against an unrelated provider.
fn admitted_progress_client<'a>(
    content: &ContentRef,
    clients: &'a [&Client],
    data_dir: &Path,
) -> Result<&'a Client, &'static str> {
    if !matches!(
        content.kind,
        ContentKind::Music | ContentKind::Episode | ContentKind::Chapter | ContentKind::Audiobook
    ) {
        return Err("unsupported_source");
    }
    if content.source_id == "legacy" {
        return clients.first().copied().ok_or("source_unavailable");
    }
    let catalog = load_catalog(data_dir).unwrap_or_default();
    if !catalog_contains_variant(&catalog, content) {
        return Err("unsupported_source");
    }
    clients
        .iter()
        .copied()
        .find(|candidate| catalog_source_id(candidate) == content.source_id)
        .ok_or("source_unavailable")
}

/// Best-effort final progress write for a daemon-owned transport boundary.
/// Playback control remains authoritative even when the provider is offline;
/// the failure is logged with a redacted code and the next explicit or close
/// attempt can retry it.  An explicit identity must describe the current
/// queued remote id, otherwise the request is ignored rather than scrobbling
/// an unrelated item.
fn finalize_progress_for_transport(
    queue: &Queue,
    explicit_content: Option<&ContentRef>,
    engine: Option<&Engine>,
    clients: &[&Client],
    rt: &tokio::runtime::Runtime,
    data_dir: &Path,
    boundary: &str,
) {
    let Some(engine) = engine else { return };
    if !engine.is_active() {
        return;
    }
    let Some(remote_id) = queue.current() else {
        return;
    };
    let selected = explicit_content.cloned().unwrap_or_else(|| {
        workspace_content_ref(&load_catalog(data_dir).unwrap_or_default(), remote_id)
    });
    if selected.remote_id != remote_id {
        tracing::warn!(
            boundary,
            "music final progress skipped for a non-current content identity"
        );
        return;
    }
    let Ok(client) = admitted_progress_client(&selected, clients, data_dir) else {
        tracing::debug!(boundary, source_id = %selected.source_id, "music final progress skipped: source unavailable");
        return;
    };
    if let Err(error) = rt.block_on(client.scrobble(&selected.remote_id, engine.position_ms())) {
        tracing::warn!(boundary, source_id = %selected.source_id, error = %error, "music final progress write failed");
    }
}

fn poll_workspace_with_authorizer(
    persist: &Persist,
    queue_path: &Path,
    engine: Option<&Engine>,
    clients: &[&Client],
    rt: &tokio::runtime::Runtime,
    cursors: &mut HashMap<String, String>,
    ledger: &mut Option<WorkspaceActionLedger>,
    authorizer: &music_action_auth::Authorizer,
) {
    let topic = format!("action/music/{WORKSPACE_ACTION_VERB}");
    let since = cursors.get(&topic).map(String::as_str);
    let msgs = match persist.list_since_limit(&topic, since, MAX_MESSAGES_PER_POLL) {
        Ok(messages) => messages,
        Err(_) => return,
    };
    let mut queue = queue::read_from(queue_path);
    let data_dir = state::data_dir();
    for msg in msgs {
        cursors.insert(topic.clone(), msg.ulid.clone());
        let body = msg.body.as_deref().unwrap_or("");
        let request_id = request_id_from_body(body);
        let reply = if let Err(_error) =
            authorize_music_mutation(authorizer, WORKSPACE_ACTION_VERB, body)
        {
            typed_result(
                &request_id,
                false,
                queue::revision(&queue),
                Some("unauthorized"),
            )
        } else {
            let request = match parse_authorized_workspace_request(body) {
                Ok(request) => request,
                Err(_) => {
                    let result = typed_result(
                        &request_id,
                        false,
                        queue::revision(&queue),
                        Some("invalid_request"),
                    );
                    let reply = typed_reply(&result);
                    let _ = persist.write(
                        &reply_topic(&msg.ulid),
                        Priority::Default,
                        None,
                        Some(&reply),
                    );
                    continue;
                }
            };
            if let Err(error_code) = request.validate() {
                typed_result(
                    &request.request_id,
                    false,
                    queue::revision(&queue),
                    Some(error_code),
                )
            } else {
                let Some(action_ledger) = ledger.as_mut() else {
                    // A corrupt/unreadable retained ledger disables typed
                    // mutations for this process; reads and legacy lanes keep
                    // operating, but no side effect is attempted here.
                    let result = typed_result(
                        &request.request_id,
                        false,
                        queue::revision(&queue),
                        Some("ledger_unavailable"),
                    );
                    let reply = typed_reply(&result);
                    let _ = persist.write(
                        &reply_topic(&msg.ulid),
                        Priority::Default,
                        None,
                        Some(&reply),
                    );
                    continue;
                };
                if action_ledger.contains(&request.request_id) {
                    action_ledger
                        .records
                        .iter()
                        .find(|record| record.request_id == request.request_id)
                        .map(|record| record.result.clone())
                        .unwrap_or_else(|| {
                            typed_result(
                                &request.request_id,
                                false,
                                queue::revision(&queue),
                                Some("replayed_request"),
                            )
                        })
                } else {
                    let revision = queue::revision(&queue);
                    if !action_ledger.reserve(&request.request_id, revision) {
                        typed_result(
                            &request.request_id,
                            false,
                            revision,
                            Some("replayed_request"),
                        )
                    } else if persist_workspace_ledger(&data_dir, action_ledger).is_err() {
                        action_ledger.release(&request.request_id);
                        typed_result(
                            &request.request_id,
                            false,
                            revision,
                            Some("ledger_unavailable"),
                        )
                    } else if request
                        .expected_queue_revision
                        .is_some_and(|expected| expected != revision)
                    {
                        let result = typed_result(
                            &request.request_id,
                            false,
                            revision,
                            Some("stale_queue_revision"),
                        );
                        action_ledger.finish(&request.request_id, result.clone());
                        let _ = persist_workspace_ledger(&data_dir, action_ledger);
                        result
                    } else {
                        let previous_queue = queue.clone();
                        let coordination_dir = state::coordination_dir();
                        let result = match apply_workspace_action_with_clients_and_coordination(
                            &request,
                            &mut queue,
                            engine,
                            clients,
                            rt,
                            &data_dir,
                            &coordination_dir,
                        ) {
                            Ok(mutated) => {
                                if mutated && queue::write_to(queue_path, &queue).is_err() {
                                    queue = previous_queue;
                                    typed_result(
                                        &request.request_id,
                                        false,
                                        revision,
                                        Some("state_persist_failed"),
                                    )
                                } else {
                                    typed_result(
                                        &request.request_id,
                                        true,
                                        queue::revision(&queue),
                                        None,
                                    )
                                }
                            }
                            Err(error_code) => {
                                typed_result(&request.request_id, false, revision, Some(error_code))
                            }
                        };
                        action_ledger.finish(&request.request_id, result.clone());
                        if let Err(error) = persist_workspace_ledger(&data_dir, action_ledger) {
                            tracing::error!(
                                request_id = %request.request_id,
                                error = %error,
                                "music action ledger finalization failed"
                            );
                        }
                        result
                    }
                }
            }
        };
        let reply = typed_reply(&reply);
        let _ = persist.write(
            &reply_topic(&msg.ulid),
            Priority::Default,
            None,
            Some(&reply),
        );
    }
}

/// Deserialize an already-authorized workspace request without weakening the
/// domain contract's `deny_unknown_fields` boundary. `music_auth` is a wire
/// envelope field verified immediately before this call; it is not part of the
/// retained action and must be removed before decoding `MusicActionRequestV1`.
/// Every other unknown field remains a hard error.
fn parse_authorized_workspace_request(
    body: &str,
) -> Result<MusicActionRequestV1, serde_json::Error> {
    let mut value = serde_json::from_str::<Value>(body)?;
    if let Some(object) = value.as_object_mut() {
        object.remove("music_auth");
    }
    serde_json::from_value(value)
}

/// One browse-poll sweep: for each browse verb, dispatch new requests
/// against the shared Airsonic client. A missing-creds state (`client: None`)
/// replies with an error (the GUI prompts the operator to connect).
///
/// MUSIC-RESPONSIVE-10 — the client is owned by [`serve`] and reused across
/// sweeps (rather than rebuilt here per sweep) so reqwest's keep-alive pool
/// stays warm; the launch-time cold connect happens once at startup, not on
/// the operator's first browse.
pub fn poll_browse(
    persist: &Persist,
    rt: &tokio::runtime::Runtime,
    cursors: &mut HashMap<String, String>,
    client: Option<&Client>,
) {
    let authorizer = music_action_auth::Authorizer::production();
    poll_browse_with_authorizer(persist, rt, cursors, client, &authorizer);
}

fn poll_browse_with_authorizer(
    persist: &Persist,
    rt: &tokio::runtime::Runtime,
    cursors: &mut HashMap<String, String>,
    client: Option<&Client>,
    authorizer: &music_action_auth::Authorizer,
) {
    let clients = client.into_iter().collect::<Vec<_>>();
    poll_browse_with_clients(persist, rt, cursors, &clients, authorizer);
}

/// Read-only catalog lanes may fan out to every configured source. Playlist
/// reads and provider mutations remain on the primary client because their IDs
/// and write authority are not yet source-routed at the public wire boundary;
/// typed playback may explicitly select a retained source variant.
fn multi_source_browse_verb(verb: &str) -> bool {
    matches!(
        verb,
        "list-albums"
            | "list-artists"
            | "search"
            | "list-genres"
            | "albums-by-genre"
            | "list-recents"
            | "list-frequent"
            | "list-starred"
            | "list-bookmarks"
    )
}

fn merge_source_array(target: &mut Vec<Value>, incoming: &[Value], source_id: &str) {
    let mut rows = incoming.to_vec();
    for row in &mut rows {
        if let Some(object) = row.as_object_mut() {
            object
                .entry("source_id")
                .or_insert_with(|| Value::String(source_id.to_string()));
        }
        if target.len() < MAX_COLLECTION_ITEMS && !target.contains(row) {
            target.push(row.clone());
        }
    }
}

fn merge_browse_replies(verb: &str, replies: Vec<(String, String)>) -> String {
    let mut merged: Option<Value> = None;
    let mut first_error = None;
    for (source_id, reply) in replies {
        let Ok(envelope) = serde_json::from_str::<Value>(&reply) else {
            continue;
        };
        if envelope.get("ok") != Some(&Value::Bool(true)) {
            if first_error.is_none() {
                first_error = envelope.get("error").cloned();
            }
            continue;
        }
        let Some(result) = envelope.get("result").cloned() else {
            continue;
        };
        let Some(result_object) = result.as_object() else {
            continue;
        };
        let target = merged.get_or_insert_with(|| json!({}));
        let Some(target_object) = target.as_object_mut() else {
            continue;
        };
        for (key, value) in result_object {
            let Some(incoming) = value.as_array() else {
                target_object
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
                continue;
            };
            let slot = target_object
                .entry(key.clone())
                .or_insert_with(|| Value::Array(Vec::new()));
            if let Some(target_rows) = slot.as_array_mut() {
                merge_source_array(target_rows, incoming, &source_id);
            }
        }
    }
    if let Some(result) = merged {
        return json!({ "ok": true, "result": result }).to_string();
    }
    json!({
        "ok": false,
        "error": first_error.unwrap_or_else(|| Value::String(format!("{verb}: no source available")))
    })
    .to_string()
}

fn poll_browse_with_clients(
    persist: &Persist,
    rt: &tokio::runtime::Runtime,
    cursors: &mut HashMap<String, String>,
    clients: &[&Client],
    authorizer: &music_action_auth::Authorizer,
) {
    for verb in BROWSE_VERBS {
        let topic = format!("action/music/{verb}");
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since_limit(&topic, since, MAX_MESSAGES_PER_POLL) {
            Ok(m) => m,
            Err(_) => continue,
        };
        for msg in msgs {
            cursors.insert(topic.clone(), msg.ulid.clone());
            let body = msg.body.as_deref().unwrap_or("");
            let reply = match authorize_music_mutation(authorizer, verb, body) {
                Err(error) => unauthorized_reply(verb, &error),
                Ok(()) if clients.is_empty() => {
                    json!({ "ok": false, "error": "no Airsonic server configured" }).to_string()
                }
                Ok(()) if multi_source_browse_verb(verb) => {
                    let mut replies = Vec::with_capacity(clients.len());
                    for client in clients {
                        let source_id = catalog_source_id(client);
                        let source_reply = dispatch_browse(verb, body, client, rt);
                        record_catalog_response(
                            &state::data_dir(),
                            client,
                            verb,
                            body,
                            &source_reply,
                        );
                        replies.push((source_id, source_reply));
                    }
                    merge_browse_replies(verb, replies)
                }
                Ok(()) => {
                    let client = clients[0];
                    let reply = dispatch_browse(verb, body, client, rt);
                    record_catalog_response(&state::data_dir(), client, verb, body, &reply);
                    reply
                }
            };
            let _ = persist.write(
                &reply_topic(&msg.ulid),
                Priority::Default,
                None,
                Some(&reply),
            );
        }
    }
}

// ───────────────────────── transport (AIR-2.d) ─────────────────────────

/// A parsed transport request — the pure half of the play flow, decided
/// from the verb + body without touching the engine (so it's
/// unit-testable). [`apply_transport`] runs the side effects.
#[derive(Debug, Clone, PartialEq)]
pub enum TransportCommand {
    /// Play the queue from the current track, gaplessly.
    Play,
    /// Pause (the buffer is preserved; resume is seamless).
    Pause,
    /// Resume after a pause.
    Resume,
    /// Stop + clear the buffer.
    Stop,
    /// Set the volume multiplier (`0.0..=1.0`, clamped by the engine).
    SetVolume(f32),
    /// MUSIC-RFX-2 — seek the current finite track to a position (ms).
    Seek(u64),
    /// Report the current playback state (no side effect).
    GetState,
}

/// Parse an `action/music/<verb>` transport request into a command. The
/// `set-volume` body is a bare number or `{"volume": N}`. `None` for an
/// unknown verb.
#[must_use]
pub fn parse_transport(verb: &str, body: &str) -> Option<TransportCommand> {
    match verb {
        "play" => Some(TransportCommand::Play),
        "pause" => Some(TransportCommand::Pause),
        "resume" => Some(TransportCommand::Resume),
        "stop" => Some(TransportCommand::Stop),
        "get-state" => Some(TransportCommand::GetState),
        "set-volume" => parse_volume(body).map(TransportCommand::SetVolume),
        "seek" => parse_position_ms(body).map(TransportCommand::Seek),
        _ => None,
    }
}

/// MUSIC-RFX-2 — seek target in ms from a bare number (`"42000"`) or
/// `{"position_ms": 42000}` / `{"ms": 42000}`.
fn parse_position_ms(body: &str) -> Option<u64> {
    let trimmed = body.trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(n) = v
            .get("position_ms")
            .or_else(|| v.get("ms"))
            .and_then(serde_json::Value::as_u64)
        {
            return Some(n);
        }
        if let Some(n) = v.as_u64() {
            return Some(n);
        }
    }
    trimmed.parse::<u64>().ok()
}

/// Volume from a bare number (`"0.6"`) or `{"volume": 0.6}`.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)] // serde_json f64 → engine f32
fn parse_volume(body: &str) -> Option<f32> {
    let trimmed = body.trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(n) = v.get("volume").and_then(serde_json::Value::as_f64) {
            return Some(n as f32);
        }
        if let Some(n) = v.as_f64() {
            return Some(n as f32);
        }
    }
    trimmed.parse::<f32>().ok()
}

/// Write this peer's authoritative [`MusicState`] (AIR-8) — the playing
/// peer heartbeats it so the mesh knows who owns playback.
fn write_playback_state(playing: bool, song_id: &str, position_ms: u64) {
    let st = MusicState {
        peer: state::local_host(),
        playing,
        song_id: song_id.to_string(),
        position_ms,
        updated_ms: state::now_ms(),
    };
    let _ = state::write_state(&state::coordination_dir(), &st);
}

/// Build the durable state left by an owner that yielded a playback handoff.
/// Keeping the song and exact position in the same authoritative state file
/// gives the requesting peer a typed resume point without creating a second
/// handoff payload or control plane.
#[must_use]
fn handoff_state(queue: &Queue, peer: &str, position_ms: u64) -> MusicState {
    MusicState {
        peer: peer.to_string(),
        playing: false,
        song_id: queue.current().unwrap_or_default().to_string(),
        position_ms,
        updated_ms: state::now_ms(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum OwnerHandoffAction {
    Yield(state::HandoffIntent),
    AwaitTarget,
    RetireCommitted,
    Reclaim(state::HandoffCompletion),
}

/// Decide the source side of the one-use handoff lease. Once a completion is
/// durable the source must not pause/yield a second time. If the target never
/// commits an honest playing state, expiry returns authority to the source.
fn owner_handoff_action(
    intent: state::HandoffIntent,
    completions: &[state::HandoffCompletion],
    peer_states: &[MusicState],
    now_ms: u64,
) -> OwnerHandoffAction {
    completions
        .iter()
        .find(|completion| completion.intent_id == intent.intent_id)
        .map_or(OwnerHandoffAction::Yield(intent), |completion| {
            let target_committed = peer_states.iter().any(|peer| {
                peer.peer == completion.from_peer
                    && peer.playing
                    && peer.updated_ms >= completion.completed_ms
                    && peer.song_id == completion.song_id
            });
            if target_committed {
                OwnerHandoffAction::RetireCommitted
            } else if now_ms > completion.expires_ms {
                OwnerHandoffAction::Reclaim(completion.clone())
            } else {
                OwnerHandoffAction::AwaitTarget
            }
        })
}

/// Admit only the exact queue carried by a fresh, target-owned one-use
/// completion. Callers must re-run this immediately before starting audio.
fn admitted_handoff_queue(
    completion: &state::HandoffCompletion,
    intents: &[state::HandoffIntent],
    target_peer: &str,
    now_ms: u64,
) -> Option<Queue> {
    state::completion_matches_intent(completion, intents, target_peer, now_ms)
        .then(|| completion.queue.clone())
}

/// Yield the local engine to the newest admitted takeover intent. The intent
/// remains beside the durable completion until the requester consumes both;
/// that binding prevents a stale or spoofed completion from authorizing a
/// target-side resume. If no local engine exists the request remains pending
/// for a reachable owner instead of claiming a handoff that did not happen.
fn apply_pending_handoff(engine: Option<&Engine>, queue_path: &Path) {
    let Some(engine) = engine else { return };
    let dir = state::coordination_dir();
    let my_host = state::local_host();
    let intents = state::read_intents(&dir);
    let Some(intent) = state::pending_takeover_for(&intents, &my_host) else {
        return;
    };
    let completions = state::read_completions(&dir);
    let peer_states = state::read_all_peer_states(&dir);
    match owner_handoff_action(intent.clone(), &completions, &peer_states, state::now_ms()) {
        OwnerHandoffAction::AwaitTarget => return,
        OwnerHandoffAction::RetireCommitted => {
            state::clear_intent(&dir, &intent.intent_id);
            state::clear_completion(&dir, &intent.intent_id);
            return;
        }
        OwnerHandoffAction::Reclaim(completion) => {
            let queue = queue::read_from(queue_path);
            if queue != completion.queue || !engine.is_active() {
                state::clear_intent(&dir, &completion.intent_id);
                state::clear_completion(&dir, &completion.intent_id);
                let _ = state::write_state(
                    &dir,
                    &handoff_state(&queue, &my_host, engine.position_ms()),
                );
                tracing::warn!(
                    intent_id = %completion.intent_id,
                    "expired music handoff was retired without resume because source identity changed"
                );
                return;
            }
            let position_ms = completion.position_ms;
            let reclaimed = MusicState {
                peer: my_host,
                playing: true,
                song_id: queue.current().unwrap_or_default().to_string(),
                position_ms,
                updated_ms: state::now_ms(),
            };
            if let Err(error) = state::write_state(&dir, &reclaimed) {
                tracing::warn!(%error, intent_id = %completion.intent_id, "expired music handoff could not restore source authority; retaining lease for retry");
                return;
            }
            state::clear_intent(&dir, &completion.intent_id);
            state::clear_completion(&dir, &completion.intent_id);
            engine.resume();
            tracing::warn!(
                intent_id = %completion.intent_id,
                song_id = %reclaimed.song_id,
                position_ms,
                "music handoff target did not commit; source reclaimed sole playback authority"
            );
            return;
        }
        OwnerHandoffAction::Yield(_) if !engine.is_playing() => return,
        OwnerHandoffAction::Yield(_) => {}
    }
    let queue = queue::read_from(queue_path);
    let position_ms = engine.position_ms();
    engine.pause();
    let snapshot = handoff_state(&queue, &my_host, position_ms);
    if let Err(error) = state::write_state(&dir, &snapshot) {
        engine.resume();
        tracing::warn!(%error, intent_id = %intent.intent_id, "music handoff state could not be persisted; retaining intent");
        return;
    }
    let completion = state::HandoffCompletion {
        intent_id: intent.intent_id.clone(),
        from_peer: intent.from_peer.clone(),
        owner_peer: my_host.clone(),
        song_id: snapshot.song_id.clone(),
        queue: queue.clone(),
        position_ms: snapshot.position_ms,
        completed_ms: snapshot.updated_ms,
        expires_ms: snapshot
            .updated_ms
            .saturating_add(state::HANDOFF_ACK_TIMEOUT_MS),
    };
    if let Err(error) = state::write_completion(&dir, &completion) {
        let restored = MusicState {
            peer: my_host,
            playing: true,
            song_id: snapshot.song_id,
            position_ms: snapshot.position_ms,
            updated_ms: state::now_ms(),
        };
        if state::write_state(&dir, &restored).is_ok() {
            engine.resume();
        }
        tracing::warn!(%error, intent_id = %intent.intent_id, "music handoff completion could not be persisted; retaining intent");
        return;
    }
    tracing::info!(
        intent_id = %intent.intent_id,
        from_peer = %intent.from_peer,
        song_id = %snapshot.song_id,
        position_ms = snapshot.position_ms,
        "music playback yielded to takeover request"
    );
}

/// Consume a durable owner-yield completion on the requesting peer. The
/// target reuses the daemon's existing queue/source-selection authority and
/// asks the native engine to seek before its first decoded packet. A completion
/// remains available until a playback start has been admitted; it is then
/// cleared with its request. The request binding rejects stale/replayed or
/// unauthorized completion files before they can alter the queue or engine.
fn apply_handoff_completions(engine: Option<&Engine>, queue_path: &Path, clients: &[&Client]) {
    let Some(engine) = engine else { return };
    let dir = state::coordination_dir();
    let my_host = state::local_host();
    let intents = state::read_intents(&dir);
    let now_ms = state::now_ms();
    for completion in state::read_completions(&dir) {
        if !state::completion_matches_intent(&completion, &intents, &my_host, now_ms) {
            tracing::warn!(
                intent_id = %completion.intent_id,
                from_peer = %completion.from_peer,
                owner_peer = %completion.owner_peer,
                "music handoff completion has no matching pending intent; dropping it"
            );
            state::clear_completion(&dir, &completion.intent_id);
            continue;
        }
        let previous_queue = queue::read_from(queue_path);
        let Some(queue) = admitted_handoff_queue(&completion, &intents, &my_host, now_ms) else {
            state::clear_completion(&dir, &completion.intent_id);
            continue;
        };
        if queue::write_to(queue_path, &queue).is_err() {
            tracing::warn!(intent_id = %completion.intent_id, "music handoff queue could not be persisted; retaining completion");
            continue;
        }
        let upcoming = if clients.is_empty() {
            cached_upcoming_tracks(&queue, &crate::cache::cache_dir()).map(|tracks| {
                tracks
                    .into_iter()
                    .map(|(url, codec)| PlaybackTrack::single(url, codec))
                    .collect::<Vec<_>>()
            })
        } else {
            let catalog = load_catalog(&state::data_dir()).unwrap_or_default();
            source_aware_upcoming_candidates(&queue, clients, &catalog)
        };
        let Some(upcoming) = upcoming.filter(|tracks| !tracks.is_empty()) else {
            let _ = queue::write_to(queue_path, &previous_queue);
            tracing::warn!(intent_id = %completion.intent_id, "music handoff target has no admitted playback source; retaining completion");
            continue;
        };
        // Revalidate the one-use lease immediately before acquiring audible
        // authority. This closes the potentially slow source-resolution window
        // and makes an expired or source-reclaimed transfer fail closed.
        let current_intents = state::read_intents(&dir);
        if admitted_handoff_queue(&completion, &current_intents, &my_host, state::now_ms()).as_ref()
            != Some(&queue)
        {
            let _ = queue::write_to(queue_path, &previous_queue);
            state::clear_completion(&dir, &completion.intent_id);
            continue;
        }
        if !engine.play_from_candidates_at(upcoming, queue.current, completion.position_ms) {
            let _ = queue::write_to(queue_path, &previous_queue);
            tracing::warn!(intent_id = %completion.intent_id, "music handoff target could not start the native engine; retaining completion");
            continue;
        }
        let target_state = MusicState {
            peer: my_host.clone(),
            playing: true,
            song_id: completion.song_id.clone(),
            position_ms: completion.position_ms,
            updated_ms: state::now_ms(),
        };
        if let Err(error) = state::write_state(&dir, &target_state) {
            engine.pause();
            let _ = queue::write_to(queue_path, &previous_queue);
            tracing::warn!(%error, intent_id = %completion.intent_id, "music handoff target state could not be persisted; audible authority revoked and completion retained");
            continue;
        }
        // Retire the authorization record first. If cleanup is interrupted
        // after this point, the completion is no longer eligible for replay.
        state::clear_intent(&dir, &completion.intent_id);
        state::clear_completion(&dir, &completion.intent_id);
        tracing::info!(
            intent_id = %completion.intent_id,
            owner_peer = %completion.owner_peer,
            song_id = %completion.song_id,
            position_ms = completion.position_ms,
            "music playback resumed from owner-yield completion"
        );
    }
}

/// Project only targets this daemon can prove through local audio and bounded
/// peer-heartbeat state. Remote seats with a fresh idle heartbeat are
/// actionable; stale or currently-owning peers remain visible with an honest
/// refusal reason instead of becoming fabricated renderers.
fn playback_targets(audio_available: bool, data_dir: &Path) -> Vec<crate::domain::PlaybackTarget> {
    let local_peer = state::local_host();
    let now = state::now_ms();
    let mut targets = local_playback_targets(audio_available);
    for peer in state::read_all_peer_states(data_dir)
        .into_iter()
        .filter(|peer| peer.peer != local_peer)
    {
        let stale = now.saturating_sub(peer.updated_ms) > state::STATE_STALE_MS;
        let unavailable_reason = if stale {
            Some("peer heartbeat is stale".to_owned())
        } else if peer.playing {
            Some("peer currently owns playback".to_owned())
        } else {
            None
        };
        targets.push(crate::domain::PlaybackTarget {
            id: peer.peer.clone(),
            name: peer.peer,
            kind: "mesh_seat".to_owned(),
            available: unavailable_reason.is_none(),
            unavailable_reason,
        });
    }
    targets.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    targets.truncate(MAX_SOURCE_RECORDS);
    targets
}

/// Project only the local target. Remote seats enter through
/// [`playback_targets`] after their bounded peer heartbeat is validated.
fn local_playback_targets(audio_available: bool) -> Vec<crate::domain::PlaybackTarget> {
    if !audio_available {
        return Vec::new();
    }
    vec![crate::domain::PlaybackTarget {
        id: format!("local-seat:{}", state::local_host()),
        name: state::local_host(),
        kind: "local_seat".to_string(),
        available: true,
        unavailable_reason: None,
    }]
}

/// Resolve the queued tail entirely from the finite audio cache. Returning
/// `None` for any missing entry is deliberate: offline play must not start a
/// partial album and then fail after the source has already been lost.
fn cached_upcoming_tracks(
    queue: &crate::queue::Queue,
    cache_dir: &Path,
) -> Option<Vec<(String, SourceCodec)>> {
    let tracks: Option<Vec<(String, SourceCodec)>> = queue
        .songs
        .iter()
        .skip(queue.current)
        .map(|song_id| {
            let suffix = crate::cache::cached_track_suffix(cache_dir, song_id)?;
            Some((
                crate::engine::cached_stream_url(song_id),
                SourceCodec::from_suffix(&suffix),
            ))
        })
        .collect();
    let tracks = tracks?;
    (!tracks.is_empty()).then_some(tracks)
}

/// Select one admitted client for each queued song from the retained source
/// variants. The fallback to the first client preserves legacy queues whose
/// catalog has not been projected yet; once a variant is known, the domain
/// policy orders cache/reachability/priority/latency and the selected source
/// identity is resolved back to the live client without exposing credentials.
#[cfg(test)]
fn source_aware_upcoming_tracks(
    queue: &crate::queue::Queue,
    clients: &[&Client],
    catalog: &CatalogStoreFile,
) -> Option<Vec<(String, SourceCodec)>> {
    source_aware_upcoming_candidates(queue, clients, catalog).map(|tracks| {
        tracks
            .into_iter()
            .filter_map(|track| track.candidates.into_iter().next())
            .collect()
    })
}

/// Resolve every logical queue entry to the ordered admitted sources that can
/// serve it. Keeping candidates grouped by queue entry lets the engine retry a
/// failed source without manufacturing an extra audible queue track.
fn source_aware_upcoming_candidates(
    queue: &crate::queue::Queue,
    clients: &[&Client],
    catalog: &CatalogStoreFile,
) -> Option<Vec<PlaybackTrack>> {
    let fallback = clients.first().copied()?;
    let tracks: Vec<PlaybackTrack> = queue
        .songs
        .iter()
        .skip(queue.current)
        .map(|song_id| {
            let retained_item = catalog
                .songs
                .iter()
                .chain(catalog.episodes.iter())
                .chain(catalog.radio.iter())
                .find(|item| {
                    matches!(
                        item.kind,
                        ContentKind::Music
                            | ContentKind::Episode
                            | ContentKind::Chapter
                            | ContentKind::Audiobook
                            | ContentKind::Radio
                    ) && item
                        .variants
                        .iter()
                        .any(|variant| variant.content.remote_id == *song_id)
                });
            let candidates = if let Some(item) = retained_item {
                let candidates = ordered_variants(&item.variants)
                    .into_iter()
                    .filter_map(|variant| {
                        clients
                            .iter()
                            .copied()
                            .find(|client| catalog_source_id(client) == variant.content.source_id)
                            .and_then(|client| {
                                let url = if variant.content.kind == ContentKind::Radio {
                                    direct_radio_stream_url(&variant.content)?.to_string()
                                } else {
                                    client.stream_url(&variant.content.remote_id)
                                };
                                Some((url, SourceCodec::Unknown))
                            })
                    })
                    .collect::<Vec<_>>();
                if item.kind != ContentKind::Radio && candidates.is_empty() {
                    vec![(fallback.stream_url(song_id), SourceCodec::Unknown)]
                } else {
                    candidates
                }
            } else {
                let candidates = catalog
                    .bookmarks
                    .iter()
                    .filter(|bookmark| bookmark.content.remote_id == *song_id)
                    .filter_map(|bookmark| {
                        clients
                            .iter()
                            .copied()
                            .find(|client| catalog_source_id(client) == bookmark.content.source_id)
                            .map(|client| {
                                (
                                    client.stream_url(&bookmark.content.remote_id),
                                    SourceCodec::Unknown,
                                )
                            })
                    })
                    .collect::<Vec<_>>();
                if candidates.is_empty() {
                    vec![(fallback.stream_url(song_id), SourceCodec::Unknown)]
                } else {
                    candidates
                }
            };
            PlaybackTrack { candidates }
        })
        .collect();
    (!tracks.is_empty() && tracks.iter().all(|track| !track.candidates.is_empty()))
        .then_some(tracks)
}

/// Accept an Internet-radio locator only as the exact HTTP(S) URL retained in
/// its typed catalog variant. This check lives in the responder so radio can
/// never fall through the song-id `/rest/stream` construction path.
fn direct_radio_stream_url(content: &ContentRef) -> Option<&str> {
    if content.kind != ContentKind::Radio {
        return None;
    }
    let parsed = reqwest::Url::parse(&content.remote_id).ok()?;
    (matches!(parsed.scheme(), "http" | "https") && parsed.host_str().is_some())
        .then_some(content.remote_id.as_str())
}

fn retained_radio_stream_url<'a>(
    catalog: &CatalogStoreFile,
    selected: &'a ContentRef,
) -> Result<&'a str, &'static str> {
    if selected.kind != ContentKind::Radio
        || !catalog.radio.iter().any(|item| {
            item.kind == ContentKind::Radio
                && item
                    .variants
                    .iter()
                    .any(|variant| variant.content == *selected)
        })
    {
        return Err("unsupported_source");
    }
    direct_radio_stream_url(selected).ok_or("invalid_stream_url")
}

fn typed_play_start_position(
    content: &ContentRef,
    requested_position_ms: Option<u64>,
) -> Result<u64, &'static str> {
    let position_ms = requested_position_ms.unwrap_or(0);
    if content.kind == ContentKind::Radio && position_ms != 0 {
        return Err("not_seekable");
    }
    Ok(position_ms)
}

/// Resolve a typed workspace play request while honoring the source variant
/// selected by the caller.  The rest of the queue still uses the normal
/// bounded source policy, but the first logical track is pinned to the
/// requested admitted source (or its finite local cache when that source is
/// currently unavailable).  This keeps source selection inside the daemon's
/// existing queue/engine path instead of reintroducing GUI-owned provider
/// playback.
fn selected_source_upcoming_candidates(
    queue: &crate::queue::Queue,
    selected: &ContentRef,
    clients: &[&Client],
    catalog: &CatalogStoreFile,
    cache_dir: &Path,
) -> Result<Vec<PlaybackTrack>, &'static str> {
    let retained_radio_url = if selected.kind == ContentKind::Radio {
        Some(retained_radio_stream_url(catalog, selected)?)
    } else {
        None
    };
    let cached_variant = catalog
        .songs
        .iter()
        .chain(catalog.episodes.iter())
        .chain(catalog.radio.iter())
        .find_map(|item| {
            (matches!(
                item.kind,
                ContentKind::Music
                    | ContentKind::Episode
                    | ContentKind::Chapter
                    | ContentKind::Audiobook
                    | ContentKind::Radio
            ))
            .then(|| {
                item.variants
                    .iter()
                    .find(|variant| variant.content == *selected)
                    .map(|variant| variant.cached)
            })
            .flatten()
        });
    let bookmark_admitted = catalog
        .bookmarks
        .iter()
        .any(|bookmark| bookmark.content == *selected);
    if cached_variant.is_none() && !bookmark_admitted {
        return Err("unsupported_source");
    }

    let mut tracks = if clients.is_empty() {
        cached_upcoming_tracks(queue, cache_dir)
            .map(|tracks| {
                tracks
                    .into_iter()
                    .map(|(url, codec)| PlaybackTrack::single(url, codec))
                    .collect::<Vec<_>>()
            })
            .ok_or("source_unavailable")?
    } else {
        source_aware_upcoming_candidates(queue, clients, catalog).ok_or("source_unavailable")?
    };
    let first = tracks.first_mut().ok_or("queue_empty")?;

    if let Some(client) = clients
        .iter()
        .copied()
        .find(|client| catalog_source_id(client) == selected.source_id)
    {
        let selected_candidate = (
            retained_radio_url
                .map_or_else(|| client.stream_url(&selected.remote_id), ToOwned::to_owned),
            SourceCodec::Unknown,
        );
        first
            .candidates
            .retain(|candidate| candidate.0 != selected_candidate.0);
        first.candidates.insert(0, selected_candidate);
        return Ok(tracks);
    }

    if selected.kind != ContentKind::Radio
        && (cached_variant == Some(true)
            || crate::cache::cached_track_suffix(cache_dir, &selected.remote_id).is_some())
    {
        let suffix = crate::cache::cached_track_suffix(cache_dir, &selected.remote_id)
            .ok_or("source_unavailable")?;
        let selected_candidate = (
            crate::engine::cached_stream_url(&selected.remote_id),
            SourceCodec::from_suffix(&suffix),
        );
        first
            .candidates
            .retain(|candidate| candidate.0 != selected_candidate.0);
        first.candidates.insert(0, selected_candidate);
        return Ok(tracks);
    }

    Err("source_unavailable")
}

/// Return true only for a source/content identity retained by the daemon's
/// bounded catalog projection. Typed curation must not turn an arbitrary
/// source id into a provider mutation just because a client happens to be
/// configured for a similar URL.
fn catalog_contains_variant(catalog: &CatalogStoreFile, selected: &ContentRef) -> bool {
    let collections = [
        &catalog.albums,
        &catalog.artists,
        &catalog.podcasts,
        &catalog.radio,
        &catalog.episodes,
        &catalog.songs,
        &catalog.starred,
        &catalog.recent,
        &catalog.frequent,
    ];
    collections.iter().any(|items| {
        items.iter().any(|item| {
            item.variants
                .iter()
                .any(|variant| variant.content == *selected)
        })
    }) || catalog.search.as_ref().is_some_and(|page| {
        page.groups.values().any(|items| {
            items.iter().any(|item| {
                item.variants
                    .iter()
                    .any(|variant| variant.content == *selected)
            })
        })
    }) || catalog
        .bookmarks
        .iter()
        .any(|bookmark| bookmark.content == *selected)
}

/// Compatibility wrapper for callers/tests that have only the primary client.
#[cfg(test)]
fn apply_transport(
    verb: &str,
    body: &str,
    engine: Option<&Engine>,
    client: Option<&Client>,
    queue: &Queue,
) -> String {
    let clients = client.into_iter().collect::<Vec<_>>();
    apply_transport_with_clients(verb, body, engine, &clients, queue)
}

/// Apply one transport request to the engine + queue, returning the reply
/// JSON. Side effects (engine + the AIR-8 state write); the pure
/// verb→command parse is [`parse_transport`].
fn apply_transport_with_clients(
    verb: &str,
    body: &str,
    engine: Option<&Engine>,
    clients: &[&Client],
    queue: &Queue,
) -> String {
    let Some(cmd) = parse_transport(verb, body) else {
        return json!({ "ok": false, "error": format!("unknown transport verb: {verb}") })
            .to_string();
    };
    let no_audio =
        || json!({ "ok": false, "error": "no audio output device on this peer" }).to_string();
    match cmd {
        // AUDIT-MESH-4: get-state is answered unconditionally so the Music
        // panel can render an honest idle / needs-audio / needs-Airsonic state
        // even on a headless peer (no audio device) or before Airsonic creds
        // are configured. `audio_available` / `needs_airsonic` let the panel
        // tell those apart instead of silently looking "idle". The mutating
        // verbs below still require a live engine.
        TransportCommand::GetState => json!({
            "ok": true,
            "playing": engine.map_or(false, |e| e.is_playing()),
            "active": engine.map_or(false, |e| e.is_active()),
            "position_ms": engine.map_or(0, |e| e.position_ms()),
            "volume": engine.map_or(1.0_f32, |e| e.volume()),
            "song_id": queue.current(),
            "audio_available": engine.is_some(),
            "needs_airsonic": clients.is_empty(),
            // MUSIC-RFX-2 — the GUI shows the scrubber only for a seekable track.
            "seekable": engine.map_or(false, |e| e.is_seekable()),
        })
        .to_string(),
        TransportCommand::Play => {
            let Some(engine) = engine else {
                return no_audio();
            };
            // Gapless album: hand the engine current..end in one list. The
            // base cursor lets the AIR-2.c auto-advance driver map the audible
            // track back to the right queue index as playback crosses gapless
            // boundaries.
            let (upcoming, offline) = if !clients.is_empty() {
                let catalog = load_catalog(&state::data_dir()).unwrap_or_default();
                let Some(upcoming) = source_aware_upcoming_candidates(queue, clients, &catalog)
                else {
                    return json!({ "ok": false, "error": "queue is empty" }).to_string();
                };
                (upcoming, false)
            } else {
                let Some(upcoming) = cached_upcoming_tracks(queue, &crate::cache::cache_dir())
                else {
                    return json!({
                        "ok": false,
                        "error": "no Airsonic server configured and queued tracks are not fully cached"
                    })
                    .to_string();
                };
                (
                    upcoming
                        .into_iter()
                        .map(|(url, codec)| PlaybackTrack::single(url, codec))
                        .collect(),
                    true,
                )
            };
            if upcoming.is_empty() {
                return json!({ "ok": false, "error": "queue is empty" }).to_string();
            }
            if offline {
                engine.play_from(
                    upcoming
                        .into_iter()
                        .filter_map(|track| track.candidates.into_iter().next())
                        .collect(),
                    queue.current,
                );
            } else {
                engine.play_from_candidates(upcoming, queue.current);
            }
            let song = queue.current().unwrap_or("");
            write_playback_state(true, song, 0);
            json!({ "ok": true, "playing": true, "offline": offline, "song_id": song }).to_string()
        }
        TransportCommand::Pause => {
            let Some(engine) = engine else {
                return no_audio();
            };
            engine.pause();
            write_playback_state(false, queue.current().unwrap_or(""), engine.position_ms());
            json!({ "ok": true, "playing": false }).to_string()
        }
        TransportCommand::Resume => {
            let Some(engine) = engine else {
                return no_audio();
            };
            let Some(position_ms) = resumable_position(engine.is_active(), engine.position_ms())
            else {
                return json!({
                    "ok": false,
                    "error": "nothing_to_resume",
                    "playing": false,
                    "position_ms": engine.position_ms(),
                })
                .to_string();
            };
            engine.resume();
            write_playback_state(true, queue.current().unwrap_or(""), position_ms);
            json!({
                "ok": true,
                "playing": true,
                "position_ms": position_ms,
            })
            .to_string()
        }
        TransportCommand::Stop => {
            let Some(engine) = engine else {
                return no_audio();
            };
            engine.stop();
            write_playback_state(false, "", 0);
            json!({ "ok": true, "playing": false }).to_string()
        }
        TransportCommand::SetVolume(v) => {
            let Some(engine) = engine else {
                return no_audio();
            };
            engine.set_volume(v);
            json!({ "ok": true, "volume": engine.volume() }).to_string()
        }
        TransportCommand::Seek(target_ms) => {
            let Some(engine) = engine else {
                return no_audio();
            };
            // No-op for a live/radio stream (engine.seek returns false); the GUI
            // hides the scrubber off get-state's `seekable`, so this is defensive.
            let accepted = engine.seek(target_ms);
            let reported_position_ms = if accepted {
                target_ms
            } else {
                engine.position_ms()
            };
            write_playback_state(
                engine.is_playing(),
                queue.current().unwrap_or(""),
                reported_position_ms,
            );
            seek_transport_reply(accepted, reported_position_ms)
        }
    }
}

fn resumable_position(active: bool, position_ms: u64) -> Option<u64> {
    active.then_some(position_ms)
}

fn seek_transport_reply(accepted: bool, position_ms: u64) -> String {
    if accepted {
        json!({
            "ok": true,
            "seeked": true,
            "position_ms": position_ms,
        })
        .to_string()
    } else {
        json!({
            "ok": false,
            "error": "not_seekable",
            "seeked": false,
            "position_ms": position_ms,
        })
        .to_string()
    }
}

struct MusicClockAudioEffects<'a> {
    music_engine: Option<&'a Engine>,
    alert_engine: Option<&'a Engine>,
    clients: &'a [&'a Client],
    catalog: &'a CatalogStoreFile,
    cache_dir: &'a Path,
    data_dir: &'a Path,
    seat_audio: &'a mut SeatAudioAuthority,
}

impl ClockAudioEffects for MusicClockAudioEffects<'_> {
    fn duck_seat_streams(&mut self, request: &ClockAudioRequestV1) -> Result<(), &'static str> {
        self.seat_audio
            .duck(SeatDuckGeneration {
                occurrence_id: request.occurrence_id.clone(),
                global_event_id: request.global_event_id.clone(),
                generation: request.occurrence_generation,
            })
            .map_err(|error| error.reason_code())
    }

    fn restore_seat_streams(&mut self) -> Result<(), &'static str> {
        self.seat_audio
            .restore()
            .map_err(|error| error.reason_code())
    }

    fn music_volume(&self) -> Option<f32> {
        self.music_engine
            .filter(|engine| engine.is_active())
            .map(|engine| engine.volume())
    }

    fn set_music_volume(&mut self, volume: f32) {
        if let Some(engine) = self.music_engine {
            engine.set_volume(volume);
        }
    }

    fn set_alert_volume(&mut self, volume: f32) {
        if let Some(engine) = self.alert_engine {
            engine.set_volume(volume);
        }
    }

    fn start_bundled(&mut self, tone_id: &str) -> Result<(), &'static str> {
        let engine = self.alert_engine.ok_or("audio_output_unavailable")?;
        engine
            .play_bundled_clock_tone(tone_id)
            .then_some(())
            .ok_or("bundled_tone_unavailable")
    }

    fn start_music(&mut self, audio: &ClockAudioRef) -> Result<(), &'static str> {
        let engine = self.alert_engine.ok_or("audio_output_unavailable")?;
        let tracks = governed_clock_playback_tracks(
            self.data_dir,
            self.catalog,
            audio,
            self.clients,
            self.cache_dir,
        )?;
        engine
            .play_from_candidates_at(tracks, 0, 0)
            .then_some(())
            .ok_or("audio_output_unavailable")
    }

    fn resolve_audio(&self, audio: &ClockAudioRef) -> Result<(), &'static str> {
        governed_clock_playback_tracks(
            self.data_dir,
            self.catalog,
            audio,
            self.clients,
            self.cache_dir,
        )
        .map(|_| ())
    }

    fn preview_audio(&mut self, audio: &ClockAudioRef) -> Result<(), &'static str> {
        let engine = self.alert_engine.ok_or("audio_output_unavailable")?;
        let tracks = governed_clock_playback_tracks(
            self.data_dir,
            self.catalog,
            audio,
            self.clients,
            self.cache_dir,
        )?;
        engine
            .play_from_candidates_at(tracks, 0, 0)
            .then_some(())
            .ok_or("audio_output_unavailable")
    }

    fn music_is_audible(&self) -> bool {
        self.alert_engine.is_some_and(|engine| {
            engine.is_renderer_healthy() && engine.is_playing() && engine.position_ms() > 0
        })
    }

    fn stop_alert(&mut self) {
        if let Some(engine) = self.alert_engine {
            engine.stop();
        }
    }

    fn revoke_alert(&mut self) {
        if let Some(engine) = self.alert_engine {
            engine.revoke_renderer();
        }
    }
}

fn governed_clock_playback_tracks(
    data_dir: &Path,
    catalog: &CatalogStoreFile,
    audio: &ClockAudioRef,
    clients: &[&Client],
    cache_dir: &Path,
) -> Result<Vec<PlaybackTrack>, &'static str> {
    if let Some(track) = governed_clock_local_track(data_dir, catalog, audio)? {
        return Ok(vec![track]);
    }
    let selected = governed_clock_content(catalog, audio)?;
    let queue = Queue {
        songs: vec![selected.remote_id.clone()],
        current: 0,
    };
    selected_source_upcoming_candidates(&queue, &selected, clients, catalog, cache_dir)
}

fn governed_clock_local_track(
    data_dir: &Path,
    catalog: &CatalogStoreFile,
    audio: &ClockAudioRef,
) -> Result<Option<PlaybackTrack>, &'static str> {
    let ClockAudioRef::Music {
        source_id,
        remote_id,
        content_kind,
        ..
    } = audio
    else {
        return Err("invalid_music_reference");
    };
    if source_id != CLOCK_LOCAL_FILE_SOURCE_ID {
        return Ok(None);
    }
    if *content_kind != ClockMusicKind::Track || !valid_clock_local_id(remote_id) {
        return Err("local_file_reference_malformed");
    }
    let admission = catalog
        .clock_local_files
        .get(remote_id)
        .ok_or("local_file_reference_missing")?;
    if !valid_clock_local_admissions(&BTreeMap::from([(remote_id.clone(), admission.clone())])) {
        return Err("local_file_reference_unauthorized");
    }
    let root = data_dir.join(CLOCK_LOCAL_FILE_DIRECTORY);
    let canonical_root =
        std::fs::canonicalize(&root).map_err(|_| "local_file_reference_unavailable")?;
    let candidate = root.join(&admission.relative_path);
    let link_metadata =
        std::fs::symlink_metadata(&candidate).map_err(|_| "local_file_reference_missing")?;
    if link_metadata.file_type().is_symlink() || !link_metadata.is_file() {
        return Err("local_file_reference_unauthorized");
    }
    let canonical_candidate =
        std::fs::canonicalize(&candidate).map_err(|_| "local_file_reference_missing")?;
    if !canonical_candidate.starts_with(&canonical_root) {
        return Err("local_file_reference_unauthorized");
    }
    let metadata =
        std::fs::metadata(&canonical_candidate).map_err(|_| "local_file_reference_missing")?;
    if metadata.len() != admission.byte_len
        || modified_at_utc_ms(&metadata).ok() != Some(admission.modified_at_utc_ms)
        || clock_file_sha256(&canonical_candidate).ok().as_deref()
            != Some(admission.sha256.as_str())
    {
        return Err("local_file_reference_stale");
    }
    let codec = SourceCodec::from_suffix(&admission.relative_path);
    let locator = crate::engine::local_file_stream_url(&canonical_candidate)
        .ok_or("local_file_reference_unauthorized")?;
    Ok(Some(PlaybackTrack::single(locator, codec)))
}

fn governed_clock_content(
    catalog: &CatalogStoreFile,
    audio: &ClockAudioRef,
) -> Result<ContentRef, &'static str> {
    governed_clock_content_at(catalog, audio, state::now_ms())
}

fn governed_clock_content_at(
    catalog: &CatalogStoreFile,
    audio: &ClockAudioRef,
    now_ms: u64,
) -> Result<ContentRef, &'static str> {
    let ClockAudioRef::Music {
        source_id,
        remote_id,
        content_kind,
        ..
    } = audio
    else {
        return Err("invalid_music_reference");
    };
    let expected = match content_kind {
        ClockMusicKind::Track => ContentKind::Music,
        ClockMusicKind::PodcastEpisode => ContentKind::Episode,
        ClockMusicKind::Radio => ContentKind::Radio,
    };

    // News Now is a catalog-owned dynamic alias: Clock retains NPR's stable
    // feed identity while Music resolves the newest admitted hourly episode at
    // ring time. The resolved provider identity never crosses back into Clock.
    if source_id == CLOCK_CATALOG_SOURCE_ID
        && remote_id == NPR_NEWS_NOW_PRESET_ID
        && expected == ContentKind::Episode
    {
        let preset = catalog
            .clock_presets
            .get(NPR_NEWS_NOW_PRESET_ID)
            .ok_or("catalog_reference_missing")?;
        if preset.admitted_at_utc_ms > now_ms
            || now_ms.saturating_sub(preset.admitted_at_utc_ms) > NPR_NEWS_NOW_MAX_AGE_MS
        {
            return Err("catalog_reference_stale");
        }
        return admitted_clock_content(catalog, &preset.content);
    }

    // A configured NPR member station remains a distinct Radio catalog item.
    // Clock stores the stable item identity, not the station's provider URL.
    if source_id == CLOCK_CATALOG_SOURCE_ID && expected == ContentKind::Radio {
        let selected = catalog
            .radio
            .iter()
            .find(|item| item.kind == ContentKind::Radio && item.id == *remote_id)
            .and_then(|item| ordered_variants(&item.variants).into_iter().next())
            .map(|variant| variant.content.clone())
            .ok_or("catalog_reference_missing")?;
        return admitted_clock_content(catalog, &selected);
    }

    let selected = catalog
        .songs
        .iter()
        .chain(catalog.episodes.iter())
        .chain(catalog.radio.iter())
        .filter(|item| item.kind == expected)
        .flat_map(|item| {
            item.variants.iter().filter(move |variant| {
                variant.content.source_id == *source_id
                    && (variant.content.remote_id == *remote_id || item.id == *remote_id)
            })
        })
        .map(|variant| variant.content.clone())
        .next()
        .ok_or("catalog_reference_missing")?;
    admitted_clock_content(catalog, &selected)
}

fn admitted_clock_content(
    catalog: &CatalogStoreFile,
    selected: &ContentRef,
) -> Result<ContentRef, &'static str> {
    let source = catalog
        .sources
        .iter()
        .find(|source| source.source_id == selected.source_id)
        .ok_or("catalog_source_unauthorized")?;
    if source.authentication_required {
        return Err("catalog_source_unauthorized");
    }
    if !source.reachable {
        return Err("catalog_source_unreachable");
    }
    let retained = catalog
        .songs
        .iter()
        .chain(catalog.episodes.iter())
        .chain(catalog.radio.iter())
        .filter(|item| item.kind == selected.kind)
        .flat_map(|item| item.variants.iter())
        .any(|variant| variant.content == *selected && (variant.reachable || variant.cached));
    retained
        .then(|| selected.clone())
        .ok_or("catalog_reference_missing")
}

fn poll_clock_audio_with_authorizer(
    persist: &Persist,
    cursors: &mut HashMap<String, String>,
    authority: &mut ClockAudioAuthority,
    seat_audio: &mut SeatAudioAuthority,
    music_engine: Option<&Engine>,
    alert_engine: Option<&Engine>,
    clients: &[&Client],
    authorizer: &music_action_auth::Authorizer,
) {
    let since = cursors.get(CLOCK_AUDIO_ACTION_TOPIC).map(String::as_str);
    let messages =
        match persist.list_since_limit(CLOCK_AUDIO_ACTION_TOPIC, since, MAX_MESSAGES_PER_POLL) {
            Ok(messages) => messages,
            Err(_) => return,
        };
    let node = state::local_host();
    let data_dir = state::data_dir();
    let catalog = load_catalog(&data_dir).unwrap_or_default();
    let cache_dir = crate::cache::cache_dir();
    for message in messages {
        cursors.insert(CLOCK_AUDIO_ACTION_TOPIC.to_string(), message.ulid.clone());
        let body = message.body.as_deref().unwrap_or("");
        let reply = match authorizer.authorize(body, "music-clock-audio", &node, "clock-audio") {
            Err(error) => unauthorized_reply(CLOCK_AUDIO_VERB, &error),
            Ok(()) => {
                let now_ms = i64::try_from(state::now_ms()).unwrap_or(i64::MAX);
                match ClockAudioRequestV1::from_json_at(body.as_bytes(), now_ms) {
                    Err(error) => json!({
                        "ok": false,
                        "error": format!("clock-audio: invalid request: {error:?}")
                    })
                    .to_string(),
                    Ok(request) => {
                        let mut effects = MusicClockAudioEffects {
                            music_engine,
                            alert_engine,
                            clients,
                            catalog: &catalog,
                            cache_dir: &cache_dir,
                            data_dir: &data_dir,
                            seat_audio,
                        };
                        let status = authority.apply(request, now_ms, &mut effects);
                        publish_clock_audio_status(persist, &node, &status);
                        serde_json::to_string(&status).unwrap_or_else(|_| {
                            r#"{"ok":false,"error":"clock-audio: reply_failed"}"#.to_string()
                        })
                    }
                }
            }
        };
        let _ = persist.write(
            &reply_topic(&message.ulid),
            Priority::Default,
            None,
            Some(&reply),
        );
    }
}

fn publish_clock_audio_status(
    persist: &Persist,
    node: &str,
    status: &mackes_mesh_types::clock::ClockAudioStatusV1,
) {
    let Ok(topic) = clock_audio_status_topic(node) else {
        return;
    };
    let Ok(body) = serde_json::to_string(status) else {
        return;
    };
    let _ = persist.write(&topic, Priority::Default, None, Some(&body));
}

fn transition_clock_audio_provider_loss(
    persist: &Persist,
    authority: &mut ClockAudioAuthority,
    seat_audio: &mut SeatAudioAuthority,
    music_engine: Option<&Engine>,
    alert_engine: Option<&Engine>,
) -> Option<f32> {
    let catalog = CatalogStoreFile::default();
    let cache_dir = crate::cache::cache_dir();
    let data_dir = state::data_dir();
    let mut effects = MusicClockAudioEffects {
        music_engine,
        alert_engine,
        clients: &[],
        catalog: &catalog,
        cache_dir: &cache_dir,
        data_dir: &data_dir,
        seat_audio,
    };
    let transition = authority.provider_lost(
        i64::try_from(state::now_ms()).unwrap_or(i64::MAX),
        &mut effects,
    );
    // Also retry a previously incomplete restore when the active Clock record
    // was already terminal. This is the daemon-shutdown/provider-loss backstop.
    let _ = effects.restore_seat_streams();
    let transition = transition?;
    publish_clock_audio_status(persist, &state::local_host(), &transition.status);
    transition.restored_music_volume
}

fn poll_clock_audio_start_deadline(
    persist: &Persist,
    authority: &mut ClockAudioAuthority,
    seat_audio: &mut SeatAudioAuthority,
    music_engine: Option<&Engine>,
    alert_engine: Option<&Engine>,
) {
    let catalog = CatalogStoreFile::default();
    let cache_dir = crate::cache::cache_dir();
    let data_dir = state::data_dir();
    let mut effects = MusicClockAudioEffects {
        music_engine,
        alert_engine,
        clients: &[],
        catalog: &catalog,
        cache_dir: &cache_dir,
        data_dir: &data_dir,
        seat_audio,
    };
    let now_ms = i64::try_from(state::now_ms()).unwrap_or(i64::MAX);
    let _ = authority.poll_preview(now_ms, &mut effects);
    if let Some(status) = authority.poll_music_start(now_ms, &mut effects) {
        publish_clock_audio_status(persist, &state::local_host(), &status);
    }
}

/// One transport-poll sweep: dispatch new `action/music/{play,pause,…}`
/// requests to the engine. MUSIC-RESPONSIVE-10 — the shared Airsonic client
/// (owned by [`serve`], refreshed on a creds change) is passed in so a
/// mid-session connect is still picked up without a per-sweep rebuild.
pub fn poll_transport(
    persist: &Persist,
    queue_path: &Path,
    engine: Option<&Engine>,
    cursors: &mut HashMap<String, String>,
    client: Option<&Client>,
) {
    let authorizer = music_action_auth::Authorizer::production();
    let clients = client.into_iter().collect::<Vec<_>>();
    poll_transport_with_authorizer(persist, queue_path, engine, cursors, &clients, &authorizer);
}

fn poll_transport_with_authorizer(
    persist: &Persist,
    queue_path: &Path,
    engine: Option<&Engine>,
    cursors: &mut HashMap<String, String>,
    clients: &[&Client],
    authorizer: &music_action_auth::Authorizer,
) {
    let queue = queue::read_from(queue_path);
    for verb in TRANSPORT_VERBS {
        let topic = format!("action/music/{verb}");
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since_limit(&topic, since, MAX_MESSAGES_PER_POLL) {
            Ok(m) => m,
            Err(_) => continue,
        };
        for msg in msgs {
            cursors.insert(topic.clone(), msg.ulid.clone());
            let body = msg.body.as_deref().unwrap_or("");
            let reply = match authorize_music_mutation(authorizer, verb, body) {
                Err(error) => unauthorized_reply(verb, &error),
                Ok(()) => apply_transport_with_clients(verb, body, engine, clients, &queue),
            };
            let _ = persist.write(
                &reply_topic(&msg.ulid),
                Priority::Default,
                None,
                Some(&reply),
            );
        }
    }
}

/// AIR-2.c — the queue index of the currently-audible track.
///
/// The play-start base cursor plus how many gapless track boundaries the engine
/// has crossed, clamped into the queue. A pure function so the boundary math is
/// unit-tested.
#[must_use]
pub fn audible_cursor(play_base: usize, track_index: usize, queue_len: usize) -> usize {
    if queue_len == 0 {
        return 0;
    }
    play_base.saturating_add(track_index).min(queue_len - 1)
}

/// AIR-2.c — the queue-driver. As gapless album playback crosses each track
/// boundary the engine's audible-track index advances, but the persisted queue
/// cursor (the now-playing `song_id` the GUI + the AIR-8 heartbeat report) was
/// pinned to the track that was current when Play was pressed. This runs every
/// serve sweep: when the engine is active and the audible track has moved past
/// the persisted cursor, it advances + persists the cursor so the now-playing
/// surface tracks the song you actually hear. No-op when idle or unchanged.
fn advance_queue_cursor(engine: Option<&Engine>, queue_path: &Path) {
    let Some(engine) = engine else { return };
    if !engine.is_active() {
        return;
    }
    let mut queue = queue::read_from(queue_path);
    let want = audible_cursor(
        engine.play_base(),
        engine.current_track_index(),
        queue.songs.len(),
    );
    // Forward-only: the engine plays its list strictly front-to-back, so the
    // driver only ever pushes the cursor FORWARD. This bounds the blast radius of
    // any base/queue skew (a mid-play queue edit, or a caller that mis-set the
    // play base) — it can lag the audible track by a sweep but never yanks the
    // now-playing cursor backward to an earlier song than the user already heard.
    if want > queue.current && want < queue.songs.len() {
        queue.current = want;
        let _ = queue::write_to(queue_path, &queue);
    }
}

/// Heartbeat this peer's playback state every [`STATE_WRITE_INTERVAL`]
/// while playing (AIR-8), and clear an ended engine's ownership claim. The
/// latter matters after provider loss: the engine can finish its bounded
/// fallback/reconnect path between sweeps and must not leave a silent peer
/// claiming playback until the stale-state timeout.
fn write_periodic_state(engine: Option<&Engine>, queue_path: &Path) {
    let Some(engine) = engine else { return };
    let queue = queue::read_from(queue_path);
    if engine.is_playing() {
        write_playback_state(true, queue.current().unwrap_or(""), engine.position_ms());
    } else if !engine.is_active() {
        write_playback_state(false, queue.current().unwrap_or(""), engine.position_ms());
    }
}

/// Continue finite media after the daemon has acquired a replacement physical
/// renderer. The recovery record is consumed only after the exact queue and
/// control generation still match and the fresh engine accepts the seeked
/// source list. Source unavailability leaves the intent pending for a later
/// bounded sweep; any user control invalidates it permanently.
fn resume_interrupted_playback(
    recovery: &mut RendererRecovery,
    engine: Option<&Engine>,
    queue_path: &Path,
    clients: &[&Client],
    controls: &ControlMarker,
) -> bool {
    recovery.invalidate_if_controls_changed(controls);
    let Some(engine) = engine else { return false };
    let queue = queue::read_from(queue_path);
    let Some(interrupted) = recovery.resumable(&queue, !engine.is_active()).cloned() else {
        return false;
    };
    let upcoming = if clients.is_empty() {
        cached_upcoming_tracks(&queue, &crate::cache::cache_dir()).map(|tracks| {
            tracks
                .into_iter()
                .map(|(url, codec)| PlaybackTrack::single(url, codec))
                .collect::<Vec<_>>()
        })
    } else {
        let catalog = load_catalog(&state::data_dir()).unwrap_or_default();
        source_aware_upcoming_candidates(&queue, clients, &catalog)
    };
    let Some(upcoming) = upcoming else {
        return false;
    };
    if !engine.play_from_candidates_at(upcoming, queue.current, interrupted.position_ms) {
        return false;
    }
    write_playback_state(true, queue.current().unwrap_or(""), interrupted.position_ms);
    recovery.complete(interrupted.generation);
    tracing::info!(
        generation = interrupted.generation,
        song_id = queue.current().unwrap_or(""),
        position_ms = interrupted.position_ms,
        "music playback continued on replacement physical renderer"
    );
    true
}

/// Run the Bus responder loop.
///
/// Polls the queue-control, library-browse, and transport
/// `action/music/<verb>` topics, dispatches them (queue + browse + the
/// AIR-5 engine), and replies on `reply/<ulid>`. Heartbeats the AIR-8
/// playback state while playing. Loops until `should_stop()` returns true.
///
/// # Panics
/// If the internal tokio runtime (for the async browse proxy) can't be
/// built — an environment fault, not a runtime condition.
/// MUSIC-RESPONSIVE-10 — whether the cached client must be rebuilt for the
/// freshly-loaded creds. A `None` cache (first build) or different creds are
/// stale; identical creds reuse the warm client.
#[must_use]
pub fn airsonic_creds_changed(cached: Option<&creds::Creds>, current: &creds::Creds) -> bool {
    cached != Some(current)
}

/// MUSIC-RESPONSIVE-10 — reload the stored creds and return the shared Airsonic
/// client, rebuilding it ONLY when the creds changed since the cached build (a
/// mid-session connect/disconnect). Reusing the client keeps reqwest's
/// connection pool warm across sweeps. `None` = no creds configured.
fn refresh_airsonic_clients(cache: &mut Vec<(creds::Creds, Client)>) {
    let currents = match creds::load_all() {
        Ok(sources) => sources,
        Err(creds::CredsError::Missing(_)) => {
            cache.clear();
            return;
        }
        Err(error) => {
            tracing::warn!(%error, "airsonic source list unavailable; retaining warm clients");
            return;
        }
    };
    let mut old = std::mem::take(cache);
    let mut refreshed = Vec::with_capacity(currents.len());
    for current in currents {
        if let Some(index) = old.iter().position(|(cached, _)| cached == &current) {
            let (_, client) = old.swap_remove(index);
            refreshed.push((current, client));
        } else {
            let client = Client::new(&current.server_url, &current.username, &current.password);
            refreshed.push((current, client));
        }
    }
    *cache = refreshed;
}

/// Return a stable, bounded startup phase for the music responder.
///
/// The phase is deliberately a tiny FNV-1a calculation rather than random
/// state: restarting one seat produces the same placement, while different
/// host identities normally land in different buckets.  Keeping this pure
/// makes the common-mode mitigation directly testable without launching the
/// daemon or touching the Bus store.
#[must_use]
pub fn initial_poll_phase(host: &str) -> Duration {
    let mut hash = 0x811c_9dc5_u32;
    for byte in host.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    Duration::from_millis(u64::from(hash % MAX_INITIAL_POLL_PHASE.as_millis() as u32))
}

/// Return a stable, bounded phase for the first active-playback heartbeat.
/// Use a distinct seed from [`initial_poll_phase`] so control and heartbeat
/// work do not collapse onto the same host bucket.
#[must_use]
pub fn initial_state_write_phase(host: &str) -> Duration {
    let mut hash = 0x9e37_79b9_u32;
    for byte in host.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    Duration::from_millis(u64::from(
        hash % (MAX_STATE_WRITE_PHASE.as_millis() as u32 + 1),
    ))
}

/// Run the single-threaded music bus responder until `should_stop` returns true.
pub fn serve<F: Fn() -> bool>(bus_root: PathBuf, queue_path: &Path, should_stop: F) {
    let mut persist = match Persist::open(bus_root.clone()) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = %e, "opening Bus store failed");
            return;
        }
    };
    // BUS-INODE-ORPHAN-1 (was MUSIC-WEDGE-2) — a BOOT-REC-3 self-heal recreate by
    // another process (unlink + new file) strands every OTHER process on the now-
    // DELETED inode, so the daemon keeps reading/writing a dead file and stops
    // seeing new requests (the "daemon not responding" wedge after long uptime).
    // The inode-swap detect + reopen now lives in the shared mde-bus crate
    // (`Persist::reopen_if_index_changed`), driven once per sweep below.
    // MUSIC-WEDGE — seed every poll cursor at the topic's CURRENT tail so a
    // restart skips the historical backlog. Without this, the first sweep's
    // `list_since_limit(None, MAX_MESSAGES_PER_POLL)` returns only the first
    // bounded page on each action topic
    // and the single-threaded loop reprocesses the whole backlog (each browse
    // verb = an Airsonic round-trip) before answering anything new — observed
    // live as the daemon "not responding" after a restart, and a stale `play`
    // could even replay. New (post-start) requests have a larger ULID and are
    // still picked up normally.
    let mut cursors: HashMap<String, String> = seed_cursors_at_tail(&persist);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime for browse proxy");
    // The engine grabs the default output device; on a headless peer (no
    // audio) it's absent and transport verbs reply with an error while
    // queue + browse keep working.
    let mut engine = Engine::new()
        .map_err(|e| {
            tracing::warn!(error = %e, "no audio output — playback disabled; queue + browse still served");
        })
        .ok();
    // Clock alerts use a second renderer owned by this daemon. It mixes through
    // PipeWire while leaving the Music queue engine, queue cursor, MPRIS state,
    // and mesh playback ownership untouched.
    let mut clock_audio_engine = engine.as_ref().and_then(|_| Engine::new().ok());
    let mut clock_audio_authority = ClockAudioAuthority::default();
    let mut seat_audio_authority = SeatAudioAuthority::production();
    let mut pending_clock_restore_volume = None;
    // AIR-6: bring up the MPRIS surface sharing this engine, so media keys
    // (sway → playerctl → MPRIS) + the lock-screen widget drive the same
    // playback the Bus does. Held for the serve loop's lifetime; dropping
    // it (when serve returns) stops the surface thread. A headless peer
    // with no audio engine — or no session bus — simply skips it.
    let mut _mpris = engine.as_ref().map(|e| {
        crate::mpris::spawn(
            e.handle(),
            queue_path.to_path_buf(),
            state::data_dir(),
            state::coordination_dir(),
        )
    });
    let mut renderer_recovery = RendererRecovery::default();
    let mut workspace_revision = match load_workspace_revision(&state::data_dir()) {
        Ok(revision) => Some(revision),
        Err(error) => {
            tracing::error!(error = %error, "music workspace revision unavailable; retained snapshots disabled");
            None
        }
    };
    // Last workspace projection written by this process, excluding its
    // revision.  A stable idle daemon should not rewrite the same retained
    // body every five seconds merely because its timer fired.
    let mut last_workspace_snapshot: Option<MusicWorkspaceSnapshotV1> = None;
    // MUSIC-AUDIO-BOOTRACE-1 — re-acquire the output device if it wasn't
    // available at startup. On a cold boot mde-musicd can start before the
    // PipeWire user session is ready, leaving `engine = None` (audio_available
    // false) so Play silently no-ops until a manual restart. Retry on a cadence
    // so playback comes up on its own once the session is.
    const AUDIO_RETRY_INTERVAL: Duration = Duration::from_secs(10);
    let mut last_audio_retry = Instant::now();
    // MUSIC-RESPONSIVE-10 — the Airsonic client persists across sweeps so
    // reqwest's keep-alive pool stays warm (it was rebuilt per sweep, so every
    // browse was a cold connect). Pre-open it at startup and warm the socket
    // with a cheap `ping` so the operator's first browse rides an established
    // connection instead of the launch-time cold-connect timeout. Rebuilt only
    // on a creds change (refresh_airsonic_clients).
    let mut airsonic: Vec<(creds::Creds, Client)> = Vec::new();
    refresh_airsonic_clients(&mut airsonic);
    for (_, client) in &airsonic {
        match rt.block_on(client.ping()) {
            Ok(_) => tracing::info!("airsonic connection warmed at startup"),
            Err(e) => {
                tracing::debug!(error = %e, "startup airsonic warm-ping failed (cold first browse)")
            }
        }
    }
    let mut last_creds_refresh = Instant::now();
    // The first loop performs one browse scan immediately; later scans are
    // deliberately slower than the transport/control-plane cadence.
    let mut browse_poll_due = true;
    let mut last_browse_poll = Instant::now();
    let authorizer = music_action_auth::Authorizer::production();
    let mut workspace_ledger = match load_workspace_ledger(&state::data_dir()) {
        Ok(ledger) => Some(ledger),
        Err(error) => {
            tracing::error!(error = %error, "music workspace action ledger unavailable");
            None
        }
    };
    if let Err(error) = recover_interrupted_downloads(&state::data_dir()) {
        tracing::warn!(%error, "music download recovery could not reconcile interrupted records");
    }
    // Spread the first full sweep across the bounded host phase.  Sleep in
    // short chunks so SIGTERM/stop predicates remain responsive even when a
    // phase is at the upper bound.
    let host = state::local_host();
    let phase = initial_poll_phase(&host);
    let state_write_phase = initial_state_write_phase(&host);
    let mut last_state_write = Instant::now() - STATE_WRITE_INTERVAL - state_write_phase;
    let phase_deadline = Instant::now() + phase;
    while !should_stop() {
        let remaining = phase_deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        std::thread::sleep(remaining.min(Duration::from_millis(20)));
    }
    while !should_stop() {
        // MUSIC-RENDERER-RECOVERY-1 — a cpal stream can fail after successful
        // startup when PipeWire restarts or the default device disappears.
        // Retaining that Engine would advertise audio_available forever while
        // its callback can no longer emit samples. Revoke the old MPRIS/control
        // surface and let the existing bounded acquisition cadence create a
        // completely fresh stream against the current default device.
        if engine
            .as_ref()
            .is_some_and(|current| !current.is_renderer_healthy())
        {
            pending_clock_restore_volume = transition_clock_audio_provider_loss(
                &persist,
                &mut clock_audio_authority,
                &mut seat_audio_authority,
                engine.as_ref(),
                clock_audio_engine.as_ref(),
            );
            clock_audio_engine = None;
            let interrupted_position_ms = engine
                .as_ref()
                .and_then(|current| current.interrupted_position_ms());
            let interrupted_queue = queue::read_from(queue_path);
            renderer_recovery.capture(
                interrupted_queue.clone(),
                interrupted_position_ms,
                control_marker(&cursors),
            );
            write_playback_state(
                false,
                interrupted_queue.current().unwrap_or(""),
                interrupted_position_ms.unwrap_or_default(),
            );
            tracing::warn!(
                "audio renderer became unavailable — playback disabled pending bounded reacquisition"
            );
            _mpris = None;
            engine = None;
            last_audio_retry = Instant::now();
        }
        if engine.is_none() && last_audio_retry.elapsed() >= AUDIO_RETRY_INTERVAL {
            last_audio_retry = Instant::now();
            if let Ok(e) = Engine::new() {
                if let Some(volume) = pending_clock_restore_volume.take() {
                    e.set_volume(volume);
                }
                tracing::info!("audio output acquired on retry — playback enabled");
                _mpris = Some(crate::mpris::spawn(
                    e.handle(),
                    queue_path.to_path_buf(),
                    state::data_dir(),
                    state::coordination_dir(),
                ));
                engine = Some(e);
            }
        }
        if engine.is_some()
            && clock_audio_engine
                .as_ref()
                .is_some_and(|current| !current.is_renderer_healthy())
        {
            let _ = transition_clock_audio_provider_loss(
                &persist,
                &mut clock_audio_authority,
                &mut seat_audio_authority,
                engine.as_ref(),
                clock_audio_engine.as_ref(),
            );
            clock_audio_engine = None;
        }
        if engine.is_some()
            && clock_audio_engine.is_none()
            && last_audio_retry.elapsed() >= AUDIO_RETRY_INTERVAL
        {
            last_audio_retry = Instant::now();
            clock_audio_engine = Engine::new().ok();
        }
        // BUS-INODE-ORPHAN-1 — if the index inode swapped under us (another
        // process recreated it), reopen so we follow the live DB instead of a
        // deleted one. Cheap stat per sweep; reopen only on an actual change.
        // Cursors carry over — new requests have larger ULIDs and are still
        // picked up.
        persist.reopen_if_index_changed();
        // AUDIT-MESH-14 — run the FAST, local-only responders (queue control,
        // transport/get-state, peer roster) BEFORE the network-bound browse
        // proxy. `poll_browse` does blocking Airsonic REST calls; if it ran
        // first, a slow/unreachable server would starve every transport reply
        // in this single-threaded loop (observed live: get-state timed out at
        // 9s, just under poll_browse's ~10s HTTP timeout). With transport first,
        // get-state is answered within POLL_INTERVAL of the request regardless
        // of browse latency. (The Airsonic client also has connect/total
        // timeouts now so browse itself can't hang forever.)
        poll_once_with_authorizer(&persist, queue_path, &mut cursors, &authorizer);
        // AIR-2.c — advance the persisted queue cursor to the audible track as
        // gapless playback crosses boundaries, BEFORE poll_transport so a
        // get-state in this same sweep reports the song you actually hear.
        advance_queue_cursor(engine.as_ref(), queue_path);
        // MUSIC-RESPONSIVE-10 — refresh the shared clients on a short bounded
        // configuration cadence rather than once per 500 ms control sweep.
        // The primary is passed to provider mutations; the full bounded set is
        // passed to transport, typed source-aware playback, and catalog work.
        if last_creds_refresh.elapsed() >= CREDS_REFRESH_INTERVAL {
            refresh_airsonic_clients(&mut airsonic);
            last_creds_refresh = Instant::now();
        }
        let clients = airsonic
            .iter()
            .map(|(_, client)| client)
            .collect::<Vec<_>>();
        poll_clock_audio_with_authorizer(
            &persist,
            &mut cursors,
            &mut clock_audio_authority,
            &mut seat_audio_authority,
            engine.as_ref(),
            clock_audio_engine.as_ref(),
            &clients,
            &authorizer,
        );
        poll_clock_audio_start_deadline(
            &persist,
            &mut clock_audio_authority,
            &mut seat_audio_authority,
            engine.as_ref(),
            clock_audio_engine.as_ref(),
        );
        poll_transport_with_authorizer(
            &persist,
            queue_path,
            engine.as_ref(),
            &mut cursors,
            &clients,
            &authorizer,
        );
        // The typed workspace lane is the migration seam for the GUI. It
        // refuses unsupported actions instead of claiming parity that has not
        // yet been implemented in the daemon authority.
        poll_workspace_with_authorizer(
            &persist,
            queue_path,
            engine.as_ref(),
            &clients,
            &rt,
            &mut cursors,
            &mut workspace_ledger,
            &authorizer,
        );
        poll_peers_with_authorizer(&persist, &mut cursors, &authorizer);
        apply_pending_handoff(engine.as_ref(), queue_path);
        apply_handoff_completions(engine.as_ref(), queue_path, &clients);
        let _ = resume_interrupted_playback(
            &mut renderer_recovery,
            engine.as_ref(),
            queue_path,
            &clients,
            &control_marker(&cursors),
        );
        if browse_poll_due || last_browse_poll.elapsed() >= BROWSE_POLL_INTERVAL {
            poll_browse_with_clients(&persist, &rt, &mut cursors, &clients, &authorizer);
            last_browse_poll = Instant::now();
            browse_poll_due = false;
        }
        if last_state_write.elapsed() >= STATE_WRITE_INTERVAL {
            write_periodic_state(engine.as_ref(), queue_path);
            if let Some(current_revision) = workspace_revision {
                if let Some(next_revision) = current_revision.checked_add(1) {
                    let snapshot = workspace_snapshot_from_dirs(
                        &queue::read_from(queue_path),
                        engine.as_ref(),
                        next_revision,
                        &state::data_dir(),
                        &state::coordination_dir(),
                    );
                    if let Err(error_code) = snapshot.validate() {
                        tracing::warn!(
                            revision = next_revision,
                            error_code,
                            "music workspace snapshot rejected before publication"
                        );
                    } else if !last_workspace_snapshot
                        .as_ref()
                        .is_some_and(|previous| workspace_snapshot_content_eq(previous, &snapshot))
                    {
                        if persist_workspace_revision(&state::data_dir(), next_revision).is_ok() {
                            match serde_json::to_string(&snapshot) {
                                Ok(body) => match persist.write(
                                    WORKSPACE_STATE_TOPIC,
                                    Priority::Default,
                                    None,
                                    Some(&body),
                                ) {
                                    Ok(_) => {
                                        last_workspace_snapshot = Some(snapshot);
                                        workspace_revision = Some(next_revision);
                                    }
                                    Err(error) => {
                                        tracing::warn!(
                                            revision = next_revision,
                                            error = %error,
                                            "music workspace snapshot could not be published; retrying"
                                        );
                                    }
                                },
                                Err(error) => tracing::warn!(
                                    revision = next_revision,
                                    error = %error,
                                    "music workspace snapshot could not be encoded; retrying"
                                ),
                            }
                        } else {
                            tracing::error!(
                                revision = next_revision,
                                "music workspace revision could not be persisted; snapshot publication disabled"
                            );
                            workspace_revision = None;
                        }
                    }
                } else {
                    tracing::error!(
                        "music workspace revision exhausted; snapshot publication disabled"
                    );
                    workspace_revision = None;
                }
            }
            last_state_write = Instant::now();
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    let _ = transition_clock_audio_provider_loss(
        &persist,
        &mut clock_audio_authority,
        &mut seat_audio_authority,
        engine.as_ref(),
        clock_audio_engine.as_ref(),
    );
    let clients = airsonic
        .iter()
        .map(|(_, client)| client)
        .collect::<Vec<_>>();
    let queue = queue::read_from(queue_path);
    finalize_progress_for_transport(
        &queue,
        None,
        engine.as_ref(),
        &clients,
        &rt,
        &state::data_dir(),
        "close",
    );
}

/// MUSIC-WEDGE — build the initial cursor map seeded at each polled topic's
/// current tail (its newest ULID), so the serve loop only handles requests that
/// arrive AFTER startup and never reprocesses the historical backlog. Topics with
/// no messages get no entry (cursor `None` → first real message is picked up).
#[must_use]
pub fn seed_cursors_at_tail(persist: &Persist) -> HashMap<String, String> {
    let mut cursors = HashMap::new();
    let verbs = ACTION_VERBS
        .iter()
        .chain(BROWSE_VERBS.iter())
        .chain(TRANSPORT_VERBS.iter())
        .chain([WORKSPACE_ACTION_VERB].iter())
        .chain(PEER_VERBS.iter())
        .chain([CLOCK_AUDIO_VERB].iter());
    for verb in verbs {
        let topic = format!("action/music/{verb}");
        if let Ok(Some(latest)) = persist.latest_ulid(&topic) {
            cursors.insert(topic, latest);
        }
    }
    cursors
}

/// One poll sweep over the AIR-15.b.5 peer verbs (`peer-states`,
/// `take-over`) — reads/writes the AIR-8 state dir, replies on reply/<ulid>.
pub fn poll_peers(persist: &Persist, cursors: &mut HashMap<String, String>) {
    let authorizer = music_action_auth::Authorizer::production();
    poll_peers_with_authorizer(persist, cursors, &authorizer);
}

fn poll_peers_with_authorizer(
    persist: &Persist,
    cursors: &mut HashMap<String, String>,
    authorizer: &music_action_auth::Authorizer,
) {
    let dir = state::coordination_dir();
    for verb in PEER_VERBS {
        let topic = format!("action/music/{verb}");
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since_limit(&topic, since, MAX_MESSAGES_PER_POLL) {
            Ok(m) => m,
            Err(_) => continue,
        };
        for msg in msgs {
            cursors.insert(topic.clone(), msg.ulid.clone());
            let body = msg.body.as_deref().unwrap_or("");
            let reply = match authorize_music_mutation(authorizer, verb, body) {
                Err(error) => unauthorized_reply(verb, &error),
                Ok(()) => dispatch_peer(verb, body, &dir),
            };
            let _ = persist.write(
                &reply_topic(&msg.ulid),
                Priority::Default,
                None,
                Some(&reply),
            );
        }
    }
}

/// One poll sweep across the queue-control action verbs (extracted so tests
/// can drive it deterministically without the sleep loop).
pub fn poll_once(persist: &Persist, queue_path: &Path, cursors: &mut HashMap<String, String>) {
    let authorizer = music_action_auth::Authorizer::production();
    poll_once_with_authorizer(persist, queue_path, cursors, &authorizer);
}

fn poll_once_with_authorizer(
    persist: &Persist,
    queue_path: &Path,
    cursors: &mut HashMap<String, String>,
    authorizer: &music_action_auth::Authorizer,
) {
    let mut q = queue::read_from(queue_path);
    for verb in ACTION_VERBS {
        let topic = format!("action/music/{verb}");
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since_limit(&topic, since, MAX_MESSAGES_PER_POLL) {
            Ok(m) => m,
            Err(_) => continue,
        };
        for msg in msgs {
            cursors.insert(topic.clone(), msg.ulid.clone());
            let body = msg.body.as_deref().unwrap_or("");
            let d = match authorize_music_mutation(authorizer, verb, body) {
                Err(error) => error_reply(&format!("{verb}: authorization refused: {error}")),
                Ok(()) => dispatch_queue_action(verb, body, &mut q),
            };
            let _ = persist.write(
                &reply_topic(&msg.ulid),
                Priority::Default,
                None,
                Some(&d.reply_json),
            );
            if d.mutated {
                let _ = queue::write_to(queue_path, &q);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUTH_KEY: &[u8] = b"music-action-auth-test-key";
    const AUTH_NOW_MS: i64 = 1_700_000_000_000;

    #[test]
    fn renderer_recovery_refuses_stale_generation_after_intervening_control() {
        let queue = Queue {
            songs: vec!["interrupted-track".to_string(), "next-track".to_string()],
            current: 0,
        };
        let before = ControlMarker(vec![(
            "action/music/stop".to_string(),
            "01-before".to_string(),
        )]);
        let after_stop = ControlMarker(vec![(
            "action/music/stop".to_string(),
            "02-hostile-stop".to_string(),
        )]);
        let mut recovery = RendererRecovery::default();
        recovery.capture(queue.clone(), Some(42_000), before);
        let stale_generation = recovery
            .resumable(&queue, true)
            .expect("finite interrupted playback is initially resumable")
            .generation;

        recovery.invalidate_if_controls_changed(&after_stop);
        assert!(
            recovery.resumable(&queue, true).is_none(),
            "a Stop observed while the renderer is absent must cancel continuation"
        );

        recovery.capture(queue.clone(), Some(43_000), after_stop);
        recovery.complete(stale_generation);
        let current = recovery
            .resumable(&queue, true)
            .expect("a stale completion cannot consume the newer generation");
        assert_ne!(current.generation, stale_generation);

        let mut replaced_queue = queue;
        replaced_queue.songs[0] = "operator-selected-track".to_string();
        assert!(
            recovery.resumable(&replaced_queue, true).is_none(),
            "continuation must retain the exact interrupted queue identity"
        );
    }

    #[test]
    fn bookmark_projection_keeps_resume_position_and_rejects_unknown_media() {
        let bookmark = crate::airsonic::Bookmark {
            id: "episode-1".into(),
            title: "Episode One".into(),
            position_ms: 42_000,
            kind: "podcast".into(),
            creator: "The Show".into(),
            parent_title: "Feed One".into(),
            duration_ms: Some(120_000),
            artwork_ref: Some("art-1".into()),
        };
        let projected = bookmark_item("airsonic:http://one.test", &bookmark).unwrap();
        assert_eq!(projected.content.kind, ContentKind::Episode);
        assert_eq!(projected.content.remote_id, "episode-1");
        assert_eq!(projected.position_ms, 42_000);
        assert!(bookmark_item(
            "airsonic:http://one.test",
            &crate::airsonic::Bookmark {
                kind: "video".into(),
                ..bookmark
            }
        )
        .is_none());
    }

    fn test_authorizer(root: &Path) -> music_action_auth::Authorizer {
        music_action_auth::Authorizer::for_test(AUTH_KEY, root.join("auth"), AUTH_NOW_MS)
    }

    fn one_shot_json_server(body: &'static str) -> String {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind one-shot server");
        let addr = listener.local_addr().expect("one-shot address");
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        });
        format!("http://{addr}")
    }

    fn one_shot_bytes_server(body: &'static [u8]) -> String {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind byte server");
        let addr = listener.local_addr().expect("byte server addr");
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut buf = [0_u8; 2048];
            let _ = stream.read(&mut buf);
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: audio/mpeg\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(body);
        });
        format!("http://{addr}")
    }

    fn armed_test_body(unsigned: &str, verb: &str, target: &str, nonce: &str) -> String {
        armed_test_body_with_expiry(unsigned, verb, target, nonce, AUTH_NOW_MS + 30_000)
    }

    fn armed_test_body_with_expiry(
        unsigned: &str,
        verb: &str,
        target: &str,
        nonce: &str,
        expires_at_ms: i64,
    ) -> String {
        let node = state::local_host();
        let request_sha256 = music_action_auth::request_digest_for_test(unsigned);
        let payload =
            format!("v2|{nonce}|{expires_at_ms}|music-{verb}|{node}|{target}|{request_sha256}");
        let signature = music_action_auth::sign_for_test(AUTH_KEY, &payload);
        let token = format!("{payload}|{signature}");
        let mut body: serde_json::Value = serde_json::from_str(unsigned).unwrap();
        body.as_object_mut()
            .unwrap()
            .insert("armed_token".to_string(), serde_json::Value::String(token));
        body.to_string()
    }

    #[test]
    fn production_music_nonce_ledger_is_user_owned_state() {
        let root = music_action_auth::production_auth_root();
        assert!(root.ends_with(".local/share/mde/music-auth-nonces"));
        assert!(!root.starts_with("/var/lib/mackesd"));
    }

    #[test]
    fn music_action_auth_rejects_capabilities_at_the_expiry_boundary() {
        let root = tempfile::tempdir().unwrap();
        let node = state::local_host();
        let unsigned = r#"{"schema_version":1,"song_id":"track-a"}"#;
        let legacy = armed_test_body_with_expiry(
            unsigned,
            "enqueue",
            "queue",
            "music-expiry-legacy",
            AUTH_NOW_MS,
        );
        assert!(test_authorizer(root.path())
            .authorize(&legacy, "music-enqueue", &node, "queue")
            .unwrap_err()
            .contains("expired"));

        let seed = [9_u8; 32];
        let signed = mackes_mesh_types::music_auth::sign_request(
            r#"{"action":"play","request_id":"expiry-ed25519","schema_version":1}"#,
            mackes_mesh_types::music_auth::MusicAuthContext {
                verb: "music-workspace",
                node: &node,
                target: "workspace",
            },
            &seed,
            "music-expiry-ed25519",
            AUTH_NOW_MS,
        )
        .unwrap();
        let verifier = music_action_auth::Authorizer::for_test_music(
            ed25519_dalek::SigningKey::from_bytes(&seed).verifying_key(),
            root.path().join("music-auth"),
            AUTH_NOW_MS,
        );
        assert!(verifier
            .authorize(&signed, "music-workspace", &node, "workspace")
            .unwrap_err()
            .contains("expired"));
    }

    #[test]
    fn workspace_revision_is_durable_across_daemon_restarts() {
        let root = tempfile::tempdir().expect("revision root");
        assert_eq!(
            load_workspace_revision(root.path()).expect("first start"),
            0
        );

        persist_workspace_revision(root.path(), 41).expect("persist first revision");
        assert_eq!(
            load_workspace_revision(root.path()).expect("reload first revision"),
            41
        );

        persist_workspace_revision(
            root.path(),
            load_workspace_revision(root.path()).expect("read before next") + 1,
        )
        .expect("persist next revision");
        assert_eq!(
            load_workspace_revision(root.path()).expect("reload next"),
            42
        );
    }

    #[test]
    fn workspace_revision_rejects_corrupt_state_instead_of_resetting() {
        let root = tempfile::tempdir().expect("revision root");
        std::fs::write(workspace_revision_path(root.path()), b"not-a-revision")
            .expect("write corrupt revision");
        assert_eq!(
            load_workspace_revision(root.path())
                .expect_err("corrupt revision must fail closed")
                .kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn unchanged_workspace_projection_ignores_revision_for_dedupe() {
        let root = tempfile::tempdir().expect("workspace root");
        let first = workspace_snapshot_from_dir(&Queue::default(), None, 41, root.path());
        let second = workspace_snapshot_from_dir(&Queue::default(), None, 42, root.path());
        assert!(workspace_snapshot_content_eq(&first, &second));

        let mut changed = second.clone();
        changed.any_source_reachable = !changed.any_source_reachable;
        assert!(!workspace_snapshot_content_eq(&first, &changed));
    }

    #[test]
    fn music_mutation_scopes_cover_every_write_and_leave_reads_open() {
        for verb in [
            "enqueue",
            "enqueue-after",
            "clear",
            "next",
            "prev",
            "queue-move",
            "queue-remove",
            "queue-remove-many",
            "queue-move-to-next",
        ] {
            assert_eq!(music_mutation_scope(verb), Some("queue"), "{verb}");
        }
        for verb in [
            "playlist-create",
            "playlist-update",
            "playlist-delete",
            "playlist-reorder",
        ] {
            assert_eq!(music_mutation_scope(verb), Some("playlists"), "{verb}");
        }
        for verb in ["play", "pause", "resume", "stop", "set-volume", "seek"] {
            assert_eq!(music_mutation_scope(verb), Some("transport"), "{verb}");
        }
        assert_eq!(music_mutation_scope("take-over"), Some("peer-takeover"));
        assert_eq!(
            music_action_scope(WORKSPACE_ACTION_VERB, r#"{"action":"transfer"}"#),
            Some("peer-takeover")
        );
        for verb in [
            "get-queue",
            "list-albums",
            "get-playlist",
            "get-state",
            "peer-states",
        ] {
            assert_eq!(
                music_mutation_scope(verb),
                None,
                "{verb} must stay read-only"
            );
        }
    }

    #[test]
    fn music_action_auth_rejects_hostile_accepts_authorized_and_replays_once() {
        let root = tempfile::tempdir().unwrap();
        let authorizer = test_authorizer(root.path());
        let verb = "enqueue";
        let target = "queue";
        let unsigned = r#"{"schema_version":1,"song_id":"track-a"}"#;
        let node = state::local_host();

        assert!(authorizer
            .authorize(unsigned, "music-enqueue", &node, target)
            .is_err());

        let armed = armed_test_body(unsigned, verb, target, "music-nonce-hostile");
        let tampered = armed.replace("track-a", "track-b");
        assert!(authorizer
            .authorize(&tampered, "music-enqueue", &node, target)
            .is_err());
        assert!(authorizer
            .authorize(&armed, "music-enqueue", &node, target)
            .is_ok());
        assert!(authorizer
            .authorize(&armed, "music-enqueue", &node, target)
            .unwrap_err()
            .contains("already used"));
    }

    #[test]
    fn music_action_auth_hmac_matches_sha256_test_vector() {
        assert_eq!(
            music_action_auth::sign_for_test(b"key", "The quick brown fox jumps over the lazy dog"),
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
    }

    #[test]
    fn music_action_auth_ed25519_is_scoped_and_single_use() {
        use ed25519_dalek::SigningKey;
        use mackes_mesh_types::music_auth::{self, MusicAuthContext};

        let root = tempfile::tempdir().unwrap();
        let seed = [9_u8; 32];
        let signing_key = SigningKey::from_bytes(&seed);
        let node = state::local_host();
        let unsigned = r#"{"schema_version":1,"action":"play","request_id":"r1"}"#;
        let signed = music_auth::sign_request(
            unsigned,
            MusicAuthContext {
                verb: "music-workspace",
                node: &node,
                target: "workspace",
            },
            &seed,
            "music-ed25519-nonce",
            AUTH_NOW_MS + 30_000,
        )
        .unwrap();
        let authorizer = music_action_auth::Authorizer::for_test_music(
            signing_key.verifying_key(),
            root.path().join("auth"),
            AUTH_NOW_MS,
        );
        assert!(authorizer
            .authorize(&signed, "music-workspace", &node, "workspace")
            .is_ok());
        assert!(authorizer
            .authorize(&signed, "music-workspace", &node, "workspace")
            .unwrap_err()
            .contains("already used"));
        let tampered = signed.replace("play", "pause");
        assert!(authorizer
            .authorize(&tampered, "music-workspace", &node, "workspace")
            .is_err());
    }

    #[test]
    fn signed_workspace_envelope_decodes_without_weakening_unknown_field_rejection() {
        use mackes_mesh_types::music_auth::{self, MusicAuthContext};

        let unsigned = r#"{"schema_version":1,"action":"play","request_id":"radio-play","content":{"source_id":"airsonic:http://radio.test","remote_id":"https://stream.test/live","kind":"radio"}}"#;
        let signed = music_auth::sign_request(
            unsigned,
            MusicAuthContext {
                verb: "music-workspace",
                node: &state::local_host(),
                target: "workspace",
            },
            &[9_u8; 32],
            "signed-workspace-decode",
            AUTH_NOW_MS + 30_000,
        )
        .unwrap();
        let request = parse_authorized_workspace_request(&signed)
            .expect("verified music_auth is a wire field, not a domain field");
        assert_eq!(request.action, "play");
        assert_eq!(request.content.unwrap().kind, ContentKind::Radio);

        let mut hostile: Value = serde_json::from_str(&signed).unwrap();
        hostile
            .as_object_mut()
            .unwrap()
            .insert("unexpected".into(), Value::Bool(true));
        assert!(parse_authorized_workspace_request(&hostile.to_string()).is_err());
    }

    #[test]
    fn airsonic_creds_changed_detects_first_build_and_changes() {
        // MUSIC-RESPONSIVE-10 — the warm-client cache rebuilds only when the
        // creds actually change; an empty cache or different creds are stale,
        // identical creds reuse the warm client.
        let a = creds::Creds {
            server_url: "http://a:4040".into(),
            username: "u".into(),
            password: "p".into(),
        };
        let same = a.clone();
        let other = creds::Creds {
            server_url: "http://b:4040".into(),
            ..a.clone()
        };
        assert!(airsonic_creds_changed(None, &a), "no cache → must build");
        assert!(
            !airsonic_creds_changed(Some(&a), &same),
            "identical creds → reuse the warm client"
        );
        assert!(
            airsonic_creds_changed(Some(&a), &other),
            "changed server → rebuild"
        );
    }

    #[test]
    fn song_id_from_accepts_id_and_song_id_keys_and_rejects_raw_objects() {
        // The Notification Hub sends {"id":...}; older callers send {"song_id":...}.
        assert_eq!(song_id_from(r#"{"id":"14119"}"#).as_deref(), Some("14119"));
        assert_eq!(song_id_from(r#"{"song_id":"22"}"#).as_deref(), Some("22"));
        // A bare (possibly quoted) id string still works.
        assert_eq!(song_id_from("14119").as_deref(), Some("14119"));
        assert_eq!(song_id_from(r#""14119""#).as_deref(), Some("14119"));
        // An unrecognised JSON object must NOT be fed raw to Airsonic (the bug:
        // {"id":...} used to reach getSong as the literal id → HTTP 400).
        assert_eq!(song_id_from(r#"{"foo":"bar"}"#), None);
        assert_eq!(song_id_from(""), None);
    }

    #[test]
    fn cover_art_reply_carries_a_path_not_base64_bytes() {
        // MUSIC-RESPONSIVE-4 — the get-cover-art reply must carry a file PATH, not
        // the base64 image blob, so the Bus spool stops growing with art bytes.
        // Point the LOCAL artwork cache at a tempdir (it keys off $HOME via
        // crate::cache::cache_dir()). One test owns $HOME so there's no parallel race.
        let home = tempfile::tempdir().expect("tmp home");
        std::env::set_var("HOME", home.path());

        let bytes = b"\xff\xd8\xff\xe0JFIF-cover-bytes".to_vec();
        let reply = cover_art_path_reply("al-42", &bytes);
        // Path, not bytes: the reply has a `path` and NO base64 `art` field.
        let path = reply
            .get("path")
            .and_then(serde_json::Value::as_str)
            .expect("path field");
        assert!(reply.get("art").is_none(), "must not carry base64 art");
        assert_eq!(
            reply.get("bytes").and_then(serde_json::Value::as_u64),
            Some(bytes.len() as u64)
        );
        // The path is a real, always-readable local file holding exactly the bytes.
        let on_disk = std::fs::read(path).expect("materialized art file");
        assert_eq!(on_disk, bytes);
        // The path string itself is tiny vs the would-be base64 payload — that's
        // the whole point: the Bus reply no longer grows with the image.
        assert!(path.len() < 256, "path reply stays small");
        // A second call for the same id reuses the file (no rewrite, same path).
        let again = cover_art_path_reply("al-42", &bytes);
        assert_eq!(again.get("path"), reply.get("path"));

        std::env::remove_var("HOME");
    }

    #[test]
    fn dispatch_peer_roster_and_take_over() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(dispatch_peer("peer-states", "", dir.path()).contains("\"peers\":[]"));
        let t = dispatch_peer("take-over", "anvil", dir.path());
        assert!(t.contains("\"ok\":true") && t.contains("intent_id"));
        assert_eq!(state::read_intents(dir.path()).len(), 1);
        assert!(dispatch_peer("bogus", "", dir.path()).contains("\"ok\":false"));
    }

    #[test]
    fn handoff_snapshot_preserves_queue_song_and_position() {
        let queue = Queue {
            songs: vec!["song-a".into(), "song-b".into()],
            current: 1,
        };
        let snapshot = handoff_state(&queue, "anvil", 42_500);
        assert_eq!(snapshot.peer, "anvil");
        assert!(!snapshot.playing);
        assert_eq!(snapshot.song_id, "song-b");
        assert_eq!(snapshot.position_ms, 42_500);
    }

    #[test]
    fn two_seat_handoff_is_exact_once_replay_safe_and_failure_honest() {
        let source_queue = Queue {
            songs: vec![
                "admitted-before".into(),
                "admitted-current".into(),
                "admitted-after".into(),
            ],
            current: 1,
        };
        let intent = state::HandoffIntent {
            intent_id: "two-seat-transfer-1".into(),
            from_peer: "target-seat".into(),
            to_peer: Some("source-seat".into()),
            issued_ms: 1_000,
        };
        let completion = state::HandoffCompletion {
            intent_id: intent.intent_id.clone(),
            from_peer: intent.from_peer.clone(),
            owner_peer: "source-seat".into(),
            song_id: "admitted-current".into(),
            queue: source_queue.clone(),
            position_ms: 42_500,
            completed_ms: 1_001,
            expires_ms: 1_001 + state::HANDOFF_ACK_TIMEOUT_MS,
        };

        let mut source_yields = 0;
        let mut source_playing = true;
        let mut target_playing = false;
        if matches!(
            owner_handoff_action(intent.clone(), &[], &[], 1_000),
            OwnerHandoffAction::Yield(_)
        ) {
            source_yields += 1;
            source_playing = false;
        }
        assert!(!source_playing && !target_playing);
        for now_ms in [1_001, 1_002, completion.expires_ms] {
            assert_eq!(
                owner_handoff_action(
                    intent.clone(),
                    std::slice::from_ref(&completion),
                    &[],
                    now_ms,
                ),
                OwnerHandoffAction::AwaitTarget,
                "a durable completion must suppress duplicate source yields"
            );
        }
        assert_eq!(source_yields, 1, "source owner must yield exactly once");

        let resumed_queue = admitted_handoff_queue(
            &completion,
            std::slice::from_ref(&intent),
            "target-seat",
            1_002,
        )
        .expect("fresh target-owned transfer is admitted");
        assert_eq!(resumed_queue, source_queue);
        assert_eq!(resumed_queue.current(), Some("admitted-current"));
        assert_eq!(completion.position_ms, 42_500);
        target_playing = true;
        assert_ne!(source_playing, target_playing, "success has one owner");
        let target_commit = MusicState {
            peer: "target-seat".into(),
            playing: true,
            song_id: "admitted-current".into(),
            position_ms: 42_500,
            updated_ms: 1_003,
        };
        assert_eq!(
            owner_handoff_action(
                intent.clone(),
                std::slice::from_ref(&completion),
                std::slice::from_ref(&target_commit),
                completion.expires_ms + 1,
            ),
            OwnerHandoffAction::RetireCommitted,
            "delayed cleanup must not let the source split authority after target commit"
        );

        let mut target_refused = target_commit.clone();
        target_refused.playing = false;
        assert!(
            matches!(
                owner_handoff_action(
                    intent.clone(),
                    std::slice::from_ref(&completion),
                    std::slice::from_ref(&target_refused),
                    completion.expires_ms + 1,
                ),
                OwnerHandoffAction::Reclaim(candidate) if candidate == completion
            ),
            "an idle target state is not proof that audible authority transferred"
        );

        assert!(
            admitted_handoff_queue(&completion, &[], "target-seat", 1_003).is_none(),
            "a consumed completion cannot replay without its one-use intent"
        );
        let superseding = state::HandoffIntent {
            intent_id: "two-seat-transfer-2".into(),
            issued_ms: 1_004,
            ..intent.clone()
        };
        assert!(
            admitted_handoff_queue(
                &completion,
                &[intent.clone(), superseding],
                "target-seat",
                1_004,
            )
            .is_none(),
            "a completion from a superseded transfer cannot seize authority"
        );

        // Hostile target-start failure: no target heartbeat is committed and
        // the completion remains. At lease expiry the production source-side
        // decision restores the paused source, leaving exactly one honest
        // playing owner instead of silent or split-brain authority.
        target_playing = false;
        assert!(matches!(
            owner_handoff_action(
                intent,
                std::slice::from_ref(&completion),
                &[],
                completion.expires_ms + 1,
            ),
            OwnerHandoffAction::Reclaim(candidate) if candidate == completion
        ));
        source_playing = true;
        assert_ne!(source_playing, target_playing, "failure leaves one owner");
    }

    #[test]
    fn str_array_reads_strings_and_numbers() {
        // MUSIC-RFX-3 — playlist write bodies carry string id arrays and numeric
        // index arrays; both flatten to Vec<String>.
        let v: serde_json::Value =
            serde_json::from_str(r#"{"song_ids":["s1","s2"],"remove_indices":[0,2]}"#).unwrap();
        assert_eq!(str_array(&v, "song_ids"), vec!["s1", "s2"]);
        assert_eq!(str_array(&v, "remove_indices"), vec!["0", "2"]);
        // Missing / non-array → empty.
        assert!(str_array(&v, "absent").is_empty());
        assert!(str_array(&json!({"x": "scalar"}), "x").is_empty());
    }

    #[test]
    fn playlist_write_verbs_are_browse_verbs() {
        // MUSIC-RFX-3/6b — the write + reorder verbs are served on the browse poll.
        for verb in [
            "playlist-create",
            "playlist-update",
            "playlist-delete",
            "playlist-reorder",
        ] {
            assert!(
                BROWSE_VERBS.contains(&verb),
                "{verb} missing from BROWSE_VERBS"
            );
        }
    }

    #[test]
    fn song_id_parsing_forms() {
        assert_eq!(song_id_from(r#"{"song_id":"s1"}"#).as_deref(), Some("s1"));
        assert_eq!(song_id_from(r#""s2""#).as_deref(), Some("s2"));
        assert_eq!(song_id_from("s3").as_deref(), Some("s3"));
        assert_eq!(song_id_from("  "), None);
    }

    #[test]
    fn audible_cursor_advances_per_gapless_boundary() {
        // AIR-2.c — Play started at queue index 2 (`play_base`), engine track 0.
        // Queue has 5 songs. The audible queue index = base + engine track index.
        assert_eq!(audible_cursor(2, 0, 5), 2); // still on the track Play began on
        assert_eq!(audible_cursor(2, 1, 5), 3); // crossed one boundary
        assert_eq!(audible_cursor(2, 2, 5), 4); // last song
                                                // Never runs off the end of the queue (clamped to len-1).
        assert_eq!(audible_cursor(2, 5, 5), 4);
        assert_eq!(audible_cursor(0, 99, 3), 2);
        // Play from the very start.
        assert_eq!(audible_cursor(0, 0, 3), 0);
        assert_eq!(audible_cursor(0, 1, 3), 1);
        // Empty queue → 0, no panic.
        assert_eq!(audible_cursor(0, 0, 0), 0);
        assert_eq!(audible_cursor(3, 2, 0), 0);
    }

    #[test]
    fn dispatch_enqueue_and_get() {
        let mut q = Queue::default();
        let d = dispatch_queue_action("enqueue", r#"{"song_id":"a"}"#, &mut q);
        assert!(d.mutated);
        assert!(d.reply_json.contains("\"ok\":true"));
        assert!(d.reply_json.contains("\"len\":1"));
        // get-queue doesn't mutate.
        let g = dispatch_queue_action("get-queue", "", &mut q);
        assert!(!g.mutated);
        assert!(g.reply_json.contains("\"current\":\"a\""));
    }

    #[test]
    fn dispatch_enqueue_after_and_walk() {
        let mut q = Queue::default();
        let _ = dispatch_queue_action("enqueue", "a", &mut q);
        let _ = dispatch_queue_action("enqueue", "b", &mut q);
        let _ = dispatch_queue_action("enqueue-after", "x", &mut q);
        assert_eq!(q.songs, vec!["a", "x", "b"]);
        let d = dispatch_queue_action("next", "", &mut q);
        assert!(d.mutated);
        assert_eq!(q.current(), Some("x"));
    }

    #[test]
    fn seed_cursors_at_tail_skips_backlog() {
        let dir = tempfile::tempdir().unwrap();
        let persist = Persist::open(dir.path().join("bus")).unwrap();
        let queue_path = dir.path().join("queue.json");
        let authorizer = test_authorizer(dir.path());
        // A stale enqueue sits in the backlog from "before the restart".
        let stale = armed_test_body(
            r#"{"schema_version":1,"song_id":"stale"}"#,
            "enqueue",
            "queue",
            "music-nonce-stale",
        );
        persist
            .write(
                "action/music/enqueue",
                Priority::Default,
                None,
                Some(&stale),
            )
            .unwrap();
        // Seed cursors at the tail (simulating daemon startup), then poll.
        let mut cursors = seed_cursors_at_tail(&persist);
        poll_once_with_authorizer(&persist, &queue_path, &mut cursors, &authorizer);
        // The stale request is NOT replayed — the queue stays empty.
        assert!(queue::read_from(&queue_path).songs.is_empty());
        // A NEW request after seeding IS handled.
        let fresh_body = armed_test_body(
            r#"{"schema_version":1,"song_id":"fresh"}"#,
            "enqueue",
            "queue",
            "music-nonce-fresh",
        );
        let fresh = persist
            .write(
                "action/music/enqueue",
                Priority::Default,
                None,
                Some(&fresh_body),
            )
            .unwrap();
        poll_once_with_authorizer(&persist, &queue_path, &mut cursors, &authorizer);
        assert_eq!(queue::read_from(&queue_path).songs, vec!["fresh"]);
        assert!(persist
            .list_since(&reply_topic(&fresh.ulid), None)
            .unwrap()
            .iter()
            .any(|m| m.body.as_deref().unwrap_or("").contains("\"ok\":true")));
    }

    #[test]
    fn initial_poll_phase_is_stable_and_bounded() {
        let first = initial_poll_phase("seat-15");
        assert_eq!(first, initial_poll_phase("seat-15"));
        assert!(first < MAX_INITIAL_POLL_PHASE);
        assert!(initial_poll_phase("seat-16") < MAX_INITIAL_POLL_PHASE);
    }

    #[test]
    fn initial_state_write_phase_is_stable_bounded_and_independent() {
        let first = initial_state_write_phase("seat-15");
        assert_eq!(first, initial_state_write_phase("seat-15"));
        assert!(first <= MAX_STATE_WRITE_PHASE);
        assert!(initial_state_write_phase("seat-16") <= MAX_STATE_WRITE_PHASE);
        assert_ne!(first, initial_poll_phase("seat-15"));
    }

    #[test]
    fn idle_non_transport_cadences_are_slower_than_control_polling() {
        assert!(POLL_INTERVAL < BROWSE_POLL_INTERVAL);
        assert!(POLL_INTERVAL < CREDS_REFRESH_INTERVAL);
    }

    #[test]
    fn poll_once_round_trips_a_request() {
        let dir = tempfile::tempdir().unwrap();
        let persist = Persist::open(dir.path().join("bus")).unwrap();
        let queue_path = dir.path().join("queue.json");
        let authorizer = test_authorizer(dir.path());
        // A GUI publishes an enqueue request on the action topic.
        let body = armed_test_body(
            r#"{"schema_version":1,"song_id":"t1"}"#,
            "enqueue",
            "queue",
            "music-nonce-round-trip",
        );
        let req = persist
            .write("action/music/enqueue", Priority::Default, None, Some(&body))
            .unwrap();
        let mut cursors = HashMap::new();
        poll_once_with_authorizer(&persist, &queue_path, &mut cursors, &authorizer);
        // A reply landed on reply/<ulid> with ok:true.
        let replies = persist.list_since(&reply_topic(&req.ulid), None).unwrap();
        assert_eq!(replies.len(), 1);
        assert!(replies[0].body.as_deref().unwrap().contains("\"ok\":true"));
        // The queue was persisted with the enqueued track.
        assert_eq!(queue::read_from(&queue_path).songs, vec!["t1"]);
        // A second poll with the advanced cursor does nothing new.
        poll_once_with_authorizer(&persist, &queue_path, &mut cursors, &authorizer);
        assert_eq!(
            persist
                .list_since(&reply_topic(&req.ulid), None)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn queue_action_recovery_reads_a_bounded_page_and_advances_the_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let persist = Persist::open(dir.path().join("bus")).unwrap();
        let queue_path = dir.path().join("queue.json");
        let authorizer = test_authorizer(dir.path());
        let mut actions = Vec::new();
        for index in 0..(MAX_MESSAGES_PER_POLL + 1) {
            actions.push(
                persist
                    .write(
                        "action/music/enqueue",
                        Priority::Default,
                        None,
                        Some(&format!(r#"{{"song_id":"bounded-{index}"}}"#)),
                    )
                    .unwrap(),
            );
        }

        let mut cursors = HashMap::new();
        poll_once_with_authorizer(&persist, &queue_path, &mut cursors, &authorizer);
        let page_last = MAX_MESSAGES_PER_POLL - 1;
        assert_eq!(
            cursors.get("action/music/enqueue").map(String::as_str),
            Some(actions[page_last].ulid.as_str())
        );
        assert_eq!(
            persist
                .list_since(&reply_topic(&actions[page_last].ulid), None)
                .unwrap()
                .len(),
            1
        );
        assert!(persist
            .list_since(&reply_topic(&actions[MAX_MESSAGES_PER_POLL].ulid), None)
            .unwrap()
            .is_empty());

        poll_once_with_authorizer(&persist, &queue_path, &mut cursors, &authorizer);
        assert_eq!(
            cursors.get("action/music/enqueue").map(String::as_str),
            Some(actions[MAX_MESSAGES_PER_POLL].ulid.as_str())
        );
        assert_eq!(
            persist
                .list_since(&reply_topic(&actions[MAX_MESSAGES_PER_POLL].ulid), None)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn dispatch_clear_and_errors() {
        let mut q = Queue::default();
        let _ = dispatch_queue_action("enqueue", "a", &mut q);
        let c = dispatch_queue_action("clear", "", &mut q);
        assert!(c.mutated);
        assert!(q.is_empty());
        // Missing id.
        let e = dispatch_queue_action("enqueue", "", &mut q);
        assert!(e.reply_json.contains("\"ok\":false"));
        // Unknown verb.
        let u = dispatch_queue_action("frobnicate", "", &mut q);
        assert!(u.reply_json.contains("unknown verb"));
    }

    #[test]
    fn parse_transport_verbs() {
        assert_eq!(parse_transport("play", ""), Some(TransportCommand::Play));
        assert_eq!(parse_transport("pause", ""), Some(TransportCommand::Pause));
        assert_eq!(
            parse_transport("resume", ""),
            Some(TransportCommand::Resume)
        );
        assert_eq!(parse_transport("stop", ""), Some(TransportCommand::Stop));
        assert_eq!(
            parse_transport("get-state", ""),
            Some(TransportCommand::GetState)
        );
        assert_eq!(parse_transport("teleport", ""), None);
    }

    #[test]
    fn parse_transport_seek_forms() {
        // MUSIC-RFX-2 — bare ms, {"position_ms":N}, {"ms":N}.
        assert_eq!(
            parse_transport("seek", "42000"),
            Some(TransportCommand::Seek(42_000))
        );
        assert_eq!(
            parse_transport("seek", r#"{"position_ms":1500}"#),
            Some(TransportCommand::Seek(1_500))
        );
        assert_eq!(
            parse_transport("seek", r#"{"ms":250}"#),
            Some(TransportCommand::Seek(250))
        );
        // Non-numeric body → no command.
        assert_eq!(parse_transport("seek", "middle"), None);
    }

    #[test]
    fn get_state_reports_seekable_and_seek_needs_engine() {
        // MUSIC-RFX-2 — get-state carries a `seekable` flag (false with no
        // engine), and a seek without an engine is refused like other mutators.
        let queue = queue::Queue::default();
        let state = apply_transport("get-state", "", None, None, &queue);
        let sv: serde_json::Value = serde_json::from_str(&state).unwrap();
        assert_eq!(sv["seekable"], false);

        let seek = apply_transport("seek", "1000", None, None, &queue);
        let kv: serde_json::Value = serde_json::from_str(&seek).unwrap();
        assert_eq!(kv["ok"], false);
    }

    #[test]
    fn typed_radio_live_transport_refuses_seek_and_idle_resume_without_inventing_position() {
        assert_eq!(resumable_position(false, 12_345), None);
        assert_eq!(resumable_position(true, 12_345), Some(12_345));

        let rejected: Value = serde_json::from_str(&seek_transport_reply(false, 12_345)).unwrap();
        assert_eq!(rejected["ok"], false);
        assert_eq!(rejected["seeked"], false);
        assert_eq!(rejected["error"], "not_seekable");
        assert_eq!(rejected["position_ms"], 12_345);
        assert_eq!(
            accepted_json(&seek_transport_reply(false, 12_345)),
            Err("not_seekable")
        );
        assert_eq!(
            accepted_json(r#"{"ok":false,"error":"nothing_to_resume"}"#),
            Err("nothing_to_resume")
        );

        let accepted: Value = serde_json::from_str(&seek_transport_reply(true, 42_000)).unwrap();
        assert_eq!(accepted["ok"], true);
        assert_eq!(accepted["seeked"], true);
        assert_eq!(accepted["position_ms"], 42_000);
    }

    #[test]
    fn parse_transport_set_volume_forms() {
        // bare number, JSON object, and an out-of-range value (engine clamps).
        assert_eq!(
            parse_transport("set-volume", "0.6"),
            Some(TransportCommand::SetVolume(0.6))
        );
        assert_eq!(
            parse_transport("set-volume", r#"{"volume":0.25}"#),
            Some(TransportCommand::SetVolume(0.25))
        );
        assert_eq!(
            parse_transport("set-volume", "2"),
            Some(TransportCommand::SetVolume(2.0))
        );
        // Non-numeric body → no command.
        assert_eq!(parse_transport("set-volume", "loud"), None);
    }

    #[test]
    fn get_state_is_answered_without_engine_or_creds() {
        // AUDIT-MESH-4: a headless peer with no audio device + no Airsonic creds
        // must still answer get-state with ok:true and honest capability flags
        // (so the panel shows "configure Airsonic" / "no audio device" rather
        // than a silent blank). Mutating verbs still return the no-audio error.
        let queue = queue::Queue::default();
        let reply = apply_transport("get-state", "", None, None, &queue);
        let v: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["active"], false);
        assert_eq!(v["playing"], false);
        assert_eq!(v["audio_available"], false);
        assert_eq!(v["needs_airsonic"], true);

        // A mutating verb without an engine is still refused.
        let play = apply_transport("play", "", None, None, &queue);
        let pv: serde_json::Value = serde_json::from_str(&play).unwrap();
        assert_eq!(pv["ok"], false);
    }

    #[test]
    fn workspace_targets_only_project_a_proven_local_seat() {
        assert!(local_playback_targets(false).is_empty());
        let targets = local_playback_targets(true);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].kind, "local_seat");
        assert!(targets[0].available);
        assert!(targets[0].id.starts_with("local-seat:"));
    }

    #[test]
    fn workspace_targets_project_fresh_idle_and_refused_peer_heartbeats() {
        let dir = tempfile::tempdir().unwrap();
        let now = state::now_ms();
        state::write_state(
            dir.path(),
            &state::MusicState {
                peer: "seat-15".into(),
                playing: false,
                song_id: String::new(),
                position_ms: 0,
                updated_ms: now,
            },
        )
        .unwrap();
        state::write_state(
            dir.path(),
            &state::MusicState {
                peer: "seat-16".into(),
                playing: true,
                song_id: "song-1".into(),
                position_ms: 1200,
                updated_ms: now,
            },
        )
        .unwrap();
        state::write_state(
            dir.path(),
            &state::MusicState {
                peer: "seat-17".into(),
                playing: false,
                song_id: String::new(),
                position_ms: 0,
                updated_ms: now.saturating_sub(state::STATE_STALE_MS + 1),
            },
        )
        .unwrap();

        let targets = playback_targets(true, dir.path());
        let target = |peer: &str| targets.iter().find(|item| item.id == peer).unwrap();
        assert!(target("seat-15").available);
        assert!(!target("seat-16").available);
        assert_eq!(
            target("seat-16").unavailable_reason.as_deref(),
            Some("peer currently owns playback")
        );
        assert!(!target("seat-17").available);
        assert_eq!(
            target("seat-17").unavailable_reason.as_deref(),
            Some("peer heartbeat is stale")
        );
    }

    #[test]
    fn offline_play_requires_every_upcoming_track_to_be_cached() {
        let dir = tempfile::tempdir().unwrap();
        let queue = queue::Queue {
            songs: vec!["song-a".to_string(), "song-b".to_string()],
            current: 0,
        };

        crate::cache::write_cached_track(dir.path(), "song-a", "flac", b"cached-a", 10, false)
            .unwrap();
        assert!(
            cached_upcoming_tracks(&queue, dir.path()).is_none(),
            "offline playback must not start a partial queued tail"
        );

        crate::cache::write_cached_track(dir.path(), "song-b", "mp3", b"cached-b", 11, false)
            .unwrap();
        let tracks = cached_upcoming_tracks(&queue, dir.path()).expect("fully cached queue");
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].1, SourceCodec::Flac);
        assert_eq!(tracks[1].1, SourceCodec::Mp3);
        assert!(tracks[0].0.starts_with("mde-cache:"));
    }

    #[test]
    fn source_aware_playback_keeps_two_catalog_candidates_under_one_queue_track() {
        let first = Client::with_salt("http://one.test", "alice", "pw", "one");
        let second = Client::with_salt("http://two.test", "alice", "pw", "two");
        let first_source = catalog_source_id(&first);
        let second_source = catalog_source_id(&second);
        let catalog = CatalogStoreFile {
            songs: vec![CatalogItem {
                id: "music|song".to_string(),
                kind: ContentKind::Music,
                title: "Song".to_string(),
                creator: "Artist".to_string(),
                parent_title: "Album".to_string(),
                duration_ms: Some(180_000),
                artwork_ref: None,
                starred: false,
                cached: false,
                variants: vec![
                    SourceVariant {
                        content: ContentRef::new(&first_source, "song-7", ContentKind::Music)
                            .unwrap(),
                        cached: false,
                        reachable: true,
                        operator_priority: 1,
                        latency_ms: Some(20),
                    },
                    SourceVariant {
                        content: ContentRef::new(&second_source, "song-7", ContentKind::Music)
                            .unwrap(),
                        cached: false,
                        reachable: true,
                        operator_priority: 9,
                        latency_ms: Some(5),
                    },
                ],
            }],
            ..CatalogStoreFile::default()
        };
        let queue = queue::Queue {
            songs: vec!["song-7".to_string()],
            current: 0,
        };
        let clients = vec![&first, &second];
        let candidates = source_aware_upcoming_candidates(&queue, &clients, &catalog)
            .expect("two admitted source candidates");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].candidates.len(), 2);
        assert!(candidates[0].candidates[0]
            .0
            .starts_with("http://two.test/"));
        assert!(candidates[0].candidates[1]
            .0
            .starts_with("http://one.test/"));
    }

    #[test]
    fn source_aware_playback_resolves_selected_variant_to_its_client() {
        let first = Client::with_salt("http://one.test", "alice", "pw", "one");
        let second = Client::with_salt("http://two.test", "alice", "pw", "two");
        let second_source = catalog_source_id(&second);
        let catalog = CatalogStoreFile {
            songs: vec![CatalogItem {
                id: "music|song".to_string(),
                kind: ContentKind::Music,
                title: "Song".to_string(),
                creator: "Artist".to_string(),
                parent_title: "Album".to_string(),
                duration_ms: Some(180_000),
                artwork_ref: None,
                starred: false,
                cached: false,
                variants: vec![SourceVariant {
                    content: ContentRef::new(&second_source, "song-7", ContentKind::Music).unwrap(),
                    cached: false,
                    reachable: true,
                    operator_priority: 9,
                    latency_ms: Some(5),
                }],
            }],
            ..CatalogStoreFile::default()
        };
        let queue = queue::Queue {
            songs: vec!["song-7".to_string()],
            current: 0,
        };
        let clients = vec![&first, &second];
        let tracks = source_aware_upcoming_tracks(&queue, &clients, &catalog)
            .expect("admitted source variant");
        assert!(tracks[0].0.starts_with("http://two.test/"));
    }

    #[test]
    fn workspace_queue_projection_preserves_source_variant_identity() {
        let source_id = "airsonic:http://two.test";
        let catalog = CatalogStoreFile {
            songs: vec![CatalogItem {
                id: "music|song".to_string(),
                kind: ContentKind::Music,
                title: "Song".to_string(),
                creator: "Artist".to_string(),
                parent_title: "Album".to_string(),
                duration_ms: Some(180_000),
                artwork_ref: None,
                starred: false,
                cached: false,
                variants: vec![SourceVariant {
                    content: ContentRef::new(source_id, "song-7", ContentKind::Music).unwrap(),
                    cached: false,
                    reachable: true,
                    operator_priority: 1,
                    latency_ms: Some(5),
                }],
            }],
            bookmarks: vec![BookmarkItem {
                content: ContentRef::new(
                    "airsonic:http://one.test",
                    "episode-7",
                    ContentKind::Episode,
                )
                .unwrap(),
                title: "Episode".into(),
                creator: "Host".into(),
                parent_title: "Show".into(),
                position_ms: 1_000,
                duration_ms: Some(10_000),
                artwork_ref: None,
            }],
            ..CatalogStoreFile::default()
        };
        let projected = workspace_content_ref(&catalog, "song-7");
        assert_eq!(projected.source_id, source_id);
        assert_eq!(projected.remote_id, "song-7");
        let bookmark_projected = workspace_content_ref(&catalog, "episode-7");
        assert_eq!(bookmark_projected.source_id, "airsonic:http://one.test");
        assert_eq!(bookmark_projected.kind, ContentKind::Episode);
        assert_eq!(
            workspace_content_ref(&catalog, "legacy-only").source_id,
            "legacy"
        );
    }

    #[test]
    fn catalog_source_health_updates_all_retained_variant_views() {
        let source_id = "airsonic:http://one.test";
        let variant = catalog_variant(source_id, "song-7", ContentKind::Music).unwrap();
        let item = CatalogItem {
            id: "music|song".to_string(),
            kind: ContentKind::Music,
            title: "Song".to_string(),
            creator: "Artist".to_string(),
            parent_title: "Album".to_string(),
            duration_ms: None,
            artwork_ref: None,
            starred: false,
            cached: false,
            variants: vec![variant],
        };
        let mut file = CatalogStoreFile {
            songs: vec![item.clone()],
            search: Some(SearchPage {
                generation: 1,
                query: "song".to_string(),
                groups: [(ContentKind::Music, vec![item])].into_iter().collect(),
                has_more: false,
            }),
            ..CatalogStoreFile::default()
        };

        set_catalog_variants_reachable(&mut file, source_id, false);
        assert!(!file.songs[0].variants[0].reachable);
        assert!(
            !file.search.as_ref().unwrap().groups[&ContentKind::Music][0].variants[0].reachable
        );
        set_catalog_variants_reachable(&mut file, source_id, true);
        assert!(file.songs[0].variants[0].reachable);
    }

    #[test]
    fn catalog_source_records_authentication_required_separately_from_outage() {
        let dir = tempfile::tempdir().unwrap();
        let client = Client::with_salt("http://one.test", "alice", "pw", "auth-state");
        record_catalog_response(
            dir.path(),
            &client,
            "list-artists",
            "",
            r#"{"ok":false,"error":{"code":40,"message":"authentication required"}}"#,
        );
        let file = load_catalog(dir.path()).unwrap();
        assert_eq!(file.sources.len(), 1);
        assert!(!file.sources[0].reachable);
        assert!(file.sources[0].authentication_required);

        record_catalog_response(
            dir.path(),
            &client,
            "list-artists",
            "",
            r#"{"ok":true,"result":{"artists":[]}}"#,
        );
        let file = load_catalog(dir.path()).unwrap();
        assert!(file.sources[0].reachable);
        assert!(!file.sources[0].authentication_required);
        assert!(file.sources[0].features.contains("list-artists"));
    }

    #[test]
    fn interrupted_download_recovery_clears_phantom_progress() {
        let data_dir = tempfile::tempdir().unwrap();
        let interrupted = DownloadRecord {
            content: ContentRef::new("legacy", "song-interrupted", ContentKind::Music).unwrap(),
            state: "downloading".into(),
            bytes: 4096,
            total_bytes: Some(8192),
            pinned: true,
            error_code: None,
        };
        let ready = DownloadRecord {
            content: ContentRef::new("legacy", "song-ready", ContentKind::Music).unwrap(),
            state: "ready".into(),
            bytes: 12,
            total_bytes: Some(12),
            pinned: true,
            error_code: None,
        };
        persist_downloads(data_dir.path(), &[interrupted, ready]).unwrap();

        assert!(recover_interrupted_downloads(data_dir.path()).unwrap());
        let records = load_downloads(data_dir.path()).unwrap();
        assert_eq!(records[0].state, "failed");
        assert_eq!(records[0].bytes, 0);
        assert_eq!(records[0].total_bytes, None);
        assert!(!records[0].pinned);
        assert_eq!(
            records[0].error_code.as_deref(),
            Some("download_interrupted")
        );
        assert_eq!(records[1].state, "ready");
        assert!(!recover_interrupted_downloads(data_dir.path()).unwrap());
    }

    #[test]
    fn download_empty_response_retains_a_redacted_failed_record() {
        let data_dir = tempfile::tempdir().unwrap();
        let cache_dir = tempfile::tempdir().unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let request = MusicActionRequestV1 {
            schema_version: MUSIC_CONTRACT_VERSION,
            request_id: "download-empty".into(),
            action: "download".into(),
            content: Some(ContentRef::new("legacy", "song-empty", ContentKind::Music).unwrap()),
            expected_queue_revision: None,
            position_ms: None,
            volume_milli: None,
            shuffle: None,
            repeat: None,
            queue_index: None,
            target_queue_index: None,
            target_peer: None,
            playlist: None,
            playlist_name: None,
            playlist_song_ids: Vec::new(),
            playlist_remove_indices: Vec::new(),
            armed_token: None,
        };
        let client = Client::with_salt(
            one_shot_bytes_server(b""),
            "alice",
            "pw-do-not-persist",
            "download-empty",
        );
        assert_eq!(
            download_to_cache(&request, &client, &rt, data_dir.path(), cache_dir.path()),
            Err("download_empty")
        );
        let records = load_downloads(data_dir.path()).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].state, "failed");
        assert_eq!(records[0].bytes, 0);
        assert_eq!(records[0].error_code.as_deref(), Some("download_empty"));
        assert!(!serde_json::to_string(&records)
            .unwrap()
            .contains("pw-do-not-persist"));
    }

    #[test]
    fn workspace_ledger_is_bounded_and_replays_without_side_effect() {
        let dir = tempfile::tempdir().unwrap();
        let mut ledger = WorkspaceActionLedger::default();
        assert!(ledger.reserve("request-0", 7));
        ledger.finish("request-0", typed_result("request-0", true, 8, None));
        assert!(!ledger.reserve("request-0", 9));

        for index in 1..=MAX_WORKSPACE_LEDGER_RECORDS {
            assert!(ledger.reserve(&format!("request-{index}"), index as u64));
        }
        assert_eq!(ledger.records.len(), MAX_WORKSPACE_LEDGER_RECORDS);
        assert!(!ledger.contains("request-0"));
        assert!(ledger.contains("request-1024"));

        persist_workspace_ledger(dir.path(), &ledger).unwrap();
        let restored = load_workspace_ledger(dir.path()).unwrap();
        assert_eq!(restored.records.len(), MAX_WORKSPACE_LEDGER_RECORDS);
        assert!(restored.contains("request-1024"));
    }

    #[test]
    fn retained_music_json_is_private_and_leaves_no_temporary_record() {
        let dir = tempfile::tempdir().unwrap();
        let mut ledger = WorkspaceActionLedger::default();
        assert!(ledger.reserve("private-state", 1));
        persist_workspace_ledger(dir.path(), &ledger).unwrap();
        persist_workspace_ledger(dir.path(), &ledger).unwrap();

        let entries = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(
            entries,
            vec![std::ffi::OsString::from(WORKSPACE_LEDGER_FILE)]
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join(WORKSPACE_LEDGER_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn successful_browse_replies_persist_typed_catalog_and_search_state() {
        let dir = tempfile::tempdir().unwrap();
        let client = Client::with_salt("http://music.test", "alice", "pw", "catalog");
        let albums = r#"{
            "ok":true,
            "result":{"albums":[{"id":"album-1","name":"Blue","artist":"Artist","songCount":2,"coverArt":"art-1"}]}
        }"#;
        record_catalog_response(dir.path(), &client, "list-albums", "", albums);
        let search = r#"{
            "ok":true,
            "result":{"artists":[{"id":"artist-1","name":"Artist","albumCount":1}],"albums":[],"songs":[{"id":"song-1","title":"Blue Sky","artist":"Artist","album":"Blue","duration":42}]}
        }"#;
        record_catalog_response(
            dir.path(),
            &client,
            "search",
            "{\"query\":\"blue\"}",
            search,
        );

        let file = load_catalog(dir.path()).unwrap();
        assert_eq!(file.sources.len(), 1);
        assert_eq!(file.sources[0].source_id, "airsonic:http://music.test");
        assert_eq!(file.albums.len(), 1);
        assert_eq!(file.albums[0].variants[0].content.remote_id, "album-1");
        assert_eq!(file.pages["albums"].offset, 0);
        assert_eq!(file.pages["albums"].size, 100);
        assert!(!file.pages["albums"].has_more);
        assert_eq!(file.search.as_ref().unwrap().query, "blue");
        assert_eq!(file.search.as_ref().unwrap().groups.len(), 2);
        assert!(file.artists.iter().any(|item| item.title == "Artist"));
        assert!(file.songs.iter().any(|item| item.title == "Blue Sky"));
    }

    #[test]
    fn cover_art_path_is_projected_into_cached_catalog_and_page_rows() {
        let dir = tempfile::tempdir().unwrap();
        let client = Client::with_salt("http://music.test", "alice", "pw", "art-path");
        record_catalog_response(
            dir.path(),
            &client,
            "list-albums",
            r#"{"offset":0,"size":100}"#,
            r#"{"ok":true,"result":{"albums":[{"id":"album-1","name":"Blue","artist":"Artist","coverArt":"art-1"}],"offset":0,"size":100,"has_more":false}}"#,
        );
        record_catalog_response(
            dir.path(),
            &client,
            "get-cover-art",
            r#"{"id":"art-1"}"#,
            r#"{"ok":true,"result":{"path":"/var/cache/mde/music/artwork/art-1.jpg","bytes":42}}"#,
        );

        let file = load_catalog(dir.path()).unwrap();
        assert_eq!(
            file.albums[0].artwork_ref.as_deref(),
            Some("/var/cache/mde/music/artwork/art-1.jpg")
        );
        assert_eq!(
            file.pages["albums"].items[0].artwork_ref.as_deref(),
            Some("/var/cache/mde/music/artwork/art-1.jpg")
        );
    }

    #[test]
    fn provider_artwork_tokens_and_paths_retain_for_podcast_radio_episode() {
        let dir = tempfile::tempdir().unwrap();
        let client = Client::with_salt("http://music.test", "alice", "pw", "art-hubs");
        record_catalog_response(
            dir.path(),
            &client,
            "list-podcasts",
            "",
            r#"{"ok":true,"result":{"podcasts":[{"id":"feed-1","title":"Mesh Weekly","coverArt":"feed-art"}]}}"#,
        );
        record_catalog_response(
            dir.path(),
            &client,
            "list-radio",
            "",
            r#"{"ok":true,"result":{"radio":[{"id":"station-1","name":"Mesh FM","streamUrl":"https://radio.test/live","coverArt":"radio-art"}]}}"#,
        );
        record_catalog_response(
            dir.path(),
            &client,
            "podcast-episodes",
            r#"{"id":"feed-1"}"#,
            r#"{"ok":true,"result":{"episodes":[{"id":"episode-1","title":"Episode 1","coverArt":"episode-art"}]}}"#,
        );

        let file = load_catalog(dir.path()).unwrap();
        assert_eq!(file.podcasts[0].artwork_ref.as_deref(), Some("feed-art"));
        assert_eq!(file.radio[0].artwork_ref.as_deref(), Some("radio-art"));
        assert_eq!(file.episodes[0].artwork_ref.as_deref(), Some("episode-art"));
        assert_eq!(file.episodes[0].parent_title, "Mesh Weekly");

        for (cover_id, path) in [
            ("feed-art", "/var/cache/mde/music/artwork/feed-art.jpg"),
            ("radio-art", "/var/cache/mde/music/artwork/radio-art.jpg"),
            (
                "episode-art",
                "/var/cache/mde/music/artwork/episode-art.jpg",
            ),
        ] {
            record_catalog_response(
                dir.path(),
                &client,
                "get-cover-art",
                &format!(r#"{{"id":"{cover_id}"}}"#),
                &format!(r#"{{"ok":true,"result":{{"path":"{path}"}}}}"#),
            );
        }

        let file = load_catalog(dir.path()).unwrap();
        assert_eq!(
            file.podcasts[0].artwork_ref.as_deref(),
            Some("/var/cache/mde/music/artwork/feed-art.jpg")
        );
        assert_eq!(
            file.radio[0].artwork_ref.as_deref(),
            Some("/var/cache/mde/music/artwork/radio-art.jpg")
        );
        assert_eq!(
            file.episodes[0].artwork_ref.as_deref(),
            Some("/var/cache/mde/music/artwork/episode-art.jpg")
        );
    }

    #[test]
    fn provider_hub_browse_rows_reach_typed_workspace_collections() {
        let dir = tempfile::tempdir().unwrap();
        let client = Client::with_salt("http://music.test", "alice", "pw", "hubs");
        record_catalog_response(
            dir.path(),
            &client,
            "list-podcasts",
            "",
            r#"{"ok":true,"result":{"podcasts":[{"id":"feed-1","title":"Mesh Weekly"}]}}"#,
        );
        record_catalog_response(
            dir.path(),
            &client,
            "list-radio",
            "",
            r#"{"ok":true,"result":{"radio":[{"id":"station-1","name":"Mesh FM","streamUrl":"https://radio.test/live"}]}}"#,
        );

        let file = load_catalog(dir.path()).unwrap();
        assert_eq!(file.podcasts.len(), 1);
        assert_eq!(file.podcasts[0].kind, ContentKind::Podcast);
        assert_eq!(file.radio.len(), 1);
        assert_eq!(file.radio[0].kind, ContentKind::Radio);
        assert_eq!(
            file.radio[0].variants[0].content.source_id,
            "airsonic:http://music.test"
        );
        assert_eq!(
            file.radio[0].variants[0].content.remote_id,
            "https://radio.test/live"
        );

        let snapshot = workspace_snapshot_from_dir(&Queue::default(), None, 9, dir.path());
        assert!(snapshot
            .collections
            .iter()
            .any(|collection| collection.kind == ContentKind::Podcast));
        assert!(snapshot
            .collections
            .iter()
            .any(|collection| collection.kind == ContentKind::Radio));
    }

    #[test]
    fn artist_album_and_podcast_detail_replies_retain_bounded_typed_rows() {
        let dir = tempfile::tempdir().unwrap();
        let client = Client::with_salt("http://music.test", "alice", "pw", "detail");

        // The exact Subsonic getArtist endpoint is exposed by the daemon as
        // both `get-artist` and its older `albums-by-artist` alias. Both
        // replies must feed the same source-qualified album projection.
        assert!(BROWSE_VERBS.contains(&"get-artist"));
        record_catalog_response(
            dir.path(),
            &client,
            "get-artist",
            r#"{"id":"artist-1"}"#,
            r#"{"ok":true,"result":{"albums":[{"id":"album-1","name":"Blue","artist":"Artist","artistId":"artist-1","songCount":2}]}}"#,
        );
        record_catalog_response(
            dir.path(),
            &client,
            "get-album",
            r#"{"id":"album-1"}"#,
            r#"{"ok":true,"result":{"album":{"id":"album-1","name":"Blue","artist":"Artist","artistId":"artist-1","songCount":2},"songs":[{"id":"song-1","title":"Blue Sky","artist":"Artist","album":"Blue","duration":42}]}}"#,
        );
        record_catalog_response(
            dir.path(),
            &client,
            "list-podcasts",
            "",
            r#"{"ok":true,"result":{"podcasts":[{"id":"feed-1","title":"Mesh Weekly"}]}}"#,
        );

        let episodes = (0..MAX_CATALOG_ITEMS + 3)
            .map(|index| {
                json!({
                    "id": format!("episode-{index}"),
                    "title": format!("Episode {index}")
                })
            })
            .collect::<Vec<_>>();
        record_catalog_response(
            dir.path(),
            &client,
            "podcast-episodes",
            r#"{"id":"feed-1"}"#,
            &json!({ "ok": true, "result": { "episodes": episodes } }).to_string(),
        );

        let file = load_catalog(dir.path()).unwrap();
        assert_eq!(file.albums.len(), 1);
        assert_eq!(
            file.albums[0].variants[0].content,
            ContentRef::new("airsonic:http://music.test", "album-1", ContentKind::Album).unwrap()
        );
        assert!(file.songs.iter().any(|item| {
            item.kind == ContentKind::Music
                && item
                    .variants
                    .iter()
                    .any(|variant| variant.content.remote_id == "song-1")
        }));
        assert_eq!(file.episodes.len(), MAX_CATALOG_ITEMS);
        assert!(file.episodes.iter().all(|item| {
            item.kind == ContentKind::Episode
                && item.parent_title == "Mesh Weekly"
                && item.variants.len() == 1
                && item.variants[0].content.source_id == "airsonic:http://music.test"
        }));
        assert_eq!(file.episodes[0].variants[0].content.remote_id, "episode-0");

        let snapshot = workspace_snapshot_from_dir(&Queue::default(), None, 10, dir.path());
        let episodes = snapshot
            .collections
            .iter()
            .find(|collection| collection.kind == ContentKind::Episode)
            .expect("episode collection is retained");
        assert_eq!(episodes.items.len(), MAX_COLLECTION_ITEMS);
        assert!(snapshot.validate().is_ok());
        assert_eq!(
            workspace_content_ref(&file, "episode-0"),
            ContentRef::new(
                "airsonic:http://music.test",
                "episode-0",
                ContentKind::Episode
            )
            .unwrap()
        );
    }

    #[test]
    fn multi_source_catalog_fanout_merges_rows_and_keeps_source_identity() {
        let merged = merge_browse_replies(
            "search",
            vec![
                (
                    "airsonic:http://one.test".into(),
                    r#"{"ok":true,"result":{"albums":[{"id":"same","name":"Blue","artist":"Artist"}]}}"#.into(),
                ),
                (
                    "airsonic:http://two.test".into(),
                    r#"{"ok":true,"result":{"albums":[{"id":"same","name":"Blue","artist":"Artist"}]}}"#.into(),
                ),
            ],
        );
        let value: Value = serde_json::from_str(&merged).unwrap();
        let albums = value["result"]["albums"].as_array().unwrap();
        assert_eq!(albums.len(), 2);
        assert_eq!(albums[0]["source_id"], "airsonic:http://one.test");
        assert_eq!(albums[1]["source_id"], "airsonic:http://two.test");
    }

    #[test]
    fn typed_play_selection_uses_requested_admitted_source_variant() {
        let cache_dir = tempfile::tempdir().unwrap();
        let first = Client::with_salt("http://one.test", "alice", "pw", "play-one");
        let second = Client::with_salt("http://two.test", "alice", "pw", "play-two");
        let selected =
            ContentRef::new("airsonic:http://two.test", "song-a", ContentKind::Music).unwrap();
        let catalog = CatalogStoreFile {
            songs: vec![CatalogItem {
                id: "song-a".into(),
                kind: ContentKind::Music,
                title: "Blue Sky".into(),
                creator: "Artist".into(),
                parent_title: "Blue".into(),
                duration_ms: Some(42_000),
                artwork_ref: None,
                starred: false,
                cached: false,
                variants: vec![
                    SourceVariant {
                        content: ContentRef::new(
                            "airsonic:http://one.test",
                            "song-a",
                            ContentKind::Music,
                        )
                        .unwrap(),
                        cached: false,
                        reachable: true,
                        operator_priority: 0,
                        latency_ms: None,
                    },
                    SourceVariant {
                        content: selected.clone(),
                        cached: false,
                        reachable: true,
                        operator_priority: 0,
                        latency_ms: None,
                    },
                ],
            }],
            ..CatalogStoreFile::default()
        };
        let queue = Queue {
            songs: vec!["song-a".into(), "song-b".into()],
            current: 0,
        };
        let clients = vec![&first, &second];
        let tracks = selected_source_upcoming_candidates(
            &queue,
            &selected,
            &clients,
            &catalog,
            cache_dir.path(),
        )
        .unwrap();
        assert!(tracks[0].candidates[0].0.starts_with("http://two.test/"));
        assert!(tracks[0]
            .candidates
            .iter()
            .any(|(url, _)| url.starts_with("http://one.test/")));

        let untrusted =
            ContentRef::new("airsonic:http://unknown.test", "song-a", ContentKind::Music).unwrap();
        assert_eq!(
            selected_source_upcoming_candidates(
                &queue,
                &untrusted,
                &clients,
                &catalog,
                cache_dir.path(),
            ),
            Err("unsupported_source")
        );
    }

    #[test]
    fn typed_play_selection_accepts_an_admitted_bookmark_audio_variant() {
        let cache_dir = tempfile::tempdir().unwrap();
        let first = Client::with_salt("http://one.test", "alice", "pw", "bookmark-play-one");
        let second = Client::with_salt("http://two.test", "alice", "pw", "bookmark-play-two");
        let selected = ContentRef::new(
            "airsonic:http://two.test",
            "episode-a",
            ContentKind::Episode,
        )
        .unwrap();
        let catalog = CatalogStoreFile {
            bookmarks: vec![BookmarkItem {
                content: selected.clone(),
                title: "Episode A".into(),
                creator: "Host".into(),
                parent_title: "Show".into(),
                position_ms: 12_000,
                duration_ms: Some(90_000),
                artwork_ref: None,
            }],
            ..CatalogStoreFile::default()
        };
        let queue = Queue {
            songs: vec!["episode-a".into()],
            current: 0,
        };
        let clients = vec![&first, &second];
        let tracks = selected_source_upcoming_candidates(
            &queue,
            &selected,
            &clients,
            &catalog,
            cache_dir.path(),
        )
        .expect("admitted bookmark should resolve through the typed source policy");
        assert!(tracks[0].candidates[0].0.starts_with("http://two.test/"));

        let untrusted = ContentRef::new(
            "airsonic:http://unknown.test",
            "episode-a",
            ContentKind::Episode,
        )
        .unwrap();
        assert_eq!(
            selected_source_upcoming_candidates(
                &queue,
                &untrusted,
                &clients,
                &catalog,
                cache_dir.path(),
            ),
            Err("unsupported_source")
        );
    }

    #[test]
    fn typed_radio_play_selection_accepts_an_admitted_stream_url() {
        let cache_dir = tempfile::tempdir().unwrap();
        let client = Client::with_salt("http://radio.test", "alice", "pw", "radio-play");
        let selected = ContentRef::new(
            "airsonic:http://radio.test",
            "https://stream.test/cspan-live",
            ContentKind::Radio,
        )
        .unwrap();
        let catalog = CatalogStoreFile {
            radio: vec![CatalogItem {
                id: "radio-cspan".into(),
                kind: ContentKind::Radio,
                title: "C-SPAN Radio".into(),
                creator: "Internet radio".into(),
                parent_title: String::new(),
                duration_ms: None,
                artwork_ref: None,
                starred: false,
                cached: false,
                variants: vec![SourceVariant {
                    content: selected.clone(),
                    cached: false,
                    reachable: true,
                    operator_priority: 0,
                    latency_ms: None,
                }],
            }],
            ..CatalogStoreFile::default()
        };
        let queue = Queue {
            songs: vec![selected.remote_id.clone()],
            current: 0,
        };

        let tracks = selected_source_upcoming_candidates(
            &queue,
            &selected,
            &[&client],
            &catalog,
            cache_dir.path(),
        )
        .expect("an admitted radio station must resolve to its direct stream");
        assert_eq!(
            tracks[0].candidates,
            vec![(
                "https://stream.test/cspan-live".to_string(),
                SourceCodec::Unknown,
            )]
        );
        assert!(!tracks[0].candidates[0].0.contains("/rest/stream"));
        assert_eq!(typed_play_start_position(&selected, None), Ok(0));
        assert_eq!(
            typed_play_start_position(&selected, Some(1)),
            Err("not_seekable")
        );
    }

    #[test]
    fn typed_radio_play_rejects_unretained_mismatched_and_missing_urls() {
        let cache_dir = tempfile::tempdir().unwrap();
        let client = Client::with_salt("http://radio.test", "alice", "pw", "radio-admission");
        let retained = ContentRef::new(
            "airsonic:http://radio.test",
            "https://stream.test/retained",
            ContentKind::Radio,
        )
        .unwrap();
        let retained_item = |content: ContentRef| CatalogItem {
            id: "radio-station".into(),
            kind: ContentKind::Radio,
            title: "Internet Radio".into(),
            creator: "Internet radio".into(),
            parent_title: String::new(),
            duration_ms: None,
            artwork_ref: None,
            starred: false,
            cached: false,
            variants: vec![SourceVariant {
                content,
                cached: false,
                reachable: true,
                operator_priority: 0,
                latency_ms: None,
            }],
        };
        let catalog = CatalogStoreFile {
            radio: vec![retained_item(retained.clone())],
            ..CatalogStoreFile::default()
        };
        let queue = Queue {
            songs: vec![retained.remote_id.clone()],
            current: 0,
        };

        for rejected in [
            ContentRef::new(
                &retained.source_id,
                "https://stream.test/not-retained",
                ContentKind::Radio,
            )
            .unwrap(),
            ContentRef::new(
                "airsonic:http://other.test",
                &retained.remote_id,
                ContentKind::Radio,
            )
            .unwrap(),
        ] {
            assert_eq!(
                selected_source_upcoming_candidates(
                    &queue,
                    &rejected,
                    &[&client],
                    &catalog,
                    cache_dir.path(),
                ),
                Err("unsupported_source")
            );
        }

        for missing_or_non_url in ["", "station-id"] {
            let malformed = ContentRef {
                source_id: retained.source_id.clone(),
                remote_id: missing_or_non_url.to_string(),
                kind: ContentKind::Radio,
            };
            let malformed_catalog = CatalogStoreFile {
                radio: vec![retained_item(malformed.clone())],
                ..CatalogStoreFile::default()
            };
            let malformed_queue = Queue {
                songs: vec![malformed.remote_id.clone()],
                current: 0,
            };
            assert_eq!(
                selected_source_upcoming_candidates(
                    &malformed_queue,
                    &malformed,
                    &[&client],
                    &malformed_catalog,
                    cache_dir.path(),
                ),
                Err("invalid_stream_url")
            );
            assert_eq!(
                source_aware_upcoming_candidates(&malformed_queue, &[&client], &malformed_catalog,),
                None,
                "malformed retained radio must not fall through to /rest/stream",
            );
        }
    }

    #[test]
    fn typed_workspace_queue_actions_use_the_shared_queue_authority() {
        let dir = tempfile::tempdir().unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut queue = Queue {
            songs: vec!["a".into(), "b".into(), "c".into()],
            current: 0,
        };
        let mut move_request = MusicActionRequestV1 {
            schema_version: MUSIC_CONTRACT_VERSION,
            request_id: "move-1".into(),
            action: "queue_move".into(),
            content: None,
            expected_queue_revision: None,
            position_ms: None,
            volume_milli: None,
            shuffle: None,
            repeat: None,
            queue_index: Some(2),
            target_queue_index: Some(1),
            target_peer: None,
            playlist: None,
            playlist_name: None,
            playlist_song_ids: Vec::new(),
            playlist_remove_indices: Vec::new(),
            armed_token: None,
        };
        assert_eq!(
            apply_workspace_action(&move_request, &mut queue, None, None, &rt, dir.path()),
            Ok(true)
        );
        assert_eq!(queue.songs, ["a", "c", "b"]);

        move_request.action = "queue_remove".into();
        move_request.queue_index = Some(1);
        move_request.target_queue_index = None;
        assert_eq!(
            apply_workspace_action(&move_request, &mut queue, None, None, &rt, dir.path()),
            Ok(true)
        );
        assert_eq!(queue.songs, ["a", "b"]);

        move_request.action = "shuffle".into();
        move_request.shuffle = Some(true);
        assert_eq!(
            apply_workspace_action(&move_request, &mut queue, None, None, &rt, dir.path()),
            Ok(false)
        );
        assert_eq!(
            crate::mpris::workspace_playback_policy(dir.path()),
            (true, "off")
        );

        move_request.action = "repeat".into();
        move_request.shuffle = None;
        move_request.repeat = Some("context".into());
        assert_eq!(
            apply_workspace_action(&move_request, &mut queue, None, None, &rt, dir.path()),
            Ok(false)
        );
        assert_eq!(
            crate::mpris::workspace_playback_policy(dir.path()),
            (true, "context")
        );

        move_request.action = "download".into();
        move_request.content =
            Some(ContentRef::new("legacy", "song-download", ContentKind::Music).unwrap());
        assert_eq!(
            apply_workspace_action(&move_request, &mut queue, None, None, &rt, dir.path()),
            Err("source_unavailable")
        );

        move_request.action = "transfer".into();
        move_request.target_peer = Some(state::local_host());
        assert_eq!(
            apply_workspace_action(&move_request, &mut queue, None, None, &rt, dir.path()),
            Err("target_is_local_peer")
        );
        state::write_state(
            dir.path(),
            &state::MusicState {
                peer: "seat-15".into(),
                playing: false,
                song_id: String::new(),
                position_ms: 0,
                updated_ms: state::now_ms(),
            },
        )
        .unwrap();
        move_request.target_peer = Some("seat-15".into());
        assert_eq!(
            apply_workspace_action(&move_request, &mut queue, None, None, &rt, dir.path()),
            Err("audio_unavailable")
        );
        move_request.target_peer = Some("unknown-seat".into());
        assert_eq!(
            apply_workspace_action(&move_request, &mut queue, None, None, &rt, dir.path()),
            Err("target_not_admitted")
        );
        state::write_state(
            dir.path(),
            &state::MusicState {
                peer: "busy-seat".into(),
                playing: true,
                song_id: "song-1".into(),
                position_ms: 100,
                updated_ms: state::now_ms(),
            },
        )
        .unwrap();
        move_request.target_peer = Some("busy-seat".into());
        assert_eq!(
            apply_workspace_action(&move_request, &mut queue, None, None, &rt, dir.path()),
            Err("target_busy")
        );
        state::write_state(
            dir.path(),
            &state::MusicState {
                peer: "stale-seat".into(),
                playing: false,
                song_id: String::new(),
                position_ms: 0,
                updated_ms: state::now_ms().saturating_sub(state::STATE_STALE_MS + 1),
            },
        )
        .unwrap();
        move_request.target_peer = Some("stale-seat".into());
        assert_eq!(
            apply_workspace_action(&move_request, &mut queue, None, None, &rt, dir.path()),
            Err("target_stale")
        );
    }

    #[test]
    fn transfer_admission_reads_only_the_mesh_coordination_root() {
        let local_data = tempfile::tempdir().unwrap();
        let coordination = tempfile::tempdir().unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let target = "remote-seat";
        let target_state = state::MusicState {
            peer: target.into(),
            playing: false,
            song_id: String::new(),
            position_ms: 0,
            updated_ms: state::now_ms(),
        };
        state::write_state(local_data.path(), &target_state).unwrap();
        let request = MusicActionRequestV1 {
            schema_version: MUSIC_CONTRACT_VERSION,
            request_id: "transfer-root-split".into(),
            action: "transfer".into(),
            content: None,
            expected_queue_revision: None,
            position_ms: None,
            volume_milli: None,
            shuffle: None,
            repeat: None,
            queue_index: None,
            target_queue_index: None,
            target_peer: Some(target.into()),
            playlist: None,
            playlist_name: None,
            playlist_song_ids: Vec::new(),
            playlist_remove_indices: Vec::new(),
            armed_token: None,
        };
        let mut queue = Queue::default();

        assert_eq!(
            apply_workspace_action_with_clients_and_coordination(
                &request,
                &mut queue,
                None,
                &[],
                &rt,
                local_data.path(),
                coordination.path(),
            ),
            Err("target_not_admitted"),
            "a local-only heartbeat must not fabricate a mesh handoff target",
        );

        state::write_state(coordination.path(), &target_state).unwrap();
        assert_eq!(
            apply_workspace_action_with_clients_and_coordination(
                &request,
                &mut queue,
                None,
                &[],
                &rt,
                local_data.path(),
                coordination.path(),
            ),
            Err("audio_unavailable"),
            "the same fresh heartbeat becomes admissible only on the mesh root",
        );
    }

    #[test]
    fn typed_star_actions_use_admitted_provider_and_refuse_other_sources() {
        let dir = tempfile::tempdir().unwrap();
        let client = Client::with_salt(
            one_shot_json_server(r#"{"subsonic-response":{"status":"ok","version":"1.16.1"}}"#),
            "alice",
            "pw",
            "star-action",
        );
        let unstar_client = Client::with_salt(
            one_shot_json_server(r#"{"subsonic-response":{"status":"ok","version":"1.16.1"}}"#),
            "alice",
            "pw",
            "unstar-action",
        );
        let mut queue = Queue::default();
        let mut request = MusicActionRequestV1 {
            schema_version: MUSIC_CONTRACT_VERSION,
            request_id: "star-1".into(),
            action: "star".into(),
            content: Some(ContentRef::new("legacy", "song-1", ContentKind::Music).unwrap()),
            expected_queue_revision: None,
            position_ms: None,
            volume_milli: None,
            shuffle: None,
            repeat: None,
            queue_index: None,
            target_queue_index: None,
            target_peer: None,
            playlist: None,
            playlist_name: None,
            playlist_song_ids: Vec::new(),
            playlist_remove_indices: Vec::new(),
            armed_token: None,
        };
        assert!(request.validate().is_ok());
        assert_eq!(
            apply_workspace_action(
                &request,
                &mut queue,
                None,
                Some(&client),
                &tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap(),
                dir.path(),
            ),
            Ok(false)
        );

        request.action = "unstar".into();
        request.request_id = "unstar-1".into();
        assert_eq!(
            apply_workspace_action(
                &request,
                &mut queue,
                None,
                Some(&unstar_client),
                &tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap(),
                dir.path(),
            ),
            Ok(false)
        );

        request.content = ContentRef::new("untrusted-source", "song-1", ContentKind::Music);
        assert_eq!(
            apply_workspace_action(
                &request,
                &mut queue,
                None,
                Some(&unstar_client),
                &tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap(),
                dir.path(),
            ),
            Err("unsupported_source")
        );
    }

    #[test]
    fn typed_source_curation_uses_the_selected_admitted_provider() {
        let dir = tempfile::tempdir().unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let first = Client::with_salt(
            one_shot_json_server(r#"{"subsonic-response":{"status":"ok","version":"1.16.1"}}"#),
            "alice",
            "pw",
            "source-one",
        );
        let second = Client::with_salt(
            one_shot_json_server(r#"{"subsonic-response":{"status":"ok","version":"1.16.1"}}"#),
            "alice",
            "pw",
            "source-two",
        );
        let selected = ContentRef::new(
            &catalog_source_id(&second),
            "song-selected",
            ContentKind::Music,
        )
        .unwrap();
        persist_catalog(
            dir.path(),
            &CatalogStoreFile {
                schema_version: CATALOG_STORE_SCHEMA_VERSION,
                songs: vec![CatalogItem {
                    id: "music|song-selected".into(),
                    kind: ContentKind::Music,
                    title: "Selected Song".into(),
                    creator: "Artist".into(),
                    parent_title: "Album".into(),
                    duration_ms: Some(42_000),
                    artwork_ref: None,
                    starred: false,
                    cached: false,
                    variants: vec![catalog_variant(
                        &selected.source_id,
                        &selected.remote_id,
                        ContentKind::Music,
                    )
                    .unwrap()],
                }],
                ..CatalogStoreFile::default()
            },
        )
        .unwrap();
        let mut request = MusicActionRequestV1 {
            schema_version: MUSIC_CONTRACT_VERSION,
            request_id: "source-star-1".into(),
            action: "star".into(),
            content: Some(selected.clone()),
            expected_queue_revision: None,
            position_ms: None,
            volume_milli: None,
            shuffle: None,
            repeat: None,
            queue_index: None,
            target_queue_index: None,
            target_peer: None,
            playlist: None,
            playlist_name: None,
            playlist_song_ids: Vec::new(),
            playlist_remove_indices: Vec::new(),
            armed_token: None,
        };
        let mut queue = Queue::default();
        let clients = vec![&first, &second];
        assert_eq!(
            apply_workspace_action_with_clients(
                &request,
                &mut queue,
                None,
                &clients,
                &rt,
                dir.path(),
            ),
            Ok(false)
        );

        request.content = Some(
            ContentRef::new(
                "airsonic:http://unadmitted.test",
                "song-selected",
                ContentKind::Music,
            )
            .unwrap(),
        );
        assert_eq!(
            apply_workspace_action_with_clients(
                &request,
                &mut queue,
                None,
                &clients,
                &rt,
                dir.path(),
            ),
            Err("unsupported_source")
        );
    }

    #[test]
    fn typed_scrobble_uses_the_selected_admitted_provider() {
        let dir = tempfile::tempdir().unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let rejected = Client::with_salt(
            one_shot_json_server(
                r#"{"subsonic-response":{"status":"failed","version":"1.16.1","error":{"code":50,"message":"wrong provider"}}}"#,
            ),
            "alice",
            "pw",
            "scrobble-source-one",
        );
        let selected = Client::with_salt(
            one_shot_json_server(r#"{"subsonic-response":{"status":"ok","version":"1.16.1"}}"#),
            "alice",
            "pw",
            "scrobble-source-two",
        );
        let selected_ref = ContentRef::new(
            &catalog_source_id(&selected),
            "song-progress",
            ContentKind::Music,
        )
        .unwrap();
        persist_catalog(
            dir.path(),
            &CatalogStoreFile {
                schema_version: CATALOG_STORE_SCHEMA_VERSION,
                songs: vec![CatalogItem {
                    id: "music|song-progress".into(),
                    kind: ContentKind::Music,
                    title: "Progress Song".into(),
                    creator: "Mesh Artist".into(),
                    parent_title: "Album".into(),
                    duration_ms: Some(90_000),
                    artwork_ref: None,
                    starred: false,
                    cached: false,
                    variants: vec![catalog_variant(
                        &selected_ref.source_id,
                        &selected_ref.remote_id,
                        ContentKind::Music,
                    )
                    .unwrap()],
                }],
                ..CatalogStoreFile::default()
            },
        )
        .unwrap();
        let request = MusicActionRequestV1 {
            schema_version: MUSIC_CONTRACT_VERSION,
            request_id: "scrobble-selected".into(),
            action: "scrobble".into(),
            content: Some(selected_ref),
            expected_queue_revision: None,
            position_ms: Some(42_500),
            volume_milli: None,
            shuffle: None,
            repeat: None,
            queue_index: None,
            target_queue_index: None,
            target_peer: None,
            playlist: None,
            playlist_name: None,
            playlist_song_ids: Vec::new(),
            playlist_remove_indices: Vec::new(),
            armed_token: None,
        };
        assert!(request.validate().is_ok());
        let clients = vec![&rejected, &selected];
        assert_eq!(
            apply_workspace_action_with_clients(
                &request,
                &mut Queue::default(),
                None,
                &clients,
                &rt,
                dir.path(),
            ),
            Ok(false)
        );

        let mut unadmitted = request;
        unadmitted.content = Some(
            ContentRef::new(
                "airsonic:http://unadmitted.test",
                "song-progress",
                ContentKind::Music,
            )
            .unwrap(),
        );
        assert_eq!(
            apply_workspace_action_with_clients(
                &unadmitted,
                &mut Queue::default(),
                None,
                &clients,
                &rt,
                dir.path(),
            ),
            Err("unsupported_source")
        );
    }

    #[test]
    fn typed_bookmark_uses_the_selected_admitted_provider() {
        let dir = tempfile::tempdir().unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let rejected = Client::with_salt(
            one_shot_json_server(
                r#"{"subsonic-response":{"status":"failed","version":"1.16.1","error":{"code":50,"message":"wrong provider"}}}"#,
            ),
            "alice",
            "pw",
            "bookmark-source-one",
        );
        let selected = Client::with_salt(
            one_shot_json_server(r#"{"subsonic-response":{"status":"ok","version":"1.16.1"}}"#),
            "alice",
            "pw",
            "bookmark-source-two",
        );
        let selected_ref = ContentRef::new(
            &catalog_source_id(&selected),
            "episode-bookmark",
            ContentKind::Episode,
        )
        .unwrap();
        persist_catalog(
            dir.path(),
            &CatalogStoreFile {
                schema_version: CATALOG_STORE_SCHEMA_VERSION,
                songs: vec![CatalogItem {
                    id: "episode|episode-bookmark".into(),
                    kind: ContentKind::Episode,
                    title: "Episode Bookmark".into(),
                    creator: "Mesh Podcast".into(),
                    parent_title: "Feed".into(),
                    duration_ms: Some(120_000),
                    artwork_ref: None,
                    starred: false,
                    cached: false,
                    variants: vec![catalog_variant(
                        &selected_ref.source_id,
                        &selected_ref.remote_id,
                        ContentKind::Episode,
                    )
                    .unwrap()],
                }],
                ..CatalogStoreFile::default()
            },
        )
        .unwrap();
        let request = MusicActionRequestV1 {
            schema_version: MUSIC_CONTRACT_VERSION,
            request_id: "bookmark-selected".into(),
            action: "bookmark".into(),
            content: Some(selected_ref),
            expected_queue_revision: None,
            position_ms: Some(37_000),
            volume_milli: None,
            shuffle: None,
            repeat: None,
            queue_index: None,
            target_queue_index: None,
            target_peer: None,
            playlist: None,
            playlist_name: None,
            playlist_song_ids: Vec::new(),
            playlist_remove_indices: Vec::new(),
            armed_token: None,
        };
        assert!(request.validate().is_ok());
        let clients = vec![&rejected, &selected];
        assert_eq!(
            apply_workspace_action_with_clients(
                &request,
                &mut Queue::default(),
                None,
                &clients,
                &rt,
                dir.path(),
            ),
            Ok(false)
        );

        let mut unadmitted = request;
        unadmitted.content = Some(
            ContentRef::new(
                "airsonic:http://unadmitted.test",
                "episode-bookmark",
                ContentKind::Episode,
            )
            .unwrap(),
        );
        assert_eq!(
            apply_workspace_action_with_clients(
                &unadmitted,
                &mut Queue::default(),
                None,
                &clients,
                &rt,
                dir.path(),
            ),
            Err("unsupported_source")
        );
    }

    #[test]
    fn typed_playlist_mutation_uses_the_selected_admitted_provider() {
        let dir = tempfile::tempdir().unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let rejected = Client::with_salt(
            one_shot_json_server(
                r#"{"subsonic-response":{"status":"failed","version":"1.16.1","error":{"code":50,"message":"wrong provider"}}}"#,
            ),
            "alice",
            "pw",
            "playlist-source-one",
        );
        let selected = Client::with_salt(
            one_shot_json_server(r#"{"subsonic-response":{"status":"ok","version":"1.16.1"}}"#),
            "alice",
            "pw",
            "playlist-source-two",
        );
        let selected_ref = ContentRef::new(
            &catalog_source_id(&selected),
            "playlist-selected",
            ContentKind::Playlist,
        )
        .unwrap();
        persist_catalog(
            dir.path(),
            &CatalogStoreFile {
                schema_version: CATALOG_STORE_SCHEMA_VERSION,
                songs: vec![CatalogItem {
                    id: "playlist|playlist-selected".into(),
                    kind: ContentKind::Playlist,
                    title: "Selected Playlist".into(),
                    creator: "Mesh User".into(),
                    parent_title: String::new(),
                    duration_ms: None,
                    artwork_ref: None,
                    starred: false,
                    cached: false,
                    variants: vec![catalog_variant(
                        &selected_ref.source_id,
                        &selected_ref.remote_id,
                        ContentKind::Playlist,
                    )
                    .unwrap()],
                }],
                ..CatalogStoreFile::default()
            },
        )
        .unwrap();
        let mut request = MusicActionRequestV1 {
            schema_version: MUSIC_CONTRACT_VERSION,
            request_id: "playlist-source-update".into(),
            action: "playlist_update".into(),
            content: None,
            expected_queue_revision: None,
            position_ms: None,
            volume_milli: None,
            shuffle: None,
            repeat: None,
            queue_index: None,
            target_queue_index: None,
            target_peer: None,
            playlist: Some(selected_ref),
            playlist_name: Some("Selected Playlist Renamed".into()),
            playlist_song_ids: vec!["song-1".into()],
            playlist_remove_indices: vec![0],
            armed_token: None,
        };
        assert!(request.validate().is_ok());
        let clients = vec![&rejected, &selected];
        assert_eq!(
            apply_workspace_action_with_clients(
                &request,
                &mut Queue::default(),
                None,
                &clients,
                &rt,
                dir.path(),
            ),
            Ok(false)
        );

        request.playlist = Some(
            ContentRef::new(
                "airsonic:http://unadmitted.test",
                "playlist-selected",
                ContentKind::Playlist,
            )
            .unwrap(),
        );
        assert_eq!(
            apply_workspace_action_with_clients(
                &request,
                &mut Queue::default(),
                None,
                &clients,
                &rt,
                dir.path(),
            ),
            Err("unsupported_source")
        );
    }

    #[test]
    fn typed_playlist_actions_use_the_admitted_provider() {
        let dir = tempfile::tempdir().unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let body = r#"{"subsonic-response":{"status":"ok","version":"1.16.1"}}"#;
        let mut queue = Queue::default();
        let mut request = MusicActionRequestV1 {
            schema_version: MUSIC_CONTRACT_VERSION,
            request_id: "playlist-create-1".into(),
            action: "playlist_create".into(),
            content: None,
            expected_queue_revision: None,
            position_ms: None,
            volume_milli: None,
            shuffle: None,
            repeat: None,
            queue_index: None,
            target_queue_index: None,
            target_peer: None,
            playlist: None,
            playlist_name: Some("Roadtrip".into()),
            playlist_song_ids: vec!["song-1".into(), "song-2".into()],
            playlist_remove_indices: Vec::new(),
            armed_token: None,
        };
        assert!(request.validate().is_ok());
        let create_client =
            Client::with_salt(one_shot_json_server(body), "alice", "pw", "playlist-create");
        assert_eq!(
            apply_workspace_action(
                &request,
                &mut queue,
                None,
                Some(&create_client),
                &rt,
                dir.path(),
            ),
            Ok(false)
        );

        request.action = "playlist_update".into();
        request.request_id = "playlist-update-1".into();
        request.content = None;
        request.playlist =
            Some(ContentRef::new("legacy", "playlist-1", ContentKind::Playlist).unwrap());
        request.playlist_name = Some("Roadtrip 2026".into());
        request.playlist_song_ids = vec!["song-3".into()];
        request.playlist_remove_indices = vec![0];
        let update_client =
            Client::with_salt(one_shot_json_server(body), "alice", "pw", "playlist-update");
        assert_eq!(
            apply_workspace_action(
                &request,
                &mut queue,
                None,
                Some(&update_client),
                &rt,
                dir.path(),
            ),
            Ok(false)
        );

        request.action = "playlist_delete".into();
        request.request_id = "playlist-delete-1".into();
        request.playlist_name = None;
        request.playlist_song_ids.clear();
        request.playlist_remove_indices.clear();
        let delete_client =
            Client::with_salt(one_shot_json_server(body), "alice", "pw", "playlist-delete");
        assert_eq!(
            apply_workspace_action(
                &request,
                &mut queue,
                None,
                Some(&delete_client),
                &rt,
                dir.path(),
            ),
            Ok(false)
        );

        request.action = "playlist_reorder".into();
        request.request_id = "playlist-reorder-1".into();
        request.playlist_song_ids = vec!["song-2".into(), "song-1".into()];
        request.playlist =
            Some(ContentRef::new("untrusted-source", "playlist-1", ContentKind::Playlist).unwrap());
        assert_eq!(
            apply_workspace_action(
                &request,
                &mut queue,
                None,
                Some(&delete_client),
                &rt,
                dir.path(),
            ),
            Err("unsupported_source")
        );
    }

    #[test]
    fn admitted_download_selects_retained_nonlegacy_provider() {
        let data_dir = tempfile::tempdir().unwrap();
        let first = Client::with_salt(
            one_shot_json_server(r#"{"subsonic-response":{"status":"ok","version":"1.16.1"}}"#),
            "alice",
            "pw",
            "download-first",
        );
        let second = Client::with_salt(
            one_shot_json_server(r#"{"subsonic-response":{"status":"ok","version":"1.16.1"}}"#),
            "alice",
            "pw",
            "download-second",
        );
        let selected = ContentRef::new(
            &catalog_source_id(&second),
            "song-selected",
            ContentKind::Music,
        )
        .unwrap();
        persist_catalog(
            data_dir.path(),
            &CatalogStoreFile {
                schema_version: CATALOG_STORE_SCHEMA_VERSION,
                songs: vec![CatalogItem {
                    id: "music|song-selected".into(),
                    kind: ContentKind::Music,
                    title: "Selected song".into(),
                    creator: "Artist".into(),
                    parent_title: "Album".into(),
                    duration_ms: Some(42_000),
                    artwork_ref: None,
                    starred: false,
                    cached: false,
                    variants: vec![catalog_variant(
                        &selected.source_id,
                        &selected.remote_id,
                        ContentKind::Music,
                    )
                    .unwrap()],
                }],
                ..CatalogStoreFile::default()
            },
        )
        .unwrap();
        let request = MusicActionRequestV1 {
            schema_version: MUSIC_CONTRACT_VERSION,
            request_id: "download-selected".into(),
            action: "download".into(),
            content: Some(selected.clone()),
            expected_queue_revision: None,
            position_ms: None,
            volume_milli: None,
            shuffle: None,
            repeat: None,
            queue_index: None,
            target_queue_index: None,
            target_peer: None,
            playlist: None,
            playlist_name: None,
            playlist_song_ids: Vec::new(),
            playlist_remove_indices: Vec::new(),
            armed_token: None,
        };
        let clients = vec![&first, &second];
        let chosen = admitted_download_client(&request, &clients, data_dir.path()).unwrap();
        assert_eq!(catalog_source_id(chosen), selected.source_id);
    }

    #[test]
    fn typed_download_lifecycle_writes_and_removes_durable_record() {
        let data_dir = tempfile::tempdir().unwrap();
        let cache_dir = tempfile::tempdir().unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut request = MusicActionRequestV1 {
            schema_version: MUSIC_CONTRACT_VERSION,
            request_id: "download-1".into(),
            action: "download".into(),
            content: Some(ContentRef::new("legacy", "song-download", ContentKind::Music).unwrap()),
            expected_queue_revision: None,
            position_ms: None,
            volume_milli: None,
            shuffle: None,
            repeat: None,
            queue_index: None,
            target_queue_index: None,
            target_peer: None,
            playlist: None,
            playlist_name: None,
            playlist_song_ids: Vec::new(),
            playlist_remove_indices: Vec::new(),
            armed_token: None,
        };
        let client = Client::with_salt(
            one_shot_bytes_server(b"finite-audio"),
            "alice",
            "pw",
            "download",
        );
        assert!(request.validate().is_ok());
        download_to_cache(&request, &client, &rt, data_dir.path(), cache_dir.path()).unwrap();
        let records = load_downloads(data_dir.path()).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].state, "ready");
        assert_eq!(records[0].bytes, 12);
        assert_eq!(
            crate::cache::read_cached_track_bytes(cache_dir.path(), "song-download", 20),
            Some(b"finite-audio".to_vec())
        );

        request.action = "pin_download".into();
        request.request_id = "download-pin-1".into();
        set_download_pinned(&request, data_dir.path(), cache_dir.path(), true).unwrap();
        assert!(load_downloads(data_dir.path()).unwrap()[0].pinned);
        assert!(crate::cache::read_index(cache_dir.path())
            .entries
            .get("song-download")
            .is_some_and(|entry| entry.starred));

        request.action = "unpin_download".into();
        request.request_id = "download-unpin-1".into();
        set_download_pinned(&request, data_dir.path(), cache_dir.path(), false).unwrap();
        assert!(!load_downloads(data_dir.path()).unwrap()[0].pinned);
        assert!(!crate::cache::read_index(cache_dir.path())
            .entries
            .get("song-download")
            .is_some_and(|entry| entry.starred));

        request.action = "cancel_download".into();
        request.request_id = "download-cancel-1".into();
        cancel_download(&request, data_dir.path()).unwrap();
        assert_eq!(
            load_downloads(data_dir.path()).unwrap()[0].state,
            "cancelled"
        );

        request.action = "remove_download".into();
        request.request_id = "download-remove-1".into();
        remove_download(&request, data_dir.path(), cache_dir.path()).unwrap();
        assert!(load_downloads(data_dir.path()).unwrap().is_empty());
        assert!(!cache_dir.path().join("song-download.audio").exists());

        request.content = Some(ContentRef::new("legacy", "radio-1", ContentKind::Radio).unwrap());
        assert_eq!(download_identity(&request), Err("unsupported_source"));
    }

    #[test]
    fn typed_request_serialization_does_not_retain_armed_token() {
        let request = MusicActionRequestV1 {
            schema_version: MUSIC_CONTRACT_VERSION,
            request_id: "redaction-1".into(),
            action: "play".into(),
            content: None,
            expected_queue_revision: None,
            position_ms: None,
            volume_milli: None,
            shuffle: None,
            repeat: None,
            queue_index: None,
            target_queue_index: None,
            target_peer: None,
            playlist: None,
            playlist_name: None,
            playlist_song_ids: Vec::new(),
            playlist_remove_indices: Vec::new(),
            armed_token: Some("secret-token".into()),
        };
        let wire = serde_json::to_string(&request).unwrap();
        assert!(!wire.contains("secret-token"));
        assert_eq!(request_id_from_body(&wire), "redaction-1");
        assert_eq!(request_id_from_body("{}"), "invalid-request");
    }

    fn clock_catalog_source(source_id: &str) -> ServerCapabilities {
        ServerCapabilities {
            source_id: source_id.to_owned(),
            api_profile: "subsonic/1.16.1".to_owned(),
            reachable: true,
            authentication_required: false,
            features: BTreeSet::from(["podcast-episodes".to_owned(), "list-radio".to_owned()]),
        }
    }

    fn clock_catalog_item(
        id: &str,
        kind: ContentKind,
        title: &str,
        source_id: &str,
        remote_id: &str,
    ) -> CatalogItem {
        CatalogItem {
            id: id.to_owned(),
            kind,
            title: title.to_owned(),
            creator: String::new(),
            parent_title: String::new(),
            duration_ms: None,
            artwork_ref: None,
            starred: false,
            cached: false,
            variants: vec![catalog_variant(source_id, remote_id, kind).unwrap()],
        }
    }

    #[test]
    fn clock_news_now_500005_resolves_newest_admitted_episode_at_ring_time() {
        let source_id = "airsonic:https://music.mesh";
        let older = clock_catalog_item(
            "episode|older",
            ContentKind::Episode,
            "News Now 11:00",
            source_id,
            "episode-older",
        );
        let newest = clock_catalog_item(
            "episode|newest",
            ContentKind::Episode,
            "News Now 12:00",
            source_id,
            "episode-newest",
        );
        let now = 1_700_000_000_000_u64;
        let catalog = CatalogStoreFile {
            sources: vec![clock_catalog_source(source_id)],
            episodes: vec![older, newest.clone()],
            clock_presets: BTreeMap::from([(
                NPR_NEWS_NOW_PRESET_ID.to_owned(),
                ClockCatalogPreset {
                    content: newest.variants[0].content.clone(),
                    admitted_at_utc_ms: now,
                },
            )]),
            ..CatalogStoreFile::default()
        };
        let stable = ClockAudioRef::Music {
            source_id: CLOCK_CATALOG_SOURCE_ID.to_owned(),
            remote_id: NPR_NEWS_NOW_PRESET_ID.to_owned(),
            content_kind: ClockMusicKind::PodcastEpisode,
            fallback_tone_id: "alarm_classic".to_owned(),
        };

        let resolved = governed_clock_content_at(&catalog, &stable, now).unwrap();
        assert_eq!(resolved.remote_id, "episode-newest");
        assert!(!serde_json::to_string(&stable).unwrap().contains("http"));
    }

    #[test]
    fn news_now_catalog_refresh_records_first_provider_episode_as_newest() {
        let dir = tempfile::tempdir().unwrap();
        let client = Client::with_salt("http://127.0.0.1:9", "alice", "unused", "clock-news-now");
        let source_id = catalog_source_id(&client);
        let podcast = clock_catalog_item(
            "Podcast|npr news now|||0",
            ContentKind::Podcast,
            "NPR News Now",
            &source_id,
            NPR_NEWS_NOW_PRESET_ID,
        );
        persist_catalog(
            dir.path(),
            &CatalogStoreFile {
                sources: vec![clock_catalog_source(&source_id)],
                podcasts: vec![podcast],
                ..CatalogStoreFile::default()
            },
        )
        .unwrap();

        record_catalog_response(
            dir.path(),
            &client,
            "podcast-episodes",
            r#"{"id":"500005"}"#,
            r#"{"ok":true,"result":{"episodes":[{"id":"episode-newest","title":"12:00"},{"id":"episode-older","title":"11:00"}]}}"#,
        );

        let catalog = load_catalog(dir.path()).unwrap();
        assert_eq!(
            catalog.clock_presets[NPR_NEWS_NOW_PRESET_ID]
                .content
                .remote_id,
            "episode-newest"
        );
    }

    #[test]
    fn clock_news_now_refuses_stale_unauthorized_unreachable_or_deleted_state() {
        let source_id = "airsonic:https://music.mesh";
        let episode = clock_catalog_item(
            "episode|newest",
            ContentKind::Episode,
            "News Now",
            source_id,
            "episode-newest",
        );
        let now = 1_700_000_000_000_u64;
        let stable = ClockAudioRef::Music {
            source_id: CLOCK_CATALOG_SOURCE_ID.to_owned(),
            remote_id: NPR_NEWS_NOW_PRESET_ID.to_owned(),
            content_kind: ClockMusicKind::PodcastEpisode,
            fallback_tone_id: "alarm_classic".to_owned(),
        };
        let base = CatalogStoreFile {
            sources: vec![clock_catalog_source(source_id)],
            episodes: vec![episode.clone()],
            clock_presets: BTreeMap::from([(
                NPR_NEWS_NOW_PRESET_ID.to_owned(),
                ClockCatalogPreset {
                    content: episode.variants[0].content.clone(),
                    admitted_at_utc_ms: now,
                },
            )]),
            ..CatalogStoreFile::default()
        };

        assert_eq!(
            governed_clock_content_at(&base, &stable, now + NPR_NEWS_NOW_MAX_AGE_MS + 1,),
            Err("catalog_reference_stale")
        );
        let mut unauthorized = base.clone();
        unauthorized.sources[0].authentication_required = true;
        assert_eq!(
            governed_clock_content_at(&unauthorized, &stable, now),
            Err("catalog_source_unauthorized")
        );
        let mut unreachable = base.clone();
        unreachable.sources[0].reachable = false;
        assert_eq!(
            governed_clock_content_at(&unreachable, &stable, now),
            Err("catalog_source_unreachable")
        );
        let mut deleted = base;
        deleted.episodes.clear();
        assert_eq!(
            governed_clock_content_at(&deleted, &stable, now),
            Err("catalog_reference_missing")
        );
    }

    #[test]
    fn clock_malformed_preset_is_rejected_by_the_existing_catalog_loader() {
        let dir = tempfile::tempdir().unwrap();
        let source_id = "airsonic:https://music.mesh";
        let episode = clock_catalog_item(
            "episode|newest",
            ContentKind::Episode,
            "News Now",
            source_id,
            "episode-newest",
        );
        persist_catalog(
            dir.path(),
            &CatalogStoreFile {
                sources: vec![clock_catalog_source(source_id)],
                episodes: vec![episode.clone()],
                clock_presets: BTreeMap::from([(
                    NPR_NEWS_NOW_PRESET_ID.to_owned(),
                    ClockCatalogPreset {
                        content: episode.variants[0].content.clone(),
                        admitted_at_utc_ms: 1_700_000_000_000,
                    },
                )]),
                ..CatalogStoreFile::default()
            },
        )
        .unwrap();
        let path = catalog_path(dir.path());
        let mut wire: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        wire["clock_presets"][NPR_NEWS_NOW_PRESET_ID]["content"]["remote_id"] =
            Value::String(String::new());
        std::fs::write(&path, serde_json::to_vec(&wire).unwrap()).unwrap();

        assert_eq!(
            load_catalog(dir.path()).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn clock_npr_live_station_uses_a_separate_stable_catalog_identity() {
        let source_id = "airsonic:https://music.mesh";
        let station = clock_catalog_item(
            "Radio|npr member station|||0",
            ContentKind::Radio,
            "NPR Member Station",
            source_id,
            "https://provider.invalid/live-stream",
        );
        let catalog = CatalogStoreFile {
            sources: vec![clock_catalog_source(source_id)],
            radio: vec![station.clone()],
            ..CatalogStoreFile::default()
        };
        let stable = ClockAudioRef::Music {
            source_id: CLOCK_CATALOG_SOURCE_ID.to_owned(),
            remote_id: station.id.clone(),
            content_kind: ClockMusicKind::Radio,
            fallback_tone_id: "alarm_classic".to_owned(),
        };

        let resolved = governed_clock_content_at(&catalog, &stable, 1).unwrap();
        assert_eq!(resolved.remote_id, "https://provider.invalid/live-stream");
        assert!(!serde_json::to_string(&stable).unwrap().contains("http"));
        assert_ne!(
            stable,
            ClockAudioRef::Music {
                source_id: CLOCK_CATALOG_SOURCE_ID.to_owned(),
                remote_id: NPR_NEWS_NOW_PRESET_ID.to_owned(),
                content_kind: ClockMusicKind::PodcastEpisode,
                fallback_tone_id: "alarm_classic".to_owned(),
            }
        );
    }

    #[test]
    fn clock_local_file_admission_resolves_only_the_stable_catalog_identity() {
        let data = tempfile::tempdir().unwrap();
        let root = data.path().join(CLOCK_LOCAL_FILE_DIRECTORY);
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("morning.wav");
        std::fs::write(&file, b"bounded-audio-fixture").unwrap();

        let audio = admit_clock_local_file(data.path(), "morning-bell", &file).unwrap();
        let catalog = load_catalog(data.path()).unwrap();
        let tracks =
            governed_clock_playback_tracks(data.path(), &catalog, &audio, &[], data.path())
                .unwrap();
        assert_eq!(tracks.len(), 1);
        let wire = serde_json::to_string(&audio).unwrap();
        assert!(wire.contains("morning-bell"));
        assert!(!wire.contains(file.to_string_lossy().as_ref()));
        assert!(!wire.contains("file:") && !wire.contains("bounded-audio-fixture"));
    }

    #[test]
    fn clock_local_file_references_fail_closed_when_malformed_stale_missing_or_unauthorized() {
        let data = tempfile::tempdir().unwrap();
        let root = data.path().join(CLOCK_LOCAL_FILE_DIRECTORY);
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("morning.wav");
        std::fs::write(&file, b"first").unwrap();
        let audio = admit_clock_local_file(data.path(), "morning-bell", &file).unwrap();
        let catalog = load_catalog(data.path()).unwrap();

        let malformed = ClockAudioRef::Music {
            source_id: CLOCK_LOCAL_FILE_SOURCE_ID.to_owned(),
            remote_id: "../morning.wav".to_owned(),
            content_kind: ClockMusicKind::Track,
            fallback_tone_id: "bell".to_owned(),
        };
        assert_eq!(
            governed_clock_local_track(data.path(), &catalog, &malformed),
            Err("local_file_reference_malformed")
        );

        std::fs::write(&file, b"changed-and-longer").unwrap();
        assert_eq!(
            governed_clock_local_track(data.path(), &catalog, &audio),
            Err("local_file_reference_stale")
        );
        std::fs::remove_file(&file).unwrap();
        assert_eq!(
            governed_clock_local_track(data.path(), &catalog, &audio),
            Err("local_file_reference_missing")
        );

        let outside = data.path().join("outside.wav");
        std::fs::write(&outside, b"outside").unwrap();
        assert_eq!(
            admit_clock_local_file(data.path(), "outside", &outside)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::PermissionDenied
        );

        let mut unauthorized = catalog;
        unauthorized.clock_local_files.insert(
            "morning-bell".to_owned(),
            ClockLocalFileAdmission {
                relative_path: "../outside.wav".to_owned(),
                byte_len: 7,
                modified_at_utc_ms: 1,
                sha256: "0".repeat(64),
            },
        );
        assert_eq!(
            governed_clock_local_track(data.path(), &unauthorized, &audio),
            Err("local_file_reference_unauthorized")
        );
    }
}
