//! VPN-GW-1 (responder) — `action/vpn/*` over the tunnel config + `wg-quick`/
//! `openvpn` (design: `docs/design/vpn-gateway.md`).
//!
//! CRUD on the per-node [`mackes_mesh_types::vpn::VpnConfig`] (TOML on the shared
//! substrate) + best-effort bring-up/down via the pure argv builders.
//!
//! VPN-GW-2 — secret distribution: on `setup-provider` a tunnel's secret material
//! (the rendered `.conf`/`.ovpn`, which carries the private key) is age-encrypted
//! into the replicated secret store (`crate::ipc::secret_store`) keyed by
//! `creds_ref`; on `tunnel-up` every enrolled node resolves `creds_ref`, reads +
//! decrypts the secret, and materializes `/etc/wireguard/<ifname>.conf` (or the
//! `.ovpn`) where `wg-quick`/`openvpn` reads it — then spawns. Honest until the
//! secret is distributed ("secret distribution pending"), never a fake success.
//!
//! VPN-GW-3 — selective egress: after a successful tunnel-up [`bring`] applies
//! the [`EgressPlan`] (fwmark/ip-rule policy routing + an nftables masquerade,
//! Nebula overlay carved out so mesh never tunnels) and clears the kill-switch;
//! on tunnel-down — and on the down/failure path — it installs the kill-switch
//! drop (leak-proof) and tears the egress rules back down.
//!
//! Same dedicated-OS-thread shape as the Connect/Route responders.

use std::collections::HashMap;
use std::path::PathBuf;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;
use serde_json::json;

use mackes_mesh_types::vpn::{self, Method, TunnelDef};
use mackes_mesh_types::vpn_egress::{self, EgressPlan, EgressRoute};
use mackes_mesh_types::vpn_providers::{
    self, AdapterError, ProducedTunnel, Provider, SecretKind, WgSetup,
};

use crate::ipc::secret_store::{self, SecretStore};

/// The VPN responder — rooted at the shared workgroup root (the config home).
#[derive(Debug, Clone)]
pub struct VpnService {
    workgroup_root: PathBuf,
    /// `wg-quick`/`openvpn`/`ip` binaries are spawned by default; tests set the
    /// flag false to exercise the pure CRUD without the system tools.
    spawn: bool,
    /// VPN-GW-2 — the secret store used to distribute/resolve tunnel secrets.
    /// `None` selects the runtime default ([`SecretStore::resolve`]) lazily; a
    /// test injects a `LocalAead` store over a tempdir so the secret-distribution
    /// path runs with real crypto and no etcd/age CLI.
    store: Option<SecretStore>,
}

impl VpnService {
    /// Build the service rooted at the shared workgroup root.
    #[must_use]
    pub const fn new(workgroup_root: PathBuf) -> Self {
        Self {
            workgroup_root,
            spawn: true,
            store: None,
        }
    }

    /// Disable the tool shell-out (tests).
    #[must_use]
    pub fn without_spawn(mut self) -> Self {
        self.spawn = false;
        self
    }

    /// Inject the secret store (tests). Production resolves it lazily from the
    /// deployed repo root + workgroup root via [`VpnService::secret_store`].
    #[must_use]
    pub fn with_store(mut self, store: SecretStore) -> Self {
        self.store = Some(store);
        self
    }

    /// The secret store for this node: the injected one, else the runtime
    /// default (the mesh `age`+etcd store when its helper is found under the
    /// deployed repo root, else the local-AEAD fallback under the workgroup
    /// root). Anchored on [`secret_store::repo_root`] (`MCNF_REPO`), NOT the
    /// process cwd — the daemon's systemd unit runs with cwd `/`.
    fn secret_store(&self) -> SecretStore {
        if let Some(s) = &self.store {
            return s.clone();
        }
        SecretStore::resolve(&secret_store::repo_root(), &self.workgroup_root)
    }
}

/// Action verbs served on `action/vpn/<verb>`.
pub const ACTION_VERBS: [&str; 14] = [
    "list-tunnels",
    "add-tunnel",
    "remove-tunnel",
    "tunnel-up",
    "tunnel-down",
    "tunnel-status",
    // VPN-GW-5 — provider adapters (5 named + generic WG paste / .ovpn import).
    "list-providers",
    "setup-provider",
    // VPN-GW-4 — mesh egress routing (per-node / group / ANY) + failover chain.
    "set-route",
    "clear-route",
    "list-routes",
    "route-status",
    // VPN-GW-6 — health + exit-IP/leak verification + auto-failover + alerts.
    "verify-egress",
    "egress-health",
];

/// Where a tunnel's DECRYPTED config lands on the node just before bring-up, for
/// the bring-up tool to read. Materialized by [`materialize_secret`] from the
/// age-encrypted secret store (VPN-GW-2). `WireGuard` configs go to the `wg-quick`
/// config dir; `.ovpn` to the `openvpn` client dir.
#[must_use]
fn secret_path(kind: SecretKind, ifname: &str) -> PathBuf {
    match kind {
        SecretKind::WgQuick => PathBuf::from(format!("/etc/wireguard/{ifname}.conf")),
        SecretKind::Ovpn => PathBuf::from(format!("/etc/openvpn/client/{ifname}.ovpn")),
    }
}

/// VPN-GW-2 — materialize a tunnel's decrypted config where the bring-up tool
/// reads it (`/etc/wireguard/<ifname>.conf` or `/etc/openvpn/client/<ifname>.ovpn`),
/// by resolving its secret from the age-encrypted store and writing it 0600.
///
/// Resolution order for the store key:
///   1. `t.creds_ref` if the durable def carries one (set when the tunnel was set
///      up), else
///   2. the derived `vpn/<ifname>` key (a tunnel added before distribution, or
///      one whose def predates `creds_ref` being populated).
///
/// Returns `Ok(())` once the config is on disk; an `Err(detail)` carrying an
/// HONEST reason otherwise:
///   * "secret distribution pending" when the secret simply isn't in the store
///     yet (it wasn't distributed / this node hasn't synced it),
///   * a store / decrypt / write error string for a real fault.
fn materialize_secret(svc: &VpnService, t: &TunnelDef, kind: SecretKind) -> Result<(), String> {
    let ifname = t.ifname();
    let store = svc.secret_store();
    let key = if t.creds_ref.trim().is_empty() {
        secret_store::creds_ref_for(&ifname)
    } else {
        t.creds_ref.trim().to_string()
    };
    let body = match store.get(&key) {
        Ok(Some(body)) => body,
        Ok(None) => {
            return Err(format!(
                "{} config missing: secret '{key}' not in store (secret distribution pending)",
                match kind {
                    SecretKind::WgQuick => "wireguard",
                    SecretKind::Ovpn => "openvpn",
                }
            ));
        }
        Err(e) => return Err(format!("secret store read '{key}': {e}")),
    };
    let path = secret_path(kind, &ifname);
    write_secret(&path, &body).map_err(|e| format!("write {}: {e}", path.display()))
}

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

