//! MON-1.b (v2.6) — Netdata aggregator-IP publisher worker.
//!
//! The remaining MON-1 piece. When a peer is the leader-
//! elected aggregator it publishes its overlay IP to a
//! QNM-Shared file. Every peer (including the leader) reads
//! the freshest published pointer, mirrors it to
//! `/var/lib/mackesd/netdata/aggregator-ip` for downstream
//! consumers, and rewrites `/etc/netdata/netdata.conf`'s
//! `[stream]` block to point Netdata at the aggregator.
//! Fail-soft per the v2.6 MON-1 design lock: when no
//! aggregator pointer is published (e.g. fleet still
//! converging on a leader, or all peers offline) the worker
//! removes the local `[stream]` block so Netdata falls back
//! to local-only mode with the 7-day dbengine retention the
//! `apply_netdata_monitor` birthright step locked.
//!
//! Leader detection mirrors `nebula_supervisor`'s pattern —
//! the role-host marker file's existence proxies for the
//! leader bit until `crate::leader` ships an async-services
//! read entry point.
//!
//! All disk writes are atomic (`tempfile + rename`). The
//! tick is gated on actual change — repeat ticks with the
//! same aggregator IP are no-ops, so `systemctl reload
//! netdata.service` only fires on transition.

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use super::{ShutdownToken, Worker};

/// Default tick cadence — matches the nebula supervisor so
/// promote/demote transitions converge in roughly one cycle.
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(5);

/// Default local mirror path consumed by birthright + the
/// future MON-5 panel. Plain-text overlay-IP, one line.
pub const DEFAULT_AGGREGATOR_IP_PATH: &str = "/var/lib/mackesd/netdata/aggregator-ip";

/// Default Netdata config the `[stream]` block lives in.
pub const DEFAULT_NETDATA_CONF: &str = "/etc/netdata/netdata.conf";

/// Default destination port — Netdata's standard streaming
/// port. Hard-coded because the v2.6 substrate locks it.
pub const DEFAULT_STREAM_PORT: u16 = 19999;

/// Default source-of-truth for this peer's overlay IP. The
/// `nebula_supervisor` GF-1.3.a writer keeps this file in
/// sync with the active Nebula bundle.
pub const DEFAULT_OVERLAY_IP_SOURCE: &str = "/var/lib/mackesd/nebula/overlay-ip";

/// Default role-host marker — its existence proxies for the
/// "this peer is the active leader" bit. Mirrors
/// `nebula_supervisor`'s `check_leader` shape so a single
/// promote/demote action lifts both subsystems.
pub const DEFAULT_ROLE_HOST_MARKER: &str = "/var/lib/mackesd/nebula/role.host";

/// Published-pointer schema. Mesh-replicated under
/// `<workgroup_root>/<self>/mackesd/netdata-aggregator.json`. The
/// `epoch_s` field carries the publish timestamp so the
/// reader picks the freshest pointer when more than one peer
/// has held the leader role recently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AggregatorPointer {
    /// `node_id` of the peer currently holding the aggregator
    /// leader lock.
    pub node_id: String,
    /// Nebula overlay IP children stream telemetry to.
    pub overlay_ip: String,
    /// Unix-epoch seconds when this pointer was published.
    /// Readers compare timestamps when multiple peers have
    /// recently held the leader role.
    pub epoch_s: u64,
}

/// Result of `apply_aggregator_ip` — lets the worker skip
/// the netdata reload when the local mirror byte-matches the
/// desired payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// Nothing on disk needed touching.
    Unchanged,
    /// Wrote a new aggregator IP.
    Updated,
    /// Removed the mirror file (no aggregator published).
    Cleared,
}

/// Result of `rewrite_stream_block` — distinguishes the
/// no-op case from the actual edit case so the worker can
/// skip a netdata reload when the on-disk config already
/// matches the desired stream block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamRewriteOutcome {
    /// Existing config already matches the desired block.
    Unchanged,
    /// Wrote a new netdata.conf with the desired block.
    Updated,
}

