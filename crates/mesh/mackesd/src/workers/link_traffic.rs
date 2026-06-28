//! MESHMAP-6 (2026-06-27) — real per-link byte counters.
//!
//! The mesh wallpaper / Peers-Map flow particles (MESHMAP-1..5) drive
//! their per-direction streams off a *proxy*: each peer's TOTAL Netdata
//! `system.net` throughput, attributed to its self→peer edge
//! (`peers.rs::sample_flows`). That answers "who's busy" but not the true
//! per-LINK volume. This worker makes it real.
//!
//! ## Counter source — nftables accounting
//!
//! OSS Nebula exposes no per-tunnel byte counters: the only introspection
//! surface this codebase uses ([`crate::nebula_admin`]'s loopback debug-SSH
//! `list-hostmap -json`) reports the chosen endpoint + relay state, never
//! byte totals, and Nebula's `stats:` block is interface-level only (and
//! isn't even rendered into our config). So we take the self-contained
//! nftables-accounting path the worklist names: maintain a dedicated
//! `inet mde_linkacct` table with one `counter` per peer overlay IP per
//! direction, hooked to the overlay interface, and read byte deltas on a
//! tick.
//!
//! Per peer overlay IP `P` on the Nebula interface:
//!
//! * **tx** (self→peer): packets we send *to* `P` — `ip daddr P` egress.
//! * **rx** (peer→self): packets we receive *from* `P` — `ip saddr P`
//!   ingress.
//!
//! The counters are *passive accounting* — every rule ends in the implicit
//! `continue` (no verdict), so the table never affects forwarding; it only
//! tallies bytes. The chains hook the `inet` family `filter` priority on
//! the named interface (`iifname` / `oifname nebula1`).
//!
//! ## What it publishes
//!
//! `~/.cache/mde/link-traffic.json` — the same `$XDG_CACHE_HOME/mde/`
//! idiom the wallpaper already reads (`mesh-latency.json`, `peer-cap.json`).
//! Keyed by **peer hostname** (the roster joins overlay-IP→name) so the GUI
//! joins per-edge exactly like the latency cache:
//!
//! ```json
//! {
//!   "checked_at": 1716499200,
//!   "peers": {
//!     "anvil": { "tx_rate": 0.42, "rx_rate": 0.11,
//!                "tx_bps": 5040000.0, "rx_bps": 1320000.0 },
//!     "forge": { "tx_rate": 0.0,  "rx_rate": 0.0,
//!                "tx_bps": 0.0,     "rx_bps": 0.0 }
//!   }
//! }
//! ```
//!
//! `tx_rate`/`rx_rate` are normalized 0.0..=1.0 against the same ~100 Mbit/s
//! reference the proxy uses ([`REF_BYTES_PER_S`]) so the consumer's
//! particle-density math is unchanged — only the *number* gets real.
//!
//! ## Honest degradation (never fake)
//!
//! * No `nft` on PATH / non-root / the overlay interface absent ⇒ the table
//!   never materializes, the read yields nothing, the cache stays absent or
//!   stale, and the consumer **falls back to the `sample_flows` proxy**.
//! * The FIRST sample for a peer has no prior reading ⇒ no delta ⇒ that peer
//!   is omitted (a rate needs two points); it appears on the next tick.
//! * A counter that went backwards (table re-created, nft restart) is
//!   treated as a fresh baseline (delta 0), never a negative rate.
//!
//! ## Cheap at idle (MESHMAP-5 budget)
//!
//! Delta-sampled on a [`DEFAULT_SAMPLE_INTERVAL`] (5 s) tick — one
//! `nft list table` read + one reconcile per tick, never a busy loop. An
//! idle link reads delta 0 ⇒ `tx_rate`/`rx_rate` 0.0 ⇒ the consumer draws
//! no particles ⇒ the animation subscription stays unarmed (`has_flow`).

#![cfg(feature = "async-services")]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use super::{ShutdownToken, Worker};
use crate::nebula_roster::export_roster;

/// Default sample cadence — 5 s between counter reads. Short enough that
/// the wallpaper's ~3 s flow tick sees fresh-ish numbers, long enough that
/// a single `nft list table` read per tick is negligible.
pub const DEFAULT_SAMPLE_INTERVAL: Duration = Duration::from_secs(5);

