//! `MeshFirewallPlan` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.

/// Handle the `mesh-firewall-plan` subcommand.
#[allow(unreachable_code)]
pub fn run() -> anyhow::Result<()> {
    {
        use mackesd_core::surrounding_hosts::{
            blocked_ips, drop_rich_rule_body, read_all_surrounding,
        };
        if let Some(data_dir) = dirs::data_dir() {
            let base = data_dir.join("mde").join("surrounding");
            for ip in blocked_ips(&read_all_surrounding(&base)) {
                println!("{}", drop_rich_rule_body(&ip));
            }
        }
    }
    Ok(())
}
