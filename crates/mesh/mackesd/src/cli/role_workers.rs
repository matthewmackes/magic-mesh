//! `RoleWorkers` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.

/// Handle the `role-workers` subcommand.
#[allow(unreachable_code)]
pub fn run(role: Option<String>) -> anyhow::Result<()> {
    {
        let show = |r: mde_role::Role| {
            let mut names = mackesd_core::worker_role::workers_for_rank(r.rank());
            names.sort_unstable();
            println!("{} (rank {}) runs {} workers:", r, r.rank(), names.len());
            for n in names {
                println!("  {n}");
            }
        };
        match role {
            Some(s) if s.eq_ignore_ascii_case("lighthouse_media") => {
                anyhow::bail!(
                    "lighthouse_media is retired; lighthouses are thin control-plane nodes"
                )
            }
            Some(s) => match s.parse::<mde_role::Role>() {
                Ok(r) => show(r),
                Err(e) => {
                    eprintln!("mackesd role-workers: {e}");
                    std::process::exit(1);
                }
            },
            None => {
                for r in mde_role::Role::all() {
                    show(r);
                }
            }
        }
    }
    Ok(())
}
