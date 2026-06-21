//! VPN-GW-1/2 (responder) — `action/vpn/*` over the tunnel config + `wg-quick`/
//! `openvpn` (design: `docs/design/vpn-gateway.md`).
//!
//! CRUD on the per-node [`mackes_mesh_types::vpn::VpnConfig`] (TOML on the shared
//! substrate) + best-effort bring-up/down via the pure argv builders.
//!
//! VPN-GW-2 adds the encrypted, leader-managed secret plane: `set-secret` seals
//! a tunnel's WireGuard/OpenVPN creds under the mesh key into the assigned
//! node's `secrets/vpn/<node>/<tunnel>.age` blob (the XCP-7 / EFF-21 pattern,
//! reusing the `ca::backup` envelope — §6), recording only a log-safe
//! `creds_ref` in `tunnels.toml`; `remove-tunnel` rotates/removes that secret +
//! the decrypted cleartext. The receiving node's
//! [`crate::workers::vpn_secret_distributor`] decrypts the blob to
//! `/etc/wireguard/<ifname>.conf` / `/etc/openvpn/client/<ifname>.ovpn` before
//! VPN-GW-1's bring-up spawns `wg-quick`/`openvpn`. Material never hits
//! `ps`/argv/logs.
//!
//! VPN-GW-5 adds two creation paths on top of GW-1/2: `add-from-provider`
//! (a named provider — Mullvad/Proton/IVPN/Nord/Surfshark — params → a WG
//! [`TunnelDef`] with a locally-minted x25519 keypair) and `import-config`
//! (a pasted `wg-quick` `.conf` or an imported `.ovpn` → a [`TunnelDef`]).
//! Both reuse the GW-2 seal path: the generated/imported key material is sealed
//! into the assigned node's `secrets/vpn/<node>/<tunnel>.age` blob and only a
//! log-safe `creds_ref` is recorded — the private key never touches argv/logs.
//! A tunnel created this way is then brought up by GW-1 and verified by GW-6.
//!
//! Same dedicated-OS-thread shape as the Connect/Route responders.

use std::collections::HashMap;
use std::path::PathBuf;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde_json::json;

use mackes_mesh_types::vpn::{self, EgressRoute, Method, TunnelDef};
use mackes_mesh_types::vpn_provider::{
    self, PeerParams, ProviderKind, ProviderParams, ProvisionedTunnel,
};

/// The VPN responder — rooted at the shared workgroup root (the config home).
#[derive(Debug, Clone)]
pub struct VpnService {
    workgroup_root: PathBuf,
    /// `wg-quick`/`openvpn`/`ip` binaries are spawned by default; tests set the
    /// flag false to exercise the pure CRUD without the system tools.
    spawn: bool,
    /// VPN-GW-2 — the mesh key used to seal tunnel secrets into the per-node
    /// `secrets/vpn/<node>/<tunnel>.age` blobs. Resolved from the env
    /// (EFF-21 boot-capture) at construction; `None` ⇒ `set-secret` honestly
    /// reports "no mesh key" instead of writing an unencrypted blob.
    mesh_key: Option<String>,
}

impl VpnService {
    /// Build the service rooted at the shared workgroup root, resolving the mesh
    /// key from the environment ([`crate::vpn_secret::mesh_key_from_env`]).
    #[must_use]
    pub fn new(workgroup_root: PathBuf) -> Self {
        Self {
            workgroup_root,
            spawn: true,
            mesh_key: crate::vpn_secret::mesh_key_from_env(),
        }
    }

    /// Disable the tool shell-out (tests).
    #[must_use]
    pub fn without_spawn(mut self) -> Self {
        self.spawn = false;
        self
    }

    /// VPN-GW-2 — supply the mesh key explicitly (tests / a CA-key fallback at
    /// the wiring layer). Wins over the env read.
    #[must_use]
    pub fn with_mesh_key(mut self, key: Option<String>) -> Self {
        self.mesh_key = key;
        self
    }
}

/// Action verbs served on `action/vpn/<verb>`. VPN-GW-2 adds `set-secret`
/// (seal a tunnel's creds to an assigned node's blob) + `secret-status`
/// (does the assigned node have a blob? — never reveals the material).
/// VPN-GW-4 adds the egress-routing surface: `set-route` (assign a scope →
/// gateway + ordered tunnel chain + kill-switch), `clear-route` (drop a scope's
/// assignment), `list-routes` (the durable assignments), and `route-status`
/// (each scope's gateway + chain + the currently-active tunnel from the
/// failover selector run against live tunnel status).
///
/// VPN-GW-6 adds `tunnel-health` — the gateway's published per-tunnel verified
/// health (verdict + exit IP) — and enriches `route-status` with the same.
pub const ACTION_VERBS: [&str; 15] = [
    "list-tunnels",
    "add-tunnel",
    "remove-tunnel",
    "tunnel-up",
    "tunnel-down",
    "tunnel-status",
    "set-secret",
    "secret-status",
    "set-route",
    "clear-route",
    "list-routes",
    "route-status",
    "tunnel-health",
    // VPN-GW-5 — first-class provider setup + generic config import.
    "add-from-provider",
    "import-config",
];

/// Responder poll interval.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);

/// Action topic for `verb`: `action/vpn/<verb>`.
#[must_use]
pub fn action_topic(verb: &str) -> String {
    format!("action/vpn/{verb}")
}

