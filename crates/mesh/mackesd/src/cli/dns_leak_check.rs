//! `DnsLeakCheck` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.

/// Handle the `dns-leak-check` subcommand.
#[allow(unreachable_code)]
pub fn run(expected: Vec<String>) -> anyhow::Result<()> {
    {
        use mackesd_core::surrounding_hosts::{dns_leak, parse_resolv_nameservers};
        let content = std::fs::read_to_string("/etc/resolv.conf").unwrap_or_default();
        let leaked = dns_leak(&parse_resolv_nameservers(&content), &expected);
        for ip in &leaked {
            println!("{ip}");
        }
        if !leaked.is_empty() {
            eprintln!(
                "DNS-LEAK: {} resolver(s) outside the expected mesh set",
                leaked.len()
            );
            std::process::exit(1);
        }
    }
    Ok(())
}
