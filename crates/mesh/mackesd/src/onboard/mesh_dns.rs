//! OW-6 — `mackesd onboard mesh-dns`: make nodes resolvable by name over the
//! overlay.
//!
//! Operators should reach a peer as `<host>.<mesh-id>` (e.g. `anvil.home-mesh`)
//! rather than memorizing its Nebula overlay IP. This verb folds the replicated
//! peer roster into a name→IP **zone** and renders it into a managed
//! `/etc/hosts` block that a resolver reads.
//!
//! The shape mirrors the sibling onboard verbs (`role_provision`, `self_test`): a
//! pure core the unit tests pin, plus a thin injectable shell for the live write.
//! * [`build_zone`] — pure: `[PeerRecord] + mesh-id → [(name, overlay-IP)]`.
//! * [`render_hosts`] — pure: a zone → the delimited managed `/etc/hosts` block
//!   (re-runs *replace* the block, they never append).
//! * [`apply`] — folds a zone through an injectable [`HostsSink`] (production
//!   [`EtcHosts`] writes `/etc/hosts`; tests pass a recorder), idempotent: a
//!   re-run with the same roster writes nothing.
//!
//! # Reuse, not reinvention (§6)
//! The roster is the PEERVER-1 own-row directory
//! ([`mackes_mesh_types::peers`]): each node writes its own
//! `<mesh-home>/peers/<hostname>.json` (with its `overlay_ip`) on the heartbeat
//! tick, Syncthing replicates the dir, and [`read_peers`] unions it. So the
//! zone already carries **this node** (its own-authored row) alongside every
//! peer — no separate self-lookup. The `overlay_ip` field exists precisely for
//! this consumer (its doc names "Mesh DNS").
//!
//! # Served + synced by the CA holder
//! The CA-holder runs this verb to write the *authoritative* block; peers pick it
//! up over the **existing** mesh file-sync (Syncthing / `/mnt/mesh-storage`) — no
//! new sync mechanism is built here. This founding, mesh-id-scoped block is
//! distinct from the runtime `.mesh`-suffixed reconciler in
//! [`crate::workers::mesh_dns`]; the two use different managed-block markers so
//! neither clobbers the other.

use std::fmt::Write as _;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use mackes_mesh_types::peers::{self, PeerRecord};

/// Opening sentinel of the onboard mesh-DNS managed block.
///
/// Deliberately distinct from [`crate::workers::mesh_dns::HOSTS_BEGIN`] so the
/// founding block and the runtime reconciler's block coexist in `/etc/hosts`
/// without overwriting each other.
pub const HOSTS_BEGIN: &str = "# >>> mde onboard mesh-dns (managed) >>>";
/// Closing sentinel of the onboard mesh-DNS managed block.
pub const HOSTS_END: &str = "# <<< mde onboard mesh-dns <<<";

/// Default resolver artifact: the system hosts file.
pub const DEFAULT_HOSTS_PATH: &str = "/etc/hosts";

