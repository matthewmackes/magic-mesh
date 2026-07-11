//! `AuditLog` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.

/// Handle the `audit-log` subcommand.
#[allow(unreachable_code)]
pub fn run(event: String, detail: String) -> anyhow::Result<()> {
    {
        use mackesd_core::audit_log::write_audit_event;
        if let Some(data_dir) = dirs::data_dir() {
            let activity_root = data_dir.join("mde").join("activity");
            match write_audit_event(&activity_root, &event, &detail) {
                Ok(path) => println!("{}", path.display()),
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            }
        }
    }
    Ok(())
}
