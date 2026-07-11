//! `WakePeer` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `wake-peer` subcommand.
#[allow(unreachable_code)]
pub fn run(
    mac: String,
    broadcast: String,
    via_lighthouse: Option<String>,
    port: u16,
) -> anyhow::Result<()> {
    {
        // DEAD-2.5 + NF-21.2 — wire mackesd_core::workers::wol so
        // the Rust port has a runtime entry point. Replaces the
        // retired Python `mesh_wol.wake_peer` for the MAC-already-
        // known case; hostname resolution is the operator's job
        // until a PeerStore lookup helper lands. `--via-lighthouse`
        // routes through a lighthouse's overlay IP for WoL-across-
        // LANs (NF-21.2).
        let Some(mac_bytes) = mackesd_core::workers::wol::normalize_mac(&mac) else {
            anyhow::bail!("wake-peer: could not parse MAC {mac:?}");
        };
        if let Some(lighthouse_ip) = via_lighthouse.as_deref() {
            mackesd_core::workers::wol::wake_via_lighthouse(mac_bytes, lighthouse_ip, port)
                .context("wake-peer: send magic packet via lighthouse")?;
            println!(
                "wake-peer: sent magic packet for {mac} via lighthouse \
                     {lighthouse_ip}:{port}"
            );
        } else {
            mackesd_core::workers::wol::wake(mac_bytes, &broadcast, port)
                .context("wake-peer: send magic packet")?;
            println!("wake-peer: sent magic packet to {mac} via {broadcast}:{port}");
        }
    }
    // AUD3 S-3 (2026-06-12): `Cmd::PeerCard` arm removed with the
    // peer_join module (targeted the deleted mde-peer-card modal).
    #[cfg(feature = "async-services")]
    Ok(())
}
