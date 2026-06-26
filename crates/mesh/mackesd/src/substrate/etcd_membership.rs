//! SUBSTRATE / HA — turn-key etcd cluster (Raft) membership.
//!
//! The single choke-point that MUTATES the etcd cluster membership, wrapping the
//! native `etcd_client` member API so the lighthouse join / retire verbs manage
//! the quorum WITHOUT shelling `etcdctl` by hand. The two properties that make it
//! safe to call from the lifecycle verbs:
//!
//!   * **Quorum-safe** — a join adds exactly ONE member (self) and refuses to add
//!     into a cluster that still has a learner catching up; a retire moves
//!     leadership off the target before removing it, so the common case never
//!     forces a re-election.
//!   * **Idempotent** — a node already in the cluster is a no-op add (returns the
//!     live cluster string), and removing an absent member is a no-op.
//!
//! etcd binds the Nebula overlay (no TLS — lock #11, §1), so a member's Raft peer
//! URL is `http://<overlay-ip>:2380` and its client URL `http://<overlay-ip>:2379`.
//! `setup-etcd.sh` remains the LOCAL-daemon bootstrapper (writes the env + starts
//! the unit); this module owns the cluster-side mutation and hands the script the
//! `ETCD_INITIAL_CLUSTER` to start the local member with `state=existing`.

use std::path::Path;

use crate::substrate::etcd::connect;

/// A member's Raft **peer** URL (the `:2380` advertise-peer address).
#[must_use]
pub fn peer_url(overlay_ip: &str) -> String {
    format!("http://{overlay_ip}:2380")
}

/// A member's **client** URL (the `:2379` address peers/clients dial).
#[must_use]
pub fn client_url(overlay_ip: &str) -> String {
    format!("http://{overlay_ip}:2379")
}

/// How to select an existing member to remove.
pub enum MemberSel {
    /// By overlay IP — matches the member's `:2380` peer URL. Used for self-leave.
    Overlay(String),
    /// By member name (= `hostname -s`). Used for an operator-driven remote retire.
    Hostname(String),
}

