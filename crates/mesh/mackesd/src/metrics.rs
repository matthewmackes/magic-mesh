//! Prometheus textfile collector writer (Phase 12.1.5).
//!
//! Per the 12.1.5 lock: "written to a local Prometheus textfile
//! collector path (`/var/lib/node_exporter/textfile_collector/
//! mackesd.prom`). No HTTP endpoint."
//!
//! The textfile collector picks up `.prom` files written to its
//! configured directory and exposes them on the node_exporter
//! scrape. Operators wire scrape themselves; mackesd just writes
//! the file.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Write;
use std::path::{Path, PathBuf};

/// One Prometheus counter — monotonically increasing, never resets
/// (except on process restart).
#[derive(Debug, Clone, Default)]
pub struct Counter {
    /// Stable Prometheus name like `mackesd_apply_total`.
    pub name: &'static str,
    /// One-line human description for the `# HELP` row.
    pub help: &'static str,
    /// Cumulative count.
    pub value: u64,
    /// Optional labels (`{key="val"}`).
    pub labels: BTreeMap<String, String>,
}

/// One Prometheus histogram bucket — `le` (less-than-or-equal)
/// upper bound + cumulative count.
#[derive(Debug, Clone)]
pub struct Bucket {
    /// Upper bound this bucket counts up to (inclusive).
    pub le: f64,
    /// Cumulative count.
    pub count: u64,
}

/// One Prometheus histogram. Buckets, sum, and overall count.
#[derive(Debug, Clone)]
pub struct Histogram {
    /// Stable Prometheus name.
    pub name: &'static str,
    /// Help text.
    pub help: &'static str,
    /// Buckets in ascending `le` order.
    pub buckets: Vec<Bucket>,
    /// Sum of all observed values (Prometheus `_sum`).
    pub sum: f64,
    /// Total observation count (Prometheus `_count`).
    pub count: u64,
}

impl Histogram {
    /// Construct an empty histogram with the given buckets.
    /// Buckets must be in ascending `le` order — callers that
    /// pass unsorted buckets get incorrect percentile estimates.
    #[must_use]
    pub fn new(name: &'static str, help: &'static str, bucket_les: &[f64]) -> Self {
        let buckets = bucket_les
            .iter()
            .map(|le| Bucket { le: *le, count: 0 })
            .collect();
        Self {
            name,
            help,
            buckets,
            sum: 0.0,
            count: 0,
        }
    }

    /// Record one observation. Increments the count of every
    /// bucket whose `le` ≥ the observed value, plus the implicit
    /// `+Inf` bucket via `self.count`.
    pub fn observe(&mut self, value: f64) {
        for b in &mut self.buckets {
            if value <= b.le {
                b.count += 1;
            }
        }
        self.sum += value;
        self.count += 1;
    }

    /// Estimate a percentile from the bucket counts. `p` is in
    /// `[0.0, 1.0]`. Returns `None` for an empty histogram.
    ///
    /// Uses Prometheus's standard linear-interpolation rule:
    /// find the bucket containing the target rank, then
    /// linear-interpolate inside the bucket from the previous
    /// bucket's upper bound.
    #[must_use]
    pub fn percentile_estimate(&self, p: f64) -> Option<f64> {
        if self.count == 0 {
            return None;
        }
        let target_rank = (self.count as f64) * p;
        let mut prev_le = 0.0_f64;
        let mut prev_count: u64 = 0;
        for b in &self.buckets {
            if (b.count as f64) >= target_rank {
                if b.count == prev_count {
                    return Some(b.le);
                }
                // Linear interpolation inside the bucket.
                let frac = (target_rank - prev_count as f64) / ((b.count - prev_count) as f64);
                return Some(prev_le + frac * (b.le - prev_le));
            }
            prev_le = b.le;
            prev_count = b.count;
        }
        // Past every finite bucket — sample lives in `+Inf`.
        // Return the highest finite `le` as the best estimate
        // (caller can detect saturation via `count > buckets[-1].count`).
        self.buckets.last().map(|b| b.le)
    }
}

/// KDC2-1.12 — SLO histogram bucket schedule for the
/// `kdc2_router_decision_us` metric. Spans 100 µs → 50 ms so
/// the p50/p99 SLO checks have full resolution across the
/// observed range. Each unit is microseconds.
#[must_use]
pub fn kdc2_router_decision_us_buckets() -> Vec<f64> {
    vec![
        100.0, 250.0, 500.0, 1_000.0, 2_500.0, 5_000.0, 10_000.0, 25_000.0, 50_000.0,
    ]
}

/// KDC2-1.12 — construct the canonical
/// `kdc2_router_decision_us` histogram. Pre-populated with the
/// SLO bucket schedule.
#[must_use]
pub fn kdc2_router_decision_us() -> Histogram {
    Histogram::new(
        "kdc2_router_decision_us",
        "Mesh-router tick decision time in microseconds",
        &kdc2_router_decision_us_buckets(),
    )
}

