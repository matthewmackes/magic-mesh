//! VPN-GW-5 — first-class provider integrations (design:
//! `docs/design/vpn-gateway.md` §"Providers").
//!
//! This module is the **pure** layer of provider setup: a [`VpnProvider`]
//! adapter trait + five named impls (Mullvad / ProtonVPN / IVPN / NordVPN /
//! Surfshark), plus two generic importers — a `wg-quick` `.conf` parser
//! ([`parse_wg_conf`]) and an OpenVPN `.ovpn` parser ([`parse_ovpn`]). Each
//! turns provider params (or a pasted config) into a GW-1 [`TunnelDef`] + the
//! GW-2 [`TunnelSecret`] that carries the cleartext key material the
//! `vpn_secret` path seals.
//!
//! ## Glue, not reimplementation (§6)
//!
//! We reuse GW-1's [`TunnelDef`]/[`Method`] and GW-2's [`TunnelSecret`]
//! verbatim — this module only *constructs* them. All five named providers
//! support WireGuard, so every named adapter models a WG tunnel: it renders a
//! `wg-quick` `[Interface]`/`[Peer]` body from the chosen server + a
//! caller-supplied x25519 keypair (the keypair is generated in `mackesd`
//! where the workspace crypto lives — §3 — and passed in here so this crate
//! stays dependency-light and the construction stays pure + testable).
//!
//! ## Request-builders, not live HTTP (mirror DDNS-EGRESS-2)
//!
//! Where a provider exposes an **API** to register a public key / fetch a
//! config (Mullvad's key-registration API; the others' config endpoints),
//! the adapter returns a pure [`ProviderRequest`] — `(method, url, headers
//! WITHOUT the secret, body)` — that the `mackesd` daemon executes with the
//! account token attached off-argv (the `curl --config` 0600-file pattern,
//! exactly like the DDNS DigitalOcean writer). **No live HTTP here.** The
//! account token is referenced by name, never embedded in a builder's output
//! (a `secret_header` names the header the daemon must add; its *value* is
//! the sealed token resolved at call time).
//!
//! ## Secrets (§3)
//!
//! The generated/imported WireGuard private key, the `.ovpn`'s inline
//! keys/creds, and the provider account token are SECRETS. This module never
//! puts them on a command line: it returns them inside a [`TunnelSecret`]
//! (sealed by the GW-2 `vpn_secret` path, materialized 0600 by the
//! distributor) or names the auth header for the daemon to fill from the
//! sealed token. The [`ProviderRequest::headers`] a builder emits never carry
//! a secret value.

use serde::{Deserialize, Serialize};

use crate::vpn::{Method, TunnelDef, TunnelSecret};

/// The five first-class providers VPN-GW-5 ships, plus the two generic import
/// paths. Serialized kebab-case so an IPC body / config selects by string.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    /// Mullvad — WireGuard; a key-registration API mints the assigned address.
    Mullvad,
    /// ProtonVPN — WireGuard; a config-download API yields the peer + address.
    Protonvpn,
    /// IVPN — WireGuard; a key-registration / config API.
    Ivpn,
    /// NordVPN (NordLynx) — WireGuard; a credentials API yields the WG key +
    /// the chosen server's pubkey/endpoint.
    Nordvpn,
    /// Surfshark — WireGuard; a config API yields the peer + assigned address.
    Surfshark,
}

impl ProviderKind {
    /// The stable provider label recorded on the [`TunnelDef::provider`] field
    /// (the same string the kebab-case serde uses) — log-safe, never a secret.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            ProviderKind::Mullvad => "mullvad",
            ProviderKind::Protonvpn => "protonvpn",
            ProviderKind::Ivpn => "ivpn",
            ProviderKind::Nordvpn => "nordvpn",
            ProviderKind::Surfshark => "surfshark",
        }
    }

    /// Resolve a provider from its label (the IPC body / config string).
    #[must_use]
    pub fn from_label(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "mullvad" => Some(ProviderKind::Mullvad),
            "protonvpn" | "proton" => Some(ProviderKind::Protonvpn),
            "ivpn" => Some(ProviderKind::Ivpn),
            "nordvpn" | "nord" | "nordlynx" => Some(ProviderKind::Nordvpn),
            "surfshark" => Some(ProviderKind::Surfshark),
            _ => None,
        }
    }

    /// Construct the named adapter for this provider.
    #[must_use]
    pub fn adapter(self) -> Box<dyn VpnProvider> {
        match self {
            ProviderKind::Mullvad => Box::new(Mullvad),
            ProviderKind::Protonvpn => Box::new(ProtonVpn),
            ProviderKind::Ivpn => Box::new(Ivpn),
            ProviderKind::Nordvpn => Box::new(NordVpn),
            ProviderKind::Surfshark => Box::new(Surfshark),
        }
    }

    /// The provider's default anti-leak DNS resolver (used when the operator
    /// didn't pin one). Single-sourced here so [`provision_wg`] needn't box an
    /// adapter just to read a static string; each adapter's
    /// [`VpnProvider::default_dns`] delegates to this.
    #[must_use]
    pub fn default_dns(self) -> &'static str {
        match self {
            ProviderKind::Mullvad => "10.64.0.1",
            ProviderKind::Protonvpn => "10.2.0.1",
            ProviderKind::Ivpn => "172.16.0.1",
            ProviderKind::Nordvpn => "103.86.96.100",
            ProviderKind::Surfshark => "162.252.172.57",
        }
    }
}