/// Compose `ETCD_INITIAL_CLUSTER` (`name=peer_url,…`) from the post-add member set
/// so the joining node can start with `state=existing`. etcd reports a just-added,
/// not-yet-started member with an EMPTY name, so we substitute `self_name` for the
/// entry whose peer URL is `self_peer`. Any OTHER still-unnamed member is skipped
/// (it has no addressable name yet and isn't us). Pure + unit-tested.
#[must_use]
pub fn initial_cluster_csv(
    members: &[(String, String)],
    self_name: &str,
    self_peer: &str,
) -> String {
    members
        .iter()
        .filter_map(|(name, peer)| {
            let resolved = if name.is_empty() {
                if peer == self_peer {
                    self_name
                } else {
                    return None;
                }
            } else {
                name.as_str()
            };
            Some(format!("{resolved}={peer}"))
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// `(name, first-peer-url)` pairs for the pure CSV builder, decoupled from the
/// etcd wire type so [`initial_cluster_csv`] stays unit-testable.
fn member_pairs(members: &[etcd_client::Member]) -> Vec<(String, String)> {
    members
        .iter()
        .map(|m| {
            (
                m.name().to_string(),
                m.peer_urls().first().cloned().unwrap_or_default(),
            )
        })
        .collect()
}

/// Idempotently add THIS node (a lighthouse) to the etcd cluster as a **voter** and
/// return the `ETCD_INITIAL_CLUSTER` csv to start the local member with
/// `state=existing`. `endpoints` are existing-anchor client URLs (an already-live
/// lighthouse); `self_name` = `hostname -s`; `self_overlay` = this node's overlay IP.
///
/// # Errors
/// etcd connect / `member_list` / `member_add` failure, or a refusal to add while
/// an existing member is still a learner (would risk quorum).
pub async fn add_self_as_voter(
    endpoints: &[String],
    self_name: &str,
    self_overlay: &str,
) -> Result<String, String> {
    let mine = peer_url(self_overlay);
    let mut c = connect(endpoints)
        .await
        .map_err(|e| format!("etcd connect: {e}"))?;
    let list = c
        .member_list()
        .await
        .map_err(|e| format!("member_list: {e}"))?;
    // Idempotency: already a member (by name or peer URL) → return the live set.
    if list
        .members()
        .iter()
        .any(|m| m.name() == self_name || m.peer_urls().iter().any(|u| u == &mine))
    {
        return Ok(initial_cluster_csv(
            &member_pairs(list.members()),
            self_name,
            &mine,
        ));
    }
    // Health gate: never add a voter while a member is still a learner — adding
    // into a not-yet-converged cluster is how you lose quorum.
    if list.members().iter().any(etcd_client::Member::is_learner) {
        return Err("refusing member_add: an existing member is still a learner".into());
    }
    let resp = c
        .member_add(vec![mine.clone()], None)
        .await
        .map_err(|e| format!("member_add: {e}"))?;
    Ok(initial_cluster_csv(
        &member_pairs(resp.member_list()),
        self_name,
        &mine,
    ))
}

/// Remove a member by selector. Idempotent (no-op if absent). If the target is the
/// current Raft leader, move leadership to a surviving non-learner first so the
/// removal doesn't force a re-election. Returns `true` if a member was removed.
///
/// # Errors
/// etcd connect / `member_list` / `status` / `move_leader` / `member_remove` failure.
pub async fn remove_member(endpoints: &[String], sel: &MemberSel) -> Result<bool, String> {
    let mut c = connect(endpoints)
        .await
        .map_err(|e| format!("etcd connect: {e}"))?;
    let list = c
        .member_list()
        .await
        .map_err(|e| format!("member_list: {e}"))?;
    // Extract the owned ids we need, then drop the borrow on `list` before mutating.
    let (target_id, surviving_id) = {
        let members = list.members();
        let target = match sel {
            MemberSel::Overlay(ip) => {
                let pu = peer_url(ip);
                members
                    .iter()
                    .find(|m| m.peer_urls().iter().any(|u| u == &pu))
            }
            MemberSel::Hostname(h) => members.iter().find(|m| m.name() == h),
        };
        let Some(target) = target else {
            return Ok(false); // already gone — idempotent.
        };
        let tid = target.id();
        let sid = members
            .iter()
            .find(|m| m.id() != tid && !m.is_learner())
            .map(etcd_client::Member::id);
        (tid, sid)
    };
    // Move leadership off the target if it leads (avoids a re-election blip).
    let status = c.status().await.map_err(|e| format!("status: {e}"))?;
    if status.leader() == target_id {
        if let Some(sid) = surviving_id {
            c.move_leader(sid)
                .await
                .map_err(|e| format!("move_leader: {e}"))?;
        }
    }
    c.member_remove(target_id)
        .await
        .map_err(|e| format!("member_remove: {e}"))?;
    Ok(true)
}

/// The overlay IPs that SHOULD be etcd voters per the canonical directory: every
/// `role==lighthouse` record carrying an overlay IP. Detection helper for an
/// operator/diagnostic readout of who is (or isn't yet) in the quorum.
#[must_use]
pub fn voter_overlays_from_directory(workgroup_root: &Path) -> Vec<String> {
    crate::substrate::peers::read_directory(workgroup_root)
        .into_iter()
        .filter(mackes_mesh_types::lighthouse::is_lighthouse)
        .filter_map(|p| p.overlay_ip)
        .filter(|ip| !ip.is_empty())
        .collect()
}

/// Blocking [`add_self_as_voter`] for the sync join path. Reuses the shared,
/// runtime-aware [`crate::substrate::peers::block_on`] bridge (safe from both a
/// plain std::thread and an async worker). `None` if a private runtime couldn't be
/// built; `Some(Err)` on an etcd error.
#[must_use]
pub fn add_self_as_voter_blocking(
    endpoints: &[String],
    self_name: &str,
    self_overlay: &str,
) -> Option<Result<String, String>> {
    crate::substrate::peers::block_on(add_self_as_voter(endpoints, self_name, self_overlay))
}

/// Blocking [`remove_member`] for the sync leave / remove-peer paths.
#[must_use]
pub fn remove_member_blocking(
    endpoints: &[String],
    sel: &MemberSel,
) -> Option<Result<bool, String>> {
    crate::substrate::peers::block_on(remove_member(endpoints, sel))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_overlay_bound() {
        assert_eq!(peer_url("10.42.0.4"), "http://10.42.0.4:2380");
        assert_eq!(client_url("10.42.0.4"), "http://10.42.0.4:2379");
    }

    #[test]
    fn initial_cluster_substitutes_self_name_for_the_unnamed_new_member() {
        // etcd reports the just-added self with an EMPTY name — we substitute it.
        let members = vec![
            ("lh-01".to_string(), "http://10.42.0.1:2380".to_string()),
            (String::new(), "http://10.42.0.4:2380".to_string()), // self, unstarted
        ];
        let csv = initial_cluster_csv(&members, "lh-nyc3", "http://10.42.0.4:2380");
        assert_eq!(
            csv,
            "lh-01=http://10.42.0.1:2380,lh-nyc3=http://10.42.0.4:2380"
        );
    }

    #[test]
    fn initial_cluster_skips_other_unnamed_members() {
        // A different unstarted member (not us) has no addressable name — skip it.
        let members = vec![
            ("lh-01".to_string(), "http://10.42.0.1:2380".to_string()),
            (String::new(), "http://10.42.0.9:2380".to_string()), // some other unstarted
            (String::new(), "http://10.42.0.4:2380".to_string()), // self
        ];
        let csv = initial_cluster_csv(&members, "lh-sfo3", "http://10.42.0.4:2380");
        assert_eq!(
            csv,
            "lh-01=http://10.42.0.1:2380,lh-sfo3=http://10.42.0.4:2380"
        );
    }

    #[test]
    fn initial_cluster_all_named_is_verbatim() {
        let members = vec![
            ("a".to_string(), "http://10.42.0.1:2380".to_string()),
            ("b".to_string(), "http://10.42.0.3:2380".to_string()),
        ];
        // self already named/present → no substitution, no skip.
        let csv = initial_cluster_csv(&members, "a", "http://10.42.0.1:2380");
        assert_eq!(csv, "a=http://10.42.0.1:2380,b=http://10.42.0.3:2380");
    }
}
