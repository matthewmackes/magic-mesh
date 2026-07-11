//! `Healthz` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `healthz` subcommand.
#[allow(unreachable_code)]
pub fn run(db_path: PathBuf) -> anyhow::Result<()> {
    {
        // EFF-8 — live report off the store: real node counts +
        // health buckets + audit-chain status (was a hardcoded
        // `empty()` baseline). On a fresh peer whose store hasn't
        // migrated yet this still degrades to the zero-node report.
        // (`is_leader`/`applied_revision` remain at defaults pending
        // the leader-lease + applied-revision query plumbing.)
        let report = match mackesd_core::store::open(&db_path) {
            Ok(conn) => mackesd_core::health::HealthReport::from_store(&conn),
            Err(_) => mackesd_core::health::HealthReport::empty(),
        };
        // OB6-FIX-4 — node_count/health-buckets/is_leader from the LIVE
        // directory + leader lease (the store nodes table read 0 on peers).
        let root = mackesd_core::default_qnm_shared_root();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let svc = mackesd_core::ipc::directory::DirectoryService::new(&root, Some(db_path.clone()));
        let (n, healthy, degraded, unreachable, is_leader, lighthouses) =
            svc.mesh_health_counts(&default_node_id(), now_ms);
        let report = report.with_mesh(n, healthy, degraded, unreachable, is_leader, lighthouses);
        println!("{}", report.to_json_line()?);
    }
    Ok(())
}
