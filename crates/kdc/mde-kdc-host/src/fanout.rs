//! KDC-MESH-9 — the mesh-fanout endpoint (design #8).
//!
//! Stock KDE Connect lists every host as a separate device (the Android-side
//! constraint, `docs/design/kdc-mesh.md`). Lock #8 realizes the "one **Quazar
//! Mesh** device" experience **host-side**: a single **designated endpoint** node
//! advertises its KDE Connect identity as [`MESH_ENDPOINT_NAME`] — the one device
//! the user drives for the *follow-everywhere* features (#6/#10) — and a
//! **mesh-fanout** relays each follow-everywhere action it receives to EVERY node
//! and **aggregates** their responses. Node-specific actions (#7) still target a
//! node by its own overlay identity via the service directory / the desktop Phones
//! hub.
//!
//! The endpoint is elected **deterministically** from the mesh roster — the
//! lexicographically-lowest hostname, "a stable primary" (#8). Every node
//! re-elects from the SAME replicated roster, so the mesh converges on ONE
//! endpoint; the follow-the-user-node override is a later refinement (the stable
//! primary is the honest v1). Election is at host start (the advertised KDC name is
//! the TLS-handshake identity, fixed for the link's life); a roster change settles
//! on the next node restart.
//!
//! The relay rides the **same own-row replicated-substrate pattern** as the SEC-5
//! phone roster + the KDC-MESH-5 notification relay: the endpoint appends a
//! [`FanoutRequest`] to its own request row; every node drains the pending requests
//! ([`collect_pending_requests`]), applies each locally, and writes a
//! [`FanoutResponse`] to its own response row; the endpoint reads them back
//! ([`aggregate_responses`]). All pure + hermetically testable against a tempdir —
//! no network, no clock (the caller injects `now_ms`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The single device name a designated endpoint advertises to stock KDE Connect
/// (#8) — the one "device" the user interacts with for follow-everywhere actions.
pub const MESH_ENDPOINT_NAME: &str = "Quazar Mesh";

/// The replicated directory holding the fanout request + response rows.
#[must_use]
pub fn fanout_dir(workgroup_root: &Path) -> PathBuf {
    workgroup_root.join("kdc-fanout")
}

/// The endpoint's request rows (`<root>/kdc-fanout/requests/<host>.json`).
#[must_use]
pub fn requests_dir(workgroup_root: &Path) -> PathBuf {
    fanout_dir(workgroup_root).join("requests")
}

/// Every node's response rows (`<root>/kdc-fanout/responses/<host>.json`).
#[must_use]
pub fn responses_dir(workgroup_root: &Path) -> PathBuf {
    fanout_dir(workgroup_root).join("responses")
}

/// The deterministic endpoint election (#8): the lexicographically-lowest hostname.
///
/// A **stable primary**; `None` for an empty roster. Every node runs this over the
/// same replicated roster, so the mesh converges on ONE endpoint without a
/// coordinator.
#[must_use]
pub fn designated_endpoint(hosts: &[String]) -> Option<String> {
    hosts.iter().filter(|h| !h.is_empty()).min().cloned()
}

/// Whether `self_host` is the designated endpoint among `hosts` (`self_host` is
/// included even if the caller didn't list it, so a lone node is its own endpoint).
#[must_use]
pub fn is_designated_endpoint(self_host: &str, hosts: &[String]) -> bool {
    let mut all: Vec<String> = hosts.to_vec();
    if !self_host.is_empty() && !all.iter().any(|h| h == self_host) {
        all.push(self_host.to_string());
    }
    designated_endpoint(&all).as_deref() == Some(self_host)
}

/// The KDE Connect `device_name` a node advertises.
///
/// [`MESH_ENDPOINT_NAME`] when it is the designated endpoint (#8), else the
/// `fallback` (the `MDE-MESH <host>` name). Pure — the worker computes
/// `is_endpoint` from the roster once at start.
#[must_use]
pub fn endpoint_device_name(is_endpoint: bool, fallback: &str) -> String {
    if is_endpoint {
        MESH_ENDPOINT_NAME.to_string()
    } else {
        fallback.to_string()
    }
}