/// Liveness check shared with the VPN-GW-6 health module so the "is the tunnel's
/// interface present" rule lives in exactly one place (the `mvpn-<id>` ifname
/// derivation already does). Re-exports the private [`iface_up`].
#[must_use]
pub fn iface_up_public(spawn: bool, ifname: &str) -> bool {
    iface_up(spawn, ifname)
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
            // DDNS-EGRESS-3 — auto-populate a templated DDNS record for the new
            // tunnel (no-op unless DDNS is enabled), so a created tunnel
            // immediately publishes its exit IP under the zone.
            let ddns_added = auto_add_ddns_record(root, &t.id);
            cfg.upsert(t);
            match vpn::save(root, &cfg) {
                Ok(_) => json!({ "ok": true, "ddns_record": ddns_added }).to_string(),
                Err(e) => err(format!("add-tunnel: save: {e}")),
            }
        }
        "remove-tunnel" => {
            let Some(id) = req_body.map(str::trim).filter(|s| !s.is_empty()) else {
                return err("remove-tunnel: missing tunnel id".into());
            };
            let mut cfg = vpn::load(root);
            if !cfg.remove(id) {
                return err(format!("remove-tunnel: no such tunnel '{id}'"));
            }
            // Best-effort: bring it down, then clear its WHOLE egress footprint
            // (rules + kill-switch) so a forgotten tunnel leaves no orphan DROP
            // pinned to a mark that a surviving tunnel could later be assigned.
            if let Some(t) = vpn::load(root).get(id) {
                let _ = bring(svc, t, false);
                if svc.spawn {
                    forget_egress(&egress_plan(svc, t));
                }
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
        "list-providers" => list_providers_reply(),
        "setup-provider" => setup_provider(svc, req_body),
        // VPN-GW-4 — mesh egress routing + failover chain.
        "set-route" => set_route(svc, req_body),
        "clear-route" => clear_route(svc, req_body),
        "list-routes" => list_routes(svc),
        "route-status" => route_status(svc, req_body),
        // VPN-GW-6 — verify one tunnel's exit IP / leak state on demand, or read
        // the full per-tunnel egress-health (with the verified exit IPs).
        "verify-egress" => verify_egress(svc, req_body),
        "egress-health" => egress_health(svc),
        other => err(format!("unknown vpn verb: {other}")),
    }
}

