//! CLI verb handlers extracted from `bin/mackesd.rs` (arch-1 SLICE 1).
//!
//! Each submodule owns one `Cmd` subcommand's handler as `pub fn run(..)`.
//! `main()` parses args and dispatches here; the daemon half (`run_serve`)
//! is untouched by this slice.

pub mod adopt_xcp;
pub mod ansible_history;
pub mod apply;
pub mod arp_spoof_check;
pub mod audit_log;
pub mod audit_verify;
pub mod ca;
pub mod captive_portal_check;
pub mod classify_host;
pub mod connect;
pub mod converge;
pub mod cutover_audit;
pub mod ddns;
pub mod decommission;
pub mod discover_mde_peers;
pub mod discover_mdns;
pub mod dns;
pub mod dns_leak_check;
pub mod enroll;
pub mod enroll_token;
pub mod events;
pub mod evil_twin_check;
pub mod fleet_push_setting;
pub mod fleet_status;
pub mod found;
pub mod generate_passcode;
pub mod healthz;
pub mod hop_advertise;
pub mod identity;
pub mod images;
pub mod import_legacy;
pub mod inventory_legacy;
pub mod join;
pub mod leave;
pub mod log_emit;
pub mod mesh_firewall_plan;
pub mod mesh_fs_status;
pub mod mesh_init;
pub mod mesh_ssh_key;
pub mod migrate;
pub mod mirrors;
pub mod nebula;
pub mod netstate;
pub mod node_admin;
pub mod nodes;
pub mod onboard;
pub mod peers;
pub mod peers_why;
pub mod playbooks;
pub mod policy;
pub mod preset_launch;
pub mod probe;
pub mod profiles;
pub mod reconcile;
pub mod record_attack;
pub mod recovery;
pub mod reenroll;
pub mod remediate;
pub mod revisions;
pub mod rogue_dhcp_check;
pub mod role_gate;
pub mod role_pin;
pub mod role_workers;
pub mod rotate_passcode;
pub mod route_trace;
pub mod secret;
pub mod service_card;
pub mod set_external_addr;
pub mod show_passcode;
pub mod state_restore;
pub mod status;
pub mod surface_mok_mint;
pub mod surrounding_list;
pub mod surrounding_trust;
pub mod tag;
pub mod tags;
pub mod take_leadership;
pub mod transfer;
pub mod upgrade;
pub mod validate;
pub mod vpn_import;
pub mod wake_peer;

/// WL-FUNC-033 — leftover retirement: `voip_rtt.rs` is gone from this
/// tree. Do not reintroduce the leftover file or a live `pub mod` path for it.
#[cfg(test)]
mod leftover_retirements {
    /// Live `pub mod` names compiled by this CLI tree (source-parsed).
    fn live_cli_module_names(source: &str) -> Vec<&str> {
        source
            .lines()
            .filter_map(|line| {
                line.trim()
                    .strip_prefix("pub mod ")
                    .and_then(|rest| rest.strip_suffix(';'))
            })
            .collect()
    }

    /// Build the retired declaration without embedding the forbidden
    /// contiguous token sequence in this file (so the raw-source scan
    /// stays honest).
    fn retired_pub_mod_needle() -> String {
        format!("{} {}", "pub mod", "voip_rtt")
    }

    #[test]
    fn voip_rtt_is_not_a_live_cli_path() {
        let source = include_str!("mod.rs");
        let live = live_cli_module_names(source);
        let needle = retired_pub_mod_needle();
        assert!(
            !live.contains(&"voip_rtt"),
            "retired voip-rtt leftover must not be a live cli path"
        );
        assert!(
            !source.contains(needle.as_str()),
            "reintroduced `{needle}` must fail even without a trailing semicolon"
        );
        assert!(
            live.contains(&"vpn_import") && live.contains(&"validate"),
            "parser still sees neighboring live cli mods"
        );
    }

    #[test]
    fn voip_rtt_rs_leftover_file_is_gone() {
        let leftover = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("cli")
            .join("voip_rtt.rs");
        assert!(
            !leftover.is_file(),
            "WL-FUNC-033 leftover file is gone and must stay deleted: {}",
            leftover.display()
        );
    }

    fn parity_ledger() -> &'static str {
        include_str!("../../../../../docs/platform/WL-FUNC-011-parity-ledger.md")
    }

    fn assert_ledger_cites(sha: &str) {
        let marker = format!("deleted in `{sha}`");
        assert!(
            parity_ledger().contains(marker.as_str()),
            "parity ledger must contain the deleting-revision marker {marker}"
        );
    }

    #[test]
    fn parity_ledger_cites_deleting_revisions() {
        assert_ledger_cites("aad4d5115e011195b01df8595a2135438073aeea");
        assert_ledger_cites("c3b589dae761df9e9e9362b6d4308b7a6bbd4dfe");
        assert_ledger_cites("1bbd9706e34ca45bec905efab73d1db0b92a3261");
        assert_ledger_cites("858ec546890fd8ffc7fb16d5e90ae8d5f2d580f7");
        assert_ledger_cites("afc45f0fb28094aa9662adefe73120b552a14b15");
        assert_ledger_cites("7f46e1f1a8ad3d8d7226fe7131c22d27970bf06a");
    }
}
