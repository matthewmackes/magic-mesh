//! VOIP-28 — pure-Rust SIP signaling for the softphone (NOT PJSIP, per
//! CLAUDE.md §1's pure-Rust lock; operator decision 2026-06-07).
//!
//! Slice 1: a SIP account loaded from `~/.config/mde/voice/account.toml` and a
//! real `REGISTER` over UDP with RFC 2617 / RFC 7616 digest auth. Requests are
//! built as SIP text (the wire protocol is text — simple + byte-testable);
//! responses are parsed with `rsip`, and the digest response is produced by
//! `rsip::services::DigestGenerator` (its own md-5/sha2-backed implementation,
//! so no separate crypto dep). The live registrar round-trip needs a running
//! SIP server → that is the SIP-server bench; everything here that does not
//! touch the socket is unit-tested.

use std::fmt::Write as _;
use std::net::{ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rsip::headers::auth::{Algorithm, AuthQop, Qop};
use rsip::headers::untyped::ToTypedHeader;
use rsip::headers::Header;
use rsip::services::DigestGenerator;
use rsip::{Method, Uri};

use crate::secure::{SecurityPolicy, SipTransport};

/// A SIP account, the credentials the softphone registers with.
///
/// VOIP-GW-4 — this is the **inbound sub-account** half of the split: the
/// per-node Vitelity sub-account this node REGISTERs as its callable identity.
/// The shared-outbound trunk config (fleet caller-ID) is a separate
/// [`SharedOutbound`]; outbound PSTN is bridged onto the shared trunk
/// Vitelity-side, so this node performs a **single** REGISTER — of the
/// sub-account only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SipAccount {
    pub username: String,
    pub password: String,
    pub server_host: String,
    pub server_port: u16,
    pub display_name: String,
    pub expires: u32,
    /// The confidentiality policy for this account's REGISTER leg (lock 17):
    /// prefer TLS with an honest UDP fallback, require TLS, or UDP-only.
    pub security: SecurityPolicy,
}

/// VOIP-GW-4 — the fleet-level **shared-outbound** trunk config (lock 4/13).
///
/// The ONE shared Vitelity account's caller-ID + trunk, held once for the
/// fleet. A node does NOT register this — Vitelity bridges the sub-account's
/// outbound onto the shared trunk — so it carries no SIP password here; it is
/// purely the caller-ID this node's external calls present, published so the
/// callee (and the panel) see the shared number.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SharedOutbound {
    /// The shared caller-ID number presented on all outbound PSTN (the ONE
    /// fleet number; verified on the callee).
    pub caller_id: String,
    /// The shared trunk host (informational — the bridge is configured
    /// Vitelity-side; the client never registers it).
    pub trunk_host: String,
}

/// VOIP-GW-4 — the split voice config a node loads: the inbound sub-account it
/// registers, plus the optional fleet shared-outbound trunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceAccounts {
    /// The per-node sub-account this node REGISTERs (its inbound identity).
    pub inbound: SipAccount,
    /// The fleet shared-outbound trunk (caller-ID). `None` on a legacy
    /// single-account config or a registrar-less P2P identity.
    pub outbound: Option<SharedOutbound>,
}

/// On-disk shape of `account.toml`.
///
/// Back-compat: the legacy **flat** form (`username`/`server`/… at top level)
/// still parses. VOIP-GW-4 adds the split form with `[inbound_sub]` +
/// `[shared_outbound]` tables; [`SipAccount::from_toml`] accepts either.
#[derive(serde::Deserialize)]
struct AccountFile {
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
    /// Registrar, as `host` or `host:port`.
    #[serde(default)]
    server: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default = "default_expires")]
    expires: u32,
    /// Transport policy token (`prefer-tls` / `require-tls` / `udp`). Absent →
    /// the secure default (`prefer-tls`).
    #[serde(default)]
    transport: Option<String>,
    /// VOIP-GW-4 split form: the inbound sub-account table.
    #[serde(default)]
    inbound_sub: Option<InboundSubFile>,
    /// VOIP-GW-4 split form: the shared-outbound trunk table.
    #[serde(default)]
    shared_outbound: Option<SharedOutboundFile>,
}

/// The `[inbound_sub]` table of the split config.
#[derive(serde::Deserialize)]
struct InboundSubFile {
    username: String,
    #[serde(default)]
    password: String,
    server: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default = "default_expires")]
    expires: u32,
    #[serde(default)]
    transport: Option<String>,
}

/// The `[shared_outbound]` table of the split config.
#[derive(serde::Deserialize)]
struct SharedOutboundFile {
    #[serde(default)]
    caller_id: String,
    #[serde(default, alias = "trunk")]
    trunk_host: String,
}

fn default_expires() -> u32 {
    3600
}

impl SipAccount {
    /// `~/.config/mde/voice/account.toml` (XDG `config_dir`).
    pub fn config_path() -> std::path::PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| std::path::PathBuf::from(".config"))
            .join("mde")
            .join("voice")
            .join("account.toml")
    }

    /// The mesh-wide gateway file on QNM-Shared (`<workgroup_root>/voip/gateway.toml`),
    /// written by the Workbench SIP Gateway panel via mackesd (VOIP-GW-1). Same
    /// `account.toml` shape, so it parses with [`from_toml`](Self::from_toml).
    #[must_use]
    pub fn mesh_gateway_path() -> std::path::PathBuf {
        mackes_mesh_types::peers::default_workgroup_root()
            .join("voip")
            .join("gateway.toml")
    }

    /// Load the account: the **mesh-wide** gateway (QNM-Shared, set once in the
    /// Workbench for all clients) wins, then a node-local `account.toml`, else
    /// `None` (P2P-only — the HUD shows "Not registered"). VOIP-GW-1.
    ///
    /// This returns the inbound sub-account alone (back-compat with every
    /// existing caller); [`load_accounts`](Self::load_accounts) returns the
    /// full split (inbound + shared-outbound).
    pub fn load() -> Option<SipAccount> {
        Self::load_accounts().map(|a| a.inbound)
    }

    /// VOIP-GW-4 — load the full split voice config (inbound sub-account +
    /// optional shared-outbound trunk). Same source precedence as [`load`]: the
    /// mesh-wide gateway wins, then the node-local file.
    ///
    /// [`load`](Self::load) is this with the outbound dropped, so existing
    /// single-account callers are unchanged.
    #[must_use]
    pub fn load_accounts() -> Option<VoiceAccounts> {
        if let Ok(text) = std::fs::read_to_string(Self::mesh_gateway_path()) {
            if let Ok(accts) = Self::accounts_from_toml(&text) {
                return Some(accts);
            }
        }
        let text = std::fs::read_to_string(Self::config_path()).ok()?;
        Self::accounts_from_toml(&text).ok()
    }

    /// VOIP-P2P — a registrar-less local identity for direct peer calls: no
    /// `account.toml` required. The username is this node's hostname (the mesh
    /// name peers dial); there is no registrar `server_host`. Used as the
    /// From/Contact identity by `place_call_direct` when the node has no SIP
    /// account configured.
    #[must_use]
    pub fn local_identity() -> SipAccount {
        let host = std::fs::read_to_string("/proc/sys/kernel/hostname")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "mde".to_string());
        SipAccount {
            username: host.clone(),
            password: String::new(),
            server_host: String::new(),
            server_port: 5060,
            display_name: host,
            expires: 0,
            // Registrar-less P2P: no REGISTER leg, so no TLS to attempt.
            security: SecurityPolicy::UdpOnly,
        }
    }

    /// Parse the inbound sub-account alone — a test-only convenience over
    /// [`accounts_from_toml`](Self::accounts_from_toml) (production code loads
    /// the full split via [`load_accounts`](Self::load_accounts)).
    #[cfg(test)]
    fn from_toml(text: &str) -> Result<SipAccount, String> {
        Self::accounts_from_toml(text).map(|a| a.inbound)
    }

    /// VOIP-GW-4 — parse the split config, accepting both the new
    /// `[inbound_sub]`/`[shared_outbound]` form and the legacy flat form.
    fn accounts_from_toml(text: &str) -> Result<VoiceAccounts, String> {
        let f: AccountFile = toml::from_str(text).map_err(|e| e.to_string())?;

        // Split form: the `[inbound_sub]` table is the registered identity.
        let (username, password, server, display_name, expires, transport) =
            if let Some(sub) = f.inbound_sub {
                (
                    sub.username,
                    sub.password,
                    sub.server,
                    sub.display_name,
                    sub.expires,
                    sub.transport,
                )
            } else {
                // Legacy flat form.
                (
                    f.username,
                    f.password,
                    f.server,
                    f.display_name,
                    f.expires,
                    f.transport,
                )
            };

        let security = SecurityPolicy::parse(transport.as_deref().unwrap_or_default());
        // Default the primary port to 5060 (back-compat with every legacy flat
        // `account.toml`). The TLS leg targets 5061 via [`SipAccount::tls_addr`]
        // unless the operator pinned an explicit non-default port.
        let (server_host, server_port) = split_host_port(&server, SipTransport::Udp.default_port());
        if username.trim().is_empty() || server_host.is_empty() {
            return Err("voice config needs a username and a server".to_string());
        }
        let display_name = display_name
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| username.clone());
        let inbound = SipAccount {
            username,
            password,
            server_host,
            server_port,
            display_name,
            expires: expires.max(1),
            security,
        };

        let outbound = f.shared_outbound.map(|o| SharedOutbound {
            caller_id: o.caller_id,
            trunk_host: o.trunk_host,
        });

        Ok(VoiceAccounts { inbound, outbound })
    }

    /// `user@host` address-of-record.
    fn aor(&self) -> String {
        format!("sip:{}@{}", self.username, self.server_host)
    }

    /// The registrar request-URI (`sip:host`).
    fn registrar_uri(&self) -> String {
        format!("sip:{}", self.server_host)
    }

    /// VOIP-GW-4 — the port for a given transport. The configured
    /// `server_port` is the UDP port; the TLS leg uses 5061 unless the operator
    /// pinned an explicit non-default (`!= 5060`) port, which is honored as-is.
    #[must_use]
    pub const fn port_for(&self, transport: SipTransport) -> u16 {
        match transport {
            SipTransport::Udp => self.server_port,
            SipTransport::Tls => {
                if self.server_port == SipTransport::Udp.default_port() {
                    SipTransport::Tls.default_port()
                } else {
                    self.server_port
                }
            }
        }
    }
}

/// Split `host` / `host:port`, defaulting the port.
fn split_host_port(server: &str, default_port: u16) -> (String, u16) {
    match server.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() => match p.parse::<u16>() {
            Ok(port) => (h.to_string(), port),
            Err(_) => (server.to_string(), default_port),
        },
        _ => (server.to_string(), default_port),
    }
}

/// Live registration state shown in the HUD topbar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationState {
    /// No `account.toml` — nothing to register.
    NoAccount,
    /// A REGISTER is in flight.
    Registering,
    /// The registrar returned 200 OK.
    Registered { server: String, expires: u32 },
    /// The attempt failed (timeout, rejected, unreachable …).
    Failed(String),
}

impl RegistrationState {
    /// One-line topbar label.
    pub fn label(&self) -> String {
        match self {
            RegistrationState::NoAccount => "Not registered".to_string(),
            RegistrationState::Registering => "Registering…".to_string(),
            RegistrationState::Registered { server, .. } => format!("Registered · {server}"),
            RegistrationState::Failed(_) => "Registration failed".to_string(),
        }
    }
}

// ── VOIP-28 / E7.5: publish agent status to the Bus ─────────────────────────
//
// The persistent SIP agent lives inside this process, so a separate reader (the
// `mde birthright` commissioning dashboard) can't see its registration state
// directly. The agent therefore publishes a small `state/voice/status` snapshot
// to the Bus on each registration change and on a heartbeat, stamped with a
// wall-clock time so a stale snapshot (a dead agent) is detectable. `Persist`
// writes are synchronous — no runtime needed on the agent thread.

/// The Bus topic the agent publishes its status to.
pub const VOICE_STATUS_TOPIC: &str = "state/voice/status";

/// How often the running agent re-publishes its status, so a reader can tell a
/// live agent (fresh `ts`) from a crashed one (stale `ts`). A reader's
/// staleness window should be a small multiple of this (birthright uses 45s).
pub const STATUS_HEARTBEAT_SECS: u64 = 15;

/// VOIP-P2P — the well-known SIP port the registrar-less agent listens on over
/// the overlay, so a peer dialing `sip:<peer>@<overlay-ip>:5060` (see
/// `place_call_direct`) reaches it.
pub const P2P_SIP_PORT: u16 = 5060;