/// Is `ifname` a present network interface? (`ip -o link show <ifname>`.)
fn iface_up(spawn: bool, ifname: &str) -> bool {
    if !spawn {
        return false;
    }
    std::process::Command::new("ip")
        .args(["-o", "link", "show", ifname])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Build the reply for one `action/vpn/<verb>` request.
#[must_use]
pub fn build_reply(svc: &VpnService, verb: &str, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let root = svc.workgroup_root.as_path();
    match verb {
        "list-tunnels" => {
            let cfg = vpn::load(root);
            json!({ "ok": true, "tunnels": cfg.tunnel }).to_string()
        }
        "add-tunnel" => {
            let Some(body) = req_body else {
                return err("add-tunnel: missing TunnelDef body".into());
            };
            let t: TunnelDef = match serde_json::from_str(body) {
                Ok(t) => t,
                Err(e) => return err(format!("add-tunnel: bad json: {e}")),
            };
            if let Err(e) = t.validate() {
                return err(format!("add-tunnel: {e}"));
            }
            let mut cfg = vpn::load(root);
            cfg.upsert(t);
            match vpn::save(root, &cfg) {
                Ok(_) => json!({ "ok": true }).to_string(),
                Err(e) => err(format!("add-tunnel: save: {e}")),
            }
        }
        "remove-tunnel" => {
            let Some(id) = req_body.map(str::trim).filter(|s| !s.is_empty()) else {
                return err("remove-tunnel: missing tunnel id".into());
            };
            let mut cfg = vpn::load(root);
            // Capture the def before removal so we can rotate its secret +
            // materialized cleartext (VPN-GW-2: "deleting a tunnel rotates/
            // removes its secret").
            let removed = cfg.get(id).cloned();
            if !cfg.remove(id) {
                return err(format!("remove-tunnel: no such tunnel '{id}'"));
            }
            if let Some(t) = &removed {
                // Best-effort: bring it down before forgetting it.
                let _ = bring(svc, t, false);
                // Rotate the secret: drop every node's sealed blob for this
                // tunnel + the local decrypted cleartext, so no key lingers.
                let _ = purge_tunnel_secret(root, id);
                let _ = crate::vpn_secret::remove_materialized(t);
            }
            match vpn::save(root, &cfg) {
                Ok(_) => json!({ "ok": true }).to_string(),
                Err(e) => err(format!("remove-tunnel: save: {e}")),
            }
        }
        "tunnel-up" | "tunnel-down" => {
            let up = verb == "tunnel-up";
            let Some(id) = req_body.map(str::trim).filter(|s| !s.is_empty()) else {
                return err(format!("{verb}: missing tunnel id"));
            };
            let cfg = vpn::load(root);
            let Some(t) = cfg.get(id) else {
                return err(format!("{verb}: no such tunnel '{id}'"));
            };
            let (ran, detail) = bring(svc, t, up);
            json!({ "ok": ran, "ifname": t.ifname(), "detail": detail }).to_string()
        }
        "tunnel-status" => {
            let Some(id) = req_body.map(str::trim).filter(|s| !s.is_empty()) else {
                return err("tunnel-status: missing tunnel id".into());
            };
            let cfg = vpn::load(root);
            let Some(t) = cfg.get(id) else {
                return err(format!("tunnel-status: no such tunnel '{id}'"));
            };
            let ifname = t.ifname();
            json!({ "ok": true, "ifname": ifname, "up": iface_up(svc.spawn, &ifname) }).to_string()
        }
        // VPN-GW-2 — seal a tunnel's creds into an assigned node's blob. Body:
        // `{ "tunnel_id": "...", "node_id": "...", "secret": <TunnelSecret> }`.
        // The cleartext is sealed under the mesh key + written to
        // `secrets/vpn/<node>/<tunnel>.age`; only a log-safe `creds_ref` is
        // recorded in `tunnels.toml`. The material never hits argv/logs.
        "set-secret" => build_set_secret_reply(svc, req_body),
        // VPN-GW-2 — does the assigned node have a sealed blob for this tunnel?
        // Body: `{ "tunnel_id": "...", "node_id": "..." }`. Reports presence +
        // size only — never the secret. Useful for the panel's "creds present"
        // badge + a sanity check that distribution landed.
        "secret-status" => build_secret_status_reply(svc, req_body),
        // VPN-GW-4 — assign a scope's internet egress to a gateway + an ordered
        // tunnel chain. Body is an `EgressRoute`
        // (`{ "scope": {...}, "gateway": "...", "chain": [...], "kill_switch": .. }`).
        // Persists into `routes.toml`; the `vpn_gateway` worker on the gateway
        // node selects the active tunnel from the chain + applies its egress.
        "set-route" => build_set_route_reply(svc, req_body),
        // VPN-GW-4 — drop a scope's assignment. Body: the scope key
        // (`node:<id>` / `group:<name>` / `any`).
        "clear-route" => build_clear_route_reply(svc, req_body),
        // VPN-GW-4 — the durable egress-route assignments.
        "list-routes" => {
            let cfg = vpn::load_routes(root);
            json!({ "ok": true, "routes": cfg.route }).to_string()
        }
        // VPN-GW-4 — each scope's gateway + chain + the currently-active tunnel.
        // The active tunnel is the failover selector run against live per-tunnel
        // up/down status (VPN-GW-1's interface check) on the box this responder
        // runs on — best-effort visibility; the gateway node's own worker is the
        // authority that actually applies the egress.
        "route-status" => build_route_status_reply(svc),
        // VPN-GW-6 — the gateway's published per-tunnel verified health (verdict
        // + exit IP). Pure read of the worker's published state.
        "tunnel-health" => build_tunnel_health_reply(svc),
        // VPN-GW-5 — provider params → a WG TunnelDef (local x25519 keypair) +
        // sealed secret, in one reachable action.
        "add-from-provider" => build_add_from_provider_reply(svc, req_body),
        // VPN-GW-5 — a pasted wg-quick .conf / imported .ovpn → a TunnelDef +
        // sealed secret.
        "import-config" => build_import_config_reply(svc, req_body),
        other => err(format!("unknown vpn verb: {other}")),
    }
}

/// Parsed body of a `set-secret` request.
#[derive(serde::Deserialize)]
struct SetSecretReq {
    tunnel_id: String,
    node_id: String,
    secret: mackes_mesh_types::vpn::TunnelSecret,
}

/// Parsed body of a `secret-status` request.
#[derive(serde::Deserialize)]
struct SecretRef {
    tunnel_id: String,
    node_id: String,
}

/// VPN-GW-2 — seal a tunnel's secret to an assigned node's blob + record the
/// `creds_ref`. Honest when the mesh key is absent or the tunnel/payload is
/// invalid; never persists cleartext into `tunnels.toml`.
fn build_set_secret_reply(svc: &VpnService, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let root = svc.workgroup_root.as_path();
    let Some(body) = req_body else {
        return err("set-secret: missing body".into());
    };
    let req: SetSecretReq = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => return err(format!("set-secret: bad json: {e}")),
    };
    let Some(mesh_key) = svc.mesh_key.as_deref() else {
        return err(format!(
            "set-secret: no mesh key ({}); can't seal tunnel secret",
            crate::vpn_secret::MESH_KEY_ENV
        ));
    };
    let mut cfg = vpn::load(root);
    let Some(def) = cfg.get(&req.tunnel_id).cloned() else {
        return err(format!("set-secret: no such tunnel '{}'", req.tunnel_id));
    };
    // Seal (validates the payload matches the method) — material stays in memory.
    let sealed = match crate::vpn_secret::seal_for(mesh_key, &def, &req.secret) {
        Ok(b) => b,
        Err(e) => return err(format!("set-secret: {e}")),
    };
    let path = vpn::secret_path(root, &req.node_id, &req.tunnel_id);
    if let Some(dir) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(dir) {
            return err(format!("set-secret: mkdir {}: {e}", dir.display()));
        }
    }
    if let Err(e) = write_blob_0600(&path, &sealed) {
        return err(format!("set-secret: write blob: {e}"));
    }
    // Record the log-safe creds_ref on the def (idempotent).
    let mut updated = def;
    updated.creds_ref = vpn::creds_ref(&req.tunnel_id);
    cfg.upsert(updated);
    match vpn::save(root, &cfg) {
        Ok(_) => json!({
            "ok": true,
            "node_id": req.node_id,
            "tunnel_id": req.tunnel_id,
            "bytes": sealed.len(),
        })
        .to_string(),
        Err(e) => err(format!("set-secret: save cfg: {e}")),
    }
}

/// VPN-GW-2 — report whether the assigned node has a sealed blob (presence +
/// byte size only). Never decrypts, never reveals the material.
fn build_secret_status_reply(svc: &VpnService, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let root = svc.workgroup_root.as_path();
    let Some(body) = req_body else {
        return err("secret-status: missing body".into());
    };
    let req: SecretRef = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => return err(format!("secret-status: bad json: {e}")),
    };
    let path = vpn::secret_path(root, &req.node_id, &req.tunnel_id);
    let (present, bytes) = std::fs::metadata(&path)
        .map(|m| (true, m.len()))
        .unwrap_or((false, 0));
    json!({
        "ok": true,
        "node_id": req.node_id,
        "tunnel_id": req.tunnel_id,
        "present": present,
        "bytes": bytes,
    })
    .to_string()
}

/// VPN-GW-4 — persist an egress-route assignment. Body is an `EgressRoute`. The
/// route is validated (non-empty gateway + chain + scope key) before save so a
/// route that can never carry egress is refused loud, not at reconcile time.
fn build_set_route_reply(svc: &VpnService, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let root = svc.workgroup_root.as_path();
    let Some(body) = req_body else {
        return err("set-route: missing EgressRoute body".into());
    };
    let r: EgressRoute = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => return err(format!("set-route: bad json: {e}")),
    };
    if let Err(e) = r.validate() {
        return err(format!("set-route: {e}"));
    }
    let scope_key = r.scope_key();
    // DDNS-EGRESS-3 — capture the chain before the move so we can auto-add a
    // DDNS record per tunnel the route references (idempotent).
    let chain = r.chain.clone();
    let mut cfg = vpn::load_routes(root);
    cfg.upsert(r);
    match vpn::save_routes(root, &cfg) {
        Ok(_) => {
            for tunnel_id in &chain {
                ddns_auto_add_tunnel_record(root, tunnel_id);
            }
            json!({ "ok": true, "scope": scope_key }).to_string()
        }
        Err(e) => err(format!("set-route: save: {e}")),
    }
}

