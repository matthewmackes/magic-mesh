//! DATACENTER-12 (scheduled-snapshot executor) — the missing consumer of the
//! Storage tab's "Save schedule".
//!
//! The Workbench Datacenter panel's Storage tab persists a scheduled-snapshot
//! config by publishing an `event/dc/snap-schedule/<sr>` record to the Bus
//! (the retired Workbench's `snap_schedule_save` originated the shape): `{ kind:
//! "snap-schedule", id, sr, retention, backup_target, dom0 }`. Until this worker,
//! NOTHING consumed that topic — the config was honest persistence with no
//! executor, so no snapshot was ever taken on a schedule and retention was never
//! enforced. This leader-gated periodic worker closes that gap: it reads the
//! latest schedule record per SR off the Bus, decides per-tick whether each SR is
//! **due** for a snapshot per its cadence, and when due reuses the EXISTING
//! storage snapshot path — `xe vdi-snapshot` over the mesh-key SSH through the
//! same injection-guarded, dom0-allow-listed contract `ipc::storage_ops` uses
//! (the `xen_ssh_key` / `xen_dom0s` resolvers + the same `ssh … xe …` shape) —
//! never re-implementing SSH or `xe`.
//!
//! Design (mirrors `dr_scheduler` + `dc_health`): the *brain* is a set of pure,
//! unit-tested helpers — [`due`] (cadence elapsed vs not), [`prune_targets`]
//! (which scheduler-made snapshots to destroy to keep N), and the record
//! (de)serialization ([`Schedule::parse`] / [`RunRecord::body`]) — and the worker
//! is thin best-effort I/O around them. It is **leader-gated** (the shared
//! `.mackesd-leader.lock`) so a multi-node mesh runs exactly one snapshot per SR
//! per interval, and it degrades cleanly (no panic) when there is no Bus, no
//! schedule config, or no dom0 — per §2.
//!
//! Retention safety: every scheduler-made snapshot is name-labelled with the
//! [`SNAP_LABEL_PREFIX`] prefix, and pruning only ever lists + destroys snapshots
//! carrying that prefix — an operator's hand-made snapshot is never touched.
//!
//! Run results land on `event/dc/snap-schedule-run/<sr>`
//! (`{ status: "ok"|"fail", sr, ts, snapshot?, detail? }`); the worker also reads
//! that lane back to recover the last-run timestamp across restarts, so a daemon
//! bounce doesn't re-snapshot every SR on the first tick. A failure additionally
//! drops an alert onto the `alert_relay` lane.

#![cfg(feature = "async-services")]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use mde_bus::persist::Persist;

use super::{ShutdownToken, Worker};

/// Loop cadence — wake every ~5 min and ask [`due`] which SRs are ready.
/// Decoupling the wake cadence from the (much longer) per-SR snapshot interval
/// keeps the worker responsive to shutdown while the cadence clock is coarse.
pub const TICK_INTERVAL: Duration = Duration::from_secs(300);

/// Default snapshot cadence when a schedule record carries no explicit
/// `interval_secs`/`cadence` — daily. The panel save (today) records retention +
/// target but not a cadence, so the executor must pick a sane honest default
/// rather than snapshot on every tick.
pub const DEFAULT_INTERVAL_SECS: u64 = 86_400;

/// Bus topic PREFIX the schedule config records live under (one per SR):
/// `event/dc/snap-schedule/<sr>`.
pub const SCHEDULE_PREFIX: &str = "event/dc/snap-schedule/";

/// Name-label prefix every scheduler-made snapshot carries. Retention pruning
/// only ever lists + destroys snapshots with this prefix, so an operator's
/// hand-made snapshot (any other label) is NEVER pruned by the scheduler.
pub const SNAP_LABEL_PREFIX: &str = "mcnf-sched-snap";

/// Max characters of a failure `detail` carried into a run record / alert. Keeps
/// the run lane compact.
pub const DETAIL_LEN: usize = 200;

/// Generous-but-finite overall bound for one SSH `xe …` invocation. The SSH args
/// already cap *connection* setup at 8 s (`ConnectTimeout`); this bounds the
/// WHOLE command so a connection that establishes then stalls mid-`xe` (a slow
/// `vdi-snapshot` on a large SR, a wedged dom0) is killed rather than blocking.
/// A snapshot of a large SR can legitimately take a couple of minutes. On expiry
/// the child is killed and the op degrades to a `fail` run record / a skipped
/// prune, exactly like a non-zero `xe` exit (mackesd-02: `run_pass` also runs off
/// the async runtime thread — see `run()` — so it can't starve the watchdog).
pub const SSH_XE_TIMEOUT: Duration = Duration::from_secs(300);

/// Bus topic the schedule config for `sr` is published to (the panel's write).
#[must_use]
pub fn schedule_topic(sr: &str) -> String {
    format!("{SCHEDULE_PREFIX}{sr}")
}

