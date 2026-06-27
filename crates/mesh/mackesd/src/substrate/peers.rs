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

use etcd_client::{Client, GetOptions, PutOptions};

use mackes_mesh_types::peers::PeerRecord;

use super::etcd::{connect, peer_key, PEERS_PREFIX, PEER_LEASE_TTL_S};

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
/// (`mesh_dns`, health_reconciler). Used by `peers` and `leader`.
pub(super) fn block_on<F>(fut: F) -> Option<F::Output>
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
    // `block_in_place` yields this worker to the pool while we join the thread.
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

/// The canonical peer directory for this node: the **etcd** substrate when the
/// coordination plane is provisioned (`/etc/mackesd/etcd-endpoints` non-empty),
/// else the replicated **fs** union (`<workgroup_root>/peers/*.json`). This is the
/// etcd-first-with-fs-fallback precedence the directory responder
/// ([`crate::ipc::directory`]), the health reconciler, and the lighthouse probe
/// already use — centralized here so every reader sees the same canonical
/// directory. SUBSTRATE/HA fix: the enroll roster + nebula supervisor reconcile
/// MUST read through this, not the fs union directly, or they go blind to live
/// etcd rows (a new lighthouse) on a cut-over node.
#[must_use]
pub fn read_directory(workgroup_root: &std::path::Path) -> Vec<PeerRecord> {
    let eps = crate::substrate::etcd::default_endpoints();
    if !eps.is_empty() {
        if let Some(rows) = read_peers_blocking(&eps) {
            return rows;
        }
    }
    mackes_mesh_types::peers::read_peers(&mackes_mesh_types::peers::peers_dir(workgroup_root))
}