/// The dedicated nftables table name (family `inet`). Isolated from
/// firewalld's tables so reconcile never touches enforcement rules.
pub const TABLE_NAME: &str = "mde_linkacct";

/// The overlay interface the accounting chains hook.
pub const DEFAULT_NEBULA_INTERFACE: &str = "nebula1";

/// Normalization reference — ~100 Mbit/s in bytes/s.
///
/// Mirrors `peers.rs::sample_flows`'s `REF_BYTES_PER_S` so a link at/above
/// this saturates the particle stream and the consumer's density math is
/// identical whether it reads the real counter or the proxy.
pub const REF_BYTES_PER_S: f64 = 12_000_000.0;

/// One peer's per-link traffic for a single sample pass.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct LinkTraffic {
    /// self→peer rate, normalized 0.0..=1.0 against [`REF_BYTES_PER_S`].
    pub tx_rate: f64,
    /// peer→self rate, normalized 0.0..=1.0.
    pub rx_rate: f64,
    /// Raw self→peer bytes/s (informational; the GUI uses the rates).
    pub tx_bps: f64,
    /// Raw peer→self bytes/s.
    pub rx_bps: f64,
}

/// One sample pass — every peer measured at the same wall-clock instant.
/// Serialized to `~/.cache/mde/link-traffic.json` for the wallpaper / Map.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LinkTrafficSnapshot {
    /// Unix-epoch seconds when the snapshot was written.
    pub checked_at: i64,
    /// Map of peer hostname → per-link traffic. `BTreeMap` so the JSON is
    /// deterministic (no spurious diffs / repaint churn).
    pub peers: BTreeMap<String, LinkTraffic>,
}

/// One peer's raw byte counters at one instant (pre-delta). Keyed by
/// overlay IP internally; joined to the hostname for the snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawCounters {
    /// Cumulative self→peer bytes (the `ip daddr <peer>` counter).
    pub tx_bytes: u64,
    /// Cumulative peer→self bytes (the `ip saddr <peer>` counter).
    pub rx_bytes: u64,
}

/// Normalize a bytes/s rate to 0.0..=1.0 against [`REF_BYTES_PER_S`].
#[must_use]
pub fn normalize(bps: f64) -> f64 {
    (bps / REF_BYTES_PER_S).clamp(0.0, 1.0)
}

/// Compute the per-second rate from two cumulative counter readings.
///
/// A counter that went backwards (nft restart / table re-created) is a
/// fresh baseline → 0.0, never negative. `elapsed_s` ≤ 0 → 0.0 (no
/// meaningful interval).
#[must_use]
pub fn rate_bps(prev: u64, cur: u64, elapsed_s: f64) -> f64 {
    if elapsed_s <= 0.0 || cur < prev {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss)]
    let delta = (cur - prev) as f64;
    delta / elapsed_s
}

/// A sanitized nft counter-name token for an overlay IP.
///
/// Dots/colons become `_`, so `10.42.0.5` → `10_42_0_5`. Used in the counter
/// object names so each peer's tx/rx counter is individually nameable +
/// readable back by IP. Pure.
#[must_use]
pub fn ip_token(ip: &str) -> String {
    ip.chars()
        .map(|c| if c == '.' || c == ':' { '_' } else { c })
        .collect()
}