/// Render a counter as Prometheus text-format.
fn render_counter(out: &mut String, c: &Counter) {
    let _ = writeln!(out, "# HELP {} {}", c.name, c.help);
    let _ = writeln!(out, "# TYPE {} counter", c.name);
    if c.labels.is_empty() {
        let _ = writeln!(out, "{} {}", c.name, c.value);
    } else {
        let labels = render_labels(&c.labels);
        let _ = writeln!(out, "{}{} {}", c.name, labels, c.value);
    }
}

/// Render a histogram (Prometheus text format requires `_bucket`,
/// `_sum`, and `_count` rows per series).
fn render_histogram(out: &mut String, h: &Histogram) {
    let _ = writeln!(out, "# HELP {} {}", h.name, h.help);
    let _ = writeln!(out, "# TYPE {} histogram", h.name);
    for b in &h.buckets {
        let _ = writeln!(out, r#"{}_bucket{{le="{}"}} {}"#, h.name, b.le, b.count);
    }
    let _ = writeln!(out, r#"{}_bucket{{le="+Inf"}} {}"#, h.name, h.count);
    let _ = writeln!(out, "{}_sum {}", h.name, h.sum);
    let _ = writeln!(out, "{}_count {}", h.name, h.count);
}

fn render_labels(labels: &BTreeMap<String, String>) -> String {
    let mut out = String::from("{");
    let mut first = true;
    for (k, v) in labels {
        if !first {
            out.push(',');
        }
        let _ = write!(out, r#"{k}="{}""#, escape_label_value(v));
        first = false;
    }
    out.push('}');
    out
}

fn escape_label_value(v: &str) -> String {
    v.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('"', "\\\"")
}

/// Write the canonical mackesd metrics file at
/// `<dir>/mackesd.prom`. Atomic: writes to a temp file first, then
/// renames into place so the collector never reads a half-written
/// snapshot.
///
/// # Errors
/// Returns `std::io::Error` if the directory isn't writable or the
/// rename fails.
pub fn write_textfile(
    dir: &Path,
    counters: &[Counter],
    histograms: &[Histogram],
) -> std::io::Result<PathBuf> {
    let final_path = dir.join("mackesd.prom");
    let tmp_path = dir.join("mackesd.prom.tmp");

    let mut body = String::new();
    for c in counters {
        render_counter(&mut body, c);
    }
    for h in histograms {
        render_histogram(&mut body, h);
    }

    let mut f = std::fs::File::create(&tmp_path)?;
    f.write_all(body.as_bytes())?;
    f.sync_data()?;
    std::fs::rename(&tmp_path, &final_path)?;
    Ok(final_path)
}

/// Default textfile collector directory per the 12.1.5 lock.
#[must_use]
pub fn default_textfile_dir() -> PathBuf {
    PathBuf::from("/var/lib/node_exporter/textfile_collector")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_counter_without_labels() {
        let c = Counter {
            name: "mackesd_apply_total",
            help: "Total applied revisions",
            value: 42,
            labels: BTreeMap::new(),
        };
        let mut out = String::new();
        render_counter(&mut out, &c);
        assert!(out.contains("# HELP mackesd_apply_total"));
        assert!(out.contains("# TYPE mackesd_apply_total counter"));
        assert!(out.contains("mackesd_apply_total 42"));
    }

    #[test]
    fn render_counter_with_labels() {
        let mut labels = BTreeMap::new();
        labels.insert("severity".to_owned(), "auto".to_owned());
        let c = Counter {
            name: "mackesd_drift_detected_total",
            help: "Drift events detected",
            value: 7,
            labels,
        };
        let mut out = String::new();
        render_counter(&mut out, &c);
        assert!(out.contains(r#"mackesd_drift_detected_total{severity="auto"} 7"#));
    }

    #[test]
    fn render_histogram_includes_inf_bucket() {
        let h = Histogram {
            name: "mackesd_probe_seconds",
            help: "Probe latency",
            buckets: vec![
                Bucket { le: 0.1, count: 5 },
                Bucket { le: 0.5, count: 10 },
                Bucket { le: 1.0, count: 12 },
            ],
            sum: 3.42,
            count: 14,
        };
        let mut out = String::new();
        render_histogram(&mut out, &h);
        assert!(out.contains(r#"mackesd_probe_seconds_bucket{le="0.1"} 5"#));
        assert!(out.contains(r#"mackesd_probe_seconds_bucket{le="+Inf"} 14"#));
        assert!(out.contains("mackesd_probe_seconds_sum 3.42"));
        assert!(out.contains("mackesd_probe_seconds_count 14"));
    }

    #[test]
    fn escape_label_value_handles_quotes_and_backslash() {
        assert_eq!(escape_label_value(r#"a"b\c"#), r#"a\"b\\c"#);
    }

    // ─────────────────────────────────────────────────────────
    // KDC2-1.12 — Histogram::observe + percentile_estimate +
    // kdc2_router_decision_us schedule
    // ─────────────────────────────────────────────────────────

    #[test]
    fn histogram_new_starts_at_zero() {
        let h = Histogram::new("x", "x", &[1.0, 2.0, 5.0]);
        assert_eq!(h.count, 0);
        assert_eq!(h.sum, 0.0);
        assert_eq!(h.buckets.len(), 3);
        for b in &h.buckets {
            assert_eq!(b.count, 0);
        }
    }

    #[test]
    fn histogram_observe_increments_correct_buckets() {
        let mut h = Histogram::new("x", "x", &[1.0, 2.0, 5.0]);
        h.observe(0.5);
        h.observe(1.5);
        h.observe(10.0); // past every finite bucket
                         // 0.5 ≤ 1.0 → all 3 buckets +1
                         // 1.5 ≤ 2.0 → buckets[1] + buckets[2] +1
                         // 10.0 past every finite bucket → no finite bucket bump
        assert_eq!(h.buckets[0].count, 1);
        assert_eq!(h.buckets[1].count, 2);
        assert_eq!(h.buckets[2].count, 2);
        assert_eq!(h.count, 3);
        assert!((h.sum - 12.0).abs() < 1e-9);
    }

    #[test]
    fn histogram_percentile_estimate_empty_returns_none() {
        let h = Histogram::new("x", "x", &[1.0, 2.0]);
        assert!(h.percentile_estimate(0.5).is_none());
    }

    #[test]
    fn histogram_percentile_estimate_p50_matches_median_bucket() {
        // 10 samples evenly spread → p50 falls in the middle
        // bucket.
        let mut h = Histogram::new("x", "x", &[100.0, 250.0, 500.0, 1_000.0]);
        for v in [
            50.0, 75.0, 120.0, 180.0, 240.0, 260.0, 300.0, 400.0, 600.0, 900.0,
        ] {
            h.observe(v);
        }
        let p50 = h.percentile_estimate(0.5).unwrap();
        // p50 of 10 samples = rank 5 → falls in the 250 bucket
        // (samples 1..=5: 50,75,120,180,240). 5th sample value
        // is 240. Linear interpolation across the 100→250 bucket
        // lands at ~205. We just lock the bucket bound.
        assert!(
            (100.0..=250.0).contains(&p50),
            "p50 estimate out of expected range: {p50}",
        );
    }

    #[test]
    fn kdc2_router_decision_us_buckets_span_100us_to_50ms() {
        let buckets = kdc2_router_decision_us_buckets();
        assert_eq!(*buckets.first().unwrap(), 100.0);
        assert_eq!(*buckets.last().unwrap(), 50_000.0);
        // Strictly ascending.
        for w in buckets.windows(2) {
            assert!(w[0] < w[1]);
        }
    }

    #[test]
    fn kdc2_router_decision_us_constructor_uses_canonical_name() {
        let h = kdc2_router_decision_us();
        assert_eq!(h.name, "kdc2_router_decision_us");
        assert_eq!(h.buckets.len(), kdc2_router_decision_us_buckets().len());
    }

    #[test]
    fn slo_check_1000_samples_p50_lt_5ms_p99_lt_25ms() {
        // KDC2-1.12 SLO: 1000-sample p50 < 5 ms, p99 < 25 ms.
        // Simulate router decisions tightly clustered around
        // 1 ms with a long-tail above. The histogram's
        // percentile_estimate must agree with the SLO.
        let mut h = kdc2_router_decision_us();
        // 950 samples in [200, 2000] µs — well under p99.
        for i in 0..950 {
            h.observe(200.0 + ((i % 1800) as f64));
        }
        // 50 samples in [5_000, 20_000] µs — heavy tail.
        for i in 0..50 {
            h.observe(5_000.0 + ((i * 300) as f64));
        }
        let p50 = h.percentile_estimate(0.50).unwrap();
        let p99 = h.percentile_estimate(0.99).unwrap();
        assert!(p50 < 5_000.0, "p50 {p50} µs ≥ 5 ms SLO");
        assert!(p99 < 25_000.0, "p99 {p99} µs ≥ 25 ms SLO");
    }

    #[test]
    fn write_textfile_creates_atomic_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let counters = vec![Counter {
            name: "x_total",
            help: "X",
            value: 1,
            labels: BTreeMap::new(),
        }];
        let path = write_textfile(dir.path(), &counters, &[]).unwrap();
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("x_total 1"));
        // No `.tmp` leftover.
        assert!(!dir.path().join("mackesd.prom.tmp").exists());
    }
}