/// Worker handle.
pub struct NetdataAggregator {
    store: Arc<Mutex<rusqlite::Connection>>,
    node_id: String,
    workgroup_root: PathBuf,
    aggregator_ip_path: PathBuf,
    netdata_conf_path: PathBuf,
    overlay_ip_source: PathBuf,
    role_marker_path: PathBuf,
    stream_port: u16,
    api_key: String,
    tick_interval: Duration,
    last_published: Option<AggregatorPointer>,
    last_aggregator_ip: Option<String>,
}

impl NetdataAggregator {
    /// Construct an aggregator worker bound to the given
    /// store + node-id + mesh root.
    ///
    /// `api_key` is the shared Netdata stream-handshake
    /// secret used by every peer in the mesh. The boot
    /// wizard derives this from the mesh-id so every peer
    /// in the same mesh shares the value automatically;
    /// passing it in explicitly keeps this worker decoupled
    /// from the wizard's key-derivation rule.
    #[must_use]
    pub fn new(
        store: Arc<Mutex<rusqlite::Connection>>,
        node_id: String,
        workgroup_root: PathBuf,
        api_key: String,
    ) -> Self {
        Self {
            store,
            node_id,
            workgroup_root,
            aggregator_ip_path: PathBuf::from(DEFAULT_AGGREGATOR_IP_PATH),
            netdata_conf_path: PathBuf::from(DEFAULT_NETDATA_CONF),
            overlay_ip_source: PathBuf::from(DEFAULT_OVERLAY_IP_SOURCE),
            role_marker_path: PathBuf::from(DEFAULT_ROLE_HOST_MARKER),
            stream_port: DEFAULT_STREAM_PORT,
            api_key,
            tick_interval: DEFAULT_TICK_INTERVAL,
            last_published: None,
            last_aggregator_ip: None,
        }
    }

    /// Override the local mirror path — used by tests that
    /// can't write to /var.
    #[must_use]
    pub fn with_aggregator_ip_path(mut self, p: PathBuf) -> Self {
        self.aggregator_ip_path = p;
        self
    }

    /// Override the netdata.conf path — used by tests.
    #[must_use]
    pub fn with_netdata_conf_path(mut self, p: PathBuf) -> Self {
        self.netdata_conf_path = p;
        self
    }

    /// Override the overlay-ip source file — used by tests.
    #[must_use]
    pub fn with_overlay_ip_source(mut self, p: PathBuf) -> Self {
        self.overlay_ip_source = p;
        self
    }

    /// Override the role-host marker path — used by tests
    /// that simulate leader promote/demote.
    #[must_use]
    pub fn with_role_marker_path(mut self, p: PathBuf) -> Self {
        self.role_marker_path = p;
        self
    }

    /// Override the streaming port — used by tests + by
    /// operators running Netdata on a non-default port.
    #[must_use]
    pub fn with_stream_port(mut self, port: u16) -> Self {
        self.stream_port = port;
        self
    }

    /// Override the tick cadence — used by tests.
    #[must_use]
    pub fn with_tick_interval(mut self, d: Duration) -> Self {
        self.tick_interval = d;
        self
    }

