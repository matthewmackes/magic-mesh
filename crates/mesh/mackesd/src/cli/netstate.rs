//! `Netstate` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `netstate` subcommand.
#[allow(unreachable_code)]
pub fn run(cmd: NetstateCmd) -> anyhow::Result<()> {
    {
        // PLANES-15 — desired (elected revision) vs actual (live
        // nmstate) interface diff (W68).
        use magic_fleet::netstate::{IpConfig, NetInterface, NetOps, SystemNetOps};
        let NetstateCmd::Diff { json } = cmd;
        let root = mackesd_core::default_qnm_shared_root();
        let desired = magic_fleet::store::elect_head(&magic_fleet::store::revisions_dir(&root))
            .map(|h| h.spec.netstate)
            .unwrap_or_default();
        let actual = SystemNetOps.read_actual();

        // Compact one-line IPv4 summary for an interface.
        fn ipv4_summary(cfg: Option<&IpConfig>) -> String {
            match cfg {
                None => "—".to_string(),
                Some(c) if !c.enabled => "disabled".to_string(),
                Some(c) if c.dhcp => "dhcp".to_string(),
                Some(c) if c.addresses.is_empty() => "no-addr".to_string(),
                Some(c) => c
                    .addresses
                    .iter()
                    .map(magic_fleet::netstate::IpAddress::cidr)
                    .collect::<Vec<_>>()
                    .join(", "),
            }
        }
        fn find<'a>(set: &'a [NetInterface], name: &str) -> Option<&'a NetInterface> {
            set.iter().find(|i| i.name == name)
        }
        // The union of managed (desired) + observed (actual) names,
        // desired first so the managed interfaces lead.
        let mut names: Vec<String> = desired.interfaces.iter().map(|i| i.name.clone()).collect();
        for i in &actual.interfaces {
            if !names.contains(&i.name) {
                names.push(i.name.clone());
            }
        }
        let rows: Vec<serde_json::Value> = names
            .iter()
            .map(|name| {
                let d = find(&desired.interfaces, name);
                let a = find(&actual.interfaces, name);
                let managed = d.is_some();
                let in_sync = match (d, a) {
                    (Some(d), Some(a)) => Some(
                        d.state == a.state
                            && ipv4_summary(d.ipv4.as_ref()) == ipv4_summary(a.ipv4.as_ref()),
                    ),
                    (Some(_), None) => Some(false), // desired but not present
                    _ => None,                      // unmanaged — informational
                };
                serde_json::json!({
                    "name": name,
                    "managed": managed,
                    "desired_state": d.map(|i| i.state.as_nmstate()),
                    "desired_ipv4": d.map(|i| ipv4_summary(i.ipv4.as_ref())),
                    "actual_state": a.map(|i| i.state.as_nmstate()),
                    "actual_ipv4": a.map(|i| ipv4_summary(i.ipv4.as_ref())),
                    "in_sync": in_sync,
                })
            })
            .collect();
        if json {
            println!("{}", serde_json::to_string(&rows)?);
        } else if rows.is_empty() {
            println!("no interfaces observed");
        } else {
            println!(
                "{:<12} {:<8} {:<18} {:<18} {:<8}",
                "IFACE", "MANAGED", "DESIRED", "ACTUAL", "SYNC"
            );
            for r in &rows {
                let sync = match r["in_sync"].as_bool() {
                    Some(true) => "ok",
                    Some(false) => "DRIFT",
                    None => "-",
                };
                println!(
                    "{:<12} {:<8} {:<18} {:<18} {:<8}",
                    r["name"].as_str().unwrap_or("-"),
                    r["managed"].as_bool().unwrap_or(false),
                    format!(
                        "{}/{}",
                        r["desired_state"].as_str().unwrap_or("-"),
                        r["desired_ipv4"].as_str().unwrap_or("-")
                    ),
                    format!(
                        "{}/{}",
                        r["actual_state"].as_str().unwrap_or("-"),
                        r["actual_ipv4"].as_str().unwrap_or("-")
                    ),
                    sync
                );
            }
        }
        return Ok(());
    }
    #[cfg(feature = "async-services")]
    Ok(())
}
