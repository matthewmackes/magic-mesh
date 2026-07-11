//! `SurroundingList` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.

/// Handle the `surrounding-list` subcommand.
#[allow(unreachable_code)]
pub fn run() -> anyhow::Result<()> {
    {
        use mackesd_core::surrounding_hosts::read_all_surrounding;
        if let Some(data_dir) = dirs::data_dir() {
            let base = data_dir.join("mde").join("surrounding");
            for ch in read_all_surrounding(&base) {
                println!("{}", serde_json::to_string(&ch)?);
            }
        }
    }
    Ok(())
}