/// Bus topic a run result for `sr` is published to:
/// `event/dc/snap-schedule-run/<sr>`.
#[must_use]
pub fn run_topic(sr: &str) -> String {
    format!("event/dc/snap-schedule-run/{sr}")
}

/// First [`DETAIL_LEN`] characters of a string (char-boundary safe).
#[must_use]
fn detail_summary(detail: &str) -> String {
    detail.chars().take(DETAIL_LEN).collect()
}

// ---- pure record (de)serialization ----

/// One scheduled-snapshot config, parsed from an `event/dc/snap-schedule/<sr>`
/// record body. Mirrors the JSON the Workbench Storage tab's "Save schedule"
/// writes; an absent/zero `interval_secs` falls back to [`DEFAULT_INTERVAL_SECS`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Schedule {
    /// The SR uuid this schedule targets.
    pub sr: String,
    /// The dom0 (Xen host) the SR lives on — allow-list-checked before any SSH.
    pub dom0: String,
    /// How many scheduler-made snapshots to keep for this SR (≥1). Older ones are
    /// destroyed after each successful snapshot.
    pub retention: u64,
    /// Snapshot cadence in seconds (≥1). Defaults to [`DEFAULT_INTERVAL_SECS`].
    pub interval_secs: u64,
    /// Optional backup target (e.g. a remote SR uuid). Carried through to the run
    /// record for the operator; the snapshot itself is local.
    pub backup_target: String,
}

impl Schedule {
    /// Parse a schedule record body. Returns `None` for non-JSON, a record whose
    /// `kind` isn't `"snap-schedule"`, an empty `sr`, or a `retention` of 0 — all
    /// of which the worker skips rather than acting on garbage (§2 degrade).
    ///
    /// `interval_secs` is read from either an explicit `interval_secs` integer or
    /// a `cadence` integer; absent/zero falls back to [`DEFAULT_INTERVAL_SECS`].
    #[must_use]
    pub fn parse(body: &str) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_str(body).ok()?;
        if v.get("kind").and_then(serde_json::Value::as_str) != Some("snap-schedule") {
            return None;
        }
        let sr = v.get("sr").and_then(serde_json::Value::as_str)?.trim();
        if sr.is_empty() {
            return None;
        }
        let retention = v.get("retention").and_then(serde_json::Value::as_u64)?;
        if retention == 0 {
            return None;
        }
        let interval_secs = v
            .get("interval_secs")
            .and_then(serde_json::Value::as_u64)
            .or_else(|| v.get("cadence").and_then(serde_json::Value::as_u64))
            .filter(|&n| n > 0)
            .unwrap_or(DEFAULT_INTERVAL_SECS);
        let dom0 = v
            .get("dom0")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let backup_target = v
            .get("backup_target")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        Some(Self {
            sr: sr.to_string(),
            dom0,
            retention,
            interval_secs,
            backup_target,
        })
    }
}

/// One run result the executor decided to record for an SR.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RunRecord {
    /// The SR uuid this run was for.
    pub sr: String,
    /// `true` on a successful snapshot, `false` on failure.
    pub ok: bool,
    /// Unix seconds the run completed at.
    pub ts: u64,
    /// The new snapshot uuid on success (empty on failure).
    pub snapshot: String,
    /// A short failure detail on failure (empty on success).
    pub detail: String,
}

impl RunRecord {
    /// JSON body for the `event/dc/snap-schedule-run/<sr>` write.
    #[must_use]
    pub fn body(&self) -> String {
        serde_json::json!({
            "status": if self.ok { "ok" } else { "fail" },
            "sr": self.sr,
            "ts": self.ts,
            "snapshot": self.snapshot,
            "detail": self.detail,
        })
        .to_string()
    }

    /// Recover the last-run unix-seconds timestamp from a previously-written run
    /// record body (`ts` field). `None` for a non-JSON / fieldless body.
    #[must_use]
    pub fn last_ts_from_body(body: &str) -> Option<u64> {
        serde_json::from_str::<serde_json::Value>(body)
            .ok()?
            .get("ts")
            .and_then(serde_json::Value::as_u64)
    }
}

// ---- pure cadence + retention logic (unit-tested without xe) ----

/// Pure cadence decision: is a snapshot due now for an SR?
///
/// Returns `true` when the SR has never been snapshotted (`last_run_secs ==
/// None`) or when at least `interval` seconds have elapsed since the last run
/// (`now - last >= interval`). A `now` earlier than `last` (clock skew) is treated
/// as not-yet-due. Mirrors `dr_scheduler::due`.
#[must_use]
pub fn due(last_run_secs: Option<u64>, now_secs: u64, interval: u64) -> bool {
    match last_run_secs {
        None => true,
        Some(last) => now_secs.saturating_sub(last) >= interval,
    }
}