    /// One sweep. Touches disk, may shell to systemctl.
    /// Errors at any individual step are logged and
    /// swallowed so a single bad tick can't kill the worker.
    async fn tick(&mut self) {
        // 1. Publish own pointer when leader.
        let is_leader = check_leader(&self.store, &self.node_id, &self.role_marker_path).await;
        if is_leader {
            if let Err(e) = self.publish_self_pointer() {
                tracing::warn!(error = %e, "netdata-aggregator: publish failed");
            }
        }

        // 2. Always: pick the freshest pointer from the
        //    mesh + materialize it locally.
        let pointers = scan_aggregator_pointers(&self.workgroup_root);
        let chosen = latest_aggregator(pointers);
        let desired_ip = chosen.as_ref().map(|p| p.overlay_ip.clone());

        let apply = apply_aggregator_ip(&self.aggregator_ip_path, desired_ip.as_deref());
        let apply = match apply {
            Ok(o) => o,
            Err(e) => {
                tracing::warn!(error = %e, "netdata-aggregator: mirror write failed");
                ApplyOutcome::Unchanged
            }
        };

        // 3. Rewrite netdata.conf's [stream] block when the
        //    aggregator IP changed. Skip the reload when
        //    nothing changed OR when only our own pointer's
        //    epoch bumped.
        let aggregator_changed = match (&self.last_aggregator_ip, &desired_ip) {
            (Some(prev), Some(cur)) => prev != cur,
            (None, Some(_)) | (Some(_), None) => true,
            (None, None) => false,
        };
        if aggregator_changed || apply == ApplyOutcome::Updated || apply == ApplyOutcome::Cleared {
            if let Err(e) = self.refresh_stream_block(desired_ip.as_deref()) {
                tracing::warn!(error = %e, "netdata-aggregator: stream-block rewrite failed");
            }
        }
        self.last_aggregator_ip = desired_ip;

        // NETDATA-1 (2026-06-17) — confine + expose the dashboard's [web] bind on
        // EVERY tick, independent of the [stream] aggregator state. The PD-7 map
        // fetches each peer's `<overlay-ip>:19999`, so every node must bind its
        // own overlay IP (plus loopback) — and must NEVER bind 0.0.0.0 on a public
        // lighthouse. Previously this was bundled into refresh_stream_block, which
        // only ran when the aggregator IP changed, so a node with no aggregator
        // pointer kept netdata on loopback-only (unreachable to peers) or, worse,
        // on the stock 0.0.0.0. Idempotent: only writes + reloads when it changes.
        if let Err(e) = self.refresh_web_block() {
            tracing::warn!(error = %e, "netdata-aggregator: web-block confine failed");
        }
    }

    /// Confine netdata's dashboard to loopback + this node's overlay IP (so peers
    /// can fetch metrics over nebula but the underlay/public never can). Runs
    /// every tick; idempotent (only rewrites + reloads when the block changes).
    fn refresh_web_block(&self) -> Result<(), String> {
        // No conf yet (netdata not installed) → nothing to confine.
        let existing = match std::fs::read_to_string(&self.netdata_conf_path) {
            Ok(s) => s,
            Err(_) => return Ok(()),
        };
        let overlay_ip = read_overlay_ip(&self.overlay_ip_source).ok();
        let web_block = build_web_block(overlay_ip.as_deref());
        let rewritten = rewrite_named_section(&existing, "[web]", Some(&web_block));
        if rewritten != existing {
            atomic_write(&self.netdata_conf_path, rewritten.as_bytes())?;
            let _ = systemctl_reload("netdata.service");
        }
        Ok(())
    }

    fn publish_self_pointer(&mut self) -> Result<(), String> {
        let overlay = read_overlay_ip(&self.overlay_ip_source)?;
        let epoch_s = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let pointer = AggregatorPointer {
            node_id: self.node_id.clone(),
            overlay_ip: overlay,
            epoch_s,
        };
        // Skip the write when only the epoch_s would tick
        // (saves a QNM-Shared touch + downstream mtime).
        if let Some(prev) = &self.last_published {
            if prev.node_id == pointer.node_id && prev.overlay_ip == pointer.overlay_ip {
                return Ok(());
            }
        }
        let path = self_pointer_path(&self.workgroup_root, &self.node_id);
        write_pointer(&path, &pointer)?;
        self.last_published = Some(pointer);
        Ok(())
    }

