//! QC-2/QC-4 — the fleet/one-state doctrine seam: WHICH `OpenStack` services
//! this node should host, read live off the mesh substrate.
//!
//! The one-state doctrine (design Q30) is authoritative: the fleet state —
//! etcd + TOML-on-Syncthing — declares the cloud, and every node converges on
//! it (Q71). This module models the node-local *view* of that doctrine
//! ([`CloudDesired`]), the pure fold from view → desired service set
//! ([`desired_services`]), and — QC-4 — the live reader ([`MeshFleetState`])
//! behind an injectable [`FleetStateSource`] seam so the whole reconcile
//! pipeline stays headless-testable.
//!
//! ## The QC-4 live read (SUBSTRATE-V2)
//!
//! The doctrine record body — `enabled`, the pinned `kolla_release`, and any
//! node scoping — rides the **Syncthing share** as its TOML companion
//! (`<workgroup_root>/cloud/doctrine.toml`), exactly as the media/music
//! workers read their replicated records off the share: it is always locally
//! present (no etcd round-trip on the read path) and any node can author it
//! (Q84). The one coordination bit — **who hosts the leader-only `MariaDB`
//! (Q15)** — folds off the existing etcd `/mesh/leader` lease
//! ([`crate::substrate::etcd::LEADER_KEY`]) via the shared
//! [`crate::substrate::leader::current_leader_blocking`] read; no new election
//! is invented. Together that is the Q30 "etcd + TOML-on-Syncthing" split
//! realized: coordination in etcd, the doctrine body on the file substrate.
//!
//! Absent record → a clean [`CloudDesired`] with `enabled: false` (a
//! pre-doctrine node still publishes an honest "no cloud here" mirror, never a
//! fabricated one). A *present but malformed* record → a typed
//! [`FleetStateError::Failed`] the reconcile surfaces as a `Gated` row (§7 —
//! we never guess a desired set from an unparseable doctrine).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;
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
    /// The fleet state declares the cloud (Q71) **and** this node is in scope.
    /// `false` = a declared no-cloud doctrine, an out-of-scope node, or no
    /// doctrine at all — the node converges to zero services (the Q72 hard
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

/// The on-substrate doctrine record.
///
/// The TOML companion on the Syncthing share
/// (`<workgroup_root>/cloud/doctrine.toml`) any node can author (Q84), and the
/// same shape an etcd `/mesh/cloud/` value would carry. Pure + parseable so the
/// fold is headless-testable.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CloudRecord {
    /// The fleet declares the cloud (Q71).
    pub enabled: bool,
    /// The pinned Kolla release (Q69).
    pub kolla_release: String,
    /// Optional node scoping (Q1 — any-role by config): the mesh names of the
    /// nodes that carry the cloud. Empty/absent ⇒ **every node** (the
    /// converge-everywhere default, Q71).
    #[serde(default)]
    pub nodes: Vec<String>,
}

impl CloudRecord {
    /// Parse the TOML companion body.
    ///
    /// # Errors
    /// A [`toml::de::Error`] when the body isn't a valid doctrine record.
    pub fn from_toml(body: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(body)
    }

    /// Fold this record + the resolved `leader` bit into this node's
    /// [`CloudDesired`] view. Node scoping (Q1) gates `enabled`: a record that
    /// lists nodes disables the cloud on any node it doesn't name.
    #[must_use]
    pub fn fold(&self, host: &str, leader: bool) -> CloudDesired {
        let in_scope = self.nodes.is_empty() || self.nodes.iter().any(|n| n == host);
        CloudDesired {
            enabled: self.enabled && in_scope,
            leader,
            kolla_release: self.kolla_release.clone(),
        }
    }
}

