//! SUBSTRATE-2 (SUBSTRATE-V2) — leader election on etcd.
//!
//! Replaces the `.mackesd-leader.lock` advisory-lockfile election
//! ([`crate::leader`]) with an etcd lease + compare-and-swap on
//! [`super::etcd::LEADER_KEY`]. The value stored at the key is the same
//! [`crate::leader::Lease`] record (node id + epoch), so every downstream reader
//! that already understands the lease shape keeps working; only the *substrate*
//! moves off the filesystem onto strongly-consistent etcd.
//!
//! Election is **stateless across ticks** (no stored lease id): each renew tick
//! re-establishes a fresh `LEADER_LEASE_TTL_S` lease via a guarded put, and etcd
//! auto-deletes the key when a lease expires — so a dead leader's key simply
//! vanishes and the next campaigner's CAS wins. The flow per tick:
//!   * key absent → grant a lease, `Txn` CAS on `create_revision == 0` to claim
//!     it (loser reads the winner);
//!   * key is ours → grant a fresh lease, CAS on the value being unchanged to
//!     renew (the old lease orphans + expires harmlessly);
//!   * key held by another → follow.
//!
//! `force` (the operator's `take-leadership --force`) bumps the epoch and writes
//! unconditionally, matching [`crate::leader::force_take`].

use std::time::{SystemTime, UNIX_EPOCH};

use etcd_client::{Client, Compare, CompareOp, Error, PutOptions, Txn, TxnOp};

use crate::leader::{AcquireResult, Lease};

use super::etcd::{LEADER_KEY, LEADER_LEASE_TTL_S};

fn now_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Read the current leader lease from etcd (the value at [`LEADER_KEY`]), or
/// `None` when the key is absent (no leader) or unparseable. Because etcd
/// auto-expires the key with its lease, an absent key already means "expired".
///
/// # Errors
/// An [`etcd_client::Error`] on a failed range read.
pub async fn current_leader(client: &mut Client) -> Result<Option<Lease>, Error> {
    let resp = client.get(LEADER_KEY, None).await?;
    Ok(resp
        .kvs()
        .first()
        .and_then(|kv| kv.value_str().ok())
        .and_then(Lease::decode))
}

/// Try to acquire or renew leadership for `node_id` via an etcd lease + CAS.
/// Returns the same [`AcquireResult`] shape as the retired fs-lock election.
///
/// # Errors
/// An [`etcd_client::Error`] on a failed lease grant / range read / txn.
pub async fn campaign(client: &mut Client, node_id: &str) -> Result<AcquireResult, Error> {
    let existing = client.get(LEADER_KEY, None).await?;
    let current = existing
        .kvs()
        .first()
        .and_then(|kv| kv.value_str().ok())
        .and_then(Lease::decode);

    match current {
        // Free (or auto-expired) — claim it iff the key is still absent.
        None => {
            let lease = client.lease_grant(LEADER_LEASE_TTL_S, None).await?.id();
            let mine = Lease {
                node_id: node_id.to_owned(),
                renewed_at_s: now_s(),
                epoch: 1,
            };
            let txn = Txn::new()
                .when([Compare::create_revision(LEADER_KEY, CompareOp::Equal, 0)])
                .and_then([TxnOp::put(
                    LEADER_KEY,
                    mine.encode(),
                    Some(PutOptions::new().with_lease(lease)),
                )]);
            let resp = client.txn(txn).await?;
            if resp.succeeded() {
                Ok(AcquireResult::Acquired)
            } else {
                // Lost the race — report whoever won.
                Ok(held_by(client).await?)
            }
        }
        // Ours — renew with a fresh lease, guarded on the value being unchanged
        // (so we don't clobber a concurrent takeover).
        Some(ref l) if l.node_id == node_id => {
            let prev = l.encode();
            let lease = client.lease_grant(LEADER_LEASE_TTL_S, None).await?.id();
            let renewed = Lease {
                node_id: node_id.to_owned(),
                renewed_at_s: now_s(),
                epoch: l.epoch,
            };
            let txn = Txn::new()
                .when([Compare::value(
                    LEADER_KEY,
                    CompareOp::Equal,
                    prev.as_bytes(),
                )])
                .and_then([TxnOp::put(
                    LEADER_KEY,
                    renewed.encode(),
                    Some(PutOptions::new().with_lease(lease)),
                )]);
            let resp = client.txn(txn).await?;
            if resp.succeeded() {
                Ok(AcquireResult::Acquired)
            } else {
                Ok(held_by(client).await?)
            }
        }
        // Held by another live node.
        Some(l) => Ok(AcquireResult::HeldBy {
            lease_remaining_s: l.remaining_s(now_s()),
            leader_id: l.node_id,
        }),
    }
}

