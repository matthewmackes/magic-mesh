//! SUBSTRATE-3 (SUBSTRATE-V2) — the peer directory on etcd.
//!
//! Each node writes its own `PeerRecord` to `/mesh/peers/<hostname>` under a
//! keepalive lease (`PEER_LEASE_TTL_S` ≈ 90 s), refreshed every heartbeat — so
//! **liveness is the lease**, not a `last_seen_ms` staleness guess: a dead node's
//! record auto-deletes when its lease expires. `read_peers` is an etcd range get
//! over the `/mesh/peers/` prefix. The `PeerRecord` JSON shape is unchanged, so
//! every consumer (the directory RPC, the panels) keeps working.
//!
//! The blocking wrappers ([`put_peer_blocking`]/[`read_peers_blocking`]) let the
//! sync heartbeat thread + the dedicated directory responder thread use etcd
//! without an ambient tokio runtime (they build a private current-thread one —
//! safe because both callers run OFF the tokio executor).

use etcd_client::{Client, GetOptions, PutOptions, Txn, TxnOp};

use mackes_mesh_types::peers::{OverlayIdentityClaim, PeerRecord};

use super::etcd::{connect, peer_key, PEERS_PREFIX, PEER_LEASE_TTL_S};

/// Lease-backed namespace for collision-detectable physical-machine and boot
/// claims. The public certificate fingerprint groups one Nebula identity;
/// machine and boot digests are separate key components so copied identities
/// coexist and remain observable instead of overwriting one hostname row.
pub const OVERLAY_IDENTITY_CLAIMS_PREFIX: &str = "/mesh/overlay-identity-claims/v1/";
/// Exact byte length of every v1 claimant key: prefix plus three 64-byte
/// lowercase SHA-256 components and their two separators.
pub const OVERLAY_IDENTITY_CLAIM_KEY_BYTES: usize =
    OVERLAY_IDENTITY_CLAIMS_PREFIX.len() + (3 * 64) + 2;

#[derive(Debug, Clone, PartialEq, Eq)]
struct OverlayIdentityClaimPublication {
    key: String,
    value: String,
}

fn overlay_identity_claim_publication(
    rec: &PeerRecord,
    claim: &OverlayIdentityClaim,
) -> anyhow::Result<OverlayIdentityClaimPublication> {
    claim.validate()?;
    let expected_node_id = format!("peer:{}", rec.hostname);
    if claim.nebula_node_id != expected_node_id || claim.nebula_name != expected_node_id {
        anyhow::bail!(
            "peer hostname does not exactly match caller-supplied Nebula node and certificate name"
        );
    }
    if rec.overlay_ip.as_deref() != Some(claim.nebula_address.as_str()) {
        anyhow::bail!("peer overlay address does not match caller-supplied Nebula claim");
    }
    let key = format!(
        "{OVERLAY_IDENTITY_CLAIMS_PREFIX}{}/{}/{}",
        claim.certificate_fingerprint, claim.machine_claimant_digest, claim.boot_claimant_digest
    );
    if key.len() != OVERLAY_IDENTITY_CLAIM_KEY_BYTES {
        anyhow::bail!("overlay identity claim key escaped its exact v1 bound");
    }
    let value = claim.to_json()?;
    Ok(OverlayIdentityClaimPublication { key, value })
}

/// Write `rec` to `/mesh/peers/<hostname>` under a fresh `PEER_LEASE_TTL_S`
/// lease. Re-running each heartbeat keeps the record alive; stopping lets the
/// lease lapse and etcd delete the row (liveness = lease).
///
/// # Errors
/// A JSON-encode failure or an etcd lease-grant / put error.
pub async fn put_peer(client: &mut Client, rec: &PeerRecord) -> anyhow::Result<()> {
    let lease = client.lease_grant(PEER_LEASE_TTL_S, None).await?.id();
    let json = serde_json::to_string(rec)?;
    client
        .put(
            peer_key(&rec.hostname),
            json,
            Some(PutOptions::new().with_lease(lease)),
        )
        .await?;
    Ok(())
}

