//! FWMON-2..4 (v5.0.0) — firewall-denied event monitor.
//!
//! Reads kernel journal entries written by firewalld's `LogDenied=all`
//! setting (enabled by birthright's `apply_firewall_log_denied` step),
//! filters out overlay + established traffic, appends net-new external
//! denials to `<mesh-storage>/firewall/<host>.jsonl`, trims entries
//! older than 7 days, and fires a Bus alert when one source IP crosses
//! the denial threshold within a rolling window.
//!
//! ## Design locks (FWMON 7-Q survey, 2026-05-29)
//!
//! - Separate worker from `firewall_preset` (which owns port-open
//!   convergence); monitor is read-only with respect to firewalld.
//! - Silent no-op when `journalctl` is absent (CI / containerised
//!   peers without systemd).
//! - Cursor persisted at `/var/lib/mackesd/firewall-journal.cursor`
//!   (epoch-ms of last read); on first run starts from 5 s ago so
//!   we don't replay the full journal.
//! - Q14 noise filter: UDP/4242 (Nebula), TCP/443 (lighthouse covert),
//!   and conntrack RELATED/ESTABLISHED entries are silently dropped.
//! - JSONL own-file authority: each peer writes its own file; all
//!   peers read the union (`firewall/*.jsonl`) for the Activity view.
//! - FWMON-4 threshold: default 10 denials from one source in a
//!   60-minute window → one Bus `event/firewall/<host>` per source
//!   per window (deduped; not per-packet).

#![cfg(feature = "async-services")]

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{ShutdownToken, Worker};

/// Default sweep cadence — 5 s, matching `gluster_worker` +
/// `meshfs_worker`.
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(5);

/// Default JSONL directory under the mesh-storage mount.
pub const DEFAULT_MESH_STORAGE_MOUNT: &str = "/mnt/mesh-storage";

/// Firewall subdirectory inside mesh-storage.
pub const FIREWALL_SUBDIR: &str = "firewall";

/// Cursor file path (last-read epoch-ms).
pub const DEFAULT_CURSOR_PATH: &str = "/var/lib/mackesd/firewall-journal.cursor";

/// 7-day retention window in milliseconds.
pub const RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1_000;

/// Default denial threshold: ≥ this many denials from one source in
/// `ALERT_WINDOW_MS` fires one Bus event (FWMON-4).
pub const DEFAULT_THRESHOLD: usize = 10;

/// Alert dedup window — one Bus event per source per 60 minutes.
pub const ALERT_WINDOW_MS: u64 = 60 * 60 * 1_000;

/// One denied-packet record.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DeniedEvent {
    /// Wall-clock epoch-milliseconds when the event was read.
    pub ts_ms: i64,
    /// Hostname of this peer (`/etc/hostname`).
    pub host: String,
    /// Source IP of the denied packet.
    pub src_ip: String,
    /// Destination port.
    pub dport: u16,
    /// Protocol string (TCP, UDP, ICMP, …).
    pub proto: String,
    /// Inbound interface name.
    pub iface: String,
    /// Conntrack state token if present (e.g. `RELATED,ESTABLISHED`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub state: String,
}

/// Parse a single kernel journal line into a [`DeniedEvent`].
///
/// Returns `None` when the line doesn't look like a firewall-denied
/// packet (missing `SRC=` or `IN=` tokens), or when `src_ip` is
/// empty after tokenizing.
///
/// Tokenizes by whitespace and strips `KEY=value` pairs
/// case-sensitively. Field order is irrelevant — all fields are
/// independent tokens.
pub fn parse_denied_line(line: &str, host: &str, now_ms: i64) -> Option<DeniedEvent> {
    if !line.contains("SRC=") || !line.contains("IN=") {
        return None;
    }

    let mut src_ip = String::new();
    let mut dport: u16 = 0;
    let mut proto = String::new();
    let mut iface = String::new();
    let mut state = String::new();

    for token in line.split_whitespace() {
        if let Some(v) = token.strip_prefix("SRC=") {
            src_ip = v.to_string();
        } else if let Some(v) = token.strip_prefix("DPT=") {
            dport = v.parse().unwrap_or(0);
        } else if let Some(v) = token.strip_prefix("PROTO=") {
            proto = v.to_string();
        } else if let Some(v) = token.strip_prefix("IN=") {
            if !v.is_empty() {
                iface = v.to_string();
            }
        } else if let Some(v) = token.strip_prefix("STATE=") {
            state = v.to_string();
        } else if let Some(v) = token.strip_prefix("CTSTATE=") {
            state = v.to_string();
        }
    }

    if src_ip.is_empty() || proto.is_empty() {
        return None;
    }

    Some(DeniedEvent {
        ts_ms: now_ms,
        host: host.to_string(),
        src_ip,
        dport,
        proto,
        iface,
        state,
    })
}

