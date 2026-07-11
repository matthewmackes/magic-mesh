//! `AuditVerify` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `audit-verify` subcommand.
#[allow(unreachable_code)]
pub fn run(json: bool, db_path: PathBuf) -> anyhow::Result<()> {
    {
        // Reads every row from the `events` table (ordered by
        // `seq` ASC) and walks the SHA-256 hash chain.
        let conn = mackesd_core::store::open(&db_path)
            .with_context(|| format!("opening store at {}", db_path.display()))?;
        let rows =
            mackesd_core::store::load_audit_rows(&conn).context("loading events from store")?;
        let outcome = mackesd_core::audit::verify(&rows);
        if json {
            // PLANES-12 — the Audit panel's data: the verify verdict
            // plus the 72 h rolling window of events (W44/W45).
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as i64);
            let window_ms: i64 = 72 * 3600 * 1000;
            let timeline: Vec<serde_json::Value> = rows
                .iter()
                .filter(|r| now_ms.saturating_sub(r.timestamp_ms) <= window_ms)
                .map(|r| {
                    serde_json::json!({
                        "event_id": r.event_id,
                        "timestamp_ms": r.timestamp_ms,
                        "payload": String::from_utf8_lossy(&r.payload),
                        "hash": r.hash.iter().map(|b| format!("{b:02x}")).collect::<String>(),
                    })
                })
                .collect();
            let (status, detail) = match &outcome {
                mackesd_core::audit::VerifyOutcome::Empty => ("empty", String::new()),
                mackesd_core::audit::VerifyOutcome::Intact { verified, .. } => {
                    ("intact", format!("{verified} events"))
                }
                mackesd_core::audit::VerifyOutcome::Break { at_event, .. } => {
                    ("break", format!("at event {at_event}"))
                }
            };
            println!(
                "{}",
                serde_json::json!({
                    "verify": status,
                    "detail": detail,
                    "total_events": rows.len(),
                    "retained_72h": timeline.len(),
                    "timeline": timeline,
                })
            );
            if status == "break" {
                std::process::exit(1);
            }
        } else {
            match outcome {
                mackesd_core::audit::VerifyOutcome::Empty => {
                    println!("audit chain empty (no events yet)");
                }
                mackesd_core::audit::VerifyOutcome::Intact { verified, .. } => {
                    println!("verified {verified} events  ·  chain intact");
                }
                mackesd_core::audit::VerifyOutcome::Break { at_event, .. } => {
                    eprintln!("audit chain BREAK at event {at_event}");
                    std::process::exit(1);
                }
            }
        }
    }
    Ok(())
}
