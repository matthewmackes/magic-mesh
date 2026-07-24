//! `Tag` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TAG_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Validate the host key before it reaches the replicated tag store.
///
/// Tag files are named `node-tags/<host>.json`. Keep the accepted value as a
/// single host component so a caller cannot turn the host key into a path (or
/// make privileged tag readers consume a file outside the target node's tag
/// entry). Mesh identifiers are intentionally not reduced to a narrow DNS-only
/// grammar here: deployed names include dotted, mixed-case, underscored, and
/// `peer:`-prefixed identifiers. The path boundary is the security invariant
/// this CLI owns.
fn validate_target_host(host: &str) -> anyhow::Result<()> {
    if host.is_empty() || host.trim() != host {
        anyhow::bail!(
            "invalid tag target host: must be non-empty and contain no surrounding whitespace"
        );
    }
    if host == "." || host == ".." {
        anyhow::bail!("invalid tag target host `{host}`: traversal component");
    }
    if host
        .chars()
        .any(|ch| ch == '/' || ch == '\\' || ch.is_control())
    {
        anyhow::bail!("invalid tag target host `{host}`: must be one path component");
    }

    let mut components = Path::new(host).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        anyhow::bail!("invalid tag target host `{host}`: must be one normal path component");
    }
    Ok(())
}

/// The shared tag store is replicated authority. Keep its CLI writer from
/// following a hostile directory component or predictable temp-file symlink.
fn write_tags_checked(
    root: &Path,
    target: &str,
    tags: &mackes_mesh_types::cap_tags::NodeTags,
) -> std::io::Result<PathBuf> {
    use mackes_mesh_types::cap_tags::{tags_dir, CapabilityTag};

    if tags.tags.contains(&CapabilityTag::Media) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "media capability is retired; lighthouse nodes are thin",
        ));
    }
    let dir = tags_dir(root);
    let path = dir.join(format!("{target}.json"));
    write_replicated_public(&path, serde_json::to_vec_pretty(tags)?.as_slice())?;
    Ok(path)
}

fn write_replicated_public(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no parent")
    })?;
    reject_symlinked_parents(path)?;
    std::fs::create_dir_all(parent)?;
    reject_symlinked_parents(path)?;
    let leaf = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid filename"))?;
    let mut created = None;
    for _ in 0..16 {
        let nonce = TAG_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = parent.join(format!(".{leaf}.tmp.{}.{}", std::process::id(), nonce));
        match std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o644)
            .open(&tmp)
        {
            Ok(file) => {
                created = Some((tmp, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    let (tmp, mut file) = created.ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::AlreadyExists, "temp-file collision")
    })?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)?;
        std::fs::File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn reject_symlinked_parents(path: &Path) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no parent")
    })?;
    let mut current = PathBuf::new();
    for component in parent.components() {
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!(
                        "refusing symlinked replicated directory {}",
                        current.display()
                    ),
                ));
            }
            Ok(meta) if !meta.is_dir() => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotADirectory,
                    format!(
                        "replicated path component is not a directory: {}",
                        current.display()
                    ),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Handle the `tag` subcommand.
#[allow(unreachable_code)]
pub fn run(host: Option<String>, set: Option<String>) -> anyhow::Result<()> {
    {
        let target = host.unwrap_or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "unknown".to_string())
        });
        // Validate before resolving or touching the tag store. Both read and
        // write paths below use this exact, already-checked target.
        validate_target_host(&target)?;
        let root = mackesd_core::default_qnm_shared_root();
        use mackes_mesh_types::cap_tags::{read_tags, CapabilityTag, NodeTags};
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
            write_tags_checked(&root, &target, &tags)?;
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

#[cfg(test)]
mod tests {
    use super::validate_target_host;

    #[test]
    fn accepts_mesh_hostname_forms_as_single_components() {
        for host in [
            "oak",
            "mcnf-build-52",
            "Lighthouse-02",
            "localhost.localdomain",
            "grafana.services.matthewmackes.com",
            "PS4-64F7B2.local",
            "node_name",
            "peer:anvil",
            "peer:localhost.localdomain",
        ] {
            assert!(
                validate_target_host(host).is_ok(),
                "mesh hostname should remain accepted: {host}"
            );
        }
    }

    #[test]
    fn rejects_host_keys_that_can_escape_the_tag_directory() {
        for host in [
            "",
            ".",
            "..",
            "../outside",
            "../../node-tags/other",
            "/absolute/target",
            "node/../../target",
            r"node\..\target",
            "node/",
            "node\0target",
            "node\nother",
            " node",
            "node ",
        ] {
            assert!(
                validate_target_host(host).is_err(),
                "hostile host key should be rejected: {host:?}"
            );
        }
    }
}