/// VOIP-P2P — the mesh Nebula overlay subnet anchor. Routing a socket toward it
/// yields this node's own overlay source IP (the address peers reach it on),
/// without enumerating interfaces. The lighthouse sits at `.1`.
const MESH_OVERLAY_ANCHOR: &str = "10.42.0.1:5060";

/// VOIP-P2P — discover this node's overlay (Nebula) source IP by asking the
/// kernel which local address it would use to reach the overlay anchor. Returns
/// `None` when the node has no route onto the overlay (voice P2P unavailable).
fn overlay_source_ip() -> Option<String> {
    let anchor: std::net::SocketAddr = MESH_OVERLAY_ANCHOR.parse().ok()?;
    let ip = route_source_ip(anchor)?;
    // Only accept an address actually on the overlay subnet — a default-route
    // (public) source means there is no overlay path.
    ip.starts_with("10.42.").then_some(ip)
}

/// Current wall-clock seconds since the Unix epoch (0 if the clock is before it).
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Pure JSON builder for the `state/voice/status` body — split out so it is
/// unit-tested without a Bus.
fn voice_status_json(reg: &RegistrationState, listening: bool, ts: u64) -> String {
    let (registered, server, detail) = match reg {
        RegistrationState::Registered { server, .. } => (true, server.clone(), reg.label()),
        RegistrationState::NoAccount
        | RegistrationState::Registering
        | RegistrationState::Failed(_) => (false, String::new(), reg.label()),
    };
    serde_json::json!({
        "registered": registered,
        "listening": listening,
        "server": server,
        "detail": detail,
        "ts": ts,
    })
    .to_string()
}

/// Publish the agent's current status to the Bus (best-effort; a missing Bus or
/// write error is logged at debug and ignored — status is advisory).
pub fn publish_voice_status(reg: &RegistrationState, listening: bool) {
    let Some(dir) = mde_bus::default_data_dir() else {
        return;
    };
    let Ok(persist) = mde_bus::persist::Persist::open(dir) else {
        return;
    };
    let body = voice_status_json(reg, listening, now_unix());
    if let Err(e) = persist.write(
        VOICE_STATUS_TOPIC,
        mde_bus::hooks::config::Priority::Default,
        None,
        Some(&body),
    ) {
        tracing::debug!(error = %e, "voice agent: status publish failed");
    }
}

/// A parsed `WWW-Authenticate` / `Proxy-Authenticate` digest challenge.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Challenge {
    realm: String,
    nonce: String,
    qop: Option<Qop>,
    algorithm: Algorithm,
    opaque: Option<String>,
    /// 407 (proxy) vs 401 (registrar) — picks the Authorization header name.
    proxy: bool,
}

/// Per-attempt transaction identifiers (Call-ID / tag / branch / CSeq). Kept
/// separate so the builder is a pure function the tests can pin.
#[derive(Debug, Clone)]
struct TxnIds {
    call_id: String,
    from_tag: String,
    branch: String,
    cseq: u32,
}

/// Build a REGISTER request as SIP text. `auth` is an optional
/// `(header_name, value)` for the authorized retry.
fn build_register(
    account: &SipAccount,
    local_host: &str,
    local_port: u16,
    transport: SipTransport,
    ids: &TxnIds,
    auth: Option<(&str, &str)>,
) -> String {
    let aor = account.aor();
    let contact = format!("sip:{}@{local_host}:{local_port}", account.username);
    let version = env!("CARGO_PKG_VERSION");
    let mut m = String::new();
    let _ = write!(m, "REGISTER {} SIP/2.0\r\n", account.registrar_uri());
    let _ = write!(
        m,
        "Via: SIP/2.0/{} {local_host}:{local_port};branch={};rport\r\n",
        transport.token(),
        ids.branch
    );
    m.push_str("Max-Forwards: 70\r\n");
    let _ = write!(m, "From: <{aor}>;tag={}\r\n", ids.from_tag);
    let _ = write!(m, "To: <{aor}>\r\n");
    let _ = write!(m, "Call-ID: {}\r\n", ids.call_id);
    let _ = write!(m, "CSeq: {} REGISTER\r\n", ids.cseq);
    let _ = write!(m, "Contact: <{contact}>\r\n");
    let _ = write!(m, "Expires: {}\r\n", account.expires);
    let _ = write!(m, "User-Agent: MCNF Voice/{version}\r\n");
    if let Some((name, value)) = auth {
        let _ = write!(m, "{name}: {value}\r\n");
    }
    m.push_str("Content-Length: 0\r\n\r\n");
    m
}

/// Render an `Algorithm` as its SIP token.
fn algorithm_token(a: Algorithm) -> &'static str {
    match a {
        Algorithm::Md5 => "MD5",
        Algorithm::Md5Sess => "MD5-sess",
        Algorithm::Sha256 => "SHA-256",
        Algorithm::Sha256Sess => "SHA-256-sess",
        Algorithm::Sha512 => "SHA-512",
        Algorithm::Sha512Sess => "SHA-512-sess",
    }
}

/// Compute the digest response and render the matching `Authorization` header
/// value. `nc` is the nonce-count for `qop` challenges.
fn authorization_value(
    account: &SipAccount,
    ch: &Challenge,
    cnonce: &str,
    nc: u8,
) -> Result<String, String> {
    let uri = Uri::try_from(account.registrar_uri()).map_err(|e| e.to_string())?;
    let method = Method::Register;
    let qop = match &ch.qop {
        Some(Qop::Auth) => Some(AuthQop::Auth {
            cnonce: cnonce.to_string(),
            nc,
        }),
        Some(Qop::AuthInt) => Some(AuthQop::AuthInt {
            cnonce: cnonce.to_string(),
            nc,
        }),
        None => None,
    };
    let response = DigestGenerator {
        username: &account.username,
        password: &account.password,
        nonce: &ch.nonce,
        uri: &uri,
        realm: &ch.realm,
        method: &method,
        qop: qop.as_ref(),
        algorithm: ch.algorithm,
    }
    .compute();

    let mut v = format!(
        "Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", response=\"{}\", algorithm={}",
        account.username,
        ch.realm,
        ch.nonce,
        account.registrar_uri(),
        response,
        algorithm_token(ch.algorithm),
    );
    if ch.qop.is_some() {
        let qop_tok = match &ch.qop {
            Some(Qop::AuthInt) => "auth-int",
            _ => "auth",
        };
        // nc is formatted to match DigestGenerator (decimal, 8-wide).
        let _ = write!(v, ", qop={qop_tok}, cnonce=\"{cnonce}\", nc={nc:08}");
    }
    if let Some(opaque) = &ch.opaque {
        let _ = write!(v, ", opaque=\"{opaque}\"");
    }
    Ok(v)
}

/// Extract a digest challenge from a 401/407 response.
///
/// Tries rsip's typed parser first, then falls back to a manual
/// param parse (AUD6-5): rsip 0.4 only accepts the non-RFC
/// `SHA256` spelling, so a registrar sending the RFC 7616 token
/// `algorithm=SHA-256` fails `typed()` wholesale — without the
/// fallback, an RFC-compliant SHA-256 challenge broke
/// registration outright.
fn parse_challenge(resp: &rsip::Response) -> Option<Challenge> {
    use rsip::headers::untyped::UntypedHeader;
    for h in resp.headers.iter() {
        match h {
            Header::WwwAuthenticate(w) => {
                if let Ok(t) = w.typed() {
                    return Some(Challenge {
                        realm: t.realm,
                        nonce: t.nonce,
                        qop: t.qop,
                        algorithm: t.algorithm.unwrap_or(Algorithm::Md5),
                        opaque: t.opaque,
                        proxy: false,
                    });
                }
                if let Some(ch) = parse_challenge_value(w.value(), false) {
                    return Some(ch);
                }
            }
            Header::ProxyAuthenticate(p) => {
                if let Ok(t) = p.typed() {
                    // `ProxyAuthenticate` is a newtype around `WwwAuthenticate`.
                    let t = t.0;
                    return Some(Challenge {
                        realm: t.realm,
                        nonce: t.nonce,
                        qop: t.qop,
                        algorithm: t.algorithm.unwrap_or(Algorithm::Md5),
                        opaque: t.opaque,
                        proxy: true,
                    });
                }
                if let Some(ch) = parse_challenge_value(p.value(), true) {
                    return Some(ch);
                }
            }
            _ => {}
        }
    }
    None
}

/// Parse a digest algorithm token, accepting both the RFC 7616
/// hyphenated spellings (`SHA-256`, `SHA-256-sess`) and the bare
/// ones rsip emits (`SHA256`). `None` for algorithms we can't
/// compute (e.g. RFC 8760 `SHA-512-256`) — the caller skips the
/// header so a sibling challenge can win.
fn parse_algorithm_token(s: &str) -> Option<Algorithm> {
    match s.trim().trim_matches('"').to_ascii_lowercase().as_str() {
        "md5" => Some(Algorithm::Md5),
        "md5-sess" => Some(Algorithm::Md5Sess),
        "sha-256" | "sha256" => Some(Algorithm::Sha256),
        "sha-256-sess" | "sha256-sess" => Some(Algorithm::Sha256Sess),
        "sha-512" | "sha512" => Some(Algorithm::Sha512),
        "sha-512-sess" | "sha512-sess" => Some(Algorithm::Sha512Sess),
        _ => None,
    }
}

/// Manual fallback parse of a `Digest k=v, k=v, …` challenge value.
fn parse_challenge_value(raw: &str, proxy: bool) -> Option<Challenge> {
    let rest = raw.trim();
    if rest.len() < 6 || !rest[..6].eq_ignore_ascii_case("digest") {
        return None;
    }
    let rest = &rest[6..];

    // Split on commas outside double quotes.
    let mut params: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for c in rest.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                cur.push(c);
            }
            ',' if !in_quotes => {
                params.push(std::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        params.push(cur);
    }

    let mut realm = None;
    let mut nonce = None;
    let mut qop = None;
    let mut algorithm = Algorithm::Md5; // absent param = MD5 (RFC 7616)
    let mut opaque = None;
    for p in &params {
        let Some((k, v)) = p.split_once('=') else {
            continue;
        };
        let v = v.trim().trim_matches('"');
        match k.trim().to_ascii_lowercase().as_str() {
            "realm" => realm = Some(v.to_string()),
            "nonce" => nonce = Some(v.to_string()),
            "opaque" => opaque = Some(v.to_string()),
            "algorithm" => algorithm = parse_algorithm_token(v)?,
            "qop" => {
                let opts: Vec<&str> = v.split(',').map(str::trim).collect();
                qop = if opts.iter().any(|o| o.eq_ignore_ascii_case("auth")) {
                    Some(Qop::Auth)
                } else if opts.iter().any(|o| o.eq_ignore_ascii_case("auth-int")) {
                    Some(Qop::AuthInt)
                } else {
                    None
                };
            }
            _ => {}
        }
    }
    Some(Challenge {
        realm: realm?,
        nonce: nonce?,
        qop,
        algorithm,
        opaque,
        proxy,
    })
}

/// A monotonic, collision-free token for Call-ID / tags / branches / cnonce.
fn gen_token(prefix: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let c = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}{n:x}{c:x}")
}

fn recv_response(sock: &UdpSocket) -> Result<rsip::Response, String> {
    let mut buf = [0u8; 4096];
    let n = sock
        .recv(&mut buf)
        .map_err(|e| format!("no reply from registrar ({e})"))?;
    rsip::Response::try_from(&buf[..n]).map_err(|e| format!("malformed SIP reply ({e})"))
}

// ── VOIP-28 slice 2: outbound call signaling (INVITE / SDP / ACK / BYE) ──────
//
// The dialog establishment is real (INVITE → digest auth → 180 Ringing → 200 OK
// → ACK, BYE to hang up) and the SDP answer is parsed into the remote RTP
// endpoint. Media (RTP/G.711 over that endpoint) is slice 3 — until then a
// connected call carries no audio, which the HUD states honestly.

/// Live call state shown in the HUD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallState {
    /// No call in progress.
    Idle,
    /// An inbound call is ringing — awaiting answer/decline.
    Incoming { from: String },
    /// INVITE sent, awaiting a final response.
    Calling { peer: String },
    /// 180 Ringing received.
    Ringing { peer: String },
    /// 200 OK + ACK — the dialog is up (audio lands in slice 3).
    InCall { peer: String },
    /// The call ended (local or remote BYE).
    Ended,
    /// Setup failed (busy, declined, timeout, unreachable…).
    Failed(String),
}