    fn refresh_stream_block(&self, aggregator_ip: Option<&str>) -> Result<(), String> {
        let existing = std::fs::read_to_string(&self.netdata_conf_path).unwrap_or_default();
        // When self is the aggregator, strip the [stream]
        // block (parent doesn't stream to itself).
        let am_aggregator = aggregator_ip
            == Some(
                read_overlay_ip(&self.overlay_ip_source)
                    .unwrap_or_default()
                    .as_str(),
            );
        let target_ip = if am_aggregator { None } else { aggregator_ip };
        let block = build_stream_block(target_ip, self.stream_port, &self.api_key);
        let rewritten = rewrite_stream_block(&existing, block.as_deref());
        // EFF-22 — also confine the dashboard's [web] bind to loopback +
        // this node's overlay IP so :19999 is never exposed on the underlay.
        let overlay_ip = read_overlay_ip(&self.overlay_ip_source).ok();
        let web_block = build_web_block(overlay_ip.as_deref());
        let rewritten = rewrite_named_section(&rewritten, "[web]", Some(&web_block));
        let outcome = if rewritten == existing {
            StreamRewriteOutcome::Unchanged
        } else {
            atomic_write(&self.netdata_conf_path, rewritten.as_bytes())?;
            StreamRewriteOutcome::Updated
        };
        if outcome == StreamRewriteOutcome::Updated {
            // Best-effort reload; failures log + continue
            // so a missing systemctl in CI doesn't poison
            // the worker.
            let _ = systemctl_reload("netdata.service");
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl Worker for NetdataAggregator {
    fn name(&self) -> &'static str {
        "netdata_aggregator"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            tokio::select! {
                _ = shutdown.wait() => return Ok(()),
                _ = tokio::time::sleep(self.tick_interval) => {
                    self.tick().await;
                }
            }
        }
    }
}

// -- pure helpers --------------------------------------------------

/// Read the local overlay IP from
/// `/var/lib/mackesd/nebula/overlay-ip` (or test override).
/// Strips trailing whitespace; empty file is an error.
///
/// # Errors
///
/// Returns the formatted underlying I/O error string on
/// read failure, or `"empty"` when the file contains only
/// whitespace.
pub fn read_overlay_ip(path: &Path) -> Result<String, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(format!("{} is empty", path.display()));
    }
    Ok(trimmed.to_owned())
}

/// Path the worker writes its own pointer to.
#[must_use]
pub fn self_pointer_path(workgroup_root: &Path, node_id: &str) -> PathBuf {
    workgroup_root
        .join(node_id)
        .join("mackesd")
        .join("netdata-aggregator.json")
}

/// Atomic-write a pointer JSON. Creates parent dirs as
/// needed.
///
/// # Errors
///
/// Returns the underlying I/O error formatted as a string
/// when directory creation, write, or rename fails. Also
/// surfaces `serde_json` serialization errors.
pub fn write_pointer(path: &Path, pointer: &AggregatorPointer) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let body = serde_json::to_vec_pretty(pointer).map_err(|e| format!("serialize pointer: {e}"))?;
    atomic_write(path, &body)
}

/// Walk `<workgroup_root>/*/mackesd/netdata-aggregator.json` and
/// deserialize every pointer that parses. Pointers that
/// fail to parse are skipped (the readwriter is forward-
/// compatible to unknown fields via `serde_json` default
/// behavior, but a wholly malformed file shouldn't kill the
/// scan).
#[must_use]
pub fn scan_aggregator_pointers(workgroup_root: &Path) -> Vec<AggregatorPointer> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(workgroup_root) else {
        return out;
    };
    for entry in entries.flatten() {
        let candidate = entry.path().join("mackesd").join("netdata-aggregator.json");
        let Ok(bytes) = std::fs::read(&candidate) else {
            continue;
        };
        if let Ok(p) = serde_json::from_slice::<AggregatorPointer>(&bytes) {
            out.push(p);
        }
    }
    out
}

/// Pick the freshest pointer by `epoch_s`. Ties broken
/// lexicographically on `node_id` so the choice is
/// deterministic across peers.
#[must_use]
pub fn latest_aggregator(mut pointers: Vec<AggregatorPointer>) -> Option<AggregatorPointer> {
    pointers.sort_by(|a, b| {
        b.epoch_s
            .cmp(&a.epoch_s)
            .then_with(|| a.node_id.cmp(&b.node_id))
    });
    pointers.into_iter().next()
}

