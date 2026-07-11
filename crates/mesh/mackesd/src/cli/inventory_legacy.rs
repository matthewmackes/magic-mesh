//! `InventoryLegacy` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `inventory-legacy` subcommand.
#[allow(unreachable_code)]
pub fn run(mesh_only: bool, json: bool) -> anyhow::Result<()> {
    {
        // Phase 12.13.1 — read-only walk of the three legacy
        // roots. Operator runs this before `import-legacy` to
        // see what's on disk.
        let roots = mackesd_core::legacy_inventory::default_roots();
        let mut artifacts = mackesd_core::legacy_inventory::inventory(&roots);
        if mesh_only {
            artifacts.retain(|a| a.mesh_data);
        }
        if json {
            println!("{}", serde_json::to_string_pretty(&artifacts)?);
        } else {
            print_inventory_table(&artifacts);
        }
    }
    #[cfg(feature = "async-services")]
    Ok(())
}

/// Render a fixed-width inventory table to stdout. Columns:
/// kind / mesh? / size / mtime (ISO-8601 UTC) / path. We pad to the
/// widest cell in each column so the output stays grep-able.
fn print_inventory_table(artifacts: &[mackesd_core::legacy_inventory::LegacyArtifact]) {
    if artifacts.is_empty() {
        println!("(no legacy artifacts found)");
        return;
    }
    let mut rows: Vec<[String; 5]> = Vec::with_capacity(artifacts.len() + 1);
    rows.push([
        "KIND".to_owned(),
        "MESH".to_owned(),
        "SIZE".to_owned(),
        "MTIME (UTC)".to_owned(),
        "PATH".to_owned(),
    ]);
    for a in artifacts {
        rows.push([
            format!("{:?}", a.artifact_kind),
            if a.mesh_data {
                "yes".to_owned()
            } else {
                "no".to_owned()
            },
            format_size(a.size_bytes),
            format_mtime(a.mtime_ms),
            a.path.display().to_string(),
        ]);
    }
    let mut widths = [0usize; 5];
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.len());
        }
    }
    for row in &rows {
        println!(
            "{:<w0$}  {:<w1$}  {:>w2$}  {:<w3$}  {}",
            row[0],
            row[1],
            row[2],
            row[3],
            row[4],
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2],
            w3 = widths[3],
        );
    }
}

/// Render a byte count as a short human-friendly string (binary
/// prefixes — KiB / MiB / GiB).
fn format_size(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let n = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KiB", n / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MiB", n / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GiB", n / (1024.0 * 1024.0 * 1024.0))
    }
}

/// Render an mtime (ms since epoch) as an ISO-8601 UTC timestamp.
/// Falls back to `-` when chrono refuses the value.
fn format_mtime(ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms).map_or_else(
        || "-".to_owned(),
        |dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
    )
}