/// A locally-generated WireGuard x25519 keypair, base64-encoded WireGuard-style
/// (the `mackesd` `vpn_keypair` module mints this with the workspace crypto —
/// no OpenSSL, §3 — and passes it in so this crate stays dependency-light). The
/// **private key is a secret**: it only ever lands in the rendered
/// [`TunnelSecret::wg_conf`] (sealed by GW-2), never in a log/argv.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WgKeypair {
    /// The 32-byte x25519 private key, standard-base64 (the `[Interface]
    /// PrivateKey`). SECRET.
    pub private_b64: String,
    /// The derived 32-byte x25519 public key, standard-base64 — the key the
    /// provider's API registers / the operator pastes into the provider portal.
    /// Not secret (it is published to the provider), so it may appear in an API
    /// request body.
    pub public_b64: String,
}

/// The operator-supplied parameters for standing up a named-provider tunnel.
/// Account credentials are referenced/carried as secrets — `account_token` is
/// the provider account/auth token (Mullvad account number, Proton/IVPN/Nord
/// session token, …) and is a SECRET (sealed via GW-2; passed to the daemon's
/// API executor off-argv). The chosen `server` + `peer` material describe the
/// exit. For a provider whose peer pubkey/endpoint is fetched from its API at
/// runtime, `peer` may be left default and resolved by the request-builder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProviderParams {
    /// Operator-chosen tunnel id (drives `mvpn-<id>`).
    pub tunnel_id: String,
    /// The chosen server / region selector (provider-specific:
    /// `us-nyc-wg-001`, a Proton `LogicalServer`, a Nord group, …).
    pub server: String,
    /// The provider account / session token (SECRET — sealed via GW-2, attached
    /// off-argv when the daemon calls the provider API). Empty when the chosen
    /// path needs no token (e.g. a Mullvad config built entirely from a pasted
    /// peer).
    pub account_token: String,
    /// The chosen exit peer's parameters when known statically (the provider's
    /// WireGuard server pubkey + endpoint + the address it assigns). When the
    /// provider's API mints these, leave default and use the request-builder.
    pub peer: PeerParams,
    /// DNS the tunnel should use (defaults to the provider's anti-leak resolver
    /// when empty — each adapter fills a sane default).
    pub dns: Option<String>,
}

/// The WireGuard exit peer + assigned interface address — either operator-known
/// (pasted from the provider portal) or filled by the provider's API.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PeerParams {
    /// The provider server's WireGuard public key (base64). Not secret.
    pub server_pubkey: String,
    /// The server endpoint `host:port`.
    pub endpoint: String,
    /// The address the provider assigns this peer (`10.x.y.z/32`, maybe a v6).
    pub assigned_address: String,
    /// Optional preshared key (base64) — SECRET when present (lands only in the
    /// sealed `wg_conf`).
    pub preshared_key: Option<String>,
}

/// One pure HTTP request a provider adapter asks the daemon to perform — the
/// DDNS-EGRESS-2 shape, extended with a `headers` list and a `secret_header`
/// name. **No secret value appears here.** The `secret_header` (when `Some`)
/// names a header (`Authorization`/`X-API-Token`/…) the daemon must add with
/// the sealed account token as the value, off-argv (the `curl --config`
/// 0600-file pattern). `headers` are the non-secret headers (Content-Type, …).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderRequest {
    /// HTTP method (`GET`/`POST`/…).
    pub method: String,
    /// Absolute URL.
    pub url: String,
    /// Non-secret headers as `(name, value)` pairs (e.g. `Content-Type`). The
    /// account token is NEVER here — see `secret_header`.
    pub headers: Vec<(String, String)>,
    /// When `Some(name)`, the daemon adds `name: <sealed account token>` off
    /// argv. `None` ⇒ the request is unauthenticated (or auth rides the body,
    /// which the adapter then omits from any logged form).
    pub secret_header: Option<String>,
    /// Optional request body (JSON for the API providers). May carry the WG
    /// *public* key (publishable), never the private key.
    pub body: Option<String>,
}

impl ProviderRequest {
    /// A bearer-authenticated JSON request: `Authorization: Bearer <token>`
    /// added by the daemon off-argv (`secret_header = "Authorization"`), with a
    /// JSON `Content-Type`.
    #[must_use]
    pub fn bearer_json(method: &str, url: impl Into<String>, body: Option<String>) -> Self {
        Self {
            method: method.to_string(),
            url: url.into(),
            headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            secret_header: Some("Authorization".to_string()),
            body,
        }
    }
}

/// What a provider adapter produces for a tunnel: the GW-1 [`TunnelDef`] +
/// the GW-2 [`TunnelSecret`] (the cleartext to seal). The [`TunnelDef`] is
/// log-safe (no key material); the [`TunnelSecret`] is the thing GW-2 seals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvisionedTunnel {
    /// The durable definition (recorded in `tunnels.toml` after `creds_ref`
    /// is stamped by the GW-2 set-secret path).
    pub def: TunnelDef,
    /// The cleartext key material to seal via `vpn_secret::seal_for`. SECRET.
    pub secret: TunnelSecret,
}

/// The provider adapter trait. An impl turns operator [`ProviderParams`] + a
/// locally-generated [`WgKeypair`] into a [`ProvisionedTunnel`] (a WireGuard
/// [`TunnelDef`] + the sealed-bound [`TunnelSecret`]). Where the provider has
/// an API to register the public key / fetch the peer, [`key_register_request`]
/// returns the pure [`ProviderRequest`] the daemon executes (token off-argv).
pub trait VpnProvider: Send + Sync {
    /// The provider this adapter speaks for.
    fn kind(&self) -> ProviderKind;