/// Atomically publish a peer directory row and its strict overlay claimant
/// under one fresh [`PEER_LEASE_TTL_S`] etcd lease.
///
/// This is the fail-closed collision-authority seam: the caller must supply a
/// fully validated public certificate identity plus privacy-bounded machine and
/// boot digests. No hostname or `PeerRecord` field is promoted into missing
/// identity authority. Repeating the same claimant refreshes the same key and
/// value; a different machine or boot using the copied Nebula identity receives
/// a distinct simultaneously-live key.
///
/// # Errors
/// Returns an error before granting a lease when the claim is malformed or its
/// exact node/name/address identity disagrees with the peer row, and propagates
/// JSON, lease, or atomic etcd transaction failures.
pub async fn put_peer_with_overlay_identity_claim(
    client: &mut Client,
    rec: &PeerRecord,
    claim: &OverlayIdentityClaim,
) -> anyhow::Result<()> {
    let publication = overlay_identity_claim_publication(rec, claim)?;
    let peer_json = serde_json::to_string(rec)?;
    let lease = client.lease_grant(PEER_LEASE_TTL_S, None).await?.id();
    let response = client
        .txn(Txn::new().and_then([
            TxnOp::put(
                peer_key(&rec.hostname),
                peer_json,
                Some(PutOptions::new().with_lease(lease)),
            ),
            TxnOp::put(
                publication.key,
                publication.value,
                Some(PutOptions::new().with_lease(lease)),
            ),
        ]))
        .await?;
    if !response.succeeded() {
        anyhow::bail!("overlay identity claim transaction was not applied");
    }
    Ok(())
}

/// Range-read every live peer record under `/mesh/peers/`, decoded + sorted by
/// hostname (matching `mackes_mesh_types::peers::read_peers`). Unparseable values
/// are skipped (never fatal — a future schema addition can't break a reader).
///
/// # Errors
/// An etcd range-get error.
pub async fn read_peers(client: &mut Client) -> anyhow::Result<Vec<PeerRecord>> {
    let resp = client
        .get(PEERS_PREFIX, Some(GetOptions::new().with_prefix()))
        .await?;
    let mut out: Vec<PeerRecord> = resp
        .kvs()
        .iter()
        .filter_map(|kv| kv.value_str().ok())
        .filter_map(|s| serde_json::from_str::<PeerRecord>(s).ok())
        .collect();
    out.sort_by(|a, b| a.hostname.cmp(&b.hostname));
    Ok(out)
}

/// Delete a peer's directory row (an explicit leave/unenroll; ordinarily the
/// lease handles departure). Idempotent.
///
/// # Errors
/// An etcd delete error.
pub async fn delete_peer(client: &mut Client, hostname: &str) -> anyhow::Result<()> {
    client.delete(peer_key(hostname), None).await?;
    Ok(())
}

/// Drive `fut` to completion from a synchronous context. Off the tokio executor
/// (the heartbeat std::thread / directory responder thread) it spins a private
/// current-thread runtime; ON the executor (a worker like `mesh_dns` that reached
/// a blocking bridge) it must NOT build a nested runtime — that panics ("Cannot
/// start a runtime from within a runtime") and on an etcd node crash-loops the
/// worker until ENT-6 circuit-breaks it. Returns `None` only when a private
/// runtime can't be built.
/// Shared substrate blocking bridge — runtime-aware so it is safe from BOTH a
/// plain std::thread (heartbeat/responder) and an async worker on the executor
/// (`mesh_dns`, health_reconciler). Used by `peers`, `leader`, and the
/// `workers::session_broker` lease-backed session store.
pub(crate) fn block_on<F>(fut: F) -> Option<F::Output>
where
    F: std::future::Future + Send,
    F::Output: Send,
{
    if tokio::runtime::Handle::try_current().is_err() {
        // Off the tokio executor (heartbeat / responder std::thread): a private
        // current-thread runtime drives `fut` directly.
        return tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()
            .map(|rt| rt.block_on(fut));
    }
    // ON the executor (a worker like `mesh_dns` reached a blocking bridge):
    // building OR entering a runtime here panics ("Cannot start a runtime from
    // within a runtime"). Drive `fut` on a FRESH OS thread that owns its own
    // current-thread runtime — that thread has no ambient runtime, so no nesting.
    //
    // `block_in_place` yields a worker from a multi-thread runtime while we
    // join the helper thread. It is itself invalid on a current-thread runtime,
    // though. The Nebula Bus responder intentionally uses that runtime because
    // its Persist/rusqlite state is !Send, so detect that flavor and use the
    // same helper thread without calling `block_in_place`.
    if matches!(
        tokio::runtime::Handle::current().runtime_flavor(),
        tokio::runtime::RuntimeFlavor::CurrentThread
    ) {
        return std::thread::scope(|s| {
            s.spawn(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .ok()
                    .map(|rt| rt.block_on(fut))
            })
            .join()
            .ok()
            .flatten()
        });
    }

    tokio::task::block_in_place(|| {
        std::thread::scope(|s| {
            s.spawn(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .ok()
                    .map(|rt| rt.block_on(fut))
            })
            .join()
            .ok()
            .flatten()
        })
    })
}

/// Blocking peer-record write to etcd (the heartbeat thread's bridge). `true` on
/// success; `false` on connect/put failure (the next heartbeat retries).
#[must_use]
pub fn put_peer_blocking(endpoints: &[String], rec: &PeerRecord) -> bool {
    block_on(async {
        match connect(endpoints).await {
            Ok(mut c) => put_peer(&mut c, rec).await.is_ok(),
            Err(_) => false,
        }
    })
    .unwrap_or(false)
}