/// Q14 noise filter — returns `true` for packets that should be
/// silently dropped:
///
/// - UDP/4242: Nebula overlay tunnel (expected inbound on every peer).
/// - TCP/443: lighthouse covert-listener traffic.
/// - RELATED or ESTABLISHED state: conntrack-tracked connections that
///   firewalld may log with `LogDenied=all` in some configurations.
pub fn is_overlay_or_established(event: &DeniedEvent) -> bool {
    let proto_up = event.proto.to_uppercase();

    if proto_up == "UDP" && event.dport == 4242 {
        return true;
    }
    if proto_up == "TCP" && event.dport == 443 {
        return true;
    }

    let s = event.state.to_uppercase();
    s.contains("RELATED") || s.contains("ESTABLISHED")
}

/// Trim JSONL entries whose `ts_ms` field is older than `cutoff_ms`
/// from `path`. Rewrites the file in-place.  No-ops when the file
/// doesn't exist.
pub fn trim_older_than(path: &Path, cutoff_ms: i64) -> std::io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let content = std::fs::read_to_string(path)?;
    let kept: Vec<&str> = content
        .lines()
        .filter(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .ok()
                .and_then(|v| v["ts_ms"].as_i64())
                .map(|ts| ts >= cutoff_ms)
                .unwrap_or(false)
        })
        .collect();
    if kept.is_empty() {
        std::fs::write(path, b"")?;
    } else {
        std::fs::write(path, format!("{}\n", kept.join("\n")))?;
    }
    Ok(())
}

/// Returns `true` when `src_ip` has accumulated ≥ `threshold`
/// denials in the window `[now_ms - window_ms, now_ms]`.
///
/// Pure function — `now_ms` is passed by the caller so tests are
/// deterministic.
pub fn threshold_tripped(
    events: &[DeniedEvent],
    src_ip: &str,
    now_ms: i64,
    window_ms: u64,
    threshold: usize,
) -> bool {
    if threshold == 0 {
        return false;
    }
    let count = events
        .iter()
        .filter(|e| {
            e.src_ip == src_ip && e.ts_ms <= now_ms && (now_ms - e.ts_ms) as u64 <= window_ms
        })
        .count();
    count >= threshold
}

/// Worker handle.
pub struct FirewallMonitorWorker {
    host: String,
    mesh_storage: PathBuf,
    tick: Duration,
    cursor_path: PathBuf,
    threshold: usize,
    /// `src_ip → last_alerted_at_ms`.  Prevents re-firing the Bus
    /// event on every tick once a source has crossed the threshold.
    alerted: Mutex<BTreeMap<String, i64>>,
}

impl FirewallMonitorWorker {
    /// Construct with production defaults.
    #[must_use]
    pub fn new(host: String) -> Self {
        Self {
            host,
            mesh_storage: PathBuf::from(DEFAULT_MESH_STORAGE_MOUNT),
            tick: DEFAULT_TICK_INTERVAL,
            cursor_path: PathBuf::from(DEFAULT_CURSOR_PATH),
            threshold: DEFAULT_THRESHOLD,
            alerted: Mutex::new(BTreeMap::new()),
        }
    }

    /// Override the mesh-storage mount root. Used in tests.
    #[must_use]
    pub fn with_mesh_storage(mut self, p: PathBuf) -> Self {
        self.mesh_storage = p;
        self
    }

    /// Override the cursor path. Used in tests.
    #[must_use]
    pub fn with_cursor_path(mut self, p: PathBuf) -> Self {
        self.cursor_path = p;
        self
    }

    /// Override the threshold. Used in tests.
    #[must_use]
    pub fn with_threshold(mut self, n: usize) -> Self {
        self.threshold = n;
        self
    }

    fn jsonl_path(&self) -> PathBuf {
        self.mesh_storage
            .join(FIREWALL_SUBDIR)
            .join(format!("{}.jsonl", self.host))
    }

