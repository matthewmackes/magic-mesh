//! VIRT-5 (v5.0.0) — VM Nebula cert signing via the Mackes Bus.
//!
//! Every peer spawns this worker. On the **CA peer** (detected by
//! presence of `~/.config/mde/nebula/ca.key`), the worker polls
//! `action/compute/cert-sign-request`, calls `nebula-cert sign` for
//! each new request, and publishes the resulting cert PEM + CA PEM
//! to `reply/<request-ulid>` per the Bus RPC convention
//! (`crates/mde-bus/src/rpc.rs`). On **non-CA peers** the same
//! topic is still drained but the handler short-circuits to a log
//! line so the cursor advances and no reply is written — `compute_
//! provision` (VIRT-6) gets its reply from the actual CA peer and
//! never confuses a "not the CA" silence with a slow signer.
//!
//! ## Topic-shape lock
//!
//! `docs/design/v5.0.0-compute.md` §3 notates the request topic as
//! `compute/cert-sign-request/<ulid>`. Per the Q96 + `rpc.rs`
//! `action/<domain>/<verb>` convention (the established RPC idiom on
//! the Mackes Bus), the actual topic shape is
//! `action/compute/cert-sign-request`, with the per-request
//! correlation ULID being the message's own ULID (returned by
//! `Persist::write` for the caller, read off `StoredMessage.ulid`
//! for the responder). The design doc's `/<ulid>` suffix was
//! informal notation meaning "different requests have different
//! correlation ULIDs," not literal topic content. This worker locks
//! the Bus-convention interpretation. (Standing-auth best-choice
//! decision per CLAUDE.md §0 Q83; flagged in worklist follow-up so
//! the design doc gets amended in a separate commit.)
//!
//! ## Outcomes
//!
//! - **Success** — reply body is
//!   `{"cert_pem": "...", "ca_pem": "..."}`.
//! - **Malformed request** — reply body is `{"error": "..."}` with
//!   a parse error description. The CA peer still replies so the
//!   requester can short-circuit its retry instead of timing out
//!   on rpc::DEFAULT_RPC_TIMEOUT (30 s).
//! - **`nebula-cert` failure** — reply body is `{"error": "..."}`
//!   with the subprocess exit description.
//! - **Non-CA peer** — no reply; the requester's `await_reply`
//!   times out per the rpc convention and `compute_provision`
//!   (VIRT-6) handles the timeout-retry-fail logic on its end.
//!
//! Cert temp files are written under `std::env::temp_dir()/
//! mde-cert-sign/<sanitized-cn>.{crt,key}` and removed after the
//! PEM is read into memory. The CA key never leaves the CA peer's
//! disk — only signed cert material crosses the Bus.

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;

use super::{ShutdownToken, Worker};

/// Bus action topic this worker drains. Locked to the
/// `action/<domain>/<verb>` Q96 convention.
pub const ACTION_TOPIC: &str = "action/compute/cert-sign-request";

/// Default poll cadence — control surface per `rpc::CONTROL_POLL_INTERVAL`
/// (cert signing is not on a human's interactive path).
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(400);

/// CA private-key path. Presence on disk is the CA-peer detection
/// signal per design doc §3 implementation notes ("On CA peer
/// (detected by presence of `~/.config/mde/ca.key`)").
pub const DEFAULT_CA_KEY_PATH: &str = "~/.config/mde/nebula/ca.key";

/// CA cert path — read once per request to populate `ca_pem` in
/// the reply so the requester can splice it into the guest's
/// `/etc/nebula/ca.crt`.
pub const DEFAULT_CA_CERT_PATH: &str = "~/.config/mde/nebula/ca.crt";

/// Default Nebula group every signed VM cert carries unless the
/// requester explicitly specifies groups.
pub const DEFAULT_NEBULA_GROUP: &str = "mde-vms";