/// Pure retention selection: given the scheduler-made snapshots for an SR as
/// `(uuid, snapshot_time)` pairs (oldest-or-newest order irrelevant) and a
/// retention count `keep`, return the uuids of the snapshots to DESTROY — the
/// oldest beyond the newest `keep`. Stable: ties on time keep input order.
///
/// `keep == 0` is treated as `keep == 1` (the schedule guarantees ≥1, but the
/// helper is defensive so it never destroys a freshly-made snapshot). The caller
/// only ever feeds this its OWN (prefix-tagged) snapshots, so the result can
/// never name an operator's hand-made snapshot.
#[must_use]
pub fn prune_targets(snapshots: &[(String, i64)], keep: u64) -> Vec<String> {
    let keep = usize::try_from(keep.max(1)).unwrap_or(usize::MAX);
    if snapshots.len() <= keep {
        return Vec::new();
    }
    // Sort newest-first (descending time); stable so equal-time ties hold input
    // order. The first `keep` survive; the rest are destroyed.
    let mut idx: Vec<usize> = (0..snapshots.len()).collect();
    idx.sort_by(|&a, &b| snapshots[b].1.cmp(&snapshots[a].1));
    idx.into_iter()
        .skip(keep)
        .map(|i| snapshots[i].0.clone())
        .collect()
}

/// Parse the `xe snapshot-list … params=uuid,snapshot-time --minimal` output the
/// worker lists scheduler snapshots with. XAPI `--minimal` prints one record per
/// line as comma-joined param values in the requested order (`uuid,snapshot-time`);
/// the snapshot-time is an ISO-8601-ish `20260625T12:00:00Z`. Returns
/// `(uuid, epoch_secs)` pairs, skipping malformed lines (best-effort). Pure.
#[must_use]
pub fn parse_snapshot_list(stdout: &str) -> Vec<(String, i64)> {
    stdout
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let (uuid, ts) = line.split_once(',')?;
            let uuid = uuid.trim();
            if uuid.is_empty() {
                return None;
            }
            Some((uuid.to_string(), parse_xapi_time(ts.trim())))
        })
        .collect()
}

/// Parse XAPI's `snapshot-time` (`20260625T12:00:00Z` or `2026-06-25T12:00:00Z`)
/// to epoch seconds. Falls back to `0` for an unparseable value so such a snapshot
/// sorts oldest (it gets pruned first, never kept over a parseable one). Pure.
#[must_use]
fn parse_xapi_time(ts: &str) -> i64 {
    // XAPI's basic-format `YYYYMMDDTHH:MM:SSZ`.
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(ts, "%Y%m%dT%H:%M:%SZ") {
        return dt.and_utc().timestamp();
    }
    // Extended-format fallback `YYYY-MM-DDTHH:MM:SSZ`.
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
        return dt.timestamp();
    }
    0
}

/// Build the `xe vdi-snapshot` argument string for an SR's snapshot, validated +
/// labelled. PURE. Snapshots EVERY VDI on the SR (XAPI has no SR-level snapshot —
/// same loop `storage_ops::sr_snapshot_all_command` uses), tagging each new
/// snapshot with the [`SNAP_LABEL_PREFIX`]-prefixed name-label so retention can
/// recognise its own. Echoes the LAST new-snapshot uuid on stdout.
///
/// # Errors
/// Returns `Err` for an empty/invalid `sr_uuid` (the same injection guard the
/// storage RPCs use — `[0-9a-fA-F-]` only).
pub fn snapshot_command(sr_uuid: &str, label: &str) -> Result<String, String> {
    check_uuid("sr_uuid", sr_uuid)?;
    check_label(label)?;
    // For each VDI on the SR: snapshot it, then label the new snapshot so
    // retention recognises it. `vdi-snapshot` prints the new uuid; capture it,
    // set its name-label, and echo the last one for the run record.
    Ok(format!(
        "last=; for v in $(xe vdi-list sr-uuid={sr_uuid} params=uuid --minimal | tr , ' '); do \
         s=$(xe vdi-snapshot uuid=$v 2>/dev/null) && \
         xe vdi-param-set uuid=$s name-label={label} >/dev/null 2>&1 && last=$s; done; echo \"$last\""
    ))
}

/// Build the `xe vdi-list` argument string that lists THIS SR's scheduler-made
/// snapshot VDIs (those whose name-label carries [`SNAP_LABEL_PREFIX`]), printing
/// `uuid,snapshot-time` per line. PURE. Validated `sr_uuid`.
///
/// # Errors
/// Returns `Err` for an empty/invalid `sr_uuid`.
pub fn list_snapshots_command(sr_uuid: &str, label: &str) -> Result<String, String> {
    check_uuid("sr_uuid", sr_uuid)?;
    check_label(label)?;
    Ok(format!(
        "vdi-list sr-uuid={sr_uuid} name-label={label} params=uuid,snapshot-time --minimal"
    ))
}

/// Build the `xe vdi-destroy` argument string for one scheduler snapshot. PURE.
///
/// # Errors
/// Returns `Err` for an empty/invalid `uuid`.
pub fn destroy_command(uuid: &str) -> Result<String, String> {
    check_uuid("uuid", uuid)?;
    Ok(format!("vdi-destroy uuid={uuid}"))
}

