//! `SurroundingList` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.

/// Handle the `surrounding-list` subcommand.
#[allow(unreachable_code)]
pub fn run() -> anyhow::Result<()> {
    {
        use mackesd_core::surrounding_hosts::read_all_surrounding;
        let base = mackesd_core::surrounding_hosts::default_surrounding_root();
        for ch in read_all_surrounding(&base) {
            println!("{}", serde_json::to_string(&ch)?);
        }
    }
    Ok(())
}