/// A **follow-everywhere** action the endpoint fans out to every node (#6/#10).
///
/// The v1 set is the two follow-everywhere actions already wired on the receiving
/// node — a phone clipboard copy and a find-my-device ring — so a copy / ring on
/// the single "Quazar Mesh" device reaches EVERY desktop, not just the endpoint.
/// Media control (#10) lands with KDC-MESH-6; a new variant slots in here without
/// touching the substrate shape (forward-compatible serde tag).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum FanoutAction {
    /// A phone clipboard copy → applied on every desktop's clipboard (#6).
    Clipboard {
        /// The copied text to place on each node's clipboard.
        content: String,
    },
    /// A find-my-device ring → every desktop rings audibly (#10).
    Ring,
}

impl FanoutAction {
    /// A short stable tag for the request id + audit line.
    #[must_use]
    pub const fn tag(&self) -> &'static str {
        match self {
            Self::Clipboard { .. } => "clipboard",
            Self::Ring => "ring",
        }
    }
}

/// The stable, mesh-wide id for one fanout action instance — `origin:ts:tag`.
///
/// Two nodes never both originate the same request (only the endpoint writes
/// requests), so `origin_host` + the wall-clock `ts_ms` + the action tag is unique;
/// it doubles as each node's apply-once de-dup key.
#[must_use]
pub fn request_id(origin_host: &str, ts_ms: i64, action: &FanoutAction) -> String {
    format!("{origin_host}:{ts_ms}:{}", action.tag())
}

/// One fanout request the endpoint appends to its own request row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FanoutRequest {
    /// The mesh-wide de-dup id ([`request_id`]).
    pub id: String,
    /// The follow-everywhere action to apply on every node.
    pub action: FanoutAction,
    /// The endpoint node that received the action from the phone.
    pub origin_host: String,
    /// Unix-ms the endpoint received it (freshness; the reader ages it out).
    pub ts_ms: i64,
}

/// One node's response to a fanout request, appended to its own response row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FanoutResponse {
    /// The [`FanoutRequest::id`] this responds to.
    pub request_id: String,
    /// The responding node's hostname.
    pub node_host: String,
    /// Whether the node applied the action (`false` = an honest local failure).
    pub applied: bool,
    /// A short human detail (`"clipboard set"`, `"rang"`, or an error note).
    pub detail: String,
    /// Unix-ms the node applied it (freshness).
    pub ts_ms: i64,
}

/// The endpoint's aggregate view of one request's responses (#8 "aggregating
/// responses").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FanoutAggregate {
    /// The request the responses answer.
    pub request_id: String,
    /// How many nodes reported `applied == true`.
    pub applied: usize,
    /// Every responding node's hostname, sorted (applied or not).
    pub responders: Vec<String>,
}

/// Append `req` to `host`'s own request row (`requests/<host>.json`).
///
/// Idempotent per [`FanoutRequest::id`] and bounded to the newest `cap` by
/// timestamp. Atomic temp + rename, own-row authority — the same shape as the
/// notification relay.
///
/// # Errors
/// IO / serialization failures.
pub fn publish_request(
    workgroup_root: &Path,
    host: &str,
    req: &FanoutRequest,
    cap: usize,
) -> std::io::Result<PathBuf> {
    append_row(
        &requests_dir(workgroup_root),
        host,
        req,
        cap,
        |r| r.id.clone(),
        |r| r.ts_ms,
    )
}