/// A typed failure from the [`FleetStateSource`] seam.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FleetStateError {
    /// A doctrine sub-read isn't wired in this build/environment yet — it needs
    /// a real prerequisite. §7-legal: a real method returning a real typed
    /// error naming what's missing. Retained as the [`FleetStateSource`]
    /// vocabulary for a gated read (the in-memory fake drives it in tests); the
    /// live [`MeshFleetState`] no longer returns it (an absent record is an
    /// honest `Disabled`, not a gate).
    #[error("cloud doctrine: integration-gated — {reason}")]
    IntegrationGated {
        /// What the live read needs before it can answer.
        reason: String,
    },
    /// The read ran and failed for a concrete runtime reason (a present but
    /// malformed doctrine record, …). Converges nothing — never a guessed set.
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
    /// A [`FleetStateError::Failed`] when a doctrine record is present but
    /// unparseable (never a fabricated view). An absent record is not an error
    /// — it folds to a `Disabled` [`CloudDesired`].
    fn read(&self) -> Result<CloudDesired, FleetStateError>;
}

/// The doctrine's TOML companion on the Syncthing share.
#[must_use]
pub fn doctrine_toml_path(workgroup_root: &Path) -> PathBuf {
    workgroup_root.join("cloud").join("doctrine.toml")
}

/// Production [`FleetStateSource`]: the one-state doctrine on the mesh
/// substrate (etcd + TOML-on-Syncthing, design Q30).
///
/// Reads the doctrine record body off the Syncthing share (the TOML companion,
/// the media/music-worker idiom) and folds the leader-only placement bit off
/// the etcd `/mesh/leader` lease. Honors the same `MDE_WORKGROUP_ROOT` override
/// the rest of mackesd resolves the share with (the caller passes the resolved
/// `workgroup_root`).
#[derive(Debug, Clone)]
pub struct MeshFleetState {
    /// This node's mesh id — the leader-lease match + the doctrine's node
    /// scoping key.
    host: String,
    /// The replicated workgroup root — where the doctrine's TOML companion
    /// lives on the Syncthing share.
    workgroup_root: PathBuf,
}

impl MeshFleetState {
    /// Construct over this node's `host` id + the mesh `workgroup_root` (the
    /// replicated shared volume the doctrine's TOML companion rides).
    #[must_use]
    pub const fn new(host: String, workgroup_root: PathBuf) -> Self {
        Self {
            host,
            workgroup_root,
        }
    }

    /// Read the doctrine record body off the Syncthing share. `Ok(None)` when
    /// no record has been authored yet (an honest pre-doctrine node), or the
    /// companion is unreadable for a non-parse reason (treated as absent — the
    /// next sync heals it); `Err(Failed)` only when a record IS present but
    /// unparseable.
    fn read_record(&self) -> Result<Option<CloudRecord>, FleetStateError> {
        let path = doctrine_toml_path(&self.workgroup_root);
        // Absent / unreadable → no declared cloud (honest Disabled); present but
        // unparseable → a typed Failed naming the path.
        std::fs::read_to_string(&path).map_or(Ok(None), |body| {
            CloudRecord::from_toml(&body)
                .map(Some)
                .map_err(|e| FleetStateError::Failed {
                    reason: format!(
                        "the cloud doctrine record {} is malformed — {e}",
                        path.display()
                    ),
                })
        })
    }

    /// Whether this node currently holds the etcd `/mesh/leader` lease (Q15).
    /// A read-only fold off the existing lease — no campaign. `false` when etcd
    /// isn't provisioned on this node yet (empty endpoints — a pre-cutover or
    /// test host), so a leaderless read never fabricates leadership.
    fn read_leader(&self) -> bool {
        let endpoints = crate::substrate::etcd::default_endpoints();
        if endpoints.is_empty() {
            return false;
        }
        crate::substrate::leader::current_leader_blocking(&endpoints)
            .is_some_and(|lease| lease.node_id == self.host)
    }
}

