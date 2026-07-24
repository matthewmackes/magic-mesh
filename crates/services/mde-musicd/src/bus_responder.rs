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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde_json::json;

use crate::airsonic::Client;
use crate::creds;
use crate::engine::{Engine, SourceCodec};

/// Shared `ipc/action_auth` wire contract for the music responder.
///
/// `mde-musicd` is a user-service crate and intentionally does not depend on
/// the root `mackesd` binary crate. Keep this verifier byte-compatible with
/// `mackesd::ipc::action_auth`: schema v1, the canonical request digest, the
/// v2 armed-token format, a 30-second maximum lifetime, and a durable
/// host-local nonce claim. Missing credentials fail closed.
mod music_action_auth {
    use std::path::{Path, PathBuf};

    use serde_json::Value;

    pub(super) const SCHEMA_VERSION: u64 = 1;
    const MAX_TTL_MS: i64 = 30_000;
    const NONCE_MIN_LEN: usize = 8;
    const CREDENTIAL_NAME: &str = "cloud-arm-key";
    const DEFAULT_AUTH_ROOT: &str = "/var/lib/mackesd/cloud-auth";

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
        auth_root: PathBuf,
        test_now_ms: Option<i64>,
    }

    impl std::fmt::Debug for Authorizer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Authorizer")
                .field("auth_root", &self.auth_root)
                .field("has_key", &self.key.is_some())
                .finish_non_exhaustive()
        }
    }

    impl Authorizer {
        pub(super) fn production() -> Self {
            let key = load_production_key().ok();
            if key.is_none() {
                tracing::error!(
                    target: "mde_musicd::action_auth",
                    "music mutation authorization unavailable; mutations are disabled"
                );
            }
            Self {
                key,
                auth_root: PathBuf::from(DEFAULT_AUTH_ROOT),
                test_now_ms: None,
            }
        }

        #[cfg(test)]
        pub(super) fn for_test(key: &[u8], auth_root: PathBuf, now_ms: i64) -> Self {
            Self {
                key: Some(key.to_vec()),
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
            if now_ms > token.expires_at_ms {
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
                .is_some_and(|expiry| expiry < now_ms);
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

    fn hex_encode(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut output = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            output.push(HEX[(byte >> 4) as usize] as char);
            output.push(HEX[(byte & 0x0f) as usize] as char);
        }
        output
    }

    fn sha256(input: &[u8]) -> [u8; 32] {
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
pub const BROWSE_VERBS: [&str; 23] = [
    "list-albums",
    "list-artists",
    "search",
    "get-album",
    "list-genres",
    "albums-by-genre",
    "albums-by-artist",
    "get-song",
    "get-cover-art",
    "list-podcasts",
    "list-radio",
    "podcast-episodes",
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

/// Authoritative-state write cadence while playing (AIR-8's 5 s heartbeat,
/// so a stale owner frees the mesh after `STATE_STALE_MS`).
pub const STATE_WRITE_INTERVAL: Duration = Duration::from_secs(5);

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
            "list-albums" => client
                .get_album_list2("newest", 100)
                .await
                .map(|a| json!({ "albums": a }))
                .map_err(|e| e.to_string()),
            "list-artists" => client
                .get_artists()
                .await
                .map(|a| json!({ "artists": a }))
                .map_err(|e| e.to_string()),
            "search" => {
                let query = song_id_from(body).unwrap_or_default();
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
            "albums-by-artist" => {
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
            "list-podcasts" => client
                .get_podcast_channels()
                .await
                .map(|c| json!({ "podcasts": c }))
                .map_err(|e| e.to_string()),
            // SVC-3 — the Radio hub card: the server's saved stations.
            // MUSIC-RESPONSIVE-7 — serve a fresh cached list (no upstream call);
            // only the first open (or a stale cache) hits the server.
            "list-radio" => {
                let cached = RADIO_CACHE.lock().ok().and_then(|g| {
                    g.as_ref()
                        .filter(|(at, _)| at.elapsed() < RADIO_CACHE_TTL)
                        .map(|(_, v)| v.clone())
                });
                if let Some(v) = cached {
                    Ok(v)
                } else {
                    match client.get_internet_radio_stations().await {
                        Ok(r) => {
                            let v = json!({ "radio": r });
                            if let Ok(mut g) = RADIO_CACHE.lock() {
                                *g = Some((Instant::now(), v.clone()));
                            }
                            Ok(v)
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
        "play" | "pause" | "resume" | "stop" | "set-volume" | "seek" => Some("transport"),
        "take-over" => Some("peer-takeover"),
        // `get-queue`, browse, `get-state`, and `peer-states` are reads.
        _ => None,
    }
}

fn authorize_music_mutation(
    authorizer: &music_action_auth::Authorizer,
    verb: &str,
    body: &str,
) -> Result<(), String> {
    let Some(target) = music_mutation_scope(verb) else {
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
    for verb in BROWSE_VERBS {
        let topic = format!("action/music/{verb}");
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since(&topic, since) {
            Ok(m) => m,
            Err(_) => continue,
        };
        for msg in msgs {
            cursors.insert(topic.clone(), msg.ulid.clone());
            let body = msg.body.as_deref().unwrap_or("");
            let reply = match authorize_music_mutation(authorizer, verb, body) {
                Err(error) => unauthorized_reply(verb, &error),
                Ok(()) => match client {
                    Some(c) => dispatch_browse(verb, body, c, rt),
                    None => {
                        json!({ "ok": false, "error": "no Airsonic server configured" }).to_string()
                    }
                },
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
    let _ = state::write_state(&state::data_dir(), &st);
}

/// Apply one transport request to the engine + queue, returning the reply
/// JSON. Side effects (engine + the AIR-8 state write); the pure
/// verb→command parse is [`parse_transport`].
fn apply_transport(
    verb: &str,
    body: &str,
    engine: Option<&Engine>,
    client: Option<&Client>,
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
            "needs_airsonic": client.is_none(),
            // MUSIC-RFX-2 — the GUI shows the scrubber only for a seekable track.
            "seekable": engine.map_or(false, |e| e.is_seekable()),
        })
        .to_string(),
        TransportCommand::Play => {
            let Some(engine) = engine else {
                return no_audio();
            };
            let Some(client) = client else {
                return json!({ "ok": false, "error": "no Airsonic server configured" })
                    .to_string();
            };
            // Gapless album: hand the engine current..end in one list. The
            // base cursor lets the AIR-2.c auto-advance driver map the audible
            // track back to the right queue index as playback crosses gapless
            // boundaries.
            let upcoming: Vec<(String, SourceCodec)> = queue
                .songs
                .iter()
                .skip(queue.current)
                .map(|id| (client.stream_url(id), SourceCodec::Unknown))
                .collect();
            if upcoming.is_empty() {
                return json!({ "ok": false, "error": "queue is empty" }).to_string();
            }
            engine.play_from(upcoming, queue.current);
            let song = queue.current().unwrap_or("");
            write_playback_state(true, song, 0);
            json!({ "ok": true, "playing": true, "song_id": song }).to_string()
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
            engine.resume();
            write_playback_state(true, queue.current().unwrap_or(""), engine.position_ms());
            json!({ "ok": true, "playing": true }).to_string()
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
            write_playback_state(
                engine.is_playing(),
                queue.current().unwrap_or(""),
                target_ms,
            );
            json!({ "ok": true, "seeked": accepted, "position_ms": target_ms }).to_string()
        }
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
    poll_transport_with_authorizer(persist, queue_path, engine, cursors, client, &authorizer);
}

fn poll_transport_with_authorizer(
    persist: &Persist,
    queue_path: &Path,
    engine: Option<&Engine>,
    cursors: &mut HashMap<String, String>,
    client: Option<&Client>,
    authorizer: &music_action_auth::Authorizer,
) {
    let queue = queue::read_from(queue_path);
    for verb in TRANSPORT_VERBS {
        let topic = format!("action/music/{verb}");
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since(&topic, since) {
            Ok(m) => m,
            Err(_) => continue,
        };
        for msg in msgs {
            cursors.insert(topic.clone(), msg.ulid.clone());
            let body = msg.body.as_deref().unwrap_or("");
            let reply = match authorize_music_mutation(authorizer, verb, body) {
                Err(error) => unauthorized_reply(verb, &error),
                Ok(()) => apply_transport(verb, body, engine, client, &queue),
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
/// while playing (AIR-8).
fn write_periodic_state(engine: Option<&Engine>, queue_path: &Path) {
    let Some(engine) = engine else { return };
    if !engine.is_playing() {
        return;
    }
    let queue = queue::read_from(queue_path);
    write_playback_state(true, queue.current().unwrap_or(""), engine.position_ms());
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
fn refresh_airsonic_client(cache: &mut Option<(creds::Creds, Client)>) -> Option<&Client> {
    match creds::load().ok() {
        Some(current) => {
            if airsonic_creds_changed(cache.as_ref().map(|(c, _)| c), &current) {
                let client = Client::new(&current.server_url, &current.username, &current.password);
                *cache = Some((current, client));
            }
            cache.as_ref().map(|(_, c)| c)
        }
        None => {
            *cache = None;
            None
        }
    }
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
    // `list_since(None)` returns every request ever made on each action topic
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
    // AIR-6: bring up the MPRIS surface sharing this engine, so media keys
    // (sway → playerctl → MPRIS) + the lock-screen widget drive the same
    // playback the Bus does. Held for the serve loop's lifetime; dropping
    // it (when serve returns) stops the surface thread. A headless peer
    // with no audio engine — or no session bus — simply skips it.
    let mut _mpris = engine
        .as_ref()
        .map(|e| crate::mpris::spawn(e.handle(), queue_path.to_path_buf(), state::data_dir()));
    let mut last_state_write = Instant::now();
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
    // on a creds change (refresh_airsonic_client).
    let mut airsonic: Option<(creds::Creds, Client)> = None;
    if let Some(client) = refresh_airsonic_client(&mut airsonic) {
        match rt.block_on(client.ping()) {
            Ok(_) => tracing::info!("airsonic connection warmed at startup"),
            Err(e) => {
                tracing::debug!(error = %e, "startup airsonic warm-ping failed (cold first browse)")
            }
        }
    }
    let authorizer = music_action_auth::Authorizer::production();
    while !should_stop() {
        if engine.is_none() && last_audio_retry.elapsed() >= AUDIO_RETRY_INTERVAL {
            last_audio_retry = Instant::now();
            if let Ok(e) = Engine::new() {
                tracing::info!("audio output acquired on retry — playback enabled");
                _mpris = Some(crate::mpris::spawn(
                    e.handle(),
                    queue_path.to_path_buf(),
                    state::data_dir(),
                ));
                engine = Some(e);
            }
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
        // MUSIC-RESPONSIVE-10 — refresh the shared client once per sweep (cheap;
        // rebuilds only on a creds change) and hand it to both network pollers.
        let client = refresh_airsonic_client(&mut airsonic);
        poll_transport_with_authorizer(
            &persist,
            queue_path,
            engine.as_ref(),
            &mut cursors,
            client,
            &authorizer,
        );
        poll_peers_with_authorizer(&persist, &mut cursors, &authorizer);
        poll_browse_with_authorizer(&persist, &rt, &mut cursors, client, &authorizer);
        if last_state_write.elapsed() >= STATE_WRITE_INTERVAL {
            write_periodic_state(engine.as_ref(), queue_path);
            last_state_write = Instant::now();
        }
        std::thread::sleep(POLL_INTERVAL);
    }
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
        .chain(PEER_VERBS.iter());
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
    let dir = state::data_dir();
    for verb in PEER_VERBS {
        let topic = format!("action/music/{verb}");
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since(&topic, since) {
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
        let msgs = match persist.list_since(&topic, since) {
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

    fn test_authorizer(root: &Path) -> music_action_auth::Authorizer {
        music_action_auth::Authorizer::for_test(AUTH_KEY, root.join("auth"), AUTH_NOW_MS)
    }

    fn armed_test_body(unsigned: &str, verb: &str, target: &str, nonce: &str) -> String {
        let node = state::local_host();
        let request_sha256 = music_action_auth::request_digest_for_test(unsigned);
        let payload = format!(
            "v2|{nonce}|{}|music-{verb}|{node}|{target}|{request_sha256}",
            AUTH_NOW_MS + 30_000
        );
        let signature = music_action_auth::sign_for_test(AUTH_KEY, &payload);
        let token = format!("{payload}|{signature}");
        let mut body: serde_json::Value = serde_json::from_str(unsigned).unwrap();
        body.as_object_mut()
            .unwrap()
            .insert("armed_token".to_string(), serde_json::Value::String(token));
        body.to_string()
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
}