/// Blocking bridge for [`put_peer_with_overlay_identity_claim`]. The required
/// claim argument makes missing collision authority an explicit `false` result;
/// callers cannot fall back to a hostname-derived or fabricated identity.
#[must_use]
pub fn put_peer_with_overlay_identity_claim_blocking(
    endpoints: &[String],
    rec: &PeerRecord,
    claim: &OverlayIdentityClaim,
) -> bool {
    block_on(async {
        match connect(endpoints).await {
            Ok(mut client) => put_peer_with_overlay_identity_claim(&mut client, rec, claim)
                .await
                .is_ok(),
            Err(_) => false,
        }
    })
    .unwrap_or(false)
}

/// Blocking peer-directory read from etcd (the directory responder's bridge).
/// `None` on connect/read failure so the caller can fall back to the fs union.
#[must_use]
pub fn read_peers_blocking(endpoints: &[String]) -> Option<Vec<PeerRecord>> {
    block_on(async {
        match connect(endpoints).await {
            Ok(mut c) => read_peers(&mut c).await.ok(),
            Err(_) => None,
        }
    })
    .flatten()
}

/// Blocking delete of a peer's `/mesh/peers/<hostname>` directory key (MIG-1 —
/// the decommission path's bridge). A deleted node's record otherwise lingers
/// until its lease lapses (or forever if it was written without one), so the
/// roster reconcile keeps re-adding a node whose droplet is already gone — the
/// stale-lighthouse entries we had to `etcdctl del` by hand during the
/// 2026-06-27 migration. `remove-peer` now drops the directory key directly.
/// `true` on a successful delete, `false` on connect/delete failure (idempotent
/// — deleting an absent key still succeeds).
#[must_use]
pub fn delete_peer_blocking(endpoints: &[String], hostname: &str) -> bool {
    block_on(async {
        match connect(endpoints).await {
            Ok(mut c) => delete_peer(&mut c, hostname).await.is_ok(),
            Err(_) => false,
        }
    })
    .unwrap_or(false)
}

/// MIG-2 — shared overlay-IP reservation keyspace: `/mesh/ipalloc/<ip>` = node_id.
pub const IPALLOC_PREFIX: &str = "/mesh/ipalloc/";

/// MIG-2 — record an overlay-IP assignment in etcd at SIGN time (best-effort), so
/// a concurrent sign on ANOTHER lighthouse sees the IP as taken immediately
/// rather than only after the new peer's first heartbeat lands its PeerRecord.
/// The peer directory is heartbeat-lagged, so without this two lighthouses
/// signing within the heartbeat window both saw the same directory and could pick
/// the same IP — the cross-lighthouse collision that handed a node 10.42.0.1 on
/// 2026-06-27. Idempotent overwrite. `true` on success.
#[must_use]
pub fn reserve_overlay_ip_blocking(endpoints: &[String], ip: &str, node_id: &str) -> bool {
    let key = format!("{IPALLOC_PREFIX}{ip}");
    let val = node_id.to_string();
    block_on(async {
        match connect(endpoints).await {
            Ok(mut c) => c.put(key, val, None).await.is_ok(),
            Err(_) => false,
        }
    })
    .unwrap_or(false)
}

