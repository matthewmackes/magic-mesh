//! VPN-GW-1 (responder) — `action/vpn/*` over the tunnel config + `wg-quick`/
//! `openvpn` (design: `docs/design/vpn-gateway.md`).
//!
//! CRUD on the per-node [`mackes_mesh_types::vpn::VpnConfig`] (TOML on the shared
//! substrate) + best-effort bring-up/down via the pure argv builders. The
//! secret-material distribution (age creds → `/etc/wireguard/<ifname>.conf`) is
//! VPN-GW-3; here `tunnel-up` spawns `wg-quick`/`openvpn` and reports the result,
//! so it works once the config is present + is honest ("config missing") when not.
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
use mackes_mesh_types::vpn_egress::EgressPlan;
use mackes_mesh_types::vpn_providers::{
    self, AdapterError, ProducedTunnel, Provider, SecretKind, WgSetup,
};

/// The VPN responder — rooted at the shared workgroup root (the config home).
#[derive(Debug, Clone)]
pub struct VpnService {
    workgroup_root: PathBuf,
    /// `wg-quick`/`openvpn`/`ip` binaries are spawned by default; tests set the
    /// flag false to exercise the pure CRUD without the system tools.
    spawn: bool,
}

impl VpnService {
    /// Build the service rooted at the shared workgroup root.
    #[must_use]
    pub fn new(workgroup_root: PathBuf) -> Self {
        Self {
            workgroup_root,
            spawn: true,
        }
    }

    /// Disable the tool shell-out (tests).
    #[must_use]
    pub fn without_spawn(mut self) -> Self {
        self.spawn = false;
        self
    }
}

/// Action verbs served on `action/vpn/<verb>`.
pub const ACTION_VERBS: [&str; 8] = [
    "list-tunnels",
    "add-tunnel",
    "remove-tunnel",
    "tunnel-up",
    "tunnel-down",
    "tunnel-status",
    // VPN-GW-5 — provider adapters (5 named + generic WG paste / .ovpn import).
    "list-providers",
    "setup-provider",
];

/// Where a produced tunnel's secret material lands on the node before bring-up.
/// VPN-GW-2/3 will age-encrypt + leader-distribute this; until then it's written
/// locally so a single-node setup works end-to-end. WireGuard configs go to the
/// `wg-quick` config dir; `.ovpn` to the openvpn client dir.
#[must_use]
fn secret_path(kind: SecretKind, ifname: &str) -> PathBuf {
    match kind {
        SecretKind::WgQuick => PathBuf::from(format!("/etc/wireguard/{ifname}.conf")),
        SecretKind::Ovpn => PathBuf::from(format!("/etc/openvpn/client/{ifname}.ovpn")),
    }
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

    let produced = match produced {
        Ok(p) => p,
        Err(e) => return err(format!("setup-provider: {e}")),
    };

    let ifname = produced.def.ifname();
    // Write the secret material where bring-up reads it (single-node path; the
    // leader-managed age distribution is VPN-GW-2/3). Best-effort — honest on a
    // write failure rather than silently claiming success.
    let mut wrote_secret = false;
    let mut secret_note = String::new();
    if svc.spawn {
        let path = secret_path(produced.secret_kind, &ifname);
        match write_secret(&path, &produced.secret) {
            Ok(()) => wrote_secret = true,
            Err(e) => secret_note = format!("secret not written ({}): {e}", path.display()),
        }
    } else {
        secret_note = "spawn disabled — secret not written".into();
    }

    // Persist the durable def (no secret material) into the node's VPN config.
    let root = svc.workgroup_root.as_path();
    let mut cfg = vpn::load(root);
    cfg.upsert(produced.def.clone());
    if let Err(e) = vpn::save(root, &cfg) {
        return err(format!("setup-provider: save tunnel: {e}"));
    }

    json!({
        "ok": true,
        "id": produced.def.id,
        "provider": produced.def.provider,
        "ifname": ifname,
        "method": match produced.def.method {
            Method::Wg => "wg",
            Method::Ovpn => "ovpn",
            Method::Cli => "cli",
            Method::Api => "api",
        },
        "secret_written": wrote_secret,
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
        assert_eq!(ACTION_VERBS.len(), 8);
        assert!(ACTION_VERBS.contains(&"list-providers"));
        assert!(ACTION_VERBS.contains(&"setup-provider"));
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
        assert_ne!(p_first.fwmark, p_second.fwmark, "tunnels must not share a mark");
        assert_ne!(p_first.table, p_second.table, "tunnels must not share a table");

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
        assert!(up.iter().any(|c| c[0] == "ip" && c.contains(&"rule".to_string())));
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
        assert!(down
            .iter()
            .filter(|c| c[0] == "ip" && c.contains(&"del".to_string()))
            .count()
            >= 2);
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
        let (_t, s) = svc(); // spawn disabled → no secret write attempted
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
        // spawn disabled → secret intentionally not written, reported honestly.
        assert_eq!(v["secret_written"], serde_json::Value::Bool(false));
        // The durable def landed in the config (and carries NO secret).
        let list = build_reply(&s, "list-tunnels", None);
        assert!(list.contains("mullvad1"), "{list}");
        assert!(
            !list.contains(PK),
            "private key must not be in the durable config: {list}"
        );
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
}
