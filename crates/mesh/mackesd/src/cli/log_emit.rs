//! `LogEmit` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `log-emit` subcommand.
#[allow(unreachable_code)]
pub fn run(level: String, target: String, message: String) -> anyhow::Result<()> {
    {
        let root = mackesd_core::default_qnm_shared_root();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let record = magic_fleet::structured_log::LogRecord {
            ts_ms: now_ms,
            host: local_hostname(),
            level,
            target,
            message,
            fields: std::collections::BTreeMap::new(),
        };
        magic_fleet::structured_log::append(&root, &record)
            .map_err(|e| anyhow::anyhow!("log-emit append: {e}"))?;
        return Ok(());
    }
    Ok(())
}
