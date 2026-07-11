//! `ClassifyHost` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.

/// Handle the `classify-host` subcommand.
#[allow(unreachable_code)]
pub fn run(
    mdns: Vec<String>,
    port: Vec<u16>,
    vendor: String,
    hostname: String,
    mac: String,
) -> anyhow::Result<()> {
    {
        // Derive the vendor from the MAC's OUI when not given directly.
        let oui_vendor = if vendor.is_empty() && !mac.is_empty() {
            mackesd_core::surrounding_hosts::load_system_oui()
                .vendor_for(&mac)
                .unwrap_or_default()
        } else {
            vendor
        };
        let sig = mackesd_core::surrounding_hosts::HostSignals {
            mdns_services: mdns,
            open_ports: port,
            oui_vendor,
            hostname,
        };
        let ty = mackesd_core::surrounding_hosts::classify(&sig);
        println!("{}", ty.wire_name());
    }
    Ok(())
}