/// Render the full `nft -f -` ruleset for `peer_ips` on `iface`.
///
/// (Re)creates the accounting table from scratch — idempotent by
/// construction: `delete table` (made safe by a leading `add table`) then a
/// fresh `add`. The two chains are passive (`type filter hook … ; policy
/// accept;`) and every rule is a bare `counter name …` with no verdict, so
/// the table only tallies bytes and never alters forwarding. Pure —
/// unit-tested for shape.
#[must_use]
pub fn render_ruleset(iface: &str, peer_ips: &[String]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    // `add table` first makes the subsequent `delete table` safe even on a
    // clean box (delete of a just-added empty table is a no-op rebuild).
    let _ = writeln!(out, "add table inet {TABLE_NAME}");
    let _ = writeln!(out, "delete table inet {TABLE_NAME}");
    let _ = writeln!(out, "add table inet {TABLE_NAME}");
    // Named counters — one tx + one rx per peer, so a list-table read maps
    // each counter straight back to a peer IP.
    for ip in peer_ips {
        let tok = ip_token(ip);
        let _ = writeln!(out, "add counter inet {TABLE_NAME} tx_{tok}");
        let _ = writeln!(out, "add counter inet {TABLE_NAME} rx_{tok}");
    }
    // Egress chain — count bytes we SEND to each peer (self→peer = tx).
    let _ = writeln!(
        out,
        "add chain inet {TABLE_NAME} egress {{ type filter hook output priority 0; policy accept; }}"
    );
    for ip in peer_ips {
        let tok = ip_token(ip);
        let _ = writeln!(
            out,
            "add rule inet {TABLE_NAME} egress oifname \"{iface}\" ip daddr {ip} counter name tx_{tok}"
        );
    }
    // Ingress chain — count bytes we RECEIVE from each peer (peer→self = rx).
    let _ = writeln!(
        out,
        "add chain inet {TABLE_NAME} ingress {{ type filter hook input priority 0; policy accept; }}"
    );
    for ip in peer_ips {
        let tok = ip_token(ip);
        let _ = writeln!(
            out,
            "add rule inet {TABLE_NAME} ingress iifname \"{iface}\" ip saddr {ip} counter name rx_{tok}"
        );
    }
    out
}

/// Parse `nft -j list table inet mde_linkacct` JSON into per-IP raw counters.
///
/// Keyed by overlay IP. The `nftables` array carries one object
/// per element; we read the `counter` objects, recover the IP from the
/// `tx_<tok>` / `rx_<tok>` name, and pair tx+rx. Pure + defensive: an
/// unparseable body / missing fields yields an empty map (honest "no
/// data", the consumer falls back to the proxy).
#[must_use]
pub fn parse_counters(json: &str) -> BTreeMap<String, RawCounters> {
    let mut tx: BTreeMap<String, u64> = BTreeMap::new();
    let mut rx: BTreeMap<String, u64> = BTreeMap::new();
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return BTreeMap::new();
    };
    let Some(arr) = v.get("nftables").and_then(|n| n.as_array()) else {
        return BTreeMap::new();
    };
    for entry in arr {
        let Some(counter) = entry.get("counter") else {
            continue;
        };
        let Some(name) = counter.get("name").and_then(|n| n.as_str()) else {
            continue;
        };
        let bytes = counter.get("bytes").and_then(serde_json::Value::as_u64);
        let Some(bytes) = bytes else { continue };
        if let Some(tok) = name.strip_prefix("tx_") {
            tx.insert(detoken_ip(tok), bytes);
        } else if let Some(tok) = name.strip_prefix("rx_") {
            rx.insert(detoken_ip(tok), bytes);
        }
    }
    // Pair tx + rx by IP. A counter present in only one direction still
    // produces a row (the missing direction reads 0) so a one-way link
    // (e.g. a peer that only downloads) still shows.
    let mut out = BTreeMap::new();
    let ips: std::collections::BTreeSet<String> = tx.keys().chain(rx.keys()).cloned().collect();
    for ip in ips {
        out.insert(
            ip.clone(),
            RawCounters {
                tx_bytes: tx.get(&ip).copied().unwrap_or(0),
                rx_bytes: rx.get(&ip).copied().unwrap_or(0),
            },
        );
    }
    out
}

/// Recover an overlay IP from a counter-name token (`10_42_0_5` →
/// `10.42.0.5`). The inverse of [`ip_token`] for IPv4 (v6 colons also
/// round-trip back to `:` — but the accounting rules are IPv4 `ip
/// saddr/daddr`, so v4 is the live path).
fn detoken_ip(tok: &str) -> String {
    tok.replace('_', ".")
}