/// Sign-request payload per design doc §3.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CertSignRequest {
    /// Nebula cert common name — typically `vm-<ulid>`. Restricted
    /// to `[A-Za-z0-9._-]` so it's safe as a filename component.
    pub common_name: String,
    /// VM's Nebula overlay IP in CIDR form (e.g. `10.42.128.1/17`).
    pub ip: String,
    /// Optional groups (defaults to `["mde-vms"]` when empty).
    #[serde(default)]
    pub groups: Vec<String>,
    /// VIRT-6 requester-side keygen (operator lock 2026-05-30): when
    /// present, the requester (`compute_provision`) generated the
    /// keypair locally via `nebula-cert keygen` and sends only the
    /// PUBLIC key here. The CA signs it with `-in-pub` and produces
    /// no private key — the key never crosses the Bus. When absent
    /// (legacy / operator-CLI path), `nebula-cert` generates the
    /// keypair CA-side and the private key is discarded after
    /// signing (the cert is unusable without a key, so the
    /// pubkey-present path is the only production flow).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_key_pem: Option<String>,
}

/// How `nebula-cert sign` sources the keypair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyMode {
    /// `nebula-cert` generates the keypair, writing the private key
    /// to this path (legacy path — the key is then discarded by
    /// [`sign_with_nebula_cert`] since this path has no use for it).
    Generate {
        /// `-out-key` path.
        out_key: String,
    },
    /// Sign an externally-generated public key (`-in-pub`); no
    /// private key is produced CA-side. The VIRT-6 requester-side
    /// keygen flow (operator lock 2026-05-30).
    ExternalPub {
        /// `-in-pub` path.
        in_pub: String,
    },
}

/// Sign-reply payload — `{cert_pem, ca_pem}` on success, `{error}`
/// on failure. All three fields are optional + serde skips Nones so
/// the wire shape stays tight.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CertSignReply {
    /// PEM-encoded signed peer cert on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cert_pem: Option<String>,
    /// PEM-encoded CA cert (copy of `<ca-cert-path>`) on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_pem: Option<String>,
    /// Human-readable error description on failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Worker handle.
pub struct CertAuthorityWorker {
    ca_key_path: PathBuf,
    ca_cert_path: PathBuf,
    poll_interval: Duration,
    bus_root_override: Option<PathBuf>,
}

impl Default for CertAuthorityWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl CertAuthorityWorker {
    /// Construct with production defaults.
    #[must_use]
    pub fn new() -> Self {
        Self {
            ca_key_path: expand_home(DEFAULT_CA_KEY_PATH),
            ca_cert_path: expand_home(DEFAULT_CA_CERT_PATH),
            poll_interval: DEFAULT_POLL_INTERVAL,
            bus_root_override: None,
        }
    }

    /// Override the CA private-key path. Used in tests.
    #[must_use]
    pub fn with_ca_key_path(mut self, p: PathBuf) -> Self {
        self.ca_key_path = p;
        self
    }

    /// Override the CA cert path. Used in tests.
    #[must_use]
    pub fn with_ca_cert_path(mut self, p: PathBuf) -> Self {
        self.ca_cert_path = p;
        self
    }

    /// Override the Bus root directory. Used in tests.
    #[must_use]
    pub fn with_bus_root(mut self, p: PathBuf) -> Self {
        self.bus_root_override = Some(p);
        self
    }

    /// Override the poll cadence. Used in tests.
    #[must_use]
    pub fn with_poll_interval(mut self, d: Duration) -> Self {
        self.poll_interval = d;
        self
    }
}

/// Parse a sign-request JSON body. Returns a human-readable error
/// suitable for use as the `error` field of a `CertSignReply`.
///
/// # Errors
///
/// Any serde-json failure surfaces as a `"malformed cert-sign
/// request: ..."` string.
pub fn parse_sign_request(body: &str) -> Result<CertSignRequest, String> {
    serde_json::from_str(body).map_err(|e| format!("malformed cert-sign request: {e}"))
}

/// CA-peer detection: this peer is the CA iff the CA private key
/// exists on disk at `ca_key_path`. The mesh CA key is operator-
/// protected (root-owned 0600 per NF-2.4 seal), so its mere presence
/// is a reliable signal — no need for a separate `role` flag.
#[must_use]
pub fn is_ca_peer(ca_key_path: &Path) -> bool {
    ca_key_path.exists()
}

