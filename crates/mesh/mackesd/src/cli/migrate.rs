//! `Migrate` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `migrate` subcommand.
#[allow(unreachable_code)]
pub fn run(db_path: PathBuf) -> anyhow::Result<()> {
    {
        let conn = mackesd_core::store::open(&db_path)
            .with_context(|| format!("opening store at {}", db_path.display()))?;
        let n = mackesd_core::store::applied_migration_count(&conn)?;
        tracing::info!("store at {} migrated (n={})", db_path.display(), n);
        println!("{n} migrations applied");
    }
    Ok(())
}