/// Compute one snapshot from the previous + current raw readings + the
/// IP→hostname join. Pure — the whole rate/normalize/join pipeline is
/// unit-testable without nft or the clock.
///
/// `prev` is `None` on the first pass (or after a re-create); every peer
/// is then omitted (a rate needs two points). A peer present in `cur` but
/// not in the `ip_to_host` join is skipped (we publish by hostname).
#[must_use]
pub fn build_snapshot(
    prev: Option<&BTreeMap<String, RawCounters>>,
    cur: &BTreeMap<String, RawCounters>,
    elapsed_s: f64,
    ip_to_host: &BTreeMap<String, String>,
    checked_at: i64,
) -> LinkTrafficSnapshot {
    let mut peers = BTreeMap::new();
    if let Some(prev) = prev {
        for (ip, c) in cur {
            let Some(host) = ip_to_host.get(ip) else {
                continue;
            };
            let Some(p) = prev.get(ip) else {
                continue; // new this pass — no delta yet
            };
            let tx_bps = rate_bps(p.tx_bytes, c.tx_bytes, elapsed_s);
            let rx_bps = rate_bps(p.rx_bytes, c.rx_bytes, elapsed_s);
            peers.insert(
                host.clone(),
                LinkTraffic {
                    tx_rate: normalize(tx_bps),
                    rx_rate: normalize(rx_bps),
                    tx_bps,
                    rx_bps,
                },
            );
        }
    }
    LinkTrafficSnapshot { checked_at, peers }
}

/// Resolve the default cache path —
/// `$XDG_CACHE_HOME/mde/link-traffic.json`, falling back to
/// `$HOME/.cache/mde/link-traffic.json`. Mirrors
/// [`crate::workers::mesh_latency::default_cache_path`].
#[must_use]
pub fn default_cache_path() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME").map_or_else(
        || {
            let home = std::env::var_os("HOME").unwrap_or_default();
            PathBuf::from(home).join(".cache")
        },
        PathBuf::from,
    );
    base.join("mde").join("link-traffic.json")
}

// ── nft shell-outs (bounded; degrade to None) ──────────────────────────

/// `true` when an `nft` binary answers `--version`.
fn nft_present() -> bool {
    let mut cmd = Command::new("nft");
    cmd.arg("--version");
    crate::workers::proc::output_with_timeout(cmd, crate::workers::proc::DEFAULT_CMD_TIMEOUT)
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Apply the rendered ruleset via `nft -f -` (stdin). Returns `false` on
/// any failure (non-root, parse error, no nft) — the caller then reads
/// nothing and the consumer keeps the proxy.
fn apply_ruleset(ruleset: &str) -> bool {
    use std::io::Write;
    use std::process::Stdio;
    let Ok(mut child) = Command::new("nft")
        .arg("-f")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(ruleset.as_bytes()).is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return false;
        }
    }
    child.wait().map(|s| s.success()).unwrap_or(false)
}

/// Read the accounting table as JSON. `None` on any failure (table absent
/// yet, non-root, no nft).
fn read_table_json() -> Option<String> {
    let mut cmd = Command::new("nft");
    cmd.args(["-j", "list", "table", "inet", TABLE_NAME]);
    let out =
        crate::workers::proc::output_with_timeout(cmd, crate::workers::proc::DEFAULT_CMD_TIMEOUT)
            .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

fn now_epoch_s() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

fn write_snapshot(cache_path: &PathBuf, snapshot: &LinkTrafficSnapshot) -> anyhow::Result<()> {
    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| anyhow::anyhow!("mkdir cache: {e}"))?;
    }
    let raw = serde_json::to_string_pretty(snapshot)
        .map_err(|e| anyhow::anyhow!("serialize snapshot: {e}"))?;
    std::fs::write(cache_path, raw).map_err(|e| anyhow::anyhow!("write cache: {e}"))?;
    Ok(())
}

/// MESHMAP-6 worker handle.
pub struct LinkTrafficWorker {
    store: Arc<Mutex<rusqlite::Connection>>,
    local_node_id: String,
    cache_path: PathBuf,
    iface: String,
    interval: Duration,
}

impl LinkTrafficWorker {
    /// Construct with production defaults (`nebula1`, 5 s cadence).
    /// `cache_path` is normally `~/.cache/mde/link-traffic.json`.
    #[must_use]
    pub fn new(
        store: Arc<Mutex<rusqlite::Connection>>,
        local_node_id: String,
        cache_path: PathBuf,
    ) -> Self {
        Self {
            store,
            local_node_id,
            cache_path,
            iface: DEFAULT_NEBULA_INTERFACE.to_string(),
            interval: DEFAULT_SAMPLE_INTERVAL,
        }
    }

    /// Override the sample cadence (tests / fast-debug).
    #[must_use]
    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Override the overlay interface (tests).
    #[must_use]
    pub fn with_iface(mut self, iface: String) -> Self {
        self.iface = iface;
        self
    }