/// MIG-2 — every overlay IP recorded under `/mesh/ipalloc/` (the sign-time
/// reservations). The enroll signer unions these with the peer-directory IPs to
/// form the global taken-set the allocator skips. Empty on connect/read failure
/// (the directory read still guards the common case). The keyed value is the
/// `<ip>` suffix of each reservation key.
#[must_use]
pub fn reserved_overlay_ips_blocking(endpoints: &[String]) -> std::collections::HashSet<String> {
    block_on(async {
        match connect(endpoints).await {
            Ok(mut c) => {
                let resp = c
                    .get(IPALLOC_PREFIX, Some(GetOptions::new().with_prefix()))
                    .await
                    .ok()?;
                Some(
                    resp.kvs()
                        .iter()
                        .filter_map(|kv| kv.key_str().ok())
                        .filter_map(|k| k.strip_prefix(IPALLOC_PREFIX).map(str::to_string))
                        .collect::<std::collections::HashSet<String>>(),
                )
            }
            Err(_) => None,
        }
    })
    .flatten()
    .unwrap_or_default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectorySource {
    Etcd,
    Filesystem,
}

/// The canonical peer directory for this node: the **etcd** substrate when the
/// coordination plane is provisioned (`/etc/mackesd/etcd-endpoints` non-empty),
/// else the replicated **fs** union (`<workgroup_root>/peers/*.json`). This is the
/// etcd-first-with-fs-fallback precedence the directory responder
/// ([`crate::ipc::directory`]), the health reconciler, and the lighthouse probe
/// already use — centralized here so every reader sees the same canonical
/// directory. SUBSTRATE/HA fix: the enroll roster + nebula supervisor reconcile
/// MUST read through this, not the fs union directly, or they go blind to live
/// etcd rows (a new lighthouse) on a cut-over node.
/// Read the canonical peer directory and identify which substrate supplied it.
/// Filesystem fallback must not be mistaken for authoritative membership when
/// configured etcd is temporarily unreachable.
#[must_use]
pub(crate) fn read_directory_with_source(
    workgroup_root: &std::path::Path,
) -> (Vec<PeerRecord>, DirectorySource) {
    let eps = crate::substrate::etcd::default_endpoints();
    if !eps.is_empty() {
        if let Some(rows) = read_peers_blocking(&eps) {
            return (rows, DirectorySource::Etcd);
        }
    }
    (
        mackes_mesh_types::peers::read_peers(&mackes_mesh_types::peers::peers_dir(workgroup_root)),
        DirectorySource::Filesystem,
    )
}

/// Read the canonical peer directory using etcd when provisioned and the
/// replicated filesystem union otherwise.
#[must_use]
pub fn read_directory(workgroup_root: &std::path::Path) -> Vec<PeerRecord> {
    read_directory_with_source(workgroup_root).0
}

#[cfg(test)]
mod tests {
    use super::{
        block_on, overlay_identity_claim_publication, OVERLAY_IDENTITY_CLAIMS_PREFIX,
        OVERLAY_IDENTITY_CLAIM_KEY_BYTES,
    };
    use mackes_mesh_types::peers::{OverlayIdentityClaim, PeerRecord};

    const CERT: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const MACHINE_A: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const MACHINE_B: &str = "2222222222222222222222222222222222222222222222222222222222222222";
    const BOOT_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const BOOT_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn peer() -> PeerRecord {
        let mut rec = PeerRecord::now("SURFACE", None, "healthy");
        rec.overlay_ip = Some("10.42.0.7".into());
        rec
    }

    fn claim(machine: &str, boot: &str) -> OverlayIdentityClaim {
        OverlayIdentityClaim::new(
            "peer:SURFACE",
            "peer:SURFACE",
            "10.42.0.7",
            CERT,
            machine,
            boot,
        )
        .expect("valid claim")
    }

    #[test]
    fn blocking_bridge_is_safe_inside_current_thread_runtime() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("current-thread runtime");

        let result = runtime.block_on(async { block_on(async { 7_u8 }) });

        assert_eq!(result, Some(7));
    }

    #[test]
    fn copied_identity_claimants_have_distinct_simultaneous_keys() {
        let first = overlay_identity_claim_publication(&peer(), &claim(MACHINE_A, BOOT_A))
            .expect("first publication");
        let second = overlay_identity_claim_publication(&peer(), &claim(MACHINE_B, BOOT_B))
            .expect("second publication");

        let live = std::collections::BTreeMap::from([
            (first.key.clone(), first.value),
            (second.key.clone(), second.value),
        ]);
        assert_eq!(live.len(), 2, "copied identities must never overwrite");
        assert_ne!(first.key, second.key);
        assert_eq!(first.key.len(), OVERLAY_IDENTITY_CLAIM_KEY_BYTES);
        assert_eq!(second.key.len(), OVERLAY_IDENTITY_CLAIM_KEY_BYTES);
        assert!(first.key.starts_with(OVERLAY_IDENTITY_CLAIMS_PREFIX));
        assert!(second.key.starts_with(OVERLAY_IDENTITY_CLAIMS_PREFIX));
        assert!(first.key.contains(CERT));
        assert!(second.key.contains(CERT));
    }

    #[test]
    fn same_claimant_refresh_is_key_and_value_idempotent() {
        let claim = claim(MACHINE_A, BOOT_A);
        let first = overlay_identity_claim_publication(&peer(), &claim).expect("first refresh");
        let second = overlay_identity_claim_publication(&peer(), &claim).expect("second refresh");

        assert_eq!(first, second);
    }

    #[test]
    fn publication_rejects_directory_address_mismatch() {
        let mut rec = peer();
        rec.overlay_ip = Some("10.42.0.8".into());

        assert!(overlay_identity_claim_publication(&rec, &claim(MACHINE_A, BOOT_A)).is_err());
    }

    #[test]
    fn publication_rejects_directory_identity_mismatch_at_same_address() {
        let other = OverlayIdentityClaim::new(
            "peer:OTHER",
            "peer:OTHER",
            "10.42.0.7",
            CERT,
            MACHINE_A,
            BOOT_A,
        )
        .expect("valid but different identity");

        assert!(overlay_identity_claim_publication(&peer(), &other).is_err());
    }
}