/// VPN-GW-4 — drop a scope's egress assignment. Body is the scope key
/// (`node:<id>` / `group:<name>` / `any`).
fn build_clear_route_reply(svc: &VpnService, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let root = svc.workgroup_root.as_path();
    let Some(key) = req_body.map(str::trim).filter(|s| !s.is_empty()) else {
        return err("clear-route: missing scope key".into());
    };
    let mut cfg = vpn::load_routes(root);
    if !cfg.remove(key) {
        return err(format!("clear-route: no route for scope '{key}'"));
    }
    match vpn::save_routes(root, &cfg) {
        Ok(_) => json!({ "ok": true, "scope": key }).to_string(),
        Err(e) => err(format!("clear-route: save: {e}")),
    }
}

/// VPN-GW-4/6 — report, per assigned scope, the gateway + the ordered chain + the
/// currently-active tunnel (the failover selector) + the kill-switch flag, plus
/// (VPN-GW-6) the per-chain-tunnel **verified health**: verdict + the verified
/// exit IP the gateway's `vpn_gateway` worker published. The active tunnel is
/// preferentially derived from the published health verdict (the same input the
/// worker fails over on) so the panel sees the SAME pick the worker applied;
/// when no health has been published yet (the worker hasn't ticked, or this
/// responder runs on a non-gateway box) it falls back to the bare interface check
/// seen from this box. Pure visibility — it applies nothing.
fn build_route_status_reply(svc: &VpnService) -> String {
    let root = svc.workgroup_root.as_path();
    let routes = vpn::load_routes(root);
    let tunnels = vpn::load(root);
    // VPN-GW-6 — the verified health the gateway worker published (verdict +
    // exit IP per tunnel id). Absent/empty when no worker has run yet.
    let health = crate::workers::vpn_gateway::HealthState::load(
        &crate::workers::vpn_gateway::default_health_path(root),
    );
    let statuses: Vec<serde_json::Value> = routes
        .route
        .iter()
        .map(|r| {
            // Prefer the published health verdict (what the worker fails over on);
            // fall back to the bare interface check when no health is published.
            let active = if health.tunnel.is_empty() {
                vpn::select_active(r, |tunnel_id| {
                    tunnels
                        .get(tunnel_id)
                        .is_some_and(|t| iface_up(svc.spawn, &t.ifname()))
                })
            } else {
                vpn::select_active(r, |tunnel_id| vpn::health_is_up(&health.tunnel, tunnel_id))
            };
            // Per-chain-tunnel verified health for the UI (GW-7): verdict + exit IP.
            let chain_health: Vec<serde_json::Value> = r
                .chain
                .iter()
                .map(|tid| match health.tunnel.get(tid) {
                    Some(h) => json!({
                        "tunnel": tid,
                        "verdict": h.verdict.as_str(),
                        "exit_ip": h.exit_ip,
                        "live": h.live,
                        "exit_ip_is_provider": h.exit_ip_is_provider,
                        "dns_leak": h.dns_leak,
                    }),
                    None => json!({ "tunnel": tid, "verdict": "down", "exit_ip": null }),
                })
                .collect();
            json!({
                "scope": r.scope_key(),
                "gateway": r.gateway,
                "chain": r.chain,
                "kill_switch": r.kill_switch,
                "active": active.tunnel_id(),
                "health": chain_health,
            })
        })
        .collect();
    json!({ "ok": true, "routes": statuses }).to_string()
}

/// VPN-GW-6 — a dedicated read of the gateway's published per-tunnel verified
/// health: verdict + exit IP for every tunnel the worker last checked. Distinct
/// from `route-status` (which is per-route) so a UI/operator can poll the raw
/// health map directly. Reports `{}` when no worker has published yet.
fn build_tunnel_health_reply(svc: &VpnService) -> String {
    let root = svc.workgroup_root.as_path();
    let state = crate::workers::vpn_gateway::HealthState::load(
        &crate::workers::vpn_gateway::default_health_path(root),
    );
    let tunnels: Vec<serde_json::Value> = state
        .tunnel
        .values()
        .map(|h| {
            json!({
                "tunnel": h.tunnel_id,
                "verdict": h.verdict.as_str(),
                "exit_ip": h.exit_ip,
                "live": h.live,
                "exit_ip_is_provider": h.exit_ip_is_provider,
                "dns_leak": h.dns_leak,
            })
        })
        .collect();
    json!({ "ok": true, "tunnels": tunnels }).to_string()
}

// ── VPN-GW-5 — provider setup + generic config import ───────────────────────

/// Parsed body of an `add-from-provider` request. The provider is named; the
/// chosen server + (optional) statically-known peer come from the operator; the
/// `account_token` is a SECRET (sealed via GW-2, NEVER echoed back / logged).
/// `node_id` is the gateway the sealed secret is distributed to (as for
/// `set-secret`).
#[derive(serde::Deserialize)]
struct AddFromProviderReq {
    provider: String,
    tunnel_id: String,
    node_id: String,
    #[serde(default)]
    server: String,
    /// SECRET — the provider account/session token, sealed alongside the key
    /// material. Never returned in the reply or logged.
    #[serde(default)]
    account_token: String,
    #[serde(default)]
    dns: Option<String>,
    // The statically-known WG peer (when the operator pasted it from the portal
    // rather than relying on the provider API at runtime).
    #[serde(default)]
    server_pubkey: String,
    #[serde(default)]
    endpoint: String,
    #[serde(default)]
    assigned_address: String,
    #[serde(default)]
    preshared_key: Option<String>,
}

/// Parsed body of an `import-config` request: a pasted `wg-quick` `.conf`
/// (`kind = "wg"`) or an imported `.ovpn` (`kind = "ovpn"`). The whole config
/// is the SECRET (it carries the private key / inline certs) — sealed via GW-2,
/// never logged.
#[derive(serde::Deserialize)]
struct ImportConfigReq {
    /// `"wg"` (paste a wg-quick conf) or `"ovpn"` (import an OpenVPN config).
    kind: String,
    tunnel_id: String,
    node_id: String,
    /// The raw config text. SECRET. Never returned / logged.
    config: String,
}