    /// Build the WireGuard [`TunnelDef`] + [`TunnelSecret`] from the params +
    /// the locally-generated keypair. The peer (server pubkey/endpoint/assigned
    /// address) comes from `params.peer`; when the provider mints it via API,
    /// the daemon first runs [`key_register_request`], fills `params.peer` from
    /// the response, then calls this. Pure.
    ///
    /// # Errors
    /// A human-readable reason when the params can't form a usable tunnel
    /// (e.g. no server pubkey + no endpoint and no API to fetch them).
    fn provision(
        &self,
        params: &ProviderParams,
        keypair: &WgKeypair,
    ) -> Result<ProvisionedTunnel, String>;

    /// The API request to register the WG **public** key / fetch the assigned
    /// peer, when the provider exposes one. `None` when this provider has no
    /// such API (the operator pastes the peer from the portal instead). The
    /// returned request never carries the account token in its body/headers —
    /// it names the `secret_header` the daemon fills off-argv. Pure.
    fn key_register_request(
        &self,
        _params: &ProviderParams,
        _keypair: &WgKeypair,
    ) -> Option<ProviderRequest> {
        None
    }

    /// The provider's default anti-leak DNS resolver, used when the operator
    /// didn't pin one. Keeps DNS inside the tunnel (GW-6 flags a DNS leak).
    fn default_dns(&self) -> &'static str;
}

/// Render a `wg-quick` `[Interface]`/`[Peer]` config body from the resolved
/// pieces. The private key + (optional) preshared key are the only secrets —
/// this string is the [`TunnelSecret::wg_conf`] GW-2 seals, never logged. Pure.
#[must_use]
fn render_wg_conf(
    private_b64: &str,
    address: &str,
    dns: &str,
    server_pubkey: &str,
    endpoint: &str,
    preshared_key: Option<&str>,
    allowed_ips: &str,
) -> String {
    let mut s = String::new();
    s.push_str("[Interface]\n");
    s.push_str(&format!("PrivateKey = {private_b64}\n"));
    if !address.is_empty() {
        s.push_str(&format!("Address = {address}\n"));
    }
    if !dns.is_empty() {
        s.push_str(&format!("DNS = {dns}\n"));
    }
    s.push('\n');
    s.push_str("[Peer]\n");
    s.push_str(&format!("PublicKey = {server_pubkey}\n"));
    if let Some(psk) = preshared_key {
        if !psk.is_empty() {
            s.push_str(&format!("PresharedKey = {psk}\n"));
        }
    }
    s.push_str(&format!("Endpoint = {endpoint}\n"));
    s.push_str(&format!("AllowedIPs = {allowed_ips}\n"));
    s
}

/// The full-tunnel `AllowedIPs` — route everything through the VPN (the egress
/// carve-out for the Nebula overlay is installed separately by GW-3, so mesh
/// traffic still bypasses the tunnel). Both families.
pub const FULL_TUNNEL_ALLOWED_IPS: &str = "0.0.0.0/0, ::/0";

/// Shared WireGuard provisioning: validate the resolved peer, render the conf,
/// and assemble the [`ProvisionedTunnel`]. The named adapters differ only in
/// their default DNS + provider label + (optional) API request; the WG body
/// construction is identical, so it's single-sourced here (§6 — glue).
fn provision_wg(
    kind: ProviderKind,
    params: &ProviderParams,
    keypair: &WgKeypair,
) -> Result<ProvisionedTunnel, String> {
    if params.tunnel_id.trim().is_empty() {
        return Err("provider: empty tunnel id".to_string());
    }
    if keypair.private_b64.trim().is_empty() {
        return Err("provider: empty WireGuard private key".to_string());
    }
    let peer = &params.peer;
    if peer.server_pubkey.trim().is_empty() {
        return Err(format!(
            "{}: no server public key (paste the peer or run the key-register API first)",
            kind.label()
        ));
    }
    if peer.endpoint.trim().is_empty() {
        return Err(format!("{}: no server endpoint", kind.label()));
    }
    if peer.assigned_address.trim().is_empty() {
        return Err(format!(
            "{}: no assigned address (the provider API or portal assigns this)",
            kind.label()
        ));
    }
    let dns = params
        .dns
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| kind.default_dns())
        .to_string();
    let wg_conf = render_wg_conf(
        &keypair.private_b64,
        &peer.assigned_address,
        &dns,
        &peer.server_pubkey,
        &peer.endpoint,
        peer.preshared_key.as_deref(),
        FULL_TUNNEL_ALLOWED_IPS,
    );
    let def = TunnelDef {
        id: params.tunnel_id.clone(),
        provider: kind.label().to_string(),
        method: Method::Wg,
        server: params.server.clone(),
        protocol: "udp".to_string(),
        creds_ref: String::new(), // stamped by the GW-2 set-secret path
        ..Default::default()
    };
    def.validate()
        .map_err(|e| format!("{}: {e}", kind.label()))?;
    Ok(ProvisionedTunnel {
        def,
        secret: TunnelSecret::wireguard(wg_conf),
    })
}

// ── the five named adapters ─────────────────────────────────────────────────

/// Mullvad — WireGuard. Exposes a key-registration API
/// (`https://api.mullvad.net/wg/`) that registers the WG **public** key under
/// an account and returns the assigned address; the account number rides the
/// `Authorization` header (off-argv). Default DNS is Mullvad's anti-leak
/// resolver.
pub struct Mullvad;

