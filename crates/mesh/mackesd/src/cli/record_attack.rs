//! `RecordAttack` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.

/// Handle the `record-attack` subcommand.
#[allow(unreachable_code)]
pub fn run(source: String) -> anyhow::Result<()> {
    {
        use mackesd_core::surrounding_hosts::{
            accumulate_alert, auto_ack, load_alert_store, save_alert_store,
        };
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        if let Some(data_dir) = dirs::data_dir() {
            let path = data_dir
                .join("mde")
                .join("surrounding")
                .join("persistent-alerts.json");
            let mut store = load_alert_store(&path);
            auto_ack(&mut store, now_ms);
            accumulate_alert(&mut store, &source, now_ms);
            let _ = save_alert_store(&path, &store);
            if let Some(a) = store.get(&source) {
                println!(
                    "{}\tcount={}\tfirst_seen_ms={}\tlast_seen_ms={}",
                    a.source, a.count, a.first_seen_ms, a.last_seen_ms
                );
            }
        }
    }
    Ok(())
}