/// A xen object uuid is a hex+dash string — the SAME command-injection guard
/// `ipc::storage_ops::check_uuid` applies before interpolating into a remote
/// `xe …` string. Returns `Err` for an empty value or any char outside
/// `[0-9a-fA-F-]`. Pure.
fn check_uuid(field: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("empty {field}"));
    }
    if !value.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err(format!("{field} contains invalid characters"));
    }
    Ok(())
}

/// The scheduler's own name-label must be `[A-Za-z0-9._-]` only — the SAME class
/// the storage module sanitizes name-labels to, since it is interpolated into the
/// remote `xe … name-label=<label>` string. Pure. (The label is worker-built, not
/// caller-supplied, but the guard keeps the command-builder injection-proof by
/// construction.)
fn check_label(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err("empty label".into());
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err("label contains invalid characters".into());
    }
    Ok(())
}

// ---- thin I/O: read schedules, run snapshots over SSH, write run records ----

/// Current unix-seconds wall clock (0 on a pre-epoch skew, never panics).
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Current unix-minute bucket — the alert-id dedupe granularity (mirrors
/// `etcd_watch::now_minute`).
fn now_minute() -> u64 {
    now_secs() / 60
}

/// The SSH-`xe` runner — mirrors `ipc::storage_ops::ssh_xe_status` EXACTLY (same
/// flags: identity, no host-key prompt, batch mode, 8 s connect timeout), reusing
/// the orchestrator's mesh-key resolver. The remote string is a full `xe …`
/// command (or a `for`-loop over `xe …`), already injection-guarded by the pure
/// command-builders above.
fn ssh_xe(key: &str, dom0: &str, remote: &str) -> std::io::Result<std::process::Output> {
    let mut cmd = std::process::Command::new("ssh");
    cmd.args([
        "-i",
        key,
        "-o",
        "StrictHostKeyChecking=no",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=8",
        &format!("root@{dom0}"),
        remote,
    ]);
    // Bound the whole command (not just connect setup) so a stalled `xe` is
    // killed at SSH_XE_TIMEOUT instead of blocking indefinitely (mackesd-02).
    super::proc::output_with_timeout(cmd, SSH_XE_TIMEOUT)
}

/// Drop one failure alert into the `alert_relay` watch dir (best-effort — a dir or
/// write failure is logged via the relay's own absence, never fatal). Mirrors
/// `etcd_watch::emit`.
fn emit_alert(alerts_dir: &std::path::Path, sr: &str, detail: &str) {
    if std::fs::create_dir_all(alerts_dir).is_err() {
        return;
    }
    let minute = now_minute();
    let id = format!("dc-snap-sched-fail-{sr}-{minute}");
    let alert = serde_json::json!({
        "id": id,
        "severity": "warn",
        "alert": "dc.snap_schedule.failed",
        "host": sr,
        "summary": format!("Scheduled snapshot of SR {sr} failed: {}", detail_summary(detail)),
    });
    let path = alerts_dir.join(format!("{id}.json"));
    let _ = std::fs::write(path, alert.to_string());
}

/// Write a run record to `event/dc/snap-schedule-run/<sr>` (best-effort — a Bus
/// write failure is logged, never fatal).
fn write_run(persist: &Persist, rec: &RunRecord) {
    if let Err(e) = persist.write(
        &run_topic(&rec.sr),
        mde_bus::hooks::config::Priority::Default,
        Some("snap-schedule-run"),
        Some(&rec.body()),
    ) {
        tracing::debug!(sr = %rec.sr, error = %e, "dc_snap_scheduler: run-record write failed");
    }
}

/// Read the latest schedule record per SR off the Bus. Walks every
/// `event/dc/snap-schedule/<sr>` topic, takes the LAST (newest-ulid) record on
/// each, and parses it; a topic with no parseable schedule is skipped. Returns a
/// map keyed by SR uuid. Best-effort: a failed list degrades to an empty map.
fn read_schedules(persist: &Persist) -> BTreeMap<String, Schedule> {
    let mut out = BTreeMap::new();
    let topics = match persist.list_topics() {
        Ok(t) => t,
        Err(e) => {
            tracing::debug!(error = %e, "dc_snap_scheduler: list_topics failed");
            return out;
        }
    };
    for topic in topics.iter().filter(|t| t.starts_with(SCHEDULE_PREFIX)) {
        let msgs = match persist.list_since(topic, None) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(topic = %topic, error = %e, "dc_snap_scheduler: list_since failed");
                continue;
            }
        };
        // Newest-ulid record wins (the operator's latest save for this SR).
        if let Some(sched) = msgs
            .iter()
            .rev()
            .find_map(|m| m.body.as_deref().and_then(Schedule::parse))
        {
            out.insert(sched.sr.clone(), sched);
        }
    }
    out
}