/// Validate a request's common_name is safe to use as a filename
/// component + a Nebula cert subject. Only ASCII alphanumerics, dot,
/// underscore, and hyphen are accepted.
///
/// # Errors
///
/// Returns a human-readable error on empty CN or unsafe chars.
pub fn validate_common_name(cn: &str) -> Result<(), String> {
    if cn.is_empty() {
        return Err("common_name empty".into());
    }
    if !cn
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(format!("common_name has unsafe chars: {cn:?}"));
    }
    Ok(())
}

/// Build the logical args for `nebula-cert sign` (name + ip +
/// groups). I/O-specific args (`-ca-crt`, `-ca-key`, `-out-crt`,
/// `-out-key`) are appended by [`sign_with_nebula_cert`] so the
/// logical-args step stays pure + testable without filesystem
/// state. Empty `groups` falls back to [`DEFAULT_NEBULA_GROUP`].
#[must_use]
pub fn build_logical_args(req: &CertSignRequest) -> Vec<String> {
    let groups = if req.groups.is_empty() {
        DEFAULT_NEBULA_GROUP.to_string()
    } else {
        req.groups.join(",")
    };
    vec![
        "sign".into(),
        "-name".into(),
        req.common_name.clone(),
        "-ip".into(),
        req.ip.clone(),
        "-groups".into(),
        groups,
    ]
}

/// Build the full `nebula-cert sign` arg vector: logical args
/// (name/ip/groups) + CA paths + output cert + the key-source arg.
/// Pure — paths are passed in so the assembly is testable without
/// filesystem state. `ExternalPub` emits `-in-pub` and no
/// `-out-key`; `Generate` emits `-out-key` and no `-in-pub`.
#[must_use]
pub fn build_sign_args(
    req: &CertSignRequest,
    ca_crt: &str,
    ca_key: &str,
    out_crt: &str,
    key_mode: &KeyMode,
) -> Vec<String> {
    let mut args = build_logical_args(req);
    args.extend([
        "-ca-crt".into(),
        ca_crt.to_string(),
        "-ca-key".into(),
        ca_key.to_string(),
        "-out-crt".into(),
        out_crt.to_string(),
    ]);
    match key_mode {
        KeyMode::Generate { out_key } => {
            args.extend(["-out-key".into(), out_key.clone()]);
        }
        KeyMode::ExternalPub { in_pub } => {
            args.extend(["-in-pub".into(), in_pub.clone()]);
        }
    }
    args
}

