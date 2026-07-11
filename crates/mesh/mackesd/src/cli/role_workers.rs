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
        // MEDIA-1 — the Lighthouse_Media subclass adds its capability worker
        // on top of the lighthouse rank set; list it so the media gate is
        // observable from the CLI alongside the plain roles.
        let show_media = || {
            let class = mackesd_core::worker_role::DeployClass {
                rank: mde_role::Role::Lighthouse.rank(),
                media: true,
            };
            let mut names = mackesd_core::worker_role::workers_for_class(class);
            names.sort_unstable();
            println!(
                "lighthouse_media (rank 0 + media) runs {} workers:",
                names.len()
            );
            for n in names {
                println!("  {n}");
            }
        };
        match role {
            Some(s) if s.eq_ignore_ascii_case("lighthouse_media") => show_media(),
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
                show_media();
            }
        }
    }
    Ok(())
}