/// Recover the last-run unix-seconds per SR from the run lane, so a daemon restart
/// doesn't re-snapshot every SR on the first tick. Best-effort.
fn read_last_runs(persist: &Persist) -> BTreeMap<String, u64> {
    const RUN_PREFIX: &str = "event/dc/snap-schedule-run/";
    let mut out = BTreeMap::new();
    let Ok(topics) = persist.list_topics() else {
        return out;
    };
    for topic in topics.iter().filter(|t| t.starts_with(RUN_PREFIX)) {
        let Some(sr) = topic.strip_prefix(RUN_PREFIX) else {
            continue;
        };
        if let Ok(msgs) = persist.list_since(topic, None) {
            if let Some(ts) = msgs
                .iter()
                .rev()
                .find_map(|m| m.body.as_deref().and_then(RunRecord::last_ts_from_body))
            {
                out.insert(sr.to_string(), ts);
            }
        }
    }
    out
}

/// Take a snapshot of one SR over SSH and return the run record. The dom0 is
/// allow-list-checked (reusing the orchestrator's `xen_dom0s` set) BEFORE any SSH,
/// exactly as the storage RPCs are. A missing dom0 / a dom0 outside the allow-list
/// / a non-zero `xe` exit / a spawn failure each degrade to a `fail` record and
/// never panic.
fn run_snapshot(sched: &Schedule) -> RunRecord {
    let fail = |detail: String| RunRecord {
        sr: sched.sr.clone(),
        ok: false,
        ts: now_secs(),
        snapshot: String::new(),
        detail: detail_summary(&detail),
    };
    if sched.dom0.is_empty() {
        return fail("schedule has no dom0 to snapshot on".into());
    }
    // SECURITY: only ever SSH a dom0 in the orchestrator's configured allow-list.
    if !crate::workers::datacenter_orchestrator::xen_dom0s()
        .iter()
        .any(|d| d == &sched.dom0)
    {
        return fail(format!("dom0 {} not in allowed set", sched.dom0));
    }
    let cmd = match snapshot_command(&sched.sr, SNAP_LABEL_PREFIX) {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();
    let remote = format!("xe {cmd}");
    match ssh_xe(&key, &sched.dom0, &remote) {
        Ok(o) if o.status.success() => {
            let snapshot = String::from_utf8_lossy(&o.stdout).trim().to_string();
            RunRecord {
                sr: sched.sr.clone(),
                ok: true,
                ts: now_secs(),
                snapshot,
                detail: String::new(),
            }
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let code = o
                .status
                .code()
                .map_or_else(|| "signal".to_string(), |c| c.to_string());
            fail(format!("xe vdi-snapshot exit {code}: {}", stderr.trim()))
        }
        Err(e) => fail(format!("ssh failed: {e}")),
    }
}

/// Enforce retention for one SR over SSH: list THIS SR's scheduler-made snapshots
/// (prefix-tagged only), select the oldest beyond `keep` via the pure
/// [`prune_targets`], and `xe vdi-destroy` each. Best-effort — every step degrades
/// to a skip on error and never panics. Returns the count destroyed (for logging).
fn enforce_retention(sched: &Schedule) -> usize {
    let key = crate::workers::datacenter_orchestrator::xen_ssh_key();
    let Ok(list_cmd) = list_snapshots_command(&sched.sr, SNAP_LABEL_PREFIX) else {
        return 0;
    };
    let stdout = match ssh_xe(&key, &sched.dom0, &format!("xe {list_cmd}")) {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => return 0,
    };
    let snaps = parse_snapshot_list(&stdout);
    let targets = prune_targets(&snaps, sched.retention);
    let mut destroyed = 0;
    for uuid in targets {
        let Ok(destroy) = destroy_command(&uuid) else {
            continue;
        };
        if let Ok(o) = ssh_xe(&key, &sched.dom0, &format!("xe {destroy}")) {
            if o.status.success() {
                destroyed += 1;
            }
        }
    }
    destroyed
}

/// One scheduler pass: read schedules + last-runs, and for each SR that is due,
/// snapshot it, enforce retention, write the run record, and alert on failure.
/// Best-effort throughout — one SR's failure never aborts the others.
fn run_pass(persist: &Persist, alerts_dir: &std::path::Path) {
    let schedules = read_schedules(persist);
    if schedules.is_empty() {
        return;
    }
    let last_runs = read_last_runs(persist);
    let now = now_secs();
    for sched in schedules.values() {
        let last = last_runs.get(&sched.sr).copied();
        if !due(last, now, sched.interval_secs) {
            continue;
        }
        let rec = run_snapshot(sched);
        if rec.ok {
            let n = enforce_retention(sched);
            tracing::info!(sr = %sched.sr, snapshot = %rec.snapshot, pruned = n,
                "dc_snap_scheduler: snapshot taken + retention enforced");
        } else {
            tracing::warn!(sr = %sched.sr, detail = %rec.detail, "dc_snap_scheduler: snapshot failed");
            emit_alert(alerts_dir, &sched.sr, &rec.detail);
        }
        write_run(persist, &rec);
    }
}

fn default_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
}