/// Mirror the chosen aggregator IP to the local
/// plain-text file. Empty `desired` (no aggregator
/// published) removes the file so downstream consumers
/// can detect the "no aggregator" state by absence.
///
/// # Errors
///
/// Returns the formatted underlying I/O error string when
/// the directory create, atomic-write, or remove fails.
pub fn apply_aggregator_ip(path: &Path, desired: Option<&str>) -> Result<ApplyOutcome, String> {
    let existing = std::fs::read_to_string(path).ok();
    match (existing.as_deref().map(str::trim), desired) {
        (Some(prev), Some(want)) if prev == want => Ok(ApplyOutcome::Unchanged),
        (None, None) => Ok(ApplyOutcome::Unchanged),
        (_, Some(want)) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
            }
            let body = format!("{want}\n");
            atomic_write(path, body.as_bytes())?;
            Ok(ApplyOutcome::Updated)
        }
        (Some(_), None) => {
            std::fs::remove_file(path).map_err(|e| format!("remove {}: {e}", path.display()))?;
            Ok(ApplyOutcome::Cleared)
        }
    }
}

/// Build the `[stream]` block body that gets spliced into
/// `netdata.conf`. Returns `None` when no aggregator IP is
/// supplied — the caller drops the block entirely in that
/// case.
#[must_use]
pub fn build_stream_block(aggregator_ip: Option<&str>, port: u16, api_key: &str) -> Option<String> {
    let ip = aggregator_ip?;
    Some(format!(
        "[stream]\n\
         \x20\x20\x20\x20# Written by mackesd netdata_aggregator (MON-1.b, v2.6).\n\
         \x20\x20\x20\x20# Don't edit by hand — every tick byte-checks + rewrites.\n\
         \x20\x20\x20\x20enabled = yes\n\
         \x20\x20\x20\x20destination = {ip}:{port}\n\
         \x20\x20\x20\x20api key = {api_key}\n"
    ))
}

/// Replace the existing `[stream]` block in `netdata.conf`
/// with the new one. When `block` is `None`, strip any
/// existing `[stream]` block. When `block` is `Some(text)`,
/// replace the existing one or append it after the last
/// pre-existing section.
///
/// Section boundaries follow Netdata's INI dialect: a
/// section starts at a line whose first non-whitespace
/// character is `[` and ends at the next such line (or EOF).
/// Comments + blank lines inside a section count as section
/// body.
#[must_use]
pub fn rewrite_stream_block(existing: &str, block: Option<&str>) -> String {
    rewrite_named_section(existing, "[stream]", block)
}

/// EFF-22 — build the `[web]` block confining the Netdata dashboard
/// (`:19999`) to loopback + the overlay IP, never the underlay/default
/// `0.0.0.0`. With no overlay IP yet (pre-enrolment) it binds loopback
/// only — the dashboard is reachable locally but never exposed off-box.
#[must_use]
pub fn build_web_block(overlay_ip: Option<&str>) -> String {
    let bind = match overlay_ip {
        Some(ip) if !ip.is_empty() => format!("127.0.0.1 {ip}"),
        _ => "127.0.0.1".to_string(),
    };
    format!(
        "[web]\n\
         \x20\x20\x20\x20# Written by mackesd netdata_aggregator (EFF-22).\n\
         \x20\x20\x20\x20# Confine the :19999 dashboard to loopback + overlay,\n\
         \x20\x20\x20\x20# never the underlay. Don't edit by hand.\n\
         \x20\x20\x20\x20bind to = {bind}\n"
    )
}

/// Replace (or strip/append) a named `[section]` block in a Netdata
/// `netdata.conf`. `header` is the exact bracketed section name (e.g.
/// `[stream]`). When `block` is `None` any existing section is removed;
/// `Some(text)` replaces the existing one or appends after the last
/// pre-existing section.
///
/// Section boundaries follow Netdata's INI dialect: a section starts at
/// a line whose first non-whitespace character is `[` and ends at the
/// next such line (or EOF). Comments + blank lines inside a section
/// count as section body.
#[must_use]
pub fn rewrite_named_section(existing: &str, header: &str, block: Option<&str>) -> String {
    let mut sections: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut in_target = false;
    for line in existing.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            if !current.is_empty() {
                sections.push(std::mem::take(&mut current));
            }
            in_target = trimmed.starts_with(header);
            if !in_target {
                current.push(line.to_owned());
            }
        } else if !in_target {
            current.push(line.to_owned());
        }
    }
    if !current.is_empty() {
        sections.push(current);
    }
    let mut out: String = sections.into_iter().flatten().collect();
    if let Some(text) = block {
        // Collapse the trailing blank-line run to a single separator so
        // re-running over an already-rewritten conf is idempotent — a
        // non-idempotent append would grow a blank line per tick and
        // churn a netdata reload every cycle.
        while out.ends_with('\n') {
            out.pop();
        }
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(text);
    }
    out
}

