//! EDITOR-9 (Part B) — the **Files → Editor** cross-surface open seam.
//!
//! "Send-to-Editor" hands a selected file to the one Construct shell's code-editor
//! surface. Reuse, not reimplementation (§6): it uses the **same persist-first Bus
//! verb pattern** the other Send-To actions use (`chat_bridge`'s `action/chat/send`,
//! `mesh_mount`'s `action/mesh-mount/*`) — a surface writes a typed verb onto a
//! local `Persist`; a consumer drains it. Here the consumer is the shell's editor
//! mount, which reads [`ACTION_EDITOR_OPEN`] and calls
//! `EditorSurface::open_path` (the EDITOR-3 seam).
//!
//! The wire body is [`EditorOpenRequest`] (one file path). The pure builder/parser
//! is unit-tested here; behind the `dbus` feature, [`BusEditorLaunch`] writes the
//! verb and [`EditorLaunchWatch`] drains it (both degrade to an honest no-op when
//! this node has no Bus — never a panic, never a hang).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Shared capability helpers for the two mde-files Bus producers that dispatch
/// mutable requests. The verifier is also used by the editor drain, so an
/// unsigned writer cannot trigger an editor open merely by writing the topic.
#[cfg(feature = "dbus")]
pub(crate) mod capability {
    use std::path::{Path, PathBuf};

    use mackes_mesh_types::cloud::{
        cloud_request_digest, decode_cloud_arm_credential, CloudArmSigner, CloudArmedToken,
        CLOUD_ACTION_SCHEMA_VERSION, CLOUD_ARM_CREDENTIAL,
    };
    use serde_json::Value;

    pub(crate) const EDITOR_OPEN_VERB: &str = "editor-open";
    pub(crate) const EDITOR_OPEN_TARGET: &str = "editor";
    pub(crate) const DIRECT_TRANSFER_VERB: &str = "mesh-transfer-direct";
    const MAX_TTL_MS: i64 = 30_000;
    const NONCE_MIN_LEN: usize = 8;
    const DEFAULT_AUTH_ROOT: &str = "/var/lib/mackesd/cloud-auth";

    pub(crate) fn local_node() -> String {
        for path in ["/etc/hostname", "/proc/sys/kernel/hostname"] {
            if let Ok(hostname) = std::fs::read_to_string(path) {
                let hostname = hostname.trim();
                if !hostname.is_empty() {
                    return hostname.to_string();
                }
            }
        }
        std::env::var("HOSTNAME")
            .ok()
            .map(|hostname| hostname.trim().to_string())
            .filter(|hostname| !hostname.is_empty())
            .unwrap_or_else(|| "unknown".to_string())
    }

    fn fresh_nonce() -> Result<String, String> {
        let mut bytes = [0_u8; 32];
        let mut random = std::fs::File::open("/dev/urandom")
            .map_err(|error| format!("open system random source: {error}"))?;
        std::io::Read::read_exact(&mut random, &mut bytes)
            .map_err(|error| format!("read system random source: {error}"))?;
        Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
    }

    fn production_signer() -> Result<CloudArmSigner, String> {
        #[cfg(not(test))]
        {
            if !running_as_root() {
                return Err("Bus mutation authorization requires the root service process".into());
            }
            let directory = std::env::var_os("CREDENTIALS_DIRECTORY")
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .ok_or_else(|| "systemd action credential is unavailable".to_string())?;
            let raw = std::fs::read(directory.join(CLOUD_ARM_CREDENTIAL))
                .map_err(|error| format!("read action credential: {error}"))?;
            let key = decode_cloud_arm_credential(&raw).map_err(str::to_string)?;
            return CloudArmSigner::new(key).map_err(str::to_string);
        }
        #[cfg(test)]
        {
            CloudArmSigner::new(b"mde-files-capability-test-key".to_vec()).map_err(str::to_string)
        }
    }