/// Build an error reply JSON body. Always succeeds (degenerate
/// fallback when serde encoding itself fails).
#[must_use]
pub fn build_error_reply(message: &str) -> String {
    let reply = CertSignReply {
        cert_pem: None,
        ca_pem: None,
        error: Some(message.to_string()),
    };
    serde_json::to_string(&reply)
        .unwrap_or_else(|_| r#"{"error":"reply encode failed"}"#.to_string())
}

/// Build a success reply JSON body.
#[must_use]
pub fn build_success_reply(cert_pem: String, ca_pem: String) -> String {
    let reply = CertSignReply {
        cert_pem: Some(cert_pem),
        ca_pem: Some(ca_pem),
        error: None,
    };
    serde_json::to_string(&reply)
        .unwrap_or_else(|_| r#"{"error":"reply encode failed"}"#.to_string())
}

fn sign_with_nebula_cert(
    req: &CertSignRequest,
    ca_key: &Path,
    ca_cert: &Path,
) -> Result<String, String> {
    let tmp_dir = std::env::temp_dir().join("mde-cert-sign");
    std::fs::create_dir_all(&tmp_dir).map_err(|e| format!("mkdir tmp: {e}"))?;
    let cert_path = tmp_dir.join(format!("{}.crt", req.common_name));
    let key_path = tmp_dir.join(format!("{}.key", req.common_name));
    let pub_path = tmp_dir.join(format!("{}.pub", req.common_name));

    // VIRT-6 requester-side keygen: when the request carries a public
    // key, write it to a tmp file + sign with `-in-pub` (no CA-side
    // private key). Otherwise fall back to the legacy generate path.
    let key_mode = if let Some(pub_pem) = &req.public_key_pem {
        std::fs::write(&pub_path, pub_pem).map_err(|e| format!("write pubkey: {e}"))?;
        KeyMode::ExternalPub {
            in_pub: pub_path.to_string_lossy().into_owned(),
        }
    } else {
        KeyMode::Generate {
            out_key: key_path.to_string_lossy().into_owned(),
        }
    };

    let args = build_sign_args(
        req,
        &ca_cert.to_string_lossy(),
        &ca_key.to_string_lossy(),
        &cert_path.to_string_lossy(),
        &key_mode,
    );
    let status = Command::new("nebula-cert")
        .args(&args)
        .status()
        .map_err(|e| format!("nebula-cert spawn: {e}"))?;
    let cleanup = || {
        let _ = std::fs::remove_file(&cert_path);
        let _ = std::fs::remove_file(&key_path);
        let _ = std::fs::remove_file(&pub_path);
    };
    if !status.success() {
        cleanup();
        return Err(format!("nebula-cert exited {status}"));
    }
    let pem = std::fs::read_to_string(&cert_path).map_err(|e| {
        cleanup();
        format!("read cert: {e}")
    })?;
    cleanup();
    Ok(pem)
}

fn handle_request(worker: &CertAuthorityWorker, body: &str) -> Result<String, String> {
    let req = parse_sign_request(body)?;
    validate_common_name(&req.common_name)?;
    let cert_pem = sign_with_nebula_cert(&req, &worker.ca_key_path, &worker.ca_cert_path)?;
    let ca_pem =
        std::fs::read_to_string(&worker.ca_cert_path).map_err(|e| format!("read ca cert: {e}"))?;
    Ok(build_success_reply(cert_pem, ca_pem))
}

fn poll_once(persist: &Persist, worker: &CertAuthorityWorker, cursor: &mut Option<String>) {
    let msgs = match persist.list_since(ACTION_TOPIC, cursor.as_deref()) {
        Ok(m) => m,
        Err(e) => {
            tracing::debug!(error = %e, "cert_authority: list_since failed");
            return;
        }
    };
    for msg in msgs {
        *cursor = Some(msg.ulid.clone());
        if !is_ca_peer(&worker.ca_key_path) {
            tracing::debug!(ulid = %msg.ulid, "cert_authority: non-CA peer, ignoring");
            continue;
        }
        let body = msg.body.as_deref().unwrap_or("");
        let reply_json = match handle_request(worker, body) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(ulid = %msg.ulid, error = %e, "cert_authority: sign failed");
                build_error_reply(&e)
            }
        };
        if let Err(e) = persist.write(
            &reply_topic(&msg.ulid),
            Priority::Default,
            None,
            Some(&reply_json),
        ) {
            tracing::warn!(ulid = %msg.ulid, error = %e, "cert_authority: reply write failed");
        }
    }
}

fn expand_home(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped);
        }
    }
    PathBuf::from(path)
}

/// Home-expanded [`DEFAULT_CA_CERT_PATH`] — the path the CA worker
/// signs against. Exposed so other workers (the EFF-9 metrics
/// exporter's EFF-11 cert-expiry probe) point at the same cert
/// without re-deriving the location.
#[must_use]
pub fn default_ca_cert_path() -> PathBuf {
    expand_home(DEFAULT_CA_CERT_PATH)
}

fn default_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
}

