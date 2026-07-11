//! `Connect` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.

/// Handle the `connect` subcommand.
#[allow(unreachable_code)]
pub fn run(ip: String, port: u16) -> anyhow::Result<()> {
    match mackesd_core::connect_actions::connect_argv(&ip, port) {
        Some((service, argv)) => {
            println!("{service}\t{}", argv.join(" "));
        }
        None => {
            eprintln!("error: no known connect-action for port {port}");
            std::process::exit(1);
        }
    }
    Ok(())
}