    /// The IP→hostname join + the peer-IP list, from the roster. Excludes
    /// the local node + rows without an overlay IP. Empty on a store error
    /// (the worker then reconciles an empty table — no peers, no counters —
    /// and the consumer keeps the proxy).
    async fn roster_join(&self) -> (Vec<String>, BTreeMap<String, String>) {
        let rows = {
            let conn = self.store.lock().await;
            export_roster(&conn).unwrap_or_default()
        };
        let mut ips = Vec::new();
        let mut ip_to_host = BTreeMap::new();
        for r in rows {
            if r.node_id == self.local_node_id || r.overlay_ip.is_empty() || r.name.is_empty() {
                continue;
            }
            ips.push(r.overlay_ip.clone());
            ip_to_host.insert(r.overlay_ip, r.name);
        }
        ips.sort();
        ips.dedup();
        (ips, ip_to_host)
    }
}

#[async_trait::async_trait]
impl Worker for LinkTrafficWorker {
    fn name(&self) -> &'static str {
        "link-traffic"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        // Honest degradation up front: no nft ⇒ the worker can't account,
        // so it idles on the shutdown token (never busy-loops) and the
        // consumer keeps the `sample_flows` proxy. Spawned (reachable §7),
        // simply a no-op data source on this box.
        if !nft_present() {
            tracing::info!(
                "link-traffic: nft not available — per-link accounting disabled, \
                 wallpaper keeps the per-node proxy (honest degradation)"
            );
            shutdown.wait().await;
            return Ok(());
        }

        // Previous raw reading + when it was taken — the delta basis.
        let mut prev: Option<BTreeMap<String, RawCounters>> = None;
        let mut prev_at: Option<Instant> = None;
        // The peer-IP set the table was last reconciled for; re-render only
        // when it changes (a new/departed peer), so a steady mesh does one
        // cheap `nft list` per tick and no rewrite.
        let mut reconciled_ips: Vec<String> = Vec::new();

