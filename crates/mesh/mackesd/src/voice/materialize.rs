//! VV-2.a (v4.0) — policy-lifecycle writer for `voice-desired.json`.
//!
//! Reads approved [`Policy::VoiceMesh`] + [`Policy::VoicePublic`]
//! rules from the latest [`crate::topology::DesiredSnapshot`] and
//! materializes the per-peer
//! [`mde_voice_config::VoiceDesired`] document at
//! `/var/lib/mackesd/voice-desired.json`. The on-disk JSON is the
//! sole input to the `mackesd voice render-config` helper that
//! Kamailio + RTPengine read at `ExecStartPre=`-time; bumping
//! the file's mtime triggers the
//! [`crate::workers::voice_config::VoiceConfigWorker`] to call
//! `systemctl try-reload-or-restart` on both units.
//!
//! Idempotent. Same input → same bytes → mtime stays put. The
//! comparison is **byte-for-byte against the existing file** —
//! we deliberately avoid `serde_json`-roundtrip-equality because
//! a future field reorder in the serializer would otherwise
//! mass-rewrite every peer's file on upgrade.
//!
//! Per-peer mesh addresses are sourced from each peer's
//! `<workgroup_root>/<peer_id>/mackesd/nebula-bundle.json`
//! ([`crate::ca::bundle::NebulaBundle::overlay_ip`]); a peer with
//! no bundle yet gets `0.0.0.0` as its dispatcher destination
//! (Kamailio answers `503` on the row until the bundle lands,
//! matching the boot-default placeholder semantics).

use std::io;
use std::path::{Path, PathBuf};

use mde_voice_config::{PeerEntry, VitelityAccount, VoiceDesired};

use crate::ca::bundle::{bundle_path, NebulaBundle};
use crate::policy::Policy;
use crate::topology::DesiredSnapshot;

/// Default on-disk path of the operator-visible `VoiceDesired`
/// JSON document. The reconciler writes here; the
/// `voice_config` worker polls this path's mtime and triggers
/// `systemctl try-reload-or-restart` when it advances. Lives
/// under `/var/lib/mackesd/` so the daemon's own user can
/// write it.
///
/// Re-exported as `crate::workers::voice_config::DEFAULT_DESIRED_JSON`
/// (legacy name) so existing async-services callers don't
/// need to flip their imports.
pub const DEFAULT_DESIRED_JSON: &str = "/var/lib/mackesd/voice-desired.json";

/// Outcome of a materialize call. Surfaced so the reconciler can
/// log `Wrote` vs `Unchanged` for operator visibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterializeOutcome {
    /// File didn't exist or its bytes differed; we wrote a new
    /// version. `voice_config` will pick up the mtime change on
    /// its next tick.
    Wrote,
    /// The file already matched the desired bytes — nothing
    /// changed on disk, no reload will fire.
    Unchanged,
    /// The snapshot had no voice policies AND no file yet existed
    /// — we left the boot-default seed step to
    /// [`crate::workers::voice_config::VoiceConfigWorker`] so the
    /// two writers don't race.
    SkippedNoPolicies,
}

