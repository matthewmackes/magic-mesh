//! `Tag` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.

/// Handle the `tag` subcommand.
#[allow(unreachable_code)]
pub fn run(host: Option<String>, set: Option<String>) -> anyhow::Result<()> {
    {
        let root = mackesd_core::default_qnm_shared_root();
        let target = host.unwrap_or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "unknown".to_string())
        });
        use mackes_mesh_types::cap_tags::{read_tags, write_tags, CapabilityTag, NodeTags};
        if let Some(spec) = set {
            let mut tags = NodeTags::default();
            for tok in spec.split(',').map(str::trim).filter(|t| !t.is_empty()) {
                match CapabilityTag::parse(tok) {
                    Some(t) => {
                        tags.tags.insert(t);
                    }
                    None => anyhow::bail!(
                        "unknown capability tag `{tok}` — expected hop|execution|headless"
                    ),
                }
            }
            write_tags(&root, &target, &tags)?;
            // W83 — audit the change (security-relevant fleet edit).
            tracing::info!(
                target: "mackesd::audit",
                event = "cap_tags.set",
                host = %target,
                tags = %spec,
                "PLANES-3: capability tags updated"
            );
            println!("tags for {target}: {}", spec);
        } else {
            let tags = read_tags(&root, &target);
            let names: Vec<&str> = tags.tags.iter().map(|t| t.as_str()).collect();
            println!(
                "tags for {target}: {}",
                if names.is_empty() {
                    "(none)".to_string()
                } else {
                    names.join(", ")
                }
            );
        }
        return Ok(());
    }
    Ok(())
}
