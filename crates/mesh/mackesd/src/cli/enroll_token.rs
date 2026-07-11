//! `EnrollToken` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `enroll-token` subcommand.
#[allow(unreachable_code)]
pub fn run(mesh_id: String, lighthouse: Option<String>, note: String) -> anyhow::Result<()> {
    {
        let root = mackesd_core::default_qnm_shared_root();
        let bearer = mackesd_core::bearer_ledger::issue(&root, &note)
            .map_err(|e| anyhow::anyhow!("minting bearer: {e}"))?;
        // DAR-19 / XPA-5 — default to THIS lighthouse's PUBLIC address at
        // the dedicated `/enroll` HTTPS port, never the overlay IP at
        // Nebula's UDP data-plane port (`:4242`). A not-yet-enrolled node
        // can reach neither the overlay IP (it isn't on the mesh yet) nor
        // `:4242` (not an HTTP(S) service) — it needs the public
        // `nebula_enroll_listener` (default `:4243`), the one endpoint
        // that works pre-overlay (docs/design/magic-onboarding.md).
        let external_addr = mackesd_core::lighthouse_addr::read_external_addr();
        let host = resolve_enroll_endpoint_host(lighthouse.as_deref(), external_addr.as_deref())
            .map_or_else(detect_primary_ipv4, Ok)?;
        let port = mackesd_core::nebula_enroll_endpoint::DEFAULT_ENROLL_PORT;
        // Pin the on-disk `/enroll` endpoint cert fingerprint when one
        // exists (a `found`-bootstrapped lighthouse) so `join` takes the
        // network path (ONBOARD-2) instead of the QNM-Shared co-located
        // fallback — which needs the peer already on the overlay and is
        // exactly the trap DAR-19 hit ("no bundle in 30s"). A
        // `mesh-init`-only node has no endpoint cert yet; the token still
        // mints (without `?fp=`), matching `join`'s documented fallback
        // for a fingerprint-less token.
        let cert_path = mackesd_core::workers::nebula_enroll_listener::DEFAULT_CERT_PATH;
        let fp = std::fs::read(cert_path).ok().and_then(|pem| {
            mackesd_core::nebula_enroll_endpoint::endpoint_fingerprint_from_pem(&pem)
        });
        if fp.is_none() {
            eprintln!(
                "enroll-token: no /enroll endpoint cert at {cert_path} yet — minting a \
                     token with no ?fp=; a non-overlay box needs QNM-Shared already reachable \
                     with this shape. Run `mackesd found` (or otherwise stand up the /enroll \
                     endpoint) and use `mackesd add-peer` for a network-joinable token."
            );
        }
        let token = mackesd_core::nebula_enroll::JoinToken {
            mesh_id,
            lighthouse: host,
            port,
            bearer,
            fp,
        }
        .encode();
        println!("{token}");
        eprintln!(
            "single-use token minted (ENT-1) — run on the joining box:\n  mackesd join '{token}'"
        );
        return Ok(());
    }
    Ok(())
}
