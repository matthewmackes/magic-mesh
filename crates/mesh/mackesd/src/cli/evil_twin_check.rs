//! `EvilTwinCheck` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.

/// Handle the `evil-twin-check` subcommand.
#[allow(unreachable_code)]
pub fn run() -> anyhow::Result<()> {
    {
        use mackesd_core::surrounding_hosts::{
            evil_twin_suspects, learn_wifi, load_wifi_baseline, save_wifi_baseline,
            scan_wifi_bssids,
        };
        let scan = scan_wifi_bssids();
        let suspects = if let Some(data_dir) = dirs::data_dir() {
            let path = data_dir
                .join("mde")
                .join("surrounding")
                .join("wifi-baseline.json");
            let mut baseline = load_wifi_baseline(&path);
            let suspects = evil_twin_suspects(&scan, &baseline);
            learn_wifi(&mut baseline, &scan); // detect-then-learn
            let _ = save_wifi_baseline(&path, &baseline);
            suspects
        } else {
            Vec::new()
        };
        for (ssid, bssid) in &suspects {
            println!("{ssid}\t{bssid}");
        }
        if !suspects.is_empty() {
            eprintln!(
                "EVIL-TWIN: {} known SSID(s) on unexpected BSSIDs",
                suspects.len()
            );
            std::process::exit(1);
        }
    }
    Ok(())
}