impl FleetStateSource for MeshFleetState {
    fn read(&self) -> Result<CloudDesired, FleetStateError> {
        let leader = self.read_leader();
        // A present record folds to the desired view; no doctrine authored yet →
        // a clean Disabled view (§7 — the honest mirror a pre-doctrine node
        // publishes), carrying the live leader bit so a flip is still observable.
        Ok(self.read_record()?.map_or_else(
            || CloudDesired {
                enabled: false,
                leader,
                kolla_release: String::new(),
            },
            |record| record.fold(&self.host, leader),
        ))
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

    // ── QC-4: the record parse + fold ──

    #[test]
    fn record_parses_and_folds_the_leader_bit() {
        let rec = CloudRecord::from_toml(
            "enabled = true\nkolla_release = \"2024.1\"\n",
        )
        .expect("valid doctrine");
        assert_eq!(rec.kolla_release, "2024.1");
        assert!(rec.nodes.is_empty(), "no scoping ⇒ every node");
        // The leader bit rides in from the /mesh/leader fold, not the record.
        assert!(rec.fold("node-a", true).leader);
        assert!(!rec.fold("node-a", false).leader);
        assert!(rec.fold("node-a", false).enabled);
    }

    #[test]
    fn node_scoping_disables_out_of_scope_nodes() {
        // Q1 — a record that names nodes carries the cloud only on those; every
        // other node folds to Disabled (converges to zero), honestly.
        let rec = CloudRecord::from_toml(
            "enabled = true\nkolla_release = \"2024.1\"\nnodes = [\"eagle\", \"nyc3\"]\n",
        )
        .expect("valid doctrine");
        assert!(rec.fold("eagle", false).enabled, "named node is in scope");
        assert!(
            !rec.fold("workstation-7", false).enabled,
            "unnamed node is out of scope"
        );
        // Out-of-scope still desires nothing.
        assert!(desired_services(&rec.fold("workstation-7", true)).is_empty());
    }

    #[test]
    fn malformed_record_is_a_typed_failure() {
        // §7 — a present-but-unparseable doctrine is a typed error the reconcile
        // gates on, never a guessed set.
        assert!(CloudRecord::from_toml("enabled = \"not a bool\"").is_err());
        assert!(CloudRecord::from_toml("kolla_release = 5").is_err());
    }

    // ── QC-4: the live MeshFleetState reader (share companion) ──

    fn seed_doctrine(root: &Path, body: &str) {
        let dir = root.join("cloud");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("doctrine.toml"), body).unwrap();
    }

    #[test]
    fn absent_record_reads_a_clean_disabled_view() {
        // A pre-doctrine node: no companion on the share → an honest Disabled
        // view (NOT an error), so the mirror still publishes.
        let share = tempfile::tempdir().unwrap();
        let src = MeshFleetState::new("node-a".into(), share.path().to_path_buf());
        let view = src.read().expect("absent record is not an error");
        assert!(!view.enabled);
        assert!(desired_services(&view).is_empty());
    }

    #[test]
    fn present_record_reads_a_parsed_desired_view() {
        // A live doctrine TOML companion on the share → a parsed CloudDesired.
        let share = tempfile::tempdir().unwrap();
        seed_doctrine(
            share.path(),
            "enabled = true\nkolla_release = \"2024.1\"\n",
        );
        let src = MeshFleetState::new("node-a".into(), share.path().to_path_buf());
        let view = src.read().expect("valid doctrine");
        assert!(view.enabled);
        assert_eq!(view.kolla_release, "2024.1");
        // Every-node services are desired (no scoping); leader-only MariaDB
        // rides the /mesh/leader fold (false here — no etcd in test).
        let set = desired_services(&view);
        assert!(set.contains(&ServiceKind::Keystone));
    }

    #[test]
    fn out_of_scope_node_reads_disabled_from_a_present_record() {
        let share = tempfile::tempdir().unwrap();
        seed_doctrine(
            share.path(),
            "enabled = true\nkolla_release = \"2024.1\"\nnodes = [\"eagle\"]\n",
        );
        let src = MeshFleetState::new("workstation-7".into(), share.path().to_path_buf());
        let view = src.read().expect("valid doctrine");
        assert!(!view.enabled, "this node isn't named in the doctrine");
    }

    #[test]
    fn malformed_companion_gates_the_read() {
        // §7 — a present but broken companion is a typed Failed (→ Gated in the
        // mirror), naming the path; never a fabricated Disabled.
        let share = tempfile::tempdir().unwrap();
        seed_doctrine(share.path(), "enabled = \"yes please\"\n");
        let src = MeshFleetState::new("node-a".into(), share.path().to_path_buf());
        let err = src.read().expect_err("malformed record must gate");
        let FleetStateError::Failed { reason } = &err else {
            unreachable!("wrong variant: {err:?}");
        };
        assert!(reason.contains("malformed"), "{reason}");
        assert!(reason.contains("doctrine.toml"), "{reason}");
    }
}
