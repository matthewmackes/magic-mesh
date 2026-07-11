//! `Profiles` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.

/// Handle the `profiles` subcommand.
#[allow(unreachable_code)]
pub fn run(
    json: bool,
    set: Option<String>,
    rm: Option<String>,
    role: Option<String>,
    description: String,
    tags: Vec<String>,
    ks_fragments: Vec<String>,
    auto_join: bool,
) -> anyhow::Result<()> {
    {
        // PLANES-21 — the install-profile catalog (core pack + TOML).
        use mackesd_core::install_profiles;
        let root = mackesd_core::default_qnm_shared_root();
        // W56 write side — delete first (so --rm is unambiguous), else
        // --set writes/overwrites a validated profile TOML.
        if let Some(name) = rm {
            match install_profiles::delete_profile(&name, &root) {
                Ok(true) => println!("removed profile '{name}'"),
                Ok(false) => {
                    println!("no on-disk profile '{name}' (core profiles have no TOML)")
                }
                Err(e) => {
                    eprintln!("mackesd profiles: rm '{name}' failed: {e}");
                    std::process::exit(1);
                }
            }
            return Ok(());
        }
        if let Some(name) = set {
            let Some(role) = role else {
                eprintln!("mackesd profiles --set requires --role <lighthouse|workstation>");
                std::process::exit(1);
            };
            let profile = install_profiles::InstallProfile {
                name,
                description,
                role,
                tags: tags.into_iter().collect(),
                ks_fragments,
                auto_join,
            };
            match install_profiles::write_profile(&profile, &root) {
                Ok(p) => println!("wrote profile '{}' → {}", profile.name, p.display()),
                Err(e) => {
                    eprintln!("mackesd profiles: set failed: {e}");
                    std::process::exit(1);
                }
            }
            return Ok(());
        }
        let profiles = install_profiles::load_profiles(&root);
        if json {
            println!("{}", serde_json::to_string(&profiles)?);
        } else {
            println!(
                "{:<14} {:<12} {:<22} {:<9}",
                "PROFILE", "ROLE", "TAGS", "AUTO-JOIN"
            );
            for p in &profiles {
                println!(
                    "{:<14} {:<12} {:<22} {:<9}",
                    p.name,
                    p.role,
                    p.tags.iter().cloned().collect::<Vec<_>>().join(","),
                    p.auto_join
                );
            }
        }
        return Ok(());
    }
    Ok(())
}