/// VPN-GW-5 — `add-from-provider`: a named provider's params → a WG
/// [`TunnelDef`] (with a locally-generated x25519 keypair) + the sealed secret,
/// in one reachable action. The account token + generated private key are
/// sealed via the GW-2 path (`vpn_secret::seal_for`) into the assigned node's
/// blob — never on argv/logs. Honest when the mesh key is absent or the params
/// can't form a usable tunnel.
fn build_add_from_provider_reply(svc: &VpnService, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let root = svc.workgroup_root.as_path();
    let Some(body) = req_body else {
        return err("add-from-provider: missing body".into());
    };
    let req: AddFromProviderReq = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => return err(format!("add-from-provider: bad json: {e}")),
    };
    let Some(kind) = ProviderKind::from_label(&req.provider) else {
        return err(format!(
            "add-from-provider: unknown provider '{}'",
            req.provider
        ));
    };
    // Need the mesh key up front — fail before minting a keypair we can't seal.
    let Some(mesh_key) = svc.mesh_key.as_deref() else {
        return err(format!(
            "add-from-provider: no mesh key ({}); can't seal provider secret",
            crate::vpn_secret::MESH_KEY_ENV
        ));
    };
    // Mint the WG keypair locally (§3 — x25519 via the workspace crypto, never
    // OpenSSL). The private key lives only in the sealed secret below.
    let keypair = crate::vpn_keypair::generate();
    let params = ProviderParams {
        tunnel_id: req.tunnel_id.clone(),
        server: req.server.clone(),
        account_token: req.account_token.clone(),
        dns: req.dns.clone(),
        peer: PeerParams {
            server_pubkey: req.server_pubkey.clone(),
            endpoint: req.endpoint.clone(),
            assigned_address: req.assigned_address.clone(),
            preshared_key: req.preshared_key.clone(),
        },
    };
    let adapter = kind.adapter();
    let provisioned = match adapter.provision(&params, &keypair) {
        Ok(p) => p,
        Err(e) => return err(format!("add-from-provider: {e}")),
    };
    // Whether a key-register/credentials API exists for this provider (the
    // daemon's API executor would run it with the token off-argv; live calls
    // are deferred — no creds here). Surfaced so the caller knows whether the
    // pasted peer was required.
    let has_api = adapter.key_register_request(&params, &keypair).is_some();
    let public_key = keypair.public_b64.clone();
    match persist_provisioned(root, mesh_key, &req.node_id, provisioned) {
        Ok(bytes) => {
            // DDNS-EGRESS-3 — auto-add a templated DDNS record for the new
            // tunnel (idempotent): assigning a VPN exit yields a subdomain with
            // zero extra steps.
            ddns_auto_add_tunnel_record(root, &req.tunnel_id);
            json!({
            "ok": true,
            "provider": kind.label(),
            "tunnel_id": req.tunnel_id,
            "node_id": req.node_id,
            // The PUBLIC key is publishable (the operator registers it with the
            // provider when there's no live API call); the private key + token
            // stay sealed and are never returned.
            "public_key": public_key,
            "has_provider_api": has_api,
            "sealed_bytes": bytes,
            })
            .to_string()
        }
        Err(e) => err(format!("add-from-provider: {e}")),
    }
}

/// VPN-GW-5 — `import-config`: a pasted `wg-quick` `.conf` or an imported
/// `.ovpn` → a [`TunnelDef`] + the sealed secret. The raw config (which carries
/// the private key / inline certs) is sealed via GW-2 — never logged.
fn build_import_config_reply(svc: &VpnService, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let root = svc.workgroup_root.as_path();
    let Some(body) = req_body else {
        return err("import-config: missing body".into());
    };
    let req: ImportConfigReq = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => return err(format!("import-config: bad json: {e}")),
    };
    let Some(mesh_key) = svc.mesh_key.as_deref() else {
        return err(format!(
            "import-config: no mesh key ({}); can't seal imported config",
            crate::vpn_secret::MESH_KEY_ENV
        ));
    };
    let provisioned = match req.kind.trim().to_ascii_lowercase().as_str() {
        "wg" | "wireguard" | "paste-wg" => {
            match vpn_provider::parse_wg_conf(&req.tunnel_id, &req.config) {
                Ok(p) => p,
                Err(e) => return err(format!("import-config: {e}")),
            }
        }
        "ovpn" | "openvpn" => match vpn_provider::parse_ovpn(&req.tunnel_id, &req.config) {
            Ok(p) => p,
            Err(e) => return err(format!("import-config: {e}")),
        },
        other => {
            return err(format!(
                "import-config: unknown kind '{other}' (want 'wg' or 'ovpn')"
            ))
        }
    };
    let method = provisioned.def.method;
    match persist_provisioned(root, mesh_key, &req.node_id, provisioned) {
        Ok(bytes) => {
            // DDNS-EGRESS-3 — auto-add a templated DDNS record for the imported
            // tunnel (idempotent).
            ddns_auto_add_tunnel_record(root, &req.tunnel_id);
            json!({
            "ok": true,
            "kind": req.kind,
            "method": match method { Method::Ovpn => "ovpn", _ => "wg" },
            "tunnel_id": req.tunnel_id,
            "node_id": req.node_id,
            "sealed_bytes": bytes,
            })
            .to_string()
        }
        Err(e) => err(format!("import-config: {e}")),
    }
}

/// VPN-GW-5 — persist a freshly [`ProvisionedTunnel`]: seal its secret to the
/// assigned node's blob (GW-2 `seal_for` → 0600 `.age`), stamp the log-safe
/// `creds_ref` on the def, and save `tunnels.toml`. The exact same secret path
/// `set-secret` uses, so a provider/imported tunnel is then brought up by GW-1
/// + verified by GW-6. Returns the sealed byte count on success. The cleartext
/// secret stays in memory only for the seal call — never on argv/logs.
fn persist_provisioned(
    root: &std::path::Path,
    mesh_key: &str,
    node_id: &str,
    provisioned: ProvisionedTunnel,
) -> Result<usize, String> {
    let ProvisionedTunnel { mut def, secret } = provisioned;
    // Seal (validates the payload matches the method) before touching disk.
    let sealed =
        crate::vpn_secret::seal_for(mesh_key, &def, &secret).map_err(|e| format!("seal: {e}"))?;
    let path = vpn::secret_path(root, node_id, &def.id);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    }
    write_blob_0600(&path, &sealed).map_err(|e| format!("write blob: {e}"))?;
    // Record the log-safe creds_ref on the def, then upsert into the config.
    def.creds_ref = vpn::creds_ref(&def.id);
    let mut cfg = vpn::load(root);
    cfg.upsert(def);
    vpn::save(root, &cfg).map_err(|e| format!("save cfg: {e}"))?;
    Ok(sealed.len())
}

/// DDNS-EGRESS-3 auto-population — when a VPN-GW tunnel/route is created, ensure
/// the `[ddns]` config has a templated record for `tunnel_id` so assigning a VPN
/// exit yields a subdomain with zero extra steps. **Idempotent** (keyed on the
/// `tunnel:<id>` source via [`DdnsConfig::auto_add_tunnel_record`]): re-creating
/// a tunnel/route doesn't duplicate, and an operator-edited record for the same
/// source is left untouched. Best-effort + non-fatal — a DDNS save failure must
/// never wedge the tunnel/route create (the operator can still add the record by
/// hand over `action/ddns/*`), so it only logs. Removing the tunnel can leave
/// the record per `on_down` (full on-down/reconnect behavior is DDNS-EGRESS-4).
fn ddns_auto_add_tunnel_record(root: &std::path::Path, tunnel_id: &str) {
    let mut cfg = mackes_mesh_types::ddns::load(root);
    if !cfg.auto_add_tunnel_record(tunnel_id) {
        return; // already tracked — nothing to persist (idempotent)
    }
    match mackes_mesh_types::ddns::save(root, &cfg) {
        Ok(_) => tracing::info!(
            tunnel = %tunnel_id,
            "ddns: auto-added a record for the new VPN tunnel (DDNS-EGRESS-3)"
        ),
        Err(e) => tracing::warn!(
            tunnel = %tunnel_id, error = %e,
            "ddns: auto-add record failed; tunnel create still succeeded"
        ),
    }
}