/// The supervised worker. Leader-gated (only the elected node snapshots +
/// publishes, so a multi-node mesh runs one snapshot per SR per interval) and
/// best-effort.
pub struct DcSnapSchedulerWorker {
    tick_interval: Duration,
    node_id: String,
    leader_lock: PathBuf,
    alerts_dir: PathBuf,
    bus_root_override: Option<PathBuf>,
}

impl DcSnapSchedulerWorker {
    /// Construct with production defaults (5 min tick, the shared leader lock
    /// under `workgroup_root`, the default Bus root + alert dir).
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: String, alerts_dir: PathBuf) -> Self {
        Self {
            tick_interval: TICK_INTERVAL,
            leader_lock: workgroup_root.join(".mackesd-leader.lock"),
            node_id,
            alerts_dir,
            bus_root_override: None,
        }
    }

    /// Override the Bus root directory. Used in tests.
    #[must_use]
    pub fn with_bus_root(mut self, p: PathBuf) -> Self {
        self.bus_root_override = Some(p);
        self
    }

    /// Only the directory leader runs the snapshots (no-fixed-center: any eligible
    /// node can be it, the elected one runs + publishes). Reuses the shared leader
    /// lock.
    fn is_leader(&self) -> bool {
        crate::leader_gate::LeaderGate::from_lock_path(
            self.leader_lock.clone(),
            self.node_id.clone(),
        )
        .is_leader()
    }
}