/// Pure fold: turn the peer roster into the `<host>.<mesh_id>` → overlay-IP zone.
///
/// One entry per peer whose `overlay_ip` parses as an [`IpAddr`]; a peer with no
/// overlay IP (or an unparseable one) is **skipped** rather than emitting a half
/// record. Because the roster is the own-row directory, `peers` already includes
/// this node's own row — the zone covers this node plus every peer. The result is
/// sorted + deduped, so the same roster always yields byte-identical output (an
/// idempotent [`render_hosts`] / [`apply`]).
#[must_use]
pub fn build_zone(peers: &[PeerRecord], mesh_id: &str) -> Vec<(String, IpAddr)> {
    let mut out: Vec<(String, IpAddr)> = peers
        .iter()
        .filter_map(|p| {
            let ip: IpAddr = p.overlay_ip.as_deref()?.trim().parse().ok()?;
            Some((format!("{}.{mesh_id}", p.hostname), ip))
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Pure render: the delimited managed `/etc/hosts` block for `zone`.
///
/// The [`HOSTS_BEGIN`]/[`HOSTS_END`] sentinels are what make a re-write *replace*
/// the prior block instead of appending — [`splice_hosts`] keys off them. An
/// empty zone renders as the empty string (there is nothing to serve, so the
/// block is removed rather than left as an empty husk). Lines are `<ip>\t<name>`,
/// in the order `zone` supplies (canonically sorted by [`build_zone`]).
#[must_use]
pub fn render_hosts(zone: &[(String, IpAddr)]) -> String {
    if zone.is_empty() {
        return String::new();
    }
    let mut s = String::new();
    let _ = writeln!(s, "{HOSTS_BEGIN}");
    for (name, ip) in zone {
        let _ = writeln!(s, "{ip}\t{name}");
    }
    let _ = writeln!(s, "{HOSTS_END}");
    s
}

/// Pure splice: drop any prior onboard managed block from `existing` and append
/// `block`, preserving every other line.
///
/// Idempotent: splicing the same `block` into an already-spliced file reproduces
/// it byte-for-byte (trailing blank lines are trimmed so newlines never accrete).
/// An empty `block` removes the managed block and leaves the rest untouched.
#[must_use]
pub fn splice_hosts(existing: &str, block: &str) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let mut in_block = false;
    for line in existing.lines() {
        let t = line.trim();
        if t == HOSTS_BEGIN {
            in_block = true;
            continue;
        }
        if t == HOSTS_END {
            in_block = false;
            continue;
        }
        if !in_block {
            kept.push(line);
        }
    }
    while kept.last().is_some_and(|l| l.trim().is_empty()) {
        kept.pop();
    }
    let mut out = kept.join("\n");
    if block.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        return out;
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(block);
    out
}

/// Injectable seam over the resolver artifact, so [`apply`] is testable without
/// touching the real `/etc/hosts`. Production wires [`EtcHosts`]; tests pass a
/// recorder.
pub trait HostsSink {
    /// Current content of the target file (empty when it is absent).
    fn read(&self) -> String;

    /// Overwrite the target file with `content`.
    ///
    /// # Errors
    /// A human-readable message when the write fails.
    fn write(&self, content: &str) -> Result<(), String>;
}

/// Production [`HostsSink`]: reads/writes a real hosts file (defaults to
/// `/etc/hosts`; [`EtcHosts::at`] targets a `hosts.d` fragment or a test path).
pub struct EtcHosts {
    path: PathBuf,
}

impl Default for EtcHosts {
    fn default() -> Self {
        Self {
            path: PathBuf::from(DEFAULT_HOSTS_PATH),
        }
    }
}

impl EtcHosts {
    /// Target a specific hosts file (a `hosts.d` fragment, or a test path).
    #[must_use]
    pub const fn at(path: PathBuf) -> Self {
        Self { path }
    }
}

impl HostsSink for EtcHosts {
    fn read(&self) -> String {
        std::fs::read_to_string(&self.path).unwrap_or_default()
    }

    fn write(&self, content: &str) -> Result<(), String> {
        std::fs::write(&self.path, content)
            .map_err(|e| format!("write {}: {e}", self.path.display()))
    }
}

/// What [`apply`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApplyOutcome {
    /// Names published in the managed block.
    pub names: usize,
    /// Whether the target file actually changed (`false` ⇒ the block was already
    /// current — an idempotent re-run).
    pub changed: bool,
}

/// Apply a `zone` through `sink`: splice the rendered managed block into the
/// target file, writing only when the content actually changes.
///
/// Idempotent — a re-run with the same roster produces an identical file, so
/// [`ApplyOutcome::changed`] is `false` and nothing is written.
///
/// # Errors
/// Propagates a [`HostsSink::write`] failure.
pub fn apply(zone: &[(String, IpAddr)], sink: &dyn HostsSink) -> Result<ApplyOutcome, String> {
    let existing = sink.read();
    let block = render_hosts(zone);
    let next = splice_hosts(&existing, &block);
    let changed = next != existing;
    if changed {
        sink.write(&next)?;
    }
    Ok(ApplyOutcome {
        names: zone.len(),
        changed,
    })
}

/// Impure shell: read the replicated peer roster off `workgroup_root` and fold it
/// into the mesh-DNS zone for `mesh_id`.
///
/// The roster read plus the pure [`build_zone`] in one call, for the CLI
/// dispatcher + the onboarding front-ends.
#[must_use]
pub fn resolve_zone(workgroup_root: &Path, mesh_id: &str) -> Vec<(String, IpAddr)> {
    let roster = peers::read_peers(&peers::peers_dir(workgroup_root));
    build_zone(&roster, mesh_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    fn peer(host: &str, ip: Option<&str>) -> PeerRecord {
        let mut r = PeerRecord::now(host, None, "healthy");
        r.overlay_ip = ip.map(str::to_string);
        r
    }

    #[test]
    fn build_zone_maps_each_peer_to_host_dot_mesh() {
        let roster = [
            peer("anvil", Some("10.42.0.2")),
            peer("forge", Some("10.42.0.3")),
        ];
        let zone = build_zone(&roster, "home-mesh");
        assert_eq!(zone.len(), 2, "one entry per peer with an overlay IP");
        // The suffix is the mesh-id.
        assert!(zone
            .iter()
            .any(|(n, ip)| n == "anvil.home-mesh" && ip.to_string() == "10.42.0.2"));
        assert!(zone
            .iter()
            .any(|(n, ip)| n == "forge.home-mesh" && ip.to_string() == "10.42.0.3"));
    }

    #[test]
    fn build_zone_skips_ip_less_and_unparseable_peers() {
        let roster = [
            peer("anvil", Some("10.42.0.2")),
            peer("ghost", None),              // never enrolled → no overlay IP
            peer("bogus", Some("not-an-ip")), // garbage → skipped, never a half record
            peer("blank", Some("   ")),       // whitespace-only → skipped
        ];
        let zone = build_zone(&roster, "m");
        assert_eq!(zone.len(), 1);
        assert_eq!(zone[0].0, "anvil.m");
    }

    #[test]
    fn build_zone_is_sorted_and_deduped_deterministically() {
        // Unsorted input with an exact duplicate row.
        let roster = [
            peer("forge", Some("10.42.0.3")),
            peer("anvil", Some("10.42.0.2")),
            peer("forge", Some("10.42.0.3")),
        ];
        let a = build_zone(&roster, "mesh");
        let b = build_zone(&roster, "mesh");
        assert_eq!(a, b, "deterministic");
        assert_eq!(
            a,
            vec![
                ("anvil.mesh".to_string(), "10.42.0.2".parse().unwrap()),
                ("forge.mesh".to_string(), "10.42.0.3".parse().unwrap()),
            ],
            "sorted + deduped"
        );
    }

    #[test]
    fn render_hosts_wraps_lines_in_the_managed_markers() {
        let zone = build_zone(
            &[
                peer("anvil", Some("10.42.0.2")),
                peer("forge", Some("10.42.0.3")),
            ],
            "home-mesh",
        );
        let block = render_hosts(&zone);
        assert!(block.starts_with(HOSTS_BEGIN));
        assert!(block.trim_end().ends_with(HOSTS_END));
        // /etc/hosts convention: `<ip>\t<name>`.
        assert!(block.contains("10.42.0.2\tanvil.home-mesh\n"));
        assert!(block.contains("10.42.0.3\tforge.home-mesh\n"));
    }

    #[test]
    fn render_hosts_of_empty_zone_is_empty() {
        assert_eq!(render_hosts(&[]), "");
    }

    #[test]
    fn splice_replaces_the_block_and_keeps_other_lines() {
        let base = "127.0.0.1\tlocalhost\n";
        let zone1 = build_zone(&[peer("anvil", Some("10.42.0.2"))], "mesh");
        let once = splice_hosts(base, &render_hosts(&zone1));
        assert!(once.contains("127.0.0.1\tlocalhost"));
        assert!(once.contains("10.42.0.2\tanvil.mesh"));

        // A new roster REPLACES the managed block (not appends): the old name is
        // gone, the surrounding content survives, and exactly one block remains.
        let zone2 = build_zone(&[peer("forge", Some("10.42.0.3"))], "mesh");
        let twice = splice_hosts(&once, &render_hosts(&zone2));
        assert!(twice.contains("127.0.0.1\tlocalhost"));
        assert!(twice.contains("10.42.0.3\tforge.mesh"));
        assert!(!twice.contains("anvil.mesh"), "prior block was replaced");
        assert_eq!(twice.matches(HOSTS_BEGIN).count(), 1, "exactly one block");
    }

    #[test]
    fn splice_is_idempotent() {
        let base = "127.0.0.1\tlocalhost\n";
        let block = render_hosts(&build_zone(&[peer("anvil", Some("10.42.0.2"))], "mesh"));
        let once = splice_hosts(base, &block);
        let twice = splice_hosts(&once, &block);
        assert_eq!(once, twice, "re-splicing the same block is a no-op");
    }

    #[test]
    fn splice_empty_block_removes_the_managed_block() {
        let base = "127.0.0.1\tlocalhost\n";
        let with = splice_hosts(
            base,
            &render_hosts(&build_zone(&[peer("anvil", Some("10.42.0.2"))], "mesh")),
        );
        assert!(with.contains(HOSTS_BEGIN));
        let without = splice_hosts(&with, "");
        assert!(!without.contains(HOSTS_BEGIN));
        assert!(without.contains("127.0.0.1\tlocalhost"));
    }

    /// Fake sink: an in-memory "hosts file" that counts writes.
    struct FakeHosts {
        content: RefCell<String>,
        writes: Cell<usize>,
    }
    impl FakeHosts {
        fn new(seed: &str) -> Self {
            Self {
                content: RefCell::new(seed.to_string()),
                writes: Cell::new(0),
            }
        }
    }
    impl HostsSink for FakeHosts {
        fn read(&self) -> String {
            self.content.borrow().clone()
        }
        fn write(&self, content: &str) -> Result<(), String> {
            *self.content.borrow_mut() = content.to_string();
            self.writes.set(self.writes.get() + 1);
            Ok(())
        }
    }

    #[test]
    fn apply_writes_the_block_then_is_a_noop_on_rerun() {
        let sink = FakeHosts::new("127.0.0.1\tlocalhost\n");
        let zone = build_zone(
            &[
                peer("anvil", Some("10.42.0.2")),
                peer("forge", Some("10.42.0.3")),
            ],
            "home-mesh",
        );

        // First apply: writes the block once.
        let first = apply(&zone, &sink).expect("apply");
        assert!(first.changed);
        assert_eq!(first.names, 2);
        assert_eq!(sink.writes.get(), 1);
        let landed = sink.read();
        assert!(
            landed.contains("127.0.0.1\tlocalhost"),
            "preserves prior lines"
        );
        assert!(landed.contains("10.42.0.2\tanvil.home-mesh"));
        assert!(landed.contains(HOSTS_BEGIN));

        // Second apply with the SAME roster: no change, no extra write.
        let second = apply(&zone, &sink).expect("apply");
        assert!(
            !second.changed,
            "re-running with the same roster is a no-op"
        );
        assert_eq!(sink.writes.get(), 1, "no second write");
    }
}