impl CallState {
    /// One-line label for the dialer status row.
    pub fn label(&self) -> String {
        match self {
            CallState::Idle => String::new(),
            CallState::Incoming { from } => format!("Incoming call · {from}"),
            CallState::Calling { peer } => format!("Calling {peer}…"),
            CallState::Ringing { peer } => format!("Ringing {peer}…"),
            CallState::InCall { peer } => format!("In call · {peer}"),
            CallState::Ended => "Call ended".to_string(),
            CallState::Failed(why) => format!("Call failed: {why}"),
        }
    }

    /// Whether a call is active (dialog up or being set up).
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            CallState::Calling { .. } | CallState::Ringing { .. } | CallState::InCall { .. }
        )
    }
}

/// Remote media endpoint parsed from the SDP answer (slice 3 sends RTP here).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteMedia {
    pub addr: String,
    pub port: u16,
    /// RTP payload type: 0 = PCMU (G.711 µ-law), 8 = PCMA (G.711 A-law).
    pub payload_type: u8,
    /// The dynamic payload type the peer negotiated for `telephone-event`
    /// (RFC 4733 DTMF), parsed from the SDP `a=rtpmap:<pt> telephone-event/8000`
    /// line. `None` when the peer did not offer out-of-band DTMF — in-call
    /// keypad presses then fall back to nothing rather than sending malformed
    /// events to a payload type the peer never agreed to.
    pub telephone_event_pt: Option<u8>,
}

/// An established dialog — enough to hang up (BYE) and (slice 3) attach media.
#[derive(Debug, Clone)]
pub struct CallSession {
    account: SipAccount,
    target: String,
    /// VOIP-P2P — the From URI used for this dialog. For a registrar call this
    /// is `account.aor()` (sip:user@server); for a registrar-less P2P call it is
    /// the local overlay identity (sip:user@<local-overlay-host>). In-dialog
    /// ACK/BYE must echo the SAME From, so it is stored rather than re-derived.
    from_uri: String,
    call_id: String,
    from_tag: String,
    to_tag: String,
    local_host: String,
    local_port: u16,
    /// The local RTP port advertised in the SDP offer (slice 3 binds it).
    pub rtp_port: u16,
    /// Where the peer wants RTP (slice 3 target).
    pub remote: RemoteMedia,
    cseq: u32,
}

/// Normalize a dialed string into a request-URI. A bare number/extension
/// becomes `sip:<number>@<registrar>`; an already-qualified `sip:` URI or
/// `user@host` is used as given.
fn target_uri(account: &SipAccount, dialed: &str) -> String {
    let d = dialed.trim();
    if d.starts_with("sip:") {
        d.to_string()
    } else if d.contains('@') {
        format!("sip:{d}")
    } else {
        // Strip dial-formatting (spaces, parens, dashes); keep digits + + * #.
        let digits: String = d
            .chars()
            .filter(|c| c.is_ascii_digit() || matches!(c, '+' | '*' | '#'))
            .collect();
        format!("sip:{digits}@{}", account.server_host)
    }
}

/// The dynamic RTP payload type we advertise for RFC 4733 `telephone-event`
/// (DTMF). 101 is the de-facto convention (Asterisk/most softphones).
const TELEPHONE_EVENT_PT: u8 = 101;

/// Minimal audio SDP offer — PCMU(0) + PCMA(8) + `telephone-event`(101, RFC
/// 4733 out-of-band DTMF, events 0-15) at 8 kHz on `rtp_port`.
fn build_sdp_offer(local_host: &str, rtp_port: u16) -> String {
    format!(
        "v=0\r\n\
         o=mwv 0 0 IN IP4 {local_host}\r\n\
         s=MCNF Voice\r\n\
         c=IN IP4 {local_host}\r\n\
         t=0 0\r\n\
         m=audio {rtp_port} RTP/AVP 0 8 {TELEPHONE_EVENT_PT}\r\n\
         a=rtpmap:0 PCMU/8000\r\n\
         a=rtpmap:8 PCMA/8000\r\n\
         a=rtpmap:{TELEPHONE_EVENT_PT} telephone-event/8000\r\n\
         a=fmtp:{TELEPHONE_EVENT_PT} 0-15\r\n\
         a=sendrecv\r\n"
    )
}

/// Parse the connection address + first audio media line from an SDP body,
/// plus the dynamic payload type the peer chose for `telephone-event` (DTMF).
fn parse_sdp(body: &str) -> Option<RemoteMedia> {
    let mut addr: Option<String> = None;
    let mut port: Option<u16> = None;
    let mut pt: Option<u8> = None;
    let mut tel_pt: Option<u8> = None;
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("c=IN IP4 ") {
            addr = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("m=audio ") {
            let mut it = rest.split_whitespace();
            port = it.next().and_then(|p| p.parse::<u16>().ok());
            let _proto = it.next(); // RTP/AVP
            pt = it.next().and_then(|p| p.parse::<u8>().ok());
        } else if let Some(rest) = line.strip_prefix("a=rtpmap:") {
            // `a=rtpmap:<pt> <encoding>/<clock>[/<channels>]` — match the peer's
            // chosen telephone-event payload type (it need not be our 101). Match
            // on the encoding NAME, tolerating any clock rate / channel suffix
            // (e.g. `telephone-event/8000` or `telephone-event/8000/1`).
            if let Some((pt_str, enc)) = rest.split_once(char::is_whitespace) {
                let name = enc.trim().split('/').next().unwrap_or("");
                if name.eq_ignore_ascii_case("telephone-event") {
                    tel_pt = pt_str.trim().parse::<u8>().ok();
                }
            }
        }
    }
    Some(RemoteMedia {
        addr: addr?,
        port: port?,
        payload_type: pt.unwrap_or(0),
        telephone_event_pt: tel_pt,
    })
}

/// Build an INVITE (with SDP offer) or its authorized retry.
fn build_invite(
    account: &SipAccount,
    target: &str,
    from_uri: &str,
    local_host: &str,
    local_port: u16,
    ids: &TxnIds,
    sdp: &str,
    auth: Option<(&str, &str)>,
) -> String {
    let from = from_uri.to_string();
    let contact = format!("sip:{}@{local_host}:{local_port}", account.username);
    let mut m = String::new();
    let _ = write!(m, "INVITE {target} SIP/2.0\r\n");
    let _ = write!(
        m,
        "Via: SIP/2.0/UDP {local_host}:{local_port};branch={};rport\r\n",
        ids.branch
    );
    m.push_str("Max-Forwards: 70\r\n");
    let _ = write!(m, "From: <{from}>;tag={}\r\n", ids.from_tag);
    let _ = write!(m, "To: <{target}>\r\n");
    let _ = write!(m, "Call-ID: {}\r\n", ids.call_id);
    let _ = write!(m, "CSeq: {} INVITE\r\n", ids.cseq);
    let _ = write!(m, "Contact: <{contact}>\r\n");
    if let Some((name, value)) = auth {
        let _ = write!(m, "{name}: {value}\r\n");
    }
    m.push_str("Content-Type: application/sdp\r\n");
    let _ = write!(m, "Content-Length: {}\r\n\r\n", sdp.len());
    m.push_str(sdp);
    m
}

/// Build the in-dialog ACK for a 2xx (its own transaction, same branch rules).
fn build_ack(session: &CallSession, branch: &str) -> String {
    let from = session.from_uri.clone();
    let mut m = String::new();
    let _ = write!(m, "ACK {} SIP/2.0\r\n", session.target);
    let _ = write!(
        m,
        "Via: SIP/2.0/UDP {}:{};branch={branch};rport\r\n",
        session.local_host, session.local_port
    );
    m.push_str("Max-Forwards: 70\r\n");
    let _ = write!(m, "From: <{from}>;tag={}\r\n", session.from_tag);
    let _ = write!(m, "To: <{}>;tag={}\r\n", session.target, session.to_tag);
    let _ = write!(m, "Call-ID: {}\r\n", session.call_id);
    let _ = write!(m, "CSeq: {} ACK\r\n", session.cseq);
    m.push_str("Content-Length: 0\r\n\r\n");
    m
}

/// Build a BYE to tear down an established dialog.
fn build_bye(session: &CallSession, branch: &str, cseq: u32) -> String {
    let from = session.from_uri.clone();
    let mut m = String::new();
    let _ = write!(m, "BYE {} SIP/2.0\r\n", session.target);
    let _ = write!(
        m,
        "Via: SIP/2.0/UDP {}:{};branch={branch};rport\r\n",
        session.local_host, session.local_port
    );
    m.push_str("Max-Forwards: 70\r\n");
    let _ = write!(m, "From: <{from}>;tag={}\r\n", session.from_tag);
    let _ = write!(m, "To: <{}>;tag={}\r\n", session.target, session.to_tag);
    let _ = write!(m, "Call-ID: {}\r\n", session.call_id);
    let _ = write!(m, "CSeq: {cseq} BYE\r\n");
    m.push_str("Content-Length: 0\r\n\r\n");
    m
}

/// Read the To-tag from a response's To header (needed to address ACK/BYE).
fn parse_to_tag(resp: &rsip::Response) -> Option<String> {
    for h in resp.headers.iter() {
        if let Header::To(t) = h {
            if let Ok(typed) = t.typed() {
                if let Some(tag) = typed.tag() {
                    return Some(tag.to_string());
                }
            }
        }
    }
    None
}

/// Place an outbound call via the configured registrar: the dialed string is
/// normalized to `sip:<n>@<server>` and the INVITE is sent to the registrar
/// (the original behavior). Identity is `account.aor()`.
pub fn place_call(
    account: &SipAccount,
    dialed: &str,
    ring_timeout: Duration,
) -> Result<CallSession, String> {
    let target = target_uri(account, dialed);
    let dest = (account.server_host.as_str(), account.server_port)
        .to_socket_addrs()
        .map_err(|e| format!("cannot resolve {}: {e}", account.server_host))?
        .next()
        .ok_or_else(|| format!("no address for {}", account.server_host))?;
    place_call_inner(account, &target, dest, false, ring_timeout)
}

/// VOIP-P2P — place a registrar-less call DIRECTLY to a mesh peer over the
/// overlay. `peer_user` is the dialed user/extension; `peer_host`/`peer_port`
/// are the peer's overlay SIP address (resolved from the mesh directory or
/// `<peer>.mesh.mde` DNS). No registrar is involved — the INVITE goes straight
/// to the peer and the From identity is this node's local overlay address.
pub fn place_call_direct(
    account: &SipAccount,
    peer_user: &str,
    peer_host: &str,
    peer_port: u16,
    ring_timeout: Duration,
) -> Result<CallSession, String> {
    let target = direct_target_uri(peer_user, peer_host);
    let dest = (peer_host, peer_port)
        .to_socket_addrs()
        .map_err(|e| format!("cannot resolve {peer_host}: {e}"))?
        .next()
        .ok_or_else(|| format!("no address for {peer_host}"))?;
    place_call_inner(account, &target, dest, true, ring_timeout)
}

/// VOIP-P2P — build the request-URI for a direct peer call: `sip:<user>@<host>`
/// (or `sip:<host>` when no user/extension is supplied). Pure + testable.
#[must_use]
pub fn direct_target_uri(peer_user: &str, peer_host: &str) -> String {
    let user = peer_user.trim();
    if user.is_empty() {
        format!("sip:{peer_host}")
    } else {
        format!("sip:{user}@{peer_host}")
    }
}

/// VOIP-P2P — a dialed string names a mesh PEER (vs a number/extension) when it
/// contains an alphabetic character (a hostname). Peer names route to a direct
/// registrar-less overlay call ([`place_call_direct`]); pure numbers route to the
/// registrar ([`place_call`]). Pure + testable. (Moved here from the HUD binary
/// so the egui surface reuses the same routing rule — §6.)
#[must_use]
pub fn looks_like_peer(dialed: &str) -> bool {
    dialed.chars().any(|c| c.is_ascii_alphabetic())
}

/// VOIP-GW-4 — how a dialed target routes (lock 12: intra-mesh stays P2P; only
/// external PSTN uses Vitelity — the mesh never hairpins internal calls through
/// the trunk).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallRoute {
    /// A mesh peer — dialed DIRECTLY over the Nebula overlay, registrar-less.
    /// `peer_host` is the resolvable overlay host. NEVER touches Vitelity.
    MeshP2p { peer_host: String },
    /// An external number — placed via the shared Vitelity outbound trunk,
    /// presenting the shared caller-ID.
    ExternalTrunk,
}