#[async_trait::async_trait]
impl Worker for DcSnapSchedulerWorker {
    fn name(&self) -> &'static str {
        "dc_snap_scheduler"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let bus_root = match self.bus_root_override.clone().or_else(default_bus_root) {
            Some(r) => r,
            None => {
                tracing::debug!("dc_snap_scheduler: no bus root; worker idle");
                return Ok(());
            }
        };
        // Validate the bus root once (same "worker idle on a persistent open
        // failure" behavior as before). Per-tick work reopens its own `Persist`
        // inside `spawn_blocking` — `Persist` is `!Sync` so it can't cross the
        // blocking await, and `Persist::open` is cheap (mackesd-02).
        if let Err(e) = Persist::open(bus_root.clone()) {
            tracing::debug!(error = %e, "dc_snap_scheduler: persist open failed; worker idle");
            return Ok(());
        }
        loop {
            if self.is_leader() {
                // run_pass shells `ssh … xe vdi-snapshot/vdi-destroy` serially
                // across every due SR — potentially minutes of blocking work.
                // Run it OFF the runtime thread so it can neither pin a worker nor
                // starve the watchdog beat (mackesd-02 / WATCHDOG-2).
                let bus_root_tick = bus_root.clone();
                let alerts_dir = self.alerts_dir.clone();
                if let Err(e) = tokio::task::spawn_blocking(move || {
                    match Persist::open(bus_root_tick) {
                        Ok(persist) => run_pass(&persist, &alerts_dir),
                        Err(e) => {
                            tracing::debug!(error = %e, "dc_snap_scheduler: tick persist open failed; skipping");
                        }
                    }
                })
                .await
                {
                    tracing::warn!(error = %e, "dc_snap_scheduler: snapshot pass task join failed");
                }
            }
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

    // ---- topics ----

    #[test]
    fn topics_format_under_the_dc_lanes() {
        assert_eq!(schedule_topic("sr-1"), "event/dc/snap-schedule/sr-1");
        assert_eq!(run_topic("sr-1"), "event/dc/snap-schedule-run/sr-1");
    }

    // ---- mackesd-02: bounded SSH `xe` invocation ----

    #[test]
    fn ssh_xe_timeout_is_generous_but_finite() {
        // A snapshot of a large SR legitimately needs a couple of minutes, so the
        // bound must be large — but finite so a stalled `xe` can't block a thread
        // forever (mackesd-02). It must also exceed the 8 s SSH connect cap.
        assert!(SSH_XE_TIMEOUT >= Duration::from_secs(60));
        assert!(SSH_XE_TIMEOUT > Duration::from_secs(8));
    }

    // ---- due-decision logic (cadence elapsed vs not) ----

    #[test]
    fn due_is_true_when_never_run() {
        assert!(due(None, 0, 86_400));
        assert!(due(None, 1_000_000, 86_400));
    }

    #[test]
    fn due_is_false_before_the_interval_elapses() {
        // Ran at t=1000, now t=1001, interval=daily → not yet due.
        assert!(!due(Some(1000), 1001, 86_400));
        // One second short of the interval → still not due.
        assert!(!due(Some(1000), 1000 + 86_399, 86_400));
    }

    #[test]
    fn due_is_true_once_the_interval_has_elapsed() {
        // Exactly the interval → due.
        assert!(due(Some(1000), 1000 + 86_400, 86_400));
        // Well past → due.
        assert!(due(Some(1000), 1000 + 200_000, 86_400));
    }

    #[test]
    fn due_handles_clock_skew_as_not_due() {
        // now earlier than last → saturating delta 0 → not due.
        assert!(!due(Some(5000), 1000, 86_400));
    }

    // ---- retention-prune selection ----

    #[test]
    fn prune_keeps_n_and_destroys_the_oldest_beyond_it() {
        // 5 scheduler snapshots, keep 2 → the 3 oldest are destroyed, the 2
        // newest survive. Times deliberately out of input order.
        let snaps = vec![
            ("a".to_string(), 100),
            ("b".to_string(), 500), // newest
            ("c".to_string(), 200),
            ("d".to_string(), 400), // 2nd newest
            ("e".to_string(), 300),
        ];
        let mut destroyed = prune_targets(&snaps, 2);
        destroyed.sort();
        // Newest two (b=500, d=400) survive; a,c,e are destroyed.
        assert_eq!(
            destroyed,
            vec!["a".to_string(), "c".to_string(), "e".to_string()]
        );
    }

    #[test]
    fn prune_keeps_everything_when_at_or_below_retention() {
        let snaps = vec![("a".to_string(), 100), ("b".to_string(), 200)];
        // Exactly the retention count → nothing to destroy.
        assert!(prune_targets(&snaps, 2).is_empty());
        // Fewer than retention → nothing to destroy.
        assert!(prune_targets(&snaps, 5).is_empty());
        // Empty input → nothing to destroy.
        assert!(prune_targets(&[], 3).is_empty());
    }

    #[test]
    fn prune_keep_one_destroys_all_but_the_newest() {
        let snaps = vec![
            ("old".to_string(), 100),
            ("new".to_string(), 300),
            ("mid".to_string(), 200),
        ];
        let destroyed = prune_targets(&snaps, 1);
        // Only the single newest ("new") survives.
        assert_eq!(destroyed.len(), 2);
        assert!(destroyed.contains(&"old".to_string()));
        assert!(destroyed.contains(&"mid".to_string()));
        assert!(!destroyed.contains(&"new".to_string()));
    }

    #[test]
    fn prune_keep_zero_is_treated_as_keep_one() {
        // Defensive: keep=0 must never destroy a freshly-made snapshot — it
        // behaves as keep=1.
        let snaps = vec![("a".to_string(), 100), ("b".to_string(), 200)];
        let destroyed = prune_targets(&snaps, 0);
        assert_eq!(destroyed, vec!["a".to_string()]); // only the newest (b) kept
    }

    #[test]
    fn prune_only_ever_names_inputs_it_was_given() {
        // The worker only feeds prune_targets its OWN prefix-tagged snapshots, so
        // every returned uuid is one of the inputs — never an operator snapshot.
        let snaps = vec![
            ("s1".to_string(), 1),
            ("s2".to_string(), 2),
            ("s3".to_string(), 3),
        ];
        for uuid in prune_targets(&snaps, 1) {
            assert!(["s1", "s2", "s3"].contains(&uuid.as_str()));
        }
    }

    // ---- snapshot-list parsing (feeds the retention selection) ----

    #[test]
    fn parse_snapshot_list_reads_uuid_and_time() {
        let out = "aaaa-1,20260625T12:00:00Z\nbbbb-2,20260624T12:00:00Z\n";
        let parsed = parse_snapshot_list(out);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].0, "aaaa-1");
        assert_eq!(parsed[1].0, "bbbb-2");
        // The 25th sorts newer than the 24th.
        assert!(parsed[0].1 > parsed[1].1);
    }

    #[test]
    fn parse_snapshot_list_skips_garbage_and_blank_lines() {
        let out = "\ngood-1,20260625T12:00:00Z\nno-comma-line\n,emptyuuid\n";
        let parsed = parse_snapshot_list(out);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, "good-1");
    }

    #[test]
    fn parse_snapshot_list_tolerates_extended_time_and_unparseable() {
        // Extended RFC3339 form parses; a junk time falls back to 0 (sorts oldest).
        let out = "ext-1,2026-06-25T12:00:00+00:00\njunk-1,not-a-time\n";
        let parsed = parse_snapshot_list(out);
        assert_eq!(parsed.len(), 2);
        assert!(parsed[0].1 > 0);
        assert_eq!(parsed[1].1, 0);
        // Pruning to keep 1 destroys the junk-time (oldest) one.
        let destroyed = prune_targets(&parsed, 1);
        assert_eq!(destroyed, vec!["junk-1".to_string()]);
    }

    // ---- schedule record (de)serialization ----

    #[test]
    fn schedule_parses_the_panel_record_shape() {
        // The exact JSON the Workbench Storage tab's snap_schedule_save writes.
        let body = r#"{"kind":"snap-schedule","id":"sr-1","sr":"sr-1","retention":3,"backup_target":"remote-sr","dom0":"172.20.0.9"}"#;
        let s = Schedule::parse(body).expect("valid schedule parses");
        assert_eq!(s.sr, "sr-1");
        assert_eq!(s.dom0, "172.20.0.9");
        assert_eq!(s.retention, 3);
        assert_eq!(s.backup_target, "remote-sr");
        // No cadence in the panel record → daily default.
        assert_eq!(s.interval_secs, DEFAULT_INTERVAL_SECS);
    }

    #[test]
    fn schedule_reads_an_explicit_cadence_when_present() {
        let body =
            r#"{"kind":"snap-schedule","sr":"sr-2","retention":2,"interval_secs":3600,"dom0":"h"}"#;
        let s = Schedule::parse(body).unwrap();
        assert_eq!(s.interval_secs, 3600);
        // The `cadence` alias is honored too.
        let body2 =
            r#"{"kind":"snap-schedule","sr":"sr-3","retention":1,"cadence":7200,"dom0":"h"}"#;
        assert_eq!(Schedule::parse(body2).unwrap().interval_secs, 7200);
    }

    #[test]
    fn schedule_rejects_garbage_and_invalid_records() {
        // Non-JSON.
        assert!(Schedule::parse("not json").is_none());
        // Wrong kind.
        assert!(Schedule::parse(r#"{"kind":"other","sr":"s","retention":1}"#).is_none());
        // Missing / empty sr.
        assert!(Schedule::parse(r#"{"kind":"snap-schedule","retention":1}"#).is_none());
        assert!(Schedule::parse(r#"{"kind":"snap-schedule","sr":"","retention":1}"#).is_none());
        // Zero / missing retention.
        assert!(Schedule::parse(r#"{"kind":"snap-schedule","sr":"s","retention":0}"#).is_none());
        assert!(Schedule::parse(r#"{"kind":"snap-schedule","sr":"s"}"#).is_none());
    }

    #[test]
    fn run_record_body_round_trips_status_and_fields() {
        let ok = RunRecord {
            sr: "sr-1".into(),
            ok: true,
            ts: 1_700_000_000,
            snapshot: "snap-9".into(),
            detail: String::new(),
        };
        let v: serde_json::Value = serde_json::from_str(&ok.body()).unwrap();
        assert_eq!(v["status"], "ok");
        assert_eq!(v["sr"], "sr-1");
        assert_eq!(v["ts"], 1_700_000_000_u64);
        assert_eq!(v["snapshot"], "snap-9");
        // The ts is recoverable for last-run tracking across restarts.
        assert_eq!(
            RunRecord::last_ts_from_body(&ok.body()),
            Some(1_700_000_000)
        );

        let fail = RunRecord {
            sr: "sr-2".into(),
            ok: false,
            ts: 1_700_000_500,
            snapshot: String::new(),
            detail: "ssh failed: timeout".into(),
        };
        let v: serde_json::Value = serde_json::from_str(&fail.body()).unwrap();
        assert_eq!(v["status"], "fail");
        assert_eq!(v["detail"], "ssh failed: timeout");
    }

    #[test]
    fn last_ts_from_body_is_none_for_garbage() {
        assert_eq!(RunRecord::last_ts_from_body("not json"), None);
        assert_eq!(RunRecord::last_ts_from_body(r#"{"status":"ok"}"#), None);
    }

    // ---- command builders reuse the storage injection guard ----

    #[test]
    fn snapshot_command_labels_with_the_scheduler_prefix() {
        let c = snapshot_command("5ab1-c0de", SNAP_LABEL_PREFIX).unwrap();
        assert!(c.contains("xe vdi-list sr-uuid=5ab1-c0de"));
        assert!(c.contains("xe vdi-snapshot uuid=$v"));
        assert!(c.contains(&format!("name-label={SNAP_LABEL_PREFIX}")));
    }

    #[test]
    fn command_builders_reject_injection() {
        // Same `[0-9a-fA-F-]` uuid guard the storage RPCs use.
        assert!(snapshot_command("sr;rm -rf /", SNAP_LABEL_PREFIX).is_err());
        assert!(snapshot_command("sr$(x)", SNAP_LABEL_PREFIX).is_err());
        assert!(snapshot_command("", SNAP_LABEL_PREFIX).is_err());
        assert!(list_snapshots_command("sr`x`", SNAP_LABEL_PREFIX).is_err());
        assert!(destroy_command("uuid;evil").is_err());
        // A label with shell metacharacters is rejected too.
        assert!(snapshot_command("5ab1-c0de", "bad;label").is_err());
    }

    #[test]
    fn list_and_destroy_commands_have_the_expected_shape() {
        let list = list_snapshots_command("5ab1-c0de", SNAP_LABEL_PREFIX).unwrap();
        assert_eq!(
            list,
            format!(
                "vdi-list sr-uuid=5ab1-c0de name-label={SNAP_LABEL_PREFIX} params=uuid,snapshot-time --minimal"
            )
        );
        // The uuid guard is hex+dash only (the storage RPCs' class), so the
        // fixture uses a hex uuid.
        assert_eq!(
            destroy_command("5ab1-c0de").unwrap(),
            "vdi-destroy uuid=5ab1-c0de"
        );
    }
}