/// Write a sealed blob at mode 0600 (atomic temp+rename). The `.age` blob is
/// already ciphertext, but 0600 keeps a stolen-bytes window from a co-tenant.
fn write_blob_0600(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let tmp = path.with_extension("age.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    std::fs::rename(&tmp, path)
}

/// Remove every node's sealed blob for `tunnel_id` (rotate-on-delete). Walks
/// `secrets/vpn/<node>/` dirs + removes the matching `<tunnel>.age`. Best-effort.
fn purge_tunnel_secret(root: &std::path::Path, tunnel_id: &str) -> std::io::Result<()> {
    let secret_root = vpn::secret_root(root);
    let Ok(nodes) = std::fs::read_dir(&secret_root) else {
        return Ok(()); // no secrets dir yet — nothing to purge
    };
    for node in nodes.flatten() {
        // secret_path sanitizes the tunnel id the same way it was written.
        let dummy_node = node.file_name();
        let blob = vpn::secret_path(root, &dummy_node.to_string_lossy(), tunnel_id);
        if blob.exists() {
            let _ = std::fs::remove_file(&blob);
        }
    }
    Ok(())
}

/// Bring a tunnel up/down via the right tool. Returns `(ran_ok, detail)`. Honest
/// when the tool/config is absent — never panics. `Cli`/`Api` methods aren't
/// process-spawned here (VPN-GW provider-integration tasks).
fn bring(svc: &VpnService, t: &TunnelDef, up: bool) -> (bool, String) {
    if !svc.spawn {
        return (false, "spawn disabled".into());
    }
    let argv = match t.method {
        Method::Wg => vpn::wg_quick_argv(t, up),
        Method::Ovpn => {
            if !up {
                // OpenVPN down = kill the daemon for this dev (best-effort).
                vec!["pkill".into(), "-f".into(), format!("--dev {}", t.ifname())]
            } else {
                // The decrypted .ovpn lands here once the secret store (VPN-GW-3)
                // distributes it; honest until then.
                let cfg = format!("/etc/openvpn/client/{}.ovpn", t.ifname());
                if !std::path::Path::new(&cfg).exists() {
                    return (
                        false,
                        format!("openvpn config missing: {cfg} (secret distribution pending)"),
                    );
                }
                vpn::openvpn_argv(t, &cfg)
            }
        }
        Method::Cli | Method::Api => {
            return (
                false,
                format!("method {:?} not yet process-driven", t.method),
            );
        }
    };
    let (cmd, rest) = argv.split_first().expect("argv non-empty");
    match std::process::Command::new(cmd).args(rest).status() {
        Ok(s) if s.success() => (true, format!("{} {}", cmd, if up { "up" } else { "down" })),
        Ok(s) => (false, format!("{cmd} exited {:?}", s.code())),
        Err(e) => (false, format!("{cmd} not run: {e}")),
    }
}

/// Run the VPN Bus responder loop until `should_stop`.
pub fn serve_bus<F: Fn() -> bool>(persist: &Persist, svc: &VpnService, should_stop: F) {
    let mut cursors: HashMap<String, String> = HashMap::new();
    while !should_stop() {
        poll_once(persist, svc, &mut cursors);
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// One poll sweep across the verbs (split out for tests).
pub fn poll_once(persist: &Persist, svc: &VpnService, cursors: &mut HashMap<String, String>) {
    for verb in ACTION_VERBS {
        let topic = action_topic(verb);
        let since = cursors.get(&topic).map(String::as_str);
        let msgs = match persist.list_since(&topic, since) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(topic = %topic, error = %e, "vpn responder: list_since failed");
                continue;
            }
        };
        for msg in msgs {
            cursors.insert(topic.clone(), msg.ulid.clone());
            let reply = if crate::ipc::body_within_cap(msg.body.as_deref()) {
                build_reply(svc, verb, msg.body.as_deref())
            } else {
                crate::ipc::body_too_large_reply(verb)
            };
            if let Err(e) = persist.write(
                &reply_topic(&msg.ulid),
                Priority::Default,
                None,
                Some(&reply),
            ) {
                tracing::warn!(ulid = %msg.ulid, error = %e, "vpn responder: reply write failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svc() -> (tempfile::TempDir, VpnService) {
        let tmp = tempfile::tempdir().unwrap();
        // Pin a deterministic mesh key so the secret verbs don't depend on the
        // build host's env (it may have MDE_BACKUP_PASSPHRASE set).
        let s = VpnService::new(tmp.path().to_path_buf())
            .without_spawn()
            .with_mesh_key(Some("test-mesh-key".into()));
        (tmp, s)
    }

    fn add(s: &VpnService, id: &str, method: &str) -> String {
        build_reply(
            s,
            "add-tunnel",
            Some(&json!({"id":id,"provider":"generic-wg","method":method}).to_string()),
        )
    }

    #[test]
    fn topic_and_verbs_lock() {
        assert_eq!(action_topic("tunnel-up"), "action/vpn/tunnel-up");
        assert_eq!(ACTION_VERBS.len(), 15);
        assert!(ACTION_VERBS.contains(&"set-secret"));
        assert!(ACTION_VERBS.contains(&"secret-status"));
        // VPN-GW-4 routing surface.
        assert!(ACTION_VERBS.contains(&"set-route"));
        assert!(ACTION_VERBS.contains(&"clear-route"));
        assert!(ACTION_VERBS.contains(&"list-routes"));
        assert!(ACTION_VERBS.contains(&"route-status"));
        // VPN-GW-6 health surface.
        assert!(ACTION_VERBS.contains(&"tunnel-health"));
        // VPN-GW-5 provider setup + config import.
        assert!(ACTION_VERBS.contains(&"add-from-provider"));
        assert!(ACTION_VERBS.contains(&"import-config"));
    }

    #[test]
    fn add_list_remove_round_trip() {
        let (_t, s) = svc();
        assert!(add(&s, "mullvad1", "wg").contains("\"ok\":true"));
        let list = build_reply(&s, "list-tunnels", None);
        assert!(list.contains("mullvad1"), "{list}");
        // Remove.
        let r = build_reply(&s, "remove-tunnel", Some("mullvad1"));
        assert!(r.contains("\"ok\":true"), "{r}");
        assert!(!build_reply(&s, "list-tunnels", None).contains("mullvad1"));
        // Removing a ghost errors.
        assert!(build_reply(&s, "remove-tunnel", Some("ghost")).contains("no such tunnel"));
    }

    #[test]
    fn add_rejects_bad_id() {
        let (_t, s) = svc();
        assert!(add(&s, "___", "wg").contains("error")); // no alnum → no ifname
    }

    #[test]
    fn status_reports_down_without_iface() {
        let (_t, s) = svc();
        let _ = add(&s, "mullvad1", "wg");
        let r = build_reply(&s, "tunnel-status", Some("mullvad1"));
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(true));
        assert_eq!(v["ifname"], "mvpn-mullvad1");
        assert_eq!(v["up"], serde_json::Value::Bool(false));
    }

    #[test]
    fn up_without_spawn_is_honest_not_a_panic() {
        let (_t, s) = svc();
        let _ = add(&s, "mullvad1", "wg");
        let r = build_reply(&s, "tunnel-up", Some("mullvad1"));
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(false)); // spawn disabled
        assert_eq!(v["ifname"], "mvpn-mullvad1");
    }

    #[test]
    fn unknown_verb_and_missing_id_error() {
        let (_t, s) = svc();
        assert!(build_reply(&s, "bogus", None).contains("unknown vpn verb"));
        assert!(build_reply(&s, "tunnel-up", None).contains("missing tunnel id"));
    }

    // ── VPN-GW-2 — set-secret / secret-status ───────────────────────────────

    #[test]
    fn set_secret_seals_blob_and_records_creds_ref() {
        let (tmp, s) = svc();
        let _ = add(&s, "mullvad1", "wg");
        let body = json!({
            "tunnel_id": "mullvad1",
            "node_id": "peer:gw",
            "secret": { "wg_conf": "[Interface]\nPrivateKey=SECRET\n[Peer]\n" },
        })
        .to_string();
        let r = build_reply(&s, "set-secret", Some(&body));
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(true), "{r}");
        // The blob landed at the node-scoped path + is real ciphertext (magic),
        // not the cleartext.
        let blob_path = vpn::secret_path(tmp.path(), "peer:gw", "mullvad1");
        let blob = std::fs::read(&blob_path).unwrap();
        assert_eq!(&blob[..4], b"MVPS");
        assert!(!blob.windows(6).any(|w| w == b"SECRET"));
        // creds_ref recorded on the def — log-safe handle, never the material.
        let cfg = vpn::load(tmp.path());
        assert_eq!(
            cfg.get("mullvad1").unwrap().creds_ref,
            "secret://vpn/mullvad1"
        );
        // tunnels.toml never contains the cleartext key.
        let toml = std::fs::read_to_string(vpn::config_path(tmp.path())).unwrap();
        assert!(
            !toml.contains("SECRET"),
            "cleartext leaked into tunnels.toml"
        );
    }

    #[test]
    fn set_secret_rejects_empty_payload_for_method() {
        let (_t, s) = svc();
        let _ = add(&s, "mullvad1", "wg");
        // A WG tunnel with an OpenVPN-only payload → empty wg_conf → rejected.
        let body = json!({
            "tunnel_id": "mullvad1",
            "node_id": "peer:gw",
            "secret": { "ovpn_conf": "client\n" },
        })
        .to_string();
        assert!(build_reply(&s, "set-secret", Some(&body)).contains("empty/mismatched"));
    }

    #[test]
    fn set_secret_without_mesh_key_is_honest() {
        let tmp = tempfile::tempdir().unwrap();
        let s = VpnService::new(tmp.path().to_path_buf())
            .without_spawn()
            .with_mesh_key(None);
        let _ = add(&s, "mullvad1", "wg");
        let body = json!({
            "tunnel_id": "mullvad1", "node_id": "peer:gw",
            "secret": { "wg_conf": "[Interface]\n" },
        })
        .to_string();
        assert!(build_reply(&s, "set-secret", Some(&body)).contains("no mesh key"));
    }

    #[test]
    fn set_secret_unknown_tunnel_errors() {
        let (_t, s) = svc();
        let body = json!({
            "tunnel_id": "ghost", "node_id": "peer:gw",
            "secret": { "wg_conf": "[Interface]\n" },
        })
        .to_string();
        assert!(build_reply(&s, "set-secret", Some(&body)).contains("no such tunnel"));
    }

    #[test]
    fn secret_status_reports_presence_without_revealing() {
        let (_t, s) = svc();
        let _ = add(&s, "mullvad1", "wg");
        let ref_body = json!({ "tunnel_id": "mullvad1", "node_id": "peer:gw" }).to_string();
        // Before set: absent.
        let r0 = build_reply(&s, "secret-status", Some(&ref_body));
        let v0: serde_json::Value = serde_json::from_str(&r0).unwrap();
        assert_eq!(v0["present"], serde_json::Value::Bool(false));
        // After set: present, with a byte count but no material.
        let set_body = json!({
            "tunnel_id": "mullvad1", "node_id": "peer:gw",
            "secret": { "wg_conf": "[Interface]\nPrivateKey=SECRET\n" },
        })
        .to_string();
        assert!(build_reply(&s, "set-secret", Some(&set_body)).contains("\"ok\":true"));
        let r1 = build_reply(&s, "secret-status", Some(&ref_body));
        assert!(!r1.contains("SECRET"));
        let v1: serde_json::Value = serde_json::from_str(&r1).unwrap();
        assert_eq!(v1["present"], serde_json::Value::Bool(true));
        assert!(v1["bytes"].as_u64().unwrap() > 0);
    }

    #[test]
    fn remove_tunnel_rotates_the_secret() {
        let (tmp, s) = svc();
        let _ = add(&s, "mullvad1", "wg");
        let set_body = json!({
            "tunnel_id": "mullvad1", "node_id": "peer:gw",
            "secret": { "wg_conf": "[Interface]\nPrivateKey=SECRET\n" },
        })
        .to_string();
        assert!(build_reply(&s, "set-secret", Some(&set_body)).contains("\"ok\":true"));
        let blob_path = vpn::secret_path(tmp.path(), "peer:gw", "mullvad1");
        assert!(blob_path.exists());
        // Removing the tunnel purges the blob (rotate-on-delete).
        assert!(build_reply(&s, "remove-tunnel", Some("mullvad1")).contains("\"ok\":true"));
        assert!(!blob_path.exists(), "secret blob survived tunnel removal");
    }

    // ── VPN-GW-4 — set-route / clear-route / list-routes / route-status ──────

    #[test]
    fn set_route_persists_and_list_routes_round_trips() {
        let (_t, s) = svc();
        let body = json!({
            "scope": { "kind": "node", "id": "peer:anvil" },
            "gateway": "peer:gw",
            "chain": ["mullvad1", "proton2"],
            "kill_switch": true,
        })
        .to_string();
        let r = build_reply(&s, "set-route", Some(&body));
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(true), "{r}");
        assert_eq!(v["scope"], "node:peer:anvil");
        // list-routes returns the persisted assignment.
        let list = build_reply(&s, "list-routes", None);
        let lv: serde_json::Value = serde_json::from_str(&list).unwrap();
        assert_eq!(lv["routes"].as_array().unwrap().len(), 1);
        assert_eq!(lv["routes"][0]["gateway"], "peer:gw");
        assert_eq!(lv["routes"][0]["chain"][0], "mullvad1");
    }

    #[test]
    fn set_route_rejects_empty_chain() {
        let (_t, s) = svc();
        let body = json!({
            "scope": { "kind": "any-mesh" },
            "gateway": "peer:gw",
            "chain": [],
        })
        .to_string();
        assert!(build_reply(&s, "set-route", Some(&body)).contains("chain is empty"));
    }

    #[test]
    fn set_route_replaces_per_scope() {
        let (_t, s) = svc();
        let any = |gw: &str| {
            json!({ "scope": { "kind": "any-mesh" }, "gateway": gw, "chain": ["t"] }).to_string()
        };
        assert!(build_reply(&s, "set-route", Some(&any("peer:gw1"))).contains("\"ok\":true"));
        assert!(build_reply(&s, "set-route", Some(&any("peer:gw2"))).contains("\"ok\":true"));
        let lv: serde_json::Value =
            serde_json::from_str(&build_reply(&s, "list-routes", None)).unwrap();
        // One assignment per scope — the second replaced the first.
        assert_eq!(lv["routes"].as_array().unwrap().len(), 1);
        assert_eq!(lv["routes"][0]["gateway"], "peer:gw2");
    }

    #[test]
    fn clear_route_drops_a_scope_and_errors_on_a_ghost() {
        let (_t, s) = svc();
        let body = json!({ "scope": { "kind": "any-mesh" }, "gateway": "peer:gw", "chain": ["t"] })
            .to_string();
        assert!(build_reply(&s, "set-route", Some(&body)).contains("\"ok\":true"));
        assert!(build_reply(&s, "clear-route", Some("any")).contains("\"ok\":true"));
        let lv: serde_json::Value =
            serde_json::from_str(&build_reply(&s, "list-routes", None)).unwrap();
        assert!(lv["routes"].as_array().unwrap().is_empty());
        // Clearing a ghost scope errors honestly.
        assert!(build_reply(&s, "clear-route", Some("any")).contains("no route for scope"));
    }

    #[test]
    fn route_status_reports_chain_and_active_tunnel() {
        let (_t, s) = svc();
        // No interfaces are up under `without_spawn` → the selector returns no
        // active tunnel (all down), exercising the failover/kill-switch path.
        let _ = add(&s, "mullvad1", "wg");
        let body = json!({
            "scope": { "kind": "any-mesh" },
            "gateway": "peer:gw",
            "chain": ["mullvad1", "proton2"],
            "kill_switch": true,
        })
        .to_string();
        assert!(build_reply(&s, "set-route", Some(&body)).contains("\"ok\":true"));
        let st = build_reply(&s, "route-status", None);
        let v: serde_json::Value = serde_json::from_str(&st).unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(true));
        let routes = v["routes"].as_array().unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0]["scope"], "any");
        assert_eq!(routes[0]["chain"][0], "mullvad1");
        assert_eq!(routes[0]["kill_switch"], serde_json::Value::Bool(true));
        // Spawn disabled → every tunnel reads down → no active tunnel.
        assert_eq!(routes[0]["active"], serde_json::Value::Null);
        // VPN-GW-6 — route-status carries a per-chain-tunnel health array even
        // before any worker has published (defaulting each to "down").
        let h = routes[0]["health"].as_array().unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(h[0]["tunnel"], "mullvad1");
        assert_eq!(h[0]["verdict"], "down");
    }

    // ── VPN-GW-6 — tunnel-health read + route-status health enrichment ───────

    #[test]
    fn tunnel_health_reads_published_worker_state() {
        let (tmp, s) = svc();
        // Simulate the worker publishing health: a healthy tunnel + a leaking one.
        let mut state = crate::workers::vpn_gateway::HealthState::default();
        state.tunnel.insert(
            "mullvad1".into(),
            mackes_mesh_types::vpn::TunnelHealth::from_probes(
                "mullvad1",
                true,
                Some("185.65.1.1".into()),
                true,
                false,
            ),
        );
        state.tunnel.insert(
            "proton2".into(),
            mackes_mesh_types::vpn::TunnelHealth::from_probes(
                "proton2",
                true,
                Some("9.9.9.9".into()),
                false, // exit IP == WAN → leaking
                false,
            ),
        );
        state
            .store(&crate::workers::vpn_gateway::default_health_path(
                tmp.path(),
            ))
            .unwrap();

        let r = build_reply(&s, "tunnel-health", None);
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(true));
        let tunnels = v["tunnels"].as_array().unwrap();
        assert_eq!(tunnels.len(), 2);
        // BTreeMap order: mullvad1 then proton2.
        assert_eq!(tunnels[0]["tunnel"], "mullvad1");
        assert_eq!(tunnels[0]["verdict"], "healthy");
        assert_eq!(tunnels[0]["exit_ip"], "185.65.1.1");
        assert_eq!(tunnels[1]["tunnel"], "proton2");
        assert_eq!(tunnels[1]["verdict"], "leaking");
    }

    #[test]
    fn route_status_uses_published_health_for_active_pick() {
        let (tmp, s) = svc();
        let _ = add(&s, "primary", "wg");
        let _ = add(&s, "fallback", "wg");
        let body = json!({
            "scope": { "kind": "any-mesh" },
            "gateway": "peer:gw",
            "chain": ["primary", "fallback"],
            "kill_switch": true,
        })
        .to_string();
        assert!(build_reply(&s, "set-route", Some(&body)).contains("\"ok\":true"));
        // Publish health: primary leaking, fallback healthy → active = fallback.
        let mut state = crate::workers::vpn_gateway::HealthState::default();
        state.tunnel.insert(
            "primary".into(),
            mackes_mesh_types::vpn::TunnelHealth::from_probes(
                "primary",
                true,
                Some("9.9.9.9".into()),
                false,
                false,
            ),
        );
        state.tunnel.insert(
            "fallback".into(),
            mackes_mesh_types::vpn::TunnelHealth::from_probes(
                "fallback",
                true,
                Some("185.65.1.1".into()),
                true,
                false,
            ),
        );
        state
            .store(&crate::workers::vpn_gateway::default_health_path(
                tmp.path(),
            ))
            .unwrap();

        let st = build_reply(&s, "route-status", None);
        let v: serde_json::Value = serde_json::from_str(&st).unwrap();
        // The active pick comes from the HEALTH verdict (fallback), not the
        // interface check (which would say all-down under without_spawn).
        assert_eq!(v["routes"][0]["active"], "fallback");
        assert_eq!(v["routes"][0]["health"][0]["verdict"], "leaking");
        assert_eq!(v["routes"][0]["health"][1]["verdict"], "healthy");
        assert_eq!(v["routes"][0]["health"][1]["exit_ip"], "185.65.1.1");
    }

    // ── VPN-GW-5 — add-from-provider / import-config ────────────────────────

    #[test]
    fn add_from_provider_seals_secret_records_def_and_never_leaks() {
        let (tmp, s) = svc();
        // A provider with a statically-known peer (no live API call needed).
        let body = json!({
            "provider": "mullvad",
            "tunnel_id": "exit1",
            "node_id": "peer:gw",
            "server": "us-nyc-wg-001",
            "account_token": "SECRET-MULLVAD-ACCOUNT",
            "server_pubkey": "SERVERpubkey00000000000000000000000000000000=",
            "endpoint": "198.51.100.10:51820",
            "assigned_address": "10.64.0.2/32",
        })
        .to_string();
        let r = build_reply(&s, "add-from-provider", Some(&body));
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(true), "{r}");
        assert_eq!(v["provider"], "mullvad");
        assert_eq!(v["tunnel_id"], "exit1");
        assert_eq!(v["has_provider_api"], serde_json::Value::Bool(true));
        // The PUBLIC key is returned (publishable); 32-byte WG b64.
        let pub_b64 = v["public_key"].as_str().unwrap();
        assert_eq!(pub_b64.len(), 44);
        // The account token + private key NEVER appear in the reply.
        assert!(
            !r.contains("SECRET-MULLVAD-ACCOUNT"),
            "token leaked in reply"
        );
        // The def is recorded with a log-safe creds_ref, as a WG tunnel.
        let cfg = vpn::load(tmp.path());
        let def = cfg.get("exit1").expect("def recorded");
        assert_eq!(def.method, Method::Wg);
        assert_eq!(def.provider, "mullvad");
        assert_eq!(def.creds_ref, "secret://vpn/exit1");
        // tunnels.toml carries NO key material + NO token.
        let toml = std::fs::read_to_string(vpn::config_path(tmp.path())).unwrap();
        assert!(!toml.contains("SECRET-MULLVAD-ACCOUNT"));
        assert!(!toml.contains("PrivateKey"));
        // The sealed blob is real ciphertext (MVPS magic), not the cleartext key.
        let blob = std::fs::read(vpn::secret_path(tmp.path(), "peer:gw", "exit1")).unwrap();
        assert_eq!(&blob[..4], b"MVPS");
        assert!(!blob.windows(7).any(|w| w == b"Private"));
        // Round-trip: the sealed secret unseals to a populated WG config.
        let secret = crate::vpn_secret::unseal("test-mesh-key", &blob).unwrap();
        assert!(secret.wg_conf.contains("PublicKey = SERVERpubkey"));
        assert!(secret.wg_conf.contains("Endpoint = 198.51.100.10:51820"));
        // The conf holds the [Interface] PRIVATE key — not the public one we
        // returned to the caller (the public key is the publishable handle).
        assert!(!secret.wg_conf.contains(pub_b64));
    }

    #[test]
    fn add_from_provider_is_honest_about_bad_input() {
        let (_t, s) = svc();
        // Unknown provider.
        let bad = json!({"provider":"acme","tunnel_id":"x","node_id":"n"}).to_string();
        assert!(build_reply(&s, "add-from-provider", Some(&bad)).contains("unknown provider"));
        // Known provider but no peer → loud error (not a silent fake tunnel).
        let no_peer = json!({"provider":"ivpn","tunnel_id":"x","node_id":"n"}).to_string();
        assert!(build_reply(&s, "add-from-provider", Some(&no_peer)).contains("server public key"));
        // No mesh key → honest, no keypair minted.
        let nokey = VpnService::new(tempfile::tempdir().unwrap().path().to_path_buf())
            .without_spawn()
            .with_mesh_key(None);
        let full = json!({
            "provider":"mullvad","tunnel_id":"x","node_id":"n",
            "server_pubkey":"p=","endpoint":"a:1","assigned_address":"10.0.0.2/32",
        })
        .to_string();
        assert!(build_reply(&nokey, "add-from-provider", Some(&full)).contains("no mesh key"));
    }

    #[test]
    fn import_config_wg_seals_pasted_conf() {
        let (tmp, s) = svc();
        let conf = "[Interface]\nPrivateKey = aAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaA0=\n\
                    Address = 10.0.0.2/32\nDNS = 1.1.1.1\n[Peer]\n\
                    PublicKey = bBbBbBbBbBbBbBbBbBbBbBbBbBbBbBbBbBbBbBbBbB0=\n\
                    Endpoint = 203.0.113.7:51820\nAllowedIPs = 0.0.0.0/0\n";
        let body = json!({
            "kind": "wg", "tunnel_id": "pasted1", "node_id": "peer:gw", "config": conf,
        })
        .to_string();
        let r = build_reply(&s, "import-config", Some(&body));
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(true), "{r}");
        assert_eq!(v["method"], "wg");
        // No key material in the reply or tunnels.toml.
        assert!(!r.contains("aAaAaAa"), "private key leaked in reply");
        let cfg = vpn::load(tmp.path());
        assert_eq!(cfg.get("pasted1").unwrap().provider, "generic-wg");
        let toml = std::fs::read_to_string(vpn::config_path(tmp.path())).unwrap();
        assert!(!toml.contains("aAaAaAa"));
        // Sealed blob round-trips to the pasted conf.
        let blob = std::fs::read(vpn::secret_path(tmp.path(), "peer:gw", "pasted1")).unwrap();
        let secret = crate::vpn_secret::unseal("test-mesh-key", &blob).unwrap();
        assert!(secret.wg_conf.contains("PrivateKey = aAaAaAa"));
    }

    #[test]
    fn import_config_ovpn_seals_imported_config() {
        let (tmp, s) = svc();
        let ovpn = "client\nproto udp\nremote vpn.example.com 1194\n\
                    <key>\n-----BEGIN PRIVATE KEY-----\nSECRETKEYMATERIAL\n-----END PRIVATE KEY-----\n</key>\n";
        let body = json!({
            "kind": "ovpn", "tunnel_id": "ovpn1", "node_id": "peer:gw", "config": ovpn,
        })
        .to_string();
        let r = build_reply(&s, "import-config", Some(&body));
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(true), "{r}");
        assert_eq!(v["method"], "ovpn");
        assert!(
            !r.contains("SECRETKEYMATERIAL"),
            "key material leaked in reply"
        );
        let cfg = vpn::load(tmp.path());
        let def = cfg.get("ovpn1").unwrap();
        assert_eq!(def.method, Method::Ovpn);
        assert_eq!(def.server, "vpn.example.com:1194");
        // tunnels.toml never carries the inline key.
        let toml = std::fs::read_to_string(vpn::config_path(tmp.path())).unwrap();
        assert!(!toml.contains("SECRETKEYMATERIAL"));
        // Sealed blob round-trips to the imported .ovpn.
        let blob = std::fs::read(vpn::secret_path(tmp.path(), "peer:gw", "ovpn1")).unwrap();
        let secret = crate::vpn_secret::unseal("test-mesh-key", &blob).unwrap();
        assert!(secret.ovpn_conf.contains("SECRETKEYMATERIAL"));
    }

    // ── DDNS-EGRESS-3 — auto-population on tunnel/route create ───────────────

    #[test]
    fn add_from_provider_auto_adds_a_ddns_record() {
        let (tmp, s) = svc();
        let body = json!({
            "provider": "mullvad",
            "tunnel_id": "exit1",
            "node_id": "peer:gw",
            "server_pubkey": "SERVERpubkey00000000000000000000000000000000=",
            "endpoint": "198.51.100.10:51820",
            "assigned_address": "10.64.0.2/32",
        })
        .to_string();
        assert!(build_reply(&s, "add-from-provider", Some(&body)).contains("\"ok\":true"));
        // The [ddns] config gained a templated record for tunnel:exit1.
        let cfg = mackes_mesh_types::ddns::load(tmp.path());
        assert_eq!(cfg.record.len(), 1);
        assert_eq!(cfg.record[0].source, "tunnel:exit1");
        assert_eq!(
            cfg.record[0].name,
            mackes_mesh_types::ddns::AUTO_RECORD_NAME_TEMPLATE
        );
    }

    #[test]
    fn auto_add_ddns_record_is_idempotent_across_recreate() {
        let (tmp, s) = svc();
        let body = json!({
            "kind": "wg", "tunnel_id": "pasted1", "node_id": "peer:gw",
            "config": "[Interface]\nPrivateKey = aAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaAaA0=\n\
                       Address = 10.0.0.2/32\n[Peer]\n\
                       PublicKey = bBbBbBbBbBbBbBbBbBbBbBbBbBbBbBbBbBbBbBbBbB0=\n\
                       Endpoint = 203.0.113.7:51820\nAllowedIPs = 0.0.0.0/0\n",
        })
        .to_string();
        // Import the same tunnel twice (re-create) → exactly one DDNS record.
        assert!(build_reply(&s, "import-config", Some(&body)).contains("\"ok\":true"));
        assert!(build_reply(&s, "import-config", Some(&body)).contains("\"ok\":true"));
        let cfg = mackes_mesh_types::ddns::load(tmp.path());
        assert_eq!(
            cfg.record.len(),
            1,
            "re-create must not duplicate the record"
        );
        assert_eq!(cfg.record[0].source, "tunnel:pasted1");
    }

    #[test]
    fn set_route_auto_adds_a_record_per_chain_tunnel() {
        let (tmp, s) = svc();
        let body = json!({
            "scope": { "kind": "any-mesh" },
            "gateway": "peer:gw",
            "chain": ["mullvad1", "proton2"],
            "kill_switch": true,
        })
        .to_string();
        assert!(build_reply(&s, "set-route", Some(&body)).contains("\"ok\":true"));
        let cfg = mackes_mesh_types::ddns::load(tmp.path());
        let sources: Vec<&str> = cfg.record.iter().map(|r| r.source.as_str()).collect();
        assert_eq!(cfg.record.len(), 2);
        assert!(sources.contains(&"tunnel:mullvad1"));
        assert!(sources.contains(&"tunnel:proton2"));
        // Re-setting the same route does not duplicate.
        assert!(build_reply(&s, "set-route", Some(&body)).contains("\"ok\":true"));
        assert_eq!(mackes_mesh_types::ddns::load(tmp.path()).record.len(), 2);
    }

    #[test]
    fn import_config_rejects_unknown_kind_and_malformed() {
        let (_t, s) = svc();
        let bad_kind =
            json!({"kind":"pptp","tunnel_id":"x","node_id":"n","config":"x"}).to_string();
        assert!(build_reply(&s, "import-config", Some(&bad_kind)).contains("unknown kind"));
        let malformed =
            json!({"kind":"wg","tunnel_id":"x","node_id":"n","config":"garbage"}).to_string();
        assert!(build_reply(&s, "import-config", Some(&malformed)).contains("error"));
    }
}