        loop {
            // Reconcile the table against the live roster.
            let (ips, ip_to_host) = self.roster_join().await;
            if ips != reconciled_ips {
                let ruleset = render_ruleset(&self.iface, &ips);
                if apply_ruleset(&ruleset) {
                    reconciled_ips = ips.clone();
                    // The table was re-created — counters reset, so the
                    // prior reading is no longer a valid delta basis.
                    prev = None;
                    prev_at = None;
                } else {
                    tracing::debug!(
                        "link-traffic: nft apply failed (non-root?) — keeping proxy this tick"
                    );
                }
            }

            // Read + delta + publish.
            if let Some(json) = read_table_json() {
                let cur = parse_counters(&json);
                let now = Instant::now();
                let elapsed_s = prev_at.map_or(0.0, |t| now.duration_since(t).as_secs_f64());
                let snapshot =
                    build_snapshot(prev.as_ref(), &cur, elapsed_s, &ip_to_host, now_epoch_s());
                // Only write once we have a real delta (prev present) so we
                // never publish an all-zero first pass that would briefly
                // mask a busy link with the proxy. The very first tick just
                // captures the baseline.
                if prev.is_some() {
                    if let Err(e) = write_snapshot(&self.cache_path, &snapshot) {
                        tracing::debug!(error = %e, "link-traffic: snapshot write failed");
                    } else {
                        tracing::debug!(
                            peer_count = snapshot.peers.len(),
                            cache = %self.cache_path.display(),
                            "link-traffic: snapshot written"
                        );
                    }
                }
                prev = Some(cur);
                prev_at = Some(now);
            }

            tokio::select! {
                _ = shutdown.wait() => break,
                _ = tokio::time::sleep(self.interval) => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ip_token_round_trips_v4() {
        assert_eq!(ip_token("10.42.0.5"), "10_42_0_5");
        assert_eq!(detoken_ip("10_42_0_5"), "10.42.0.5");
    }

    #[test]
    fn normalize_clamps_to_unit() {
        assert!((normalize(0.0) - 0.0).abs() < 1e-9);
        assert!((normalize(REF_BYTES_PER_S) - 1.0).abs() < 1e-9);
        assert!((normalize(REF_BYTES_PER_S * 2.0) - 1.0).abs() < 1e-9); // clamps
        assert!((normalize(REF_BYTES_PER_S / 2.0) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn rate_bps_handles_counter_reset_and_zero_interval() {
        // Normal forward delta.
        assert!((rate_bps(1000, 6000, 5.0) - 1000.0).abs() < 1e-9); // 5000B / 5s
                                                                    // Counter went backwards (nft restart) → fresh baseline, never neg.
        assert!(rate_bps(6000, 1000, 5.0).abs() < 1e-9);
        // No interval → 0 (avoid div-by-zero / inf).
        assert!(rate_bps(0, 1000, 0.0).abs() < 1e-9);
    }

    #[test]
    fn render_ruleset_is_passive_per_peer_accounting() {
        let rs = render_ruleset("nebula1", &["10.42.0.5".into(), "10.42.0.6".into()]);
        // Recreates the table from scratch.
        assert!(rs.contains(&format!("add table inet {TABLE_NAME}")));
        assert!(rs.contains(&format!("delete table inet {TABLE_NAME}")));
        // One tx + one rx counter per peer.
        assert!(rs.contains("add counter inet mde_linkacct tx_10_42_0_5"));
        assert!(rs.contains("add counter inet mde_linkacct rx_10_42_0_5"));
        assert!(rs.contains("add counter inet mde_linkacct tx_10_42_0_6"));
        // tx = egress oifname + daddr; rx = ingress iifname + saddr.
        assert!(
            rs.contains("egress oifname \"nebula1\" ip daddr 10.42.0.5 counter name tx_10_42_0_5")
        );
        assert!(
            rs.contains("ingress iifname \"nebula1\" ip saddr 10.42.0.5 counter name rx_10_42_0_5")
        );
        // Passive: chains accept, rules carry no drop/reject verdict.
        assert!(rs.contains("policy accept;"));
        assert!(!rs.contains("drop"));
        assert!(!rs.contains("reject"));
    }

    #[test]
    fn render_ruleset_empty_peer_set_still_builds_table() {
        let rs = render_ruleset("nebula1", &[]);
        assert!(rs.contains("add table inet mde_linkacct"));
        // Chains exist but no per-peer rules.
        assert!(rs.contains("egress"));
        assert!(rs.contains("ingress"));
        assert!(!rs.contains("counter name"));
    }

    #[test]
    fn parse_counters_pairs_tx_rx_by_ip() {
        // Representative `nft -j list table` shape.
        let json = r#"{"nftables":[
          {"metainfo":{"version":"1.0.9"}},
          {"table":{"family":"inet","name":"mde_linkacct"}},
          {"counter":{"family":"inet","table":"mde_linkacct","name":"tx_10_42_0_5","packets":10,"bytes":5040000}},
          {"counter":{"family":"inet","table":"mde_linkacct","name":"rx_10_42_0_5","packets":4,"bytes":1320000}},
          {"counter":{"family":"inet","table":"mde_linkacct","name":"tx_10_42_0_6","packets":0,"bytes":0}},
          {"chain":{"family":"inet","name":"egress"}}
        ]}"#;
        let m = parse_counters(json);
        assert_eq!(m.len(), 2);
        assert_eq!(m["10.42.0.5"].tx_bytes, 5_040_000);
        assert_eq!(m["10.42.0.5"].rx_bytes, 1_320_000);
        // tx-only peer still shows; missing rx reads 0.
        assert_eq!(m["10.42.0.6"].tx_bytes, 0);
        assert_eq!(m["10.42.0.6"].rx_bytes, 0);
    }

    #[test]
    fn parse_counters_rejects_garbage() {
        assert!(parse_counters("not json").is_empty());
        assert!(parse_counters("{}").is_empty());
        assert!(parse_counters(r#"{"nftables":[]}"#).is_empty());
    }

    #[test]
    fn build_snapshot_omits_first_pass_and_joins_by_host() {
        let mut cur = BTreeMap::new();
        cur.insert(
            "10.42.0.5".to_string(),
            RawCounters {
                tx_bytes: 6000,
                rx_bytes: 2000,
            },
        );
        let mut ip_to_host = BTreeMap::new();
        ip_to_host.insert("10.42.0.5".to_string(), "anvil".to_string());

        // First pass (prev None) → empty (a rate needs two points).
        let first = build_snapshot(None, &cur, 5.0, &ip_to_host, 100);
        assert!(first.peers.is_empty());
        assert_eq!(first.checked_at, 100);

        // Second pass with a prior reading → real per-host rates.
        let mut prev = BTreeMap::new();
        prev.insert(
            "10.42.0.5".to_string(),
            RawCounters {
                tx_bytes: 1000,
                rx_bytes: 0,
            },
        );
        let snap = build_snapshot(Some(&prev), &cur, 5.0, &ip_to_host, 200);
        assert_eq!(snap.peers.len(), 1);
        let t = snap.peers["anvil"];
        assert!((t.tx_bps - 1000.0).abs() < 1e-9); // 5000B / 5s
        assert!((t.rx_bps - 400.0).abs() < 1e-9); // 2000B / 5s
        assert!((t.tx_rate - normalize(1000.0)).abs() < 1e-9);
    }

    #[test]
    fn build_snapshot_skips_unjoined_ip_and_new_peer() {
        // A peer with counters but no roster name is skipped (we publish by
        // hostname); a peer new this pass (not in prev) has no delta yet.
        let mut prev = BTreeMap::new();
        prev.insert(
            "10.42.0.5".to_string(),
            RawCounters {
                tx_bytes: 0,
                rx_bytes: 0,
            },
        );
        let mut cur = BTreeMap::new();
        cur.insert(
            "10.42.0.5".to_string(),
            RawCounters {
                tx_bytes: 100,
                rx_bytes: 0,
            },
        );
        cur.insert(
            "10.42.0.9".to_string(), // new peer this pass
            RawCounters {
                tx_bytes: 999,
                rx_bytes: 0,
            },
        );
        let mut ip_to_host = BTreeMap::new();
        ip_to_host.insert("10.42.0.5".to_string(), "anvil".to_string());
        // 10.42.0.9 deliberately absent from the join.
        let snap = build_snapshot(Some(&prev), &cur, 1.0, &ip_to_host, 0);
        assert_eq!(snap.peers.len(), 1);
        assert!(snap.peers.contains_key("anvil"));
    }

    #[test]
    fn default_cache_path_uses_xdg_when_set() {
        std::env::set_var("XDG_CACHE_HOME", "/tmp/xdg-linktraffic-test");
        let p = default_cache_path();
        assert_eq!(
            p,
            PathBuf::from("/tmp/xdg-linktraffic-test/mde/link-traffic.json")
        );
        std::env::remove_var("XDG_CACHE_HOME");
    }

    #[test]
    fn snapshot_round_trips_through_json() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("link-traffic.json");
        let mut peers = BTreeMap::new();
        peers.insert(
            "anvil".to_string(),
            LinkTraffic {
                tx_rate: 0.42,
                rx_rate: 0.11,
                tx_bps: 5_040_000.0,
                rx_bps: 1_320_000.0,
            },
        );
        let snap = LinkTrafficSnapshot {
            checked_at: 1_716_499_200,
            peers,
        };
        write_snapshot(&path, &snap).expect("write");
        let raw = std::fs::read_to_string(&path).expect("read back");
        let parsed: LinkTrafficSnapshot = serde_json::from_str(&raw).expect("parse");
        assert_eq!(parsed.checked_at, 1_716_499_200);
        assert!((parsed.peers["anvil"].tx_rate - 0.42).abs() < 1e-9);
    }

    #[tokio::test]
    async fn worker_name_is_locked() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let conn = crate::store::open(&tmp.path().join("nodes.sqlite")).expect("open");
        let w = LinkTrafficWorker::new(
            Arc::new(Mutex::new(conn)),
            "peer:test".into(),
            tmp.path().join("link-traffic.json"),
        );
        assert_eq!(w.name(), "link-traffic");
    }

    #[tokio::test]
    async fn worker_exits_on_shutdown_token() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let conn = crate::store::open(&tmp.path().join("nodes.sqlite")).expect("open");
        let mut w = LinkTrafficWorker::new(
            Arc::new(Mutex::new(conn)),
            "peer:test".into(),
            tmp.path().join("link-traffic.json"),
        )
        .with_interval(Duration::from_millis(50));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let _ = tx.send(true);
        let result = tokio::time::timeout(Duration::from_secs(3), w.run(token))
            .await
            .expect("worker must exit on shutdown");
        assert!(result.is_ok());
    }
}