impl VpnProvider for Mullvad {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Mullvad
    }
    fn provision(
        &self,
        params: &ProviderParams,
        keypair: &WgKeypair,
    ) -> Result<ProvisionedTunnel, String> {
        provision_wg(ProviderKind::Mullvad, params, keypair)
    }
    fn key_register_request(
        &self,
        params: &ProviderParams,
        keypair: &WgKeypair,
    ) -> Option<ProviderRequest> {
        // Mullvad's WG key-registration: POST the account + the WG public key;
        // the account number is the bearer (added off-argv by the daemon). The
        // public key is publishable, so it rides the body.
        let body = serde_json::json!({
            "account": "@MULLVAD_ACCOUNT@", // placeholder — the daemon substitutes from the sealed token; never logged
            "pubkey": keypair.public_b64,
        })
        .to_string();
        let _ = params;
        Some(ProviderRequest {
            method: "POST".to_string(),
            url: "https://api.mullvad.net/wg/".to_string(),
            headers: vec![(
                "Content-Type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            )],
            // Mullvad's WG endpoint takes the account in the form body, not a
            // header; the daemon substitutes the sealed token for the
            // placeholder off-argv, so the token never appears in a builder's
            // output or a log line.
            secret_header: None,
            body: Some(body),
        })
    }
    fn default_dns(&self) -> &'static str {
        self.kind().default_dns()
    }
}

/// ProtonVPN — WireGuard. The Proton API issues a per-key WG config; the
/// session token is the bearer (off-argv). Default DNS is Proton's resolver.
pub struct ProtonVpn;

impl VpnProvider for ProtonVpn {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Protonvpn
    }
    fn provision(
        &self,
        params: &ProviderParams,
        keypair: &WgKeypair,
    ) -> Result<ProvisionedTunnel, String> {
        provision_wg(ProviderKind::Protonvpn, params, keypair)
    }
    fn key_register_request(
        &self,
        params: &ProviderParams,
        keypair: &WgKeypair,
    ) -> Option<ProviderRequest> {
        // Proton: register the client WG public key for the chosen logical
        // server; the bearer session token is added off-argv.
        let body = serde_json::json!({
            "ClientPublicKey": keypair.public_b64,
            "Mode": "persistent",
            "DeviceName": params.tunnel_id,
            "LogicalServerID": params.server,
        })
        .to_string();
        Some(ProviderRequest::bearer_json(
            "POST",
            "https://api.protonvpn.ch/vpn/v1/certificate",
            Some(body),
        ))
    }
    fn default_dns(&self) -> &'static str {
        self.kind().default_dns()
    }
}

/// IVPN — WireGuard. A key-registration API associates the WG public key with
/// the account; the account id is the bearer (off-argv).
pub struct Ivpn;

impl VpnProvider for Ivpn {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Ivpn
    }
    fn provision(
        &self,
        params: &ProviderParams,
        keypair: &WgKeypair,
    ) -> Result<ProvisionedTunnel, String> {
        provision_wg(ProviderKind::Ivpn, params, keypair)
    }
    fn key_register_request(
        &self,
        params: &ProviderParams,
        keypair: &WgKeypair,
    ) -> Option<ProviderRequest> {
        let body = serde_json::json!({
            "public_key": keypair.public_b64,
            "connect_server": params.server,
        })
        .to_string();
        Some(ProviderRequest::bearer_json(
            "POST",
            "https://api.ivpn.net/v5/session/wg/set",
            Some(body),
        ))
    }
    fn default_dns(&self) -> &'static str {
        self.kind().default_dns()
    }
}

/// NordVPN (NordLynx) — WireGuard. A credentials API returns the per-account
/// WG private key + the chosen server's pubkey/endpoint; the access token is
/// the bearer (off-argv).
pub struct NordVpn;

impl VpnProvider for NordVpn {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Nordvpn
    }
    fn provision(
        &self,
        params: &ProviderParams,
        keypair: &WgKeypair,
    ) -> Result<ProvisionedTunnel, String> {
        provision_wg(ProviderKind::Nordvpn, params, keypair)
    }
    fn key_register_request(
        &self,
        _params: &ProviderParams,
        _keypair: &WgKeypair,
    ) -> Option<ProviderRequest> {
        // Nord issues the WG private key from the account credentials endpoint
        // (NordLynx); the daemon parses the WG key from the response. No public
        // key is *registered* (Nord mints the keypair server-side), so the body
        // is empty and the access token is the bearer.
        Some(ProviderRequest::bearer_json(
            "GET",
            "https://api.nordvpn.com/v1/users/services/credentials",
            None,
        ))
    }
    fn default_dns(&self) -> &'static str {
        self.kind().default_dns()
    }
}

/// Surfshark — WireGuard. A config API returns the peer + the assigned address
/// for a registered public key; the session token is the bearer (off-argv).
pub struct Surfshark;

impl VpnProvider for Surfshark {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Surfshark
    }
    fn provision(
        &self,
        params: &ProviderParams,
        keypair: &WgKeypair,
    ) -> Result<ProvisionedTunnel, String> {
        provision_wg(ProviderKind::Surfshark, params, keypair)
    }
    fn key_register_request(
        &self,
        params: &ProviderParams,
        keypair: &WgKeypair,
    ) -> Option<ProviderRequest> {
        let body = serde_json::json!({
            "pubKey": keypair.public_b64,
            "server": params.server,
        })
        .to_string();
        Some(ProviderRequest::bearer_json(
            "POST",
            "https://api.surfshark.com/v1/account/users/public-keys",
            Some(body),
        ))
    }
    fn default_dns(&self) -> &'static str {
        self.kind().default_dns()
    }
}

// ── generic import: paste a wg-quick .conf ──────────────────────────────────

