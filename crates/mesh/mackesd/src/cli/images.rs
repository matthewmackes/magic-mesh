//! `Images` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.

/// Handle the `images` subcommand.
#[allow(unreachable_code)]
pub fn run(
    json: bool,
    record: bool,
    build: bool,
    name: Option<String>,
    kind: Option<String>,
    version: Option<String>,
    size_bytes: Option<u64>,
    profile: Option<String>,
) -> anyhow::Result<()> {
    {
        // PLANES-22 — the four buildable kinds, each with its
        // versioned builds present on the Syncthing share (W53/W55).
        use mackesd_core::image_catalog::{self, ImageKind};
        let root = mackesd_core::default_qnm_shared_root();
        // W54 — build the artifact now (then record it). Runs the real
        // per-kind tool; gated to execution-tagged nodes when launched
        // via the jobs engine.
        if build {
            let (Some(name), Some(kind_s), Some(version)) =
                (name.clone(), kind.clone(), version.clone())
            else {
                eprintln!("mackesd images --build requires --name, --kind, and --version");
                std::process::exit(1);
            };
            let Some(image_kind) = ImageKind::parse(&kind_s) else {
                eprintln!("mackesd images --build: unknown kind '{kind_s}' (iso|vm|container|usb)");
                std::process::exit(1);
            };
            use mackesd_core::image_build::{build_image, now_ms, BuildInputs, SubprocessBuild};
            let runner = SubprocessBuild::new(BuildInputs::default());
            match build_image(
                &runner,
                &root,
                image_kind,
                &name,
                &version,
                profile.clone(),
                now_ms(),
            ) {
                Ok(m) => println!(
                    "built {} {} v{} ({} bytes) — manifest recorded",
                    m.kind,
                    m.name,
                    m.version,
                    m.size_bytes.unwrap_or(0)
                ),
                Err(e) => {
                    eprintln!("mackesd images --build: {e}");
                    std::process::exit(1);
                }
            }
            return Ok(());
        }
        // W55 — register a completed build's manifest.
        if record {
            let (Some(name), Some(kind), Some(version)) = (name, kind, version) else {
                eprintln!("mackesd images --record requires --name, --kind, and --version");
                std::process::exit(1);
            };
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64);
            let manifest = image_catalog::ImageManifest {
                name,
                kind,
                version,
                built_at_ms: Some(now_ms),
                size_bytes,
                profile,
            };
            match image_catalog::record_manifest(&manifest, &root) {
                Ok(p) => println!(
                    "recorded {} {} v{} → {}",
                    manifest.kind,
                    manifest.name,
                    manifest.version,
                    p.display()
                ),
                Err(e) => {
                    eprintln!("mackesd images: record failed: {e}");
                    std::process::exit(1);
                }
            }
            return Ok(());
        }
        let manifests = image_catalog::load_manifests(&root);
        let rows: Vec<serde_json::Value> = ImageKind::all()
            .iter()
            .map(|kind| {
                let builds: Vec<serde_json::Value> = manifests
                    .iter()
                    .filter(|m| m.kind == kind.as_str())
                    .map(|m| {
                        serde_json::json!({
                            "name": m.name,
                            "version": m.version,
                            "built_at_ms": m.built_at_ms,
                            "size_bytes": m.size_bytes,
                            "profile": m.profile,
                        })
                    })
                    .collect();
                serde_json::json!({
                    "kind": kind.as_str(),
                    "label": kind.label(),
                    "description": kind.description(),
                    "builds": builds,
                })
            })
            .collect();
        if json {
            println!("{}", serde_json::to_string(&rows)?);
        } else {
            for kind in ImageKind::all() {
                let n = manifests.iter().filter(|m| m.kind == kind.as_str()).count();
                println!(
                    "{:<18} {} build(s) — {}",
                    kind.label(),
                    n,
                    kind.description()
                );
                for m in manifests.iter().filter(|m| m.kind == kind.as_str()) {
                    println!("    {} v{}", m.name, m.version);
                }
            }
        }
        return Ok(());
    }
    Ok(())
}