/// Top-level entry. Builds the [`VoiceDesired`] from `snapshot`'s
/// voice policies, then writes it to `desired_json_path` only
/// when the serialized bytes differ from the existing file.
///
/// `node_id` is THIS peer's stable id — used to find this peer's
/// own VoicePublic row and to look up its own overlay IP.
///
/// `workgroup_root` is the QNM-Shared root; the function reads
/// `<workgroup_root>/<peer_id>/mackesd/nebula-bundle.json` for each
/// peer named in a `VoiceMesh` rule to resolve the dispatcher
/// destination address.
///
/// `desired_json_path` is typically
/// `/var/lib/mackesd/voice-desired.json` ([`DEFAULT_DESIRED_JSON`])
/// but tests + dev rigs pass a tempdir path.
///
/// # Errors
///
/// Returns the underlying IO error if write or rename fails.
/// Reading a missing nebula-bundle is NOT an error — that peer
/// just gets `0.0.0.0` as its dispatcher address.
pub fn materialize_voice_desired(
    snapshot: &DesiredSnapshot,
    node_id: &str,
    workgroup_root: &Path,
    desired_json_path: &Path,
) -> io::Result<MaterializeOutcome> {
    let has_voice = snapshot
        .voice_policies
        .iter()
        .any(|p| matches!(p, Policy::VoiceMesh { .. } | Policy::VoicePublic { .. }));
    if !has_voice && !desired_json_path.exists() {
        return Ok(MaterializeOutcome::SkippedNoPolicies);
    }

    // VV-2.a — the per-peer RTT resolver reads the mesh-latency
    // worker's cache so the dispatcher priority is latency-aware.
    // `None` (no measurement yet) leaves the row at neutral priority.
    let latency = read_latency_cache();
    let desired = build_voice_desired(
        snapshot,
        node_id,
        |peer_id| read_overlay_ip(workgroup_root, peer_id),
        |peer_id| latency.as_ref().and_then(|l| l.get(peer_id).copied()),
    );
    write_if_changed(&desired, desired_json_path)
}

/// VV-2.a — peer → measured RTT (ms) from the mesh-latency worker's
/// cache. `None` when the worker hasn't written it yet (fresh boot)
/// or it's unparseable — callers fall back to neutral priority.
///
/// The cache is parsed generically (only the `peers.<id>.rtt_ms`
/// path is read) rather than via the worker's `LatencySnapshot`
/// type, because that type lives behind the `async-services` feature
/// while this reconcile path is always-compiled. The on-disk field
/// path is the only coupling.
type LatencyMap = std::collections::BTreeMap<String, f64>;
fn read_latency_cache() -> Option<LatencyMap> {
    // Mirror of the worker's path logic: $XDG_CACHE_HOME/mde or
    // $HOME/.cache/mde, file `mesh-latency.json`.
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    let path = base.join("mde").join("mesh-latency.json");
    let raw = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let peers = v.get("peers")?.as_object()?;
    let mut out = LatencyMap::new();
    for (id, pl) in peers {
        if let Some(rtt) = pl.get("rtt_ms").and_then(serde_json::Value::as_f64) {
            out.insert(id.clone(), rtt);
        }
    }
    Some(out)
}

/// Build a [`VoiceDesired`] from the snapshot's voice policies +
/// the supplied per-peer address resolver.
///
/// Splitting this out from [`materialize_voice_desired`] keeps
/// the I/O-free part testable without a tempdir of nebula
/// bundles. The reconciler call passes a closure that reads
/// `<workgroup_root>/<peer_id>/mackesd/nebula-bundle.json`; tests pass
/// a closure backed by a `HashMap`.
pub fn build_voice_desired<F, G>(
    snapshot: &DesiredSnapshot,
    node_id: &str,
    mut address_lookup: F,
    mut latency_lookup: G,
) -> VoiceDesired
where
    F: FnMut(&str) -> Option<String>,
    // VV-2.a — per-peer measured RTT (ms); `None` ⇒ neutral priority.
    G: FnMut(&str) -> Option<f64>,
{
    let mut out = VoiceDesired::boot_default(node_id);

    // Own overlay-IP wins over the boot-default `0.0.0.0`
    // placeholder. The `nebula1` device + RTP-port range come
    // straight from `VoiceDesired::boot_default`.
    if let Some(own_ip) = address_lookup(node_id) {
        out.mesh_bind_address = own_ip;
    }

    // VoiceMesh — one PeerEntry per *remote* peer (we don't
    // emit a dispatcher row for ourselves).
    let mut peers: Vec<PeerEntry> = snapshot
        .voice_policies
        .iter()
        .filter_map(|p| match p {
            Policy::VoiceMesh {
                extension,
                node_id: target,
                display_name,
                ..
            } if target != node_id => Some(PeerEntry {
                extension: extension.clone(),
                node_id: target.clone(),
                display_name: display_name.clone(),
                mesh_address: address_lookup(target).unwrap_or_else(|| "0.0.0.0".to_owned()),
                // VV-2.a — latency-aware priority via VV-4's
                // `dispatcher_priority` (best_path → score). With a
                // measured RTT, a healthy direct path ranks high and
                // Kamailio prefers it within the setid; with no
                // measurement yet the row stays at neutral `0`
                // (round-robin) — never fabricate a ranking.
                priority: latency_lookup(target).map_or(0, |rtt| {
                    let cand = crate::voice::Candidate {
                        via: target.clone(),
                        rtt_ms: rtt as f32,
                        // The latency cache only carries RTT; an
                        // entry's presence means reachable (loss 0).
                        loss_pct: 0.0,
                    };
                    crate::voice::dispatcher_priority(target, std::slice::from_ref(&cand))
                }),
            }),
            _ => None,
        })
        .collect();
    // Deterministic ordering so the file's byte contents only
    // change when the inputs change (not when SQLite hands the
    // reconciler the rows in a different order).
    peers.sort_by(|a, b| a.extension.cmp(&b.extension));
    out.peers = peers;

    // VoicePublic — at most one entry matches this peer. The
    // policy-validator's `vp_peer_unique` invariant guarantees
    // we don't see two rows for `node_id`; if we do anyway,
    // taking the first preserves deterministic behavior + lets
    // the operator see the duplicate in the Pending Changes
    // inbox.
    out.vitelity = snapshot.voice_policies.iter().find_map(|p| match p {
        Policy::VoicePublic {
            peer_node_id,
            vitelity_username,
            vitelity_password,
            outbound_cid,
            ..
        } if peer_node_id == node_id => Some(VitelityAccount {
            username: vitelity_username.clone(),
            password: vitelity_password.clone(),
            outbound_cid: outbound_cid.clone(),
        }),
        _ => None,
    });

    out
}