/// Parse a raw `wg-quick` `.conf` (an `[Interface]`/`[Peer]` body the operator
/// pasted from any provider) into a [`ProvisionedTunnel`]. The whole config is
/// the secret (it contains the `[Interface] PrivateKey`), so the parsed body is
/// returned as the [`TunnelSecret::wg_conf`] verbatim (after canonical
/// re-render so a round-trip is stable + the secret is normalized) and the
/// [`TunnelDef`] is log-safe. `tunnel_id` is operator-chosen.
///
/// Accepts the standard keys: `[Interface]` `PrivateKey`/`Address`/`DNS`,
/// `[Peer]` `PublicKey`/`Endpoint`/`AllowedIPs`/`PresharedKey`. Comments
/// (`#`/`;`) and blank lines are ignored; keys are case-insensitive.
///
/// # Errors
/// A human-readable reason when a required field is missing (no PrivateKey, no
/// Peer PublicKey, no Endpoint) or the file has no recognizable sections.
pub fn parse_wg_conf(tunnel_id: &str, raw: &str) -> Result<ProvisionedTunnel, String> {
    if tunnel_id.trim().is_empty() {
        return Err("paste-wg: empty tunnel id".to_string());
    }
    #[derive(Default)]
    struct Acc {
        private_key: Option<String>,
        address: Option<String>,
        dns: Option<String>,
        public_key: Option<String>,
        endpoint: Option<String>,
        allowed_ips: Option<String>,
        preshared_key: Option<String>,
    }
    let mut acc = Acc::default();
    let mut section = "";
    let mut saw_interface = false;
    let mut saw_peer = false;
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(sec) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = match sec.trim().to_ascii_lowercase().as_str() {
                "interface" => {
                    saw_interface = true;
                    "interface"
                }
                "peer" => {
                    saw_peer = true;
                    "peer"
                }
                _ => "",
            };
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim().to_ascii_lowercase();
        let val = v.trim().to_string();
        if val.is_empty() {
            continue;
        }
        match (section, key.as_str()) {
            ("interface", "privatekey") => acc.private_key = Some(val),
            ("interface", "address") => acc.address = Some(val),
            ("interface", "dns") => acc.dns = Some(val),
            ("peer", "publickey") => acc.public_key = Some(val),
            ("peer", "endpoint") => acc.endpoint = Some(val),
            ("peer", "allowedips") => acc.allowed_ips = Some(val),
            ("peer", "presharedkey") => acc.preshared_key = Some(val),
            _ => {} // ignore unknown keys (ListenPort, MTU, …) — not load-bearing
        }
    }
    if !saw_interface || !saw_peer {
        return Err("paste-wg: missing [Interface] or [Peer] section".to_string());
    }
    let private_key = acc
        .private_key
        .ok_or_else(|| "paste-wg: no [Interface] PrivateKey".to_string())?;
    let public_key = acc
        .public_key
        .ok_or_else(|| "paste-wg: no [Peer] PublicKey".to_string())?;
    let endpoint = acc
        .endpoint
        .ok_or_else(|| "paste-wg: no [Peer] Endpoint".to_string())?;
    let address = acc.address.unwrap_or_default();
    let dns = acc.dns.unwrap_or_default();
    let allowed_ips = acc
        .allowed_ips
        .unwrap_or_else(|| FULL_TUNNEL_ALLOWED_IPS.to_string());
    // Canonical re-render so the sealed secret is normalized (and the
    // round-trip is stable + testable).
    let wg_conf = render_wg_conf(
        &private_key,
        &address,
        &dns,
        &public_key,
        &endpoint,
        acc.preshared_key.as_deref(),
        &allowed_ips,
    );
    let def = TunnelDef {
        id: tunnel_id.to_string(),
        provider: "generic-wg".to_string(),
        method: Method::Wg,
        server: endpoint.clone(),
        protocol: "udp".to_string(),
        creds_ref: String::new(),
        ..Default::default()
    };
    def.validate().map_err(|e| format!("paste-wg: {e}"))?;
    Ok(ProvisionedTunnel {
        def,
        secret: TunnelSecret::wireguard(wg_conf),
    })
}

// ── generic import: an OpenVPN .ovpn ────────────────────────────────────────