    #[cfg(not(test))]
    fn running_as_root() -> bool {
        #[cfg(target_os = "linux")]
        {
            return std::fs::read_to_string("/proc/self/status")
                .ok()
                .and_then(|status| {
                    status.lines().find_map(|line| {
                        let mut fields = line.split_whitespace();
                        if fields.next() == Some("Uid:") {
                            fields.nth(1).and_then(|euid| euid.parse::<u32>().ok())
                        } else {
                            None
                        }
                    })
                })
                == Some(0);
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }

    pub(crate) struct Authorizer {
        signer: Option<CloudArmSigner>,
        auth_root: PathBuf,
        test_now_ms: Option<i64>,
    }

    impl Authorizer {
        pub(crate) fn production() -> Self {
            let signer = production_signer().ok();
            Self {
                signer,
                auth_root: PathBuf::from(DEFAULT_AUTH_ROOT),
                test_now_ms: None,
            }
        }

        #[cfg(test)]
        pub(crate) fn for_test(key: &[u8], auth_root: PathBuf, now_ms: i64) -> Self {
            Self {
                signer: CloudArmSigner::new(key.to_vec()).ok(),
                auth_root,
                test_now_ms: Some(now_ms),
            }
        }

        pub(crate) fn authorize(
            &self,
            body: &str,
            verb: &str,
            node: &str,
            target: &str,
        ) -> Result<(), String> {
            if body.len() > 64 * 1024 {
                return Err("request body exceeds the 64 KiB cap".to_string());
            }
            let object = serde_json::from_str::<Value>(body)
                .map_err(|_| "request body is not a JSON object".to_string())?
                .as_object()
                .cloned()
                .ok_or_else(|| "request body is not a JSON object".to_string())?;
            if object.get("schema_version").and_then(Value::as_u64)
                != Some(u64::from(CLOUD_ACTION_SCHEMA_VERSION))
            {
                return Err(format!(
                    "privileged action requires schema_version {CLOUD_ACTION_SCHEMA_VERSION}"
                ));
            }
            let raw_token = object
                .get("armed_token")
                .and_then(Value::as_str)
                .filter(|token| !token.trim().is_empty())
                .ok_or_else(|| "no armed token supplied".to_string())?;
            let token = CloudArmedToken::parse(raw_token)
                .ok_or_else(|| "armed token is malformed".to_string())?;
            if token.nonce.len() < NONCE_MIN_LEN
                || token.verb != verb
                || token.node != node
                || token.target != target
            {
                return Err("armed token does not authorize this verb/node/target".to_string());
            }
            if token.request_sha256 != cloud_request_digest(body).map_err(str::to_string)? {
                return Err("armed token does not authorize this request body".to_string());
            }
            let now_ms = self.test_now_ms.unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
                    .unwrap_or(0)
            });
            if now_ms > token.expires_at_ms {
                return Err("armed token has expired".to_string());
            }
            if token.expires_at_ms > now_ms.saturating_add(MAX_TTL_MS) {
                return Err("armed token exceeds the 30-second lifetime".to_string());
            }
            let signer = self
                .signer
                .as_ref()
                .ok_or_else(|| "Bus mutation credential is unavailable".to_string())?;
            if !signer.verify_payload(&token.signing_payload(), &token.signature) {
                return Err("armed token signature did not verify".to_string());
            }
            claim_nonce(&self.auth_root, &token.nonce, token.expires_at_ms)
        }
    }

    pub(crate) fn mint_body(
        unsigned_body: &str,
        verb: &str,
        target: &str,
    ) -> Result<String, String> {
        let mut document: Value = serde_json::from_str(unsigned_body)
            .map_err(|error| format!("request body is not valid JSON: {error}"))?;
        let object = document
            .as_object_mut()
            .ok_or_else(|| "request body must be a JSON object".to_string())?;
        object.remove("armed_token");
        object.insert(
            "schema_version".to_string(),
            Value::from(CLOUD_ACTION_SCHEMA_VERSION),
        );
        let node = local_node();
        for (label, value) in [("verb", verb), ("node", node.as_str()), ("target", target)] {
            if value.is_empty() || value.len() > 255 || value.contains('|') {
                return Err(format!("capability {label} is not capability-safe"));
            }
        }
        let unsigned = document.to_string();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
            .map_err(|_| "system clock is before the Unix epoch".to_string())?;
        let signer = production_signer()?;
        let token = CloudArmedToken::mint(
            &signer,
            &fresh_nonce()?,
            now_ms.saturating_add(MAX_TTL_MS),
            verb,
            &node,
            target,
            &cloud_request_digest(&unsigned).map_err(str::to_string)?,
        )
        .encode();
        document
            .as_object_mut()
            .expect("validated JSON object")
            .insert("armed_token".to_string(), Value::String(token));
        serde_json::to_string(&document).map_err(|error| format!("encode request: {error}"))
    }

    fn claim_nonce(root: &Path, nonce: &str, expires_at_ms: i64) -> Result<(), String> {
        use std::io::Write as _;
        use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

        // Encode the complete nonce rather than hashing it: this keeps the
        // replay filename collision-free without adding another crate edge,
        // while the byte-to-hex form is always a safe single path component.
        if nonce.is_empty() || nonce.len() > 256 {
            return Err("armed-token nonce has an invalid length".to_string());
        }
        let filename: String = nonce
            .as_bytes()
            .iter()
            .flat_map(|byte| [byte >> 4, byte & 0x0f])
            .map(|nibble| b"0123456789abcdef"[usize::from(nibble)] as char)
            .collect();

        let dir = root.join("spent-nonces");
        std::fs::create_dir_all(&dir)
            .map_err(|error| format!("create armed-token replay store: {error}"))?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("secure armed-token replay store: {error}"))?;
        let path = dir.join(filename);
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err("armed token was already used".to_string());
            }
            Err(error) => return Err(format!("claim armed-token nonce: {error}")),
        };
        file.write_all(expires_at_ms.to_string().as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("persist armed-token nonce: {error}"))
    }
}

