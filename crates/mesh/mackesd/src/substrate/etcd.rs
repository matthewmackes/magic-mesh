//! SUBSTRATE-2 (SUBSTRATE-V2) — the etcd coordination-plane client foundation.
//!
//! etcd is the strongly-consistent home for the three coordination concerns that
//! used to live as lockfiles + JSON on the LizardFS QNM-Shared mount (and took
//! the whole mesh down with the mount): **leader election**, the **peer
//! directory**, and **health**. This module is the shared foundation the three
//! migrations (SUBSTRATE-2 leader / -3 directory / -4 health) build on:
//!   * the endpoints contract — `setup-etcd.sh` (SUBSTRATE-1) writes the client
//!     URLs to [`ENDPOINTS_FILE`]; [`default_endpoints`] reads them;
//!   * the key schema — [`LEADER_KEY`] / [`peer_key`] / [`health_key`] /
//!     [`syncthing_key`] under `/mesh/`;
//!   * [`connect`] / [`probe`] over the etcd v3 client.
//!
//! No TLS: lock #11 — etcd binds the Nebula overlay (already encrypted); client-
//! cert TLS is a deferred follow-on before any non-overlay exposure.

use std::path::Path;

use etcd_client::{Client, Error, WatchOptions, WatchStream};

/// Where `setup-etcd.sh` records the comma/newline-separated client URLs this
/// node connects to (its own member on an anchor, the anchors on a workstation).
pub const ENDPOINTS_FILE: &str = "/etc/mackesd/etcd-endpoints";

/// etcd key for the leader election (lease + campaign) — replaces the
/// `.mackesd-leader.lock` advisory lockfile.
pub const LEADER_KEY: &str = "/mesh/leader";
/// Prefix for the peer directory: `/mesh/peers/<hostname>` = `PeerRecord` JSON,
/// written under a keepalive lease so liveness IS the lease (no `last_seen_ms`).
pub const PEERS_PREFIX: &str = "/mesh/peers/";
/// Prefix for per-node health keys (`/mesh/health/<hostname>`).
pub const HEALTH_PREFIX: &str = "/mesh/health/";
/// Prefix for the Syncthing device-ID registry (`/mesh/syncthing/<hostname>`),
/// so each node auto-configures the full-mesh share without public discovery.
pub const SYNCTHING_PREFIX: &str = "/mesh/syncthing/";
/// Prefix for the VDI roaming-session plane (`/mesh/vdi/sessions/<session_id>`).
/// Each session record is written under a keep-alive lease so a crashed
/// converging node's rows auto-expire (E12-5/8) — the strongly-consistent,
/// lease-backed analogue of the file store's replicated-directory scan.
pub const SESSIONS_PREFIX: &str = "/mesh/vdi/sessions/";

/// Leader lease TTL — the 60 s lease the campaign holds (matches the retired
/// [`crate::leader::LEASE_DURATION`]).
pub const LEADER_LEASE_TTL_S: i64 = 60;
/// Peer-record keepalive lease TTL — a peer's directory entry vanishes ~90 s
/// after its heartbeat stops (the lease IS liveness).
pub const PEER_LEASE_TTL_S: i64 = 90;
/// VDI session-record keepalive lease TTL. Comfortably larger than the
/// session-broker's 2 s convergence poll (so a live session is refreshed several
/// times per lease window), yet short enough that a crashed converging node's
/// sessions free within roughly a poll-and-a-lease-window.
pub const SESSION_LEASE_TTL_S: i64 = 30;

/// etcd key for a peer's directory entry.
#[must_use]
pub fn peer_key(hostname: &str) -> String {
    format!("{PEERS_PREFIX}{hostname}")
}

/// etcd key for a peer's health entry.
#[must_use]
pub fn health_key(hostname: &str) -> String {
    format!("{HEALTH_PREFIX}{hostname}")
}

/// etcd key for a peer's Syncthing device-ID registration.
#[must_use]
pub fn syncthing_key(hostname: &str) -> String {
    format!("{SYNCTHING_PREFIX}{hostname}")
}

/// etcd key for a VDI session record. The session id rides verbatim after the
/// prefix — unlike the file store, etcd keys are hierarchical byte strings, so a
/// `/` or `:` in an id needs no escaping (a prefix range still returns it and the
/// id is recovered from the record's own JSON, not the key).
#[must_use]
pub fn session_key(id: &str) -> String {
    format!("{SESSIONS_PREFIX}{id}")
}