/// Resolve `<workgroup_root>/<peer_id>/mackesd/nebula-bundle.json`
/// → `overlay_ip`. Returns `None` for any missing / unreadable /
/// malformed bundle (those are non-fatal — the materializer
/// falls back to `0.0.0.0`).
fn read_overlay_ip(workgroup_root: &Path, peer_id: &str) -> Option<String> {
    let path = bundle_path(workgroup_root, peer_id);
    let bytes = std::fs::read(&path).ok()?;
    let bundle: NebulaBundle = serde_json::from_slice(&bytes).ok()?;
    Some(bundle.overlay_ip)
}

/// Compare the serialized `desired` bytes against the existing
/// file (if any); write atomically only when they differ.
fn write_if_changed(
    desired: &VoiceDesired,
    desired_json_path: &Path,
) -> io::Result<MaterializeOutcome> {
    let body = serde_json::to_string_pretty(desired)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("encode: {e}")))?;
    let body_bytes = body.as_bytes();

    if let Ok(existing) = std::fs::read(desired_json_path) {
        if existing == body_bytes {
            return Ok(MaterializeOutcome::Unchanged);
        }
    }

    if let Some(parent) = desired_json_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp: PathBuf = desired_json_path.with_extension("json.tmp");
    std::fs::write(&tmp, body_bytes)?;
    std::fs::rename(&tmp, desired_json_path)?;
    Ok(MaterializeOutcome::Wrote)
}