impl CallRoute {
    /// Whether this route uses Vitelity (the external trunk). A mesh peer
    /// never does — the guard the "no hairpin" acceptance turns on.
    #[must_use]
    pub const fn uses_vitelity(&self) -> bool {
        matches!(self, Self::ExternalTrunk)
    }
}

/// VOIP-GW-4 — decide how a dialed string routes.
///
/// A peer name (any letter) resolves to a direct P2P overlay call; a pure
/// number/extension routes to the shared Vitelity trunk. Pure — the single
/// decision point for the "intra-mesh stays P2P, no Vitelity hairpin"
/// invariant. Reuses [`looks_like_peer`] + [`peer_host_for`].
#[must_use]
pub fn route_call(dialed: &str) -> CallRoute {
    if looks_like_peer(dialed) {
        CallRoute::MeshP2p {
            peer_host: peer_host_for(dialed),
        }
    } else {
        CallRoute::ExternalTrunk
    }
}

/// VOIP-P2P — normalize a dialed peer name to a resolvable overlay host. A bare
/// name gets the `.mesh.mde` mesh-DNS suffix (which resolves to the peer's
/// overlay IP); an already-qualified host (contains `.`), or a `sip:`/`user@`
/// form, yields just its host part. Pure + testable.
#[must_use]
pub fn peer_host_for(dialed: &str) -> String {
    let d = dialed.trim();
    let host = d.strip_prefix("sip:").unwrap_or(d);
    // Take the host part after any `user@`.
    let host = host.rsplit('@').next().unwrap_or(host).trim();
    if host.contains('.') {
        host.to_string()
    } else {
        format!("{host}.mesh.mde")
    }
}

/// Place an outbound call: INVITE (+ digest retry) → await a final response,
/// ACK a 2xx, and return the established `CallSession`. Blocking + socket —
/// run off the UI thread. The live audio path is slice 3 (RTP/ALSA).
fn place_call_inner(
    account: &SipAccount,
    target: &str,
    dest_addr: std::net::SocketAddr,
    direct: bool,
    ring_timeout: Duration,
) -> Result<CallSession, String> {
    let sock = UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("socket bind failed ({e})"))?;
    sock.set_read_timeout(Some(Duration::from_secs(2))).ok();
    sock.connect(dest_addr)
        .map_err(|e| format!("connect failed ({e})"))?;
    let local = sock
        .local_addr()
        .map_err(|e| format!("no local addr ({e})"))?;
    let local_host = local.ip().to_string();
    let local_port = local.port();
    // VOIP-P2P — From identity: registrar AOR for a registrar call; the local
    // overlay address for a registrar-less direct (P2P) call.
    let from_uri = if direct {
        format!("sip:{}@{local_host}", account.username)
    } else {
        account.aor()
    };
    // Advertise an RTP port (slice 3 binds it); derive it from the signaling
    // port range so it is deterministic per call without a second bind here.
    let rtp_port = 40000 + (local_port % 1000) * 2;
    let sdp = build_sdp_offer(&local_host, rtp_port);

    let call_id = gen_token("mwv-");
    let from_tag = gen_token("t");
    let mut ids = TxnIds {
        call_id: call_id.clone(),
        from_tag: from_tag.clone(),
        branch: format!("z9hG4bK{}", gen_token("")),
        cseq: 1,
    };

    let req = build_invite(
        account,
        &target,
        &from_uri,
        &local_host,
        local_port,
        &ids,
        &sdp,
        None,
    );
    sock.send(req.as_bytes())
        .map_err(|e| format!("send failed ({e})"))?;

    // Await a final (>=200) response, honouring provisional 1xx and one auth
    // challenge, bounded by ring_timeout.
    let deadline_passes = (ring_timeout.as_secs().max(1) / 2 + 1) as u32 * 8;
    let mut authed = false;
    for _ in 0..deadline_passes {
        let resp = match recv_response(&sock) {
            Ok(r) => r,
            Err(_) => continue, // 2s read timeout tick; keep waiting for ring_timeout
        };
        let code = u16::from(resp.status_code.clone());
        match code {
            100..=199 => continue, // Trying / Ringing — keep waiting
            200..=299 => {
                let to_tag = parse_to_tag(&resp).unwrap_or_default();
                let remote = parse_sdp(&String::from_utf8_lossy(resp.body()))
                    .ok_or("200 OK without a usable SDP answer")?;
                let session = CallSession {
                    account: account.clone(),
                    target: target.to_string(),
                    from_uri: from_uri.clone(),
                    call_id,
                    from_tag,
                    to_tag,
                    local_host,
                    local_port,
                    rtp_port,
                    remote,
                    cseq: ids.cseq,
                };
                let ack = build_ack(&session, &format!("z9hG4bK{}", gen_token("")));
                sock.send(ack.as_bytes())
                    .map_err(|e| format!("ACK send failed ({e})"))?;
                return Ok(session);
            }
            401 | 407 if !authed => {
                let ch = parse_challenge(&resp).ok_or("auth challenge unparseable")?;
                // ACK the failure response (INVITE 4xx requires an ACK).
                authed = true;
                let auth_value = authorization_value(account, &ch, &gen_token("c"), 1)?;
                let name = if ch.proxy {
                    "Proxy-Authorization"
                } else {
                    "Authorization"
                };
                ids = TxnIds {
                    call_id: ids.call_id.clone(),
                    from_tag: ids.from_tag.clone(),
                    branch: format!("z9hG4bK{}", gen_token("")),
                    cseq: 2,
                };
                let req2 = build_invite(
                    account,
                    target,
                    &from_uri,
                    &local_host,
                    local_port,
                    &ids,
                    &sdp,
                    Some((name, &auth_value)),
                );
                sock.send(req2.as_bytes())
                    .map_err(|e| format!("send failed ({e})"))?;
            }
            486 => return Err("busy".to_string()),
            603 => return Err("declined".to_string()),
            other => return Err(format!("call rejected ({other})")),
        }
    }
    Err("no answer (timeout)".to_string())
}

/// Tear down an established call with a BYE (best-effort; never panics).
pub fn hang_up(session: &CallSession) -> Result<(), String> {
    let server_addr = (
        session.account.server_host.as_str(),
        session.account.server_port,
    )
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .next()
        .ok_or("no address")?;
    let sock = UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
    sock.set_read_timeout(Some(Duration::from_secs(1))).ok();
    sock.connect(server_addr).map_err(|e| e.to_string())?;
    let bye = build_bye(
        session,
        &format!("z9hG4bK{}", gen_token("")),
        session.cseq + 1,
    );
    sock.send(bye.as_bytes()).map_err(|e| e.to_string())?;
    let _ = recv_response(&sock); // best-effort 200 OK
    Ok(())
}

// ── VOIP-28 slice 4: inbound INVITE parsing + SIP response building ──────────

/// A parsed inbound INVITE — carries the caller identity, the offered media,
/// the source to reply to, and the raw dialog headers a response must echo.
#[derive(Debug, Clone)]
pub struct InboundInvite {
    /// Caller display name (or the user part of the From URI).
    pub from_display: String,
    /// The caller's address-of-record (`sip:…`).
    pub from_uri: String,
    /// `Call-ID`, identifying the dialog.
    pub call_id: String,
    /// The offered remote media endpoint (our 200 OK answers it).
    pub offer: Option<RemoteMedia>,
    /// Where to send responses (the UDP source of the INVITE).
    pub source: std::net::SocketAddr,
    /// Our generated To-tag for this dialog.
    pub to_tag: String,
    // Raw header values echoed verbatim in every response (Via carries the
    // branch the transaction is keyed on).
    via: String,
    from: String,
    to: String,
    cseq: String,
}

/// First value of header `name` (case-insensitive), trimmed.
fn header_value<'a>(raw: &'a str, name: &str) -> Option<&'a str> {
    raw.lines().take_while(|l| !l.is_empty()).find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim()
            .eq_ignore_ascii_case(name)
            .then(|| v.trim_start().trim_end_matches('\r'))
    })
}

/// Extract a display name / user part from a `From:` header value.
fn from_display_name(from: &str) -> String {
    // "Alice" <sip:alice@h>  →  Alice ; <sip:1001@h>;tag=x → 1001
    if let Some(start) = from.find('"') {
        if let Some(end) = from[start + 1..].find('"') {
            let name = from[start + 1..start + 1 + end].trim();
            if !name.is_empty() {
                return name.to_string();
            }
        }
    }
    let uri = from_uri_of(from);
    uri.strip_prefix("sip:")
        .unwrap_or(&uri)
        .split('@')
        .next()
        .unwrap_or("unknown")
        .to_string()
}

/// Extract the bare `sip:…` URI from a From/To header value.
fn from_uri_of(hdr: &str) -> String {
    if let (Some(a), Some(b)) = (hdr.find('<'), hdr.find('>')) {
        if a < b {
            return hdr[a + 1..b].to_string();
        }
    }
    hdr.split(';').next().unwrap_or(hdr).trim().to_string()
}

/// Parse an inbound INVITE message. `source` is the UDP sender to reply to.
pub fn parse_invite(raw: &str, source: std::net::SocketAddr) -> Option<InboundInvite> {
    let first = raw.lines().next()?;
    if !first.starts_with("INVITE ") {
        return None;
    }
    let via = header_value(raw, "Via")?.to_string();
    let from = header_value(raw, "From")?.to_string();
    let to = header_value(raw, "To")?.to_string();
    let call_id = header_value(raw, "Call-ID")?.to_string();
    let cseq = header_value(raw, "CSeq")?.to_string();
    let offer = raw
        .split_once("\r\n\r\n")
        .and_then(|(_, body)| parse_sdp(body));
    Some(InboundInvite {
        from_display: from_display_name(&from),
        from_uri: from_uri_of(&from),
        call_id,
        offer,
        source,
        to_tag: gen_token("t"),
        via,
        from,
        to,
        cseq,
    })
}

/// Build a SIP response to an inbound INVITE, echoing its dialog headers (and
/// adding our To-tag). `sdp` attaches an answer body (for the 200 OK).
pub fn build_invite_response(
    inv: &InboundInvite,
    account: &SipAccount,
    local_host: &str,
    local_port: u16,
    code: u16,
    reason: &str,
    sdp: Option<&str>,
) -> String {
    let mut m = String::new();
    let _ = write!(m, "SIP/2.0 {code} {reason}\r\n");
    let _ = write!(m, "Via: {}\r\n", inv.via);
    let _ = write!(m, "From: {}\r\n", inv.from);
    // Echo the To header, adding our tag if it lacks one.
    if inv.to.contains(";tag=") {
        let _ = write!(m, "To: {}\r\n", inv.to);
    } else {
        let _ = write!(m, "To: {};tag={}\r\n", inv.to, inv.to_tag);
    }
    let _ = write!(m, "Call-ID: {}\r\n", inv.call_id);
    let _ = write!(m, "CSeq: {}\r\n", inv.cseq);
    if (200..300).contains(&code) {
        let contact = format!("sip:{}@{local_host}:{local_port}", account.username);
        let _ = write!(m, "Contact: <{contact}>\r\n");
    }
    match sdp {
        Some(body) => {
            m.push_str("Content-Type: application/sdp\r\n");
            let _ = write!(m, "Content-Length: {}\r\n\r\n", body.len());
            m.push_str(body);
        }
        None => m.push_str("Content-Length: 0\r\n\r\n"),
    }
    m
}

/// Build the SDP answer to an inbound offer, advertising the same G.711 codec
/// the caller will use, on our `rtp_port`.
pub fn build_sdp_answer(local_host: &str, rtp_port: u16) -> String {
    build_sdp_offer(local_host, rtp_port)
}

/// Build a simple final response (echoing a request's dialog headers) — used
/// to 200-OK an inbound BYE.
fn build_simple_response(raw: &str, code: u16, reason: &str) -> Option<String> {
    let via = header_value(raw, "Via")?;
    let from = header_value(raw, "From")?;
    let to = header_value(raw, "To")?;
    let call_id = header_value(raw, "Call-ID")?;
    let cseq = header_value(raw, "CSeq")?;
    let mut m = String::new();
    let _ = write!(m, "SIP/2.0 {code} {reason}\r\n");
    let _ = write!(m, "Via: {via}\r\n");
    let _ = write!(m, "From: {from}\r\n");
    let _ = write!(m, "To: {to}\r\n");
    let _ = write!(m, "Call-ID: {call_id}\r\n");
    let _ = write!(m, "CSeq: {cseq}\r\n");
    m.push_str("Content-Length: 0\r\n\r\n");
    Some(m)
}

