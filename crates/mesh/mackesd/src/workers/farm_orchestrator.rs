//! FARM-AUTO-1 — mackesd farm-orchestrator worker.
//!
//! The mackesd-native brain for the build farm: it bridges the farm's job
//! lifecycle (the etcd work-queue + results that FARM-AUTO-3 maintains) onto the
//! **Bus**, publishing `event/farm/<jobid>` events as jobs are queued and finish.
//! That makes farm activity first-class mesh state — visible to the Notification
//! Hub and the Workbench Build panel (FARM-AUTO-5) like any other event, with no
//! AI in the loop.
//!
//! Design: the *brain* ([`FarmOrchestrator`]) is a pure, deduped state machine
//! (emit an event only on a phase transition); the worker is thin I/O around it —
//! read the etcd queue/results over the HTTP gateway, emit via `mde-bus publish`.
//! Reading the shared etcd store (not local files) is what lets a mackesd worker
//! on any node reflect farm state the control host produced — no-fixed-center.

#![cfg(feature = "async-services")]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use super::{ShutdownToken, Worker};

/// Sweep cadence — 10 s (farm jobs are coarse vs the 5 s firewall sweeps).
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(10);

/// etcd HTTP-gateway base (the FARM-AUTO-3 queue store). Overridable for tests/CI.
fn etcd_base() -> String {
    std::env::var("MCNF_ETCD").unwrap_or_else(|_| "http://172.20.145.192:2379".to_string())
}

/// A farm job's lifecycle phase as the orchestrator has last published it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JobPhase {
    /// Job is in the queue, build not yet finished.
    Queued,
    /// Build finished (with an outcome).
    Done,
}

/// One Bus event the orchestrator decided to emit.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FarmEvent {
    pub jobid: String,
    pub phase: JobPhase,
    /// pass/fail for `Done`; `None` for `Queued`.
    pub outcome: Option<String>,
}