    /// Read new kernel journal lines since the last cursor.
    fn read_new_lines(&self) -> Vec<String> {
        if std::process::Command::new("journalctl")
            .arg("--version")
            .output()
            .is_err()
        {
            return vec![];
        }

        let now_ms = now_epoch_ms();
        let since_ms = std::fs::read_to_string(&self.cursor_path)
            .ok()
            .and_then(|s| s.trim().parse::<i64>().ok())
            .unwrap_or_else(|| now_ms - 5_000);

        let since_sec = since_ms / 1_000;
        let since_arg = format!("@{since_sec}");

        let output = Command::new("journalctl")
            .args(["-k", "--no-pager", "-o", "cat"])
            .arg(format!("--since={since_arg}"))
            .output();

        let _ = std::fs::write(&self.cursor_path, now_ms.to_string());

        output
            .ok()
            .filter(|o| o.status.success())
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn tick_once(&self) {
        let now_ms = now_epoch_ms();
        let lines = self.read_new_lines();

        let events: Vec<DeniedEvent> = lines
            .iter()
            .filter_map(|l| parse_denied_line(l, &self.host, now_ms))
            .filter(|e| !is_overlay_or_established(e))
            .collect();

        if !events.is_empty() {
            let jsonl = self.jsonl_path();
            if let Some(parent) = jsonl.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&jsonl)
            {
                for e in &events {
                    if let Ok(line) = serde_json::to_string(e) {
                        let _ = writeln!(f, "{line}");
                    }
                }
            }
        }

        // 7-day trim on every tick (cheap: skips when file absent).
        let cutoff = now_ms - RETENTION_MS;
        let _ = trim_older_than(&self.jsonl_path(), cutoff);

        // FWMON-4: threshold alert per source.
        let unique_srcs: std::collections::BTreeSet<&str> =
            events.iter().map(|e| e.src_ip.as_str()).collect();
        for src in unique_srcs {
            if !threshold_tripped(&events, src, now_ms, ALERT_WINDOW_MS, self.threshold) {
                continue;
            }
            let already_alerted = self
                .alerted
                .lock()
                .expect("alerted mutex")
                .get(src)
                .copied()
                .map(|last| (now_ms - last) as u64 <= ALERT_WINDOW_MS)
                .unwrap_or(false);
            if already_alerted {
                continue;
            }
            let count = events.iter().filter(|e| e.src_ip == src).count();
            publish_firewall_alert(&self.host, src, count);
            self.alerted
                .lock()
                .expect("alerted mutex")
                .insert(src.to_string(), now_ms);
        }
    }
}

fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn publish_firewall_alert(host: &str, src_ip: &str, count: usize) {
    let topic = format!("event/firewall/{host}");
    let body =
        format!(r#"{{"host":"{host}","src_ip":"{src_ip}","denial_count":{count},"alert":true}}"#);
    let _ = Command::new("mde-bus")
        .args(["publish", &topic, "--body-flag", &body])
        .spawn();
}

#[async_trait::async_trait]
impl Worker for FirewallMonitorWorker {
    fn name(&self) -> &'static str {
        "firewall_monitor"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(self.tick) => {
                    self.tick_once();
                }
                _ = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_denied_line ---

    #[test]
    fn parse_basic_iptables_line() {
        let line = "kernel: [123.456] IN=eth0 OUT= MAC=aa:bb SRC=1.2.3.4 DST=10.0.0.1 LEN=60 TTL=64 ID=1 PROTO=TCP SPT=12345 DPT=22 WINDOW=8192";
        let e = parse_denied_line(line, "mypeer", 9999).expect("should parse");
        assert_eq!(e.src_ip, "1.2.3.4");
        assert_eq!(e.dport, 22);
        assert_eq!(e.proto, "TCP");
        assert_eq!(e.iface, "eth0");
        assert_eq!(e.host, "mypeer");
        assert_eq!(e.ts_ms, 9999);
    }

    #[test]
    fn parse_udp_line() {
        let line =
            "kernel: DENIED IN=nebula1 SRC=5.6.7.8 DST=10.42.0.1 PROTO=UDP SPT=9999 DPT=4242";
        let e = parse_denied_line(line, "peer", 0).expect("parse udp");
        assert_eq!(e.proto, "UDP");
        assert_eq!(e.dport, 4242);
        assert_eq!(e.src_ip, "5.6.7.8");
    }

    #[test]
    fn parse_missing_src_returns_none() {
        let line = "kernel: IN=eth0 OUT= DST=10.0.0.1 PROTO=TCP DPT=22";
        assert!(parse_denied_line(line, "peer", 0).is_none());
    }

    #[test]
    fn parse_missing_in_returns_none() {
        let line = "kernel: SRC=1.2.3.4 DST=10.0.0.1 PROTO=TCP DPT=22";
        assert!(parse_denied_line(line, "peer", 0).is_none());
    }

    #[test]
    fn parse_field_reorder_tolerant() {
        // Fields in different order — parser must still extract them.
        let line = "PROTO=UDP DPT=53 SRC=9.9.9.9 IN=eth0 DST=1.1.1.1";
        let e = parse_denied_line(line, "h", 1).expect("reorder");
        assert_eq!(e.src_ip, "9.9.9.9");
        assert_eq!(e.dport, 53);
        assert_eq!(e.proto, "UDP");
    }

    #[test]
    fn parse_with_ctstate() {
        let line = "IN=eth0 SRC=1.1.1.1 DST=10.0.0.1 PROTO=TCP DPT=80 CTSTATE=RELATED,ESTABLISHED";
        let e = parse_denied_line(line, "h", 0).expect("ctstate");
        assert_eq!(e.state, "RELATED,ESTABLISHED");
    }

    // --- is_overlay_or_established ---

    fn make_event(proto: &str, dport: u16, state: &str) -> DeniedEvent {
        DeniedEvent {
            ts_ms: 0,
            host: "h".into(),
            src_ip: "1.2.3.4".into(),
            dport,
            proto: proto.into(),
            iface: "eth0".into(),
            state: state.into(),
        }
    }

    #[test]
    fn filter_drops_nebula_udp() {
        assert!(is_overlay_or_established(&make_event("UDP", 4242, "")));
    }

    #[test]
    fn filter_drops_lighthouse_tcp() {
        assert!(is_overlay_or_established(&make_event("TCP", 443, "")));
    }

    #[test]
    fn filter_drops_established() {
        assert!(is_overlay_or_established(&make_event(
            "TCP",
            22,
            "ESTABLISHED"
        )));
    }

    #[test]
    fn filter_drops_related() {
        assert!(is_overlay_or_established(&make_event(
            "TCP",
            22,
            "RELATED,ESTABLISHED"
        )));
    }

    #[test]
    fn filter_keeps_external_ssh() {
        assert!(!is_overlay_or_established(&make_event("TCP", 22, "")));
    }

    #[test]
    fn filter_keeps_external_udp_other_port() {
        assert!(!is_overlay_or_established(&make_event("UDP", 53, "")));
    }

    // --- trim_older_than ---

    #[test]
    fn trim_removes_old_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("fw.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, r#"{{"ts_ms":100,"host":"h","src_ip":"1.1.1.1","dport":22,"proto":"TCP","iface":"eth0"}}"#).unwrap();
        writeln!(f, r#"{{"ts_ms":9000,"host":"h","src_ip":"2.2.2.2","dport":22,"proto":"TCP","iface":"eth0"}}"#).unwrap();

        trim_older_than(&path, 1000).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(!content.contains("1.1.1.1"), "old entry should be gone");
        assert!(content.contains("2.2.2.2"), "new entry should stay");
    }

    #[test]
    fn trim_noop_when_file_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nonexistent.jsonl");
        trim_older_than(&path, 0).unwrap();
    }

    // --- threshold_tripped ---

    fn events_for(src: &str, ts_ms_list: &[i64]) -> Vec<DeniedEvent> {
        ts_ms_list
            .iter()
            .map(|&ts| DeniedEvent {
                ts_ms: ts,
                host: "h".into(),
                src_ip: src.into(),
                dport: 22,
                proto: "TCP".into(),
                iface: "eth0".into(),
                state: String::new(),
            })
            .collect()
    }

    #[test]
    fn threshold_tripped_at_exactly_threshold() {
        let evs = events_for("1.1.1.1", &[1000, 2000, 3000]);
        assert!(threshold_tripped(&evs, "1.1.1.1", 5000, 10_000, 3));
    }

    #[test]
    fn threshold_not_tripped_below() {
        let evs = events_for("1.1.1.1", &[1000, 2000]);
        assert!(!threshold_tripped(&evs, "1.1.1.1", 5000, 10_000, 3));
    }

    #[test]
    fn threshold_ignores_expired_events() {
        // now=20_000, window=5_000 → only events in [15_000, 20_000] count
        let evs = events_for("1.1.1.1", &[1000, 2000, 3000, 17_000, 18_000]);
        // events in window: 17_000, 18_000 → count=2 < threshold=3
        assert!(!threshold_tripped(&evs, "1.1.1.1", 20_000, 5_000, 3));
    }

    #[test]
    fn threshold_zero_never_trips() {
        let evs = events_for("1.1.1.1", &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        assert!(!threshold_tripped(&evs, "1.1.1.1", 100, 1_000, 0));
    }

    #[test]
    fn threshold_only_counts_matching_src() {
        let mut evs = events_for("1.1.1.1", &[1000, 2000, 3000]);
        evs.extend(events_for("9.9.9.9", &[500, 1500, 2500]));
        // 1.1.1.1 has 3, 9.9.9.9 has 3 — check they're counted independently
        assert!(threshold_tripped(&evs, "1.1.1.1", 5000, 10_000, 3));
        assert!(!threshold_tripped(&evs, "1.1.1.1", 5000, 10_000, 4));
    }
}
