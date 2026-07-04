//! QC-2 — the fleet/one-state doctrine seam: WHICH `OpenStack` services this
//! node should host.
//!
//! The one-state doctrine (design Q30) is authoritative: the fleet state —
//! etcd + TOML-on-Syncthing — declares the cloud, and every node converges on
//! it (Q71). This module models the node-local *view* of that doctrine
//! ([`CloudDesired`]) plus the pure fold from view → desired service set
//! ([`desired_services`]), behind an injectable [`FleetStateSource`] seam so
//! the whole reconcile pipeline is headless-testable.
//!
//! The production source ([`MeshFleetState`]) answers a typed
//! [`FleetStateError::IntegrationGated`] today: no cloud doctrine record
//! exists on the live mesh yet — QC-4 authors it when the foundation services
//! land — and fabricating "disabled" from an unread substrate would be a fake
//! answer (§7). The gated reason names exactly what the live read needs.

use std::collections::BTreeSet;
use std::path::PathBuf;

use thiserror::Error;

use super::catalog::{Placement, ServiceKind};

/// This node's folded view of the fleet cloud doctrine — everything the
/// reconcile needs to know about *what should run here*.
///
/// Deliberately node-local (the worker never reasons about other nodes'
/// service sets): the leader-hosted placement (Q15) arrives pre-resolved as
/// [`Self::leader`], read from the same etcd leader lease the rest of the
/// platform elects on (`/mesh/leader`, SUBSTRATE-V2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudDesired {
    /// The fleet state declares the cloud (Q71). `false` = a declared
    /// no-cloud doctrine — the node converges to zero services (the Q72 hard
    /// cutover direction also rides this).
    pub enabled: bool,
    /// This node currently holds the etcd leader lease — it hosts the
    /// [`Placement::LeaderOnly`] services (Q15).
    pub leader: bool,
    /// The pinned Kolla release tag (Q69 — pin until forced). Names the image
    /// tags the QC-3 mirror lane loads; the doctrine record is the single
    /// authoritative pin.
    pub kolla_release: String,
}

/// A typed failure from the [`FleetStateSource`] seam.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FleetStateError {
    /// The live doctrine read isn't wired in this build/environment yet — it
    /// needs a real prerequisite (the QC-4 doctrine record). §7-legal: a real
    /// method returning a real typed error naming what's missing, exactly as
    /// the `session_broker`'s `SessionStore` seam does.
    #[error("cloud doctrine: integration-gated — {reason}")]
    IntegrationGated {
        /// What the live read needs before it can answer.
        reason: String,
    },
    /// The read ran and failed for a concrete runtime reason (etcd
    /// unreachable, malformed record, …).
    #[error("cloud doctrine read failed: {reason}")]
    Failed {
        /// The failure detail.
        reason: String,
    },
}

/// The injectable doctrine seam: read this node's [`CloudDesired`] view off
/// the fleet state.
///
/// Production wires [`MeshFleetState`]; tests drive an in-memory fake so the
/// drain → fold → converge pipeline runs without etcd or Syncthing.
pub trait FleetStateSource {
    /// This node's current doctrine view.
    ///
    /// # Errors
    /// A [`FleetStateError`] — `IntegrationGated` until the live doctrine
    /// record + reader land (QC-4), else `Failed` on a concrete runtime
    /// error. Never a fabricated view.
    fn read(&self) -> Result<CloudDesired, FleetStateError>;
}

/// Production [`FleetStateSource`]: the one-state doctrine on the mesh
/// substrate (etcd + TOML-on-Syncthing, design Q30).
///
/// This slice (QC-2) delivers the model + the seam; the live leg is
/// integration-gated because the doctrine record itself does not exist yet:
/// QC-4 authors the `/mesh/cloud/` etcd record (+ its TOML companion on the
/// Syncthing share) alongside the foundation services, and the leader bit
/// folds off the existing `/mesh/leader` lease
/// ([`crate::substrate::etcd::LEADER_KEY`]). Until then every read answers a
/// typed [`FleetStateError::IntegrationGated`] naming exactly that — never a
/// fake "disabled" (§7).
#[derive(Debug, Clone)]
pub struct MeshFleetState {
    /// The replicated workgroup root — where the doctrine's TOML companion
    /// will live on the Syncthing share.
    workgroup_root: PathBuf,
}