// ── VOIP-28 slice 4: the persistent SIP agent (register + listen) ────────────

/// Event from the agent thread to the UI (over an mpsc channel).
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// Registration state changed.
    Registration(RegistrationState),
    /// An inbound call is ringing.
    Incoming { from: String, call_id: String },
    /// The local user answered and the dialog/media are up.
    Established,
    /// The remote party hung up (inbound BYE).
    RemoteHangup,
}

/// Command from the UI to the agent thread.
#[derive(Debug, Clone)]
pub enum AgentCommand {
    /// Answer the ringing call (200 OK + SDP answer + media).
    Answer,
    /// Decline the ringing call (486 Busy).
    Decline,
    /// Hang up the active inbound call.
    HangUp,
    /// Send a DTMF keypress (RFC 4733 telephone-event) on the active inbound
    /// call — the answered call's media session lives in the agent thread, so
    /// in-call keypad digits route here. A no-op if no call/media is up or the
    /// key is not a DTMF digit.
    Dtmf(char),
    /// POLISH-voicehud-loadstate — re-attempt registration now (the topbar Retry
    /// affordance). Reuses the agent's existing periodic re-REGISTER path
    /// (`agent_register`) by firing it on the next loop tick; a no-op for a
    /// registrar-less P2P agent (nothing to register — it is already reachable).
    Reregister,
}

/// Discover the local IP that routes to `peer` (the overlay IP for a mesh
/// registrar) by connecting a throwaway UDP socket.
fn route_source_ip(peer: std::net::SocketAddr) -> Option<String> {
    let probe = UdpSocket::bind("0.0.0.0:0").ok()?;
    probe.connect(peer).ok()?;
    Some(probe.local_addr().ok()?.ip().to_string())
}

/// REGISTER on the agent's shared UDP socket (send_to/recv_from the registrar),
/// honouring one digest challenge. Returns the resulting state.
fn agent_register(
    sock: &UdpSocket,
    registrar: std::net::SocketAddr,
    account: &SipAccount,
    local_ip: &str,
    local_port: u16,
) -> RegistrationState {
    let ids = TxnIds {
        call_id: gen_token("reg-"),
        from_tag: gen_token("t"),
        branch: format!("z9hG4bK{}", gen_token("")),
        cseq: 1,
    };
    let req = build_register(account, local_ip, local_port, SipTransport::Udp, &ids, None);
    if sock.send_to(req.as_bytes(), registrar).is_err() {
        return RegistrationState::Failed("REGISTER send failed".into());
    }
    let mut buf = [0u8; 4096];
    let mut authed = false;
    for _ in 0..12 {
        let Ok((n, _)) = sock.recv_from(&mut buf) else {
            continue;
        };
        let Ok(resp) = rsip::Response::try_from(&buf[..n]) else {
            continue;
        };
        let code = u16::from(resp.status_code.clone());
        if code == 200 {
            return RegistrationState::Registered {
                server: format!("{}:{} · UDP", account.server_host, registrar.port()),
                expires: account.expires,
            };
        }
        if (code == 401 || code == 407) && !authed {
            authed = true;
            let Some(ch) = parse_challenge(&resp) else {
                continue;
            };
            let Ok(auth) = authorization_value(account, &ch, &gen_token("c"), 1) else {
                continue;
            };
            let name = if ch.proxy {
                "Proxy-Authorization"
            } else {
                "Authorization"
            };
            let ids2 = TxnIds {
                call_id: ids.call_id.clone(),
                from_tag: ids.from_tag.clone(),
                branch: format!("z9hG4bK{}", gen_token("")),
                cseq: 2,
            };
            let req2 = build_register(
                account,
                local_ip,
                local_port,
                SipTransport::Udp,
                &ids2,
                Some((name, &auth)),
            );
            let _ = sock.send_to(req2.as_bytes(), registrar);
        }
    }
    RegistrationState::Failed("no REGISTER reply".into())
}

/// Load the system trust store into a rustls client config (ring provider +
/// TLS 1.2/1.3), matching mackesd's https443 transport. `Err` when no CA roots
/// are available (`ca-certificates` missing) — an honest reason, not a
/// certificate-verification bypass.
fn tls_client_config() -> Result<std::sync::Arc<rustls::ClientConfig>, String> {
    let mut roots = rustls::RootCertStore::empty();
    let loaded = rustls_native_certs::load_native_certs();
    for cert in loaded.certs {
        let _ = roots.add(cert);
    }
    if roots.is_empty() {
        return Err("no system trust store (ca-certificates missing)".to_string());
    }
    let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
    let cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("rustls protocol setup failed ({e})"))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(std::sync::Arc::new(cfg))
}

/// Read one framed SIP message from a byte stream (SIP-over-TCP/TLS): headers
/// to the blank line, then `Content-Length` body bytes. Used by the TLS
/// register leg.
fn read_sip_over_stream<S: std::io::Read>(s: &mut S) -> Result<rsip::Response, String> {
    let mut buf: Vec<u8> = Vec::with_capacity(2048);
    let mut tmp = [0u8; 2048];
    loop {
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            let head = &buf[..pos];
            let content_len = content_length_of(head);
            let total = pos + 4 + content_len;
            if buf.len() >= total {
                return rsip::Response::try_from(&buf[..total])
                    .map_err(|e| format!("malformed TLS SIP reply ({e})"));
            }
        }
        let n = s
            .read(&mut tmp)
            .map_err(|e| format!("TLS read failed ({e})"))?;
        if n == 0 {
            if buf.is_empty() {
                return Err("TLS connection closed before a reply".to_string());
            }
            return rsip::Response::try_from(buf.as_slice())
                .map_err(|e| format!("truncated TLS SIP reply ({e})"));
        }
        buf.extend_from_slice(&tmp[..n]);
    }
}

/// First index of `needle` in `haystack`.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Parse the `Content-Length` from a raw header block (case-insensitive), 0 if
/// absent/unparseable (typical for a REGISTER response).
fn content_length_of(head: &[u8]) -> usize {
    let text = String::from_utf8_lossy(head);
    for line in text.lines() {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                return v.trim().parse::<usize>().unwrap_or(0);
            }
        }
    }
    0
}

/// VOIP-GW-4 — REGISTER over SIP/TLS (a **real** rustls handshake).
///
/// Opens a TCP connection to the registrar's TLS port (5061 unless the operator
/// pinned a non-default port), completes the TLS handshake against the system
/// trust store, then runs the same digest REGISTER exchange over the encrypted
/// stream.
///
/// `Err(reason)` means the **TLS transport is unavailable** (no route /
/// connect / handshake / config failure): the caller downgrades (`PreferTls`)
/// or surfaces it as an honest `Error` (`RequireTls`) — never a silent
/// plaintext register. `Ok(state)` means the TLS channel came up: `Registered`
/// on a 200, or a `Failed` when the credentials were rejected over a good
/// channel. Blocking.
fn tls_register(
    account: &SipAccount,
    local_ip: &str,
    local_port: u16,
) -> Result<RegistrationState, String> {
    use std::io::Write as _;

    let host = account.server_host.clone();
    let port = account.port_for(SipTransport::Tls);
    let addr = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| format!("resolve {host}:{port} failed ({e})"))?
        .next()
        .ok_or_else(|| format!("no address for {host}:{port}"))?;
    let mut tcp = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(5))
        .map_err(|e| format!("TLS connect to {addr} failed ({e})"))?;
    tcp.set_read_timeout(Some(Duration::from_secs(5))).ok();
    tcp.set_write_timeout(Some(Duration::from_secs(5))).ok();

    let config = tls_client_config()?;
    let server_name = rustls::pki_types::ServerName::try_from(host.clone())
        .map_err(|e| format!("bad TLS server name {host} ({e})"))?;
    let mut conn = rustls::ClientConnection::new(config, server_name)
        .map_err(|e| format!("TLS client init failed ({e})"))?;
    let mut tls = rustls::Stream::new(&mut conn, &mut tcp);

    // First REGISTER — the first write drives the TLS handshake; a handshake
    // failure surfaces here as an io error → "TLS transport unavailable".
    let ids = TxnIds {
        call_id: gen_token("reg-"),
        from_tag: gen_token("t"),
        branch: format!("z9hG4bK{}", gen_token("")),
        cseq: 1,
    };
    let req = build_register(account, local_ip, local_port, SipTransport::Tls, &ids, None);
    tls.write_all(req.as_bytes())
        .map_err(|e| format!("TLS handshake/write failed ({e})"))?;
    let _ = tls.flush();

    let mut authed = false;
    for _ in 0..2 {
        let resp = match read_sip_over_stream(&mut tls) {
            Ok(r) => r,
            Err(e) => return Ok(RegistrationState::Failed(e)),
        };
        let code = u16::from(resp.status_code.clone());
        if code == 200 {
            return Ok(RegistrationState::Registered {
                server: format!("{host}:{port} · TLS"),
                expires: account.expires,
            });
        }
        if (code == 401 || code == 407) && !authed {
            authed = true;
            let Some(ch) = parse_challenge(&resp) else {
                return Ok(RegistrationState::Failed(
                    "TLS auth challenge unparseable".into(),
                ));
            };
            let auth = match authorization_value(account, &ch, &gen_token("c"), 1) {
                Ok(a) => a,
                Err(e) => return Ok(RegistrationState::Failed(format!("digest failed ({e})"))),
            };
            let name = if ch.proxy {
                "Proxy-Authorization"
            } else {
                "Authorization"
            };
            let ids2 = TxnIds {
                call_id: ids.call_id.clone(),
                from_tag: ids.from_tag.clone(),
                branch: format!("z9hG4bK{}", gen_token("")),
                cseq: 2,
            };
            let req2 = build_register(
                account,
                local_ip,
                local_port,
                SipTransport::Tls,
                &ids2,
                Some((name, &auth)),
            );
            if let Err(e) = tls.write_all(req2.as_bytes()) {
                return Ok(RegistrationState::Failed(format!("TLS write failed ({e})")));
            }
            let _ = tls.flush();
        } else {
            return Ok(RegistrationState::Failed(format!(
                "registrar rejected REGISTER over TLS ({code})"
            )));
        }
    }
    Ok(RegistrationState::Failed(
        "REGISTER not accepted over TLS".into(),
    ))
}

/// VOIP-GW-4 — register the account under its [`SecurityPolicy`].
///
/// Returns the resulting state + the resolved [`TransportChoice`] (`None` only
/// when `RequireTls` failed with no reachable secure endpoint — the caller
/// publishes the honest `Error`). The TLS attempt itself is the availability
/// probe: a connect/handshake failure drives the policy's fallback via
/// [`crate::secure::select_transport`].
fn register_with_policy(
    sock: &UdpSocket,
    account: &SipAccount,
    local_ip: &str,
    local_port: u16,
) -> (RegistrationState, Option<crate::secure::TransportChoice>) {
    use crate::secure::{select_transport, SecurityPolicy};

    // Resolve the UDP registrar address once (used by UDP-only + the downgrade).
    let udp_register = |sock: &UdpSocket| -> RegistrationState {
        match (
            account.server_host.as_str(),
            account.port_for(SipTransport::Udp),
        )
            .to_socket_addrs()
            .ok()
            .and_then(|mut it| it.next())
        {
            Some(r) => agent_register(sock, r, account, local_ip, local_port),
            None => RegistrationState::Failed("cannot resolve UDP registrar".into()),
        }
    };

    if account.security == SecurityPolicy::UdpOnly {
        let choice = select_transport(SecurityPolicy::UdpOnly, false).ok();
        return (udp_register(sock), choice);
    }

    // Prefer/Require TLS: the real TLS attempt doubles as the availability probe.
    match tls_register(account, local_ip, local_port) {
        Ok(state) => {
            // The TLS channel worked (state may be Registered or a creds Failed).
            let choice = select_transport(account.security, true).ok();
            (state, choice)
        }
        Err(reason) => match select_transport(account.security, false) {
            // PreferTls → honest downgrade to UDP; log why TLS was unavailable.
            Ok(choice) => {
                tracing::warn!(reason = %reason, "voice-hud: TLS unavailable, downgrading to UDP");
                (udp_register(sock), Some(choice))
            }
            // RequireTls → no fallback; surface the real reason as Error.
            Err(err) => (RegistrationState::Failed(format!("{err}: {reason}")), None),
        },
    }
}

