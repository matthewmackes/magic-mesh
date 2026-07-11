//! `VpnImport` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.

/// Handle the `vpn-import` subcommand.
#[allow(unreachable_code)]
pub fn run(name: String, kind: String, file: std::path::PathBuf) -> anyhow::Result<()> {
    {
        use mackesd_core::nebula_topology::{write_vpn_profile, VpnKind, VpnProfile};
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