/// Default desired-json path — re-exported for callers that
/// don't want to pull the workers module just for the constant.
#[must_use]
pub fn default_desired_json_path() -> PathBuf {
    PathBuf::from(DEFAULT_DESIRED_JSON)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ca::bundle::{write_bundle, LighthouseEntry, NebulaBundle};
    use crate::policy::Policy;
    use crate::topology::DesiredSnapshot;
    use std::collections::HashMap;

    fn fixture_bundle(overlay: &str) -> NebulaBundle {
        NebulaBundle {
            mesh_id: "test-mesh".into(),
            epoch: 1,
            ca_cert_pem: "ca".into(),
            peer_cert_pem: "p".into(),
            peer_key_pem: "k".into(),
            overlay_ip: overlay.into(),
            mesh_cidr: "10.42.0.0/16".into(),
            lighthouses: vec![LighthouseEntry {
                node_id: "peer:lh".into(),
                overlay_ip: "10.42.0.1".into(),
                external_addr: "203.0.113.1:4242".into(),
            }],
            created_at: 1_700_000_000,
        }
    }

    fn lookup<'a>(map: HashMap<&'a str, &'a str>) -> impl FnMut(&str) -> Option<String> + 'a {
        move |id: &str| map.get(id).map(|s| (*s).to_owned())
    }

    #[test]
    fn no_voice_policies_and_no_file_skips() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let snap = DesiredSnapshot::default();
        let path = tmp.path().join("voice-desired.json");
        let qnm = tmp.path().join("qnm");
        std::fs::create_dir_all(&qnm).expect("qnm");
        let out = materialize_voice_desired(&snap, "peer:self", &qnm, &path).expect("ok");
        assert_eq!(out, MaterializeOutcome::SkippedNoPolicies);
        assert!(!path.exists());
    }

    #[test]
    fn writes_voice_desired_from_voice_mesh_rows() {
        let snap = DesiredSnapshot {
            nodes: vec![],
            allow_east_west: vec![],
            settings_keys: vec![],
            voice_policies: vec![
                Policy::VoiceMesh {
                    id: "vm-1".into(),
                    extension: "1001".into(),
                    node_id: "peer:alice".into(),
                    display_name: "Alice".into(),
                },
                Policy::VoiceMesh {
                    id: "vm-2".into(),
                    extension: "1002".into(),
                    node_id: "peer:bob".into(),
                    display_name: "Bob".into(),
                },
                // The own-peer row is intentionally elided from
                // the generated dispatcher table.
                Policy::VoiceMesh {
                    id: "vm-self".into(),
                    extension: "1000".into(),
                    node_id: "peer:self".into(),
                    display_name: "Self".into(),
                },
            ],
        };
        let mut map = HashMap::new();
        map.insert("peer:self", "10.42.0.5");
        map.insert("peer:alice", "10.42.0.7");
        map.insert("peer:bob", "10.42.0.8");
        let desired = build_voice_desired(&snap, "peer:self", lookup(map), |_| None);

        assert_eq!(desired.node_id, "peer:self");
        assert_eq!(desired.mesh_bind_address, "10.42.0.5");
        assert_eq!(desired.peers.len(), 2);
        // Sorted by extension.
        assert_eq!(desired.peers[0].extension, "1001");
        assert_eq!(desired.peers[0].mesh_address, "10.42.0.7");
        assert_eq!(desired.peers[0].display_name, "Alice");
        assert_eq!(desired.peers[1].extension, "1002");
        assert_eq!(desired.peers[1].node_id, "peer:bob");
        // No latency supplied → neutral priority (round-robin).
        assert_eq!(desired.peers[0].priority, 0);
        assert_eq!(desired.peers[1].priority, 0);
    }

    #[test]
    fn vv4_latency_aware_priority_prefers_the_faster_peer() {
        // VV-2.a / AUD4-1 — a measured RTT drives the dispatcher
        // priority via VV-4's best_path: lower RTT ⇒ higher priority,
        // an unmeasured peer stays neutral, an over-budget peer floors.
        let snap = DesiredSnapshot {
            nodes: vec![],
            allow_east_west: vec![],
            settings_keys: vec![],
            voice_policies: vec![
                Policy::VoiceMesh {
                    id: "vm-fast".into(),
                    extension: "1001".into(),
                    node_id: "peer:fast".into(),
                    display_name: "Fast".into(),
                },
                Policy::VoiceMesh {
                    id: "vm-slow".into(),
                    extension: "1002".into(),
                    node_id: "peer:slow".into(),
                    display_name: "Slow".into(),
                },
                Policy::VoiceMesh {
                    id: "vm-unknown".into(),
                    extension: "1003".into(),
                    node_id: "peer:unknown".into(),
                    display_name: "Unknown".into(),
                },
                Policy::VoiceMesh {
                    id: "vm-faraway".into(),
                    extension: "1004".into(),
                    node_id: "peer:faraway".into(),
                    display_name: "Faraway".into(),
                },
            ],
        };
        let rtt = |peer: &str| match peer {
            "peer:fast" => Some(8.0),
            "peer:slow" => Some(60.0),
            "peer:faraway" => Some(120.0), // over the 80 ms direct cap
            _ => None,                     // peer:unknown — no measurement
        };
        let desired = build_voice_desired(&snap, "peer:self", |_| Some("10.0.0.1".into()), rtt);
        let pri = |ext: &str| {
            desired
                .peers
                .iter()
                .find(|p| p.extension == ext)
                .unwrap()
                .priority
        };
        assert!(pri("1001") > pri("1002"), "fast peer outranks slow peer");
        assert_eq!(pri("1003"), 0, "unmeasured peer stays neutral");
        assert_eq!(
            pri("1004"),
            0,
            "over-budget peer floors to the transit tier"
        );
    }

    #[test]
    fn missing_bundle_falls_back_to_placeholder_address() {
        let snap = DesiredSnapshot {
            nodes: vec![],
            allow_east_west: vec![],
            settings_keys: vec![],
            voice_policies: vec![Policy::VoiceMesh {
                id: "vm-1".into(),
                extension: "1001".into(),
                node_id: "peer:alice".into(),
                display_name: "Alice".into(),
            }],
        };
        let desired = build_voice_desired(&snap, "peer:self", |_| None, |_| None);
        // Own bind: stays at boot-default 0.0.0.0 when own
        // bundle is missing.
        assert_eq!(desired.mesh_bind_address, "0.0.0.0");
        // Peer mesh_address falls back when the peer's bundle
        // hasn't replicated yet.
        assert_eq!(desired.peers[0].mesh_address, "0.0.0.0");
    }

    #[test]
    fn voice_public_for_this_peer_populates_vitelity() {
        let snap = DesiredSnapshot {
            nodes: vec![],
            allow_east_west: vec![],
            settings_keys: vec![],
            voice_policies: vec![
                Policy::VoicePublic {
                    id: "vp-self".into(),
                    peer_node_id: "peer:self".into(),
                    vitelity_username: "mde-self".into(),
                    vitelity_password: "s3cret".into(),
                    outbound_cid: "15551234567".into(),
                },
                // Another peer's VoicePublic is correctly skipped.
                Policy::VoicePublic {
                    id: "vp-other".into(),
                    peer_node_id: "peer:other".into(),
                    vitelity_username: "mde-other".into(),
                    vitelity_password: "x".into(),
                    outbound_cid: "15559999999".into(),
                },
            ],
        };
        let desired = build_voice_desired(&snap, "peer:self", |_| None, |_| None);
        let v = desired.vitelity.expect("own vitelity populated");
        assert_eq!(v.username, "mde-self");
        assert_eq!(v.outbound_cid, "15551234567");
    }

    #[test]
    fn voice_public_for_other_peer_leaves_vitelity_none() {
        let snap = DesiredSnapshot {
            nodes: vec![],
            allow_east_west: vec![],
            settings_keys: vec![],
            voice_policies: vec![Policy::VoicePublic {
                id: "vp".into(),
                peer_node_id: "peer:other".into(),
                vitelity_username: "x".into(),
                vitelity_password: "y".into(),
                outbound_cid: "1".into(),
            }],
        };
        let desired = build_voice_desired(&snap, "peer:self", |_| None, |_| None);
        assert!(desired.vitelity.is_none());
    }

    #[test]
    fn materialize_reads_overlay_ip_from_real_bundle() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let qnm = tmp.path().join("qnm");
        std::fs::create_dir_all(&qnm).expect("qnm root");
        write_bundle(
            &bundle_path(&qnm, "peer:self"),
            &fixture_bundle("10.42.0.5"),
        )
        .expect("self bundle");
        write_bundle(
            &bundle_path(&qnm, "peer:alice"),
            &fixture_bundle("10.42.0.7"),
        )
        .expect("alice bundle");

        let snap = DesiredSnapshot {
            nodes: vec![],
            allow_east_west: vec![],
            settings_keys: vec![],
            voice_policies: vec![Policy::VoiceMesh {
                id: "vm-1".into(),
                extension: "1001".into(),
                node_id: "peer:alice".into(),
                display_name: "Alice".into(),
            }],
        };
        let path = tmp.path().join("voice-desired.json");
        let out = materialize_voice_desired(&snap, "peer:self", &qnm, &path).expect("materialize");
        assert_eq!(out, MaterializeOutcome::Wrote);
        let body = std::fs::read_to_string(&path).expect("file");
        let back: VoiceDesired = serde_json::from_str(&body).expect("parse");
        assert_eq!(back.mesh_bind_address, "10.42.0.5");
        assert_eq!(back.peers[0].mesh_address, "10.42.0.7");
    }

    #[test]
    fn second_call_with_same_inputs_is_unchanged() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let qnm = tmp.path().join("qnm");
        std::fs::create_dir_all(&qnm).expect("qnm root");
        let snap = DesiredSnapshot {
            nodes: vec![],
            allow_east_west: vec![],
            settings_keys: vec![],
            voice_policies: vec![Policy::VoiceMesh {
                id: "vm-1".into(),
                extension: "1001".into(),
                node_id: "peer:alice".into(),
                display_name: "Alice".into(),
            }],
        };
        let path = tmp.path().join("voice-desired.json");
        let first = materialize_voice_desired(&snap, "peer:self", &qnm, &path).expect("first");
        assert_eq!(first, MaterializeOutcome::Wrote);
        let second = materialize_voice_desired(&snap, "peer:self", &qnm, &path).expect("second");
        assert_eq!(second, MaterializeOutcome::Unchanged);
    }

    #[test]
    fn changed_policy_rewrites_the_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let qnm = tmp.path().join("qnm");
        std::fs::create_dir_all(&qnm).expect("qnm root");
        let path = tmp.path().join("voice-desired.json");

        let snap1 = DesiredSnapshot {
            nodes: vec![],
            allow_east_west: vec![],
            settings_keys: vec![],
            voice_policies: vec![Policy::VoiceMesh {
                id: "vm-1".into(),
                extension: "1001".into(),
                node_id: "peer:alice".into(),
                display_name: "Alice".into(),
            }],
        };
        assert_eq!(
            materialize_voice_desired(&snap1, "peer:self", &qnm, &path).expect("first"),
            MaterializeOutcome::Wrote
        );

        let snap2 = DesiredSnapshot {
            nodes: vec![],
            allow_east_west: vec![],
            settings_keys: vec![],
            voice_policies: vec![
                Policy::VoiceMesh {
                    id: "vm-1".into(),
                    extension: "1001".into(),
                    node_id: "peer:alice".into(),
                    display_name: "Alice".into(),
                },
                Policy::VoiceMesh {
                    id: "vm-2".into(),
                    extension: "1002".into(),
                    node_id: "peer:bob".into(),
                    display_name: "Bob".into(),
                },
            ],
        };
        assert_eq!(
            materialize_voice_desired(&snap2, "peer:self", &qnm, &path).expect("second"),
            MaterializeOutcome::Wrote
        );
        let body = std::fs::read_to_string(&path).expect("re-read");
        let parsed: VoiceDesired = serde_json::from_str(&body).expect("parse");
        assert_eq!(parsed.peers.len(), 2);
    }

    #[test]
    fn non_voice_policies_dont_count_toward_has_voice() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let snap = DesiredSnapshot {
            nodes: vec![],
            allow_east_west: vec![],
            settings_keys: vec![],
            voice_policies: vec![Policy::AllowEastWest {
                id: "aew-1".into(),
                from_region: "us-east".into(),
                to_region: "us-west".into(),
            }],
        };
        let path = tmp.path().join("voice-desired.json");
        let qnm = tmp.path().join("qnm");
        std::fs::create_dir_all(&qnm).expect("qnm");
        let out = materialize_voice_desired(&snap, "peer:self", &qnm, &path).expect("ok");
        assert_eq!(out, MaterializeOutcome::SkippedNoPolicies);
    }

    #[test]
    fn default_desired_path_matches_worker_constant() {
        assert_eq!(
            default_desired_json_path(),
            PathBuf::from(DEFAULT_DESIRED_JSON)
        );
    }
}