#[async_trait::async_trait]
impl Worker for CertAuthorityWorker {
    fn name(&self) -> &'static str {
        "cert_authority"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let bus_root = match self.bus_root_override.clone().or_else(default_bus_root) {
            Some(r) => r,
            None => {
                tracing::debug!("cert_authority: no bus root; worker idle");
                return Ok(());
            }
        };
        let persist = match Persist::open(bus_root) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(error = %e, "cert_authority: persist open failed; worker idle");
                return Ok(());
            }
        };
        let mut cursor: Option<String> = None;
        let mut tick = tokio::time::interval(self.poll_interval);
        // First `tick().await` resolves immediately — burn it so the
        // first real iteration waits the full interval and we don't
        // hammer the bus on startup.
        tick.tick().await;
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    poll_once(&persist, self, &mut cursor);
                }
                _ = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_sign_request ──

    #[test]
    fn parse_request_happy_path() {
        let req = parse_sign_request(
            r#"{"common_name":"vm-01JAN","ip":"10.42.128.1/17","groups":["mde-vms"]}"#,
        )
        .expect("parse");
        assert_eq!(req.common_name, "vm-01JAN");
        assert_eq!(req.ip, "10.42.128.1/17");
        assert_eq!(req.groups, vec!["mde-vms".to_string()]);
    }

    #[test]
    fn parse_request_groups_optional() {
        let req =
            parse_sign_request(r#"{"common_name":"vm-02","ip":"10.42.128.2/17"}"#).expect("parse");
        assert!(req.groups.is_empty());
    }

    // ── Required scenario 4: malformed request rejected ──

    #[test]
    fn malformed_request_yields_parse_error_reply() {
        let err = parse_sign_request("this is not json").expect_err("should fail");
        assert!(err.contains("malformed"), "got {err}");
        // Builder produces a reply containing the error string.
        let reply = build_error_reply(&err);
        let v: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert!(v["error"].as_str().unwrap().contains("malformed"));
        assert!(!v.as_object().unwrap().contains_key("cert_pem"));
    }

    // ── is_ca_peer (drives required scenarios 1 + 2) ──

    #[test]
    fn ca_peer_when_key_file_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let key = tmp.path().join("ca.key");
        std::fs::write(&key, b"private").unwrap();
        assert!(is_ca_peer(&key));
    }

    // ── Required scenario 2: non-CA peer ignores ──

    #[test]
    fn non_ca_peer_when_key_file_missing() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!is_ca_peer(&tmp.path().join("no-such.key")));
    }

    // ── validate_common_name ──

    #[test]
    fn validate_cn_accepts_clean_string() {
        validate_common_name("vm-01jan.dev_1").expect("clean");
    }

    #[test]
    fn validate_cn_rejects_empty() {
        assert!(validate_common_name("").is_err());
    }

    #[test]
    fn validate_cn_rejects_slashes() {
        assert!(validate_common_name("vm/01").is_err());
    }

    #[test]
    fn validate_cn_rejects_spaces() {
        assert!(validate_common_name("vm 01").is_err());
    }

    // ── build_logical_args ──

    fn req_with(groups: Vec<String>, pubkey: Option<String>) -> CertSignRequest {
        CertSignRequest {
            common_name: "vm-01".into(),
            ip: "10.42.128.1/17".into(),
            groups,
            public_key_pem: pubkey,
        }
    }

    #[test]
    fn logical_args_explicit_groups() {
        let req = req_with(vec!["mde-vms".into(), "ops".into()], None);
        let args = build_logical_args(&req);
        assert_eq!(
            args,
            vec![
                "sign",
                "-name",
                "vm-01",
                "-ip",
                "10.42.128.1/17",
                "-groups",
                "mde-vms,ops",
            ]
        );
    }

    #[test]
    fn logical_args_default_group_when_empty() {
        let req = req_with(vec![], None);
        let args = build_logical_args(&req);
        // The last arg is the group value.
        assert_eq!(args.last().unwrap(), &"mde-vms".to_string());
    }

    // ── VIRT-6 requester-side keygen: build_sign_args key modes ──

    #[test]
    fn sign_args_generate_mode_emits_out_key_not_in_pub() {
        let req = req_with(vec![], None);
        let args = build_sign_args(
            &req,
            "/ca.crt",
            "/ca.key",
            "/out.crt",
            &KeyMode::Generate {
                out_key: "/out.key".into(),
            },
        );
        assert!(args.contains(&"-out-key".to_string()));
        assert!(args.contains(&"/out.key".to_string()));
        assert!(!args.contains(&"-in-pub".to_string()));
        // CA paths present.
        assert!(args.contains(&"/ca.key".to_string()));
        assert!(args.contains(&"/out.crt".to_string()));
    }

    #[test]
    fn sign_args_external_pub_mode_emits_in_pub_not_out_key() {
        let req = req_with(vec![], Some("PUBKEY".into()));
        let args = build_sign_args(
            &req,
            "/ca.crt",
            "/ca.key",
            "/out.crt",
            &KeyMode::ExternalPub {
                in_pub: "/host.pub".into(),
            },
        );
        assert!(args.contains(&"-in-pub".to_string()));
        assert!(args.contains(&"/host.pub".to_string()));
        assert!(
            !args.contains(&"-out-key".to_string()),
            "external-pub signing must NOT generate a CA-side private key"
        );
    }

    #[test]
    fn parse_request_carries_public_key_when_present() {
        let req = parse_sign_request(
            r#"{"common_name":"vm-03","ip":"10.42.129.1/17","public_key_pem":"---PUB---"}"#,
        )
        .expect("parse");
        assert_eq!(req.public_key_pem.as_deref(), Some("---PUB---"));
    }

    #[test]
    fn parse_request_public_key_defaults_none_for_legacy_clients() {
        let req =
            parse_sign_request(r#"{"common_name":"vm-04","ip":"10.42.129.2/17"}"#).expect("parse");
        assert!(req.public_key_pem.is_none());
    }

    // ── Required scenario 1: CA peer replies (success-shape) ──

    #[test]
    fn success_reply_shape_carries_both_pems() {
        let body = build_success_reply("CERT_PEM".into(), "CA_PEM".into());
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["cert_pem"], "CERT_PEM");
        assert_eq!(v["ca_pem"], "CA_PEM");
        assert!(!v.as_object().unwrap().contains_key("error"));
    }

    #[test]
    fn error_reply_shape_omits_pems() {
        let body = build_error_reply("nebula-cert exited 1");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["error"], "nebula-cert exited 1");
        assert!(!v.as_object().unwrap().contains_key("cert_pem"));
        assert!(!v.as_object().unwrap().contains_key("ca_pem"));
    }

    // ── Required scenario 3: timeout handled (the compute_provision
    //    requester uses rpc::await_reply with rpc::DEFAULT_RPC_TIMEOUT
    //    = 30 s; this worker's contract is to publish under the
    //    `action/` prefix so the requester's rpc::publish_request
    //    accepts the topic). ──

    #[test]
    fn action_topic_under_action_prefix() {
        // rpc::publish_request rejects any topic outside `action/`,
        // so locking ACTION_TOPIC's prefix here guarantees the VIRT-6
        // requester can publish via the canonical RPC API.
        assert!(
            ACTION_TOPIC.starts_with("action/"),
            "ACTION_TOPIC {ACTION_TOPIC:?} must start with `action/`"
        );
    }

    #[test]
    fn action_topic_is_canonical_three_segments() {
        // Locks the action/<domain>/<verb> three-segment shape so a
        // future rename doesn't accidentally collide with another
        // namespace.
        let parts: Vec<&str> = ACTION_TOPIC.split('/').collect();
        assert_eq!(parts, vec!["action", "compute", "cert-sign-request"]);
    }

    // ── handle_request integration on a non-CA peer + malformed input ──

    #[test]
    fn handle_request_surfaces_parse_error_string() {
        let tmp = tempfile::tempdir().unwrap();
        let worker = CertAuthorityWorker::new()
            .with_ca_key_path(tmp.path().join("ca.key"))
            .with_ca_cert_path(tmp.path().join("ca.crt"));
        let err = handle_request(&worker, "garbage").expect_err("malformed");
        assert!(err.contains("malformed"), "{err}");
    }

    #[test]
    fn handle_request_rejects_unsafe_common_name() {
        let tmp = tempfile::tempdir().unwrap();
        let worker = CertAuthorityWorker::new()
            .with_ca_key_path(tmp.path().join("ca.key"))
            .with_ca_cert_path(tmp.path().join("ca.crt"));
        let body = r#"{"common_name":"vm/01","ip":"10.42.128.1/17"}"#;
        let err = handle_request(&worker, body).expect_err("unsafe cn");
        assert!(err.contains("unsafe chars"), "{err}");
    }
}