impl FarmEvent {
    /// The Bus topic + JSON body for this event (the `mde-bus publish` payload).
    #[must_use]
    pub fn topic(&self) -> String {
        format!("event/farm/{}", self.jobid)
    }
    #[must_use]
    pub fn body(&self) -> String {
        match (&self.phase, &self.outcome) {
            (JobPhase::Queued, _) => {
                format!(r#"{{"jobid":"{}","phase":"queued"}}"#, self.jobid)
            }
            (JobPhase::Done, Some(o)) => {
                format!(
                    r#"{{"jobid":"{}","phase":"done","outcome":"{o}","alert":{}}}"#,
                    self.jobid,
                    o == "fail"
                )
            }
            (JobPhase::Done, None) => {
                format!(r#"{{"jobid":"{}","phase":"done"}}"#, self.jobid)
            }
        }
    }
}

/// Pure orchestration core. Tracks the last phase published per job and, given the
/// current queue + completed results, returns ONLY the new transitions to emit —
/// so the Bus never sees a duplicate `queued`/`done` for the same job.
#[derive(Default)]
pub struct FarmOrchestrator {
    published: BTreeMap<String, JobPhase>,
}

impl FarmOrchestrator {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reconcile against the current world: `queued` = job ids in the queue,
    /// `done` = `(jobid, outcome)` finished results. Returns the events to emit
    /// (queued-on-first-sight, done-on-completion) and advances internal state.
    pub fn reconcile(&mut self, queued: &[String], done: &[(String, String)]) -> Vec<FarmEvent> {
        let mut events = Vec::new();
        for jid in queued {
            if !self.published.contains_key(jid) {
                self.published.insert(jid.clone(), JobPhase::Queued);
                events.push(FarmEvent {
                    jobid: jid.clone(),
                    phase: JobPhase::Queued,
                    outcome: None,
                });
            }
        }
        for (jid, outcome) in done {
            if self.published.get(jid) != Some(&JobPhase::Done) {
                self.published.insert(jid.clone(), JobPhase::Done);
                events.push(FarmEvent {
                    jobid: jid.clone(),
                    phase: JobPhase::Done,
                    outcome: Some(outcome.clone()),
                });
            }
        }
        events
    }

    /// Forget jobs no longer present anywhere (queue drained + result reaped), so
    /// `published` doesn't grow unbounded across the daemon's lifetime.
    pub fn forget_absent(&mut self, present: &std::collections::BTreeSet<String>) {
        self.published.retain(|k, _| present.contains(k));
    }
}

// ---- thin I/O: read the etcd queue/results, emit via the Bus ----

/// Decode an etcd v3 range response, returning the job ids under a `/farm/<sub>/`
/// prefix (the key suffix after the prefix). Pure — fed the raw HTTP body.
pub fn parse_jobids(range_json: &str, prefix: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(range_json) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(kvs) = v.get("kvs").and_then(|k| k.as_array()) {
        for kv in kvs {
            if let Some(k64) = kv.get("key").and_then(|k| k.as_str()) {
                if let Ok(bytes) = base64_decode(k64) {
                    if let Ok(key) = String::from_utf8(bytes) {
                        if let Some(id) = key.strip_prefix(prefix) {
                            out.push(id.to_string());
                        }
                    }
                }
            }
        }
    }
    out
}

/// Minimal base64 decode (etcd keys/values are std-alphabet, padded).
fn base64_decode(s: &str) -> Result<Vec<u8>, ()> {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut rev = [255u8; 256];
    for (i, &c) in T.iter().enumerate() {
        rev[c as usize] = i as u8;
    }
    let s = s.trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0u8;
    for &c in s {
        let val = rev[c as usize];
        if val == 255 {
            return Err(());
        }
        buf = (buf << 6) | u32::from(val);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Ok(out)
}

/// The supervised worker. Leader-gated (only the elected node orchestrates, so a
/// 3-node mesh doesn't triple-publish) and best-effort: a missing etcd is a no-op.
pub struct FarmOrchestratorWorker {
    core: FarmOrchestrator,
    tick_interval: Duration,
    node_id: String,
    leader_lock: PathBuf,
}

impl FarmOrchestratorWorker {
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: String) -> Self {
        Self {
            core: FarmOrchestrator::new(),
            tick_interval: DEFAULT_TICK_INTERVAL,
            leader_lock: workgroup_root.join(".mackesd-leader.lock"),
            node_id,
        }
    }

    /// Only the directory leader orchestrates (no-fixed-center: any node can be it,
    /// the elected one coordinates). Reuses the same leader lock other workers do,
    /// so a 3-node mesh publishes each farm event once.
    fn is_leader(&self) -> bool {
        crate::leader_gate::LeaderGate::from_lock_path(
            self.leader_lock.clone(),
            self.node_id.clone(),
        )
        .is_leader()
    }

    fn etcd_range_keys(prefix: &str) -> Vec<String> {
        let base = etcd_base();
        // range_end = prefix with last byte +1 (etcd "prefix" convention).
        let mut end = prefix.as_bytes().to_vec();
        if let Some(last) = end.last_mut() {
            *last += 1;
        }
        let body = format!(
            r#"{{"key":"{}","range_end":"{}","keys_only":true}}"#,
            b64(prefix.as_bytes()),
            b64(&end)
        );
        let out = std::process::Command::new("curl")
            .args([
                "-s",
                "-X",
                "POST",
                &format!("{base}/v3/kv/range"),
                "-d",
                &body,
            ])
            .output();
        match out {
            Ok(o) if o.status.success() => {
                parse_jobids(&String::from_utf8_lossy(&o.stdout), prefix)
            }
            _ => Vec::new(),
        }
    }

    fn etcd_get(key: &str) -> Option<String> {
        let base = etcd_base();
        let body = format!(r#"{{"key":"{}"}}"#, b64(key.as_bytes()));
        let o = std::process::Command::new("curl")
            .args([
                "-s",
                "-X",
                "POST",
                &format!("{base}/v3/kv/range"),
                "-d",
                &body,
            ])
            .output()
            .ok()?;
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).ok()?;
        let val64 = v.get("kvs")?.as_array()?.first()?.get("value")?.as_str()?;
        String::from_utf8(base64_decode(val64).ok()?).ok()
    }

    fn tick_once(&mut self) {
        if !self.is_leader() {
            return;
        }
        let queued = Self::etcd_range_keys("/farm/queue/");
        let result_ids = Self::etcd_range_keys("/farm/result/");
        let done: Vec<(String, String)> = result_ids
            .iter()
            .map(|id| {
                let outcome = Self::etcd_get(&format!("/farm/result/{id}"))
                    .and_then(|j| {
                        serde_json::from_str::<serde_json::Value>(&j)
                            .ok()
                            .and_then(|v| {
                                v.get("outcome").and_then(|o| o.as_str()).map(String::from)
                            })
                    })
                    .unwrap_or_else(|| "unknown".to_string());
                (id.clone(), outcome)
            })
            .collect();

        for ev in self.core.reconcile(&queued, &done) {
            publish(&ev);
        }
    }
}

fn b64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18 & 63) as usize] as char);
        out.push(T[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Emit a farm event onto the Bus in-process (perf-10 / arch-6) — no fork+exec
/// of the `mde-bus` CLI per event. Byte-identical stored row to the old
/// `mde-bus publish <topic> --body-flag <body>`. Targets
/// [`crate::bus_publish::default_bus_root`] (honours `MDE_BUS_ROOT`).
fn publish(ev: &FarmEvent) {
    publish_to(crate::bus_publish::default_bus_root().as_deref(), ev);
}

/// Root-injectable body of [`publish`] — fresh-opens the Bus at `bus_root` and
/// writes the event in-process (mirrors the CLI's per-call open). Best-effort;
/// tests pass a temp root.
fn publish_to(bus_root: Option<&std::path::Path>, ev: &FarmEvent) {
    if let Some(mut persist) =
        crate::bus_publish::open_bus(bus_root.map(std::path::Path::to_path_buf))
    {
        crate::bus_publish::publish_body(&mut persist, &ev.topic(), &ev.body());
    }
}

#[async_trait::async_trait]
impl Worker for FarmOrchestratorWorker {
    fn name(&self) -> &'static str {
        "farm_orchestrator"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            self.tick_once();
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(self.tick_interval) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn reconcile_emits_queued_once_then_done_once() {
        let mut o = FarmOrchestrator::new();
        // First sight of a queued job → one queued event.
        let e = o.reconcile(&["a".into(), "b".into()], &[]);
        assert_eq!(e.len(), 2);
        assert!(e.iter().all(|x| x.phase == JobPhase::Queued));
        // Same queue again → no duplicate events.
        assert!(o.reconcile(&["a".into(), "b".into()], &[]).is_empty());
        // `a` finishes → exactly one done event with the outcome.
        let e = o.reconcile(&["b".into()], &[("a".into(), "pass".into())]);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].phase, JobPhase::Done);
        assert_eq!(e[0].outcome.as_deref(), Some("pass"));
        // `a` done again → no re-emit.
        assert!(o.reconcile(&[], &[("a".into(), "pass".into())]).is_empty());
    }

    #[test]
    fn done_event_flags_failures_for_the_hub() {
        let ev = FarmEvent {
            jobid: "j1".into(),
            phase: JobPhase::Done,
            outcome: Some("fail".into()),
        };
        assert!(ev.body().contains(r#""alert":true"#));
        assert_eq!(ev.topic(), "event/farm/j1");
        let ok = FarmEvent {
            jobid: "j2".into(),
            phase: JobPhase::Done,
            outcome: Some("pass".into()),
        };
        assert!(ok.body().contains(r#""alert":false"#));
    }

    #[test]
    fn forget_absent_bounds_the_map() {
        let mut o = FarmOrchestrator::new();
        o.reconcile(&["a".into(), "b".into()], &[]);
        let present: BTreeSet<String> = ["a".to_string()].into_iter().collect();
        o.forget_absent(&present);
        // `b` forgotten → seen as new again.
        let e = o.reconcile(&["b".into()], &[]);
        assert_eq!(e.len(), 1);
    }

    #[test]
    fn parse_jobids_strips_the_prefix() {
        // etcd range body with two keys under /farm/queue/ (base64 of the keys).
        let k1 = b64(b"/farm/queue/jobAAA");
        let k2 = b64(b"/farm/queue/jobBBB");
        let json = format!(r#"{{"kvs":[{{"key":"{k1}"}},{{"key":"{k2}"}}]}}"#);
        let ids = parse_jobids(&json, "/farm/queue/");
        assert_eq!(ids, vec!["jobAAA".to_string(), "jobBBB".to_string()]);
    }

    #[test]
    fn base64_roundtrips() {
        for s in ["", "f", "fo", "foo", "/farm/queue/x", "hello world!"] {
            assert_eq!(base64_decode(&b64(s.as_bytes())).unwrap(), s.as_bytes());
        }
    }
}