/// Parse an OpenVPN `.ovpn` (the operator imported from any provider) into a
/// [`ProvisionedTunnel`]. The whole `.ovpn` is the secret (it carries inline
/// `<key>`/`<cert>`/`<tls-auth>` + maybe `auth-user-pass`), so the parsed body
/// is returned verbatim as [`TunnelSecret::ovpn_conf`]; any referenced
/// `auth-user-pass <file>` whose contents the operator supplied lands in
/// [`TunnelSecret::extra`] keyed by basename. The [`TunnelDef`] is log-safe
/// (records the `remote`/`proto`/`port` as the server selector, never the keys).
///
/// Recognizes: `remote <host> [port] [proto]`, `proto <udp|tcp>`,
/// `port <n>`, and the inline blocks `<ca>`/`<cert>`/`<key>`/`<tls-auth>`/
/// `<tls-crypt>`. Comments (`#`/`;`) and blank lines are ignored.
///
/// # Errors
/// A human-readable reason when there is no `remote` directive (an `.ovpn`
/// with no server to connect to can never come up).
pub fn parse_ovpn(tunnel_id: &str, raw: &str) -> Result<ProvisionedTunnel, String> {
    if tunnel_id.trim().is_empty() {
        return Err("import-ovpn: empty tunnel id".to_string());
    }
    let mut remote_host = String::new();
    let mut remote_port = String::new();
    let mut proto = String::new();
    let mut in_block = false;
    let mut saw_inline_secret = false;
    let mut auth_user_pass_file: Option<String> = None;
    for line in raw.lines() {
        let trimmed = line.trim();
        // Track inline blocks so a directive-looking token inside <key>…</key>
        // (base64 with `=`) isn't mistaken for a config directive. We only need
        // to know we *saw* inline secret material (for the no-op guard); the
        // body is preserved verbatim regardless.
        if trimmed.starts_with('<') && trimmed.ends_with('>') {
            let tag = trimmed.trim_matches(['<', '>']);
            if let Some(close) = tag.strip_prefix('/') {
                in_block = false;
                let _ = close;
            } else {
                in_block = true;
                if matches!(tag, "key" | "tls-auth" | "tls-crypt" | "cert" | "ca") {
                    saw_inline_secret = true;
                }
            }
            continue;
        }
        if in_block {
            continue; // inline secret payload — preserved verbatim in the body
        }
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        let mut parts = trimmed.split_whitespace();
        let Some(directive) = parts.next() else {
            continue;
        };
        match directive {
            "remote" => {
                if let Some(h) = parts.next() {
                    remote_host = h.to_string();
                }
                if let Some(p) = parts.next() {
                    remote_port = p.to_string();
                }
                if let Some(pr) = parts.next() {
                    proto = pr.to_string();
                }
            }
            "proto" => {
                if let Some(p) = parts.next() {
                    proto = p.to_string();
                }
            }
            "port" => {
                if let Some(p) = parts.next() {
                    remote_port = p.to_string();
                }
            }
            "auth-user-pass" => {
                // `auth-user-pass <file>` references a side file the operator
                // must supply; bare `auth-user-pass` prompts interactively
                // (unusable headless — recorded but no side file).
                if let Some(f) = parts.next() {
                    auth_user_pass_file = Some(f.to_string());
                }
            }
            _ => {}
        }
    }
    let _ = saw_inline_secret; // future: warn when neither inline secret nor auth-user-pass present
    if remote_host.is_empty() {
        return Err("import-ovpn: no `remote` directive".to_string());
    }
    // Normalize the proto from the obfuscation hint (tcp on obfuscated lines).
    let proto = if proto.is_empty() {
        "udp".to_string()
    } else {
        proto.to_ascii_lowercase()
    };
    let server = if remote_port.is_empty() {
        remote_host.clone()
    } else {
        format!("{remote_host}:{remote_port}")
    };
    let def = TunnelDef {
        id: tunnel_id.to_string(),
        provider: "generic-ovpn".to_string(),
        method: Method::Ovpn,
        server,
        protocol: proto,
        creds_ref: String::new(),
        ..Default::default()
    };
    def.validate().map_err(|e| format!("import-ovpn: {e}"))?;
    let mut secret = TunnelSecret::openvpn(raw.to_string());
    // If the .ovpn references an auth-user-pass file, leave a placeholder slot
    // in `extra` keyed by its basename so the operator/daemon can fill it; the
    // distributor lays it down 0600 beside the .ovpn. We don't invent creds.
    if let Some(file) = auth_user_pass_file {
        if let Some(base) = std::path::Path::new(&file)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
        {
            // Only stage the slot if the operator hasn't inlined it; an empty
            // value is honest ("you must provide these creds"), not a fake one.
            secret.extra.entry(base).or_default();
        }
    }
    Ok(ProvisionedTunnel { def, secret })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keypair() -> WgKeypair {
        WgKeypair {
            private_b64: "QF4l8m7n2p3q4r5s6t7u8v9w0x1y2z3A4B5C6D7E8F0=".to_string(),
            public_b64: "PUBKEYbase64aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa=".to_string(),
        }
    }

    fn full_params(id: &str) -> ProviderParams {
        ProviderParams {
            tunnel_id: id.to_string(),
            server: "us-nyc-wg-001".to_string(),
            account_token: "SECRET-ACCOUNT-TOKEN".to_string(),
            peer: PeerParams {
                server_pubkey: "SERVERpubkey00000000000000000000000000000000=".to_string(),
                endpoint: "198.51.100.10:51820".to_string(),
                assigned_address: "10.64.0.2/32".to_string(),
                preshared_key: None,
            },
            dns: None,
        }
    }

    #[test]
    fn provider_kind_label_round_trips() {
        for k in [
            ProviderKind::Mullvad,
            ProviderKind::Protonvpn,
            ProviderKind::Ivpn,
            ProviderKind::Nordvpn,
            ProviderKind::Surfshark,
        ] {
            assert_eq!(ProviderKind::from_label(k.label()), Some(k));
        }
        // Aliases resolve.
        assert_eq!(
            ProviderKind::from_label("proton"),
            Some(ProviderKind::Protonvpn)
        );
        assert_eq!(
            ProviderKind::from_label("nordlynx"),
            Some(ProviderKind::Nordvpn)
        );
        assert_eq!(ProviderKind::from_label("unknown"), None);
    }

    #[test]
    fn every_named_provider_builds_a_wg_tunnel() {
        for k in [
            ProviderKind::Mullvad,
            ProviderKind::Protonvpn,
            ProviderKind::Ivpn,
            ProviderKind::Nordvpn,
            ProviderKind::Surfshark,
        ] {
            let adapter = k.adapter();
            let prov = adapter
                .provision(&full_params("exit1"), &keypair())
                .unwrap_or_else(|e| panic!("{} provision: {e}", k.label()));
            // The def is a WG tunnel, log-safe (no key material), labelled.
            assert_eq!(prov.def.method, Method::Wg);
            assert_eq!(prov.def.provider, k.label());
            assert_eq!(prov.def.id, "exit1");
            assert!(prov.def.creds_ref.is_empty()); // stamped later by GW-2
                                                    // The def carries no secret.
            let toml = mackes_serde_def(&prov.def);
            assert!(!toml.contains("QF4l8m7n"), "private key leaked into def");
            // The secret carries the private key + the rendered peer.
            assert!(prov.secret.wg_conf.contains("PrivateKey = QF4l8m7n"));
            assert!(prov.secret.wg_conf.contains("PublicKey = SERVERpubkey"));
            assert!(prov
                .secret
                .wg_conf
                .contains("Endpoint = 198.51.100.10:51820"));
            assert!(prov.secret.wg_conf.contains("AllowedIPs = 0.0.0.0/0, ::/0"));
            // Default DNS filled when the operator didn't pin one.
            assert!(prov.secret.wg_conf.contains("DNS = "));
            // The secret is populated for the WG method (GW-2 will accept it).
            assert!(prov.secret.is_populated_for(Method::Wg));
        }
    }

    fn mackes_serde_def(def: &TunnelDef) -> String {
        serde_json::to_string(def).unwrap()
    }

    #[test]
    fn operator_pinned_dns_wins_over_default() {
        let mut p = full_params("exit1");
        p.dns = Some("9.9.9.9".to_string());
        let prov = Mullvad.provision(&p, &keypair()).unwrap();
        assert!(prov.secret.wg_conf.contains("DNS = 9.9.9.9"));
        assert!(!prov.secret.wg_conf.contains("DNS = 10.64.0.1"));
    }

    #[test]
    fn missing_peer_fields_are_rejected_loud() {
        let mut p = full_params("exit1");
        p.peer.server_pubkey = String::new();
        assert!(Mullvad
            .provision(&p, &keypair())
            .unwrap_err()
            .contains("server public key"));
        let mut p = full_params("exit1");
        p.peer.endpoint = String::new();
        assert!(Ivpn
            .provision(&p, &keypair())
            .unwrap_err()
            .contains("endpoint"));
        let mut p = full_params("exit1");
        p.peer.assigned_address = String::new();
        assert!(Surfshark
            .provision(&p, &keypair())
            .unwrap_err()
            .contains("assigned address"));
    }

    #[test]
    fn empty_id_or_key_rejected() {
        let mut p = full_params("");
        p.tunnel_id = String::new();
        assert!(NordVpn.provision(&p, &keypair()).is_err());
        let mut kp = keypair();
        kp.private_b64 = String::new();
        assert!(ProtonVpn.provision(&full_params("x"), &kp).is_err());
    }

    // ── API request-builders: shape + secret never in the rendered output ───

    #[test]
    fn mullvad_key_register_request_carries_pubkey_not_private_or_token() {
        let req = Mullvad
            .key_register_request(&full_params("exit1"), &keypair())
            .expect("mullvad has a key-register API");
        assert_eq!(req.method, "POST");
        assert_eq!(req.url, "https://api.mullvad.net/wg/");
        let body = req.body.as_deref().unwrap();
        // The publishable PUBLIC key rides the body…
        assert!(body.contains("PUBKEYbase64"));
        // …but never the PRIVATE key or the account token.
        assert!(!body.contains("QF4l8m7n"), "private key in request body");
        assert!(
            !body.contains("SECRET-ACCOUNT-TOKEN"),
            "token in request body"
        );
        // No header carries the token either (it's substituted off-argv).
        let joined = format!("{:?}{:?}", req.headers, req.secret_header);
        assert!(!joined.contains("SECRET-ACCOUNT-TOKEN"));
    }

    #[test]
    fn bearer_providers_name_the_auth_header_without_the_token() {
        for k in [
            ProviderKind::Protonvpn,
            ProviderKind::Ivpn,
            ProviderKind::Nordvpn,
            ProviderKind::Surfshark,
        ] {
            let adapter = k.adapter();
            let req = adapter
                .key_register_request(&full_params("exit1"), &keypair())
                .unwrap_or_else(|| {
                    panic!("{} should expose a key-register/credentials API", k.label())
                });
            // The daemon adds the bearer off-argv → the builder only NAMES the
            // header, never the value.
            assert_eq!(
                req.secret_header.as_deref(),
                Some("Authorization"),
                "{}",
                k.label()
            );
            let rendered = serde_json::to_string(&req).unwrap();
            assert!(
                !rendered.contains("SECRET-ACCOUNT-TOKEN"),
                "{} token leaked",
                k.label()
            );
            assert!(
                !rendered.contains("QF4l8m7n"),
                "{} private key leaked",
                k.label()
            );
            // The publishable public key may appear (Nord's is keyless → body None).
            if let Some(body) = &req.body {
                assert!(body.contains("PUBKEYbase64") || k == ProviderKind::Nordvpn);
            }
        }
    }

    // ── paste-WG parser ─────────────────────────────────────────────────────

    const SAMPLE_WG: &str = "\
# a provider's WireGuard config
[Interface]
PrivateKey = aAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaA0=
Address = 10.64.0.2/32, fc00:bbbb::2/128
DNS = 10.64.0.1
ListenPort = 51820

[Peer]
PublicKey = bBbBbBbBbBbBbBbBbBbBbBbBbBbBbBbBbBbBbBbBbB0=
PresharedKey = cCcCcCcCcCcCcCcCcCcCcCcCcCcCcCcCcCcCcCcCcC0=
Endpoint = 203.0.113.7:51820
AllowedIPs = 0.0.0.0/0, ::/0
";

    #[test]
    fn paste_wg_parses_all_fields_and_round_trips() {
        let prov = parse_wg_conf("pasted1", SAMPLE_WG).unwrap();
        assert_eq!(prov.def.method, Method::Wg);
        assert_eq!(prov.def.provider, "generic-wg");
        assert_eq!(prov.def.id, "pasted1");
        assert_eq!(prov.def.server, "203.0.113.7:51820");
        let c = &prov.secret.wg_conf;
        assert!(c.contains("PrivateKey = aAaAaAa"));
        assert!(c.contains("Address = 10.64.0.2/32, fc00:bbbb::2/128"));
        assert!(c.contains("DNS = 10.64.0.1"));
        assert!(c.contains("PublicKey = bBbBbBb"));
        assert!(c.contains("PresharedKey = cCcCcCc"));
        assert!(c.contains("Endpoint = 203.0.113.7:51820"));
        assert!(c.contains("AllowedIPs = 0.0.0.0/0, ::/0"));
        // Re-parsing the rendered body yields the same secret (round-trip).
        let again = parse_wg_conf("pasted1", c).unwrap();
        assert_eq!(again.secret, prov.secret);
        // Never leaks into the log-safe def.
        assert!(!serde_json::to_string(&prov.def)
            .unwrap()
            .contains("aAaAaAa"));
    }

    #[test]
    fn paste_wg_is_case_insensitive_and_skips_comments() {
        let raw = "\
; semicolon comment
[interface]
privatekey = KEYkeyKEYkeyKEYkeyKEYkeyKEYkeyKEYkeyKEYke0=
[PEER]
PUBLICKEY = PUBpubPUBpubPUBpubPUBpubPUBpubPUBpubPUBpu0=
endpoint = 198.51.100.1:1234
";
        let prov = parse_wg_conf("ci", raw).unwrap();
        assert!(prov.secret.wg_conf.contains("PrivateKey = KEYkey"));
        assert!(prov.secret.wg_conf.contains("PublicKey = PUBpub"));
        // No AllowedIPs in the input → defaults to full-tunnel.
        assert!(prov.secret.wg_conf.contains("AllowedIPs = 0.0.0.0/0, ::/0"));
    }

    #[test]
    fn paste_wg_rejects_malformed_input() {
        // No sections.
        assert!(parse_wg_conf("x", "just some text\nno sections").is_err());
        // Interface but no peer.
        assert!(parse_wg_conf("x", "[Interface]\nPrivateKey = k=\n")
            .unwrap_err()
            .contains("[Peer]"));
        // Peer with no pubkey.
        assert!(parse_wg_conf(
            "x",
            "[Interface]\nPrivateKey = k=\n[Peer]\nEndpoint = a:1\n"
        )
        .unwrap_err()
        .contains("PublicKey"));
        // No private key.
        assert!(
            parse_wg_conf("x", "[Interface]\n[Peer]\nPublicKey = p=\nEndpoint = a:1\n")
                .unwrap_err()
                .contains("PrivateKey")
        );
        // No endpoint.
        assert!(
            parse_wg_conf("x", "[Interface]\nPrivateKey=k=\n[Peer]\nPublicKey=p=\n")
                .unwrap_err()
                .contains("Endpoint")
        );
        // Empty id.
        assert!(parse_wg_conf("", SAMPLE_WG).is_err());
    }

    // ── .ovpn parser ────────────────────────────────────────────────────────

    const SAMPLE_OVPN: &str = "\
client
dev tun
proto udp
remote vpn.example.com 1194 udp
auth-user-pass auth.txt
<ca>
-----BEGIN CERTIFICATE-----
MIIBcaCERTcaCERTcaCERTca
-----END CERTIFICATE-----
</ca>
<key>
-----BEGIN PRIVATE KEY-----
proto secret-looking-line-inside-the-key-block
-----END PRIVATE KEY-----
</key>
<tls-auth>
deadbeef
</tls-auth>
";

    #[test]
    fn ovpn_parses_remote_proto_port_and_preserves_body() {
        let prov = parse_ovpn("ovpn1", SAMPLE_OVPN).unwrap();
        assert_eq!(prov.def.method, Method::Ovpn);
        assert_eq!(prov.def.provider, "generic-ovpn");
        assert_eq!(prov.def.id, "ovpn1");
        assert_eq!(prov.def.server, "vpn.example.com:1194");
        assert_eq!(prov.def.protocol, "udp");
        // The whole .ovpn (incl. inline secret blocks) is preserved verbatim.
        assert_eq!(prov.secret.ovpn_conf, SAMPLE_OVPN);
        assert!(prov.secret.ovpn_conf.contains("BEGIN PRIVATE KEY"));
        // auth-user-pass file staged in extra (empty — operator supplies creds).
        assert!(prov.secret.extra.contains_key("auth.txt"));
        assert_eq!(
            prov.secret.extra.get("auth.txt").map(String::as_str),
            Some("")
        );
        // The def is log-safe: no key material.
        assert!(!serde_json::to_string(&prov.def)
            .unwrap()
            .contains("PRIVATE KEY"));
    }

    #[test]
    fn ovpn_directive_inside_key_block_is_not_misparsed() {
        // The `proto secret-looking-line-inside-the-key-block` line lives INSIDE
        // <key>…</key>; it must NOT override the real `proto udp`.
        let prov = parse_ovpn("ovpn1", SAMPLE_OVPN).unwrap();
        assert_eq!(prov.def.protocol, "udp");
    }

    #[test]
    fn ovpn_proto_and_port_directives_override() {
        let raw = "\
client
remote vpn.example.com
proto tcp
port 443
";
        let prov = parse_ovpn("o", raw).unwrap();
        assert_eq!(prov.def.server, "vpn.example.com:443");
        assert_eq!(prov.def.protocol, "tcp");
    }

    #[test]
    fn ovpn_without_remote_is_rejected() {
        assert!(parse_ovpn("o", "client\nproto udp\n")
            .unwrap_err()
            .contains("remote"));
        assert!(parse_ovpn("", SAMPLE_OVPN).is_err());
    }

    #[test]
    fn ovpn_is_populated_for_ovpn_method() {
        let prov = parse_ovpn("o", SAMPLE_OVPN).unwrap();
        assert!(prov.secret.is_populated_for(Method::Ovpn));
        assert!(!prov.secret.is_populated_for(Method::Wg));
    }
}