/// Parse the endpoints file body into a clean list of client URLs. Accepts
/// comma / whitespace / newline separators; trims; drops blanks + `#` comments.
/// Pure + testable — the SUBSTRATE-1 ↔ SUBSTRATE-2 contract.
#[must_use]
pub fn parse_endpoints(raw: &str) -> Vec<String> {
    raw.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .flat_map(|l| l.split([',', ' ', '\t']))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

/// Read + parse [`ENDPOINTS_FILE`] at `path` (empty on a missing/unreadable file
/// — a node that hasn't been provisioned onto etcd yet, pre-cutover).
#[must_use]
pub fn endpoints_from_file(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .map(|s| parse_endpoints(&s))
        .unwrap_or_default()
}

/// The configured etcd client endpoints for this node (empty when etcd isn't
/// provisioned here — callers treat empty as "coordination plane not active").
#[must_use]
pub fn default_endpoints() -> Vec<String> {
    endpoints_from_file(Path::new(ENDPOINTS_FILE))
}

/// Connect an etcd v3 client to `endpoints` (no TLS — overlay-only, lock #11).
///
/// # Errors
/// An [`etcd_client::Error`] when no endpoint is reachable.
pub async fn connect(endpoints: &[String]) -> Result<Client, Error> {
    Client::connect(endpoints, None).await
}

/// Open an etcd v3 **watch stream** on `key` with `options`. The returned
/// [`WatchStream`] yields a [`etcd_client::WatchResponse`] per `message()`
/// await, each carrying the Put/Delete events for keys matching the watch — the
/// push primitive the [`crate::workers::etcd_watch`] worker uses to turn a
/// `/mesh/peers/` keepalive-lease delete or a `/mesh/leader` value change into an
/// INSTANT alert (no poll). Pass `WatchOptions::new().with_prefix()` to watch a
/// whole prefix (the peer directory) or `None` for a single key (the leader).
///
/// # Errors
/// An [`etcd_client::Error`] when no endpoint is reachable or the watch RPC is
/// rejected (the caller degrades + reconnects rather than panics, §2).
pub async fn watch(
    client: &mut Client,
    key: impl Into<Vec<u8>>,
    options: Option<WatchOptions>,
) -> Result<WatchStream, Error> {
    client.watch(key, options).await
}

/// Best-effort reachability probe: connect + a trivial range read (a get on a
/// never-written key returns Ok with no kvs, confirming the client can talk to a
/// quorum member). `false` on any connect/read error.
pub async fn probe(endpoints: &[String]) -> bool {
    if endpoints.is_empty() {
        return false;
    }
    match connect(endpoints).await {
        Ok(mut client) => client.get("__mcnf_probe__", None).await.is_ok(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_endpoints_handles_comma_newline_whitespace_and_comments() {
        let raw = "# anchors\nhttp://10.42.0.1:2379, http://10.42.0.2:2379\n\
                   http://10.42.0.3:2379\n\n  \t \n# trailing comment\n";
        assert_eq!(
            parse_endpoints(raw),
            vec![
                "http://10.42.0.1:2379".to_string(),
                "http://10.42.0.2:2379".to_string(),
                "http://10.42.0.3:2379".to_string(),
            ]
        );
    }

    #[test]
    fn parse_endpoints_empty_on_blank() {
        assert!(parse_endpoints("").is_empty());
        assert!(parse_endpoints("   \n \t \n").is_empty());
        assert!(parse_endpoints("# only a comment\n").is_empty());
    }

    #[test]
    fn endpoints_from_file_missing_is_empty() {
        assert!(endpoints_from_file(Path::new("/nonexistent/xyzzy/etcd-endpoints")).is_empty());
    }

    #[test]
    fn endpoints_from_file_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("etcd-endpoints");
        std::fs::write(&p, "http://10.42.0.1:2379,http://10.42.0.2:2379\n").unwrap();
        assert_eq!(
            endpoints_from_file(&p),
            vec![
                "http://10.42.0.1:2379".to_string(),
                "http://10.42.0.2:2379".to_string()
            ]
        );
    }

    #[test]
    fn key_schema_is_under_mesh_namespace() {
        assert_eq!(peer_key("eagle"), "/mesh/peers/eagle");
        assert_eq!(health_key("eagle"), "/mesh/health/eagle");
        assert_eq!(syncthing_key("eagle"), "/mesh/syncthing/eagle");
        assert_eq!(session_key("01J-ulid"), "/mesh/vdi/sessions/01J-ulid");
        assert_eq!(LEADER_KEY, "/mesh/leader");
        assert!(peer_key("x").starts_with(PEERS_PREFIX));
        assert!(session_key("x").starts_with(SESSIONS_PREFIX));
    }

    #[test]
    fn lease_ttls_match_the_locked_durations() {
        // Leader lease = the retired fs-lock's 60 s; peer keepalive = 90 s; VDI
        // session lease > the 2 s convergence poll so a live session is refreshed
        // several times per window.
        assert_eq!(LEADER_LEASE_TTL_S, 60);
        assert_eq!(PEER_LEASE_TTL_S, 90);
        assert_eq!(SESSION_LEASE_TTL_S, 30);
        assert!(
            SESSION_LEASE_TTL_S > 2,
            "lease must outlast the poll cadence"
        );
    }

    #[tokio::test]
    async fn probe_empty_endpoints_is_false() {
        assert!(!probe(&[]).await);
    }
}