/// Map an in-process [`RegistrationState`] to the published (lock 9) phase +
/// reason.
fn reg_phase_of(st: &RegistrationState) -> (crate::secure::RegPhase, String) {
    use crate::secure::RegPhase;
    match st {
        RegistrationState::Registered { .. } => (RegPhase::Registered, String::new()),
        RegistrationState::Registering => (RegPhase::Provisioning, String::new()),
        RegistrationState::NoAccount => (RegPhase::Unregistered, String::new()),
        RegistrationState::Failed(reason) => (RegPhase::Error, reason.clone()),
    }
}

/// VOIP-GW-4 — publish this node's registration state to `state/voice/<node>`.
///
/// The lock-9 shape (`Registered/Unregistered/Provisioning/Error+reason` + the
/// live transport + honest downgrade flag + caller-ID) for the VOIP-GW-3 worker
/// to mirror to the fleet board. Best-effort (a missing Bus is ignored).
pub fn publish_node_reg_state(
    st: &RegistrationState,
    choice: Option<crate::secure::TransportChoice>,
    caller_id: &str,
) {
    let Some(dir) = mde_bus::default_data_dir() else {
        return;
    };
    let Ok(persist) = mde_bus::persist::Persist::open(dir) else {
        return;
    };
    let node = crate::secure::node_name();
    let (phase, reason) = reg_phase_of(st);
    let server = match st {
        RegistrationState::Registered { server, .. } => server.clone(),
        _ => String::new(),
    };
    let body = crate::secure::node_reg_state_json(
        &node,
        phase,
        &reason,
        choice,
        &server,
        caller_id,
        now_unix(),
    );
    let topic = crate::secure::node_reg_topic(&node);
    if let Err(e) = persist.write(
        &topic,
        mde_bus::hooks::config::Priority::Default,
        None,
        Some(&body),
    ) {
        tracing::debug!(error = %e, "voice agent: node reg-state publish failed");
    }
}

/// The persistent SIP agent: binds a stable socket on the route-to-registrar
/// interface, registers (Contact = that socket), and serves inbound INVITE/BYE
/// + UI answer/decline commands until told to shut down. Blocking — run on a
/// dedicated thread. Never panics; transport failures end the loop cleanly.
///
/// Registers only the **inbound sub-account** (`account`) — a single REGISTER,
/// under its [`SecurityPolicy`] (TLS-preferred with an honest UDP fallback).
/// Outbound is bridged onto the shared trunk Vitelity-side, so there is no
/// second registration here.
pub fn run_agent(
    account: &SipAccount,
    events: &std::sync::mpsc::Sender<AgentEvent>,
    commands: &std::sync::mpsc::Receiver<AgentCommand>,
) {
    run_agent_inner(account, "", events, commands);
}

/// VOIP-GW-4 — the split-aware agent entry.
///
/// Registers `accounts.inbound` and publishes the shared-outbound caller-ID
/// (from `accounts.outbound`) in the per-node reg-state (`state/voice/<node>`)
/// so the callee/board see the shared number. `run_agent` is this with no
/// outbound (caller-ID empty).
pub fn run_agent_accounts(
    accounts: &VoiceAccounts,
    events: &std::sync::mpsc::Sender<AgentEvent>,
    commands: &std::sync::mpsc::Receiver<AgentCommand>,
) {
    let caller_id = accounts
        .outbound
        .as_ref()
        .map(|o| o.caller_id.as_str())
        .unwrap_or_default();
    run_agent_inner(&accounts.inbound, caller_id, events, commands);
}