/// Read every OTHER node's pending fanout requests (skip our own row).
///
/// The origin already applied locally, so its own row is skipped; only entries
/// fresher than `stale_ms` are kept. The caller de-dups by [`FanoutRequest::id`]
/// against its apply-once seen-set before applying each. Sorted by id for a
/// deterministic apply order.
#[must_use]
pub fn collect_pending_requests(
    workgroup_root: &Path,
    self_host: &str,
    now_ms: i64,
    stale_ms: i64,
) -> Vec<FanoutRequest> {
    let mut out: Vec<FanoutRequest> = read_rows::<FanoutRequest>(&requests_dir(workgroup_root))
        .into_iter()
        .filter(|(stem, _)| stem != self_host)
        .flat_map(|(_, rows)| rows)
        .filter(|r| now_ms.saturating_sub(r.ts_ms) <= stale_ms)
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Append `resp` to `host`'s own response row (`responses/<host>.json`).
///
/// Idempotent per `request_id` (a node's latest response to a given request wins)
/// and bounded to the newest `cap`. Atomic temp + rename, own-row authority.
///
/// # Errors
/// IO / serialization failures.
pub fn publish_response(
    workgroup_root: &Path,
    host: &str,
    resp: &FanoutResponse,
    cap: usize,
) -> std::io::Result<PathBuf> {
    append_row(
        &responses_dir(workgroup_root),
        host,
        resp,
        cap,
        |r| r.request_id.clone(),
        |r| r.ts_ms,
    )
}

/// Aggregate every node's responses to `request_id` (#8).
///
/// The endpoint's view of how far a follow-everywhere action reached: reads all
/// response rows, keeps the matching + fresh ones, counts `applied`, and lists the
/// responders sorted.
#[must_use]
pub fn aggregate_responses(
    workgroup_root: &Path,
    request_id: &str,
    now_ms: i64,
    stale_ms: i64,
) -> FanoutAggregate {
    let mut applied = 0usize;
    let mut responders: Vec<String> = Vec::new();
    for (_, rows) in read_rows::<FanoutResponse>(&responses_dir(workgroup_root)) {
        for r in rows {
            if r.request_id != request_id || now_ms.saturating_sub(r.ts_ms) > stale_ms {
                continue;
            }
            if r.applied {
                applied += 1;
            }
            if !responders.contains(&r.node_host) {
                responders.push(r.node_host);
            }
        }
    }
    responders.sort();
    FanoutAggregate {
        request_id: request_id.to_string(),
        applied,
        responders,
    }
}

// ── shared own-row substrate helpers ────────────────────────────────────────

/// Append one `entry` to `dir/<host>.json`, idempotent per `key_of` and bounded to
/// the newest `cap` by `ts_of`. Atomic temp + rename (the notification-relay shape,
/// factored so requests + responses share it).
fn append_row<T, K, TS>(
    dir: &Path,
    host: &str,
    entry: &T,
    cap: usize,
    key_of: K,
    ts_of: TS,
) -> std::io::Result<PathBuf>
where
    T: Serialize + for<'de> Deserialize<'de> + Clone,
    K: Fn(&T) -> String,
    TS: Fn(&T) -> i64,
{
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{host}.json"));
    let mut rows: Vec<T> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    let key = key_of(entry);
    rows.retain(|r| key_of(r) != key);
    rows.push(entry.clone());
    if rows.len() > cap.max(1) {
        rows.sort_by_key(&ts_of);
        let start = rows.len() - cap.max(1);
        rows.drain(0..start);
    }
    let body = serde_json::to_string_pretty(&rows)?;
    let tmp = dir.join(format!(".{host}.json.tmp"));
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Read every `<host>.json` row-file under `dir` as `(hostname, rows)`. Junk /
/// half-replicated files are skipped, like every other replicated reader.
fn read_rows<T: for<'de> Deserialize<'de>>(dir: &Path) -> Vec<(String, Vec<T>)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Ok(rows) = serde_json::from_str::<Vec<T>>(&raw) {
            out.push((stem.to_string(), rows));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosts(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn election_picks_the_stable_primary_and_every_node_agrees() {
        let roster = hosts(&["oak", "eagle", "pine"]);
        assert_eq!(designated_endpoint(&roster).as_deref(), Some("eagle"));
        // Every node computes the same endpoint from the same roster.
        assert!(is_designated_endpoint("eagle", &roster));
        assert!(!is_designated_endpoint("oak", &roster));
        assert!(!is_designated_endpoint("pine", &roster));
    }

    #[test]
    fn a_lone_node_is_its_own_endpoint_even_if_unlisted() {
        // The roster hasn't synced this node's own row yet — it still elects self.
        assert!(is_designated_endpoint("solo", &[]));
        assert!(is_designated_endpoint("aaa", &hosts(&["zzz"])));
    }

    #[test]
    fn endpoint_device_name_is_quazar_mesh_only_for_the_endpoint() {
        assert_eq!(MESH_ENDPOINT_NAME, "Quazar Mesh");
        assert_eq!(
            endpoint_device_name(true, "MDE-MESH eagle"),
            MESH_ENDPOINT_NAME
        );
        assert_eq!(endpoint_device_name(false, "MDE-MESH oak"), "MDE-MESH oak");
    }

    #[test]
    fn request_id_is_stable_and_distinguishes_actions() {
        let clip = FanoutAction::Clipboard {
            content: "hi".into(),
        };
        let ring = FanoutAction::Ring;
        assert_eq!(request_id("eagle", 10, &clip), "eagle:10:clipboard");
        assert_ne!(
            request_id("eagle", 10, &clip),
            request_id("eagle", 10, &ring)
        );
    }

    #[test]
    fn relay_then_drain_then_aggregate_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        // The endpoint (eagle) relays a clipboard action.
        let action = FanoutAction::Clipboard {
            content: "shared text".into(),
        };
        let id = request_id("eagle", 1_000, &action);
        publish_request(
            root,
            "eagle",
            &FanoutRequest {
                id: id.clone(),
                action,
                origin_host: "eagle".into(),
                ts_ms: 1_000,
            },
            16,
        )
        .unwrap();

        // A peer (oak) drains — it sees eagle's request (not its own row), applies,
        // and responds. The endpoint drains too but sees no peer request.
        let pending_oak = collect_pending_requests(root, "oak", 1_100, 60_000);
        assert_eq!(pending_oak.len(), 1);
        assert_eq!(pending_oak[0].id, id);
        assert!(collect_pending_requests(root, "eagle", 1_100, 60_000).is_empty());

        publish_response(
            root,
            "oak",
            &FanoutResponse {
                request_id: id.clone(),
                node_host: "oak".into(),
                applied: true,
                detail: "clipboard set".into(),
                ts_ms: 1_100,
            },
            16,
        )
        .unwrap();
        // A second peer (pine) also applied.
        publish_response(
            root,
            "pine",
            &FanoutResponse {
                request_id: id.clone(),
                node_host: "pine".into(),
                applied: true,
                detail: "clipboard set".into(),
                ts_ms: 1_120,
            },
            16,
        )
        .unwrap();

        // The endpoint aggregates: two peers applied.
        let agg = aggregate_responses(root, &id, 1_200, 60_000);
        assert_eq!(agg.applied, 2);
        assert_eq!(agg.responders, hosts(&["oak", "pine"]));
    }

    #[test]
    fn collect_pending_skips_self_and_stale_requests() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let fresh = FanoutRequest {
            id: "eagle:9000:ring".into(),
            action: FanoutAction::Ring,
            origin_host: "eagle".into(),
            ts_ms: 9_000,
        };
        let stale = FanoutRequest {
            id: "eagle:1000:ring".into(),
            action: FanoutAction::Ring,
            origin_host: "eagle".into(),
            ts_ms: 1_000,
        };
        publish_request(root, "eagle", &fresh, 16).unwrap();
        publish_request(root, "eagle", &stale, 16).unwrap();
        // oak sees only the fresh one; eagle (self = origin) sees nothing.
        let got = collect_pending_requests(root, "oak", 10_000, 5_000);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "eagle:9000:ring");
        assert!(collect_pending_requests(root, "eagle", 10_000, 5_000).is_empty());
    }

    #[test]
    fn a_request_row_is_idempotent_per_id_and_bounded() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mk = |ts: i64| FanoutRequest {
            id: format!("eagle:{ts}:ring"),
            action: FanoutAction::Ring,
            origin_host: "eagle".into(),
            ts_ms: ts,
        };
        // Re-publish the same id twice → one row.
        let dup = mk(5);
        publish_request(root, "eagle", &dup, 4).unwrap();
        publish_request(root, "eagle", &dup, 4).unwrap();
        // Then overflow the cap of 4.
        for ts in [10, 20, 30, 40, 50] {
            publish_request(root, "eagle", &mk(ts), 4).unwrap();
        }
        let rows = collect_pending_requests(root, "oak", 1_000_000, i64::MAX);
        assert_eq!(rows.len(), 4, "bounded to the newest 4");
        // The oldest (ts=5 dup + ts=10) were evicted; newest four remain.
        assert!(rows.iter().all(|r| r.ts_ms >= 20));
    }

    #[test]
    fn junk_and_half_written_rows_are_skipped_not_fatal() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(requests_dir(root)).unwrap();
        std::fs::write(requests_dir(root).join("bad.json"), b"not json").unwrap();
        publish_request(
            root,
            "eagle",
            &FanoutRequest {
                id: "eagle:1:ring".into(),
                action: FanoutAction::Ring,
                origin_host: "eagle".into(),
                ts_ms: 1,
            },
            8,
        )
        .unwrap();
        let got = collect_pending_requests(root, "oak", 100, i64::MAX);
        assert_eq!(got.len(), 1);
    }
}