/// The verb the shell's editor mount drains: open a file path in `Surface::Editor`.
///
/// A JSON boundary — this crate owns the request shape; the shell parses it with
/// [`EditorOpenRequest::from_body`] and calls its editor's `open_path`.
pub const ACTION_EDITOR_OPEN: &str = "action/editor/open";

/// The `action/editor/open` request body: the file to open in the Editor surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorOpenRequest {
    /// The file to open (absolute — resolved from the selected row's path).
    pub path: PathBuf,
}

impl EditorOpenRequest {
    /// Build a request to open `path`.
    #[must_use]
    pub fn new<P: Into<PathBuf>>(path: P) -> Self {
        Self { path: path.into() }
    }

    /// Serialize to the JSON wire body. An (impossible) serialize failure yields an
    /// empty string — honest, never a panic (`unwrap_used`/`panic` are lint-denied).
    #[must_use]
    pub fn to_body(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Parse a wire body back into a request; `None` on malformed JSON (the drain
    /// skips it rather than acting on a garbled path).
    #[must_use]
    pub fn from_body(body: &str) -> Option<Self> {
        serde_json::from_str(body).ok()
    }
}

#[cfg(feature = "dbus")]
pub use bus::{BusEditorLaunch, EditorLaunchWatch};

#[cfg(feature = "dbus")]
mod bus {
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use mde_bus::hooks::config::Priority;
    use mde_bus::persist::Persist;

    use super::{capability, EditorOpenRequest, ACTION_EDITOR_OPEN};

    fn authorizer_for_root(root: Option<&Path>) -> capability::Authorizer {
        #[cfg(test)]
        {
            let auth_root = root
                .map(|root| root.join("auth"))
                .unwrap_or_else(|| std::env::temp_dir().join("mde-files-editor-auth"));
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
                .unwrap_or(0);
            capability::Authorizer::for_test(
                b"mde-files-capability-test-key",
                auth_root,
                // The sender and watcher are deliberately constructed in
                // either order by the edge-trigger tests. Give the verifier
                // a small deterministic lead so a token minted immediately
                // afterward is still within the exact 30-second test window.
                now_ms.saturating_add(5_000),
            )
        }
        #[cfg(not(test))]
        {
            let _ = root;
            capability::Authorizer::production()
        }
    }

    /// The live sender — a synchronous local `Persist` write onto
    /// [`ACTION_EDITOR_OPEN`], the same persist-first path `BusChatBridge` /
    /// `BusMeshMount` take. Holds only the resolved Bus spool dir; a fresh
    /// `Persist` opens per call (it isn't `Send`).
    pub struct BusEditorLaunch {
        /// The resolved Bus client spool dir, or `None` when this node has no Bus.
        bus_root: Option<PathBuf>,
    }

    impl BusEditorLaunch {
        /// Resolve the Bus spool dir from the environment (the production path).
        #[must_use]
        pub fn from_env() -> Self {
            Self::with_root(mde_bus::client_data_dir())
        }

        /// Construct with an explicit spool root (tests point this at a tempdir, or
        /// `None` to exercise the honest no-Bus no-op).
        #[must_use]
        pub fn with_root(bus_root: Option<PathBuf>) -> Self {
            Self { bus_root }
        }

        /// Post an open request for `path`. Best-effort — a missing Bus / open
        /// failure is a silent no-op, never a panic.
        pub fn send(&self, path: &Path) {
            let Some(root) = self.bus_root.clone() else {
                return; // no Bus on this node — the honest solo-host no-op
            };
            let Ok(persist) = Persist::open(root) else {
                return; // a transient open failure = a silent no-op
            };
            let unsigned = EditorOpenRequest::new(path).to_body();
            let Ok(body) = capability::mint_body(
                &unsigned,
                capability::EDITOR_OPEN_VERB,
                capability::EDITOR_OPEN_TARGET,
            ) else {
                return; // missing credential/randomness = fail closed
            };
            let _ = persist.write(ACTION_EDITOR_OPEN, Priority::Default, None, Some(&body));
        }
    }

    /// The cadence for [`EditorLaunchWatch::take`] — the shell calls it every frame,
    /// so the Bus is read at most this often.
    const POLL: Duration = Duration::from_millis(300);

    /// The shell-side drain: reads the newest not-yet-seen [`ACTION_EDITOR_OPEN`]
    /// request (edge-triggered on the ULID cursor, so each request fires once),
    /// cadence-gated so a per-frame call is cheap. Degrades to `None` with no Bus.
    pub struct EditorLaunchWatch {
        /// The resolved Bus client spool dir, or `None` when this node has no Bus.
        bus_root: Option<PathBuf>,
        authorizer: capability::Authorizer,
        /// The last request ULID acted on — the `list_since` cursor (edge-trigger).
        last_ulid: Option<String>,
        /// When the Bus was last read, for the [`POLL`] cadence gate.
        last_poll: Option<Instant>,
    }

    impl EditorLaunchWatch {
        /// Resolve the Bus spool dir from the environment (the production path).
        #[must_use]
        pub fn from_env() -> Self {
            Self::with_root(mde_bus::client_data_dir())
        }

        /// Construct with an explicit spool root (tests point this at a tempdir).
        #[must_use]
        pub fn with_root(bus_root: Option<PathBuf>) -> Self {
            Self {
                authorizer: authorizer_for_root(bus_root.as_deref()),
                bus_root,
                last_ulid: None,
                last_poll: None,
            }
        }

        /// The newest unseen open request's path, if one has landed since the last
        /// read. Cadence-gated (returns `None` until [`POLL`] has elapsed), then
        /// drains the Bus edge-triggered. Honest no-Bus / dark-Bus → `None`.
        pub fn take(&mut self) -> Option<PathBuf> {
            let due = self.last_poll.is_none_or(|t| t.elapsed() >= POLL);
            if !due {
                return None;
            }
            self.last_poll = Some(Instant::now());
            self.drain()
        }

        /// Read the newest request past the ULID cursor, advancing it — the core
        /// edge-triggered drain, ignoring the cadence (so tests can exercise it
        /// directly). A malformed body advances the cursor and yields `None` (skip).
        fn drain(&mut self) -> Option<PathBuf> {
            let root = self.bus_root.clone()?;
            let persist = Persist::open(root).ok()?;
            let msgs = persist
                .list_since(ACTION_EDITOR_OPEN, self.last_ulid.as_deref())
                .ok()?;
            let newest = msgs.last()?;
            self.last_ulid = Some(newest.ulid.clone());
            let body = newest.body.as_deref()?;
            if self
                .authorizer
                .authorize(
                    body,
                    capability::EDITOR_OPEN_VERB,
                    &capability::local_node(),
                    capability::EDITOR_OPEN_TARGET,
                )
                .is_err()
            {
                return None;
            }
            EditorOpenRequest::from_body(body).map(|req| req.path)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{BusEditorLaunch, EditorLaunchWatch, ACTION_EDITOR_OPEN};
        use std::path::{Path, PathBuf};
        use std::time::{SystemTime, UNIX_EPOCH};

        /// A unique temp dir used as a Bus spool root, cleaned up on drop.
        struct TempDir(PathBuf);
        impl TempDir {
            fn new(tag: &str) -> Self {
                let base = std::env::temp_dir().join(format!(
                    "mde-files-editor-open-{}-{}-{}",
                    tag,
                    std::process::id(),
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_nanos())
                        .unwrap_or(0)
                ));
                std::fs::create_dir_all(&base).expect("create temp bus root");
                Self(base)
            }
        }
        impl Drop for TempDir {
            fn drop(&mut self) {
                std::fs::remove_dir_all(&self.0).ok();
            }
        }

        #[test]
        fn send_then_drain_round_trips_the_exact_path() {
            let bus = TempDir::new("rt");
            let file = Path::new("/home/matthew/notes/todo.rs");
            BusEditorLaunch::with_root(Some(bus.0.clone())).send(file);

            let mut watch = EditorLaunchWatch::with_root(Some(bus.0.clone()));
            assert_eq!(
                watch.take(),
                Some(file.to_path_buf()),
                "the drained request carries the exact posted path"
            );
        }

        #[test]
        fn drain_is_edge_triggered_and_fires_once_per_request() {
            let bus = TempDir::new("edge");
            let sender = BusEditorLaunch::with_root(Some(bus.0.clone()));
            let mut watch = EditorLaunchWatch::with_root(Some(bus.0.clone()));

            sender.send(Path::new("/tmp/one.txt"));
            assert_eq!(watch.drain(), Some(PathBuf::from("/tmp/one.txt")));
            // Already consumed — the ULID cursor advanced, so it does not re-fire.
            assert_eq!(watch.drain(), None, "a consumed request fires only once");

            // A second request past the cursor is picked up.
            sender.send(Path::new("/tmp/two.txt"));
            assert_eq!(watch.drain(), Some(PathBuf::from("/tmp/two.txt")));
        }

        #[test]
        fn no_bus_root_is_a_silent_no_op() {
            // The honest solo-host path: no Bus dir → send does nothing, drain None.
            BusEditorLaunch::with_root(None).send(Path::new("/tmp/whatever.rs"));
            let mut watch = EditorLaunchWatch::with_root(None);
            assert_eq!(watch.take(), None);
        }

        #[test]
        fn unsigned_editor_open_is_not_drained() {
            let bus = TempDir::new("unsigned");
            let persist = mde_bus::persist::Persist::open(bus.0.clone()).expect("open bus");
            let unsigned = super::super::EditorOpenRequest::new("/tmp/forged.rs").to_body();
            persist
                .write(
                    ACTION_EDITOR_OPEN,
                    mde_bus::hooks::config::Priority::Default,
                    None,
                    Some(&unsigned),
                )
                .expect("publish forged body");

            let mut watch = EditorLaunchWatch::with_root(Some(bus.0.clone()));
            assert_eq!(
                watch.drain(),
                None,
                "unsigned editor request must fail closed"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EditorOpenRequest, ACTION_EDITOR_OPEN};
    use std::path::PathBuf;

    #[test]
    fn the_verb_is_the_action_editor_open_topic() {
        assert_eq!(ACTION_EDITOR_OPEN, "action/editor/open");
    }

    #[test]
    fn body_carries_and_round_trips_the_path() {
        let req = EditorOpenRequest::new("/home/matthew/src/lib.rs");
        let body = req.to_body();
        let value: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(value["path"], "/home/matthew/src/lib.rs");

        let back = EditorOpenRequest::from_body(&body).expect("parses back");
        assert_eq!(back.path, PathBuf::from("/home/matthew/src/lib.rs"));
        assert_eq!(back, req);
    }

    #[test]
    fn malformed_body_parses_to_none() {
        assert!(EditorOpenRequest::from_body("not json").is_none());
        assert!(EditorOpenRequest::from_body("{}").is_none());
    }
}