fn run_agent_inner(
    account: &SipAccount,
    caller_id: &str,
    events: &std::sync::mpsc::Sender<AgentEvent>,
    commands: &std::sync::mpsc::Receiver<AgentCommand>,
) {
    use std::sync::mpsc::TryRecvError;
    use std::time::Instant;

    // VOIP-GW-4 — re-REGISTER backoff bounds (lock 19): the normal refresh
    // period applies on success; on a failed attempt the retry backs off
    // exponentially from `base` to `cap`, then resumes the refresh cadence.
    let backoff_base = Duration::from_secs(2);
    let backoff_cap = Duration::from_secs(300);
    let mut consecutive_failures: u32 = 0;
    // The last resolved transport choice, republished with each state change.
    let mut choice: Option<crate::secure::TransportChoice> = None;

    // VOIP-P2P — registrar-less mode (no `server_host`): skip REGISTER and bind
    // the well-known SIP port on the overlay so peers can dial us directly.
    let registrar_less = account.server_host.trim().is_empty();
    let bail = |events: &std::sync::mpsc::Sender<AgentEvent>, why: &str| {
        let st = RegistrationState::Failed(why.to_string());
        publish_voice_status(&st, false);
        let _ = events.send(AgentEvent::Registration(st));
    };
    let (sock, local_ip, registrar): (UdpSocket, String, Option<std::net::SocketAddr>) =
        if registrar_less {
            let Some(local_ip) = overlay_source_ip() else {
                bail(events, "no overlay address for P2P voice");
                return;
            };
            // Prefer the well-known P2P port; fall back to ephemeral if taken.
            let Ok(sock) = UdpSocket::bind((local_ip.as_str(), P2P_SIP_PORT))
                .or_else(|_| UdpSocket::bind((local_ip.as_str(), 0)))
            else {
                bail(events, "agent socket bind failed");
                return;
            };
            (sock, local_ip, None)
        } else {
            let Some(registrar) = (account.server_host.as_str(), account.server_port)
                .to_socket_addrs()
                .ok()
                .and_then(|mut it| it.next())
            else {
                bail(events, "cannot resolve registrar");
                return;
            };
            let Some(local_ip) = route_source_ip(registrar) else {
                bail(events, "no route to registrar");
                return;
            };
            let Ok(sock) = UdpSocket::bind((local_ip.as_str(), 0)) else {
                bail(events, "agent socket bind failed");
                return;
            };
            (sock, local_ip, Some(registrar))
        };
    sock.set_read_timeout(Some(Duration::from_millis(200))).ok();
    let local_port = sock.local_addr().map(|a| a.port()).unwrap_or(0);
    let rtp_port = 40000 + (local_port % 1000) * 2;

    let reg_period = Duration::from_secs(u64::from(account.expires.max(60)) / 2);
    // The socket is bound, so the agent is now listening for inbound INVITEs;
    // `listening` stays true for the rest of the loop.
    let mut reg_state = match registrar {
        Some(_) => {
            // VOIP-GW-4 — register the sub-account under its security policy
            // (TLS-preferred, honest UDP fallback). `registrar` staying `Some`
            // keeps the UDP inbound listener + re-REGISTER cadence below.
            let (st, ch) = register_with_policy(&sock, account, &local_ip, local_port);
            choice = ch;
            consecutive_failures = u32::from(!matches!(st, RegistrationState::Registered { .. }));
            st
        }
        // VOIP-P2P — no registrar: we are reachable on the overlay directly.
        None => RegistrationState::Registered {
            server: format!("{local_ip}:{local_port} · P2P overlay"),
            expires: 0,
        },
    };
    publish_voice_status(&reg_state, true);
    publish_node_reg_state(&reg_state, choice, caller_id);
    let _ = events.send(AgentEvent::Registration(reg_state.clone()));
    let mut next_reg = Instant::now() + reg_period;
    let mut next_status = Instant::now() + Duration::from_secs(STATUS_HEARTBEAT_SECS);
    let mut pending: Option<InboundInvite> = None;
    let mut media: Option<crate::media::MediaSession> = None;
    let mut buf = [0u8; 4096];

    loop {
        match commands.try_recv() {
            Ok(AgentCommand::Answer) => {
                if let Some(inv) = pending.take() {
                    let sdp = build_sdp_answer(&local_ip, rtp_port);
                    let ok = build_invite_response(
                        &inv,
                        account,
                        &local_ip,
                        local_port,
                        200,
                        "OK",
                        Some(&sdp),
                    );
                    let _ = sock.send_to(ok.as_bytes(), inv.source);
                    if let Some(offer) = &inv.offer {
                        media = crate::media::start_media(rtp_port, offer).ok();
                    }
                    let _ = events.send(AgentEvent::Established);
                }
            }
            Ok(AgentCommand::Decline) => {
                if let Some(inv) = pending.take() {
                    let busy = build_invite_response(
                        &inv,
                        account,
                        &local_ip,
                        local_port,
                        486,
                        "Busy Here",
                        None,
                    );
                    let _ = sock.send_to(busy.as_bytes(), inv.source);
                }
            }
            Ok(AgentCommand::Dtmf(key)) => {
                // In-call keypad digit on an answered (agent-owned) call → send
                // it as an RFC 4733 tone on the live media session. No-op if no
                // media is up or the peer never negotiated telephone-event.
                if let Some(m) = &media {
                    let _ = m.send_dtmf(key);
                }
            }
            Ok(AgentCommand::HangUp) => {
                if let Some(m) = media.take() {
                    m.stop();
                }
                pending = None;
            }
            Ok(AgentCommand::Reregister) => {
                // POLISH-voicehud-loadstate — re-register now by collapsing the
                // periodic re-REGISTER timer to "due"; the existing block below
                // runs `agent_register` on this same iteration (registrar
                // accounts only — a registrar-less P2P agent has no registrar).
                next_reg = Instant::now();
            }
            // The UI dropping its sender (app exit) is the shutdown signal.
            Err(TryRecvError::Disconnected) => {
                if let Some(m) = media.take() {
                    m.stop();
                }
                break;
            }
            Err(TryRecvError::Empty) => {}
        }

        if let Ok((n, src)) = sock.recv_from(&mut buf) {
            let raw = String::from_utf8_lossy(&buf[..n]);
            if raw.starts_with("INVITE ") {
                if let Some(inv) = parse_invite(&raw, src) {
                    tracing::info!(from = %inv.from_display, uri = %inv.from_uri, "voice-hud: inbound INVITE");
                    let ringing = build_invite_response(
                        &inv, account, &local_ip, local_port, 180, "Ringing", None,
                    );
                    let _ = sock.send_to(ringing.as_bytes(), src);
                    let _ = events.send(AgentEvent::Incoming {
                        from: inv.from_display.clone(),
                        call_id: inv.call_id.clone(),
                    });
                    pending = Some(inv);
                }
            } else if raw.starts_with("BYE ") {
                if let Some(ok) = build_simple_response(&raw, 200, "OK") {
                    let _ = sock.send_to(ok.as_bytes(), src);
                }
                if let Some(m) = media.take() {
                    m.stop();
                }
                pending = None;
                let _ = events.send(AgentEvent::RemoteHangup);
            }
        }

        // VOIP-P2P — only re-REGISTER when there is a registrar; a registrar-less
        // P2P agent just keeps listening (no registration to refresh).
        if registrar.is_some() && Instant::now() >= next_reg {
            let (st, ch) = register_with_policy(&sock, account, &local_ip, local_port);
            reg_state = st;
            choice = ch;
            publish_voice_status(&reg_state, true);
            publish_node_reg_state(&reg_state, choice, caller_id);
            let _ = events.send(AgentEvent::Registration(reg_state.clone()));
            // VOIP-GW-4 — auto-recover on drop with backoff: a successful
            // register resumes the steady refresh cadence; a failed one retries
            // sooner, backing off exponentially (capped) as failures persist.
            if matches!(reg_state, RegistrationState::Registered { .. }) {
                consecutive_failures = 0;
                next_reg = Instant::now() + reg_period;
            } else {
                let delay = crate::secure::reregister_backoff(
                    consecutive_failures,
                    backoff_base,
                    backoff_cap,
                );
                consecutive_failures = consecutive_failures.saturating_add(1);
                next_reg = Instant::now() + delay;
            }
        }

        // Heartbeat: re-publish the (unchanged) status so a reader can tell a
        // live agent from a crashed one by the freshness of its `ts`.
        if Instant::now() >= next_status {
            publish_voice_status(&reg_state, true);
            next_status = Instant::now() + Duration::from_secs(STATUS_HEARTBEAT_SECS);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_target_uri_builds_peer_request_uri() {
        assert_eq!(direct_target_uri("", "pine.mesh.mde"), "sip:pine.mesh.mde");
        assert_eq!(
            direct_target_uri("1004", "pine.mesh.mde"),
            "sip:1004@pine.mesh.mde"
        );
        // Whitespace-only user is treated as no user.
        assert_eq!(
            direct_target_uri("  ", "birch.mesh.mde"),
            "sip:birch.mesh.mde"
        );
    }

    #[test]
    fn local_identity_is_registrar_less() {
        let id = SipAccount::local_identity();
        // No registrar server, non-empty local username (the hostname).
        assert!(id.server_host.is_empty());
        assert!(!id.username.is_empty());
        // Its From identity is sip:<host>@<host> shape only via place_call_direct
        // (which substitutes the local overlay host); the AOR here is degenerate
        // but never used for a direct call's From.
    }

    fn sample_account() -> SipAccount {
        SipAccount {
            username: "alice".into(),
            password: "secret".into(),
            server_host: "sip.example.com".into(),
            server_port: 5060,
            display_name: "Alice".into(),
            expires: 3600,
            security: SecurityPolicy::UdpOnly,
        }
    }

    #[test]
    fn split_host_port_defaults_and_explicit() {
        assert_eq!(split_host_port("host", 5060), ("host".into(), 5060));
        assert_eq!(split_host_port("host:5080", 5060), ("host".into(), 5080));
        // A bare IPv4 with no port keeps the default.
        assert_eq!(split_host_port("10.0.0.1", 5060), ("10.0.0.1".into(), 5060));
    }

    #[test]
    fn voice_status_json_reflects_registration() {
        let reg = RegistrationState::Registered {
            server: "sip.example.com:5060".into(),
            expires: 3600,
        };
        let v: serde_json::Value =
            serde_json::from_str(&voice_status_json(&reg, true, 1_700_000_000)).unwrap();
        assert_eq!(v["registered"], true);
        assert_eq!(v["listening"], true);
        assert_eq!(v["server"], "sip.example.com:5060");
        assert_eq!(v["ts"], 1_700_000_000_u64);

        let down = voice_status_json(&RegistrationState::NoAccount, false, 1);
        let dv: serde_json::Value = serde_json::from_str(&down).unwrap();
        assert_eq!(dv["registered"], false);
        assert_eq!(dv["listening"], false);
        assert_eq!(dv["detail"], "Not registered");
    }

    #[test]
    fn from_toml_parses_minimal_account() {
        let a = SipAccount::from_toml(
            "username = \"alice\"\npassword = \"secret\"\nserver = \"sip.example.com:5080\"\n",
        )
        .unwrap();
        assert_eq!(a.username, "alice");
        assert_eq!(a.server_host, "sip.example.com");
        assert_eq!(a.server_port, 5080);
        assert_eq!(a.display_name, "alice"); // defaults to username
        assert_eq!(a.expires, 3600);
    }

    #[test]
    fn from_toml_rejects_empty_username() {
        assert!(SipAccount::from_toml("username = \"\"\nserver = \"h\"\n").is_err());
    }

    #[test]
    fn build_register_has_required_lines() {
        let ids = TxnIds {
            call_id: "cid1".into(),
            from_tag: "tag1".into(),
            branch: "z9hG4bKbranch1".into(),
            cseq: 1,
        };
        let msg = build_register(
            &sample_account(),
            "192.168.1.5",
            5062,
            SipTransport::Udp,
            &ids,
            None,
        );
        assert!(msg.starts_with("REGISTER sip:sip.example.com SIP/2.0\r\n"));
        assert!(msg.contains("Via: SIP/2.0/UDP 192.168.1.5:5062;branch=z9hG4bKbranch1;rport\r\n"));
        assert!(msg.contains("From: <sip:alice@sip.example.com>;tag=tag1\r\n"));
        assert!(msg.contains("To: <sip:alice@sip.example.com>\r\n"));
        assert!(msg.contains("Call-ID: cid1\r\n"));
        assert!(msg.contains("CSeq: 1 REGISTER\r\n"));
        assert!(msg.contains("Contact: <sip:alice@192.168.1.5:5062>\r\n"));
        assert!(msg.contains("Expires: 3600\r\n"));
        assert!(msg.contains("Content-Length: 0\r\n"));
        assert!(msg.ends_with("\r\n\r\n"));
        // No auth header on the first pass.
        assert!(!msg.contains("Authorization:"));
    }

    #[test]
    fn build_register_embeds_auth_when_given() {
        let ids = TxnIds {
            call_id: "cid".into(),
            from_tag: "t".into(),
            branch: "z9hG4bKb".into(),
            cseq: 2,
        };
        let msg = build_register(
            &sample_account(),
            "10.0.0.2",
            5060,
            SipTransport::Udp,
            &ids,
            Some(("Authorization", "Digest realm=\"r\"")),
        );
        assert!(msg.contains("Authorization: Digest realm=\"r\"\r\n"));
        assert!(msg.contains("CSeq: 2 REGISTER\r\n"));
    }

    #[test]
    fn authorization_value_no_qop_matches_digest_generator() {
        // RFC 2617-style inputs, qop absent.
        let acct = SipAccount {
            username: "Mufasa".into(),
            password: "Circle Of Life".into(),
            server_host: "host.com".into(),
            server_port: 5060,
            display_name: "Mufasa".into(),
            expires: 60,
            security: SecurityPolicy::UdpOnly,
        };
        let ch = Challenge {
            realm: "testrealm@host.com".into(),
            nonce: "dcd98b7102dd2f0e8b11d0f600bfb0c093".into(),
            qop: None,
            algorithm: Algorithm::Md5,
            opaque: None,
            proxy: false,
        };
        let value = authorization_value(&acct, &ch, "unused", 1).unwrap();
        // Independently compute the expected response via the same generator.
        let uri = Uri::try_from(acct.registrar_uri()).unwrap();
        let method = Method::Register;
        let expected = DigestGenerator {
            username: &acct.username,
            password: &acct.password,
            nonce: &ch.nonce,
            uri: &uri,
            realm: &ch.realm,
            method: &method,
            qop: None,
            algorithm: Algorithm::Md5,
        }
        .compute();
        assert!(value.contains(&format!("response=\"{expected}\"")));
        assert!(value.contains("username=\"Mufasa\""));
        assert!(value.contains("realm=\"testrealm@host.com\""));
        assert!(value.contains("algorithm=MD5"));
        // No qop machinery when the challenge omits it.
        assert!(!value.contains("qop="));
        assert!(!value.contains("cnonce="));
    }

    #[test]
    fn authorization_value_qop_auth_includes_cnonce_nc() {
        let ch = Challenge {
            realm: "r".into(),
            nonce: "n".into(),
            qop: Some(Qop::Auth),
            algorithm: Algorithm::Md5,
            opaque: Some("op".into()),
            proxy: false,
        };
        let value = authorization_value(&sample_account(), &ch, "abc123", 1).unwrap();
        assert!(value.contains("qop=auth"));
        assert!(value.contains("cnonce=\"abc123\""));
        assert!(value.contains("nc=00000001"));
        assert!(value.contains("opaque=\"op\""));
    }

    #[test]
    fn parse_challenge_reads_www_authenticate() {
        let raw = "SIP/2.0 401 Unauthorized\r\n\
             Via: SIP/2.0/UDP 10.0.0.2:5060;branch=z9hG4bKx\r\n\
             From: <sip:alice@sip.example.com>;tag=t\r\n\
             To: <sip:alice@sip.example.com>;tag=s\r\n\
             Call-ID: cid\r\n\
             CSeq: 1 REGISTER\r\n\
             WWW-Authenticate: Digest realm=\"asterisk\", nonce=\"abc\", algorithm=MD5, qop=\"auth\"\r\n\
             Content-Length: 0\r\n\r\n";
        let resp = rsip::Response::try_from(raw.as_bytes()).unwrap();
        let ch = parse_challenge(&resp).expect("challenge");
        assert_eq!(ch.realm, "asterisk");
        assert_eq!(ch.nonce, "abc");
        assert!(!ch.proxy);
        assert!(matches!(ch.qop, Some(Qop::Auth)));
    }

    #[test]
    fn sha256_challenge_answered_with_sha256() {
        // AUD6-5 (§3): the client echoes the registrar's challenged
        // algorithm — a SHA-256 challenge gets a SHA-256 response;
        // the MD5 default applies ONLY when the challenge omits the
        // algorithm param (which RFC 7616 defines as MD5).
        let raw = "SIP/2.0 401 Unauthorized\r\n\
             Via: SIP/2.0/UDP 10.0.0.2:5060;branch=z9hG4bKx\r\n\
             From: <sip:alice@sip.example.com>;tag=t\r\n\
             To: <sip:alice@sip.example.com>;tag=s\r\n\
             Call-ID: cid\r\n\
             CSeq: 1 REGISTER\r\n\
             WWW-Authenticate: Digest realm=\"asterisk\", nonce=\"abc\", algorithm=SHA-256, qop=\"auth\"\r\n\
             Content-Length: 0\r\n\r\n";
        let resp = rsip::Response::try_from(raw.as_bytes()).unwrap();
        let ch = parse_challenge(&resp).expect("challenge");
        assert!(matches!(ch.algorithm, Algorithm::Sha256));
        let value = authorization_value(&sample_account(), &ch, "abc123", 1).unwrap();
        assert!(value.contains("algorithm=SHA-256"));
        assert!(!value.contains("algorithm=MD5"));
    }

    #[test]
    fn registration_state_labels() {
        assert_eq!(RegistrationState::NoAccount.label(), "Not registered");
        assert_eq!(RegistrationState::Registering.label(), "Registering…");
        assert_eq!(
            RegistrationState::Registered {
                server: "sip.example.com:5060".into(),
                expires: 3600
            }
            .label(),
            "Registered · sip.example.com:5060"
        );
    }

    // ── slice 2: call signaling ──────────────────────────────────────────

    #[test]
    fn target_uri_normalizes_dialed_strings() {
        let a = sample_account();
        assert_eq!(target_uri(&a, "1001"), "sip:1001@sip.example.com");
        assert_eq!(
            target_uri(&a, "(415) 555 1234"),
            "sip:4155551234@sip.example.com"
        );
        assert_eq!(target_uri(&a, "bob@other.net"), "sip:bob@other.net");
        assert_eq!(target_uri(&a, "sip:bob@other.net"), "sip:bob@other.net");
    }

    #[test]
    fn sdp_offer_advertises_g711_audio_and_dtmf() {
        let sdp = build_sdp_offer("10.0.0.5", 40002);
        assert!(sdp.contains("m=audio 40002 RTP/AVP 0 8 101\r\n"));
        assert!(sdp.contains("a=rtpmap:0 PCMU/8000\r\n"));
        assert!(sdp.contains("a=rtpmap:8 PCMA/8000\r\n"));
        // RFC 4733 out-of-band DTMF on the dynamic PT, events 0-15.
        assert!(sdp.contains("a=rtpmap:101 telephone-event/8000\r\n"));
        assert!(sdp.contains("a=fmtp:101 0-15\r\n"));
        assert!(sdp.contains("c=IN IP4 10.0.0.5\r\n"));
    }

    #[test]
    fn parse_sdp_extracts_remote_endpoint() {
        let body = "v=0\r\no=x 0 0 IN IP4 1.2.3.4\r\nc=IN IP4 1.2.3.4\r\n\
                    t=0 0\r\nm=audio 5004 RTP/AVP 8 0\r\na=rtpmap:8 PCMA/8000\r\n";
        let r = parse_sdp(body).expect("sdp");
        assert_eq!(r.addr, "1.2.3.4");
        assert_eq!(r.port, 5004);
        assert_eq!(r.payload_type, 8);
        // No telephone-event line → no out-of-band DTMF agreed.
        assert_eq!(r.telephone_event_pt, None);
    }

    #[test]
    fn parse_sdp_picks_up_the_peers_telephone_event_pt() {
        // The peer can pick its OWN dynamic PT for telephone-event (here 96, not
        // our 101). We must DTMF to the PT the peer agreed to, not assume 101.
        let body = "v=0\r\no=x 0 0 IN IP4 1.2.3.4\r\nc=IN IP4 1.2.3.4\r\n\
                    t=0 0\r\nm=audio 5004 RTP/AVP 0 96\r\n\
                    a=rtpmap:0 PCMU/8000\r\na=rtpmap:96 telephone-event/8000\r\n\
                    a=fmtp:96 0-15\r\n";
        let r = parse_sdp(body).expect("sdp");
        assert_eq!(r.payload_type, 0);
        assert_eq!(r.telephone_event_pt, Some(96));
    }

    #[test]
    fn build_invite_carries_sdp_body_and_length() {
        let ids = TxnIds {
            call_id: "cid".into(),
            from_tag: "ft".into(),
            branch: "z9hG4bKb".into(),
            cseq: 1,
        };
        let sdp = build_sdp_offer("10.0.0.5", 40002);
        let msg = build_invite(
            &sample_account(),
            "sip:1001@sip.example.com",
            "sip:alice@sip.example.com",
            "10.0.0.5",
            5070,
            &ids,
            &sdp,
            None,
        );
        assert!(msg.starts_with("INVITE sip:1001@sip.example.com SIP/2.0\r\n"));
        assert!(msg.contains("CSeq: 1 INVITE\r\n"));
        assert!(msg.contains("Content-Type: application/sdp\r\n"));
        assert!(msg.contains(&format!("Content-Length: {}\r\n", sdp.len())));
        assert!(msg.ends_with(&sdp));
    }

    fn sample_session() -> CallSession {
        CallSession {
            account: sample_account(),
            target: "sip:1001@sip.example.com".into(),
            from_uri: "sip:alice@sip.example.com".into(),
            call_id: "cid".into(),
            from_tag: "ft".into(),
            to_tag: "tt".into(),
            local_host: "10.0.0.5".into(),
            local_port: 5070,
            rtp_port: 40002,
            remote: RemoteMedia {
                addr: "1.2.3.4".into(),
                port: 5004,
                payload_type: 0,
                telephone_event_pt: Some(TELEPHONE_EVENT_PT),
            },
            cseq: 1,
        }
    }

    #[test]
    fn ack_and_bye_address_the_established_dialog() {
        let s = sample_session();
        let ack = build_ack(&s, "z9hG4bKack");
        assert!(ack.starts_with("ACK sip:1001@sip.example.com SIP/2.0\r\n"));
        assert!(ack.contains("To: <sip:1001@sip.example.com>;tag=tt\r\n"));
        assert!(ack.contains("From: <sip:alice@sip.example.com>;tag=ft\r\n"));
        assert!(ack.contains("Call-ID: cid\r\n"));
        assert!(ack.contains("CSeq: 1 ACK\r\n"));

        let bye = build_bye(&s, "z9hG4bKbye", 2);
        assert!(bye.starts_with("BYE sip:1001@sip.example.com SIP/2.0\r\n"));
        assert!(bye.contains("To: <sip:1001@sip.example.com>;tag=tt\r\n"));
        assert!(bye.contains("CSeq: 2 BYE\r\n"));
    }

    #[test]
    fn call_state_labels_and_active() {
        assert_eq!(CallState::Idle.label(), "");
        assert_eq!(
            CallState::Ringing {
                peer: "1001".into()
            }
            .label(),
            "Ringing 1001…"
        );
        assert!(CallState::InCall { peer: "x".into() }.is_active());
        assert!(CallState::Calling { peer: "x".into() }.is_active());
        assert!(!CallState::Idle.is_active());
        assert!(!CallState::Ended.is_active());
    }

    // ── slice 4: inbound INVITE parse + response building ─────────────────

    fn sample_inbound() -> (InboundInvite, std::net::SocketAddr) {
        let src: std::net::SocketAddr = "203.0.113.9:5060".parse().unwrap();
        let raw = "INVITE sip:alice@sip.example.com SIP/2.0\r\n\
             Via: SIP/2.0/UDP 203.0.113.9:5060;branch=z9hG4bKinbound;rport\r\n\
             From: \"Bob Smith\" <sip:bob@sip.example.com>;tag=callerTag\r\n\
             To: <sip:alice@sip.example.com>\r\n\
             Call-ID: inbound-call-1\r\n\
             CSeq: 1 INVITE\r\n\
             Content-Type: application/sdp\r\n\
             Content-Length: 0\r\n\r\n\
             v=0\r\no=bob 0 0 IN IP4 203.0.113.9\r\nc=IN IP4 203.0.113.9\r\n\
             t=0 0\r\nm=audio 6000 RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\n";
        (parse_invite(raw, src).expect("invite"), src)
    }

    #[test]
    fn parse_invite_extracts_caller_and_offer() {
        let (inv, src) = sample_inbound();
        assert_eq!(inv.from_display, "Bob Smith");
        assert_eq!(inv.from_uri, "sip:bob@sip.example.com");
        assert_eq!(inv.call_id, "inbound-call-1");
        assert_eq!(inv.source, src);
        let offer = inv.offer.expect("offer");
        assert_eq!(offer.addr, "203.0.113.9");
        assert_eq!(offer.port, 6000);
        assert_eq!(offer.payload_type, 0);
    }

    #[test]
    fn from_display_falls_back_to_user_part() {
        assert_eq!(
            from_display_name("<sip:1001@host>;tag=x"),
            "1001".to_string()
        );
        assert_eq!(from_uri_of("<sip:1001@host>;tag=x"), "sip:1001@host");
    }

    #[test]
    fn build_200_ok_echoes_dialog_and_carries_answer() {
        let (inv, _) = sample_inbound();
        let acct = sample_account();
        let sdp = build_sdp_answer("10.0.0.5", 40002);
        let resp = build_invite_response(&inv, &acct, "10.0.0.5", 5070, 200, "OK", Some(&sdp));
        assert!(resp.starts_with("SIP/2.0 200 OK\r\n"));
        assert!(resp.contains("Via: SIP/2.0/UDP 203.0.113.9:5060;branch=z9hG4bKinbound;rport\r\n"));
        assert!(resp.contains("From: \"Bob Smith\" <sip:bob@sip.example.com>;tag=callerTag\r\n"));
        assert!(resp.contains(&format!(
            "To: <sip:alice@sip.example.com>;tag={}\r\n",
            inv.to_tag
        )));
        assert!(resp.contains("Call-ID: inbound-call-1\r\n"));
        assert!(resp.contains("CSeq: 1 INVITE\r\n"));
        assert!(resp.contains("Contact: <sip:alice@10.0.0.5:5070>\r\n"));
        assert!(resp.contains("Content-Type: application/sdp\r\n"));
        assert!(resp.ends_with(&sdp));
    }

    #[test]
    fn build_486_busy_has_no_body() {
        let (inv, _) = sample_inbound();
        let resp = build_invite_response(
            &inv,
            &sample_account(),
            "10.0.0.5",
            5070,
            486,
            "Busy Here",
            None,
        );
        assert!(resp.starts_with("SIP/2.0 486 Busy Here\r\n"));
        assert!(resp.contains("Content-Length: 0\r\n"));
        assert!(!resp.contains("Content-Type:"));
        assert!(!resp.contains("Contact:")); // non-2xx → no Contact
    }

    // ── VOIP-P2P: dialed-string routing helpers (moved from the HUD binary) ──

    #[test]
    fn looks_like_peer_distinguishes_names_from_numbers() {
        // Mesh peer names (have letters) → direct P2P.
        assert!(looks_like_peer("pine"));
        assert!(looks_like_peer("pine.mesh.mde"));
        assert!(looks_like_peer("UNIT-EAGLE"));
        // Numbers / extensions / SIP digits → registrar.
        assert!(!looks_like_peer("1004"));
        assert!(!looks_like_peer("+15551234567"));
        assert!(!looks_like_peer("*69"));
        assert!(!looks_like_peer(""));
    }

    #[test]
    fn peer_host_for_appends_mesh_suffix_for_bare_names() {
        assert_eq!(peer_host_for("pine"), "pine.mesh.mde");
        // Already-qualified hosts are used as-is.
        assert_eq!(peer_host_for("pine.mesh.mde"), "pine.mesh.mde");
        assert_eq!(peer_host_for("pine.mesh"), "pine.mesh");
        // sip: / user@ forms reduce to the host part.
        assert_eq!(peer_host_for("sip:pine"), "pine.mesh.mde");
        assert_eq!(peer_host_for("sip:matt@birch.mesh.mde"), "birch.mesh.mde");
    }

    // ── VOIP-GW-4: account split, transport ports, mesh-vs-external routing ──

    #[test]
    fn legacy_flat_account_still_parses_to_inbound_only() {
        let a = SipAccount::accounts_from_toml(
            "username = \"alice\"\npassword = \"secret\"\nserver = \"sip.example.com:5080\"\n",
        )
        .unwrap();
        assert_eq!(a.inbound.username, "alice");
        assert_eq!(a.inbound.server_port, 5080);
        // No transport → the secure default.
        assert_eq!(a.inbound.security, SecurityPolicy::PreferTls);
        // A flat config has no shared-outbound trunk.
        assert!(a.outbound.is_none());
    }

    #[test]
    fn split_config_parses_inbound_sub_and_shared_outbound() {
        let toml = "\
            [inbound_sub]\n\
            username = \"eagle\"\n\
            password = \"subpw\"\n\
            server = \"sip.vitelity.net\"\n\
            transport = \"require-tls\"\n\
            expires = 1800\n\
            \n\
            [shared_outbound]\n\
            caller_id = \"+15551230000\"\n\
            trunk = \"outbound.vitelity.net\"\n";
        let a = SipAccount::accounts_from_toml(toml).unwrap();
        // The registered identity is the sub-account.
        assert_eq!(a.inbound.username, "eagle");
        assert_eq!(a.inbound.password, "subpw");
        assert_eq!(a.inbound.expires, 1800);
        assert_eq!(a.inbound.security, SecurityPolicy::RequireTls);
        // The shared-outbound trunk carries the fleet caller-ID, no password.
        let out = a.outbound.expect("shared outbound");
        assert_eq!(out.caller_id, "+15551230000");
        assert_eq!(out.trunk_host, "outbound.vitelity.net");
    }

    #[test]
    fn split_config_defaults_transport_to_prefer_tls() {
        let toml = "\
            [inbound_sub]\n\
            username = \"pine\"\n\
            server = \"sip.vitelity.net\"\n";
        let a = SipAccount::accounts_from_toml(toml).unwrap();
        assert_eq!(a.inbound.security, SecurityPolicy::PreferTls);
    }

    #[test]
    fn tls_port_defaults_to_5061_but_honors_explicit() {
        // Configured on the UDP default → TLS leg uses 5061.
        let a = SipAccount::accounts_from_toml(
            "username = \"a\"\nserver = \"h\"\ntransport = \"prefer-tls\"\n",
        )
        .unwrap()
        .inbound;
        assert_eq!(a.server_port, 5060);
        assert_eq!(a.port_for(SipTransport::Udp), 5060);
        assert_eq!(a.port_for(SipTransport::Tls), 5061);

        // An operator-pinned non-default port is honored for both legs.
        let b = SipAccount::accounts_from_toml("username = \"a\"\nserver = \"h:5070\"\n")
            .unwrap()
            .inbound;
        assert_eq!(b.port_for(SipTransport::Tls), 5070);
        assert_eq!(b.port_for(SipTransport::Udp), 5070);
    }

    #[test]
    fn mesh_peer_routes_p2p_never_through_vitelity() {
        // A peer NAME → direct P2P over the overlay, no Vitelity hairpin.
        let r = route_call("pine");
        assert_eq!(
            r,
            CallRoute::MeshP2p {
                peer_host: "pine.mesh.mde".into()
            }
        );
        assert!(
            !r.uses_vitelity(),
            "intra-mesh must not hairpin to Vitelity"
        );
        assert!(!route_call("UNIT-EAGLE").uses_vitelity());
        assert!(!route_call("sip:matt@birch.mesh.mde").uses_vitelity());
    }

    #[test]
    fn external_number_routes_to_shared_trunk() {
        assert_eq!(route_call("915551234567"), CallRoute::ExternalTrunk);
        assert!(route_call("915551234567").uses_vitelity());
        assert!(route_call("+15551234567").uses_vitelity());
    }

    #[test]
    fn content_length_of_reads_header_case_insensitively() {
        assert_eq!(
            content_length_of(b"SIP/2.0 200 OK\r\nContent-Length: 42"),
            42
        );
        assert_eq!(content_length_of(b"SIP/2.0 200 OK\r\ncontent-length: 7"), 7);
        // Absent → 0 (typical REGISTER response).
        assert_eq!(content_length_of(b"SIP/2.0 200 OK\r\nVia: x"), 0);
    }

    #[test]
    fn read_sip_over_stream_frames_by_content_length() {
        // A 200 with a zero-length body arrives in one chunk.
        let raw = b"SIP/2.0 200 OK\r\nVia: SIP/2.0/TLS h:5061;branch=z9hG4bKx\r\n\
                    From: <sip:a@h>;tag=t\r\nTo: <sip:a@h>;tag=s\r\n\
                    Call-ID: c\r\nCSeq: 1 REGISTER\r\nContent-Length: 0\r\n\r\n";
        let mut cursor = std::io::Cursor::new(raw.to_vec());
        let resp = read_sip_over_stream(&mut cursor).expect("framed reply");
        assert_eq!(u16::from(resp.status_code), 200);
    }
}