fn atomic_write(path: &Path, body: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, body).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| format!("rename {} → {}: {e}", tmp.display(), path.display()))
}

fn systemctl_reload(unit: &str) -> Result<(), String> {
    // EFF-20 — bound systemctl so a hung reload can't pin the tick.
    let mut cmd = std::process::Command::new("systemctl");
    cmd.args(["reload-or-restart", unit]);
    let out =
        crate::workers::proc::output_with_timeout(cmd, crate::workers::proc::DEFAULT_CMD_TIMEOUT)
            .map_err(|e| format!("systemctl reload-or-restart {unit}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
    }
}

async fn check_leader(
    _store: &Arc<Mutex<rusqlite::Connection>>,
    _node_id: &str,
    role_marker_path: &Path,
) -> bool {
    // Mirrors nebula_supervisor::check_leader's pattern.
    // The role-host marker file's existence proxies for the
    // leader bit until crate::leader exposes an
    // async-services read entry point.
    role_marker_path.exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_overlay_ip_trims_trailing_newline() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = tmp.path().join("overlay-ip");
        std::fs::write(&p, "10.42.0.5\n").expect("write");
        assert_eq!(read_overlay_ip(&p).expect("read"), "10.42.0.5");
    }

    #[test]
    fn read_overlay_ip_handles_ipv6() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = tmp.path().join("overlay-ip");
        std::fs::write(&p, "fd42::5\n").expect("write");
        assert_eq!(read_overlay_ip(&p).expect("read"), "fd42::5");
    }

    #[test]
    fn read_overlay_ip_empty_file_errors() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = tmp.path().join("overlay-ip");
        std::fs::write(&p, "  \n").expect("write");
        assert!(read_overlay_ip(&p).is_err());
    }

    #[test]
    fn read_overlay_ip_missing_file_errors() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = tmp.path().join("nope");
        assert!(read_overlay_ip(&p).is_err());
    }

    #[test]
    fn write_pointer_roundtrips_through_scan() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let pointer = AggregatorPointer {
            node_id: "peer:self".into(),
            overlay_ip: "10.42.0.5".into(),
            epoch_s: 1_716_000_000,
        };
        let p = self_pointer_path(tmp.path(), &pointer.node_id);
        write_pointer(&p, &pointer).expect("write");
        let scanned = scan_aggregator_pointers(tmp.path());
        assert_eq!(scanned, vec![pointer]);
    }

    #[test]
    fn scan_aggregator_pointers_missing_root_returns_empty() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let nope = tmp.path().join("does-not-exist");
        assert!(scan_aggregator_pointers(&nope).is_empty());
    }

    #[test]
    fn scan_aggregator_pointers_skips_unparseable() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let bad = tmp.path().join("peer:bad").join("mackesd");
        std::fs::create_dir_all(&bad).expect("mkdir");
        std::fs::write(bad.join("netdata-aggregator.json"), b"not json").expect("write");
        let good = AggregatorPointer {
            node_id: "peer:good".into(),
            overlay_ip: "10.42.0.7".into(),
            epoch_s: 100,
        };
        let gp = self_pointer_path(tmp.path(), &good.node_id);
        write_pointer(&gp, &good).expect("write");
        let scanned = scan_aggregator_pointers(tmp.path());
        assert_eq!(scanned.len(), 1);
        assert_eq!(scanned[0].node_id, "peer:good");
    }

    #[test]
    fn latest_aggregator_picks_newest_epoch() {
        let pointers = vec![
            AggregatorPointer {
                node_id: "peer:a".into(),
                overlay_ip: "10.42.0.1".into(),
                epoch_s: 100,
            },
            AggregatorPointer {
                node_id: "peer:b".into(),
                overlay_ip: "10.42.0.2".into(),
                epoch_s: 200,
            },
            AggregatorPointer {
                node_id: "peer:c".into(),
                overlay_ip: "10.42.0.3".into(),
                epoch_s: 150,
            },
        ];
        let chosen = latest_aggregator(pointers).expect("some");
        assert_eq!(chosen.node_id, "peer:b");
    }

    #[test]
    fn latest_aggregator_breaks_ties_lexicographically_for_determinism() {
        let pointers = vec![
            AggregatorPointer {
                node_id: "peer:z".into(),
                overlay_ip: "10.42.0.26".into(),
                epoch_s: 100,
            },
            AggregatorPointer {
                node_id: "peer:a".into(),
                overlay_ip: "10.42.0.1".into(),
                epoch_s: 100,
            },
        ];
        let chosen = latest_aggregator(pointers).expect("some");
        assert_eq!(chosen.node_id, "peer:a");
    }

    #[test]
    fn latest_aggregator_empty_input_returns_none() {
        assert!(latest_aggregator(Vec::new()).is_none());
    }

    #[test]
    fn apply_aggregator_ip_writes_new() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = tmp.path().join("aggregator-ip");
        let outcome = apply_aggregator_ip(&p, Some("10.42.0.5")).expect("apply");
        assert_eq!(outcome, ApplyOutcome::Updated);
        let body = std::fs::read_to_string(&p).expect("read");
        assert_eq!(body, "10.42.0.5\n");
    }

    #[test]
    fn apply_aggregator_ip_unchanged_when_bytes_match() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = tmp.path().join("aggregator-ip");
        std::fs::write(&p, "10.42.0.5\n").expect("seed");
        let outcome = apply_aggregator_ip(&p, Some("10.42.0.5")).expect("apply");
        assert_eq!(outcome, ApplyOutcome::Unchanged);
    }

    #[test]
    fn apply_aggregator_ip_clears_when_desired_is_none() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = tmp.path().join("aggregator-ip");
        std::fs::write(&p, "10.42.0.5\n").expect("seed");
        let outcome = apply_aggregator_ip(&p, None).expect("apply");
        assert_eq!(outcome, ApplyOutcome::Cleared);
        assert!(!p.exists());
    }

    #[test]
    fn apply_aggregator_ip_unchanged_when_both_absent() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = tmp.path().join("aggregator-ip");
        let outcome = apply_aggregator_ip(&p, None).expect("apply");
        assert_eq!(outcome, ApplyOutcome::Unchanged);
    }

    #[test]
    fn build_stream_block_returns_none_when_ip_missing() {
        assert!(build_stream_block(None, 19999, "k").is_none());
    }

    #[test]
    fn build_stream_block_renders_destination_port_apikey() {
        let block = build_stream_block(Some("10.42.0.5"), 19999, "mesh-id-abc").expect("some");
        assert!(block.starts_with("[stream]\n"));
        assert!(block.contains("destination = 10.42.0.5:19999\n"));
        assert!(block.contains("api key = mesh-id-abc\n"));
        assert!(block.contains("enabled = yes\n"));
    }

    #[test]
    fn rewrite_stream_block_appends_when_absent() {
        let existing = "[global]\n    memory mode = dbengine\n";
        let block = build_stream_block(Some("10.42.0.5"), 19999, "k").expect("some");
        let out = rewrite_stream_block(existing, Some(&block));
        assert!(out.starts_with("[global]\n"));
        assert!(out.contains("[stream]\n"));
        assert!(out.contains("destination = 10.42.0.5:19999\n"));
    }

    #[test]
    fn rewrite_stream_block_replaces_existing() {
        let existing = "[global]\n    memory mode = dbengine\n\n\
                        [stream]\n    destination = 10.42.0.99:19999\n    api key = old\n\n\
                        [web]\n    bind to = 127.0.0.1\n";
        let block = build_stream_block(Some("10.42.0.5"), 19999, "k").expect("some");
        let out = rewrite_stream_block(existing, Some(&block));
        assert!(out.contains("destination = 10.42.0.5:19999\n"));
        assert!(!out.contains("10.42.0.99"));
        assert!(out.contains("[web]\n"));
        assert!(out.contains("bind to = 127.0.0.1\n"));
    }

    #[test]
    fn rewrite_stream_block_strips_when_block_is_none() {
        let existing = "[global]\n    memory mode = dbengine\n\n\
                        [stream]\n    destination = 10.42.0.99:19999\n\n\
                        [web]\n    bind to = 127.0.0.1\n";
        let out = rewrite_stream_block(existing, None);
        assert!(!out.contains("[stream]"));
        assert!(!out.contains("10.42.0.99"));
        assert!(out.contains("[global]"));
        assert!(out.contains("[web]"));
    }

    #[test]
    fn rewrite_stream_block_handles_empty_input() {
        let block = build_stream_block(Some("10.42.0.5"), 19999, "k").expect("some");
        let out = rewrite_stream_block("", Some(&block));
        assert_eq!(out, block);
    }

    #[test]
    fn rewrite_stream_block_no_op_on_empty_in_none_out() {
        assert_eq!(rewrite_stream_block("", None), "");
    }

    #[test]
    fn rewrite_stream_block_preserves_section_order() {
        let existing =
            "[global]\n    a = 1\n[cloud]\n    enabled = no\n[plugins]\n    python.d = yes\n";
        let block = build_stream_block(Some("10.42.0.5"), 19999, "k").expect("some");
        let out = rewrite_stream_block(existing, Some(&block));
        let pos_global = out.find("[global]").expect("global");
        let pos_cloud = out.find("[cloud]").expect("cloud");
        let pos_plugins = out.find("[plugins]").expect("plugins");
        let pos_stream = out.find("[stream]").expect("stream");
        assert!(pos_global < pos_cloud);
        assert!(pos_cloud < pos_plugins);
        // Stream block always trails the existing sections.
        assert!(pos_plugins < pos_stream);
    }

    #[test]
    fn build_web_block_confines_to_overlay_and_loopback() {
        // EFF-22 — with an overlay IP, bind to loopback + overlay only.
        let block = build_web_block(Some("10.42.0.5"));
        assert!(block.contains("bind to = 127.0.0.1 10.42.0.5"));
        assert!(!block.contains("0.0.0.0"));
        // Pre-enrolment (no overlay IP): loopback only.
        let none = build_web_block(None);
        assert!(none.contains("bind to = 127.0.0.1\n"));
        assert!(!none.contains("0.0.0.0"));
    }

    #[test]
    fn rewrite_named_section_replaces_default_web_bind() {
        // A stock netdata.conf binding 0.0.0.0 must be rewritten to the
        // confined block; the result must not leave the wildcard bind.
        let existing = "[global]\n    memory mode = dbengine\n\n\
                        [web]\n    bind to = 0.0.0.0\n    mode = static-threaded\n";
        let web = build_web_block(Some("10.42.0.7"));
        let out = rewrite_named_section(existing, "[web]", Some(&web));
        assert!(
            !out.contains("0.0.0.0"),
            "wildcard bind must be gone: {out}"
        );
        assert!(out.contains("bind to = 127.0.0.1 10.42.0.7"));
        assert!(out.contains("[global]"));
        // Idempotent: re-running over the rewritten conf is a no-op.
        let again = rewrite_named_section(&out, "[web]", Some(&web));
        assert_eq!(again, out);
    }

    #[test]
    fn self_pointer_path_lays_under_node_id() {
        let root = PathBuf::from("/q");
        let p = self_pointer_path(&root, "peer:foo");
        assert_eq!(
            p,
            PathBuf::from("/q/peer:foo/mackesd/netdata-aggregator.json")
        );
    }
}
