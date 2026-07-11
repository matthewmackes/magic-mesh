//! `Events` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `events` subcommand.
#[allow(unreachable_code)]
pub fn run(cmd: EventsCmd, db_path: PathBuf) -> anyhow::Result<()> {
    {
        // CB-1.8 mesh_history follow-up — audit-log
        // viewer surface.
        let conn = mackesd_core::store::open(&db_path)
            .with_context(|| format!("opening store at {}", db_path.display()))?;
        match cmd {
            EventsCmd::List { json } => {
                let rows = mackesd_core::store::load_audit_rows(&conn)
                    .context("loading events from store")?;
                let serial: Vec<serde_json::Value> = rows
                    .into_iter()
                    .map(|r| {
                        let payload_str = String::from_utf8(r.payload).unwrap_or_default();
                        serde_json::json!({
                            "event_id":     r.event_id,
                            "timestamp_ms": r.timestamp_ms,
                            "payload":      payload_str,
                            "hash":         hex_encode(&r.hash),
                        })
                    })
                    .collect();
                if json {
                    println!("{}", serde_json::to_string_pretty(&serial)?);
                } else if serial.is_empty() {
                    println!("(audit chain empty — no events yet)");
                } else {
                    for r in &serial {
                        let id = r.get("event_id").and_then(|v| v.as_u64()).unwrap_or(0);
                        let ts = r.get("timestamp_ms").and_then(|v| v.as_i64()).unwrap_or(0);
                        let payload = r.get("payload").and_then(|v| v.as_str()).unwrap_or("");
                        println!("{id:>8}  {ts}  {payload}");
                    }
                }
            }
        }
    }
    Ok(())
}

/// Lowercase hex string of a fixed byte slice. Avoids the
/// hex crate dep for one helper.
fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}