impl MeshFleetState {
    /// Construct over the mesh `workgroup_root` (the replicated shared
    /// volume the doctrine's TOML companion rides).
    #[must_use]
    pub const fn new(workgroup_root: PathBuf) -> Self {
        Self { workgroup_root }
    }
}

impl FleetStateSource for MeshFleetState {
    fn read(&self) -> Result<CloudDesired, FleetStateError> {
        Err(FleetStateError::IntegrationGated {
            reason: format!(
                "no cloud doctrine record exists on the live mesh yet — QC-4 authors the \
                 `/mesh/cloud/` etcd record (+ its TOML companion on the Syncthing share \
                 under {}) when the foundation services land; the leader bit then folds \
                 off the existing `/mesh/leader` lease",
                self.workgroup_root.display()
            ),
        })
    }
}

/// The pure doctrine fold: this node's desired service set (design Q5/Q15/
/// Q22 — every-node services everywhere, leader-only services on the leader,
/// nothing when the cloud isn't declared).
#[must_use]
pub fn desired_services(view: &CloudDesired) -> BTreeSet<ServiceKind> {
    if !view.enabled {
        return BTreeSet::new();
    }
    ServiceKind::ALL
        .iter()
        .copied()
        .filter(|kind| match kind.placement() {
            Placement::EveryNode => true,
            Placement::LeaderOnly => view.leader,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(enabled: bool, leader: bool) -> CloudDesired {
        CloudDesired {
            enabled,
            leader,
            kolla_release: "2024.1".into(),
        }
    }

    #[test]
    fn disabled_doctrine_desires_nothing() {
        assert!(desired_services(&view(false, true)).is_empty());
        assert!(desired_services(&view(false, false)).is_empty());
    }

    #[test]
    fn every_node_hosts_the_full_set_minus_leader_only() {
        // Q22 — APIs on every node; Q15 — MariaDB only on the leader.
        let set = desired_services(&view(true, false));
        assert_eq!(set.len(), ServiceKind::ALL.len() - 1);
        assert!(!set.contains(&ServiceKind::Mariadb));
        assert!(set.contains(&ServiceKind::Keystone));
        assert!(set.contains(&ServiceKind::NovaCompute));
        assert!(set.contains(&ServiceKind::Rabbitmq));
        assert!(set.contains(&ServiceKind::Memcached));
    }

    #[test]
    fn the_leader_adds_mariadb() {
        // Q15 — the DB is a workload on the etcd leader, re-placed on
        // failover (a leader flip changes the fold output, nothing else).
        let set = desired_services(&view(true, true));
        assert_eq!(set.len(), ServiceKind::ALL.len());
        assert!(set.contains(&ServiceKind::Mariadb));
    }

    #[test]
    fn production_source_is_honestly_gated() {
        // §7 — the live doctrine read answers a typed gate naming the exact
        // prerequisite (the QC-4 record), never a fabricated view.
        let src = MeshFleetState::new(PathBuf::from("/mnt/mesh-storage"));
        let err = src.read().expect_err("must be gated");
        let FleetStateError::IntegrationGated { reason } = &err else {
            unreachable!("wrong variant: {err:?}");
        };
        assert!(reason.contains("/mesh/cloud/"), "{reason}");
        assert!(reason.contains("QC-4"), "{reason}");
        assert!(reason.contains("/mnt/mesh-storage"), "{reason}");
        // The Display carries the gate marker the mirror + logs surface.
        assert!(err.to_string().contains("integration-gated"), "{err}");
    }
}
