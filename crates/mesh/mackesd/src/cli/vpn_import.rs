//! `VpnImport` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.

/// Handle the `vpn-import` subcommand.
#[allow(unreachable_code)]
pub fn run(name: String, kind: String, file: std::path::PathBuf) -> anyhow::Result<()> {
    {
        use mackesd_core::nebula_topology::{write_vpn_profile, VpnKind, VpnProfile};
        validate_profile_name(&name)?;
        let root = mackesd_core::default_qnm_shared_root();
        let kind = match kind.to_ascii_lowercase().as_str() {
            "wireguard" | "wg" => VpnKind::Wireguard,
            "openvpn" | "ovpn" => VpnKind::Openvpn,
            other => anyhow::bail!("unknown VPN kind `{other}` — expected wireguard|openvpn"),
        };
        let config = std::fs::read_to_string(&file)
            .map_err(|e| anyhow::anyhow!("read {}: {e}", file.display()))?;
        let path = write_vpn_profile(
            &root,
            &VpnProfile {
                name: name.clone(),
                kind,
                config,
            },
        )?;
        println!("imported VPN client profile `{name}` → {}", path.display());
        let all = mackesd_core::nebula_topology::list_vpn_profiles(&root);
        println!("stored client profiles ({}):", all.len());
        for (n, k) in all {
            println!("  - {n} ({k:?})");
        }
        return Ok(());
    }
    Ok(())
}

/// Keep the profile name a single filename component before it reaches the
/// shared-root writer. The writer appends the protocol extension to this value,
/// so accepting separators would let a root CLI invocation escape the
/// `topology/vpn-profiles` directory.
fn validate_profile_name(name: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !name.is_empty()
            && name != "."
            && name != ".."
            && !name.contains(['/', '\\'])
            && !name.chars().any(char::is_control),
        "VPN profile name must be a single safe filename component"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_profile_name;

    #[test]
    fn profile_name_rejects_path_escape_components() {
        for name in ["../escape", "nested/name", "/tmp/absolute", r"..\escape"] {
            assert!(
                validate_profile_name(name).is_err(),
                "unsafe profile name accepted: {name:?}"
            );
        }
        assert!(validate_profile_name("branch-office").is_ok());
    }
}
