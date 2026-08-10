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

use crate::substrate::etcd::connect;

/// One authoritative etcd member plus the result of a direct, bounded client
/// probe to that member. Retirement policy consumes this instead of replicated
/// peer-directory rows, which can outlive an unreachable voter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemberSnapshot {
    /// etcd member name; empty until a newly added member starts.
    pub name: String,
    /// Whether etcd still classifies this member as a learner.
    pub learner: bool,
    /// Whether a linearizable read succeeded through this exact member.
    pub reachable: bool,
}

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

fn client_endpoint_from_peer(peer: &str) -> Option<String> {
    peer.strip_suffix(":2380")
        .map(|prefix| format!("{prefix}:2379"))
}

/// Read the authoritative member set and directly probe every member's client
/// endpoint. A successful probe is a bounded linearizable read through that
/// exact endpoint, so a stale directory row or merely configured URL cannot be
/// counted as a live HA survivor.
pub async fn member_snapshots(endpoints: &[String]) -> Result<Vec<MemberSnapshot>, String> {
    let mut client = connect(endpoints)
        .await
        .map_err(|error| format!("etcd connect: {error}"))?;
    let response = client
        .member_list()
        .await
        .map_err(|error| format!("member_list: {error}"))?;
    let mut snapshots = Vec::with_capacity(response.members().len());
    for member in response.members() {
        let endpoint = member
            .peer_urls()
            .first()
            .and_then(|peer| client_endpoint_from_peer(peer));
        let reachable = match endpoint {
            Some(endpoint) => connect(&[endpoint]).await.is_ok(),
            None => false,
        };
        snapshots.push(MemberSnapshot {
            name: member.name().to_string(),
            learner: member.is_learner(),
            reachable,
        });
    }
    Ok(snapshots)
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

/// Blocking bridge for the destructive lighthouse-retirement preflight.
#[must_use]
pub fn member_snapshots_blocking(
    endpoints: &[String],
) -> Option<Result<Vec<MemberSnapshot>, String>> {
    crate::substrate::peers::block_on(member_snapshots(endpoints))
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
    fn peer_urls_map_only_the_etcd_raft_port_to_the_client_port() {
        assert_eq!(
            client_endpoint_from_peer("http://10.42.0.2:2380"),
            Some("http://10.42.0.2:2379".to_string())
        );
        assert_eq!(client_endpoint_from_peer("http://10.42.0.2:1234"), None);
        assert_eq!(client_endpoint_from_peer(""), None);
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
