//! `CutoverAudit` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `cutover-audit` subcommand.
#[allow(unreachable_code)]
pub fn run(repo_root: PathBuf, vm_rebuild_ledger: PathBuf, json: bool) -> anyhow::Result<()> {
    {
        let report = mackesd_core::cutover_audit::audit_cutover(&repo_root, &vm_rebuild_ledger);
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "ok": report.ok(),
                    "failures": report.failures(),
                    "repo_root": report.repo_root,
                    "vm_rebuild_ledger": report.vm_rebuild_ledger,
                    "checks": report.checks,
                })
            );
        } else {
            println!(
                "QC-15 cutover audit: {}",
                if report.ok() { "clean" } else { "FAILED" }
            );
            for check in &report.checks {
                let mark = match check.status {
                    mackesd_core::cutover_audit::CutoverAuditStatus::Pass => "ok",
                    mackesd_core::cutover_audit::CutoverAuditStatus::Fail => "fail",
                };
                println!("{mark:<4} {:<32} {}", check.id, check.detail);
            }
        }
        if !report.ok() {
            std::process::exit(1);
        }
    }
    Ok(())
}