/// Bring a tunnel up/down via the right tool. Returns `(ran_ok, detail)`. Honest
/// when the tool/config is absent — never panics. `Cli`/`Api` methods aren't
/// process-spawned here (VPN-GW provider-integration tasks).
fn bring(svc: &VpnService, t: &TunnelDef, up: bool) -> (bool, String) {
    if !svc.spawn {
        return (false, "spawn disabled".into());
    }
    let ifname = t.ifname();
    let argv = match t.method {
        Method::Wg => {
            if up {
                // VPN-GW-2 — materialize `/etc/wireguard/<ifname>.conf` from the
                // age-encrypted secret store before `wg-quick up` reads it.
                // Honest "secret distribution pending" when it isn't distributed.
                if let Err(detail) = materialize_secret(svc, t, SecretKind::WgQuick) {
                    return (false, detail);
                }
            }
            vpn::wg_quick_argv(t, up)
        }
        Method::Ovpn => {
            if !up {
                // OpenVPN down = kill the daemon for this dev (best-effort).
                vec!["pkill".into(), "-f".into(), format!("--dev {ifname}")]
            } else {
                // VPN-GW-2 — materialize the decrypted `.ovpn` from the secret
                // store, then hand it to `openvpn`. Honest until distributed.
                let cfg = secret_path(SecretKind::Ovpn, &ifname);
                if let Err(detail) = materialize_secret(svc, t, SecretKind::Ovpn) {
                    return (false, detail);
                }
                vpn::openvpn_argv(t, &cfg.to_string_lossy())
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
    let tool = match std::process::Command::new(cmd).args(rest).status() {
        Ok(s) if s.success() => (true, format!("{} {}", cmd, if up { "up" } else { "down" })),
        Ok(s) => (false, format!("{cmd} exited {:?}", s.code())),
        Err(e) => (false, format!("{cmd} not run: {e}")),
    };

    // VPN-GW-3 — selective egress (policy-routing + NAT + kill-switch). Tie the
    // egress rules to the tunnel's lifecycle: a clean up installs the fwmark /
    // ip-rule / masquerade and clears the kill-switch; a down — or a failed
    // up — installs the kill-switch DROP first (leak-proof on flap) and then
    // tears the egress rules down. The overlay subnet is carved out in every
    // case so mesh traffic never tunnels (design §-risk).
    let plan = egress_plan(svc, t);
    let (tool_ok, detail) = tool;
    if up && tool_ok {
        // Tunnel is up: route + NAT the marked traffic out it, then drop the
        // kill-switch so the (now-routable) egress can flow.
        apply_egress(&plan);
    } else {
        // Down, or a failed bring-up: block first (no WAN leak), then unwind.
        engage_kill_switch(&plan);
    }
    // The reported detail stays the tool's own up/down message; the egress
    // rules are best-effort glue layered on top.
    (tool_ok, detail)
}

/// The selective-egress plan for `t`, keyed on its **interface name** so the
/// `fwmark`/routing-table numbers are a stable, distinct-per-tunnel function of
/// the tunnel — not its mutable position in the config. A positional slot would
/// silently remap a live tunnel's mark (and orphan its kill-switch onto another
/// tunnel's mark) whenever a sibling is added or removed; the id-derived slot
/// guarantees the teardown argv always reclaim exactly what the matching
/// bring-up installed. `svc` is unused now but kept so the signature stays the
/// `(svc, t)` shape the responder threads.
fn egress_plan(_svc: &VpnService, t: &TunnelDef) -> EgressPlan {
    EgressPlan::for_ifname_on_default_overlay(&t.ifname())
}

/// Run a batch of argv (one command per inner vec), best-effort. Each command's
/// failure is logged but never aborts the batch — the rules are independent and
/// a partially-present prior state must still converge. `nft`/`ip` not on PATH
/// (a dev box without the tools) is logged, not fatal.
fn run_argv_batch(label: &str, batch: &[Vec<String>]) {
    for cmd in batch {
        let Some((bin, rest)) = cmd.split_first() else {
            continue;
        };
        match std::process::Command::new(bin).args(rest).status() {
            Ok(s) if s.success() => {}
            Ok(s) => tracing::debug!(label, cmd = ?cmd, code = ?s.code(), "egress argv non-zero"),
            Err(e) => tracing::debug!(label, cmd = ?cmd, error = %e, "egress argv not run"),
        }
    }
}

/// Install the selective-egress rules for an up tunnel, then clear the
/// kill-switch so the now-routable marked traffic can flow.
fn apply_egress(plan: &EgressPlan) {
    // Re-applying over a stale state can leave duplicate rules; tear down any
    // prior egress for this tunnel first so `up` is idempotent on a re-up.
    run_argv_batch("egress-reset", &plan.down_argv());
    run_argv_batch("egress-up", &plan.up_argv());
    run_argv_batch("kill-switch-clear", &plan.kill_switch_clear_argv());
}

/// Engage the kill-switch (DROP the marked egress — no WAN leak) and tear the
/// egress routing/NAT rules down. Used on tunnel-down and on a failed bring-up.
/// Ordering is leak-proof: the DROP is installed *before* the egress rules are
/// removed, so there is never a window where marked traffic can escape direct.
fn engage_kill_switch(plan: &EgressPlan) {
    run_argv_batch("kill-switch", &plan.kill_switch_argv());
    run_argv_batch("egress-down", &plan.down_argv());
}

/// Remove ALL of this tunnel's egress footprint — the routing/NAT rules AND the
/// kill-switch DROP — when the tunnel is being forgotten (`remove-tunnel`).
/// Unlike [`engage_kill_switch`], this clears the kill-switch too: a deleted
/// tunnel must leave no orphan DROP behind (its mark is no longer protected, and
/// a lingering DROP rule is dead state on the box).
fn forget_egress(plan: &EgressPlan) {
    run_argv_batch("egress-forget", &plan.down_argv());
    run_argv_batch("kill-switch-forget", &plan.kill_switch_clear_argv());
}

/// The first-class providers + the two generic paths, with the per-provider
/// facts the add-tunnel wizard needs (method, CLI, multi-instance, the WG port).
/// Pure catalog — derived from the [`Provider`] enum.
const PROVIDER_CATALOG: [Provider; 7] = [
    Provider::Mullvad,
    Provider::Proton,
    Provider::Ivpn,
    Provider::Nord,
    Provider::Surfshark,
    Provider::GenericWg,
    Provider::GenericOvpn,
];

/// `list-providers` — the static provider catalog for the add-tunnel wizard.
fn list_providers_reply() -> String {
    let providers: Vec<serde_json::Value> = PROVIDER_CATALOG
        .iter()
        .map(|p| {
            json!({
                "id": p.label(),
                "method": match p.method() {
                    Method::Wg => "wg",
                    Method::Ovpn => "ovpn",
                    Method::Cli => "cli",
                    Method::Api => "api",
                },
                "cli": p.cli(),
                "wg_port": p.default_wg_port(),
                "multi_instance": p.allows_multi_instance(),
                "exit_check": vpn_providers::exit_check_target(*p),
            })
        })
        .collect();
    json!({ "ok": true, "providers": providers }).to_string()
}

/// `setup-provider` — run a provider adapter (VPN-GW-5) end-to-end: build the
/// verifiable tunnel config from the operator's inputs, write the secret
/// material to where the existing bring-up machinery reads it, persist the
/// [`TunnelDef`] into the durable config, and report the produced tunnel + its
/// exit-IP check target. The body is `{provider, id, server?, ...}` where the
/// remaining fields depend on the provider:
///   - WireGuard providers / `generic-wg` (non-paste): a flat [`WgSetup`].
///   - `generic-wg` paste path: `{provider:"generic-wg", id, server?, wg_config}`.
///   - `generic-ovpn`: `{provider:"generic-ovpn", id, server?, ovpn}`.
///
/// Reachable from the already-spawned vpn responder (no new serve registration).
fn setup_provider(svc: &VpnService, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(body) = req_body else {
        return err("setup-provider: missing body".into());
    };
    let v: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(format!("setup-provider: bad json: {e}")),
    };
    let Some(provider_label) = v.get("provider").and_then(serde_json::Value::as_str) else {
        return err("setup-provider: missing 'provider'".into());
    };
    let Some(provider) = Provider::from_label(provider_label) else {
        return err(format!(
            "setup-provider: unknown provider '{provider_label}'"
        ));
    };
    let id = v
        .get("id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let server = v
        .get("server")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    // A usable pasted `wg_config` blob (a non-empty string — not a present-but-
    // null/non-string key) routes through the paste importer for ANY WireGuard
    // provider, so a dashboard-exported `.conf` keeps that provider's label.
    let wg_config = v
        .get("wg_config")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());

    // Dispatch to the right adapter. The generic .ovpn / WG-paste paths take a
    // raw config blob; otherwise a structured WgSetup from flat fields.
    let produced: Result<ProducedTunnel, AdapterError> = if provider == Provider::GenericOvpn {
        match v.get("ovpn").and_then(serde_json::Value::as_str) {
            Some(o) => vpn_providers::import_ovpn(id, server, o),
            None => return err("setup-provider: generic-ovpn needs an 'ovpn' body".into()),
        }
    } else if let Some(conf) = wg_config {
        vpn_providers::import_wg_paste(provider, id, server, conf)
    } else {
        let setup = WgSetup {
            id: id.to_string(),
            private_key: str_field(&v, "private_key"),
            address: str_field(&v, "address"),
            peer_public_key: str_field(&v, "peer_public_key"),
            endpoint: str_field(&v, "endpoint"),
            dns: str_field(&v, "dns"),
            server: server.to_string(),
            preshared_key: str_field(&v, "preshared_key"),
        };
        vpn_providers::build_wg(provider, &setup)
    };

    let mut produced = match produced {
        Ok(p) => p,
        Err(e) => return err(format!("setup-provider: {e}")),
    };

    let ifname = produced.def.ifname();

    // VPN-GW-2 — age-encrypted secret distribution. The secret material (the
    // rendered `.conf`/`.ovpn`, which carries the private key) is age-encrypted
    // into the secret store keyed by `creds_ref`, so every enrolled node resolves
    // it at bring-up — never inlined into the durable def. The `creds_ref` (a
    // deterministic `vpn/<ifname>` key) is set on the def so it round-trips in
    // config regardless of which node holds the plaintext.
    //
    // The node handling this request is the ONLY one with the secret plaintext,
    // so it MUST write the store — gating the write on leadership would simply
    // lose the secret on a non-leader / single-node box (nothing else can ever
    // re-derive it). With the replicated mesh store a redundant write is
    // idempotent, so every enrolled node reads the same `creds_ref`.
    let creds_ref = secret_store::creds_ref_for(&ifname);
    produced.def.creds_ref.clone_from(&creds_ref);

    let mut distributed = false;
    let mut secret_note = String::new();
    match svc.secret_store().put(&creds_ref, &produced.secret) {
        Ok(()) => distributed = true,
        // Honest: the store was unreachable. The def still persists (with its
        // creds_ref); bring-up will report "distribution pending".
        Err(e) => secret_note = format!("secret not distributed: {e}"),
    }

    // Persist the durable def (no secret material, only `creds_ref`) into the
    // node's VPN config.
    let root = svc.workgroup_root.as_path();
    let mut cfg = vpn::load(root);
    // DDNS-EGRESS-3 — auto-populate a templated DDNS record for the provisioned
    // tunnel (no-op unless DDNS is enabled), so the new exit publishes a stable
    // hostname under the zone as soon as the tunnel comes up.
    let ddns_added = auto_add_ddns_record(root, &produced.def.id);
    cfg.upsert(produced.def.clone());
    if let Err(e) = vpn::save(root, &cfg) {
        return err(format!("setup-provider: save tunnel: {e}"));
    }

    json!({
        "ok": true,
        "id": produced.def.id,
        "provider": produced.def.provider,
        "ifname": ifname,
        // DDNS-EGRESS-3: whether a templated DDNS record was auto-added for this
        // tunnel (true only when DDNS is enabled + the record didn't already exist).
        "ddns_record": ddns_added,
        "method": match produced.def.method {
            Method::Wg => "wg",
            Method::Ovpn => "ovpn",
            Method::Cli => "cli",
            Method::Api => "api",
        },
        // VPN-GW-2: the durable creds reference + whether the secret was
        // age-encrypted into the store on this request (so enrolled nodes can
        // read it at bring-up).
        "creds_ref": creds_ref,
        "secret_distributed": distributed,
        "secret_note": secret_note,
        // The daemon-side verifier curls this THROUGH the tunnel to confirm the
        // exit IP is the provider's (live verification needs a real account).
        "exit_check": vpn_providers::exit_check_target(provider),
    })
    .to_string()
}

/// Read a string field from the request body (empty if absent/non-string).
fn str_field(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// DDNS-EGRESS-3 — auto-populate a templated DDNS record for a newly created
/// tunnel `id`. Returns the FQDN-template label that was added, or `None` when no
/// record was added (DDNS disabled, or a record for this tunnel already exists).
///
/// The hook is **opt-in via the DDNS master enable**: it adds a record ONLY when
/// `[ddns].enabled` (so a node not using DDNS never accumulates phantom records),
/// and it is idempotent — re-running `setup-provider`/`add-tunnel` for the same
/// tunnel won't duplicate the record (the `add-record` upsert keys on the name
/// template, which is unique per tunnel id). The templated record's source is
/// `tunnel:<id>` (so the reconcile worker resolves its exit IP from the VPN-GW-6
/// verifier) and its name template is `{node}-<id>` → a stable per-tunnel label
/// under the zone. The reconcile worker (DDNS-EGRESS-3) then publishes it.
fn auto_add_ddns_record(root: &std::path::Path, tunnel_id: &str) -> Option<String> {
    use mackes_mesh_types::ddns::{self, OnDown, RecordDef};
    let mut cfg = ddns::load(root);
    if !cfg.enabled {
        return None;
    }
    // `{node}-<id>` — the node label is templated at publish time; the tunnel id is
    // baked in so the record is unique per tunnel.
    let name = format!("{{node}}-{tunnel_id}");
    if cfg.record.iter().any(|r| r.name == name) {
        return None; // idempotent: already present.
    }
    cfg.record.push(RecordDef {
        name: name.clone(),
        source: format!("tunnel:{tunnel_id}"),
        // A removed tunnel's name must not keep pointing at a dead exit; remove it.
        on_down: OnDown::Remove,
    });
    match ddns::save(root, &cfg) {
        Ok(_) => Some(name),
        Err(e) => {
            tracing::warn!(tunnel = tunnel_id, error = %e, "ddns auto-populate: save failed");
            None
        }
    }
}

// ── VPN-GW-4 — mesh egress routing (per-node / group / ANY) + failover chain ──

/// `set-route` — assign egress for a node / node-group / ANth (all-mesh) target
/// to a gateway with a primary tunnel + an ordered failover chain (+ a per-route
/// kill-switch). The body is an [`EgressRoute`] JSON
/// (`{target:{scope,name?}, gateway, primary, failover?, kill_switch?}`). The
/// assignment is validated + persisted to the durable routing table on the
/// shared substrate (one route per target, set replaces in place); every node
/// then resolves its own effective route from the same table.
fn set_route(svc: &VpnService, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
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
    let root = svc.workgroup_root.as_path();
    let mut routing = vpn_egress::load_routing(root);
    let key = r.target.key();
    routing.set(r);
    match vpn_egress::save_routing(root, &routing) {
        Ok(_) => json!({ "ok": true, "target": key }).to_string(),
        Err(e) => err(format!("set-route: save: {e}")),
    }
}

/// `clear-route` — remove the egress assignment for a target key
/// (`node:<name>` / `group:<name>` / `any`); the target then falls back to a
/// less-specific route (a node to its group's, a group to ANY) or to direct WAN
/// if none remains.
fn clear_route(svc: &VpnService, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(key) = req_body.map(str::trim).filter(|s| !s.is_empty()) else {
        return err("clear-route: missing target key (node:<n> / group:<n> / any)".into());
    };
    let root = svc.workgroup_root.as_path();
    let mut routing = vpn_egress::load_routing(root);
    if !routing.clear(key) {
        return err(format!("clear-route: no route for target '{key}'"));
    }
    match vpn_egress::save_routing(root, &routing) {
        Ok(_) => json!({ "ok": true, "target": key }).to_string(),
        Err(e) => err(format!("clear-route: save: {e}")),
    }
}

/// `list-routes` — the durable egress-routing table (every assignment).
fn list_routes(svc: &VpnService) -> String {
    let routing = vpn_egress::load_routing(svc.workgroup_root.as_path());
    json!({ "ok": true, "routes": routing.route }).to_string()
}

/// `route-status` — for a target key, report the route's gateway + ordered chain
/// and which tunnel is **currently active** (the first chain tunnel whose
/// interface is up on this node), so the operator sees the live failover state.
/// `active` is `null` when the whole chain is down — the kill-switch (if set)
/// then blocks egress (no WAN leak) instead of failing over.
fn route_status(svc: &VpnService, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(key) = req_body.map(str::trim).filter(|s| !s.is_empty()) else {
        return err("route-status: missing target key (node:<n> / group:<n> / any)".into());
    };
    let root = svc.workgroup_root.as_path();
    let routing = vpn_egress::load_routing(root);
    let Some(route) = routing.get(key) else {
        return err(format!("route-status: no route for target '{key}'"));
    };
    // The live down-set for the chain on THIS node (the gateway): a tunnel is
    // down when its `mvpn-<id>` interface isn't present. Resolving the id → its
    // ifname through the durable tunnel config keeps the sanitize/bound rule in
    // one place.
    let cfg = vpn::load(root);
    let down: Vec<String> = route
        .chain()
        .into_iter()
        .filter(|id| {
            let ifname = cfg
                .get(id)
                .map_or_else(|| format!("mvpn-{id}"), TunnelDef::ifname);
            !iface_up(svc.spawn, &ifname)
        })
        .collect();
    let active = route.active_tunnel(&down);
    json!({
        "ok": true,
        "target": key,
        "gateway": route.gateway,
        "chain": route.chain(),
        "active": active,
        "kill_switch": route.kill_switch,
    })
    .to_string()
}

// ── VPN-GW-6 — health + exit-IP/leak verification (operator-facing reads) ──

/// `verify-egress` — verify ONE tunnel's egress on demand: liveness, the real
/// exit IP fetched through the tunnel, the WAN-leak comparison, and a DNS-leak
/// probe. The body is the tunnel id. Returns the [`vpn_health::TunnelReport`]
/// (health verdict + the verified exit IP + the leak reason). This is the live
/// verification the UI's "verify now" button calls; the periodic sweep
/// ([`serve_bus`]) runs the same check + raises the alert.
fn verify_egress(svc: &VpnService, req_body: Option<&str>) -> String {
    let err = |m: String| json!({ "error": m }).to_string();
    let Some(id) = req_body.map(str::trim).filter(|s| !s.is_empty()) else {
        return err("verify-egress: missing tunnel id".into());
    };
    let root = svc.workgroup_root.as_path();
    let cfg = vpn::load(root);
    let Some(t) = cfg.get(id) else {
        return err(format!("verify-egress: no such tunnel '{id}'"));
    };
    // The raw-WAN IP (default route, not the tunnel) the exit is compared
    // against — only fetched when the tools can spawn.
    let wan = if svc.spawn {
        crate::ipc::vpn_health::wan_ip()
    } else {
        None
    };
    let report = crate::ipc::vpn_health::verify_tunnel(svc.spawn, t, wan.as_deref());
    json!({ "ok": true, "report": report.to_json() }).to_string()
}

/// `egress-health` — verify EVERY tunnel on every route's chain right now and
/// report the per-tunnel health (incl. the verified exit IP). The same sweep the
/// responder runs on its interval, exposed as a read so the UI can show the live
/// egress-health table (and DDNS-EGRESS-1 can read the verified exit IP). Does
/// NOT raise alerts — that's the periodic sweep's job; this is a pure read.
fn egress_health(svc: &VpnService) -> String {
    let root = svc.workgroup_root.as_path();
    let cfg = vpn::load(root);
    let routing = vpn_egress::load_routing(root);
    let wan = if svc.spawn {
        crate::ipc::vpn_health::wan_ip()
    } else {
        None
    };
    // Verify each tunnel that appears on some chain exactly once (reusing the
    // shared chain-tunnel verifier so the missing-tunnel rule lives in one place).
    let mut seen = std::collections::HashSet::new();
    let mut reports = Vec::new();
    for route in &routing.route {
        for id in route.chain() {
            if !seen.insert(id.clone()) {
                continue;
            }
            let report =
                crate::ipc::vpn_health::verify_chain_tunnel(svc.spawn, &id, &cfg, wan.as_deref());
            reports.push(report.to_json());
        }
    }
    json!({ "ok": true, "tunnels": reports }).to_string()
}

/// Write the produced secret material with owner-only perms (it carries the
/// private key). Creates the parent dir. Best-effort 0600.
fn write_secret(path: &std::path::Path, body: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// VPN-GW-6 — how often the responder runs the egress-health sweep (verify each
/// routed tunnel's exit IP / leak state, fail over the chain, raise
/// `vpn/tunnel-down`). Slower than the action-poll interval: the sweep shells out
/// to `curl` per tunnel, so a tight loop would hammer the reflectors. 30 s
/// catches a silent leak well within an operator's reaction window.
pub const HEALTH_SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// Run the VPN Bus responder loop until `should_stop`. Each iteration serves the
/// `action/vpn/*` verbs (fast poll) and, on the [`HEALTH_SWEEP_INTERVAL`] cadence,
/// runs the VPN-GW-6 egress-health sweep over the durable routes — verifying each
/// tunnel's real exit IP, failing the chain over off a leaking/down tunnel, and
/// raising the `vpn/tunnel-down` alert on `event/vpn/signals`.
pub fn serve_bus<F: Fn() -> bool>(persist: &Persist, svc: &VpnService, should_stop: F) {
    let mut cursors: HashMap<String, String> = HashMap::new();
    let mut last_sweep = std::time::Instant::now()
        .checked_sub(HEALTH_SWEEP_INTERVAL)
        .unwrap_or_else(std::time::Instant::now);
    while !should_stop() {
        poll_once(persist, svc, &mut cursors);
        if last_sweep.elapsed() >= HEALTH_SWEEP_INTERVAL {
            // The sweep is best-effort: it spawns its own probes and only writes
            // alert events, never blocks the action responder for long.
            let _ = crate::ipc::vpn_health::sweep(persist, svc.workgroup_root.as_path(), svc.spawn);
            last_sweep = std::time::Instant::now();
        }
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
        let s = VpnService::new(tmp.path().to_path_buf()).without_spawn();
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
        assert_eq!(ACTION_VERBS.len(), 14);
        assert!(ACTION_VERBS.contains(&"list-providers"));
        assert!(ACTION_VERBS.contains(&"setup-provider"));
        // VPN-GW-4 routing verbs.
        for v in ["set-route", "clear-route", "list-routes", "route-status"] {
            assert!(ACTION_VERBS.contains(&v), "missing {v}");
        }
        // VPN-GW-6 health verbs.
        for v in ["verify-egress", "egress-health"] {
            assert!(ACTION_VERBS.contains(&v), "missing {v}");
        }
    }

    // ── VPN-GW-6: health + exit-IP/leak verification reachable as verbs ──

    #[test]
    fn verify_egress_reports_down_without_iface() {
        // spawn disabled → the tunnel's interface reads absent → Down, with the
        // verified exit IP null. The verb is reachable + honest, no host I/O.
        let (_t, s) = svc();
        let _ = add(&s, "mullvad1", "wg");
        let r = build_reply(&s, "verify-egress", Some("mullvad1"));
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(true), "{r}");
        assert_eq!(v["report"]["ifname"], "mvpn-mullvad1");
        assert_eq!(v["report"]["health"], "down");
        assert_eq!(v["report"]["verified_exit_ip"], serde_json::Value::Null);
    }

    #[test]
    fn verify_egress_errors_on_missing_or_unknown() {
        let (_t, s) = svc();
        assert!(build_reply(&s, "verify-egress", None).contains("missing tunnel id"));
        assert!(build_reply(&s, "verify-egress", Some("ghost")).contains("no such tunnel"));
    }

    #[test]
    fn egress_health_verifies_every_chain_tunnel_once() {
        // Two routes sharing a failover tunnel: egress-health reports each
        // distinct chain tunnel exactly once (deduped), all Down with spawn off.
        let (_t, s) = svc();
        let body = set_route_body("any", None, "gw1", "mullvad1", &["proton1"]);
        assert!(build_reply(&s, "set-route", Some(&body)).contains("\"ok\":true"));
        let body = set_route_body("node", Some("anvil"), "gw1", "ivpn1", &["proton1"]);
        assert!(build_reply(&s, "set-route", Some(&body)).contains("\"ok\":true"));

        let r = build_reply(&s, "egress-health", None);
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(true), "{r}");
        let tunnels = v["tunnels"].as_array().unwrap();
        let ids: Vec<&str> = tunnels.iter().filter_map(|t| t["id"].as_str()).collect();
        // proton1 appears in both chains but is verified once.
        assert_eq!(
            ids.iter().filter(|i| **i == "proton1").count(),
            1,
            "{ids:?}"
        );
        for want in ["mullvad1", "proton1", "ivpn1"] {
            assert!(ids.contains(&want), "missing {want} in {ids:?}");
        }
        assert!(tunnels.iter().all(|t| t["health"] == "down"));
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

    // ── VPN-GW-3: selective egress wired to the tunnel lifecycle ──

    #[test]
    fn egress_plan_mark_is_iface_stable_not_config_position() {
        let (_t, s) = svc();
        let _ = add(&s, "first", "wg");
        let _ = add(&s, "second", "wg");
        let cfg = vpn::load(s.workgroup_root.as_path());
        let p_first = egress_plan(&s, cfg.get("first").unwrap());
        let p_second = egress_plan(&s, cfg.get("second").unwrap());
        // Each tunnel gets its own interface, fwmark, and routing table.
        assert_eq!(p_first.ifname, "mvpn-first");
        assert_eq!(p_second.ifname, "mvpn-second");
        assert_ne!(
            p_first.fwmark, p_second.fwmark,
            "tunnels must not share a mark"
        );
        assert_ne!(
            p_first.table, p_second.table,
            "tunnels must not share a table"
        );

        // The mark is a function of the interface, NOT the config position:
        // remove the FIRST tunnel (so "second" shifts from index 1 → 0) and
        // "second"'s mark must be unchanged. A positional slot would have
        // remapped it (and orphaned a kill-switch onto the old mark).
        let _ = build_reply(&s, "remove-tunnel", Some("first"));
        let cfg2 = vpn::load(s.workgroup_root.as_path());
        let p_second_after = egress_plan(&s, cfg2.get("second").unwrap());
        assert_eq!(
            p_second.fwmark, p_second_after.fwmark,
            "a sibling removal must not remap a surviving tunnel's mark"
        );
    }

    #[test]
    fn egress_plan_carves_out_the_nebula_overlay_so_mesh_never_tunnels() {
        let (_t, s) = svc();
        let _ = add(&s, "mullvad1", "wg");
        let cfg = vpn::load(s.workgroup_root.as_path());
        let p = egress_plan(&s, cfg.get("mullvad1").unwrap());
        // The default mesh overlay is carved out in both the up path and the
        // kill-switch (mesh traffic is direct, never dropped/tunneled).
        let overlay = mackes_mesh_types::vpn_egress::DEFAULT_OVERLAY_CIDR.to_string();
        assert_eq!(p.overlay_cidr, overlay);
        assert!(p.up_argv().iter().any(|c| c.contains(&overlay)));
        assert!(p.kill_switch_argv().iter().any(|c| c.contains(&overlay)));
        // The masquerade names the tunnel's real interface.
        assert!(p
            .up_argv()
            .iter()
            .any(|c| c.contains(&"masquerade".to_string())
                && c.contains(&"\"mvpn-mullvad1\"".to_string())));
    }

    #[test]
    fn egress_lifecycle_argv_is_complete_and_self_consistent() {
        // The lifecycle argv (applied by `bring` on a real up/down) must cover
        // the four pieces VPN-GW-3 locks: a route table, an fwmark ip-rule, an
        // nft masquerade, and a kill-switch drop — all on this tunnel's iface.
        // (We assert the argv here rather than spawn `ip`/`nft`, which would
        // mutate the host's real routing; the spawn path is `bring`, gated on
        // `svc.spawn` and reached at runtime by the responder.)
        let (_t, s) = svc();
        let _ = add(&s, "k", "wg");
        let cfg = vpn::load(s.workgroup_root.as_path());
        let p = egress_plan(&s, cfg.get("k").unwrap());
        let up = p.up_argv();
        assert!(up
            .iter()
            .any(|c| c[0] == "ip" && c.contains(&"rule".to_string())));
        assert!(up
            .iter()
            .any(|c| c[0] == "ip" && c.contains(&"route".to_string())));
        assert!(up.iter().any(|c| c.contains(&"masquerade".to_string())));
        assert!(p
            .kill_switch_argv()
            .iter()
            .any(|c| c.contains(&"drop".to_string())));
        // Down is the inverse: it deletes the nft table and the ip rules.
        let down = p.down_argv();
        assert!(down
            .iter()
            .any(|c| c[0] == "nft" && c.contains(&"delete".to_string())));
        assert!(
            down.iter()
                .filter(|c| c[0] == "ip" && c.contains(&"del".to_string()))
                .count()
                >= 2
        );
    }

    // ── VPN-GW-5: provider adapters reachable from the vpn responder ──

    // 44-char base64-looking WG keys for the setup-provider tests.
    const PK: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    const PUB: &str = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=";

    #[test]
    fn list_providers_returns_the_seven() {
        let (_t, s) = svc();
        let r = build_reply(&s, "list-providers", None);
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(true));
        let provs = v["providers"].as_array().unwrap();
        assert_eq!(provs.len(), 7);
        let ids: Vec<&str> = provs.iter().filter_map(|p| p["id"].as_str()).collect();
        for want in [
            "mullvad",
            "proton",
            "ivpn",
            "nord",
            "surfshark",
            "generic-wg",
            "generic-ovpn",
        ] {
            assert!(ids.contains(&want), "missing {want} in {ids:?}");
        }
        // Mullvad surfaces its first-party exit-check reflector.
        let mullvad = provs.iter().find(|p| p["id"] == "mullvad").unwrap();
        assert_eq!(mullvad["exit_check"], "https://am.i.mullvad.net/json");
        assert_eq!(mullvad["cli"], "mullvad");
    }

    #[test]
    fn setup_provider_wg_persists_tunnel_and_reports_exit_check() {
        // Inject a real-AEAD store over a tempdir so the distribution `put` is
        // deterministic (independent of the host's MCNF_REPO / age key).
        let tmp = tempfile::tempdir().unwrap();
        let key_path = tmp.path().join("mcnf-age-key");
        std::fs::write(&key_path, "AGE-SECRET-KEY-1PERSISTSTESTZZZ\n").unwrap();
        let s = VpnService::new(tmp.path().to_path_buf())
            .without_spawn()
            .with_store(SecretStore::LocalAead {
                dir: tmp.path().join("secrets"),
                key_path,
            });
        let body = json!({
            "provider": "mullvad",
            "id": "mullvad1",
            "server": "us-nyc",
            "private_key": PK,
            "peer_public_key": PUB,
            "address": "10.64.0.2/32",
            "endpoint": "us-nyc-wg-301.relays.example",
            "dns": "10.64.0.1",
        })
        .to_string();
        let r = build_reply(&s, "setup-provider", Some(&body));
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(true), "{r}");
        assert_eq!(v["provider"], "mullvad");
        assert_eq!(v["ifname"], "mvpn-mullvad1");
        assert_eq!(v["method"], "wg");
        assert_eq!(v["exit_check"], "https://am.i.mullvad.net/json");
        // The handling node (it holds the plaintext) writes the secret to its
        // store regardless of leadership — so distribution succeeds here.
        assert_eq!(
            v["secret_distributed"],
            serde_json::Value::Bool(true),
            "{r}"
        );
        // The durable def carries the deterministic creds_ref (not the secret).
        assert_eq!(v["creds_ref"], "vpn/mvpn-mullvad1");
        // The durable def landed in the config (and carries NO secret material).
        let list = build_reply(&s, "list-tunnels", None);
        assert!(list.contains("mullvad1"), "{list}");
        assert!(
            !list.contains(PK),
            "private key must not be in the durable config: {list}"
        );
        // The creds_ref IS in the durable config (so bring-up can resolve it).
        assert!(list.contains("vpn/mvpn-mullvad1"), "{list}");
        // The secret round-trips out of the store decrypted (real distribution).
        let got = s.secret_store().get("vpn/mvpn-mullvad1").unwrap().unwrap();
        assert!(got.contains(&format!("PrivateKey = {PK}")), "{got}");
    }

    // ── VPN-GW-2: secret distribution + materialized bring-up ──

    /// A service whose secret store is a real-AEAD local store over a tempdir, so
    /// the distribution `put` + the bring-up `get` exercise real crypto with no
    /// etcd/age CLI. The returned dir keeps the workgroup root + store alive.
    fn svc_with_store() -> (tempfile::TempDir, VpnService) {
        let tmp = tempfile::tempdir().unwrap();
        let key_path = tmp.path().join("mcnf-age-key");
        std::fs::write(
            &key_path,
            "AGE-SECRET-KEY-1QQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQSXKLP0E\n",
        )
        .unwrap();
        let store = SecretStore::LocalAead {
            dir: tmp.path().join("secrets"),
            key_path,
        };
        let s = VpnService::new(tmp.path().to_path_buf()).with_store(store);
        (tmp, s)
    }

    #[test]
    fn secret_round_trip_distribute_then_materialize_on_up() {
        let (_t, s) = svc_with_store();
        // setup-provider age-encrypts the secret into the store.
        let body = json!({
            "provider": "mullvad",
            "id": "mullvad1",
            "server": "us-nyc",
            "private_key": PK,
            "peer_public_key": PUB,
            "address": "10.64.0.2/32",
            "endpoint": "us-nyc-wg-301.relays.example",
            "dns": "10.64.0.1",
        })
        .to_string();
        let r = build_reply(&s, "setup-provider", Some(&body));
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(true), "{r}");
        // The secret really was age-encrypted + distributed.
        assert_eq!(
            v["secret_distributed"],
            serde_json::Value::Bool(true),
            "{r}"
        );
        assert_eq!(v["creds_ref"], "vpn/mvpn-mullvad1");

        // The store now holds the ENCRYPTED secret (not plaintext) and decrypts
        // back to the rendered wg-quick config.
        let got = s
            .secret_store()
            .get("vpn/mvpn-mullvad1")
            .unwrap()
            .expect("secret distributed");
        assert!(got.contains("[Interface]"));
        assert!(got.contains(&format!("PrivateKey = {PK}")));

        // tunnel-up resolves the SAME distributed body via the def's creds_ref,
        // which materialize_secret then writes where wg-quick reads it. (We read
        // the store body directly rather than spawn the real `wg-quick`, which
        // would mutate the host; the path-write is covered by write_secret.)
        let t = vpn::load(s.workgroup_root.as_path())
            .get("mullvad1")
            .cloned()
            .unwrap();
        let resolved = s
            .secret_store()
            .get(&t.creds_ref)
            .unwrap()
            .expect("creds_ref resolves");
        assert_eq!(
            resolved, got,
            "tunnel-up resolves the same distributed body"
        );
        assert!(resolved.contains("AllowedIPs = 0.0.0.0/0, ::/0"));
    }

    #[test]
    fn tunnel_up_honest_pending_when_secret_not_distributed() {
        // A follower-shaped service (bare svc, no leader lease) that set up a
        // tunnel: creds_ref is set but no node distributed the secret. With a
        // spawn-enabled service whose store has no entry, materialize must
        // report the honest "distribution pending", NOT spawn / fake-succeed.
        let tmp = tempfile::tempdir().unwrap();
        let key_path = tmp.path().join("mcnf-age-key");
        std::fs::write(&key_path, "AGE-SECRET-KEY-1EMPTYSTOREZZZ\n").unwrap();
        let store = SecretStore::LocalAead {
            dir: tmp.path().join("secrets"),
            key_path,
        };
        let s = VpnService::new(tmp.path().to_path_buf()).with_store(store);
        // A tunnel with a creds_ref but nothing in the store.
        let t = TunnelDef {
            id: "mullvad1".into(),
            provider: "mullvad".into(),
            method: Method::Wg,
            server: "us-nyc".into(),
            protocol: "udp".into(),
            creds_ref: "vpn/mvpn-mullvad1".into(),
        };
        let detail = materialize_secret(&s, &t, SecretKind::WgQuick).unwrap_err();
        assert!(
            detail.contains("secret distribution pending"),
            "expected honest pending, got: {detail}"
        );
        assert!(detail.contains("vpn/mvpn-mullvad1"), "{detail}");
    }

    #[test]
    fn setup_provider_generic_wg_paste_path() {
        let (_t, s) = svc();
        let conf = format!(
            "[Interface]\nPrivateKey = {PK}\nAddress = 10.2.0.2/32\nDNS = 1.1.1.1\n[Peer]\nPublicKey = {PUB}\nAllowedIPs = 0.0.0.0/0\nEndpoint = paste.example.net:51820\n"
        );
        let body = json!({
            "provider": "generic-wg",
            "id": "paste1",
            "server": "fra",
            "wg_config": conf,
        })
        .to_string();
        let r = build_reply(&s, "setup-provider", Some(&body));
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(true), "{r}");
        assert_eq!(v["provider"], "generic-wg");
        assert_eq!(v["ifname"], "mvpn-paste1");
        // No first-party reflector → neutral.
        assert_eq!(v["exit_check"], "https://ipinfo.io/json");
    }

    #[test]
    fn setup_provider_named_provider_paste_keeps_label() {
        // A Mullvad-exported .conf pasted into wg_config keeps the mullvad
        // label (and its exit-check host), not generic-wg.
        let (_t, s) = svc();
        let conf = format!(
            "[Interface]\nPrivateKey = {PK}\nAddress = 10.64.0.2/32\n[Peer]\nPublicKey = {PUB}\nEndpoint = m.example.net:51820\n"
        );
        let body = json!({"provider":"mullvad","id":"m1","wg_config":conf}).to_string();
        let r = build_reply(&s, "setup-provider", Some(&body));
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(true), "{r}");
        assert_eq!(v["provider"], "mullvad");
        assert_eq!(v["exit_check"], "https://am.i.mullvad.net/json");
    }

    #[test]
    fn setup_provider_null_wg_config_falls_through_to_flat_fields() {
        // wg_config present as JSON null must NOT hijack the paste path; the
        // flat WgSetup fields drive the structured build.
        let (_t, s) = svc();
        let body = json!({
            "provider": "ivpn",
            "id": "i1",
            "wg_config": serde_json::Value::Null,
            "private_key": PK,
            "peer_public_key": PUB,
            "address": "10.0.0.2/32",
            "endpoint": "ivpn.example:51820",
        })
        .to_string();
        let r = build_reply(&s, "setup-provider", Some(&body));
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(true), "{r}");
        assert_eq!(v["provider"], "ivpn");
        assert_eq!(v["ifname"], "mvpn-i1");
    }

    #[test]
    fn setup_provider_generic_ovpn_import_path() {
        let (_t, s) = svc();
        let body = json!({
            "provider": "generic-ovpn",
            "id": "ovpn1",
            "ovpn": "client\nremote nl-ams.example.com 1194 udp\nauth-user-pass\n",
        })
        .to_string();
        let r = build_reply(&s, "setup-provider", Some(&body));
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(true), "{r}");
        assert_eq!(v["provider"], "generic-ovpn");
        assert_eq!(v["method"], "ovpn");
        assert_eq!(v["ifname"], "mvpn-ovpn1");
    }

    #[test]
    fn setup_provider_rejects_bad_input() {
        let (_t, s) = svc();
        // Missing provider.
        assert!(build_reply(&s, "setup-provider", Some("{}")).contains("missing 'provider'"));
        // Unknown provider.
        let r = build_reply(
            &s,
            "setup-provider",
            Some(&json!({"provider":"nope","id":"x"}).to_string()),
        );
        assert!(r.contains("unknown provider"), "{r}");
        // Malformed WG key surfaces the adapter error.
        let body = json!({
            "provider": "ivpn",
            "id": "i1",
            "private_key": "not-a-key",
            "peer_public_key": PUB,
            "address": "10.0.0.2/32",
            "endpoint": "h.example",
        })
        .to_string();
        let r = build_reply(&s, "setup-provider", Some(&body));
        assert!(r.contains("invalid private_key"), "{r}");
        // generic-ovpn without an ovpn body.
        let r = build_reply(
            &s,
            "setup-provider",
            Some(&json!({"provider":"generic-ovpn","id":"o"}).to_string()),
        );
        assert!(r.contains("needs an 'ovpn' body"), "{r}");
    }

    // ── VPN-GW-4: mesh egress routing (per-node / group / ANY) + failover ──

    fn set_route_body(
        scope: &str,
        name: Option<&str>,
        gw: &str,
        primary: &str,
        chain: &[&str],
    ) -> String {
        let mut target = json!({ "scope": scope });
        if let Some(n) = name {
            target["name"] = json!(n);
        }
        json!({
            "target": target,
            "gateway": gw,
            "primary": primary,
            "failover": chain,
        })
        .to_string()
    }

    #[test]
    fn set_list_clear_route_round_trip() {
        let (_t, s) = svc();
        // Set an ANY/all-mesh route through a gateway with a failover chain.
        let body = set_route_body("any", None, "gw1", "mullvad1", &["proton1"]);
        let r = build_reply(&s, "set-route", Some(&body));
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(true), "{r}");
        assert_eq!(v["target"], "any");

        // And a per-node override.
        let body = set_route_body("node", Some("anvil"), "gw2", "ivpn1", &[]);
        assert!(build_reply(&s, "set-route", Some(&body)).contains("\"ok\":true"));

        // list-routes carries both, with the chain + kill-switch default.
        let list = build_reply(&s, "list-routes", None);
        let lv: serde_json::Value = serde_json::from_str(&list).unwrap();
        let routes = lv["routes"].as_array().unwrap();
        assert_eq!(routes.len(), 2, "{list}");
        let any = routes
            .iter()
            .find(|r| r["target"]["scope"] == "any")
            .unwrap();
        assert_eq!(any["gateway"], "gw1");
        assert_eq!(any["primary"], "mullvad1");
        assert_eq!(any["failover"][0], "proton1");
        // Kill-switch defaulted on (Q8).
        assert_eq!(any["kill_switch"], serde_json::Value::Bool(true));

        // Re-setting ANY replaces in place (still 2 routes total).
        let body = set_route_body("any", None, "gwX", "nord1", &[]);
        assert!(build_reply(&s, "set-route", Some(&body)).contains("\"ok\":true"));
        let list = build_reply(&s, "list-routes", None);
        let lv: serde_json::Value = serde_json::from_str(&list).unwrap();
        assert_eq!(lv["routes"].as_array().unwrap().len(), 2);

        // Clear the node route by its key.
        let r = build_reply(&s, "clear-route", Some("node:anvil"));
        assert!(r.contains("\"ok\":true"), "{r}");
        let list = build_reply(&s, "list-routes", None);
        let lv: serde_json::Value = serde_json::from_str(&list).unwrap();
        assert_eq!(lv["routes"].as_array().unwrap().len(), 1);
        // Clearing a ghost errors.
        assert!(build_reply(&s, "clear-route", Some("node:ghost")).contains("no route"));
    }

    #[test]
    fn set_route_rejects_bad_input() {
        let (_t, s) = svc();
        // Missing body.
        assert!(build_reply(&s, "set-route", None).contains("missing EgressRoute body"));
        // Empty primary → validation error.
        let body = set_route_body("any", None, "gw1", "", &[]);
        assert!(build_reply(&s, "set-route", Some(&body)).contains("primary tunnel is empty"));
        // A tunnel can't fail over to itself.
        let body = set_route_body("any", None, "gw1", "m1", &["m1"]);
        assert!(build_reply(&s, "set-route", Some(&body)).contains("duplicate tunnel"));
        // clear-route with no key.
        assert!(build_reply(&s, "clear-route", None).contains("missing target key"));
    }

    #[test]
    fn route_status_reports_chain_and_active_tunnel() {
        // With spawn disabled, every tunnel interface reads down, so the whole
        // chain is down and `active` is null — exactly the kill-switch-floor
        // state. The chain + gateway + kill-switch flag still report.
        let (_t, s) = svc();
        let body = set_route_body("any", None, "gw1", "mullvad1", &["proton1"]);
        assert!(build_reply(&s, "set-route", Some(&body)).contains("\"ok\":true"));
        let r = build_reply(&s, "route-status", Some("any"));
        let v: serde_json::Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["ok"], serde_json::Value::Bool(true), "{r}");
        assert_eq!(v["gateway"], "gw1");
        assert_eq!(v["chain"][0], "mullvad1");
        assert_eq!(v["chain"][1], "proton1");
        // No interface is up (spawn disabled) → whole chain down → active null.
        assert_eq!(v["active"], serde_json::Value::Null);
        assert_eq!(v["kill_switch"], serde_json::Value::Bool(true));
        // Status for an unknown target errors.
        assert!(build_reply(&s, "route-status", Some("node:nope")).contains("no route"));
        assert!(build_reply(&s, "route-status", None).contains("missing target key"));
    }
}