/// Re-read the key and report it as `HeldBy` (used after a lost CAS race).
async fn held_by(client: &mut Client) -> Result<AcquireResult, Error> {
    Ok(match current_leader(client).await? {
        Some(l) => AcquireResult::HeldBy {
            lease_remaining_s: l.remaining_s(now_s()),
            leader_id: l.node_id,
        },
        // Vanished again between reads — treat as expired/free.
        None => AcquireResult::ExpiredLease,
    })
}

/// Blocking read of the current etcd leader (the healthz `is_leader` bridge).
/// `None` on connect/read failure or no leader. MUST run OFF the tokio executor
/// (the healthz enrichment helper thread qualifies) — it builds a private
/// current-thread runtime.
#[must_use]
pub fn current_leader_blocking(endpoints: &[String]) -> Option<Lease> {
    // Route through the shared runtime-aware bridge: a bare
    // `new_current_thread().block_on()` here panicked ("runtime within a runtime")
    // when called from the async `mesh_dns` worker and crash-looped it on every
    // etcd node. `super::peers::block_on` is safe from both a std::thread and an
    // async executor.
    super::peers::block_on(async {
        match super::etcd::connect(endpoints).await {
            Ok(mut c) => current_leader(&mut c).await.ok().flatten(),
            Err(_) => None,
        }
    })
    .flatten()
}

/// Force `node_id` into leadership: bump the epoch past the prior holder's and
/// write unconditionally under a fresh lease. The operator's last resort
/// (`mackesd take-leadership --force`), mirroring [`crate::leader::force_take`].
///
/// # Errors
/// An [`etcd_client::Error`] on a failed read / lease grant / put.
pub async fn force(client: &mut Client, node_id: &str) -> Result<Lease, Error> {
    let prior_epoch = current_leader(client).await?.map_or(0, |l| l.epoch);
    let lease = client.lease_grant(LEADER_LEASE_TTL_S, None).await?.id();
    let next = Lease {
        node_id: node_id.to_owned(),
        renewed_at_s: now_s(),
        epoch: prior_epoch + 1,
    };
    client
        .put(
            LEADER_KEY,
            next.encode(),
            Some(PutOptions::new().with_lease(lease)),
        )
        .await?;
    Ok(next)
}

/// Blocking façade over [`force`] for synchronous callers off the tokio executor
/// (the LIGHTHOUSE-6 `lighthouse-promote` responder, which runs on the off-tokio
/// host-ops thread). Mirrors [`current_leader_blocking`]: routes through the
/// runtime-aware `super::peers::block_on` bridge so it's safe from both a
/// `std::thread` and an async executor (a bare `block_on` would panic with
/// "runtime within a runtime").
///
/// # Errors
/// `Err(<message>)` when the etcd runtime is unavailable or the connect / force
/// round-trip fails — the same string-typed surface the responder returns to the
/// Bus reply.
pub fn force_blocking(endpoints: &[String], node_id: &str) -> Result<Lease, String> {
    super::peers::block_on(async {
        let mut client = super::etcd::connect(endpoints)
            .await
            .map_err(|e| format!("etcd connect: {e}"))?;
        force(&mut client, node_id)
            .await
            .map_err(|e| format!("etcd force: {e}"))
    })
    .ok_or_else(|| "etcd runtime unavailable".to_string())?
}
