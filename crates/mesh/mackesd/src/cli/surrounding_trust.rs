//! `SurroundingTrust` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.

/// Handle the `surrounding-trust` subcommand.
#[allow(unreachable_code)]
pub fn run(key: String, state: String) -> anyhow::Result<()> {
    {
        use mackesd_core::surrounding_hosts::{set_host_trust, TrustState};
        let ts = match state.to_ascii_lowercase().as_str() {
            "trusted" => TrustState::Trusted,
            "blocked" => TrustState::Blocked,
            "unknown" => TrustState::Unknown,
            other => {
                eprintln!("error: unknown trust state '{other}' (want trusted|blocked|unknown)");
                std::process::exit(1);
            }
        };
        let Some(data_dir) = dirs::data_dir() else {
            eprintln!("error: no XDG data dir");
            std::process::exit(1);
        };
        let path = data_dir.join("mde").join("surrounding").join("trust.json");
        match set_host_trust(&path, &key, ts) {
            Ok(_) => println!("{key}\t{}", ts.wire_name()),
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
    }
    Ok(())
}
